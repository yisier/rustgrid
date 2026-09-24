//! Local port forwarding through a chain of tunnel/proxy layers.
//!
//! The driver dials the database through a tunnel by starting a listener on `127.0.0.1:<port>` and
//! pointing the connection at it. Each accepted connection is forwarded through the configured
//! chain: the client connects to the first layer, which forwards to the second, and so on, with the
//! last layer forwarding to the real database server.
//!
//! The SSH layer is a real SSH client (password or private-key authentication) using
//! `direct-tcpip`; the proxy/HTTP layers speak an HTTP `CONNECT` handshake.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rustgrid_core::{TunnelAuth, TunnelKind, TunnelLayer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// A boxed, bidirectional byte stream (a TCP socket, an SSH channel, or a proxied socket).
trait ByteStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> ByteStream for T {}
type Stream = Box<dyn ByteStream>;

/// A running tunnel: a local listener plus the task that accepts and forwards connections. Dropping
/// it stops the listener, so it must be kept alive for as long as the database connection is.
pub struct Tunnel {
    local_port: u16,
    task: JoinHandle<()>,
}

impl Tunnel {
    /// Start forwarding `127.0.0.1:<ephemeral>` to `target` through `layers` (outermost first).
    pub async fn start(
        layers: Vec<TunnelLayer>,
        target_host: String,
        target_port: u16,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| format!("could not start the tunnel listener: {error}"))?;
        let local_port = listener
            .local_addr()
            .map_err(|error| format!("could not read the tunnel port: {error}"))?
            .port();

        let task = tokio::spawn(async move {
            loop {
                let Ok((mut client, _peer)) = listener.accept().await else {
                    break;
                };
                let layers = layers.clone();
                let target_host = target_host.clone();
                tokio::spawn(async move {
                    if let Ok(mut remote) = open_stream(&layers, (&target_host, target_port)).await
                    {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut remote).await;
                    }
                });
            }
        });

        Ok(Self { local_port, task })
    }

    /// The local port the connection should point at.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Dial the first layer, then hand the socket off through every layer until it reaches `target`.
async fn open_stream(layers: &[TunnelLayer], target: (&str, u16)) -> Result<Stream, String> {
    let first = &layers[0];
    let tcp = TcpStream::connect((first.host.as_str(), first.port))
        .await
        .map_err(|error| {
            format!(
                "could not reach the tunnel server {}:{}: {error}",
                first.host, first.port
            )
        })?;
    let _ = tcp.set_nodelay(true);

    let mut stream: Stream = Box::new(tcp);
    for (index, layer) in layers.iter().enumerate() {
        let next = layers
            .get(index + 1)
            .map(|next| (next.host.as_str(), next.port))
            .unwrap_or(target);
        stream = match layer.kind {
            TunnelKind::Ssh => ssh_handshake(stream, layer, next).await?,
            TunnelKind::Socks5 => socks5_connect(stream, layer, next).await?,
            TunnelKind::Http => http_connect(stream, layer, next).await?,
        };
    }
    Ok(stream)
}

/// The russh client handler. It accepts any host key; the connection is still encrypted, but the
/// server identity is not pinned.
struct Client;

impl russh::client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// A stream produced by an SSH `direct-tcpip` channel. It keeps the session handle alive alongside
/// the channel stream, because dropping the handle would tear the session (and the channel) down.
struct SshStream {
    stream: russh::ChannelStream<russh::client::Msg>,
    _session: russh::client::Handle<Client>,
}

impl AsyncRead for SshStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for SshStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// Authenticate an SSH session over `stream` and open a `direct-tcpip` channel to `target`.
async fn ssh_handshake(
    stream: Stream,
    layer: &TunnelLayer,
    target: (&str, u16),
) -> Result<Stream, String> {
    let config = Arc::new(russh::client::Config {
        inactivity_timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    let mut session = russh::client::connect_stream(config, stream, Client)
        .await
        .map_err(|error| format!("SSH handshake failed: {error}"))?;

    let auth = if layer.auth == TunnelAuth::KeyFile {
        let key = russh::keys::load_secret_key(layer.key_path.trim(), None)
            .map_err(|error| format!("could not read the SSH key {}: {error}", layer.key_path))?;
        let hash = session
            .best_supported_rsa_hash()
            .await
            .map_err(|error| format!("SSH negotiation failed: {error}"))?
            .flatten();
        session
            .authenticate_publickey(
                layer.username.clone(),
                russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash),
            )
            .await
    } else {
        session
            .authenticate_password(layer.username.clone(), layer.password.clone())
            .await
    }
    .map_err(|error| format!("SSH authentication failed: {error}"))?;

    if !auth.success() {
        return Err("SSH authentication was rejected".to_string());
    }

    let channel = session
        .channel_open_direct_tcpip(target.0.to_string(), u32::from(target.1), "127.0.0.1", 0)
        .await
        .map_err(|error| {
            format!(
                "SSH could not forward to {}:{}: {error}",
                target.0, target.1
            )
        })?;

    Ok(Box::new(SshStream {
        stream: channel.into_stream(),
        _session: session,
    }))
}

/// Perform an HTTP `CONNECT` handshake through an HTTP proxy / tunnel.
async fn http_connect(
    mut stream: Stream,
    layer: &TunnelLayer,
    target: (&str, u16),
) -> Result<Stream, String> {
    let authority = format!("{}:{}", target.0, target.1);
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if !layer.username.is_empty() {
        let credentials = BASE64.encode(format!("{}:{}", layer.username, layer.password));
        request.push_str(&format!("Proxy-Authorization: Basic {credentials}\r\n"));
    }
    request.push_str("\r\n");

    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| format!("could not send the proxy request: {error}"))?;
    let _ = stream.flush().await;

    // Read the response head (up to the blank line). The proxy may send extra bytes afterwards that
    // belong to the tunnelled stream, so read one byte at a time and stop exactly at the CRLFCRLF.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = stream
            .read(&mut byte)
            .await
            .map_err(|error| format!("could not read the proxy response: {error}"))?;
        if read == 0 {
            return Err("the proxy closed the connection".to_string());
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() > 16 * 1024 {
            return Err("the proxy response header is too large".to_string());
        }
    }

    let status = String::from_utf8_lossy(&head);
    let status_line = status.lines().next().unwrap_or_default();
    if !status_line.contains(" 200") {
        return Err(format!("the proxy refused the connection: {status_line}"));
    }

    Ok(stream)
}

