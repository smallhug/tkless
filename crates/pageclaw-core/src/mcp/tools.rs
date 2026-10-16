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
                        "description": "Path to cookie file (supports Netscape and standard header formats)"
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
            description: "Performs a zero-API-key web search using DuckDuckGo HTML endpoint and returns structured results with titles, direct URLs, and text snippets.".to_string(),
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
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "json"],
                        "default": "markdown",
                        "description": "Output presentation format: 'markdown' (GFM numbered list) or 'json'"
                    }
                },
                "required": ["query"]
            }),
        }
    ]
}
