use super::{
    format_doc_result, ExtractionMode, ExtractionOptions, FetchOptions, McpServer,
    McpToolCallResult, OnTooLarge, OutputFormat, TableExportFormat, TklessEngine, TokenizerKind,
    Value,
};
use url::Url;

impl McpServer {
    pub(super) async fn call_fetch_webpage(&self, args: &Value) -> McpToolCallResult {
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
            initial_target_origin: Url::parse(url)
                .ok()
                .map(|u| u.origin().ascii_serialization()),
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
}
