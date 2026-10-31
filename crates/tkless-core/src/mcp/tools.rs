use super::types::McpTool;
use serde::Serialize;

/// Catalog metadata entry for dynamic tool discovery.
#[derive(Debug, Clone, Serialize)]
pub struct ToolCatalogEntry {
    pub name: &'static str,
    pub category: &'static str,
    pub summary: &'static str,
    pub keywords: &'static [&'static str],
    pub is_core: bool,
}

/// Returns the complete catalog metadata for all specialized TkLess tools.
pub fn get_tool_catalog() -> Vec<ToolCatalogEntry> {
    vec![
        ToolCatalogEntry {
            name: "list_wechat_articles", category: "content", summary: "Public WeChat account discovery, article identity and album cursor pagination with explicit completeness.",
            keywords: &["wechat", "公众号", "微信", "album", "articles", "history"], is_core: false,
        },
        ToolCatalogEntry {
            name: "browse_page", category: "content", summary: "Extract browser-rendered content after bounded wait_for, fill, click, and scroll actions.",
            keywords: &["browser", "interactive", "click", "fill", "wait", "dynamic"], is_core: false,
        },
        ToolCatalogEntry {
            name: "sync_collection", category: "content", summary: "Persist snapshots and incremental added/updated/removed events; incomplete runs never delete missing records.",
            keywords: &["sync", "snapshot", "incremental", "updates", "knowledge-base"], is_core: false,
        },
        ToolCatalogEntry {
            name: "collect_pages", category: "content", summary: "Collect paginated HTML or cursor JSON lists and linked details with explicit completion status.",
            keywords: &["pagination", "cursor", "list", "details", "collect"], is_core: false,
        },
        // Core tools
        ToolCatalogEntry {
            name: "fetch_webpage",
            category: "content",
            summary: "Fetches webpage/article, strips noise, converts to Markdown/JSON with native adapters.",
            keywords: &["fetch", "webpage", "article", "html", "markdown", "bilibili", "scrape", "skills", "skills.sh"],
            is_core: true,
        },
        ToolCatalogEntry {
            name: "web_search",
            category: "search",
            summary: "Performs full-web search via Bing without API keys, with optional fetch-content.",
            keywords: &["search", "bing", "query", "find", "web"],
            is_core: true,
        },
        // Specialized Content tools
        ToolCatalogEntry {
            name: "crawl_site",
            category: "content",
            summary: "Deep site crawler compiling linked pages into a consolidated single-file Markdown knowledge base.",
            keywords: &["crawl", "recursive", "sitemap", "knowledge-base", "docs"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "batch_fetch_webpages",
            category: "content",
            summary: "Bounded batch fetcher with streamed results, durable checkpoints and failed-item retry.",
            keywords: &["batch", "concurrency", "parallel", "bulk", "urls", "stream", "checkpoint", "resume", "retry"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "clean_html",
            category: "content",
            summary: "Converts raw HTML string to clean Markdown or JSON with noise stripping.",
            keywords: &["clean", "html", "markdown", "noise", "readability"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "fetch_feed",
            category: "content",
            summary: "Parses RSS 2.0 or Atom 1.0 XML feeds into Markdown or JSON.",
            keywords: &["rss", "atom", "feed", "syndication", "xml"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "scaffold_schema",
            category: "content",
            summary: "Analyzes DOM structure skeleton and generates suggested extraction schema templates.",
            keywords: &["scaffold", "schema", "dom", "skeleton", "template"],
            is_core: false,
        },
        // Specialized Search tools
        ToolCatalogEntry {
            name: "search_bilibili",
            category: "search",
            summary: "Searches Bilibili videos with ordering (click, pubdate, dm, stow) and pagination.",
            keywords: &["bilibili", "video", "search", "order", "page"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "inspect_links",
            category: "search",
            summary: "Verifies reachability, HTTP status codes, and latency for a list of URLs with SSRF protection.",
            keywords: &["links", "health", "status", "redirect", "inspect"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "list_adapters",
            category: "search",
            summary: "Lists all 22 native platform adapters, domain patterns, and capabilities.",
            keywords: &["adapters", "platforms", "bilibili", "zhihu", "wechat", "github", "skills.sh"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "deep_research",
            category: "search",
            summary: "Autonomous multi-hop web research loop across multiple sub-queries with citation tracking and report synthesis.",
            keywords: &["research", "deep_research", "investigate", "report", "synthesis", "study", "analysis"],
            is_core: false,
        },
        // Specialized Media tools
        ToolCatalogEntry {
            name: "fetch_bilibili_feed",
            category: "media",
            summary: "Fetches Bilibili recommendation, popular, or ranking video streams.",
            keywords: &["bilibili", "feed", "recommendations", "popular", "ranking"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "download_media",
            category: "media",
            summary: "Downloads progressive video streams (Bilibili MP4) or video cover images.",
            keywords: &["download", "video", "media", "mp4", "cover", "image"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "take_screenshot",
            category: "media",
            summary: "Captures high-resolution webpage screenshot via Chrome DevTools Protocol (CDP).",
            keywords: &["screenshot", "cdp", "image", "capture", "render", "chrome"],
            is_core: false,
        },
        // Specialized Analysis tools
        ToolCatalogEntry {
            name: "export_tables",
            category: "analysis",
            summary: "Extracts HTML tables and exports to SQL DDL/INSERT, CSV, TSV, or JSON.",
            keywords: &["table", "sql", "csv", "tsv", "database", "export"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "summarize_content",
            category: "analysis",
            summary: "Extracts key takeaway bullet points (TL;DR) locally without external LLM API calls.",
            keywords: &["summarize", "tldr", "takeaways", "bullets", "summary"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "split_document",
            category: "analysis",
            summary: "Splits Markdown documents into structured sections based on heading hierarchy.",
            keywords: &["split", "headings", "sections", "toc", "structure"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "chunk_markdown",
            category: "analysis",
            summary: "Segments Markdown into heading-aware semantic chunks with token budgets for RAG vector databases.",
            keywords: &["chunk", "rag", "vector", "tokens", "semantic"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "diff_documents",
            category: "analysis",
            summary: "Computes unified diff and change metrics between two document versions.",
            keywords: &["diff", "compare", "changes", "versions", "revision"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "compare_similarity",
            category: "analysis",
            summary: "Computes 64-bit SimHash and Hamming distance between two texts for near-duplicate detection.",
            keywords: &["similarity", "simhash", "dedup", "hamming", "duplicate"],
            is_core: false,
        },
        // Specialized System tools
        ToolCatalogEntry {
            name: "manage_cache",
            category: "system",
            summary: "Inspects cache hit statistics, storage metrics, or purges cached responses.",
            keywords: &["cache", "stats", "clear", "purge", "memory", "disk"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "login_platform",
            category: "system",
            summary: "Generates interactive QR code in terminal for platform authentication and cookie extraction.",
            keywords: &["login", "auth", "qrcode", "cookie", "session"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "check_auth",
            category: "system",
            summary: "Verifies validity of platform session cookies without triggering anti-scraping challenges.",
            keywords: &["auth", "check", "cookie", "session", "verify"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "jobs_start",
            category: "system",
            summary: "Starts crawl_site, batch_fetch_webpages, collect_pages or sync_collection in the background and returns a job_id.",
            keywords: &["jobs", "async", "background", "start", "long-running", "task"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "jobs_status",
            category: "system",
            summary: "Reports state, elapsed time, progress and the final structured result of one background job.",
            keywords: &["jobs", "status", "progress", "result", "poll"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "jobs_cancel",
            category: "system",
            summary: "Cancels a queued or running background job and aborts its task.",
            keywords: &["jobs", "cancel", "abort", "stop"],
            is_core: false,
        },
        ToolCatalogEntry {
            name: "jobs_list",
            category: "system",
            summary: "Lists in-process background jobs with optional state filter and result-free summaries.",
            keywords: &["jobs", "list", "active", "summary"],
            is_core: false,
        },
    ]
}

/// Returns the 4 dynamic discovery meta-tools enabling Stage 1 on-demand tool search, schema inspection, execution, and activation.
pub fn get_dynamic_discovery_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            output_schema: None,
            name: "discover_tools".to_string(),
            description: "Discovers specialized TkLess tools by keyword or category (content, search, media, analysis, system, all).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search keyword (e.g. 'table', 'diff', 'bilibili', 'crawl', 'cache')" },
                    "category": {
                        "type": "string",
                        "enum": ["content", "search", "media", "analysis", "system", "all"],
                        "default": "all",
                        "description": "Category filter"
                    }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "get_tool_schema".to_string(),
            description: "Retrieves complete inputSchema and parameter definitions for a specific tool on demand.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tool_name": { "type": "string", "description": "Name of tool to inspect (e.g. 'export_tables', 'crawl_site')" }
                },
                "required": ["tool_name"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "execute_tool".to_string(),
            description: "Executes any specialized TkLess tool by name and arguments on demand without requiring it in tools/list.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tool_name": { "type": "string", "description": "Name of tool to execute" },
                    "arguments": { "type": "object", "description": "Arguments object matching the tool's inputSchema" }
                },
                "required": ["tool_name"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "activate_tools".to_string(),
            description: "Dynamically activates tools into active tools/list and notifies the MCP client (list_changed).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tool_names": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of tool names to dynamically add to tools/list"
                    }
                },
                "required": ["tool_names"]
            }),
        },
    ]
}

/// Returns the initial core tools exposed in dynamic mode (fetch_webpage and web_search).
pub fn get_core_tools() -> Vec<McpTool> {
    let all = get_available_tools();
    all.into_iter()
        .filter(|t| t.name == "fetch_webpage" || t.name == "web_search")
        .collect()
}

/// Returns the initial minimal toolset for Two-Stage Dynamic Tool Discovery (Core tools + Discovery Meta-Tools).
pub fn get_initial_dynamic_tools() -> Vec<McpTool> {
    let mut tools = get_core_tools();
    tools.extend(get_dynamic_discovery_tools());
    tools
}

/// Returns the complete registry of MCP tools exposed by TkLess.
pub fn get_available_tools() -> Vec<McpTool> {
    let mut tools = vec![
        McpTool {
            output_schema: None, name: "list_wechat_articles".into(),
            description: "Discover public WeChat articles by name/article/album. Supports account identity checks, linked album expansion and cursor pagination with explicit completion status. Never bypasses verification.".into(),
            input_schema: serde_json::json!({"type":"object","additionalProperties":false,"required":["source"],"properties":{
                "source":{"type":"string","minLength":1,"maxLength":4096},
                "limit":{"type":"integer","minimum":1,"maximum":crate::wechat::MAX_LIMIT,"default":crate::wechat::DEFAULT_LIMIT},
                "max_pages":{"type":"integer","minimum":1,"maximum":crate::wechat::MAX_PAGES,"default":crate::wechat::DEFAULT_PAGES},
                "delay_ms":{"type":"integer","minimum":0,"maximum":60000,"default":300},
                "fetch_content":{"type":"boolean","default":false},"follow_albums":{"type":"boolean","default":true},
                "expected_biz":{"type":"string","minLength":1,"maxLength":128},
                "max_tokens_per_article":{"type":"integer","minimum":1,"maximum":100000,"default":4000}
            }}),
        },
        McpTool {
            output_schema: None, name: "browse_page".into(),
            description: "Execute at most 32 declarative browser steps then extract content. Blocks non-GET/HEAD/OPTIONS requests and cross-origin navigation by default. No arbitrary JavaScript. Use wait_for after actions that load content.".into(),
            input_schema: serde_json::json!({"type":"object","additionalProperties":false,"required":["url","plan"],"properties":{
                "url":{"type":"string"},"plan":{"type":"object","description":"BrowserPlan: endpoint, timeout_ms (1..120000), wait_ms, allow_cross_origin_navigation, steps [{action:wait_for,selector,timeout_ms}|{action:fill,selector,value}|{action:click,selector}|{action:scroll,pixels}]"},
                "allow_private_addresses":{"type":"boolean","default":false}
            }}),
        },
        McpTool {
            output_schema: None, name: "sync_collection".into(),
            description: "Run collection and store versioned snapshots and a durable manifest in state_dir. Requires config.id_pointer or details.url_pointer. Emits changes; incomplete runs preserve unseen records. This tool writes local files.".into(),
            input_schema: serde_json::json!({"type":"object","additionalProperties":false,"required":["config","state_dir"],"properties":{
                "config":{"type":"object","description":"Same configuration as collect_pages"},"state_dir":{"type":"string"},
                "max_state_mb":{"type":"integer","minimum":1,"maximum":crate::limits::SYNC_MAX_STATE_MB,"default":crate::limits::SYNC_DEFAULT_STATE_MB},"allow_private_addresses":{"type":"boolean","default":false}
            }}),
        },
        McpTool {
            output_schema: None,
            name: "collect_pages".into(),
            description: "Collect HTML lists using schema.items_selector and fields with next_link or page_number pagination; cursor mode reads JSON pointers. Optional details.url_pointer fetches linked pages. Returns partial results and a stop reason on failure or limits.".into(),
            input_schema: serde_json::json!({"type":"object","additionalProperties":false,"required":["config"],"properties":{
                "config":{"type":"object","description":"CollectionConfig: url, schema, pagination {kind:none|next_link|page_number|cursor}, optional details {url_pointer,schema}, id_pointer, max_pages (1..1000), max_items (1..100000), delay_ms, allow_cross_origin"},
                "allow_private_addresses":{"type":"boolean","default":false}
            }}),
        },
        McpTool {
            output_schema: None,
            name: "fetch_webpage".to_string(),
            description: "Fetches a webpage or video URL, strips DOM noise, and converts content to Markdown or JSON.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Target webpage or Bilibili video URL / BV ID" },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json", "jsonl", "text", "html", "raw"],
                        "default": "markdown",
                        "description": "Output format"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["article", "full", "raw"],
                        "default": "article",
                        "description": "Extraction mode: article (Readability), full, or raw"
                    },
                    "max_tokens": { "type": "integer", "description": "Max token budget with safe sentence-boundary truncation" },
                    "tokenizer": {
                        "type": "string",
                        "enum": ["o200k", "cl100k", "heuristic"],
                        "default": "o200k",
                        "description": "Tokenizer model encoding"
                    },
                    "frontmatter": { "type": "boolean", "default": false, "description": "Prepend YAML frontmatter with metadata" },
                    "no_images": { "type": "boolean", "default": false, "description": "Strip images from Markdown" },
                    "no_links": { "type": "boolean", "default": false, "description": "Strip hyperlinks, keeping only anchor text" },
                    "no_metadata": { "type": "boolean", "default": false, "description": "Skip metadata extraction" },
                    "no_adapter": { "type": "boolean", "default": false, "description": "Force raw HTML scraping without platform adapters" },
                    "skills_pages": { "type": "integer", "minimum": 1, "maximum": 50, "default": 3, "description": "Skills.sh leaderboard page cap, including first page; auto-pagination is enabled by default" },
                    "skills_limit": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 1000, "description": "Skills.sh distinct result cap; upstream site search supports at most 200 results without a cursor" },
                    "timeout": { "type": "integer", "default": 15, "description": "Request timeout in seconds" },
                    "connect_timeout": { "type": "integer", "default": 10, "description": "Connection handshake timeout in seconds" },
                    "retries": { "type": "integer", "default": 0, "description": "Retry attempts with exponential backoff" },
                    "on_too_large": {
                        "type": "string",
                        "enum": ["error", "truncate"],
                        "default": "error",
                        "description": "Action when response exceeds max-size"
                    },
                    "max_size_mb": { "type": "integer", "minimum": 1, "maximum": 2048, "default": 10, "description": "Maximum response download size in megabytes" },
                    "allow_private_addresses": { "type": "boolean", "default": false, "description": "Allow private/localhost IPs (bypasses SSRF guard)" },
                    "allow_http_errors": { "type": "boolean", "default": false, "description": "Do not fail on HTTP 4xx or 5xx responses" },
                    "use_cache": { "type": "boolean", "default": false, "description": "Enable response caching (opt-in, matching the CLI)" },
                    "selector": { "type": "string", "description": "CSS selector to extract" },
                    "remove_selector": { "type": "string", "description": "CSS selector to exclude/prune" },
                    "extract_links": { "type": "boolean", "default": false, "description": "Extract structured hyperlinks list" },
                    "extract_images": { "type": "boolean", "default": false, "description": "Extract structured images list" },
                    "extract_tables": { "type": "boolean", "default": false, "description": "Extract structured tables" },
                    "subtitles": { "type": "boolean", "default": false, "description": "Extract Bilibili CC subtitles" },
                    "danmaku": { "type": "boolean", "default": false, "description": "Extract Bilibili timestamped danmaku comments" },
                    "danmaku_limit": { "type": "integer", "description": "Max danmaku comments count" },
                    "comments": { "type": "boolean", "default": false, "description": "Extract Bilibili top comments" },
                    "comments_limit": { "type": "integer", "description": "Max top comments count" },
                    "conclusion": { "type": "boolean", "default": false, "description": "Extract Bilibili AI video summary and outline" },
                    "cookie": { "type": "string", "description": "Cookie header string" },
                    "cookie_file": { "type": "string", "description": "Path to cookie file" },
                    "cdp_endpoint": { "type": "string", "description": "CDP endpoint for JS rendering (e.g. http://127.0.0.1:9222)" },
                    "cdp_wait_ms": { "type": "integer", "description": "CDP wait time in ms for DOM hydration" },
                    "cdp_block_resources": { "type": "boolean", "default": true, "description": "Block heavy static resources (images, fonts, media, analytics) in CDP rendering to reduce resource loading" },
                    "cdp_auto_scroll": { "type": "boolean", "default": false, "description": "Automatically scroll down dynamic / infinite scroll pages in CDP to trigger lazy loading" },
                    "schema": { "type": "string", "description": "Extraction schema JSON or path for structured entity extraction" },
                    "schema_strict": { "type": "boolean", "default": false, "description": "Fail on Schema parsing or extraction errors instead of returning fallback content; requires schema" },
                    "chunk": { "type": "boolean", "default": false, "description": "Segment Markdown into semantic chunks for RAG" },
                    "chunk_size": { "type": "integer", "default": 500, "description": "Target token size per chunk" },
                    "chunk_overlap": { "type": "integer", "default": 50, "description": "Token overlap between chunks" },
                    "proxy": { "type": "string", "description": "HTTP/HTTPS proxy URL" },
                    "download_images": { "type": "boolean", "default": false, "description": "Download remote images to local assets directory" },
                    "assets_dir": { "type": "string", "default": "./assets", "description": "Destination directory for downloaded images" },
                    "tables_only": { "type": "boolean", "default": false, "description": "Extract only tables, discarding body text" },
                    "table_format": {
                        "type": "string",
                        "enum": ["markdown", "csv", "tsv", "json", "sql"],
                        "default": "markdown",
                        "description": "Table presentation format"
                    },
                    "mobile": { "type": "boolean", "default": false, "description": "Emulate mobile browser layout" },
                    "stats": { "type": "boolean", "default": false, "description": "Include telemetry and token economy metrics" },
                    "auto_cdp": { "type": "boolean", "default": false, "description": "Auto-fallback to CDP on anti-bot challenges" },
                    "summarize": { "type": "boolean", "default": false, "description": "Prepend extractive key takeaway summary bullets" }
                },
                "required": ["url"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "fetch_bilibili_feed".to_string(),
            description: "Fetches Bilibili recommendation, popular, or ranking video streams.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "feed_type": {
                        "type": "string",
                        "enum": ["recommendations", "popular", "ranking"],
                        "default": "recommendations",
                        "description": "Feed type"
                    },
                    "partition": {
                        "type": "string",
                        "enum": [
                            "all", "anime", "guochuang", "music", "dance", "game",
                            "tech", "knowledge", "car", "sports", "life", "food",
                            "animal", "kichiku", "fashion", "ent", "cinephile"
                        ],
                        "default": "all",
                        "description": "Category partition for rankings"
                    },
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown", "description": "Output format" },
                    "max_tokens": { "type": "integer", "description": "Max token budget" },
                    "use_cache": { "type": "boolean", "default": false, "description": "Enable response caching (opt-in, matching the CLI)" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "download_media".to_string(),
            description: "Downloads Bilibili video stream or cover image to local disk.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url_or_bvid": { "type": "string", "description": "Bilibili video URL or BV ID" },
                    "media_type": { "type": "string", "enum": ["video", "cover"], "description": "Media type to download" },
                    "destination": { "type": "string", "description": "Destination file path or directory" },
                    "quality": { "type": "string", "enum": ["720p", "480p", "360p"], "default": "720p", "description": "Video quality" },
                    "page": { "type": "integer", "default": 1, "description": "Video part number (1-indexed)" }
                },
                "required": ["url_or_bvid", "media_type"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "search_bilibili".to_string(),
            description: "Searches Bilibili videos by keyword, returning table or JSON.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "keyword": { "type": "string", "description": "Search keyword" },
                    "page": { "type": "integer", "default": 1, "description": "Page number (1-indexed)" },
                    "order": {
                        "type": "string",
                        "enum": ["totalrank", "click", "pubdate", "dm", "stow"],
                        "default": "totalrank",
                        "description": "Sort order"
                    },
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown", "description": "Output format" },
                    "use_cache": { "type": "boolean", "default": false, "description": "Enable response caching (opt-in, matching the CLI)" }
                },
                "required": ["keyword"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "web_search".to_string(),
            description: "Web search via Bing with optional instant content fetching.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "query": { "type": "string", "description": "Search query" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 20, "default": 10, "description": "Max Bing results count (1-20)" },
                    "allow_private_addresses": { "type": "boolean", "description": "Explicitly allow private/local result addresses" },
                    "scope": {
                        "type": "string",
                        "enum": ["all", "wechat", "zhihu", "github", "juejin", "csdn", "medium", "substack", "arxiv", "huggingface", "wikipedia", "reddit", "stackoverflow", "v2ex", "baike"],
                        "default": "all",
                        "description": "Platform scope filter"
                    },
                    "dedup": { "type": "boolean", "default": false, "description": "Filter near-duplicates via SimHash during fetch_content" },
                    "fetch_content": { "type": "boolean", "default": false, "description": "Concurrently fetch and clean content for each search result" },
                    "max_tokens_per_page": { "type": "integer", "minimum": 1, "description": "Max tokens per webpage; requires fetch_content=true (default: 1500)" },
                    "cdp_endpoint": { "type": "string", "description": "CDP endpoint for Bing browser search" },
                    "cdp_wait_ms": { "type": "integer", "description": "CDP wait time in ms" },
                    "auto_cdp": { "type": "boolean", "default": false, "description": "Use CDP rendering when Bing is blocked or its HTTP connection fails" },
                    "schema": { "type": "string", "description": "Extraction schema for entity extraction during fetch" },
                    "schema_strict": { "type": "boolean", "default": false, "description": "Fail on Schema parsing or extraction errors instead of returning fallback content; requires schema" },
                    "proxy": { "type": "string", "description": "HTTP/HTTPS proxy URL" },
                    "mobile": { "type": "boolean", "default": false, "description": "Emulate mobile browser during fetch" },
                    "since": { "type": "string", "maxLength": 10, "description": "Inclusive lower date bound (YYYY-MM-DD); best-effort Bing filter" },
                    "until": { "type": "string", "maxLength": 10, "description": "Inclusive upper date bound (YYYY-MM-DD); best-effort Bing filter" },
                    "language": { "type": "string", "maxLength": 16, "description": "Result language, e.g. zh-CN; best-effort Bing filter" },
                    "region": { "type": "string", "maxLength": 2, "description": "Two-letter region code, e.g. cn; best-effort Bing filter" },
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown", "description": "Output format" }
                },
                "required": ["query"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "deep_research".to_string(),
            description: "Autonomous multi-hop deep web research loop. Decomposes complex topics into sub-queries, executes Bing searches, tracks citations, and compiles evidence-linked extractive findings without LLM synthesis.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "topic": { "type": "string", "description": "Research question, thesis, or technical topic" },
                    "depth": { "type": "integer", "default": 2, "description": "Research hop depth (1: single-hop broad search, 2: two-hop citation tracking)" },
                    "breadth": { "type": "integer", "default": 3, "description": "Number of research dimensions/sub-queries to explore (2-5)" },
                    "max_pages": { "type": "integer", "default": 8, "description": "Maximum number of total pages to analyze (3-20)" },
                    "max_tokens_per_page": { "type": "integer", "default": 1500, "description": "Max tokens per webpage during deep analysis" },
                    "max_tokens": { "type": "integer", "default": 8000, "description": "Overall token budget for the compiled research report" },
                    "dedup": { "type": "boolean", "default": true, "description": "Filter near-duplicate pages via SimHash" },
                    "proxy": { "type": "string", "description": "HTTP/HTTPS proxy URL" },
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown", "description": "Output presentation format" }
                },
                "required": ["topic"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "crawl_site".to_string(),
            description: "Recursively crawls site from seed URL into Markdown knowledge base with TOC.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "seed_url": { "type": "string", "description": "Seed webpage URL" },
                    "concurrency": { "type": "integer", "minimum": 1, "maximum": 32, "default": 5 },
                    "max_depth": { "type": "integer", "default": 2, "description": "Max crawl depth" },
                    "max_pages": { "type": "integer", "default": 20, "description": "Max pages limit" },
                    "same_path_prefix": { "type": "boolean", "default": false, "description": "Restrict crawl to same path prefix" },
                    "sitemap": { "type": "boolean", "default": false, "description": "Parse seed URL as XML sitemap to extract URLs" },
                    "max_tokens": { "type": "integer", "description": "Overall token budget for assembled knowledge base" },
                    "cdp_endpoint": { "type": "string", "description": "CDP endpoint for JS-rendered crawling" },
                    "cdp_wait_ms": { "type": "integer", "description": "CDP wait time in ms" },
                    "schema": { "type": "string", "description": "Extraction schema for entity extraction per page" },
                    "schema_strict": { "type": "boolean", "default": false, "description": "Fail on Schema parsing or extraction errors instead of returning fallback content; requires schema" },
                    "dedup": { "type": "boolean", "default": false, "description": "Skip near-duplicate pages via SimHash" },
                    "proxy": { "type": "string", "description": "HTTP/HTTPS proxy URL" },
                    "mobile": { "type": "boolean", "default": false, "description": "Emulate mobile browser" },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json", "jsonl"],
                        "default": "markdown",
                        "description": "Output format: markdown (with TOC), json, or jsonl"
                    },
                    "delay_ms": { "type": "integer", "default": 0, "description": "Inter-request delay in ms" }
                },
                "required": ["seed_url"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "batch_fetch_webpages".to_string(),
            description: "Concurrently fetches and cleans multiple URLs in a single request.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "urls": { "type": "array", "minItems": 1, "items": { "type": "string", "minLength": 1 }, "description": "Ordered webpage URL list; each entry retains its zero-based index" },
                    "concurrency": { "type": "integer", "minimum": 1, "maximum": 20, "default": 5, "description": "Max concurrent page tasks" },
                    "per_host_concurrency": { "type": "integer", "minimum": 1, "maximum": 20, "default": 2, "description": "Max simultaneous page tasks per host" },
                    "per_host_delay_ms": { "type": "integer", "minimum": 0, "maximum": 3600000, "default": 0, "description": "Minimum start interval per host" },
                    "order": { "type": "string", "enum": ["input", "completion"], "default": "input", "description": "Result delivery order" },
                    "checkpoint": { "type": "string", "minLength": 1, "description": "Local durable checkpoint path; a new job requires a new file" },
                    "resume": { "type": "boolean", "default": false, "description": "Resume the checkpoint; requires checkpoint" },
                    "retry_failed": { "type": "boolean", "default": false, "description": "Re-fetch failed entries on resume; successful entries are reused" },
                    "stream_results": { "type": "boolean", "default": false, "description": "Send each item in notifications/progress params._meta['tkless/batch_item']; requires _meta.progressToken; final response is summary only" },
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown", "description": "Output format" },
                    "max_tokens_per_page": { "type": "integer", "description": "Max token budget per webpage" },
                    "selector": { "type": "string", "description": "CSS selector to extract across all pages" },
                    "remove_selector": { "type": "string", "description": "CSS selector to exclude/prune across all pages" },
                    "extract_links": { "type": "boolean", "default": false, "description": "Extract structured hyperlinks from each page" },
                    "extract_tables": { "type": "boolean", "default": false, "description": "Extract structured tables from each page" },
                    "no_images": { "type": "boolean", "default": false, "description": "Strip images from Markdown output" },
                    "no_links": { "type": "boolean", "default": false, "description": "Strip hyperlinks, keeping anchor text" },
                    "schema": { "type": "string", "description": "Extraction schema for structured entities" },
                    "schema_strict": { "type": "boolean", "default": false, "description": "Fail on Schema parsing or extraction errors instead of returning fallback content; requires schema" },
                    "cdp_endpoint": { "type": "string", "description": "CDP endpoint for JS rendering" },
                    "cdp_wait_ms": { "type": "integer", "description": "CDP wait time in ms" },
                    "proxy": { "type": "string", "description": "HTTP/HTTPS proxy URL" },
                    "mobile": { "type": "boolean", "default": false, "description": "Emulate mobile browser" },
                    "delay_ms": { "type": "integer", "minimum": 0, "maximum": 3600000, "default": 0, "description": "Global page-task start interval in ms" }
                },
                "required": ["urls"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "clean_html".to_string(),
            description: "Sanitizes raw HTML string into clean Markdown or structured entities.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "html": { "type": "string", "description": "Raw HTML string content to clean" },
                    "base_url": { "type": "string", "description": "Base URL to resolve relative links and images" },
                    "format": { "type": "string", "enum": ["markdown", "json", "text", "html"], "default": "markdown", "description": "Output format" },
                    "mode": { "type": "string", "enum": ["article", "full", "raw"], "default": "article", "description": "Cleaning mode: article, full, or raw" },
                    "selector": { "type": "string", "description": "CSS selector to extract" },
                    "remove_selector": { "type": "string", "description": "CSS selector to exclude/prune" },
                    "schema": { "type": "string", "description": "Extraction schema for entity extraction" },
                    "schema_strict": { "type": "boolean", "default": false, "description": "Fail on Schema parsing or extraction errors instead of returning fallback content; requires schema" },
                    "max_tokens": { "type": "integer", "description": "Max token budget with safe truncation" },
                    "strip_images": { "type": "boolean", "default": false, "description": "Strip images from Markdown" },
                    "strip_links": { "type": "boolean", "default": false, "description": "Strip hyperlinks, keeping only anchor text" },
                    "frontmatter": { "type": "boolean", "default": false, "description": "Prepend YAML frontmatter with metadata" },
                    "chunk": { "type": "boolean", "default": false, "description": "Segment Markdown into semantic chunks for RAG" },
                    "chunk_size": { "type": "integer", "default": 500, "description": "Target token size per chunk" },
                    "chunk_overlap": { "type": "integer", "default": 50, "description": "Token overlap between chunks" },
                    "tables_only": { "type": "boolean", "default": false, "description": "Extract only tables, discarding body text" },
                    "table_format": { "type": "string", "enum": ["markdown", "csv", "tsv", "json", "sql"], "default": "markdown", "description": "Table format" },
                    "stats": { "type": "boolean", "default": false, "description": "Include telemetry and token economy metrics" },
                    "summarize": { "type": "boolean", "default": false, "description": "Prepend extractive key takeaway summary bullets" }
                },
                "required": ["html"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "login_platform".to_string(),
            description: "Interactive QR code login flow to extract and save platform cookies.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "platform": { "type": "string", "enum": ["bilibili", "zhihu"], "description": "Target platform to log in" },
                    "output_cookie_file": { "type": "string", "default": "./cookies.txt", "description": "Destination file path to save cookies" },
                    "no_browser": { "type": "boolean", "default": true, "description": "Print terminal QR code without launching system browser" },
                    "timeout_secs": { "type": "integer", "default": 180, "description": "Timeout in seconds for scanning QR code" }
                },
                "required": ["platform"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "scaffold_schema".to_string(),
            description: "Detects repeating DOM containers and recommends extraction schema.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Webpage URL to fetch and analyze" },
                    "html": { "type": "string", "description": "Raw HTML string to analyze directly" },
                    "cdp_endpoint": { "type": "string", "description": "CDP endpoint for JS-rendered dynamic pages" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "compare_similarity".to_string(),
            description: "Computes 64-bit SimHash and Hamming distance between two texts.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "text1": { "type": "string", "description": "First text or HTML string" },
                    "text2": { "type": "string", "description": "Second text or HTML string" },
                    "max_hamming_distance": { "type": "integer", "default": 3, "description": "Max Hamming distance threshold for near-duplicate" }
                },
                "required": ["text1", "text2"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "manage_cache".to_string(),
            description: "Inspects cache statistics or purges cached responses.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["stats", "clear", "prune_expired"],
                        "default": "stats",
                        "description": "Cache operation"
                    },
                    "cache_dir": { "type": "string", "description": "Custom cache directory path override" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "fetch_feed".to_string(),
            description: "Parses RSS 2.0 or Atom 1.0 XML feed into Markdown or JSON.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Feed URL to fetch" },
                    "xml": { "type": "string", "description": "Raw RSS/Atom XML string to parse directly" },
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown", "description": "Output format" },
                    "limit": { "type": "integer", "description": "Max feed items to return" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "inspect_links".to_string(),
            description: "Checks reachability, status codes, and latency for a list of URLs.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "urls": { "type": "array", "items": { "type": "string" }, "description": "List of URLs to verify" },
                    "concurrency": { "type": "integer", "default": 10, "description": "Max concurrent network requests" },
                    "timeout_secs": { "type": "integer", "default": 10, "description": "Timeout in seconds per link" },
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown", "description": "Output format" }
                },
                "required": ["urls"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "check_auth".to_string(),
            description: "Checks if platform session cookies are valid and authenticated.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "platform": { "type": "string", "enum": ["bilibili", "zhihu", "github", "wechat"], "description": "Target platform" },
                    "cookie_file": { "type": "string", "default": "./cookies.txt", "description": "Path to cookie file" },
                    "cookie": { "type": "string", "description": "Direct cookie header string" }
                },
                "required": ["platform"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "summarize_content".to_string(),
            description: "Extracts key takeaway bullet points (TL;DR) without LLM API calls.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Webpage URL to fetch and summarize" },
                    "text": { "type": "string", "description": "Raw text or Markdown content to summarize directly" },
                    "max_bullets": { "type": "integer", "default": 3, "description": "Max number of key takeaway bullets (1-10)" },
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown", "description": "Output format" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "split_document".to_string(),
            description: "Splits Markdown document into sections by heading hierarchy.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "markdown": { "type": "string", "description": "Markdown document string to split" },
                    "max_heading_level": { "type": "integer", "default": 2, "description": "Max heading depth level (1-3)" }
                },
                "required": ["markdown"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "chunk_markdown".to_string(),
            description: "Splits Markdown into semantic heading-aware chunks for RAG vector DBs.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Webpage URL to fetch, convert, and chunk" },
                    "markdown": { "type": "string", "description": "Markdown content to chunk directly" },
                    "chunk_size": { "type": "integer", "default": 500, "description": "Target token budget per chunk" },
                    "chunk_overlap": { "type": "integer", "default": 50, "description": "Token overlap between chunks" },
                    "tokenizer": { "type": "string", "enum": ["o200k", "cl100k", "heuristic"], "default": "o200k", "description": "Tokenizer model encoding" },
                    "format": { "type": "string", "enum": ["json", "markdown"], "default": "json", "description": "Output format" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "diff_documents".to_string(),
            description: "Computes unified diff and change metrics between two document versions.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "old_text": { "type": "string", "description": "Original text version" },
                    "new_text": { "type": "string", "description": "Updated text version" },
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown", "description": "Output format" }
                },
                "required": ["old_text", "new_text"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "take_screenshot".to_string(),
            description: "Captures high-res webpage screenshot via Chrome DevTools Protocol (CDP).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Webpage URL to render and screenshot" },
                    "output_path": { "type": "string", "description": "Local file path to save image" },
                    "format": { "type": "string", "enum": ["png", "jpeg"], "default": "png", "description": "Image format" },
                    "full_page": { "type": "boolean", "default": true, "description": "Capture full scrollable page height" },
                    "cdp_endpoint": { "type": "string", "description": "CDP endpoint (default: http://127.0.0.1:9222)" },
                    "cdp_wait_ms": { "type": "integer", "default": 1000, "description": "Wait time in ms for DOM hydration" }
                },
                "required": ["url"]
            }),
        },
        McpTool {
            output_schema: None,
            name: "export_tables".to_string(),
            description: "Extracts HTML tables into SQL DDL/INSERT, CSV, TSV, JSON, or Markdown.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Webpage URL to fetch and extract tables from" },
                    "html": { "type": "string", "description": "Raw HTML string containing <table> elements" },
                    "format": {
                        "type": "string",
                        "enum": ["sql", "csv", "tsv", "json", "markdown"],
                        "default": "sql",
                        "description": "Table export format"
                    },
                    "table_name": { "type": "string", "default": "extracted_table", "description": "SQL table name identifier" },
                    "table_index": { "type": "integer", "description": "0-indexed specific table to extract" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "list_adapters".to_string(),
            description: "Lists supported platform adapters, domain patterns, and capabilities.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown", "description": "Output format" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "jobs_start".to_string(),
            description: "Starts a long-running tool in the background and returns a job_id immediately. Jobs are in-process, capped at 4 active, and expire after their TTL or on restart. Poll with jobs_status.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["tool", "arguments"],
                "properties": {
                    "tool": { "type": "string", "enum": ["crawl_site", "batch_fetch_webpages", "collect_pages", "sync_collection"], "description": "Target tool to run asynchronously" },
                    "arguments": { "type": "object", "description": "Arguments object matching the target tool inputSchema" },
                    "ttl_secs": { "type": "integer", "minimum": 60, "maximum": 86400, "default": 3600, "description": "Retention after the job reaches a terminal state" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "jobs_status".to_string(),
            description: "Returns one background job's state, elapsed time, progress, error and the final structured result once completed.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["job_id"],
                "properties": {
                    "job_id": { "type": "string", "minLength": 1, "description": "Identifier returned by jobs_start" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "jobs_cancel".to_string(),
            description: "Cancels a queued or running job. Cancelling a finished job is idempotent and returns its final state.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["job_id"],
                "properties": {
                    "job_id": { "type": "string", "minLength": 1, "description": "Identifier returned by jobs_start" }
                }
            }),
        },
        McpTool {
            output_schema: None,
            name: "jobs_list".to_string(),
            description: "Lists in-process background jobs, newest first. Summaries omit results; use jobs_status for a completed job's result.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "state": { "type": "string", "enum": ["queued", "running", "completed", "failed", "cancelled"], "description": "Optional state filter" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20, "description": "Maximum summaries to return" }
                }
            }),
        }
    ];
    for tool in &mut tools {
        if tool.name == "collect_pages" || tool.name == "sync_collection" {
            tool.input_schema["properties"]["config"] = crate::collection::config_schema();
        }
        if tool.name == "browse_page" {
            tool.input_schema["properties"]["plan"] = crate::browser_workflow::plan_schema();
        }
        if tool.name == "crawl_site" {
            let properties = tool.input_schema["properties"].as_object_mut().unwrap();
            properties.extend(serde_json::json!({
                "checkpoint":{"type":"string","minLength":1},"resume":{"type":"boolean","default":false},
                "retry_failed":{"type":"boolean","default":false},
                "max_seconds":{"type":"integer","minimum":1,"maximum":86400},
                "max_output_bytes":{"type":"integer","minimum":1},
                "max_pending_urls":{"type":"integer","minimum":1,"maximum":100000},
                "respect_robots":{"type":"boolean","default":false}
            }).as_object().unwrap().clone());
        }
        tool.output_schema = super::contracts::output_schema(&tool.name);
        if matches!(
            tool.name.as_str(),
            "fetch_webpage" | "clean_html" | "batch_fetch_webpages"
        ) {
            tool.input_schema["additionalProperties"] = serde_json::json!(false);
        }
        if tool.name == "web_search" {
            tool.input_schema["properties"]["diagnostics"] = serde_json::json!({"type":"boolean","default":false,"description":"Include query and filtering diagnostics in Markdown output; JSON and structuredContent always include them"});
        }
    }
    tools
}
