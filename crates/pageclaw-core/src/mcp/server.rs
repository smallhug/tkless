use super::tools::get_available_tools;
use super::types::*;
use crate::adapters::BilibiliAdapter;
use crate::cache::DocumentCache;
use crate::error::PageclawError;
use crate::model::{
    ExtractionMode, ExtractionOptions, FetchOptions, OnTooLarge, OutputFormat, SearchEngineKind,
    TableExportFormat, TokenizerKind, VideoQuality,
};
use crate::PageclawEngine;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};
use url::Url;

/// Type alias for an unbounded JSON notification sender used for MCP progress streaming and logging.
pub type NotificationSender = tokio::sync::mpsc::UnboundedSender<Value>;

/// In-memory and stdio Model Context Protocol (MCP) server for PageClaw.
pub struct McpServer {
    engine: Arc<PageclawEngine>,
    cache: Arc<DocumentCache>,
}

impl McpServer {
    /// Creates a new MCP server wrapping the provided extraction engine and document cache.
    pub fn new(engine: PageclawEngine, cache: DocumentCache) -> Self {
        Self {
            engine: Arc::new(engine),
            cache: Arc::new(cache),
        }
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

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
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

        let req_id = request.id.unwrap_or(Value::Null);

        debug!(method = %request.method, id = ?req_id, "Handling MCP request");

        match request.method.as_str() {
            "initialize" => {
                let init_result = serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {},
                        "logging": {}
                    },
                    "serverInfo": {
                        "name": "pageclaw",
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
                let tools = get_available_tools();
                Some(JsonRpcResponse::success(
                    req_id,
                    serde_json::json!({ "tools": tools }),
                ))
            }
            "tools/call" => {
                let params = request.params.unwrap_or(Value::Null);
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));
                let progress_token = params
                    .get("_meta")
                    .and_then(|m| m.get("progressToken"))
                    .cloned();

                let tool_result = match tool_name {
                    "fetch_webpage" => self.call_fetch_webpage(&arguments).await,
                    "fetch_bilibili_feed" => self.call_fetch_bilibili_feed(&arguments).await,
                    "search_bilibili" => self.call_search_bilibili(&arguments).await,
                    "download_media" => self.call_download_media(&arguments).await,
                    "web_search" => self.call_web_search(&arguments).await,
                    "crawl_site" => {
                        self.call_crawl_site(&arguments, notifier, progress_token.as_ref())
                            .await
                    }
                    "batch_fetch_webpages" => {
                        self.call_batch_fetch_webpages(
                            &arguments,
                            notifier,
                            progress_token.as_ref(),
                        )
                        .await
                    }
                    "clean_html" => self.call_clean_html(&arguments).await,
                    "login_platform" => self.call_login_platform(&arguments).await,
                    "scaffold_schema" => self.call_scaffold_schema(&arguments).await,
                    "compare_similarity" => self.call_compare_similarity(&arguments).await,
                    "manage_cache" => self.call_manage_cache(&arguments).await,
                    _ => McpToolCallResult::error(format!("Unknown tool: '{tool_name}'")),
                };

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
                    "Method '{}' is not supported by PageClaw MCP server",
                    request.method
                ),
            )),
        }
    }

    /// Asynchronous stdio loop reading requests from stdin and streaming responses/notifications to stdout.
    pub async fn run_stdio(&self) -> Result<(), PageclawError> {
        info!("Starting PageClaw MCP Server (transport: stdio, protocol: 2024-11-05)");

        let (stdout_tx, mut stdout_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        let writer_handle = tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            while let Some(line) = stdout_rx.recv().await {
                if let Err(e) = stdout.write_all(line.as_bytes()).await {
                    error!(error = %e, "Failed to write MCP response to stdout");
                    break;
                }
                let _ = stdout.flush().await;
            }
        });

        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let stdout_tx_notify = stdout_tx.clone();
        let forwarder_handle = tokio::spawn(async move {
            while let Some(notif) = notify_rx.recv().await {
                if let Ok(mut json_line) = serde_json::to_string(&notif) {
                    json_line.push('\n');
                    let _ = stdout_tx_notify.send(json_line);
                }
            }
        });

        let stdin = tokio::io::stdin();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await.map_err(PageclawError::Io)? {
            if let Some(response) = self
                .handle_message_with_notifier(&line, Some(&notify_tx))
                .await
            {
                match serde_json::to_string(&response) {
                    Ok(mut json_line) => {
                        json_line.push('\n');
                        let _ = stdout_tx.send(json_line);
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to serialize JSON-RPC response");
                    }
                }
            }
        }

        drop(notify_tx);
        drop(stdout_tx);
        let _ = forwarder_handle.await;
        let _ = writer_handle.await;

        info!("PageClaw MCP Server connection closed");
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Tool Dispatch Handlers
    // -------------------------------------------------------------------------

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
            _ => TableExportFormat::Markdown,
        };
        let stats = args.get("stats").and_then(|v| v.as_bool()).unwrap_or(false);

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
            schema,
            chunk,
            chunk_size,
            chunk_overlap,
            download_images,
            assets_dir,
            tables_only,
            table_format,
            stats,
            remove_selector,
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

        let engine = match PageclawEngine::with_options(fetch_options) {
            Ok(eng) => eng,
            Err(e) => return McpToolCallResult::error(format!("Failed to initialize engine: {e}")),
        };

        let doc = if use_cache {
            match engine
                .process_url_with_cache(url, &options, &self.cache)
                .await
            {
                Ok(d) => d,
                Err(e) => {
                    return McpToolCallResult::error(format!("Failed to fetch URL '{url}': {e}"))
                }
            }
        } else {
            match engine.process_url(url, &options).await {
                Ok(d) => d,
                Err(e) => {
                    return McpToolCallResult::error(format!("Failed to fetch URL '{url}': {e}"))
                }
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
            Some(self.cache.generate_key(&target_url, &options))
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
            Some(self.cache.generate_key(&target_url, &options))
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

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .clamp(1, 20) as usize;
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };
        let engine = match args.get("engine").and_then(|v| v.as_str()) {
            Some("google") => SearchEngineKind::Google,
            Some("auto") => SearchEngineKind::Auto,
            _ => SearchEngineKind::Bing,
        };
        let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("all");
        let effective_query = match scope {
            "wechat" => {
                let clean = query.trim();
                if clean.starts_with("site:") {
                    clean.to_string()
                } else {
                    format!("site:mp.weixin.qq.com {clean}")
                }
            }
            "zhihu" => {
                let clean = query.trim();
                if clean.starts_with("site:") {
                    clean.to_string()
                } else {
                    format!("site:zhihu.com {clean}")
                }
            }
            "github" => {
                let clean = query.trim();
                if clean.starts_with("site:") {
                    clean.to_string()
                } else {
                    format!("site:github.com {clean}")
                }
            }
            "juejin" => {
                let clean = query.trim();
                if clean.starts_with("site:") {
                    clean.to_string()
                } else {
                    format!("site:juejin.cn {clean}")
                }
            }
            "csdn" => {
                let clean = query.trim();
                if clean.starts_with("site:") {
                    clean.to_string()
                } else {
                    format!("site:blog.csdn.net {clean}")
                }
            }
            "medium" => {
                let clean = query.trim();
                if clean.starts_with("site:") {
                    clean.to_string()
                } else {
                    format!("site:medium.com {clean}")
                }
            }
            "substack" => {
                let clean = query.trim();
                if clean.starts_with("site:") {
                    clean.to_string()
                } else {
                    format!("site:substack.com {clean}")
                }
            }
            _ => query.to_string(),
        };

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

        let custom_engine;
        let engine_ref = if proxy.is_some() || mobile {
            let mut fetch_opts = self.engine.fetcher().options().clone();
            if let Some(ref p) = proxy {
                fetch_opts.proxy = Some(p.clone());
            }
            if mobile {
                fetch_opts.mobile = true;
            }
            match PageclawEngine::with_options(fetch_opts) {
                Ok(eng) => {
                    custom_engine = eng;
                    &custom_engine
                }
                Err(_) => &self.engine,
            }
        } else {
            &self.engine
        };

        if fetch_content {
            let cdp_endpoint = args
                .get("cdp_endpoint")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let cdp_wait_ms = args.get("cdp_wait_ms").and_then(|v| v.as_u64());
            let schema = args.get("schema").map(|v| {
                if let Some(s) = v.as_str() {
                    s.to_string()
                } else {
                    v.to_string()
                }
            });

            let options = ExtractionOptions {
                cdp_endpoint,
                cdp_wait_ms,
                schema,
                ..Default::default()
            };

            match crate::search::web_search_and_fetch(
                engine_ref,
                &effective_query,
                limit,
                engine,
                &options,
                max_tokens_per_page,
                dedup,
            )
            .await
            {
                Ok(items) => match format {
                    OutputFormat::Json => match serde_json::to_string_pretty(&items) {
                        Ok(json_str) => McpToolCallResult::success(json_str),
                        Err(e) => {
                            McpToolCallResult::error(format!("Failed to serialize JSON: {e}"))
                        }
                    },
                    _ => {
                        let md =
                            crate::search::format_search_fetch_markdown(&effective_query, &items);
                        McpToolCallResult::success(md)
                    }
                },
                Err(e) => McpToolCallResult::error(format!("Search & fetch failed: {e}")),
            }
        } else {
            match crate::search::web_search_with_engine(
                engine_ref.client(),
                &effective_query,
                limit,
                engine,
            )
            .await
            {
                Ok(results) => match format {
                    OutputFormat::Json => match serde_json::to_string_pretty(&results) {
                        Ok(json_str) => McpToolCallResult::success(json_str),
                        Err(e) => {
                            McpToolCallResult::error(format!("Failed to serialize JSON: {e}"))
                        }
                    },
                    _ => {
                        let md = crate::search::format_search_results_markdown(
                            &effective_query,
                            &results,
                        );
                        McpToolCallResult::success(md)
                    }
                },
                Err(e) => McpToolCallResult::error(format!("Web search failed: {e}")),
            }
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
            concurrency: 5,
            same_domain_only: true,
            same_path_prefix,
            exclude_patterns: crate::crawler::CrawlOptions::default().exclude_patterns,
            sitemap,
            dedup,
        };

        let extraction_options = ExtractionOptions {
            cdp_endpoint,
            cdp_wait_ms,
            schema,
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
            match PageclawEngine::with_options(fetch_opts) {
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
                    let _ = tx.send(notif);
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
            Ok(report) => match format {
                OutputFormat::Json => match serde_json::to_string_pretty(&report) {
                    Ok(json_str) => McpToolCallResult::success(json_str),
                    Err(e) => McpToolCallResult::error(format!("Failed to serialize JSON: {e}")),
                },
                _ => {
                    let mut md = crate::crawler::format_crawl_report_markdown(&report);
                    if let Some(limit) = max_tokens {
                        let current_tokens =
                            crate::token::count_tokens(&md, TokenizerKind::default());
                        if current_tokens > limit {
                            let (truncated_md, _) = crate::token::truncate_to_tokens(
                                &md,
                                limit,
                                TokenizerKind::default(),
                            );
                            md = truncated_md;
                        }
                    }
                    McpToolCallResult::success(md)
                }
            },
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
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>(),
            _ => return McpToolCallResult::error("Missing or empty 'urls' array"),
        };

        if urls.is_empty() {
            return McpToolCallResult::error(
                "'urls' array must contain at least one valid URL string",
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
            match PageclawEngine::with_options(fetch_opts) {
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
            cdp_endpoint,
            cdp_wait_ms,
            ..Default::default()
        };

        let total_urls = urls.len();
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut join_set = tokio::task::JoinSet::new();

        for (idx, url) in urls.into_iter().enumerate() {
            let sem = semaphore.clone();
            let eng = engine_ref.clone();
            let opts = base_options.clone();

            join_set.spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let res = eng.process_url(&url, &opts).await;
                (idx, url, res)
            });
        }

        let mut completed = 0;
        let mut results = Vec::new();
        while let Some(res) = join_set.join_next().await {
            if let Ok(item) = res {
                results.push(item);
            }
            completed += 1;
            if let (Some(tx), Some(token)) = (notifier, progress_token) {
                let notif = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/progress",
                    "params": {
                        "progressToken": token,
                        "progress": completed,
                        "total": total_urls
                    }
                });
                let _ = tx.send(notif);
            }
        }
        results.sort_by_key(|(idx, _, _)| *idx);

        let mut docs = Vec::new();
        let mut errors = Vec::new();

        for (_, url, res) in results {
            match res {
                Ok(doc) => docs.push(doc),
                Err(e) => errors.push(format!("- `{url}`: {e}")),
            }
        }

        match format {
            OutputFormat::Json => match serde_json::to_string_pretty(&docs) {
                Ok(json_str) => McpToolCallResult::success(json_str),
                Err(e) => {
                    McpToolCallResult::error(format!("Failed to serialize documents to JSON: {e}"))
                }
            },
            _ => {
                let mut out = docs
                    .iter()
                    .map(|d| d.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n---\n\n");
                if !errors.is_empty() {
                    out.push_str("\n\n### 部分 URL 获取失败 (Failed URLs)\n\n");
                    out.push_str(&errors.join("\n"));
                }
                McpToolCallResult::success(out)
            }
        }
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
            _ => TableExportFormat::Markdown,
        };
        let stats = args.get("stats").and_then(|v| v.as_bool()).unwrap_or(false);

        let options = ExtractionOptions {
            mode,
            format,
            selector,
            schema,
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
            ..Default::default()
        };

        match self.engine.process_html(html, base_url, &options) {
            Ok(doc) => format_doc_result(&doc, format),
            Err(e) => McpToolCallResult::error(format!("Failed to clean HTML: {e}")),
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
                    "知乎登录说明：\n1. 请在本地浏览器访问授权登录页：https://www.zhihu.com/signin\n2. 登录成功后，从浏览器开发者工具 (F12 -> Application -> Cookies) 复制 Cookie 字符串\n3. 将凭据以 'zhihu: ...' 前缀格式追加到 `{output_file}`：\n```\nzhihu: d_c0=\"...\"; z_c0=\"...\"\n```\n4. 保存后 PageClaw 将自动识别知乎会话凭据进行鉴权访问。"
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
                Ok(_) => McpToolCallResult::success("✅ PageClaw 本地响应缓存已成功清空。"),
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
}

fn format_doc_result(doc: &crate::model::PageDocument, format: OutputFormat) -> McpToolCallResult {
    match format {
        OutputFormat::Json => match serde_json::to_string_pretty(doc) {
            Ok(json_str) => McpToolCallResult::success(json_str),
            Err(e) => McpToolCallResult::error(format!("Failed to serialize JSON: {e}")),
        },
        _ => {
            if let Some(ref chunks) = doc.chunks {
                let mut out = String::new();
                out.push_str(&format!(
                    "# ✂️ 语义切块输出: {} (共 {} 块)\n\n",
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
    }
}
