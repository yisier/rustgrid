pub mod config;
pub mod format;
pub mod http;
pub mod protocol;
pub mod security;
pub mod tools;

pub use config::*;
pub use format::*;
pub use http::*;
pub use protocol::*;
pub use security::*;
pub use tools::*;

use std::sync::Arc;

use rustgrid_config::ConfigStore;
use rustgrid_core::DriverRegistry;
use serde_json::{Value, json};

pub async fn process_request(
    config: &ConfigStore,
    mcp_cfg: &McpConfig,
    registry: &Arc<DriverRegistry>,
    request: JsonRpcRequest,
) -> JsonRpcResponse {
    let req_id = request.id.clone();
    match request.method.as_str() {
        "initialize" => {
            let res = InitializeResult {
                protocol_version: PROTOCOL_VERSION.to_string(),
                capabilities: ServerCapabilities {
                    tools: ToolsCapability {
                        list_changed: Some(false),
                    },
                },
                server_info: ServerInfo {
                    name: "rustgrid".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
            };
            JsonRpcResponse::success(req_id, json!(res))
        }
        "ping" => JsonRpcResponse::success(req_id, json!({})),
        "tools/list" => {
            let list = ListToolsResult {
                tools: tools::list_tools(),
            };
            JsonRpcResponse::success(req_id, json!(list))
        }
        "tools/call" => {
            let params = request.params.unwrap_or_else(|| json!({}));
            let tool_name = match params.get("name").and_then(Value::as_str) {
                Some(n) => n,
                None => {
                    return JsonRpcResponse::error(
                        req_id,
                        JsonRpcError::invalid_params("Missing required 'name' field in params"),
                    );
                }
            };
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            let tool_res = match tool_name {
                "list_connections" => tools::handle_list_connections(config, mcp_cfg),
                "get_schema" => tools::handle_get_schema(config, mcp_cfg, registry, &args).await,
                "describe_table" => {
                    tools::handle_describe_table(config, mcp_cfg, registry, &args).await
                }
                "execute_query" => {
                    tools::handle_execute_query(config, mcp_cfg, registry, &args).await
                }
                "sample_data" => tools::handle_sample_data(config, mcp_cfg, registry, &args).await,
                unknown => {
                    return JsonRpcResponse::error(
                        req_id,
                        JsonRpcError::method_not_found(format!("Unknown tool: {unknown}")),
                    );
                }
            };

            let res_val = serde_json::to_value(&tool_res).unwrap_or_else(|_| json!({}));
            JsonRpcResponse::success(req_id, res_val)
        }
        unknown => JsonRpcResponse::error(
            req_id,
            JsonRpcError::method_not_found(format!("Unsupported method: {unknown}")),
        ),
    }
}
