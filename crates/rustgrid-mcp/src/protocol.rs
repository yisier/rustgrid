use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const PERMISSION_DENIED: i64 = -32001;

pub const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(code: i64, message: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            message: message.into(),
            data: Some(data),
        }
    }

    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self::new(PARSE_ERROR, msg)
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self::new(INVALID_REQUEST, msg)
    }

    pub fn method_not_found(msg: impl Into<String>) -> Self {
        Self::new(METHOD_NOT_FOUND, msg)
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, msg)
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, msg)
    }

    pub fn permission_denied(msg: impl Into<String>) -> Self {
        Self::new(PERMISSION_DENIED, msg)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub tools: ToolsCapability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ToolsCapability {
    #[serde(
        rename = "listChanged",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpTool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    #[serde(
        rename = "isError",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub is_error: bool,
}

impl CallToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::text(text)],
            is_error: false,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::text(msg)],
            is_error: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
}

impl ToolContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_jsonrpc_request_serde() {
        let req_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#;
        let req: JsonRpcRequest = serde_json::from_str(req_json).expect("deserialize");
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(json!(1)));
        assert_eq!(req.method, "initialize");
        assert!(req.params.is_some());

        let reserialized = serde_json::to_string(&req).expect("serialize");
        assert!(reserialized.contains("\"method\":\"initialize\""));
    }

    #[test]
    fn test_jsonrpc_response_success() {
        let res = JsonRpcResponse::success(Some(json!(1)), json!({"status":"ok"}));
        assert_eq!(res.jsonrpc, "2.0");
        assert_eq!(res.id, Some(json!(1)));
        assert!(res.result.is_some());
        assert!(res.error.is_none());

        let s = serde_json::to_string(&res).unwrap();
        assert!(s.contains("\"result\":{\"status\":\"ok\"}"));
    }

    #[test]
    fn test_jsonrpc_response_error() {
        let err = JsonRpcError::method_not_found("unknown method foo");
        let res = JsonRpcResponse::error(Some(json!(42)), err);
        assert_eq!(res.id, Some(json!(42)));
        assert_eq!(res.error.as_ref().unwrap().code, METHOD_NOT_FOUND);
        assert_eq!(res.error.as_ref().unwrap().message, "unknown method foo");

        let s = serde_json::to_string(&res).unwrap();
        assert!(s.contains("\"code\":-32601"));
    }

    #[test]
    fn test_jsonrpc_errors_all_constructors() {
        assert_eq!(JsonRpcError::parse_error("bad json").code, PARSE_ERROR);
        assert_eq!(
            JsonRpcError::invalid_request("bad req").code,
            INVALID_REQUEST
        );
        assert_eq!(
            JsonRpcError::invalid_params("bad param").code,
            INVALID_PARAMS
        );
        assert_eq!(
            JsonRpcError::internal_error("internal").code,
            INTERNAL_ERROR
        );
        assert_eq!(
            JsonRpcError::permission_denied("denied").code,
            PERMISSION_DENIED
        );

        let with_data = JsonRpcError::with_data(100, "with data", json!({"hint":"retry"}));
        assert_eq!(with_data.code, 100);
        assert_eq!(with_data.data, Some(json!({"hint":"retry"})));
    }

    #[test]
    fn test_mcp_structures() {
        let init_result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities {
                tools: ToolsCapability {
                    list_changed: Some(true),
                },
            },
            server_info: ServerInfo {
                name: "rustgrid".to_string(),
                version: "0.1.14".to_string(),
            },
        };
        let init_json = serde_json::to_string(&init_result).unwrap();
        assert!(init_json.contains("\"protocolVersion\":\"2024-11-05\""));
        assert!(
            init_json.contains("\"serverInfo\":{\"name\":\"rustgrid\",\"version\":\"0.1.14\"}")
        );

        let tool = McpTool {
            name: "execute_query".to_string(),
            description: "Run SQL query".to_string(),
            input_schema: json!({"type":"object"}),
        };
        let tool_list = ListToolsResult { tools: vec![tool] };
        let tool_list_json = serde_json::to_string(&tool_list).unwrap();
        assert!(tool_list_json.contains("\"inputSchema\":{\"type\":\"object\"}"));

        let ok_call = CallToolResult::text("Result rows");
        assert!(!ok_call.is_error);
        assert_eq!(ok_call.content.len(), 1);

        let err_call = CallToolResult::error("Blocked by read-only mode");
        assert!(err_call.is_error);
        assert_eq!(err_call.content.len(), 1);
    }
}
