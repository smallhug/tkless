mod auth;
mod content;
mod discovery;
mod extraction;
mod http;
mod inspection;
mod jobs;
mod jobs_registry;
mod media;
mod search;
mod transport;
mod utilities;
mod workflows;

use super::tools::{
    get_available_tools, get_dynamic_discovery_tools, get_initial_dynamic_tools, get_tool_catalog,
};
use super::types::*;
use crate::adapters::BilibiliAdapter;
use crate::cache::DocumentCache;
use crate::error::TklessError;
use crate::model::{
    ExtractionMode, ExtractionOptions, FetchOptions, OnTooLarge, OutputFormat, TableExportFormat,
    TokenizerKind, VideoQuality,
};
use crate::TklessEngine;
use jobs_registry::JobRegistry;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

pub use http::HttpTransportOptions;

/// Bounded notification sender; consumers must drain it concurrently with streaming calls.
pub type NotificationSender = tokio::sync::mpsc::Sender<Value>;

/// Operational mode for the MCP server: Full exposure vs Two-Stage Dynamic Discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpMode {
    /// Standard full mode: exposes all business tools in tools/list.
    #[default]
    Full,
    /// Two-stage dynamic discovery mode: exposes 2 core tools + 4 dynamic discovery meta-tools
    /// (discover_tools, get_tool_schema, execute_tool, activate_tools) reducing the initial tool declarations.
    Dynamic,
}

/// In-memory and stdio Model Context Protocol (MCP) server for TkLess.
#[derive(Clone)]
pub struct McpServer {
    engine: Arc<TklessEngine>,
    cache: Arc<DocumentCache>,
    mode: McpMode,
    active_dynamic_tools: Arc<RwLock<HashSet<String>>>,
    tool_timeout_secs: u64,
    jobs: Arc<JobRegistry>,
}

impl McpServer {
    /// Creates a new MCP server wrapping the provided extraction engine and document cache in default Full mode.
    pub fn new(engine: TklessEngine, cache: DocumentCache) -> Self {
        Self::new_with_mode(engine, cache, McpMode::Full)
    }

    /// Creates a new MCP server with a specific operational mode (Full or Dynamic).
    pub fn new_with_mode(engine: TklessEngine, cache: DocumentCache, mode: McpMode) -> Self {
        Self {
            engine: Arc::new(engine),
            cache: Arc::new(cache),
            mode,
            active_dynamic_tools: Arc::new(RwLock::new(HashSet::new())),
            tool_timeout_secs: 300,
            jobs: Arc::new(JobRegistry::new()),
        }
    }

    /// Builder method to change the per-call timeout used by both transports.
    pub fn with_tool_timeout(mut self, seconds: u64) -> Self {
        self.tool_timeout_secs = seconds.clamp(1, 3600);
        self
    }

    /// Builder method to specify the MCP operational mode.
    /// Library callers may configure an existing server; the CLI uses `new_with_mode`.
    pub fn with_mode(mut self, mode: McpMode) -> Self {
        self.mode = mode;
        self
    }

    /// Returns the current MCP operational mode.
    pub fn mode(&self) -> McpMode {
        self.mode
    }

    /// Handles a single incoming JSON-RPC line and returns the appropriate response, if any.
    pub async fn handle_message(&self, line: &str) -> Option<JsonRpcResponse> {
        self.handle_message_with_notifier(line, None).await
    }

