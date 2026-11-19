//! End-to-end tests for the `RustGrid --mcp-stdio` entry point.
//!
//! The protocol test only needs the executable and a temporary configuration directory.
//! The SQLite test creates a local database through the same driver the app ships, so it
//! exercises the real registry, permission checks, read-only session and stdio framing without a
//! network server.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rustgrid_config::{ConfigStore, McpConfig, McpConnectionPermission};
use rustgrid_core::{ConnectionConfig, ConnectionProfile, Driver, DriverId};
use serde_json::{Value, json};

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("rustgrid_mcp_app_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp config dir");
    root
}

fn run_stdio(root: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_RustGrid"))
        .arg("--mcp-stdio")
        .env("RUSTGRID_CONFIG_DIR", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn RustGrid --mcp-stdio");

    {
        let mut stdin = child.stdin.take().expect("stdin");
        for request in requests {
            serde_json::to_writer(&mut stdin, request).expect("write request");
            stdin.write_all(b"\n").expect("newline");
        }
        stdin.flush().expect("flush");
    }

    let output = child.wait_with_output().expect("wait for stdio server");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut responses = Vec::new();
    for line in stdout.lines() {
        match serde_json::from_str::<Value>(line) {
            Ok(response) => responses.push(response),
            Err(error) => {
                panic!("stdout carried a non-JSON line ({error}): {line:?}\nstderr:\n{stderr}")
            }
        }
    }
    assert!(
        output.status.success(),
        "stdio server exited with {:?}\nstderr:\n{stderr}",
        output.status
    );
    responses
}

fn create_sqlite_database(path: &Path) {
    // The driver deliberately refuses to create a missing file (a typo must not produce an empty
    // database), so the test creates the file first; SQLite treats a zero-length file as a new DB.
    std::fs::File::create(path).expect("create sqlite file");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let driver = rustgrid_sqlite::SqliteDriver::new();
        let config = ConnectionConfig {
            driver: DriverId::new("sqlite"),
            host: String::new(),
            port: 0,
            username: String::new(),
            password: None,
            database: Some(path.to_string_lossy().to_string()),
            options: Default::default(),
            settings: Default::default(),
        };
        let connection = driver.connect(&config).await.expect("connect sqlite");
        connection
            .execute_query(
                None,
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            )
            .await
            .expect("create users");
        connection
            .execute_query(
                None,
                "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob')",
            )
            .await
            .expect("insert users");
        connection
            .execute_query(
                None,
                "CREATE TABLE secret (id INTEGER PRIMARY KEY, token TEXT NOT NULL)",
            )
            .await
            .expect("create secret");
        connection
            .execute_query(None, "INSERT INTO secret (id, token) VALUES (1, 'hidden')")
            .await
            .expect("insert secret");
    });
}

#[test]
fn stdio_protocol_works_and_a_disabled_server_refuses_tools() {
    let root = temp_root("protocol");
    let responses = run_stdio(
        &root,
        &[
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {}
            }),
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "list_connections", "arguments": {} }
            }),
        ],
    );
    assert_eq!(
        responses.len(),
        3,
        "the notification must not get a response"
    );
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "rustgrid");
    assert_eq!(responses[1]["result"]["tools"].as_array().unwrap().len(), 5);
    assert_eq!(responses[2]["result"]["isError"], true);
    assert!(
        responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("disabled")
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sqlite_tools_enforce_read_only_and_the_sensitive_table_list() {
    let root = temp_root("sqlite");
    let database_path = root.join("mcp-test.sqlite3");
    create_sqlite_database(&database_path);

    let store = ConfigStore::with_root(root.clone());
    let profile = ConnectionProfile {
        id: "sqlite-test".to_string(),
        name: "SQLite Test".to_string(),
        driver: DriverId::new("sqlite"),
        host: String::new(),
        port: 0,
        username: String::new(),
        database: Some(database_path.to_string_lossy().to_string()),
        options: Default::default(),
        settings: Default::default(),
    };
    store.save_profiles(&[profile]).expect("save profile");
    let mut mcp = McpConfig {
        enabled: true,
        ..Default::default()
    };
    mcp.connections.insert(
        "sqlite-test".to_string(),
        McpConnectionPermission {
            read_only: true,
            allowed_databases: Vec::new(),
            blacklisted_tables: vec!["secret".to_string()],
            context_prompt: String::new(),
        },
    );
    store.save_mcp(&mcp).expect("save mcp config");

    let responses = run_stdio(
        &root,
        &[
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "get_schema",
                    "arguments": { "connection": "sqlite-test" }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "describe_table",
                    "arguments": { "connection": "sqlite-test", "database": "main", "table": "users" }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "sample_data",
                    "arguments": { "connection": "sqlite-test", "database": "main", "table": "users" }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "execute_query",
                    "arguments": { "connection": "sqlite-test", "sql": "SELECT name FROM users ORDER BY id" }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "execute_query",
                    "arguments": { "connection": "sqlite-test", "sql": "SELECT * FROM secret" }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": "execute_query",
                    "arguments": { "connection": "sqlite-test", "sql": "DROP TABLE users" }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "list_connections",
                    "arguments": {}
                }
            }),
        ],
    );

    assert_eq!(responses.len(), 7);
    let text = |index: usize| {
        responses[index]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let schema = text(0);
    assert!(schema.contains("users"), "schema: {schema}");
    assert!(
        !schema.contains("secret"),
        "blacklist leaked into schema: {schema}"
    );
    assert_ne!(responses[0]["result"]["isError"], true);

    assert!(text(1).contains("CREATE TABLE"));
    assert!(text(2).contains("Alice"));
    assert!(text(3).contains("Alice") && text(3).contains("Bob"));

    assert_eq!(responses[4]["result"]["isError"], true);
    assert!(text(4).to_lowercase().contains("blacklist"));
    assert_eq!(responses[5]["result"]["isError"], true);
    assert!(text(5).to_lowercase().contains("permission denied"));
    assert!(text(6).contains("SQLite Test"));

    let _ = std::fs::remove_dir_all(root);
}
