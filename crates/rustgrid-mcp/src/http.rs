use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use rustgrid_config::ConfigStore;
use rustgrid_core::DriverRegistry;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use crate::config::{McpConfig, load_mcp_config, save_mcp_config};
use crate::process_request;
use crate::protocol::*;

/// Generates a detailed, ready-to-use instruction manual and system prompt for AI agents.
pub fn generate_agent_prompt(port: u16, token: &str, allow_lan: bool) -> String {
    let auth_header = if token.is_empty() {
        String::new()
    } else {
        format!("  -H \"Authorization: Bearer {token}\" \\\n")
    };
    let host_label = if allow_lan { "0.0.0.0" } else { "127.0.0.1" };
    let sse_url = if token.is_empty() {
        format!("http://{host_label}:{port}/sse")
    } else {
        format!("http://{host_label}:{port}/sse?token={token}")
    };

    format!(
        r#"# RustGrid AI / MCP Database Server

Welcome! You are connected to the **RustGrid MCP Database Server**, a secure local database management interface.
This server allows you to explore database schemas, inspect table structures, sample data, and safely execute SQL queries on user-authorized database connections.

---

## 1. Server Connection Information
- **Base URL**: `http://{host_label}:{port}`
- **Auth Token**: `{token}`
- **Protocol**: Model Context Protocol (MCP) & JSON-RPC 2.0
- **Supported Endpoints**:
  - `POST /mcp` (or `POST /`): Direct JSON-RPC 2.0 tool execution (requires `Authorization: Bearer {token}`)
  - `GET /sse`: Server-Sent Events stream for standard MCP SSE clients (`{sse_url}`)
  - `POST /message`: Message submission endpoint for SSE sessions
  - `GET /`: Server health check and instructions

---

## 2. Client Setup Configurations

### A. Cursor (`.cursor/mcp.json`)
```json
{{
  "mcpServers": {{
    "rustgrid": {{
      "url": "{sse_url}"
    }}
  }}
}}
```

### B. Direct HTTP JSON-RPC (cURL / Python / Any Agent)
```bash
curl -X POST http://{host_label}:{port}/mcp \
{auth_header}  -H "Content-Type: application/json" \
  -d '{{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {{
      "name": "list_connections",
      "arguments": {{}}
    }}
  }}'
```

---

## 3. Available MCP Tools Reference

### 1. `list_connections`
Lists all authorized database connections available to the AI.
- **Parameters**: None
- **Returns**: Markdown list of connection names, engine types (MySQL, PostgreSQL, SQLite, Oracle, etc.), access modes (Read-Only / Read-Write), and context prompts.

### 2. `get_schema`
Discovers all database names and table/view names for a chosen connection.
- **Parameters**:
  - `connection` (string, required): Connection name or ID.
  - `database` (string, optional): Database name to scope schema inspection.
- **Returns**: Markdown hierarchy of databases and their tables/views.

### 3. `describe_table`
Retrieves detailed table schema, including columns, data types, primary keys, nullability, auto-increment flags, and comments.
- **Parameters**:
  - `connection` (string, required): Connection name or ID.
  - `table` (string, required): Target table name.
  - `database` (string, optional): Target database or schema name.

### 4. `sample_data`
Retrieves representative rows (default 3 rows) to help you understand column values, date formats, and data patterns before writing queries.
- **Parameters**:
  - `connection` (string, required): Connection name or ID.
  - `table` (string, required): Target table name.
  - `database` (string, optional): Target database or schema name.
  - `limit` (integer, optional, default: 3): Number of sample rows to fetch.

### 5. `execute_query`
Executes SQL queries against an authorized database connection.
- **Parameters**:
  - `connection` (string, required): Connection name or ID.
  - `sql` (string, required): The SQL query string.
  - `database` (string, optional): Target database name.
  - `max_rows` (integer, optional): Maximum rows to return. **Pass `max_rows: 0` for unlimited rows (no cap)**. Defaults to user's system setting (50).
- **Safety**: By default, only read-only statements (`SELECT`, `SHOW`, `EXPLAIN`, `DESCRIBE`) are permitted unless Read-Write mode is explicitly granted by the user.

---

## 4. Best Practices for AI Agents
1. **Always inspect first**: Call `list_connections` to discover what databases are accessible.
2. **Explore before querying**: Use `describe_table` and `sample_data` to verify column names and formatting. Avoid guessing column names.
3. **Be specific with columns**: Prefer `SELECT id, name, created_at` over `SELECT *` where possible to keep response sizes optimal.
4. **Unlimited Rows**: If you need complete dataset analysis, pass `"max_rows": 0` in `execute_query`.
"#,
        port = port,
        token = token,
        auth_header = auth_header,
        sse_url = sse_url,
        host_label = host_label
    )
}

