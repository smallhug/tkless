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
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use url::Url;

/// Bounded notification sender; consumers must drain it concurrently with streaming calls.
pub type NotificationSender = tokio::sync::mpsc::Sender<Value>;

/// Operational mode for the MCP server: Full exposure vs Two-Stage Dynamic Discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpMode {
    /// Standard full mode: exposes all 23 tools in tools/list.
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
        }
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
                        "experimental": {"tkless/batchStreaming":{"notification":"notifications/progress","itemKey":"tkless/batch_item"}}
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

    /// Run the standard transport, keeping the input responsive during long tool calls.
    pub async fn run_stdio(&self) -> Result<(), TklessError> {
        self.run_transport(tokio::io::stdin(), tokio::io::stdout())
            .await
    }

    /// Transport-independent entry point for embedded hosts and protocol regression tests.
    pub async fn run_transport<R, W>(&self, input: R, mut output: W) -> Result<(), TklessError>
    where
        R: tokio::io::AsyncRead + Unpin + Send,
        W: tokio::io::AsyncWrite + Unpin + Send,
    {
        let mut lines = BufReader::new(input).lines();
        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel::<Value>(8);
        let mut tasks = tokio::task::JoinSet::new();
        let mut active = std::collections::HashMap::<String, tokio::task::AbortHandle>::new();
        let mut eof = false;
        loop {
            if eof && tasks.is_empty() {
                while let Ok(notification) = notify_rx.try_recv() {
                    write_rpc(&mut output, &notification).await?;
                }
                break;
            }
            tokio::select! {
                line = lines.next_line(), if !eof => {
                    let Some(line) = line? else { eof = true; continue; };
                    let value = serde_json::from_str::<Value>(line.trim().trim_matches('\u{feff}')).ok();
                    if value.as_ref().and_then(|v| v.get("method")).and_then(Value::as_str) == Some("notifications/cancelled") {
                        if let Some(id) = value.as_ref().and_then(|v| v.pointer("/params/requestId")) {
                            if let Some(task) = active.remove(&id.to_string()) { task.abort(); }
                        }
                        continue;
                    }
                    let id = value.as_ref().and_then(|v| v.get("id")).cloned();
                    let tool_call = value.as_ref().and_then(|v| v.get("method")).and_then(Value::as_str) == Some("tools/call");
                    if let (true, Some(id)) = (tool_call, id) {
                        let key = id.to_string();
                        if active.contains_key(&key) || active.len() >= 64 {
                            let response = JsonRpcResponse::error(id, INVALID_REQUEST, "Duplicate request ID or too many concurrent requests (64)");
                            write_rpc(&mut output, &response).await?;
                            continue;
                        }
                        let server = self.clone();
                        let tx = notify_tx.clone();
                        let task_key = key.clone();
                        let task = tasks.spawn(async move {
                            let response = match tokio::time::timeout(std::time::Duration::from_secs(300), server.handle_message_with_notifier(&line, Some(&tx))).await {
                                Ok(response) => response,
                                Err(_) => Some(JsonRpcResponse::error(id, -32001, "Tool call exceeded the 300 second timeout")),
                            };
                            (task_key, response)
                        });
                        active.insert(key, task);
                    } else if let Some(response) = self.handle_message_with_notifier(&line, Some(&notify_tx)).await {
                        write_rpc(&mut output, &response).await?;
                    }
                }
                joined = tasks.join_next_with_id(), if !tasks.is_empty() => {
                    if let Some(Ok((task_id, (key, response)))) = joined {
                        if active.get(&key).is_some_and(|handle| handle.id() == task_id) {
                            active.remove(&key);
                            if let Some(response) = response {
                                let pending = notify_rx.len();
                                for _ in 0..pending {
                                    if let Ok(notification) = notify_rx.try_recv() { write_rpc(&mut output, &notification).await?; }
                                }
                                write_rpc(&mut output, &response).await?;
                            }
                        }
                    }
                }
                Some(notification) = notify_rx.recv() => { write_rpc(&mut output, &notification).await?; }
            }
        }
        Ok(())
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

    async fn call_discover_tools(&self, args: &Value) -> McpToolCallResult {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_lowercase());
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("all")
            .trim()
            .to_lowercase();

        let catalog = get_tool_catalog();
        let active_set = self.active_dynamic_tools.read().await;

        let filtered: Vec<_> = catalog
            .into_iter()
            .filter(|entry| {
                if category != "all" && !entry.category.eq_ignore_ascii_case(&category) {
                    return false;
                }
                if let Some(ref q) = query {
                    if q.is_empty() {
                        return true;
                    }
                    entry.name.to_lowercase().contains(q)
                        || entry.summary.to_lowercase().contains(q)
                        || entry.category.to_lowercase().contains(q)
                        || entry.keywords.iter().any(|k| k.to_lowercase().contains(q))
                } else {
                    true
                }
            })
            .collect();

        if filtered.is_empty() {
            let msg = format!(
                "No specialized tools matched query '{:?}' in category '{}'. Available categories: content, search, media, analysis, system, all.",
                query.unwrap_or_default(),
                category
            );
            return McpToolCallResult::success(msg);
        }

        let mut md = String::new();
        md.push_str(&format!(
            "### TkLess 动态工具发现目录 (匹配到 {} 个工具)\n\n",
            filtered.len()
        ));
        md.push_str("| 工具名称 (Tool) | 分类 (Category) | 运行状态 | 功能概述 (Summary) |\n");
        md.push_str("| :--- | :--- | :--- | :--- |\n");

        for entry in &filtered {
            let status = if entry.is_core {
                "核心常驻"
            } else if active_set.contains(entry.name) {
                "已动态激活"
            } else {
                "按需拉取"
            };
            md.push_str(&format!(
                "| `{}` | `{}` | {} | {} |\n",
                entry.name, entry.category, status, entry.summary
            ));
        }

        md.push_str("\n> **两阶段调用建议 (Usage Guidelines)**:\n");
        md.push_str("> 1. **单次即时调用**: 直接调用 `execute_tool(tool_name: \"...\", arguments: {...})` 立即执行。\n");
        md.push_str("> 2. **查看完整参数**: 调用 `get_tool_schema(tool_name: \"...\")` 获取 inputSchema 与参数描述。\n");
        md.push_str("> 3. **会话动态注册**: 调用 `activate_tools(tool_names: [\"...\"])` 加入 tools/list 并广播 list_changed。\n");

        McpToolCallResult::success(md)
    }

    async fn call_get_tool_schema(&self, args: &Value) -> McpToolCallResult {
        let tool_name = match args.get("tool_name").and_then(|v| v.as_str()) {
            Some(n) if !n.trim().is_empty() => n.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'tool_name'"),
        };

        let mut all_tools = get_available_tools();
        all_tools.extend(get_dynamic_discovery_tools());

        if let Some(tool) = all_tools.into_iter().find(|t| t.name == tool_name) {
            let schema_pretty =
                serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default();
            let mut response = format!(
                "### 工具参数定义: `{}`\n\n**描述**: {}\n\n**inputSchema**:\n```json\n{}\n```\n",
                tool.name, tool.description, schema_pretty
            );
            if let Some(ref output_schema) = tool.output_schema {
                response.push_str(&format!(
                    "\n**outputSchema**:\n```json\n{}\n```\n",
                    serde_json::to_string_pretty(output_schema).unwrap_or_default()
                ));
            }
            McpToolCallResult::success(response).with_structured(&tool)
        } else {
            McpToolCallResult::error(format!(
                "Tool '{tool_name}' not found. Call 'discover_tools' to search available tools."
            ))
        }
    }

    async fn call_activate_tools(
        &self,
        args: &Value,
        notifier: Option<&NotificationSender>,
    ) -> McpToolCallResult {
        let tool_names = match args.get("tool_names").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr,
            _ => return McpToolCallResult::error("Missing or empty 'tool_names' array"),
        };

        let available = get_available_tools();
        if tool_names.iter().any(|item| {
            !item
                .as_str()
                .is_some_and(|name| available.iter().any(|tool| tool.name == name))
        }) {
            return McpToolCallResult::invalid_argument(
                "tool_names must contain canonical business tool names from tools/list",
            );
        }
        let mut newly_activated = Vec::new();
        let mut active_set = self.active_dynamic_tools.write().await;
        for item in tool_names {
            if let Some(name) = item.as_str() {
                active_set.insert(name.to_string());
                newly_activated.push(name.to_string());
            }
        }

        drop(active_set);

        if let Some(tx) = notifier {
            let notif = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/tools/list_changed"
            });
            let _ = tx.send(notif).await;
        }

        let resp = format!(
            "成功激活工具: [{}]。已向 MCP 客户端广播 'notifications/tools/list_changed'。后续 tools/list 声明将包含这些工具，您也可以直接调用。",
            newly_activated.join(", ")
        );
        McpToolCallResult::success(resp)
    }

    async fn call_fetch_webpage(&self, args: &Value) -> McpToolCallResult {
        let raw_url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) if !u.trim().is_empty() => u.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'url'"),
        };
        let normalized_url = crate::adapters::bilibili::normalize_bilibili_url(raw_url);
        let url = &normalized_url;

        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            Some("text") => OutputFormat::Text,
            Some("html") => OutputFormat::Html,
            Some("raw") => OutputFormat::Raw,
            _ => OutputFormat::Markdown,
        };

        let mode = match args.get("mode").and_then(|v| v.as_str()) {
            Some("full") => ExtractionMode::Full,
            Some("raw") => ExtractionMode::Raw,
            _ => ExtractionMode::Article,
        };

        let max_tokens = args
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let frontmatter = args
            .get("frontmatter")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let no_images = args
            .get("no_images")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let no_links = args
            .get("no_links")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let no_metadata = args
            .get("no_metadata")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let no_adapter = args
            .get("no_adapter")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let use_cache = args
            .get("use_cache")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let tokenizer = match args.get("tokenizer").and_then(|v| v.as_str()) {
            Some("cl100k") => TokenizerKind::Cl100k,
            Some("heuristic") => TokenizerKind::Heuristic,
            _ => TokenizerKind::O200k,
        };

        let timeout = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(15);
        let connect_timeout = args
            .get("connect_timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(10);
        let retries = args.get("retries").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let on_too_large = match args.get("on_too_large").and_then(|v| v.as_str()) {
            Some("truncate") => OnTooLarge::Truncate,
            _ => OnTooLarge::Error,
        };
        let allow_private_addresses = args
            .get("allow_private_addresses")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let allow_http_errors = args
            .get("allow_http_errors")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let selector = args
            .get("selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let remove_selector = args
            .get("remove_selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let extract_links = args
            .get("extract_links")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let extract_images = args
            .get("extract_images")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let extract_tables = args
            .get("extract_tables")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let subtitles = args
            .get("subtitles")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let danmaku_limit = args
            .get("danmaku_limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let danmaku = args
            .get("danmaku")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || danmaku_limit.is_some();
        let comments_limit = args
            .get("comments_limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let comments = args
            .get("comments")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || comments_limit.is_some();
        let conclusion = args
            .get("conclusion")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let cookie = args
            .get("cookie")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let cookie_file = args
            .get("cookie_file")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let proxy = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let mobile = args
            .get("mobile")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let cdp_endpoint = args
            .get("cdp_endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let cdp_wait_ms = args.get("cdp_wait_ms").and_then(|v| v.as_u64());
        let cdp_block_resources = args
            .get("cdp_block_resources")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let cdp_auto_scroll = args
            .get("cdp_auto_scroll")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let schema = args.get("schema").map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            }
        });
        let chunk = args.get("chunk").and_then(|v| v.as_bool()).unwrap_or(false);
        let chunk_size = args
            .get("chunk_size")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let chunk_overlap = args
            .get("chunk_overlap")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let download_images = args
            .get("download_images")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let assets_dir = args
            .get("assets_dir")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let tables_only = args
            .get("tables_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let table_format = match args.get("table_format").and_then(|v| v.as_str()) {
            Some("csv") => TableExportFormat::Csv,
            Some("tsv") => TableExportFormat::Tsv,
            Some("json") => TableExportFormat::Json,
            Some("sql") => TableExportFormat::Sql,
            _ => TableExportFormat::Markdown,
        };
        let stats = args.get("stats").and_then(|v| v.as_bool()).unwrap_or(false);
        let auto_cdp = args
            .get("auto_cdp")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let summarize = args
            .get("summarize")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let options = ExtractionOptions {
            mode,
            format,
            max_tokens,
            tokenizer,
            include_metadata: !no_metadata,
            include_json_ld: matches!(format, OutputFormat::Json),
            strip_images: no_images,
            strip_links: no_links,
            frontmatter,
            no_adapter,
            feed: false,
            selector,
            extract_links,
            extract_images,
            extract_tables,
            subtitles,
            danmaku,
            danmaku_limit,
            comments,
            comments_limit,
            conclusion,
            cdp_endpoint,
            cdp_wait_ms,
            cdp_block_resources,
            cdp_auto_scroll,
            schema,
            schema_strict: args
                .get("schema_strict")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            chunk,
            chunk_size,
            chunk_overlap,
            download_images,
            assets_dir,
            tables_only,
            table_format,
            stats,
            remove_selector,
            auto_cdp,
            summarize,
            skills_pages: args
                .get("skills_pages")
                .and_then(Value::as_u64)
                .unwrap_or(3) as usize,
            skills_limit: args
                .get("skills_limit")
                .and_then(Value::as_u64)
                .unwrap_or(1000) as usize,
        };

        let initial_target_host = Url::parse(url.as_str())
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_lowercase()));

        let fetch_options = FetchOptions {
            user_agent: None,
            timeout_secs: timeout,
            connect_timeout_secs: connect_timeout,
            retries,
            max_bytes: 10 * 1024 * 1024,
            on_too_large,
            headers: Vec::new(),
            cookie,
            cookie_file,
            initial_target_host,
            allow_private_addresses,
            allow_http_errors,
            proxy,
            mobile,
        };

        let engine = match TklessEngine::with_options(fetch_options) {
            Ok(eng) => eng,
            Err(e) => return McpToolCallResult::error(format!("Failed to initialize engine: {e}")),
        };

        let doc = if use_cache {
            match engine
                .process_url_with_cache(url, &options, &self.cache)
                .await
            {
                Ok(d) => d,
                Err(e) => return McpToolCallResult::from_error(&e),
            }
        } else {
            match engine.process_url(url, &options).await {
                Ok(d) => d,
                Err(e) => return McpToolCallResult::from_error(&e),
            }
        };

        format_doc_result(&doc, format)
    }

    async fn call_fetch_bilibili_feed(&self, args: &Value) -> McpToolCallResult {
        let feed_type = args
            .get("feed_type")
            .and_then(|v| v.as_str())
            .unwrap_or("recommendations");
        let partition = args
            .get("partition")
            .and_then(|v| v.as_str())
            .unwrap_or("all");
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };
        let max_tokens = args
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let use_cache = args
            .get("use_cache")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let (target_url, feed) = match feed_type {
            "ranking" => {
                if partition == "all" {
                    (
                        "https://www.bilibili.com/v/popular/rank/all".to_string(),
                        false,
                    )
                } else {
                    (
                        format!("https://www.bilibili.com/v/popular/rank/{partition}"),
                        false,
                    )
                }
            }
            "popular" => ("https://www.bilibili.com/v/popular/all".to_string(), false),
            _ => ("https://www.bilibili.com/".to_string(), true),
        };

        let options = ExtractionOptions {
            mode: ExtractionMode::Article,
            format,
            max_tokens,
            tokenizer: TokenizerKind::default(),
            include_metadata: true,
            include_json_ld: matches!(format, OutputFormat::Json),
            strip_images: false,
            strip_links: false,
            frontmatter: false,
            no_adapter: false,
            feed,
            ..Default::default()
        };

        let cache_key = if use_cache {
            Some(self.engine.cache_key(&self.cache, &target_url, &options))
        } else {
            None
        };

        if let Some(ref key) = cache_key {
            if let Some(cached_doc) = self.cache.get(key) {
                return format_doc_result(&cached_doc, format);
            }
        }

        match self.engine.process_url(&target_url, &options).await {
            Ok(doc) => {
                if let Some(ref key) = cache_key {
                    let _ = self.cache.set(key, &doc, None);
                }
                format_doc_result(&doc, format)
            }
            Err(e) => McpToolCallResult::error(format!("Failed to fetch Bilibili feed: {e}")),
        }
    }

    async fn call_search_bilibili(&self, args: &Value) -> McpToolCallResult {
        let keyword = match args.get("keyword").and_then(|v| v.as_str()) {
            Some(k) if !k.trim().is_empty() => k.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'keyword'"),
        };

        let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
        let order = args.get("order").and_then(|v| v.as_str()).unwrap_or("");
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };
        let use_cache = args
            .get("use_cache")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let encoded_kw: String = url::form_urlencoded::byte_serialize(keyword.as_bytes()).collect();
        let mut target_url =
            format!("https://search.bilibili.com/all?keyword={encoded_kw}&page={page}");
        if !order.is_empty() {
            target_url.push_str(&format!("&order={order}"));
        }

        let options = ExtractionOptions {
            mode: ExtractionMode::Article,
            format,
            max_tokens: None,
            tokenizer: TokenizerKind::default(),
            include_metadata: true,
            include_json_ld: matches!(format, OutputFormat::Json),
            strip_images: false,
            strip_links: false,
            frontmatter: false,
            no_adapter: false,
            feed: false,
            ..Default::default()
        };

        let cache_key = if use_cache {
            Some(self.engine.cache_key(&self.cache, &target_url, &options))
        } else {
            None
        };

        if let Some(ref key) = cache_key {
            if let Some(cached_doc) = self.cache.get(key) {
                return format_doc_result(&cached_doc, format);
            }
        }

        match self.engine.process_url(&target_url, &options).await {
            Ok(doc) => {
                if let Some(ref key) = cache_key {
                    let _ = self.cache.set(key, &doc, None);
                }
                format_doc_result(&doc, format)
            }
            Err(e) => McpToolCallResult::error(format!("Failed to search Bilibili: {e}")),
        }
    }

    async fn call_download_media(&self, args: &Value) -> McpToolCallResult {
        let url_or_bvid = match args.get("url_or_bvid").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'url_or_bvid'"),
        };

        let media_type = match args.get("media_type").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => {
                return McpToolCallResult::error(
                    "Missing required parameter 'media_type' ('video' or 'cover')",
                )
            }
        };

        let destination = args
            .get("destination")
            .and_then(|v| v.as_str())
            .map(Path::new);
        let quality = match args.get("quality").and_then(|v| v.as_str()) {
            Some("480p") => VideoQuality::Q480p,
            Some("360p") => VideoQuality::Q360p,
            _ => VideoQuality::Q720p,
        };
        let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(1) as usize;

        let bvid = crate::adapters::bilibili::extract_video_info(url_or_bvid)
            .map(|(b, _)| b)
            .unwrap_or_else(|| url_or_bvid.to_string());

        match media_type {
            "cover" => {
                match BilibiliAdapter::download_cover(
                    self.engine.fetcher(),
                    &bvid,
                    destination,
                    None,
                )
                .await
                {
                    Ok(path) => McpToolCallResult::success(format!(
                        "Cover image saved successfully to: {}",
                        path.display()
                    )),
                    Err(e) => McpToolCallResult::error(format!("Failed to download cover: {e}")),
                }
            }
            "video" => {
                match BilibiliAdapter::download_video(
                    self.engine.fetcher(),
                    &bvid,
                    page,
                    quality,
                    destination,
                    None,
                )
                .await
                {
                    Ok(path) => McpToolCallResult::success(format!(
                        "Video stream saved successfully to: {}",
                        path.display()
                    )),
                    Err(e) => McpToolCallResult::error(format!("Failed to download video: {e}")),
                }
            }
            _ => McpToolCallResult::error(format!(
                "Invalid media_type '{media_type}'. Expected 'video' or 'cover'."
            )),
        }
    }

    async fn call_web_search(&self, args: &Value) -> McpToolCallResult {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'query'"),
        };

        let limit = match args.get("limit") {
            None => 10,
            Some(value) => match value.as_u64().filter(|value| (1..=20).contains(value)) {
                Some(value) => value as usize,
                None => {
                    return McpToolCallResult::error("limit must be an integer between 1 and 20")
                }
            },
        };
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };
        let scope = match args.get("scope") {
            None => None,
            Some(Value::String(value)) => Some(value.as_str()),
            _ => return McpToolCallResult::error("scope must be a platform name"),
        };
        let effective_scope = scope;
        let effective_query = query.to_string();

        let fetch_content = args
            .get("fetch_content")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_tokens_per_page = args
            .get("max_tokens_per_page")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let proxy = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let mobile = args
            .get("mobile")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let dedup = args.get("dedup").and_then(|v| v.as_bool()).unwrap_or(false);

        let allow_private = args.get("allow_private_addresses").and_then(Value::as_bool);
        let custom_engine;
        let engine_ref = if proxy.is_some() || mobile || allow_private.is_some() {
            let mut fetch_opts = self.engine.fetcher().options().clone();
            if let Some(ref p) = proxy {
                fetch_opts.proxy = Some(p.clone());
            }
            if mobile {
                fetch_opts.mobile = true;
            }
            if let Some(allow) = allow_private {
                fetch_opts.allow_private_addresses = allow;
            }
            match TklessEngine::with_options(fetch_opts) {
                Ok(eng) => {
                    custom_engine = eng;
                    &custom_engine
                }
                Err(error) => return McpToolCallResult::error(error.to_string()),
            }
        } else {
            &self.engine
        };

        let options = ExtractionOptions {
            cdp_endpoint: args
                .get("cdp_endpoint")
                .and_then(Value::as_str)
                .map(str::to_owned),
            cdp_wait_ms: args.get("cdp_wait_ms").and_then(Value::as_u64),
            auto_cdp: args
                .get("auto_cdp")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            schema: args.get("schema").map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            }),
            schema_strict: args
                .get("schema_strict")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ..Default::default()
        };
        if let Err(error) = options.validate() {
            return McpToolCallResult::from_error(&error);
        }
        let cdp = options
            .cdp_endpoint
            .as_ref()
            .map(|endpoint| crate::CdpOptions {
                endpoint: endpoint.clone(),
                wait_ms: options.cdp_wait_ms,
                ..Default::default()
            });
        let report = match crate::web_search(
            engine_ref,
            &effective_query,
            &crate::SearchOptions {
                scope: effective_scope.map(str::to_owned),
                limit,
                cdp,
                auto_cdp: options.auto_cdp,
            },
        )
        .await
        {
            Ok(report) => report,
            Err(error) => return McpToolCallResult::from_error(&error),
        };
        let diagnostics = args
            .get("diagnostics")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let diagnostic_md = if diagnostics {
            report.markdown_diagnostics()
        } else {
            String::new()
        };
        let (structured, text) = if fetch_content {
            let items = match crate::search::fetch_search_results(
                engine_ref,
                report.results.clone(),
                &options,
                max_tokens_per_page,
                dedup,
            )
            .await
            {
                Ok(items) => items,
                Err(error) => return McpToolCallResult::from_error(&error),
            };
            let md = crate::search::format_search_fetch_markdown(&effective_query, &items)
                + &diagnostic_md;
            let text = md;
            let value = serde_json::json!({"query":report.query,"effective_query":report.effective_query,"status":report.status,
                "results":items,"diagnostics":report.diagnostics,"warnings":report.warnings});
            (value, text)
        } else {
            let text =
                crate::search::format_search_results_markdown(&effective_query, &report.results)
                    + &diagnostic_md;
            (
                serde_json::to_value(&report).expect("SearchReport is serializable"),
                text,
            )
        };
        let text = if format == OutputFormat::Json {
            serde_json::to_string_pretty(&structured).unwrap_or_default()
        } else {
            text
        };
        McpToolCallResult::success(text).with_structured(structured)
    }

    async fn call_deep_research(&self, args: &Value) -> McpToolCallResult {
        let topic = match args.get("topic").and_then(|v| v.as_str()) {
            Some(t) if !t.trim().is_empty() => t.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'topic'"),
        };

        let depth = args
            .get("depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(2)
            .clamp(1, 2) as usize;
        let breadth = args
            .get("breadth")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .clamp(2, 5) as usize;
        let max_pages = args
            .get("max_pages")
            .and_then(|v| v.as_u64())
            .unwrap_or(8)
            .clamp(3, 20) as usize;
        let max_tokens_per_page = args
            .get("max_tokens_per_page")
            .and_then(|v| v.as_u64())
            .unwrap_or(1500) as usize;
        let max_tokens = args
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .or(Some(8000));
        let dedup = args.get("dedup").and_then(|v| v.as_bool()).unwrap_or(true);
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };

        let options = crate::research::ResearchOptions {
            depth,
            breadth,
            max_pages,
            max_tokens_per_page,
            max_tokens,
            dedup,
            format,
        };

        match crate::research::deep_research(&self.engine, topic, &options).await {
            Ok(report) => match crate::research::render_deep_research(&report, &options) {
                Ok(text) => McpToolCallResult::success(text),
                Err(e) => McpToolCallResult::error(e.to_string()),
            },
            Err(e) => McpToolCallResult::error(format!("Deep research failed: {e}")),
        }
    }

    async fn call_crawl_site(
        &self,
        args: &Value,
        notifier: Option<&NotificationSender>,
        progress_token: Option<&Value>,
    ) -> McpToolCallResult {
        let seed_url = match args.get("seed_url").and_then(|v| v.as_str()) {
            Some(u) if !u.trim().is_empty() => u.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'seed_url'"),
        };

        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(2)
            .clamp(0, 5) as usize;
        let max_pages = args
            .get("max_pages")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .clamp(1, 100) as usize;
        let same_path_prefix = args
            .get("same_path_prefix")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let sitemap = args
            .get("sitemap")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let dedup = args.get("dedup").and_then(|v| v.as_bool()).unwrap_or(false);

        let is_jsonl = args.get("format").and_then(|v| v.as_str()) == Some("jsonl");
        let delay_ms = args.get("delay_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };

        let max_tokens = args
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let cdp_endpoint = args
            .get("cdp_endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let cdp_wait_ms = args.get("cdp_wait_ms").and_then(|v| v.as_u64());
        let proxy = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let mobile = args
            .get("mobile")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let schema = args.get("schema").map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            }
        });

        let crawl_options = crate::crawler::CrawlOptions {
            max_depth,
            max_pages,
            concurrency: args
                .get("concurrency")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .clamp(1, 32) as usize,
            same_domain_only: true,
            same_path_prefix,
            exclude_patterns: crate::crawler::CrawlOptions::default().exclude_patterns,
            sitemap,
            dedup,
            delay_ms,
        };

        let extraction_options = ExtractionOptions {
            cdp_endpoint,
            cdp_wait_ms,
            schema,
            schema_strict: args
                .get("schema_strict")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ..Default::default()
        };

        let custom_engine;
        let engine_ref = if proxy.is_some() || mobile {
            let mut fetch_opts = self.engine.fetcher().options().clone();
            if let Some(ref p) = proxy {
                fetch_opts.proxy = Some(p.clone());
            }
            if mobile {
                fetch_opts.mobile = true;
            }
            match TklessEngine::with_options(fetch_opts) {
                Ok(eng) => {
                    custom_engine = eng;
                    &custom_engine
                }
                Err(_) => &self.engine,
            }
        } else {
            &self.engine
        };

        let notifier_clone = notifier.cloned();
        let progress_token_clone = progress_token.cloned();

        let on_progress = if let (Some(tx), Some(token)) = (notifier_clone, progress_token_clone) {
            Some(
                move |crawled_count: usize, max_pages: usize, current_url: &str| {
                    let notif = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/progress",
                        "params": {
                            "progressToken": token,
                            "progress": crawled_count,
                            "total": max_pages,
                            "url": current_url
                        }
                    });
                    let _ = tx.try_send(notif);
                },
            )
        } else {
            None
        };

        match crate::crawler::crawl_site_with_progress(
            engine_ref,
            seed_url,
            &crawl_options,
            &extraction_options,
            on_progress,
        )
        .await
        {
            Ok(report) => {
                let mut payload = if is_jsonl {
                    crate::crawler::format_crawl_report_jsonl(&report)
                } else if format == OutputFormat::Json {
                    match serde_json::to_string(&report) {
                        Ok(text) => text,
                        Err(e) => return McpToolCallResult::error(e.to_string()),
                    }
                } else {
                    crate::crawler::format_crawl_report_markdown(&report)
                };
                if let Some(limit) = max_tokens {
                    if (is_jsonl || format == OutputFormat::Json)
                        && crate::token::count_tokens(&payload, TokenizerKind::default()) > limit
                    {
                        return McpToolCallResult::error("JSON report exceeds max_tokens; increase the budget or reduce max_pages");
                    }
                    if !is_jsonl && format != OutputFormat::Json {
                        payload = crate::token::truncate_to_tokens(
                            &payload,
                            limit,
                            TokenizerKind::default(),
                        )
                        .0;
                    }
                }
                if !report.pages.is_empty() && report.pages.iter().all(|page| page.error.is_some())
                {
                    McpToolCallResult::error(payload)
                } else {
                    McpToolCallResult::success(payload)
                }
            }
            Err(e) => McpToolCallResult::error(format!("Site crawl failed: {e}")),
        }
    }

    async fn call_batch_fetch_webpages(
        &self,
        args: &Value,
        notifier: Option<&NotificationSender>,
        progress_token: Option<&Value>,
    ) -> McpToolCallResult {
        let urls = match args.get("urls").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .collect::<Vec<_>>(),
            _ => return McpToolCallResult::error("Missing or empty 'urls' array"),
        };

        if urls.is_empty() || urls.iter().any(String::is_empty) {
            return McpToolCallResult::invalid_argument(
                "'urls' entries must be non-empty URL strings",
            );
        }

        let concurrency = args
            .get("concurrency")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .clamp(1, 20) as usize;

        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };

        let max_tokens = args
            .get("max_tokens_per_page")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let selector = args
            .get("selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let remove_selector = args
            .get("remove_selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let delay_ms = args.get("delay_ms").and_then(|v| v.as_u64()).unwrap_or(0);

        let schema = args.get("schema").map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            }
        });

        let cdp_endpoint = args
            .get("cdp_endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let cdp_wait_ms = args.get("cdp_wait_ms").and_then(|v| v.as_u64());
        let proxy = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let mobile = args
            .get("mobile")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let custom_engine;
        let engine_ref = if proxy.is_some() || mobile {
            let mut fetch_opts = self.engine.fetcher().options().clone();
            if let Some(ref p) = proxy {
                fetch_opts.proxy = Some(p.clone());
            }
            if mobile {
                fetch_opts.mobile = true;
            }
            match TklessEngine::with_options(fetch_opts) {
                Ok(eng) => {
                    custom_engine = eng;
                    std::sync::Arc::new(custom_engine)
                }
                Err(_) => self.engine.clone(),
            }
        } else {
            self.engine.clone()
        };

        let base_options = ExtractionOptions {
            format,
            max_tokens,
            selector,
            remove_selector,
            schema,
            schema_strict: args
                .get("schema_strict")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cdp_endpoint,
            cdp_wait_ms,
            ..Default::default()
        };

        let job = crate::batch::BatchOptions {
            concurrency,
            delay_ms,
            per_host_concurrency: args
                .get("per_host_concurrency")
                .and_then(Value::as_u64)
                .unwrap_or(2) as usize,
            per_host_delay_ms: args
                .get("per_host_delay_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            order: if args.get("order").and_then(Value::as_str) == Some("completion") {
                crate::batch::BatchOrder::Completion
            } else {
                crate::batch::BatchOrder::Input
            },
            checkpoint: args
                .get("checkpoint")
                .and_then(Value::as_str)
                .map(std::path::PathBuf::from),
            resume: args.get("resume").and_then(Value::as_bool).unwrap_or(false),
            retry_failed: args
                .get("retry_failed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        let stream_results = args
            .get("stream_results")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if stream_results && (notifier.is_none() || progress_token.is_none()) {
            return McpToolCallResult::invalid_argument(
                "stream_results requires an active notification receiver and _meta.progressToken",
            );
        }
        let mut completed = 0usize;
        let total = urls.len();
        let mut results = Vec::new();
        let summary = crate::batch::run_batch(&engine_ref, &urls, &base_options, &job, |item| {
            completed += 1;
            let notification = if stream_results || completed == total || completed.is_multiple_of(total.div_ceil(10).max(1)) {
                progress_token.map(|token| {
                    let mut params = serde_json::json!({"progressToken":token,"progress":completed,"total":total,"message":item.url});
                    if stream_results { params["_meta"] = serde_json::json!({"tkless/batch_item":item}); }
                    serde_json::json!({"jsonrpc":"2.0","method":"notifications/progress","params":params})
                })
            } else { None };
            if !stream_results { results.push(item); }
            let tx = notifier.cloned();
            async move {
                if let (Some(tx), Some(notification)) = (tx, notification) {
                    tx.send(notification).await.map_err(|_| TklessError::McpError { code: INTERNAL_ERROR, message: "Batch notification receiver disconnected".into() })?;
                }
                Ok(())
            }
        }).await;
        let summary = match summary {
            Ok(summary) => summary,
            Err(error) => return McpToolCallResult::from_error(&error),
        };
        let failed = summary.failed > 0 && summary.succeeded == 0;
        let (structured, markdown) = if stream_results {
            let mut value = serde_json::to_value(&summary).expect("BatchSummary is serializable");
            value["streamed"] = serde_json::json!(true);
            let markdown = format!("Batch finished: {} total, {} succeeded, {} failed, {} restored, {} fetched. Results were streamed through progress notifications.", summary.total, summary.succeeded, summary.failed, summary.resumed, summary.fetched);
            (value, markdown)
        } else {
            let report = crate::batch::BatchReport { summary, results };
            (
                serde_json::to_value(&report).expect("BatchReport is serializable"),
                report.markdown(),
            )
        };
        let text = if format == OutputFormat::Json {
            serde_json::to_string_pretty(&structured).unwrap_or_default()
        } else {
            markdown
        };
        let response = if failed {
            McpToolCallResult::error(text)
        } else {
            McpToolCallResult::success(text)
        };
        response.with_structured(structured)
    }

    async fn call_clean_html(&self, args: &Value) -> McpToolCallResult {
        let html = match args.get("html").and_then(|v| v.as_str()) {
            Some(h) if !h.trim().is_empty() => h,
            _ => return McpToolCallResult::error("Missing required parameter 'html'"),
        };

        let base_url = args.get("base_url").and_then(|v| v.as_str());

        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            Some("text") => OutputFormat::Text,
            Some("html") => OutputFormat::Html,
            _ => OutputFormat::Markdown,
        };

        let mode = match args.get("mode").and_then(|v| v.as_str()) {
            Some("full") => ExtractionMode::Full,
            Some("raw") => ExtractionMode::Raw,
            _ => ExtractionMode::Article,
        };

        let selector = args
            .get("selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let remove_selector = args
            .get("remove_selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let schema = args.get("schema").map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            }
        });

        let max_tokens = args
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let strip_images = args
            .get("strip_images")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let strip_links = args
            .get("strip_links")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let frontmatter = args
            .get("frontmatter")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let chunk = args.get("chunk").and_then(|v| v.as_bool()).unwrap_or(false);
        let chunk_size = args
            .get("chunk_size")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let chunk_overlap = args
            .get("chunk_overlap")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let tables_only = args
            .get("tables_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let table_format = match args.get("table_format").and_then(|v| v.as_str()) {
            Some("csv") => TableExportFormat::Csv,
            Some("tsv") => TableExportFormat::Tsv,
            Some("json") => TableExportFormat::Json,
            Some("sql") => TableExportFormat::Sql,
            _ => TableExportFormat::Markdown,
        };
        let stats = args.get("stats").and_then(|v| v.as_bool()).unwrap_or(false);
        let summarize = args
            .get("summarize")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let options = ExtractionOptions {
            mode,
            format,
            selector,
            schema,
            schema_strict: args
                .get("schema_strict")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            max_tokens,
            strip_images,
            strip_links,
            frontmatter,
            chunk,
            chunk_size,
            chunk_overlap,
            tables_only,
            table_format,
            stats,
            remove_selector,
            summarize,
            ..Default::default()
        };

        match self.engine.process_html(html, base_url, &options) {
            Ok(doc) => format_doc_result(&doc, format),
            Err(e) => McpToolCallResult::from_error(&e),
        }
    }

    async fn call_login_platform(&self, args: &Value) -> McpToolCallResult {
        let platform = match args.get("platform").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.trim().to_ascii_lowercase(),
            _ => {
                return McpToolCallResult::error(
                    "Missing required parameter 'platform' ('bilibili' or 'zhihu')",
                )
            }
        };

        let output_file = args
            .get("output_cookie_file")
            .and_then(|v| v.as_str())
            .unwrap_or("./cookies.txt");

        let no_browser = args
            .get("no_browser")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(180);

        match platform.as_str() {
            "bilibili" | "bili" => {
                let login_opts = crate::auth::LoginOptions {
                    output_path: std::path::PathBuf::from(output_file),
                    no_browser,
                    timeout_secs,
                };
                match crate::auth::execute_login_flow(Some(self.engine.client()), login_opts).await
                {
                    Ok(cookie_str) => McpToolCallResult::success(format!(
                        "Bilibili 登录成功！凭据已自动以 'bilibili: ...' 格式保存至 `{output_file}`。\n凭据摘要: 长度 {} 字符",
                        cookie_str.len()
                    )),
                    Err(e) => McpToolCallResult::error(format!("Bilibili 扫码登录失败: {e}")),
                }
            }
            "zhihu" => {
                let msg = format!(
                    "知乎登录说明：\n1. 请在本地浏览器访问授权登录页：https://www.zhihu.com/signin\n2. 登录成功后，从浏览器开发者工具 (F12 -> Application -> Cookies) 复制 Cookie 字符串\n3. 将凭据以 'zhihu: ...' 前缀格式追加到 `{output_file}`：\n```\nzhihu: d_c0=\"...\"; z_c0=\"...\"\n```\n4. 保存后 TkLess 将自动识别知乎会话凭据进行鉴权访问。"
                );
                McpToolCallResult::success(msg)
            }
            other => McpToolCallResult::error(format!(
                "Unsupported platform '{other}'. Currently supported: 'bilibili', 'zhihu'"
            )),
        }
    }

    async fn call_scaffold_schema(&self, args: &Value) -> McpToolCallResult {
        let raw_html = if let Some(html) = args.get("html").and_then(|v| v.as_str()) {
            html.to_string()
        } else if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
            let parsed_url = match url::Url::parse(url) {
                Ok(u) => u,
                Err(e) => return McpToolCallResult::error(format!("Invalid URL: {e}")),
            };

            if let Err(e) = self.engine.fetcher().validate_ssrf(&parsed_url).await {
                return McpToolCallResult::error(format!("SSRF blocked: {e}"));
            }

            if let Some(cdp_endpoint) = args.get("cdp_endpoint").and_then(|v| v.as_str()) {
                let cdp_opts = crate::cdp::CdpOptions {
                    endpoint: cdp_endpoint.to_string(),
                    wait_ms: Some(1000),
                    timeout_ms: None,
                    user_agent: self.engine.fetcher().options().user_agent.clone(),
                    cookie: self.engine.fetcher().resolve_cookie_for_url(&parsed_url),
                    ..Default::default()
                };
                match crate::cdp::fetch_html_via_cdp(url, &cdp_opts).await {
                    Ok(h) => h,
                    Err(e) => return McpToolCallResult::error(format!("CDP fetch failed: {e}")),
                }
            } else {
                match self.engine.fetcher().fetch(url).await {
                    Ok(resp) => resp.html,
                    Err(e) => return McpToolCallResult::error(format!("Fetch URL failed: {e}")),
                }
            }
        } else {
            return McpToolCallResult::error(
                "Either 'url' or 'html' parameter must be provided to scaffold_schema",
            );
        };

        let report = crate::schema::scaffold_dom_skeleton(&raw_html);
        McpToolCallResult::success(report.summary_markdown)
    }

    async fn call_compare_similarity(&self, args: &Value) -> McpToolCallResult {
        let text1 = match args.get("text1").and_then(|v| v.as_str()) {
            Some(t) if !t.trim().is_empty() => t,
            _ => return McpToolCallResult::error("Missing required parameter 'text1'"),
        };
        let text2 = match args.get("text2").and_then(|v| v.as_str()) {
            Some(t) if !t.trim().is_empty() => t,
            _ => return McpToolCallResult::error("Missing required parameter 'text2'"),
        };

        let max_dist = args
            .get("max_hamming_distance")
            .and_then(|v| v.as_u64())
            .unwrap_or(3) as u32;

        let hash1 = crate::dedup::simhash(text1);
        let hash2 = crate::dedup::simhash(text2);
        let dist = crate::dedup::hamming_distance(hash1, hash2);
        let is_duplicate = dist <= max_dist;
        let similarity_pct = (1.0 - (dist as f64 / 64.0)) * 100.0;

        let res = serde_json::json!({
            "simhash1": format!("0x{:016x}", hash1),
            "simhash2": format!("0x{:016x}", hash2),
            "hamming_distance": dist,
            "threshold": max_dist,
            "is_near_duplicate": is_duplicate,
            "similarity_percentage": (similarity_pct * 100.0).round() / 100.0
        });

        match serde_json::to_string_pretty(&res) {
            Ok(json_str) => McpToolCallResult::success(json_str),
            Err(e) => McpToolCallResult::error(format!("Failed to serialize result: {e}")),
        }
    }

    async fn call_manage_cache(&self, args: &Value) -> McpToolCallResult {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("stats");
        match action {
            "clear" => match self.cache.clear() {
                Ok(_) => McpToolCallResult::success("TkLess 本地响应缓存已成功清空。"),
                Err(e) => McpToolCallResult::error(format!("Failed to clear cache: {e}")),
            },
            "prune_expired" => {
                let pruned = self.cache.prune_expired();
                McpToolCallResult::success(format!("已成功清理 {pruned} 条过期缓存。"))
            }
            _ => {
                let stats = self.cache.stats();
                let res = serde_json::json!({
                    "memory_entries": stats.memory_entries,
                    "disk_entries": stats.disk_entries,
                    "disk_bytes": stats.disk_bytes,
                    "disk_human": crate::downloader::format_bytes(stats.disk_bytes),
                    "cache_dir": stats.cache_dir
                });
                match serde_json::to_string_pretty(&res) {
                    Ok(json_str) => McpToolCallResult::success(json_str),
                    Err(e) => McpToolCallResult::error(format!("Serialization error: {e}")),
                }
            }
        }
    }

    async fn call_fetch_feed(&self, args: &Value) -> McpToolCallResult {
        let channel = if let Some(raw_xml) = args.get("xml").and_then(|v| v.as_str()) {
            crate::feed::parse_feed(raw_xml)
        } else if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
            let parsed_url = match url::Url::parse(url) {
                Ok(u) => u,
                Err(e) => return McpToolCallResult::error(format!("Invalid URL: {e}")),
            };
            if let Err(e) = self.engine.fetcher().validate_ssrf(&parsed_url).await {
                return McpToolCallResult::error(format!("SSRF blocked: {e}"));
            }
            match self.engine.fetcher().fetch(url).await {
                Ok(resp) => crate::feed::parse_feed(&resp.html),
                Err(e) => return McpToolCallResult::error(format!("Failed to fetch feed: {e}")),
            }
        } else {
            return McpToolCallResult::error(
                "Either 'url' or 'xml' parameter must be provided to fetch_feed",
            );
        };

        let mut feed_channel = match channel {
            Ok(c) => c,
            Err(e) => {
                return McpToolCallResult::error(format!("Failed to parse syndication feed: {e}"))
            }
        };

        if let Some(limit) = args.get("limit").and_then(|v| v.as_u64()) {
            feed_channel.items.truncate(limit as usize);
        }

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");
        match format {
            "json" => match serde_json::to_string_pretty(&feed_channel) {
                Ok(json_str) => McpToolCallResult::success(json_str),
                Err(e) => {
                    McpToolCallResult::error(format!("Failed to serialize feed to JSON: {e}"))
                }
            },
            _ => McpToolCallResult::success(crate::feed::format_feed_markdown(&feed_channel)),
        }
    }

    async fn call_inspect_links(&self, args: &Value) -> McpToolCallResult {
        let urls = match args.get("urls").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>(),
            _ => return McpToolCallResult::error("Missing or empty 'urls' array"),
        };

        let concurrency = args
            .get("concurrency")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 30) as usize;

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 60);

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
        let client = self.engine.client().clone();
        let fetcher = self.engine.fetcher().clone();

        let mut join_set = tokio::task::JoinSet::new();

        for (idx, target_url) in urls.into_iter().enumerate() {
            let sem = semaphore.clone();
            let cli = client.clone();
            let parsed_res = Url::parse(&target_url);

            let ssrf_res = if let Ok(ref p_url) = parsed_res {
                fetcher.validate_ssrf(p_url).await
            } else {
                Err(TklessError::InvalidArgument(format!(
                    "Invalid URL: {target_url}"
                )))
            };

            join_set.spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let start = std::time::Instant::now();

                if let Err(e) = ssrf_res {
                    return (
                        idx,
                        target_url,
                        0u16,
                        start.elapsed().as_millis() as u64,
                        None::<String>,
                        Some(format!("SSRF Blocked: {e}")),
                    );
                }

                let req_timeout = std::time::Duration::from_secs(timeout_secs);
                let resp_res = cli.head(&target_url).timeout(req_timeout).send().await;

                let (status, final_url, err) = match resp_res {
                    Ok(resp) => {
                        let st = resp.status().as_u16();
                        let redirect_target = if (300..400).contains(&st) {
                            resp.headers()
                                .get(reqwest::header::LOCATION)
                                .and_then(|h| h.to_str().ok())
                                .and_then(|loc| {
                                    Url::parse(&target_url)
                                        .ok()
                                        .and_then(|base| base.join(loc).ok())
                                })
                                .map(|u| u.to_string())
                        } else {
                            None
                        };
                        let final_dest = redirect_target.or_else(|| {
                            if resp.url().as_str() != target_url {
                                Some(resp.url().to_string())
                            } else {
                                None
                            }
                        });
                        (st, final_dest, None)
                    }
                    Err(_) => {
                        let get_res = cli
                            .get(&target_url)
                            .header("Range", "bytes=0-100")
                            .timeout(req_timeout)
                            .send()
                            .await;

                        match get_res {
                            Ok(resp) => {
                                let st = resp.status().as_u16();
                                let redirect_target = if (300..400).contains(&st) {
                                    resp.headers()
                                        .get(reqwest::header::LOCATION)
                                        .and_then(|h| h.to_str().ok())
                                        .and_then(|loc| {
                                            Url::parse(&target_url)
                                                .ok()
                                                .and_then(|base| base.join(loc).ok())
                                        })
                                        .map(|u| u.to_string())
                                } else {
                                    None
                                };
                                let final_dest = redirect_target.or_else(|| {
                                    if resp.url().as_str() != target_url {
                                        Some(resp.url().to_string())
                                    } else {
                                        None
                                    }
                                });
                                (st, final_dest, None)
                            }
                            Err(e) => (0, None, Some(e.to_string())),
                        }
                    }
                };

                let latency = start.elapsed().as_millis() as u64;
                (idx, target_url, status, latency, final_url, err)
            });
        }

        let mut results = Vec::new();
        while let Some(res) = join_set.join_next().await {
            if let Ok(item) = res {
                results.push(item);
            }
        }
        results.sort_by_key(|(idx, ..)| *idx);

        if format == "json" {
            let json_records: Vec<_> = results
                .into_iter()
                .map(|(_, url, status, latency_ms, final_url, error)| {
                    serde_json::json!({
                        "url": url,
                        "status": status,
                        "latency_ms": latency_ms,
                        "is_reachable": (200..400).contains(&status),
                        "is_redirect": final_url.is_some(),
                        "final_url": final_url,
                        "error": error
                    })
                })
                .collect();

            match serde_json::to_string_pretty(&json_records) {
                Ok(s) => McpToolCallResult::success(s),
                Err(e) => McpToolCallResult::error(format!("Serialization error: {e}")),
            }
        } else {
            let mut md = String::new();
            md.push_str("# 链接健康度与状态巡检报告 (Link Health Inspection)\n\n");
            md.push_str("| # | 目标链接 (URL) | 状态码 (Status) | 耗时 (Latency) | 重定向跳转 (Redirect) | 结果 (Health) |\n");
            md.push_str("|---|---|---|---|---|---|\n");

            for (idx, url, status, latency, final_url, err) in results {
                let status_str = if status > 0 {
                    format!("{status}")
                } else {
                    "Error".to_string()
                };

                let redirect_str = if let Some(dest) = final_url {
                    format!("[跳转]({dest})")
                } else {
                    "-".to_string()
                };

                let health_str = if (200..400).contains(&status) {
                    "可访问 (OK)".to_string()
                } else if let Some(e) = err {
                    format!("异常 ({e})")
                } else {
                    format!("异常 ({status})")
                };

                md.push_str(&format!(
                    "| {} | `{url}` | {} | {}ms | {} | {} |\n",
                    idx + 1,
                    status_str,
                    latency,
                    redirect_str,
                    health_str
                ));
            }

            McpToolCallResult::success(md)
        }
    }

    async fn call_check_auth(&self, args: &Value) -> McpToolCallResult {
        let platform = match args.get("platform").and_then(|v| v.as_str()) {
            Some(p) => p.trim().to_lowercase(),
            None => return McpToolCallResult::error("Missing required parameter 'platform'"),
        };

        let cookie_file = args
            .get("cookie_file")
            .and_then(|v| v.as_str())
            .unwrap_or("./cookies.txt");

        let cli_cookie = args
            .get("cookie")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let target_test_url = match platform.as_str() {
            "bilibili" => "https://www.bilibili.com/",
            "zhihu" => "https://www.zhihu.com/",
            "github" => "https://github.com/",
            "wechat" => "https://mp.weixin.qq.com/",
            other => {
                return McpToolCallResult::error(format!(
                    "Unsupported platform '{other}'. Currently supported: 'bilibili', 'zhihu', 'github', 'wechat'"
                ))
            }
        };

        let resolved_cookie = if let Some(ref c) = cli_cookie {
            Some(c.clone())
        } else {
            let path = std::path::Path::new(cookie_file);
            crate::fetcher::load_cookie_from_file_for_url(path, Some(target_test_url)).ok()
        };

        let cookie_str = match resolved_cookie {
            Some(c) if !c.trim().is_empty() => c,
            _ => {
                return McpToolCallResult::success(format!(
                    "# [{}] 未检测到有效登录凭据 (No Credentials Found)\n\n\
                    - 凭据文件: `{cookie_file}`\n\
                    - 说明: 未在此文件中找到带 `{platform}:` 前缀的凭据记录。\n\
                    - 建议操作: 执行登录提取指令以生成凭据，例如在 CLI 运行 `tkless login {platform}`，或手动将凭据按 `{platform}: key=val; ...` 写入 `{cookie_file}`。",
                    platform.to_uppercase()
                ));
            }
        };

        match platform.as_str() {
            "bilibili" => {
                let nav_url = "https://api.bilibili.com/x/web-interface/nav";
                match self
                    .engine
                    .client()
                    .get(nav_url)
                    .header("Cookie", &cookie_str)
                    .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36")
                    .header("Referer", "https://www.bilibili.com/")
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(v) = resp.json::<Value>().await {
                            let is_login = v.pointer("/data/isLogin").and_then(|b| b.as_bool()).unwrap_or(false);
                            if is_login {
                                let uname = v.pointer("/data/uname").and_then(|u| u.as_str()).unwrap_or("未知");
                                let mid = v.pointer("/data/mid").and_then(|m| m.as_u64()).unwrap_or(0);
                                let level = v.pointer("/data/level_info/current_level").and_then(|l| l.as_u64()).unwrap_or(0);
                                let vip_type = v.pointer("/data/vipType").and_then(|t| t.as_u64()).unwrap_or(0);
                                let vip_str = if vip_type > 0 { "大会员 (Active)" } else { "普通用户 (Standard)" };

                                McpToolCallResult::success(format!(
                                    "# [BILIBILI] 身份凭据鉴权成功 (Authenticated)\n\n\
                                    - **用户昵称**: {uname}\n\
                                    - **UID (mid)**: `{mid}`\n\
                                    - **账号等级**: Lv{level}\n\
                                    - **会员状态**: {vip_str}\n\
                                    - **凭据作用域**: `bilibili.com` (已隔离存储)"
                                ))
                            } else {
                                McpToolCallResult::success(
                                    "# [BILIBILI] 凭据已失效或已过期 (Cookie Expired)\n\n\
                                    - Bilibili 鉴权接口返回 `isLogin: false`。\n\
                                    - 建议操作: 运行 `tkless login bilibili` 重新扫码更新凭据。"
                                        .to_string(),
                                )
                            }
                        } else {
                            McpToolCallResult::success(format!("已加载 Bilibili 凭据 (长度 {} 字符)，解析接口响应异常。", cookie_str.len()))
                        }
                    }
                    Ok(resp) => McpToolCallResult::error(format!("Bilibili 鉴权检查网络响应异常: {}", resp.status())),
                    Err(e) => McpToolCallResult::error(format!("Bilibili 鉴权检查请求失败: {e}")),
                }
            }
            "zhihu" | "github" | "wechat" => {
                match crate::auth::verify_web_session(self.engine.client(), &platform, &cookie_str)
                    .await
                {
                    Ok(status) => McpToolCallResult::success(status.to_string()),
                    Err(error) => McpToolCallResult::error(format!(
                        "Session verification failed; validity is unknown: {error}"
                    )),
                }
            }
            _ => McpToolCallResult::error(format!("Unsupported platform '{platform}'")),
        }
    }

    async fn call_summarize_content(&self, args: &Value) -> McpToolCallResult {
        let text = if let Some(t) = args.get("text").and_then(|v| v.as_str()) {
            t.to_string()
        } else if let Some(u) = args.get("url").and_then(|v| v.as_str()) {
            let fetch_opts = ExtractionOptions {
                mode: ExtractionMode::Article,
                format: OutputFormat::Markdown,
                ..Default::default()
            };
            match self.engine.process_url(u, &fetch_opts).await {
                Ok(doc) => doc.content,
                Err(e) => {
                    return McpToolCallResult::error(format!(
                        "Failed to fetch and process URL '{u}': {e}"
                    ))
                }
            }
        } else {
            return McpToolCallResult::error(
                "Missing required parameter: either 'url' or 'text' must be provided",
            );
        };

        let max_bullets = args
            .get("max_bullets")
            .or_else(|| args.get("max_points"))
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .clamp(1, 10) as usize;

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

        let bullets = crate::summarizer::extract_key_takeaways(&text, max_bullets);

        match format {
            "json" => match serde_json::to_string_pretty(&bullets) {
                Ok(json_str) => McpToolCallResult::success(json_str),
                Err(e) => {
                    McpToolCallResult::error(format!("Failed to serialize summary to JSON: {e}"))
                }
            },
            _ => {
                let mut md = String::new();
                md.push_str("### 核心要点速览 (Key Takeaways)\n\n");
                for bullet in bullets {
                    md.push_str(&format!("- {}\n", bullet.trim()));
                }
                McpToolCallResult::success(md)
            }
        }
    }

    async fn call_chunk_markdown(&self, args: &Value) -> McpToolCallResult {
        let markdown = if let Some(m) = args.get("markdown").and_then(|v| v.as_str()) {
            m.to_string()
        } else if let Some(u) = args.get("url").and_then(|v| v.as_str()) {
            let fetch_opts = ExtractionOptions {
                mode: ExtractionMode::Article,
                format: OutputFormat::Markdown,
                ..Default::default()
            };
            match self.engine.process_url(u, &fetch_opts).await {
                Ok(doc) => doc.content,
                Err(e) => {
                    return McpToolCallResult::error(format!(
                        "Failed to fetch and process URL '{u}': {e}"
                    ))
                }
            }
        } else {
            return McpToolCallResult::error(
                "Missing required parameter: either 'url' or 'markdown' must be provided",
            );
        };

        let chunk_size = args
            .get("chunk_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(500) as usize;
        let chunk_overlap = args
            .get("chunk_overlap")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;
        let tokenizer = match args.get("tokenizer").and_then(|v| v.as_str()) {
            Some("cl100k") => TokenizerKind::Cl100k,
            Some("heuristic") => TokenizerKind::Heuristic,
            _ => TokenizerKind::O200k,
        };
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("json");

        let chunks =
            crate::chunker::chunk_markdown(&markdown, chunk_size, chunk_overlap, tokenizer);

        if format == "markdown" {
            let mut out = String::new();
            out.push_str(&format!("# 语义切块结果 (共 {} 块)\n\n", chunks.len()));
            for chunk in &chunks {
                let breadcrumbs = if chunk.heading_path.is_empty() {
                    "正文".to_string()
                } else {
                    chunk.heading_path.join(" > ")
                };
                out.push_str(&format!(
                    "### 切块 #{}: {} ({} Tokens / {} 字符)\n\n{}\n\n---\n\n",
                    chunk.index + 1,
                    breadcrumbs,
                    chunk.token_count,
                    chunk.content.chars().count(),
                    chunk.content
                ));
            }
            McpToolCallResult::success(out)
        } else {
            match serde_json::to_string_pretty(&chunks) {
                Ok(s) => McpToolCallResult::success(s),
                Err(e) => {
                    McpToolCallResult::error(format!("Failed to serialize chunks to JSON: {e}"))
                }
            }
        }
    }

    async fn call_split_document(&self, args: &Value) -> McpToolCallResult {
        let markdown = match args.get("markdown").and_then(|v| v.as_str()) {
            Some(m) if !m.trim().is_empty() => m,
            _ => return McpToolCallResult::error("Missing required parameter 'markdown'"),
        };

        let max_level = args
            .get("max_heading_level")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as usize;

        let sections = crate::splitter::split_markdown_by_headings(markdown, max_level);
        match serde_json::to_string_pretty(&sections) {
            Ok(json_str) => McpToolCallResult::success(json_str),
            Err(e) => McpToolCallResult::error(format!("Serialization error: {e}")),
        }
    }

    async fn call_diff_content(&self, args: &Value) -> McpToolCallResult {
        let old_text = match args.get("old_text").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return McpToolCallResult::error("Missing required parameter 'old_text'"),
        };
        let new_text = match args.get("new_text").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return McpToolCallResult::error("Missing required parameter 'new_text'"),
        };

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

        let report = crate::diff::compute_diff(old_text, new_text);

        if format == "json" {
            match serde_json::to_string_pretty(&report) {
                Ok(s) => McpToolCallResult::success(s),
                Err(e) => McpToolCallResult::error(format!("Serialization error: {e}")),
            }
        } else {
            let mut md = String::new();
            md.push_str("# 文档版本差异与变更分析 (Content Diff Report)\n\n");
            md.push_str(&format!(
                "> 新增行数: `+{}` | 删除行数: `-{}` | 未变行数: `{}` | 变更比例: `{:.1}%`\n\n",
                report.added_lines,
                report.removed_lines,
                report.unchanged_lines,
                report.change_ratio
            ));
            md.push_str("```diff\n");
            md.push_str(&report.unified_diff);
            md.push_str("\n```");
            McpToolCallResult::success(md)
        }
    }

    async fn call_capture_screenshot(&self, args: &Value) -> McpToolCallResult {
        let raw_url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) if !u.trim().is_empty() => u.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'url'"),
        };
        let parsed_url = match Url::parse(raw_url) {
            Ok(u) => u,
            Err(e) => return McpToolCallResult::error(format!("Invalid URL: {e}")),
        };

        if let Err(e) = self.engine.fetcher().validate_ssrf(&parsed_url).await {
            return McpToolCallResult::error(format!("SSRF blocked: {e}"));
        }

        let cdp_ep = crate::cdp::resolve_cdp_endpoint(
            args.get("cdp_endpoint").and_then(|v| v.as_str()),
            true,
        )
        .unwrap_or_else(|| "http://127.0.0.1:9222".to_string());

        let wait_ms = args
            .get("cdp_wait_ms")
            .and_then(|v| v.as_u64())
            .or(Some(1000));
        let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("png");
        let output_path = args.get("output_path").and_then(|v| v.as_str());

        let cdp_opts = crate::cdp::CdpOptions {
            endpoint: cdp_ep,
            wait_ms,
            timeout_ms: None,
            user_agent: self.engine.fetcher().options().user_agent.clone(),
            cookie: self.engine.fetcher().resolve_cookie_for_url(&parsed_url),
            block_resources: Some(false),
            ..Default::default()
        };

        match crate::cdp::capture_screenshot_via_cdp(raw_url, &cdp_opts, Some(format)).await {
            Ok(bytes) => {
                if let Some(dest) = output_path {
                    if let Err(e) = std::fs::write(dest, &bytes) {
                        return McpToolCallResult::error(format!(
                            "Failed to write screenshot file '{dest}': {e}"
                        ));
                    }
                    McpToolCallResult::success(format!(
                        "页面高清快照截图成功！已保存至 `{dest}` (容量: {})",
                        crate::downloader::format_bytes(bytes.len() as u64)
                    ))
                } else {
                    let b64 = crate::cdp::encode_base64(&bytes);
                    McpToolCallResult::success(format!("data:image/{format};base64,{b64}"))
                }
            }
            Err(e) => McpToolCallResult::error(format!("CDP 截取网页快照失败: {e}")),
        }
    }

    async fn call_export_tables(&self, args: &Value) -> McpToolCallResult {
        let html = if let Some(h) = args.get("html").and_then(|v| v.as_str()) {
            h.to_string()
        } else if let Some(u) = args.get("url").and_then(|v| v.as_str()) {
            let parsed_url = match Url::parse(u) {
                Ok(p) => p,
                Err(e) => return McpToolCallResult::error(format!("Invalid URL: {e}")),
            };
            if let Err(e) = self.engine.fetcher().validate_ssrf(&parsed_url).await {
                return McpToolCallResult::error(format!("SSRF blocked: {e}"));
            }
            match self.engine.fetcher().fetch(u).await {
                Ok(res) => res.html,
                Err(e) => {
                    return McpToolCallResult::error(format!("Failed to fetch URL '{u}': {e}"))
                }
            }
        } else {
            return McpToolCallResult::error(
                "Missing required parameter: either 'url' or 'html' must be provided",
            );
        };

        let format_str = args.get("format").and_then(|v| v.as_str()).unwrap_or("sql");

        let table_format = match format_str {
            "sql" => TableExportFormat::Sql,
            "csv" => TableExportFormat::Csv,
            "tsv" => TableExportFormat::Tsv,
            "json" => TableExportFormat::Json,
            _ => TableExportFormat::Markdown,
        };

        let table_name = args
            .get("table_name")
            .and_then(|v| v.as_str())
            .unwrap_or("extracted_table");

        let doc = scraper::Html::parse_document(&html);
        let mut tables = crate::extractor::extract_tables(&doc);

        if tables.is_empty() {
            return McpToolCallResult::error(
                "未在提供的页面或 HTML 中检测到有效的 <table> 表格元素",
            );
        }

        if let Some(idx) = args
            .get("table_index")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
        {
            if idx >= tables.len() {
                return McpToolCallResult::error(format!(
                    "table_index {} 超出范围，页面仅包含 {} 个表格 (有效索引: 0-{})",
                    idx,
                    tables.len(),
                    tables.len().saturating_sub(1)
                ));
            }
            tables = vec![tables.swap_remove(idx)];
        }

        if table_format == TableExportFormat::Sql {
            let sql_ddl = tables
                .iter()
                .enumerate()
                .map(|(idx, t)| {
                    let name = if tables.len() > 1 {
                        format!("{table_name}_{}", idx + 1)
                    } else {
                        table_name.to_string()
                    };
                    t.to_sql(&name)
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            McpToolCallResult::success(sql_ddl)
        } else {
            let formatted = crate::model::ExtractedTable::format_tables(&tables, table_format);
            McpToolCallResult::success(formatted)
        }
    }

    async fn call_list_adapters(&self, args: &Value) -> McpToolCallResult {
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");
        let list = crate::adapters::AdapterRegistry::list_adapter_metadata();
        if format == "json" {
            match serde_json::to_string_pretty(&list) {
                Ok(s) => McpToolCallResult::success(s),
                Err(e) => McpToolCallResult::error(format!("Serialization error: {e}")),
            }
        } else {
            McpToolCallResult::success(crate::adapters::format_adapters_markdown(&list))
        }
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

async fn write_rpc<W: tokio::io::AsyncWrite + Unpin, T: serde::Serialize>(
    output: &mut W,
    value: &T,
) -> Result<(), TklessError> {
    let mut line = serde_json::to_vec(value).map_err(|e| TklessError::McpError {
        code: INTERNAL_ERROR,
        message: e.to_string(),
    })?;
    line.push(b'\n');
    output.write_all(&line).await?;
    output.flush().await?;
    Ok(())
}
