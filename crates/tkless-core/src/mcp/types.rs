use serde::{Deserialize, Serialize};

/// Standard JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

/// Standard JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: serde_json::Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    pub fn error_with_data(
        id: serde_json::Value,
        code: i32,
        message: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: Some(data),
            }),
        }
    }

    pub fn unsupported_protocol_version(id: serde_json::Value, requested: &str) -> Self {
        Self::error_with_data(
            id,
            UNSUPPORTED_PROTOCOL_VERSION,
            "Unsupported protocol version",
            serde_json::json!({
                "supported": [PROTOCOL_VERSION],
                "requested": requested
            }),
        )
    }

    pub fn header_mismatch(id: serde_json::Value, message: impl Into<String>) -> Self {
        Self::error(id, HEADER_MISMATCH, message)
    }
}

/// Standard JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// Standard JSON-RPC 2.0 error codes
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

/// MCP protocol version implemented by this server.
pub const PROTOCOL_VERSION: &str = "2026-07-28";
/// MCP-defined error: HTTP headers do not match the request body.
pub const HEADER_MISMATCH: i32 = -32020;
/// MCP-defined error: the client lacks a required capability.
pub const MISSING_REQUIRED_CLIENT_CAPABILITY: i32 = -32021;
/// MCP-defined error: the requested protocol version is not supported.
pub const UNSUPPORTED_PROTOCOL_VERSION: i32 = -32022;

/// Model Context Protocol (MCP) Tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
    #[serde(
        rename = "outputSchema",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub output_schema: Option<serde_json::Value>,
}

/// Model Context Protocol text content item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

impl McpContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content_type: "text".to_string(),
            text: text.into(),
        }
    }
}

/// Model Context Protocol tool execution result payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCallResult {
    pub content: Vec<McpContent>,
    #[serde(rename = "isError")]
    pub is_error: bool,
    #[serde(
        rename = "structuredContent",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub structured_content: Option<serde_json::Value>,
}

impl McpToolCallResult {
    pub fn success(text: impl Into<String>) -> Self {
        Self {
            content: vec![McpContent::text(text)],
            is_error: false,
            structured_content: None,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            content: vec![McpContent::text(text.clone())],
            is_error: true,
            structured_content: Some(
                serde_json::json!({"error": {"code":"tool_error", "message":text,"retryable":false,"exit_code":1}}),
            ),
        }
    }

    pub fn with_structured(mut self, value: impl Serialize) -> Self {
        match serde_json::to_value(value) {
            Ok(value) => self.structured_content = Some(value),
            Err(error) => {
                return Self::error(format!("Failed to serialize structured result: {error}"))
            }
        }
        self
    }

    pub fn from_error(error: &crate::TklessError) -> Self {
        Self::error(error.to_string())
            .with_structured(serde_json::json!({"error": error.details()}))
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::from_error(&crate::TklessError::InvalidArgument(message.into()))
    }
}