/// Runs the MCP HTTP / SSE server in background.
pub async fn start_http_server(
    config: ConfigStore,
    registry: Arc<DriverRegistry>,
    active_port: Arc<AtomicU16>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> anyhow::Result<()> {
    let mut mcp_cfg = load_mcp_config(config.root());

    // Port persistence: attempt desired port; if 0 or busy, bind ephemeral port and persist it
    let bind_host = if mcp_cfg.allow_lan {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    let listener = if mcp_cfg.port > 0 {
        match TcpListener::bind(format!("{bind_host}:{}", mcp_cfg.port)).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(
                    "Failed to bind configured port {}: {}. Falling back to random port.",
                    mcp_cfg.port,
                    e
                );
                TcpListener::bind(format!("{bind_host}:0")).await?
            }
        }
    } else {
        TcpListener::bind(format!("{bind_host}:0")).await?
    };

    let local_port = listener.local_addr()?.port();
    active_port.store(local_port, Ordering::SeqCst);

    // If port was 0 or reassigned, persist the assigned port so it stays fixed for future launches
    if mcp_cfg.port != local_port {
        mcp_cfg.port = local_port;
        let _ = save_mcp_config(config.root(), &mcp_cfg);
    }

    eprintln!("[rustgrid-mcp] HTTP/SSE server listening on http://127.0.0.1:{local_port}");

    loop {
        tokio::select! {
            accept_res = listener.accept() => {
                match accept_res {
                    Ok((stream, _)) => {
                        let cfg = config.clone();
                        let reg = registry.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_http_connection(stream, cfg, reg, local_port).await {
                                eprintln!("[rustgrid-mcp] HTTP connection error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("[rustgrid-mcp] Accept error: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                eprintln!("[rustgrid-mcp] Shutting down HTTP server.");
                break;
            }
        }
    }

    active_port.store(0, Ordering::SeqCst);
    Ok(())
}

#[derive(Clone, Default)]
pub struct McpServerManager {
    active_port: Arc<AtomicU16>,
    shutdown_tx: Option<broadcast::Sender<()>>,
}

impl McpServerManager {
    pub fn new() -> Self {
        Self {
            active_port: Arc::new(AtomicU16::new(0)),
            shutdown_tx: None,
        }
    }

    pub fn active_port(&self) -> u16 {
        self.active_port.load(Ordering::SeqCst)
    }

    pub fn active_port_handle(&self) -> Arc<AtomicU16> {
        self.active_port.clone()
    }

    pub fn restart(&mut self, config: ConfigStore, registry: Arc<DriverRegistry>) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let mcp_cfg = load_mcp_config(config.root());
        if !mcp_cfg.enabled {
            self.active_port.store(0, Ordering::SeqCst);
            return;
        }
        let (tx, rx) = broadcast::channel(1);
        self.shutdown_tx = Some(tx);
        let active_port = self.active_port.clone();
        let fut = async move {
            let _ = start_http_server(config, registry, active_port, rx).await;
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(fut);
        } else {
            let _ = std::thread::Builder::new()
                .name("rustgrid-mcp-http".to_string())
                .spawn(move || {
                    if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        rt.block_on(fut);
                    }
                });
        }
    }
}

fn is_origin_allowed(origin: &str, allow_lan: bool) -> bool {
    let o = origin.trim().to_lowercase();
    if o.starts_with("http://localhost")
        || o.starts_with("http://127.0.0.1")
        || o.starts_with("vscode-webview://")
        || o.starts_with("http://[::1]")
    {
        return true;
    }
    if allow_lan {
        if let Some(host) = o
            .strip_prefix("http://")
            .or_else(|| o.strip_prefix("https://"))
        {
            let host_part = host.split(':').next().unwrap_or(host);
            if host_part.starts_with("192.168.")
                || host_part.starts_with("10.")
                || (host_part.starts_with("172.") && is_172_private(host_part))
                || host_part.ends_with(".local")
            {
                return true;
            }
        }
    }
    false
}

fn is_172_private(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4 && parts[0] == "172" {
        if let Ok(second) = parts[1].parse::<u8>() {
            return (16..=31).contains(&second);
        }
    }
    false
}

pub fn is_host_allowed(host: Option<&str>, allow_lan: bool) -> bool {
    let host_val = match host {
        Some(h) if !h.trim().is_empty() => h.trim().to_lowercase(),
        _ => return true,
    };
    let host_part = host_val.split(':').next().unwrap_or(&host_val);
    if host_part == "localhost" || host_part == "127.0.0.1" || host_part == "[::1]" {
        return true;
    }
    if allow_lan {
        if host_part.starts_with("192.168.")
            || host_part.starts_with("10.")
            || (host_part.starts_with("172.") && is_172_private(host_part))
            || host_part.ends_with(".local")
        {
            return true;
        }
    }
    false
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.len() != b_bytes.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a_bytes.iter().zip(b_bytes.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Handles a single TCP connection as an HTTP 1.1 request.
async fn handle_http_connection(
    mut stream: TcpStream,
    config: ConfigStore,
    registry: Arc<DriverRegistry>,
    port: u16,
) -> anyhow::Result<()> {
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    buf.truncate(n);

    let (headers_part, body_part) = match find_subsequence(&buf, b"\r\n\r\n") {
        Some(pos) => (&buf[..pos], &buf[pos + 4..]),
        None => (&buf[..], &[][..]),
    };

    let header_str = String::from_utf8_lossy(headers_part);
    let mut lines = header_str.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");

    let mut content_length: usize = 0;
    let mut origin_header: Option<String> = None;
    let mut auth_header: Option<String> = None;
    let mut host_header: Option<String> = None;

    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_lowercase();
            let val = v.trim();
            if key == "content-length" {
                content_length = val.parse().unwrap_or(0);
            } else if key == "origin" {
                origin_header = Some(val.to_string());
            } else if key == "authorization" {
                auth_header = Some(val.to_string());
            } else if key == "host" {
                host_header = Some(val.to_string());
            }
        }
    }

    // Read full body if content-length requires more bytes than already received
    let mut full_body = body_part.to_vec();
    while full_body.len() < content_length {
        let mut temp = vec![0u8; (content_length - full_body.len()).min(8192)];
        let read_bytes = stream.read(&mut temp).await?;
        if read_bytes == 0 {
            break;
        }
        full_body.extend_from_slice(&temp[..read_bytes]);
    }

    let mcp_cfg = load_mcp_config(config.root());
    let response = process_http_route(
        method,
        path,
        &full_body,
        origin_header.as_deref(),
        auth_header.as_deref(),
        host_header.as_deref(),
        &config,
        &mcp_cfg,
        &registry,
        port,
    )
    .await;

    match response {
        HttpResponse::Standard {
            status,
            content_type,
            body,
            cors_allow_origin,
        } => {
            let cors_header = if let Some(ref origin) = cors_allow_origin {
                format!(
                    "Access-Control-Allow-Origin: {origin}\r\n\
                     Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
                     Access-Control-Allow-Headers: Authorization, Content-Type\r\n"
                )
            } else {
                String::new()
            };

            let res_header = format!(
                "HTTP/1.1 {status}\r\n\
                 Content-Type: {content_type}\r\n\
                 Content-Length: {}\r\n\
                 {cors_header}\
                 Connection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(res_header.as_bytes()).await?;
            stream.write_all(&body).await?;
            stream.flush().await?;
        }
        HttpResponse::SseStream { cors_allow_origin } => {
            let cors_header = if let Some(ref origin) = cors_allow_origin {
                format!("Access-Control-Allow-Origin: {origin}\r\n")
            } else {
                String::new()
            };

            let sse_header = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/event-stream\r\n\
                 Cache-Control: no-cache\r\n\
                 Connection: keep-alive\r\n\
                 {cors_header}\r\n"
            );
            stream.write_all(sse_header.as_bytes()).await?;
            let init_event = "event: endpoint\r\ndata: /message\r\n\r\n";
            stream.write_all(init_event.as_bytes()).await?;
            stream.flush().await?;

            // Keep connection alive with periodic pings until client disconnects
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if stream.write_all(b": ping\r\n\r\n").await.is_err() {
                    break;
                }
            }
        }
    }

    Ok(())
}

pub enum HttpResponse {
    Standard {
        status: &'static str,
        content_type: &'static str,
        body: Vec<u8>,
        cors_allow_origin: Option<String>,
    },
    SseStream {
        cors_allow_origin: Option<String>,
    },
}

pub async fn process_http_route(
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
    auth: Option<&str>,
    host: Option<&str>,
    config: &ConfigStore,
    mcp_cfg: &McpConfig,
    registry: &Arc<DriverRegistry>,
    port: u16,
) -> HttpResponse {
    // DNS Rebinding protection: validate Host header
    if !is_host_allowed(host, mcp_cfg.allow_lan) {
        return HttpResponse::Standard {
            status: "400 Bad Request",
            content_type: "text/plain",
            body: b"Bad Request: Invalid or forbidden Host header".to_vec(),
            cors_allow_origin: None,
        };
    }

    let mut parts = path.splitn(2, '?');
    let clean_path = parts.next().unwrap_or(path);
    let query_string = parts.next().unwrap_or("");

    // P1-3: Restrict CORS. If an external origin is supplied (e.g. https://evil.example), reject with 403 Forbidden.
    let cors_allow_origin = if let Some(orig) = origin {
        if !is_origin_allowed(orig, mcp_cfg.allow_lan) {
            return HttpResponse::Standard {
                status: "403 Forbidden",
                content_type: "text/plain",
                body: b"Forbidden: Cross-origin request rejected".to_vec(),
                cors_allow_origin: None,
            };
        }
        Some(orig.to_string())
    } else {
        None
    };

    // 1. CORS Preflight
    if method == "OPTIONS" {
        return HttpResponse::Standard {
            status: "204 No Content",
            content_type: "text/plain",
            body: Vec::new(),
            cors_allow_origin,
        };
    }

    // 2. Health & Prompt documentation (open for user viewing)
    // Never expose the secret API token over unauthenticated GET /; use a placeholder instead.
    if (clean_path == "/" || clean_path == "/help" || clean_path == "/prompt") && method == "GET" {
        let prompt = generate_agent_prompt(port, "<YOUR_BEARER_TOKEN>", mcp_cfg.allow_lan);
        return HttpResponse::Standard {
            status: "200 OK",
            content_type: "text/markdown; charset=utf-8",
            body: prompt.into_bytes(),
            cors_allow_origin,
        };
    }

    // P1-3: Bearer token validation for API endpoints (/mcp, /sse, /message)
    if !mcp_cfg.api_token.is_empty() {
        let mut authorized = false;
        if let Some(auth_val) = auth {
            if let Some(token) = auth_val.strip_prefix("Bearer ") {
                if constant_time_eq(token.trim(), &mcp_cfg.api_token) {
                    authorized = true;
                }
            }
        }
        // Only allow ?token= query parameter for SSE stream (/sse)
        if !authorized && clean_path == "/sse" && !query_string.is_empty() {
            for param in query_string.split('&') {
                if let Some((k, v)) = param.split_once('=') {
                    if k == "token" && constant_time_eq(v, &mcp_cfg.api_token) {
                        authorized = true;
                        break;
                    }
                }
            }
        }

        if !authorized {
            return HttpResponse::Standard {
                status: "401 Unauthorized",
                content_type: "application/json",
                body: json!({
                    "error": "Unauthorized: Invalid or missing Bearer token in Authorization header."
                })
                .to_string()
                .into_bytes(),
                cors_allow_origin,
            };
        }
    }

    // 3. MCP SSE Stream
    if clean_path == "/sse" && method == "GET" {
        return HttpResponse::SseStream { cors_allow_origin };
    }

    // 4. JSON-RPC Direct POST
    if (clean_path == "/mcp" || clean_path == "/" || clean_path == "/message") && method == "POST" {
        let req: JsonRpcRequest = match serde_json::from_slice(body) {
            Ok(r) => r,
            Err(e) => {
                let err_resp = JsonRpcResponse::error(
                    None,
                    JsonRpcError::parse_error(format!("Invalid JSON: {e}")),
                );
                return HttpResponse::Standard {
                    status: "200 OK",
                    content_type: "application/json",
                    body: serde_json::to_vec(&err_resp).unwrap_or_default(),
                    cors_allow_origin,
                };
            }
        };

        if req.id.is_none() {
            // Notification: acknowledge without content
            return HttpResponse::Standard {
                status: "204 No Content",
                content_type: "application/json",
                body: Vec::new(),
                cors_allow_origin,
            };
        }

        let resp = process_request(config, mcp_cfg, registry, req).await;
        let resp_bytes = serde_json::to_vec(&resp).unwrap_or_default();
        return HttpResponse::Standard {
            status: "200 OK",
            content_type: "application/json",
            body: resp_bytes,
            cors_allow_origin,
        };
    }

    // 5. 404 Not Found
    HttpResponse::Standard {
        status: "404 Not Found",
        content_type: "text/plain",
        body: b"404 Not Found".to_vec(),
        cors_allow_origin,
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_agent_prompt() {
        let prompt = generate_agent_prompt(8080, "test_token_123", false);
        assert!(prompt.contains("http://127.0.0.1:8080"));
        assert!(prompt.contains("test_token_123"));
        assert!(prompt.contains("list_connections"));
        assert!(prompt.contains("execute_query"));
        assert!(prompt.contains("max_rows: 0"));

        let lan_prompt = generate_agent_prompt(8080, "test_token_123", true);
        assert!(lan_prompt.contains("http://0.0.0.0:8080"));
    }

    #[test]
    fn test_is_origin_allowed() {
        assert!(is_origin_allowed("http://localhost:3000", false));
        assert!(is_origin_allowed("http://127.0.0.1:8080", false));
        assert!(is_origin_allowed("vscode-webview://my-view", false));
        assert!(!is_origin_allowed("http://192.168.1.50:3000", false));
        assert!(is_origin_allowed("http://192.168.1.50:3000", true));
        assert!(is_origin_allowed("http://10.0.0.5:8080", true));
        assert!(is_origin_allowed("http://172.20.10.2:3000", true));
        assert!(!is_origin_allowed("https://evil.example.com", false));
        assert!(!is_origin_allowed("https://evil.example.com", true));
        assert!(!is_origin_allowed("http://malicious.org", true));
    }

    #[tokio::test]
    async fn test_process_http_routes() {
        let config = ConfigStore::new().unwrap();
        let registry = Arc::new(DriverRegistry::new());

        let mcp_cfg = McpConfig {
            api_token: "secret123".to_string(),
            ..Default::default()
        };

        // 0. DNS Rebinding: Invalid Host blocked
        let bad_host_res = process_http_route(
            "GET",
            "/",
            b"",
            None,
            None,
            Some("evil.example:9000"),
            &config,
            &mcp_cfg,
            &registry,
            9000,
        )
        .await;
        match bad_host_res {
            HttpResponse::Standard { status, .. } => assert_eq!(status, "400 Bad Request"),
            _ => panic!("Expected standard response"),
        }

        // 1. CORS Preflight
        let opt_res = process_http_route(
            "OPTIONS",
            "/mcp",
            b"",
            Some("http://localhost:3000"),
            None,
            Some("localhost:9000"),
            &config,
            &mcp_cfg,
            &registry,
            9000,
        )
        .await;
        match opt_res {
            HttpResponse::Standard {
                status,
                cors_allow_origin,
                ..
            } => {
                assert_eq!(status, "204 No Content");
                assert_eq!(cors_allow_origin.as_deref(), Some("http://localhost:3000"));
            }
            _ => panic!("Expected standard response"),
        }

        // 2. Evil Origin blocked
        let evil_res = process_http_route(
            "GET",
            "/",
            b"",
            Some("https://evil.example"),
            None,
            Some("localhost:9000"),
            &config,
            &mcp_cfg,
            &registry,
            9000,
        )
        .await;
        match evil_res {
            HttpResponse::Standard { status, .. } => assert_eq!(status, "403 Forbidden"),
            _ => panic!("Expected standard response"),
        }

        // 3. Unauthorized request to /mcp
        let unauth_res = process_http_route(
            "POST",
            "/mcp",
            b"{}",
            None,
            None,
            Some("localhost:9000"),
            &config,
            &mcp_cfg,
            &registry,
            9000,
        )
        .await;
        match unauth_res {
            HttpResponse::Standard { status, .. } => assert_eq!(status, "401 Unauthorized"),
            _ => panic!("Expected standard response"),
        }

        // 4. Authorized request with Bearer token
        let rpc_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05"}
        });
        let rpc_bytes = serde_json::to_vec(&rpc_req).unwrap();
        let auth_res = process_http_route(
            "POST",
            "/mcp",
            &rpc_bytes,
            None,
            Some("Bearer secret123"),
            Some("localhost:9000"),
            &config,
            &mcp_cfg,
            &registry,
            9000,
        )
        .await;
        match auth_res {
            HttpResponse::Standard {
                status,
                content_type,
                body,
                ..
            } => {
                assert_eq!(status, "200 OK");
                assert_eq!(content_type, "application/json");
                let resp_val: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(resp_val["id"], 1);
            }
            _ => panic!("Expected standard response"),
        }
    }
}
