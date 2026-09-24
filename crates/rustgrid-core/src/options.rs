use serde::{Deserialize, Serialize};

/// The TLS/SSL behaviour for a connection, mirroring the MySQL client's SSL modes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    /// Never use TLS.
    #[default]
    Disabled,
    /// Use TLS when the server supports it.
    Preferred,
    /// Require TLS, but do not verify the server certificate.
    Required,
    /// Require TLS and verify the server certificate against the CA.
    VerifyCa,
    /// Require TLS, verify the certificate and that the host name matches.
    VerifyIdentity,
}

impl TlsMode {
    /// The label key for this mode (resolved through `t!` in the UI).
    pub fn label_key(self) -> &'static str {
        match self {
            TlsMode::Disabled => "form.tls.mode.disabled",
            TlsMode::Preferred => "form.tls.mode.preferred",
            TlsMode::Required => "form.tls.mode.required",
            TlsMode::VerifyCa => "form.tls.mode.verify_ca",
            TlsMode::VerifyIdentity => "form.tls.mode.verify_identity",
        }
    }

    /// Every mode, in the order the dropdown shows them.
    pub fn all() -> [TlsMode; 5] {
        [
            TlsMode::Disabled,
            TlsMode::Preferred,
            TlsMode::Required,
            TlsMode::VerifyCa,
            TlsMode::VerifyIdentity,
        ]
    }
}

/// One hop of the tunnel/proxy chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelKind {
    /// An SSH server with local port forwarding (`direct-tcpip`).
    Ssh,
    /// A SOCKS5 proxy.
    Socks5,
    /// An HTTP tunnel (`CONNECT`).
    Http,
}

impl TunnelKind {
    pub fn label_key(self) -> &'static str {
        match self {
            TunnelKind::Ssh => "form.tunnel.ssh",
            TunnelKind::Socks5 => "form.tunnel.socks5",
            TunnelKind::Http => "form.tunnel.http",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            TunnelKind::Ssh => 22,
            TunnelKind::Socks5 => 1080,
            TunnelKind::Http => 8080,
        }
    }
}

/// One layer of a tunnel chain: the client connects to the first layer, which forwards to the
/// second, and so on, with the last forwarding to the database server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelLayer {
    pub kind: TunnelKind,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// Private key path for an SSH layer using [`TunnelAuth::KeyFile`].
    #[serde(default)]
    pub key_path: String,
    /// How an SSH layer authenticates (ignored by the proxy kinds).
    #[serde(default)]
    pub auth: TunnelAuth,
}

/// How an SSH tunnel layer authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelAuth {
    /// Username + password.
    #[default]
    Password,
    /// A private key file on disk.
    KeyFile,
}

impl TunnelAuth {
    pub fn label_key(self) -> &'static str {
        match self {
            TunnelAuth::Password => "form.tunnel.auth.password",
            TunnelAuth::KeyFile => "form.tunnel.auth.key",
        }
    }

    pub fn all() -> [TunnelAuth; 2] {
        [TunnelAuth::Password, TunnelAuth::KeyFile]
    }
}

impl TunnelLayer {
    pub fn new(kind: TunnelKind) -> Self {
        Self {
            kind,
            host: String::new(),
            port: kind.default_port(),
            username: String::new(),
            password: String::new(),
            key_path: String::new(),
            auth: TunnelAuth::default(),
        }
    }
}

/// Engine-agnostic connection settings edited in the New/Edit Connection window and interpreted by
/// each driver's `connect`. Everything here is optional; the defaults reproduce the plain
/// (no TLS, no tunnel, no timeouts) connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionOptions {
    /// Ask MySQL for the cleartext-password authentication plugin (`mysql_clear_password`).
    #[serde(default)]
    pub cleartext_password: bool,
    #[serde(default)]
    pub tls: TlsOptions,
    /// The tunnel/proxy chain, outermost hop first. Empty means a direct connection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tunnel: Vec<TunnelLayer>,
    /// Connection timeout in seconds (how long to wait for the server to accept a connection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout: Option<u64>,
    /// Per-statement query timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_timeout: Option<u64>,
    /// Keep-alive interval in seconds (how long an idle pooled connection is kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keepalive: Option<u64>,
    /// Open the session read-only (`SET SESSION transaction_read_only = 1`).
    #[serde(default)]
    pub read_only: bool,
    /// The databases to show in the tree; empty means every database.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visible_databases: Vec<String>,
    /// SQL run on every new pooled connection (session initialization).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub init_sql: String,
    /// A free-form environment label (本地 / 开发 / 生产), shown in the header and list.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment: String,
}

/// The TLS/SSL part of [`ConnectionOptions`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsOptions {
    #[serde(default)]
    pub mode: TlsMode,
    /// Path to a CA certificate (PEM) used to verify the server.
    #[serde(default)]
    pub ca: String,
    /// Path to the client certificate (PEM) for mutual TLS.
    #[serde(default)]
    pub cert: String,
    /// Path to the client private key (PEM) for mutual TLS.
    #[serde(default)]
    pub key: String,
}

impl TlsOptions {
    /// Whether any TLS setting is configured (so the form can decide whether to send them).
    pub fn is_configured(&self) -> bool {
        self.mode != TlsMode::Disabled
            || !self.ca.is_empty()
            || !self.cert.is_empty()
            || !self.key.is_empty()
    }
}

impl ConnectionOptions {
    /// Whether this is the plain default (so it can be omitted from the profile JSON).
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