    /// Handles a single incoming JSON-RPC line with optional progress/logging notification streaming.
    pub async fn handle_message_with_notifier(
        &self,
        line: &str,
        notifier: Option<&NotificationSender>,
    ) -> Option<JsonRpcResponse> {
        let trimmed = line.trim().trim_matches('\u{feff}');
        if trimmed.is_empty() {
            return None;
        }

        let value: Value = match serde_json::from_str(trimmed) {
            Ok(req) => req,
            Err(e) => {
                warn!(error = %e, "Malformed JSON-RPC request");
                return Some(JsonRpcResponse::error(
                    Value::Null,
                    PARSE_ERROR,
                    "Invalid JSON-RPC 2.0 payload",
                ));
            }
        };
        let request_id = value.get("id").cloned().unwrap_or(Value::Null);
        let request: JsonRpcRequest = match serde_json::from_value(value) {
            Ok(request) => request,
            Err(error) => {
                return Some(JsonRpcResponse::error(
                    request_id,
                    INVALID_REQUEST,
                    format!("Invalid JSON-RPC request: {error}"),
                ));
            }
        };

        if request.jsonrpc != "2.0" {
            return Some(JsonRpcResponse::error(
                request.id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "Expected JSON-RPC 2.0",
            ));
        }
        if request.id.is_none() && request.method != "notifications/initialized" {
            return None;
        }
        let req_id = request.id.unwrap_or(Value::Null);

        debug!(method = %request.method, id = ?req_id, "Handling MCP request");

        match request.method.as_str() {
            "initialize" => {
                let version = "2025-11-25";
                let init_result = serde_json::json!({
                    "protocolVersion": version,
                    "capabilities": {
                        "tools": {"listChanged": self.mode == McpMode::Dynamic},
                        "logging": {},
                        "experimental": {"tkless/batchStreaming":{"notification":"notifications/progress","itemKey":"tkless/batch_item"},"tkless/asyncJobs":{"start":"jobs_start","status":"jobs_status","cancel":"jobs_cancel","list":"jobs_list","maxActive":4}}
                    },
                    "serverInfo": {
                        "name": "tkless",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                });
                Some(JsonRpcResponse::success(req_id, init_result))
            }
            "notifications/initialized" => {
                info!("MCP client initialized notification received");
                None // Notifications do not return a JSON-RPC response
            }
            "ping" => Some(JsonRpcResponse::success(req_id, serde_json::json!({}))),
            "tools/list" => {
                let tools = match self.mode {
                    McpMode::Full => get_available_tools(),
                    McpMode::Dynamic => {
                        let mut dynamic_tools = get_initial_dynamic_tools();
                        let active_set = self.active_dynamic_tools.read().await;
                        if !active_set.is_empty() {
                            let all_tools = get_available_tools();
                            for tool in all_tools {
                                if active_set.contains(&tool.name)
                                    && !dynamic_tools.iter().any(|t| t.name == tool.name)
                                {
                                    dynamic_tools.push(tool);
                                }
                            }
                        }
                        dynamic_tools
                    }
                };
                Some(JsonRpcResponse::success(
                    req_id,
                    serde_json::json!({ "tools": tools }),
                ))
            }
            "tools/call" => {
                let params = request.params.unwrap_or(Value::Null);
                let raw_tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));
                let progress_token = params
                    .get("_meta")
                    .and_then(|m| m.get("progressToken"))
                    .cloned();

                let tool_result = self
                    .dispatch_tool_execution(
                        raw_tool_name,
                        &arguments,
                        notifier,
                        progress_token.as_ref(),
                    )
                    .await;

                let result_value = serde_json::to_value(tool_result).unwrap_or_else(|e| {
                    serde_json::json!({
                        "content": [{"type": "text", "text": format!("Serialization error: {e}")}],
                        "isError": true
                    })
                });

                Some(JsonRpcResponse::success(req_id, result_value))
            }
            _ => Some(JsonRpcResponse::error(
                req_id,
                METHOD_NOT_FOUND,
                format!(
                    "Method '{}' is not supported by TkLess MCP server",
                    request.method
                ),
            )),
        }
    }

    // -------------------------------------------------------------------------
    // Tool Dispatch Handlers & Dynamic Tool Discovery
    // -------------------------------------------------------------------------

    /// Executes canonical tool names directly or through execute_tool.
    pub async fn dispatch_tool_execution(
        &self,
        raw_tool_name: &str,
        arguments: &Value,
        notifier: Option<&NotificationSender>,
        progress_token: Option<&Value>,
    ) -> McpToolCallResult {
        let tool_name = raw_tool_name;
        if let Err(error) = super::contracts::validate_arguments(tool_name, arguments) {
            return McpToolCallResult::from_error(&error);
        }
        let result = match tool_name {
            "browse_page" => self.call_browse_page(arguments).await,
            "collect_pages" => self.call_collect_pages(arguments).await,
            "list_wechat_articles" => self.call_list_wechat_articles(arguments).await,
            "sync_collection" => self.call_sync_collection(arguments).await,
            // Stage 1: Dynamic Discovery Meta-Tools
            "discover_tools" => self.call_discover_tools(arguments).await,
            "get_tool_schema" => self.call_get_tool_schema(arguments).await,
            "execute_tool" => {
                let target_tool = arguments
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if target_tool.is_empty() {
                    return McpToolCallResult::error("Missing required parameter 'tool_name'");
                }
                if target_tool == "execute_tool" {
                    return McpToolCallResult::error("Cannot execute 'execute_tool' recursively");
                }
                let target_args = arguments
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                Box::pin(self.dispatch_tool_execution(
                    target_tool,
                    &target_args,
                    notifier,
                    progress_token,
                ))
                .await
            }
            "activate_tools" => self.call_activate_tools(arguments, notifier).await,

            // Core & Specialized Tools
            "fetch_webpage" => self.call_fetch_webpage(arguments).await,
            "fetch_bilibili_feed" => self.call_fetch_bilibili_feed(arguments).await,
            "search_bilibili" => self.call_search_bilibili(arguments).await,
            "download_media" => self.call_download_media(arguments).await,
            "web_search" => self.call_web_search(arguments).await,
            "crawl_site" => {
                self.call_crawl_site(arguments, notifier, progress_token)
                    .await
            }
            "batch_fetch_webpages" => {
                self.call_batch_fetch_webpages(arguments, notifier, progress_token)
                    .await
            }
            "clean_html" => self.call_clean_html(arguments).await,
            "login_platform" => self.call_login_platform(arguments).await,
            "scaffold_schema" => self.call_scaffold_schema(arguments).await,
            "compare_similarity" => self.call_compare_similarity(arguments).await,
            "manage_cache" => self.call_manage_cache(arguments).await,
            "fetch_feed" => self.call_fetch_feed(arguments).await,
            "inspect_links" => self.call_inspect_links(arguments).await,
            "check_auth" => self.call_check_auth(arguments).await,
            "summarize_content" => self.call_summarize_content(arguments).await,
            "split_document" => self.call_split_document(arguments).await,
            "chunk_markdown" => self.call_chunk_markdown(arguments).await,
            "diff_documents" => self.call_diff_content(arguments).await,
            "deep_research" => self.call_deep_research(arguments).await,
            "jobs_start" => self.call_jobs_start(arguments).await,
            "jobs_status" => self.call_jobs_status(arguments).await,
            "jobs_cancel" => self.call_jobs_cancel(arguments).await,
            "jobs_list" => self.call_jobs_list(arguments).await,
            "take_screenshot" => self.call_capture_screenshot(arguments).await,
            "export_tables" => self.call_export_tables(arguments).await,
            "list_adapters" => self.call_list_adapters(arguments).await,
            _ => McpToolCallResult::error(format!("Unknown tool: '{raw_tool_name}'")),
        };
        if let Some(schema) = super::contracts::output_schema(tool_name) {
            let valid = result
                .structured_content
                .as_ref()
                .ok_or_else(|| "Missing structuredContent".to_string())
                .and_then(|value| super::contracts::validate_value(&schema, value, "result"));
            if let Err(message) = valid {
                return McpToolCallResult::from_error(&TklessError::McpError {
                    code: INTERNAL_ERROR,
                    message: format!("Invalid output from {tool_name}: {message}"),
                });
            }
        }
        result
    }
}

