//! MCP's stdio transport.
//!
//! The client launches `RustGrid --mcp-stdio`; this module reads one JSON-RPC object per line from
//! stdin and writes one response per line to stdout. **Nothing else may be written to stdout** —
//! tracing, warnings and errors go to stderr — or the client's framing breaks.

use std::io::{BufRead, Write};
use std::sync::Arc;

use rustgrid_config::ConfigStore;
use rustgrid_core::DriverRegistry;

use crate::process_request;
use crate::protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};

/// Serve one standard-input/standard-output pair until EOF.
///
/// The caller owns the runtime because RustGrid already has one (the app's database runtime).
/// `runtime.block_on` runs each tool call; the read loop itself is deliberately synchronous, which
/// makes it testable with in-memory readers/writers and avoids a nested runtime.
pub fn serve<R: BufRead, W: Write>(
    reader: R,
    mut writer: W,
    config: &ConfigStore,
    registry: &Arc<DriverRegistry>,
    runtime: &tokio::runtime::Handle,
) -> anyhow::Result<()> {
    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(error) => {
                write_response(
                    &mut writer,
                    &JsonRpcResponse::error(
                        None,
                        JsonRpcError::parse_error(format!("Invalid JSON: {error}")),
                    ),
                )?;
                continue;
            }
        };

        // JSON-RPC notifications (including MCP's `notifications/initialized`) have no response.
        if request.id.is_none() {
            continue;
        }

        let mcp_cfg = match config.load_mcp() {
            Ok(config) => config,
            Err(error) => {
                write_response(
                    &mut writer,
                    &JsonRpcResponse::error(
                        request.id.clone(),
                        JsonRpcError::internal_error(format!("Could not read mcp.json: {error}")),
                    ),
                )?;
                continue;
            }
        };

        let response = runtime.block_on(process_request(config, &mcp_cfg, registry, request));
        write_response(&mut writer, &response)?;
    }
    Ok(())
}

fn write_response<W: Write>(writer: &mut W, response: &JsonRpcResponse) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *writer, response)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use rustgrid_config::McpConfig;

    use super::*;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    fn temp_store(name: &str) -> (ConfigStore, std::path::PathBuf) {
        let root =
            std::env::temp_dir().join(format!("rustgrid_mcp_stdio_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp config dir");
        (ConfigStore::with_root(root.clone()), root)
    }

    fn run(input: &str, store: &ConfigStore, runtime: &tokio::runtime::Handle) -> Vec<u8> {
        let mut output = Vec::new();
        serve(
            Cursor::new(input.as_bytes().to_vec()),
            &mut output,
            store,
            &Arc::new(DriverRegistry::new()),
            runtime,
        )
        .expect("serve");
        output
    }

    #[test]
    fn answers_initialize_and_tools_list_and_skips_notifications() {
        let (store, root) = temp_store("protocol");
        store.save_mcp(&McpConfig::default()).expect("save");
        let rt = runtime();
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n",
        );
        let output = run(input, &store, rt.handle());
        let lines: Vec<serde_json::Value> = String::from_utf8(output)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("json line"))
            .collect();
        assert_eq!(lines.len(), 2, "the notification must not get a response");
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "rustgrid");
        assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 5);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reports_a_bad_json_line_and_keeps_going() {
        let (store, root) = temp_store("parse");
        let rt = runtime();
        let input = concat!(
            "not json\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n",
        );
        let output = run(input, &store, rt.handle());
        let lines: Vec<serde_json::Value> = String::from_utf8(output)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("json line"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["error"]["code"], -32700);
        assert_eq!(lines[1]["result"], serde_json::json!({}));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_disabled_server_refuses_tools() {
        let (store, root) = temp_store("disabled");
        store.save_mcp(&McpConfig::default()).expect("save");
        let rt = runtime();
        let input = "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"list_connections\",\"arguments\":{}}}\n";
        let output = run(input, &store, rt.handle());
        let response: serde_json::Value =
            serde_json::from_str(String::from_utf8(output).expect("utf8").trim()).expect("json");
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("disabled")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
