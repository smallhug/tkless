mod contracts;
pub mod server;
pub mod tools;
pub mod types;

pub use server::{HttpTransportOptions, McpMode, McpServer};
pub use tools::{
    get_available_tools, get_core_tools, get_dynamic_discovery_tools, get_initial_dynamic_tools,
    get_tool_catalog, ToolCatalogEntry,
};
pub use types::{
    McpContent, McpTool, McpToolCallResult, HEADER_MISMATCH, INVALID_PARAMS, METHOD_NOT_FOUND,
    MISSING_REQUIRED_CLIENT_CAPABILITY, PROTOCOL_VERSION, UNSUPPORTED_PROTOCOL_VERSION,
};