fn format_doc_result(doc: &crate::model::PageDocument, format: OutputFormat) -> McpToolCallResult {
    let result = match format {
        OutputFormat::Json => match serde_json::to_string_pretty(doc) {
            Ok(json_str) => McpToolCallResult::success(json_str),
            Err(e) => McpToolCallResult::error(format!("Failed to serialize JSON: {e}")),
        },
        _ => {
            if let Some(ref chunks) = doc.chunks {
                let mut out = String::new();
                out.push_str(&format!(
                    "# 语义切块输出: {} (共 {} 块)\n\n",
                    doc.title.as_deref().unwrap_or(&doc.url),
                    chunks.len()
                ));
                for chunk in chunks {
                    let path_str = if chunk.heading_path.is_empty() {
                        "根节点".to_string()
                    } else {
                        chunk.heading_path.join(" > ")
                    };
                    out.push_str(&format!(
                        "### [Chunk {}/{}] (Tokens: {}) | 标题路径: {}\n\n",
                        chunk.index + 1,
                        chunks.len(),
                        chunk.token_count,
                        path_str
                    ));
                    out.push_str(chunk.content.trim());
                    out.push_str("\n\n---\n\n");
                }
                McpToolCallResult::success(out)
            } else {
                McpToolCallResult::success(&doc.content)
            }
        }
    };
    result.with_structured(serde_json::json!({"document":doc}))
}
