use super::tools::get_available_tools;
use super::types::*;
use crate::adapters::BilibiliAdapter;
use crate::cache::DocumentCache;
use crate::error::PageclawError;
use crate::model::{
    ExtractionMode, ExtractionOptions, FetchOptions, OnTooLarge, OutputFormat, SearchEngineKind,
    TokenizerKind, VideoQuality,
};
use crate::PageclawEngine;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};

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
                        "tools": {}
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

                let tool_result = match tool_name {
                    "fetch_webpage" => self.call_fetch_webpage(&arguments).await,
                    "fetch_bilibili_feed" => self.call_fetch_bilibili_feed(&arguments).await,
                    "search_bilibili" => self.call_search_bilibili(&arguments).await,
                    "download_media" => self.call_download_media(&arguments).await,
                    "web_search" => self.call_web_search(&arguments).await,
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

    /// Asynchronous stdio loop reading requests from stdin and streaming responses to stdout.
    pub async fn run_stdio(&self) -> Result<(), PageclawError> {
        info!("Starting PageClaw MCP Server (transport: stdio, protocol: 2024-11-05)");

        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await.map_err(PageclawError::Io)? {
            if let Some(response) = self.handle_message(&line).await {
                match serde_json::to_string(&response) {
                    Ok(mut json_line) => {
                        json_line.push('\n');
                        stdout
                            .write_all(json_line.as_bytes())
                            .await
                            .map_err(PageclawError::Io)?;
                        stdout.flush().await.map_err(PageclawError::Io)?;
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to serialize JSON-RPC response");
                    }
                }
            }
        }

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
        };

        // Cache check
        let cache_key = if use_cache {
            Some(self.cache.generate_key(url, &options))
        } else {
            None
        };

        if let Some(ref key) = cache_key {
            if let Some(cached_doc) = self.cache.get(key) {
                return format_doc_result(&cached_doc, format);
            }
        }

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
            allow_private_addresses,
            allow_http_errors,
        };

        let engine = match PageclawEngine::with_options(fetch_options) {
            Ok(eng) => eng,
            Err(e) => return McpToolCallResult::error(format!("Failed to initialize engine: {e}")),
        };

        match engine.process_url(url, &options).await {
            Ok(doc) => {
                if let Some(ref key) = cache_key {
                    let _ = self.cache.set(key, &doc, None);
                }
                format_doc_result(&doc, format)
            }
            Err(e) => McpToolCallResult::error(format!("Failed to fetch URL '{url}': {e}")),
        }
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
                    self.engine.client(),
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
                    self.engine.client(),
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
        let fetch_content = args
            .get("fetch_content")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_tokens_per_page = args
            .get("max_tokens_per_page")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        if fetch_content {
            let options = ExtractionOptions::default();
            match crate::search::web_search_and_fetch(
                &self.engine,
                query,
                limit,
                engine,
                &options,
                max_tokens_per_page,
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
                        let md = crate::search::format_search_fetch_markdown(query, &items);
                        McpToolCallResult::success(md)
                    }
                },
                Err(e) => McpToolCallResult::error(format!("Search & fetch failed: {e}")),
            }
        } else {
            match crate::search::web_search_with_engine(self.engine.client(), query, limit, engine)
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
                        let md = crate::search::format_search_results_markdown(query, &results);
                        McpToolCallResult::success(md)
                    }
                },
                Err(e) => McpToolCallResult::error(format!("Web search failed: {e}")),
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
        _ => McpToolCallResult::success(&doc.content),
    }
}