/// Perform a SOCKS5 handshake (no-auth or username/password) and CONNECT to `target`.
async fn socks5_connect(
    mut stream: Stream,
    layer: &TunnelLayer,
    target: (&str, u16),
) -> Result<Stream, String> {
    let use_auth = !layer.username.is_empty();

    // Method negotiation: offer no-auth, plus username/password when credentials are set.
    let greeting: &[u8] = if use_auth {
        &[0x05, 0x02, 0x00, 0x02]
    } else {
        &[0x05, 0x01, 0x00]
    };
    stream
        .write_all(greeting)
        .await
        .map_err(|error| format!("SOCKS5: could not send the greeting: {error}"))?;

    let mut method = [0u8; 2];
    stream
        .read_exact(&mut method)
        .await
        .map_err(|error| format!("SOCKS5: could not read the greeting reply: {error}"))?;
    if method[0] != 0x05 {
        return Err("SOCKS5: unexpected protocol version".to_string());
    }
    match method[1] {
        0x00 => {}
        0x02 => {
            let user = layer.username.as_bytes();
            let password = layer.password.as_bytes();
            if user.len() > 255 || password.len() > 255 {
                return Err("SOCKS5: credentials are too long".to_string());
            }
            let mut auth = vec![0x01, user.len() as u8];
            auth.extend_from_slice(user);
            auth.push(password.len() as u8);
            auth.extend_from_slice(password);
            stream
                .write_all(&auth)
                .await
                .map_err(|error| format!("SOCKS5: could not send credentials: {error}"))?;
            let mut reply = [0u8; 2];
            stream
                .read_exact(&mut reply)
                .await
                .map_err(|error| format!("SOCKS5: could not read the auth reply: {error}"))?;
            if reply[1] != 0x00 {
                return Err("SOCKS5: authentication failed".to_string());
            }
        }
        other => {
            return Err(format!("SOCKS5: unsupported auth method {other:#04x}"));
        }
    }

    // CONNECT with a domain name.
    let host = target.0.as_bytes();
    if host.len() > 255 {
        return Err("SOCKS5: host name is too long".to_string());
    }
    let mut request = vec![0x05, 0x01, 0x00, 0x03, host.len() as u8];
    request.extend_from_slice(host);
    request.push((target.1 >> 8) as u8);
    request.push((target.1 & 0xff) as u8);
    stream
        .write_all(&request)
        .await
        .map_err(|error| format!("SOCKS5: could not send the connect request: {error}"))?;

    let mut head = [0u8; 4];
    stream
        .read_exact(&mut head)
        .await
        .map_err(|error| format!("SOCKS5: could not read the connect reply: {error}"))?;
    if head[0] != 0x05 {
        return Err("SOCKS5: unexpected reply version".to_string());
    }
    if head[1] != 0x00 {
        return Err(format!(
            "SOCKS5: the proxy refused the connection (code {})",
            head[1]
        ));
    }

    // Skip the bound address that follows the reply header.
    let skip = match head[3] {
        0x01 => 4 + 2,
        0x04 => 16 + 2,
        0x03 => {
            let mut length = [0u8; 1];
            stream
                .read_exact(&mut length)
                .await
                .map_err(|error| format!("SOCKS5: could not read the bound address: {error}"))?;
            usize::from(length[0]) + 2
        }
        other => return Err(format!("SOCKS5: unexpected address type {other:#04x}")),
    };
    let mut discard = vec![0u8; skip];
    stream
        .read_exact(&mut discard)
        .await
        .map_err(|error| format!("SOCKS5: could not read the bound address: {error}"))?;

    Ok(stream)
}
