use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU16;
use std::time::Duration;

use rustgrid_config::ConfigStore;
use rustgrid_core::{ConnectionProfile, DriverId, DriverRegistry};
use rustgrid_mcp::config::{ConnectionMcpPermission, McpConfig, load_mcp_config, save_mcp_config};
use rustgrid_mcp::http::start_http_server;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;

#[tokio::test]
async fn test_local_mcp_http_server_e2e() -> anyhow::Result<()> {
    let config = ConfigStore::new()?;
    let registry = Arc::new(DriverRegistry::new());
    let active_port = Arc::new(AtomicU16::new(0));
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let orig_mcp_cfg = load_mcp_config(config.root());

    let test_token = "rg_mcp_test_token_abcdef123456";
    let mut conn_perms = BTreeMap::new();
    conn_perms.insert(
        "mock_test_id".to_string(),
        ConnectionMcpPermission {
            enabled: true,
            read_only: true,
            allowed_databases: Vec::new(),
            allowed_tables: Vec::new(),
        },
    );
    let mcp_cfg = McpConfig {
        enabled: true,
        http_enabled: true,
        port: 0,
        api_token: test_token.to_string(),
        connections: conn_perms,
        ..Default::default()
    };
    save_mcp_config(config.root(), &mcp_cfg)?;

    // Add a mock test profile
    let mut profile_opts = BTreeMap::new();
    profile_opts.insert("mcp.enabled".to_string(), "true".to_string());
    profile_opts.insert("mcp.read_only".to_string(), "true".to_string());
    let test_profile = ConnectionProfile {
        id: "mock_test_id".to_string(),
        name: "mock_conn".to_string(),
        driver: DriverId("sqlite".to_string()),
        host: String::new(),
        port: 0,
        username: String::new(),
        database: Some("/tmp/nonexistent_test.db".to_string()),
        options: profile_opts,
        settings: Default::default(),
    };
    let orig_profiles = config.load_profiles().unwrap_or_default();
    let mut test_profiles = orig_profiles.clone();
    test_profiles.push(test_profile);
    config.save_profiles(&test_profiles)?;

    let srv_config = config.clone();
    let srv_registry = registry.clone();
    let srv_port = active_port.clone();

    tokio::spawn(async move {
        let _ = start_http_server(srv_config, srv_registry, srv_port, shutdown_rx).await;
    });

    // Wait for server to bind port
    let mut port = 0;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        port = active_port.load(std::sync::atomic::Ordering::SeqCst);
        if port > 0 {
            break;
        }
    }
    assert!(port > 0, "Server failed to start and bind port within 2.5s");

    // Helper to send HTTP raw request
    async fn send_raw(port: u16, req: &str) -> anyhow::Result<(String, String)> {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await?;
        stream.write_all(req.as_bytes()).await?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;
        let full = String::from_utf8_lossy(&buf).to_string();
        if let Some((header, body)) = full.split_once("\r\n\r\n") {
            Ok((header.to_string(), body.to_string()))
        } else {
            Ok((full, String::new()))
        }
    }

    // 1. GET / prompt documentation (never leaks the actual test_token)
    let (head, body) = send_raw(
        port,
        &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    )
    .await?;
    assert!(
        head.starts_with("HTTP/1.1 200 OK"),
        "GET / returned: {head}"
    );
    assert!(body.contains("RustGrid AI / MCP Database Server"));
    assert!(
        !body.contains(test_token),
        "GET / must NOT leak plaintext token"
    );
    assert!(
        body.contains("<YOUR_BEARER_TOKEN>"),
        "GET / must contain placeholder"
    );

    // 1b. DNS Rebinding protection: Host: evil.example -> 400 Bad Request
    let (head, _) = send_raw(
        port,
        &format!("GET / HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n"),
    )
    .await?;
    assert!(
        head.starts_with("HTTP/1.1 400 Bad Request"),
        "DNS rebinding Host rejection expected, got: {head}"
    );

    // 2. P1-3: Origin: https://evil.example -> 403 Forbidden
    let (head, _) = send_raw(
        port,
        &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n"),
    ).await?;
    assert!(
        head.starts_with("HTTP/1.1 403 Forbidden"),
        "CORS rejection expected, got: {head}"
    );

    // 3. P1-3: Unauthorized POST /mcp -> 401 Unauthorized
    let (head, _) = send_raw(
        port,
        &format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"),
    ).await?;
    assert!(
        head.starts_with("HTTP/1.1 401 Unauthorized"),
        "Expected 401 without auth, got: {head}"
    );

    // 4. Authorized POST /mcp -> initialize
    let init_payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "2024-11-05"}
    })
    .to_string();
    let (head, body) = send_raw(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {test_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            init_payload.len(),
            init_payload
        ),
    ).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "init returned: {head}");
    let init_res: serde_json::Value = serde_json::from_str(&body)?;
    assert_eq!(init_res["result"]["serverInfo"]["name"], "rustgrid");

    // 5. tools/list -> verify 5 tools
    let list_payload = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    })
    .to_string();
    let (_, body) = send_raw(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {test_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            list_payload.len(),
            list_payload
        ),
    ).await?;
    let list_res: serde_json::Value = serde_json::from_str(&body)?;
    let tools = list_res["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 5);

    // 6. P0-1: CTE Mutating Bypass check (on authorized connection mock_conn)
    let cte_payload = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "execute_query",
            "arguments": {
                "connection": "mock_conn",
                "sql": "WITH x AS (DELETE FROM users RETURNING *) SELECT * FROM x"
            }
        }
    })
    .to_string();
    let (_, body) = send_raw(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {test_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            cte_payload.len(),
            cte_payload
        ),
    ).await?;
    let cte_res: serde_json::Value = serde_json::from_str(&body)?;
    let is_error = cte_res["result"]["isError"].as_bool().unwrap_or(false);
    let err_msg = cte_res["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(is_error);
    assert!(
        err_msg.contains("DELETE"),
        "Expected DELETE error, got: {err_msg}"
    );

    // 7. P0-2: Arbitrary file reading function check
    let file_payload = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "execute_query",
            "arguments": {
                "connection": "mock_conn",
                "sql": "SELECT pg_read_file('/etc/hosts')"
            }
        }
    })
    .to_string();
    let (_, body) = send_raw(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {test_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            file_payload.len(),
            file_payload
        ),
    ).await?;
    let file_res: serde_json::Value = serde_json::from_str(&body)?;
    let is_error = file_res["result"]["isError"].as_bool().unwrap_or(false);
    let err_msg = file_res["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(is_error);
    assert!(
        err_msg.contains("PG_READ_FILE"),
        "Expected PG_READ_FILE error, got: {err_msg}"
    );

    // 8. P2-4: describe_table on nonexistent connection/table returns error
    let desc_payload = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "describe_table",
            "arguments": {
                "connection": "nonexistent_conn_12345",
                "table": "nonexistent_tbl"
            }
        }
    })
    .to_string();
    let (_, body) = send_raw(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {test_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            desc_payload.len(),
            desc_payload
        ),
    ).await?;
    let desc_res: serde_json::Value = serde_json::from_str(&body)?;
    let is_error = desc_res["result"]["isError"].as_bool().unwrap_or(false);
    assert!(
        is_error,
        "describe_table on nonexistent connection must return isError: true"
    );

    // 8b. Query param ?token= on /mcp should be rejected (only Authorization: Bearer is accepted for HTTP POST)
    let (head, _) = send_raw(
        port,
        &format!(
            "POST /mcp?token={test_token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            init_payload.len(),
            init_payload
        ),
    ).await?;
    assert!(
        head.starts_with("HTTP/1.1 401 Unauthorized"),
        "Expected 401 when passing token via query string on /mcp, got: {head}"
    );

    // 8c. P1-B: SELECT ... INTO is blocked
    let into_payload = json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "execute_query",
            "arguments": {
                "connection": "mock_conn",
                "sql": "SELECT 1 AS x INTO TEMP _rg_tmp_probe"
            }
        }
    })
    .to_string();
    let (_, body) = send_raw(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {test_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            into_payload.len(),
            into_payload
        ),
    ).await?;
    let into_res: serde_json::Value = serde_json::from_str(&body)?;
    let is_error = into_res["result"]["isError"].as_bool().unwrap_or(false);
    let err_msg = into_res["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(is_error);
    assert!(
        err_msg.contains("INTO"),
        "Expected INTO error, got: {err_msg}"
    );

    // 8d. P2-C: pg_terminate_backend is blocked
    let term_payload = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "execute_query",
            "arguments": {
                "connection": "mock_conn",
                "sql": "SELECT pg_terminate_backend(2147483647)"
            }
        }
    })
    .to_string();
    let (_, body) = send_raw(
        port,
        &format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {test_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            term_payload.len(),
            term_payload
        ),
    ).await?;
    let term_res: serde_json::Value = serde_json::from_str(&body)?;
    let is_error = term_res["result"]["isError"].as_bool().unwrap_or(false);
    let err_msg = term_res["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(is_error);
    assert!(
        err_msg.contains("PG_TERMINATE_BACKEND"),
        "Expected PG_TERMINATE_BACKEND error, got: {err_msg}"
    );

    // 9. Shutdown server cleanly
    let _ = shutdown_tx.send(());

    // Restore profiles and original mcp config
    config.save_profiles(&orig_profiles)?;
    let _ = save_mcp_config(config.root(), &orig_mcp_cfg);

    Ok(())
}
