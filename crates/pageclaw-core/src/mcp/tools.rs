use super::types::McpTool;

/// Returns the complete registry of MCP tools exposed by PageClaw.
pub fn get_available_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "fetch_webpage".to_string(),
            description: "Fetches any webpage or Bilibili video URL, sanitizes DOM noise, and extracts core content into LLM-optimized Markdown or structured JSON with token budget management.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Target webpage URL (e.g. https://docs.rs/tokio) or Bilibili video URL / BV ID"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json", "text", "html", "raw"],
                        "default": "markdown",
                        "description": "Output format: 'markdown' (default GFM), 'json' (structured document payload), 'text' (clean plain text), 'html' (sanitized HTML), 'raw' (unmodified HTML)"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["article", "full", "raw"],
                        "default": "article",
                        "description": "Extraction mode: 'article' (Readability heuristic body extraction), 'full' (sanitized whole page body), 'raw' (unparsed HTML)"
                    },
                    "max_tokens": {
                        "type": "integer",
                        "description": "Maximum token budget. Content will be safely truncated at sentence/paragraph boundaries with auto-closed code blocks"
                    },
                    "tokenizer": {
                        "type": "string",
                        "enum": ["o200k", "cl100k", "heuristic"],
                        "default": "o200k",
                        "description": "Tokenizer model encoding scheme for token counting: 'o200k' (default, GPT-4o/Claude 3.5), 'cl100k' (GPT-4), 'heuristic'"
                    },
                    "frontmatter": {
                        "type": "boolean",
                        "default": false,
                        "description": "Prepend YAML frontmatter with title, URL, date, and metadata (Markdown format only)"
                    },
                    "no_images": {
                        "type": "boolean",
                        "default": false,
                        "description": "Strip images from Markdown to conserve token budget"
                    },
                    "no_links": {
                        "type": "boolean",
                        "default": false,
                        "description": "Strip hyperlinks, keeping only the anchor text"
                    },
                    "no_metadata": {
                        "type": "boolean",
                        "default": false,
                        "description": "Do not extract document metadata (OpenGraph, Twitter Cards, description)"
                    },
                    "no_adapter": {
                        "type": "boolean",
                        "default": false,
                        "description": "Disable platform-specific API adapters and force raw HTML scraping"
                    },
                    "timeout": {
                        "type": "integer",
                        "default": 15,
                        "description": "Total request timeout in seconds"
                    },
                    "connect_timeout": {
                        "type": "integer",
                        "default": 10,
                        "description": "Connection handshake timeout in seconds"
                    },
                    "retries": {
                        "type": "integer",
                        "default": 0,
                        "description": "Number of retry attempts with exponential backoff on 5xx or network errors"
                    },
                    "on_too_large": {
                        "type": "string",
                        "enum": ["error", "truncate"],
                        "default": "error",
                        "description": "Behavior when response size exceeds max-size: 'error' (default) or 'truncate'"
                    },
                    "allow_private_addresses": {
                        "type": "boolean",
                        "default": false,
                        "description": "Allow fetching private IP addresses, localhost, and internal network endpoints"
                    },
                    "allow_http_errors": {
                        "type": "boolean",
                        "default": false,
                        "description": "Do not fail with an error on HTTP 4xx or 5xx error responses"
                    },
                    "use_cache": {
                        "type": "boolean",
                        "default": true,
                        "description": "Whether to use response cache to avoid repeated network requests"
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for targeted DOM extraction (e.g. 'article.content, #readme, table')"
                    },
                    "remove_selector": {
                        "type": "string",
                        "description": "CSS selector to prune and exclude unwanted DOM subtrees (e.g. '.ads, .footer, #comments, .author-bio')"
                    },
                    "extract_links": {
                        "type": "boolean",
                        "default": false,
                        "description": "Extract structured hyperlink assets list"
                    },
                    "extract_images": {
                        "type": "boolean",
                        "default": false,
                        "description": "Extract structured image assets list"
                    },
                    "extract_tables": {
                        "type": "boolean",
                        "default": false,
                        "description": "Extract structured 2D tables and row dictionary records"
                    },
                    "subtitles": {
                        "type": "boolean",
                        "default": false,
                        "description": "Extract official/AI CC subtitle tracks (for Bilibili video URLs)"
                    },
                    "danmaku": {
                        "type": "boolean",
                        "default": false,
                        "description": "Extract timestamped danmaku bullet comments (for Bilibili video URLs)"
                    },
                    "danmaku_limit": {
                        "type": "integer",
                        "description": "Maximum number of danmaku bullet comments to extract"
                    },
                    "comments": {
                        "type": "boolean",
                        "default": false,
                        "description": "Extract top liked comments (for Bilibili video URLs)"
                    },
                    "comments_limit": {
                        "type": "integer",
                        "description": "Maximum number of top comments to extract"
                    },
                    "conclusion": {
                        "type": "boolean",
                        "default": false,
                        "description": "Extract official AI video summary, key takeaways, and timestamped chapter outline (for Bilibili video URLs)"
                    },
                    "cookie": {
                        "type": "string",
                        "description": "Cookie header string for authenticated requests"
                    },
                    "cookie_file": {
                        "type": "string",
                        "description": "Path to cookie file (supports platform prefix format like bilibili: ... or [platform] INI sections)"
                    },
                    "cdp_endpoint": {
                        "type": "string",
                        "description": "Chrome DevTools Protocol (CDP) endpoint (e.g. 'http://127.0.0.1:9222') for JS-rendered dynamic SPAs or anti-bot challenge bypass"
                    },
                    "cdp_wait_ms": {
                        "type": "integer",
                        "description": "Additional wait time in milliseconds for CDP DOM hydration (default: 1000)"
                    },
                    "schema": {
                        "type": "string",
                        "description": "Extraction schema (JSON string or file path) defining fields/selectors to extract structured JSON entities directly"
                    },
                    "chunk": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to segment Markdown into semantic heading-aware chunks for RAG vector ingestion"
                    },
                    "chunk_size": {
                        "type": "integer",
                        "default": 500,
                        "description": "Target token size per chunk (default: 500)"
                    },
                    "chunk_overlap": {
                        "type": "integer",
                        "default": 50,
                        "description": "Token overlap between consecutive chunks (default: 50)"
                    },
                    "proxy": {
                        "type": "string",
                        "description": "Optional HTTP/HTTPS proxy URL (e.g. 'http://127.0.0.1:7890')"
                    },
                    "download_images": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to download remote images to local assets directory and rewrite Markdown links to local relative paths"
                    },
                    "assets_dir": {
                        "type": "string",
                        "default": "./assets",
                        "description": "Destination directory for downloaded image assets (default: ./assets)"
                    },
                    "tables_only": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to extract and output only the tables, discarding the body text"
                    },
                    "table_format": {
                        "type": "string",
                        "enum": ["markdown", "csv", "tsv", "json"],
                        "default": "markdown",
                        "description": "Presentation format for table export: 'markdown' (default), 'csv', 'tsv', or 'json'"
                    },
                    "mobile": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to emulate a mobile browser User-Agent to trigger lightweight mobile layouts"
                    },
                    "stats": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to compute and include telemetry performance and token economy metrics"
                    },
                    "auto_cdp": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to automatically detect anti-scraping challenges (Cloudflare, captcha) and fall back to CDP headless rendering"
                    },
                    "summarize": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to extract and prepend local heuristic key takeaways / summary bullet points"
                    }
                },
                "required": ["url"]
            }),
        },
        McpTool {
            name: "fetch_bilibili_feed".to_string(),
            description: "Fetches Bilibili recommendation streams, general trending leaderboards, or category-specific partition rankings with sub-200ms latency.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "feed_type": {
                        "type": "string",
                        "enum": ["recommendations", "popular", "ranking"],
                        "default": "recommendations",
                        "description": "Feed type: 'recommendations' (homepage feed), 'popular' (general hot stream), 'ranking' (leaderboard)"
                    },
                    "partition": {
                        "type": "string",
                        "enum": [
                            "all", "anime", "guochuang", "music", "dance", "game",
                            "tech", "knowledge", "car", "sports", "life", "food",
                            "animal", "kichiku", "fashion", "ent", "cinephile"
                        ],
                        "default": "all",
                        "description": "Category partition for rankings (e.g. 'all', 'anime', 'tech', 'game', 'knowledge')"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json"],
                        "default": "markdown",
                        "description": "Output presentation format: 'markdown' (GFM table) or 'json'"
                    },
                    "max_tokens": {
                        "type": "integer",
                        "description": "Maximum token budget for feed table"
                    },
                    "use_cache": {
                        "type": "boolean",
                        "default": true,
                        "description": "Whether to use response cache"
                    }
                }
            }),
        },
        McpTool {
            name: "download_media".to_string(),
            description: "Downloads Bilibili video cover images or progressive MP4 video streams to local disk without external ffmpeg dependencies.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url_or_bvid": {
                        "type": "string",
                        "description": "Bilibili video URL (e.g. https://www.bilibili.com/video/BV1xx411c7mD) or bare BV ID"
                    },
                    "media_type": {
                        "type": "string",
                        "enum": ["video", "cover"],
                        "description": "Media type to download: 'video' (progressive MP4 stream) or 'cover' (HD cover image)"
                    },
                    "destination": {
                        "type": "string",
                        "description": "Optional destination file path or directory. Defaults to '{bvid}.mp4' or '{bvid}_cover.jpg'"
                    },
                    "quality": {
                        "type": "string",
                        "enum": ["720p", "480p", "360p"],
                        "default": "720p",
                        "description": "Video quality selection (for 'video' media_type)"
                    },
                    "page": {
                        "type": "integer",
                        "default": 1,
                        "description": "Video page / part number for multi-part videos (1-indexed)"
                    }
                },
                "required": ["url_or_bvid", "media_type"]
            }),
        },
        McpTool {
            name: "search_bilibili".to_string(),
            description: "Searches Bilibili for videos by keyword and returns a structured GFM Markdown table or JSON payload with titles, BV IDs, authors, view counts, danmaku counts, durations, and publication dates.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "keyword": {
                        "type": "string",
                        "description": "Search keyword or query (e.g. 'Rust教程')"
                    },
                    "page": {
                        "type": "integer",
                        "default": 1,
                        "description": "Page number for paginated search results (1-indexed, default: 1)"
                    },
                    "order": {
                        "type": "string",
                        "enum": ["totalrank", "click", "pubdate", "dm", "stow"],
                        "default": "totalrank",
                        "description": "Sort order: 'totalrank' (comprehensive/default), 'click' (most views), 'pubdate' (latest), 'dm' (most danmaku), 'stow' (most favorites)"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json"],
                        "default": "markdown",
                        "description": "Output presentation format: 'markdown' (GFM table) or 'json' (structured document with Schema.org VideoObject list)"
                    },
                    "use_cache": {
                        "type": "boolean",
                        "default": true,
                        "description": "Whether to use response cache"
                    }
                },
                "required": ["keyword"]
            }),
        },
        McpTool {
            name: "web_search".to_string(),
            description: "Performs a zero-API-key web search using Microsoft Bing, Google, or Auto failover and returns structured results with titles, direct URLs, snippets, and source engine.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query or question"
                    },
                    "limit": {
                        "type": "integer",
                        "default": 5,
                        "description": "Maximum number of search results to return (default: 5, max: 20)"
                    },
                    "engine": {
                        "type": "string",
                        "enum": ["bing", "google", "auto"],
                        "default": "bing",
                        "description": "Search engine provider: 'bing' (default, fast connectivity and great Chinese coverage), 'google' (global documentation index), or 'auto' (failover)"
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["all", "wechat", "zhihu", "github", "juejin", "csdn", "medium", "substack", "arxiv", "huggingface", "wikipedia", "reddit", "stackoverflow"],
                        "default": "all",
                        "description": "Search scope limitation: 'all' (entire web), 'wechat' (WeChat Official Accounts), 'zhihu' (Zhihu questions and articles), 'github' (GitHub repositories and issues), 'juejin' (Juejin tech posts), 'csdn' (CSDN blog posts), 'medium' (Medium articles), 'substack' (Substack newsletters), 'arxiv' (arXiv research papers), 'huggingface' (Hugging Face models, papers, blogs), 'wikipedia' (Wikipedia articles), 'reddit' (Reddit discussions), or 'stackoverflow' (Stack Overflow Q&A)"
                    },
                    "dedup": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to filter out near-duplicate and mirror pages using 64-bit SimHash when fetch_content is enabled"
                    },
                    "fetch_content": {
                        "type": "boolean",
                        "default": false,
                        "description": "Concurrently fetch and extract clean Markdown content for each search result in parallel (All-in-One search & fetch)"
                    },
                    "max_tokens_per_page": {
                        "type": "integer",
                        "default": 1500,
                        "description": "Maximum tokens extracted per webpage when fetch_content is enabled [default: 1500]"
                    },
                    "cdp_endpoint": {
                        "type": "string",
                        "description": "Optional Chrome DevTools Protocol (CDP) endpoint (e.g. 'http://127.0.0.1:9222') for JS rendering when fetch_content is enabled"
                    },
                    "cdp_wait_ms": {
                        "type": "integer",
                        "description": "Additional wait time in milliseconds for CDP DOM hydration when fetch_content is enabled"
                    },
                    "schema": {
                        "type": "string",
                        "description": "Optional extraction schema (JSON string or file path) for structured entity extraction when fetch_content is enabled"
                    },
                    "proxy": {
                        "type": "string",
                        "description": "Optional HTTP/HTTPS proxy URL (e.g. 'http://127.0.0.1:7890')"
                    },
                    "mobile": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to emulate a mobile browser User-Agent when fetch_content is enabled"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json"],
                        "default": "markdown",
                        "description": "Output presentation format: 'markdown' (GFM numbered list) or 'json'"
                    }
                },
                "required": ["query"]
            }),
        },
        McpTool {
            name: "crawl_site".to_string(),
            description: "Deeply crawls documentation or content pages from a seed URL (BFS link discovery) and assembles an integrated Markdown knowledge base with Table of Contents (TOC) or structured JSON.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "seed_url": {
                        "type": "string",
                        "description": "Seed webpage URL to start deep crawling"
                    },
                    "max_depth": {
                        "type": "integer",
                        "default": 2,
                        "description": "Maximum link traversal depth (default: 2, max: 5)"
                    },
                    "max_pages": {
                        "type": "integer",
                        "default": 20,
                        "description": "Maximum number of pages to crawl (default: 20, max: 100)"
                    },
                    "same_path_prefix": {
                        "type": "boolean",
                        "default": false,
                        "description": "Restrict crawling strictly to URLs sharing the same path prefix as seed URL"
                    },
                    "sitemap": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to parse seed URL as XML sitemap to extract and crawl canonical URLs directly"
                    },
                    "max_tokens": {
                        "type": "integer",
                        "description": "Maximum overall token budget for the assembled crawl knowledge base"
                    },
                    "cdp_endpoint": {
                        "type": "string",
                        "description": "Optional Chrome DevTools Protocol endpoint (e.g. 'http://127.0.0.1:9222') for JS-rendered crawling"
                    },
                    "cdp_wait_ms": {
                        "type": "integer",
                        "description": "Additional wait time in milliseconds for CDP DOM hydration during crawling"
                    },
                    "schema": {
                        "type": "string",
                        "description": "Optional extraction schema (JSON string or file path) to extract structured entities from each crawled page"
                    },
                    "dedup": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to automatically skip near-duplicate crawled pages based on 64-bit SimHash Hamming distance"
                    },
                    "proxy": {
                        "type": "string",
                        "description": "Optional HTTP/HTTPS proxy URL (e.g. 'http://127.0.0.1:7890')"
                    },
                    "mobile": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to emulate a mobile browser User-Agent during crawl"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json", "jsonl"],
                        "default": "markdown",
                        "description": "Output presentation format: 'markdown' (assembled knowledge base with TOC), 'json', or 'jsonl' (line-delimited JSON for RAG vector databases)"
                    },
                    "delay_ms": {
                        "type": "integer",
                        "default": 0,
                        "description": "Inter-request delay in milliseconds between crawled pages to respect server rate limits"
                    }
                },
                "required": ["seed_url"]
            }),
        },
        McpTool {
            name: "batch_fetch_webpages".to_string(),
            description: "Concurrently fetches and cleans a batch of URLs using a multi-threaded asynchronous worker pool, returning aggregated Markdown or JSON results in a single round-trip.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "urls": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of webpage URLs to fetch and extract concurrently"
                    },
                    "concurrency": {
                        "type": "integer",
                        "default": 5,
                        "description": "Maximum concurrent network requests (default: 5, max: 20)"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json"],
                        "default": "markdown",
                        "description": "Output format: 'markdown' (separated by dividers) or 'json' (array of document objects)"
                    },
                    "max_tokens_per_page": {
                        "type": "integer",
                        "description": "Maximum token budget per webpage"
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for targeted DOM extraction across all pages"
                    },
                    "remove_selector": {
                        "type": "string",
                        "description": "CSS selector to prune and exclude unwanted DOM subtrees across all pages"
                    },
                    "schema": {
                        "type": "string",
                        "description": "Optional extraction schema (JSON string) for structured entity extraction"
                    },
                    "cdp_endpoint": {
                        "type": "string",
                        "description": "Optional Chrome DevTools Protocol endpoint (e.g. 'http://127.0.0.1:9222')"
                    },
                    "cdp_wait_ms": {
                        "type": "integer",
                        "description": "Wait time in milliseconds for CDP DOM hydration"
                    },
                    "proxy": {
                        "type": "string",
                        "description": "Optional HTTP/HTTPS proxy URL (e.g. 'http://127.0.0.1:7890')"
                    },
                    "mobile": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to emulate a mobile browser User-Agent during batch fetching"
                    },
                    "delay_ms": {
                        "type": "integer",
                        "default": 0,
                        "description": "Inter-request delay in milliseconds between batch requests to respect server rate limits"
                    }
                },
                "required": ["urls"]
            }),
        },
        McpTool {
            name: "clean_html".to_string(),
            description: "Locally sanitizes and cleans an arbitrary HTML string (e.g. from local file, email, or external scraper), strips scripts/ads/noise, and converts it into pure Markdown or extracts structured entities via CSS selector/Schema.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "html": {
                        "type": "string",
                        "description": "Raw HTML string content to clean"
                    },
                    "base_url": {
                        "type": "string",
                        "description": "Optional base URL to resolve relative hyperlinks and images"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json", "text", "html"],
                        "default": "markdown",
                        "description": "Output presentation format: 'markdown', 'json', 'text', or 'html'"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["article", "full", "raw"],
                        "default": "article",
                        "description": "Cleaning mode: 'article' (Readability heuristic body extraction), 'full' (sanitized whole body), 'raw'"
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for targeted DOM extraction"
                    },
                    "remove_selector": {
                        "type": "string",
                        "description": "CSS selector to prune and exclude unwanted DOM subtrees"
                    },
                    "schema": {
                        "type": "string",
                        "description": "Extraction schema (JSON string) defining fields to extract structured JSON directly"
                    },
                    "max_tokens": {
                        "type": "integer",
                        "description": "Maximum token budget with smart sentence-boundary truncation"
                    },
                    "strip_images": {
                        "type": "boolean",
                        "default": false,
                        "description": "Strip images from Markdown"
                    },
                    "strip_links": {
                        "type": "boolean",
                        "default": false,
                        "description": "Strip hyperlinks, keeping only the anchor text"
                    },
                    "frontmatter": {
                        "type": "boolean",
                        "default": false,
                        "description": "Prepend YAML frontmatter with title, URL, date, and metadata"
                    },
                    "chunk": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to segment Markdown into semantic heading-aware chunks for RAG vector ingestion"
                    },
                    "chunk_size": {
                        "type": "integer",
                        "default": 500,
                        "description": "Target token size per chunk (default: 500)"
                    },
                    "chunk_overlap": {
                        "type": "integer",
                        "default": 50,
                        "description": "Token overlap between consecutive chunks (default: 50)"
                    },
                    "tables_only": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to extract and output only the tables, discarding the body text"
                    },
                    "table_format": {
                        "type": "string",
                        "enum": ["markdown", "csv", "tsv", "json"],
                        "default": "markdown",
                        "description": "Presentation format for table export: 'markdown' (default), 'csv', 'tsv', or 'json'"
                    },
                    "stats": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to compute and include telemetry performance and token economy metrics"
                    },
                    "summarize": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether to extract and prepend local heuristic key takeaways / summary bullet points"
                    }
                },
                "required": ["html"]
            }),
        },
        McpTool {
            name: "login_platform".to_string(),
            description: "Initiates an interactive QR code login flow for supported platforms (e.g. 'bilibili', 'zhihu') to extract authenticated cookies and save them to cookies.txt with platform-prefix isolation.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "platform": {
                        "type": "string",
                        "enum": ["bilibili", "zhihu"],
                        "description": "Target platform to log in ('bilibili' or 'zhihu')"
                    },
                    "output_cookie_file": {
                        "type": "string",
                        "default": "./cookies.txt",
                        "description": "Destination file path to save extracted cookies (default: ./cookies.txt)"
                    },
                    "no_browser": {
                        "type": "boolean",
                        "default": true,
                        "description": "Whether to skip opening the system browser automatically (outputs QR code text in tool result, default: true)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "default": 180,
                        "description": "Maximum waiting timeout in seconds for user to scan the QR code (default: 180)"
                    }
                },
                "required": ["platform"]
            }),
        },
        McpTool {
            name: "scaffold_schema".to_string(),
            description: "Analyzes the DOM structure of a webpage URL or raw HTML string, detects repeating list containers (e.g. products, articles, cards), and generates a ready-to-use ExtractionSchema template to drastically save AI Agent tokens.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Webpage URL to fetch and analyze DOM skeleton"
                    },
                    "html": {
                        "type": "string",
                        "description": "Optional raw HTML string to analyze directly without network request"
                    },
                    "cdp_endpoint": {
                        "type": "string",
                        "description": "Optional Chrome DevTools Protocol endpoint (e.g. 'http://127.0.0.1:9222') for JS-rendered dynamic pages"
                    }
                }
            }),
        },
        McpTool {
            name: "compare_similarity".to_string(),
            description: "Calculates the 64-bit SimHash and Hamming distance between two texts (or HTML snippets) to detect near-duplicates, syndicated copies, or plagiarism (Hamming distance <= 3 is considered a near-duplicate).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "text1": {
                        "type": "string",
                        "description": "First text or HTML string"
                    },
                    "text2": {
                        "type": "string",
                        "description": "Second text or HTML string"
                    },
                    "max_hamming_distance": {
                        "type": "integer",
                        "default": 3,
                        "description": "Maximum Hamming distance threshold to qualify as near-duplicate (default: 3)"
                    }
                },
                "required": ["text1", "text2"]
            }),
        },
        McpTool {
            name: "manage_cache".to_string(),
            description: "Inspects telemetry statistics (memory/disk entries and disk space usage) or purges/clears the local HTTP response and document cache.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["stats", "clear", "prune_expired"],
                        "default": "stats",
                        "description": "Cache operation: 'stats' (query cache size/entries), 'clear' (empty all cache), 'prune_expired' (remove stale records)"
                    },
                    "cache_dir": {
                        "type": "string",
                        "description": "Optional custom cache directory path override"
                    }
                }
            }),
        },
        McpTool {
            name: "fetch_feed".to_string(),
            description: "Fetches and parses an RSS 2.0 or Atom 1.0 XML syndication feed URL (or raw XML string) into structured JSON or clean Markdown reading list.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Feed URL to fetch (e.g. 'https://blog.rust-lang.org/feed.xml')"
                    },
                    "xml": {
                        "type": "string",
                        "description": "Optional raw RSS/Atom XML string to parse directly without network requests"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json"],
                        "default": "markdown",
                        "description": "Output presentation format: 'markdown' (GFM article reading list) or 'json' (structured FeedChannel object)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Optional maximum number of feed items to return"
                    }
                }
            }),
        },
        McpTool {
            name: "inspect_links".to_string(),
            description: "High-speed concurrent link health and status checker. Verifies reachability, HTTP status codes, response latency, and redirection targets for a list of URLs with SSRF protection.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "urls": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of URLs to inspect and verify"
                    },
                    "concurrency": {
                        "type": "integer",
                        "default": 10,
                        "description": "Maximum concurrent network requests (default: 10, max: 30)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "default": 10,
                        "description": "Per-link handshake and request timeout in seconds (default: 10)"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json"],
                        "default": "markdown",
                        "description": "Output format: 'markdown' (summary table) or 'json'"
                    }
                },
                "required": ["urls"]
            }),
        },
        McpTool {
            name: "check_auth".to_string(),
            description: "Verifies whether authenticated session cookies for a platform (e.g. 'bilibili', 'zhihu', 'github') are present and valid, reporting login status, user profile info, or expiration warnings.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "platform": {
                        "type": "string",
                        "enum": ["bilibili", "zhihu", "github", "wechat"],
                        "description": "Target platform to check authentication status ('bilibili', 'zhihu', 'github', or 'wechat')"
                    },
                    "cookie_file": {
                        "type": "string",
                        "default": "./cookies.txt",
                        "description": "Path to cookie file (default: ./cookies.txt)"
                    },
                    "cookie": {
                        "type": "string",
                        "description": "Optional direct cookie header string to check"
                    }
                },
                "required": ["platform"]
            }),
        },
        McpTool {
            name: "summarize_text".to_string(),
            description: "Locally distills salient key takeaway bullet points (TL;DR) from arbitrary text or Markdown using term frequency and positional heuristics without requiring external LLM API calls.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Raw text or Markdown content to extract key takeaways from"
                    },
                    "max_bullets": {
                        "type": "integer",
                        "default": 3,
                        "description": "Maximum number of key takeaway bullet points to extract (default: 3, range: 1-10)"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json"],
                        "default": "markdown",
                        "description": "Output format: 'markdown' (formatted bullet points) or 'json' (array of strings)"
                    }
                },
                "required": ["text"]
            }),
        },
        McpTool {
            name: "split_document".to_string(),
            description: "Splits a monolithic Markdown document into modular sections by heading hierarchy (# or ##) for RAG vector indexing or multi-file export.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "markdown": {
                        "type": "string",
                        "description": "Raw Markdown document string to split by headings"
                    },
                    "max_heading_level": {
                        "type": "integer",
                        "default": 2,
                        "description": "Maximum heading depth level to split on (1 for # only, 2 for # and ##, 3 for #, ##, ###; default: 2)"
                    }
                },
                "required": ["markdown"]
            }),
        }
    ]
}
