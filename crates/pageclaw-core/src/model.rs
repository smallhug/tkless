use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Mode of content extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractionMode {
    /// Readability-based article extraction (extracts main content, discards nav/ads/footer).
    #[default]
    Article,
    /// Sanitized full page body with scripts and styles removed.
    Full,
    /// Raw original HTML without modification.
    Raw,
}

/// Output presentation format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// GitHub Flavored Markdown format.
    #[default]
    Markdown,
    /// Structured JSON payload with metadata and content.
    Json,
    /// Clean plain text stripped of HTML/Markdown tags.
    Text,
    /// Sanitized HTML content.
    Html,
    /// Raw HTML content.
    Raw,
}

/// Behavior when response body exceeds max_bytes limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnTooLarge {
    /// Abort with ResponseTooLarge error (default).
    #[default]
    Error,
    /// Truncate body at limit, continue extraction, and mark document as truncated.
    Truncate,
}

/// Search engine provider for web searching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngineKind {
    /// Microsoft Bing search (default, fast connectivity and great Chinese coverage)
    #[default]
    Bing,
    /// Google search (global technical community and documentation index)
    Google,
    /// Automatic failover mode (prefers Bing, falls back to Google on error)
    Auto,
}

impl SearchEngineKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchEngineKind::Bing => "bing",
            SearchEngineKind::Google => "google",
            SearchEngineKind::Auto => "auto",
        }
    }
}

/// Tokenizer model encoding scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenizerKind {
    /// OpenAI o200k_base (GPT-4o, Claude 3.5, DeepSeek benchmark standard).
    #[default]
    O200k,
    /// OpenAI cl100k_base (GPT-4 benchmark standard).
    Cl100k,
    /// Fast heuristic character/word counter.
    Heuristic,
}

impl TokenizerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TokenizerKind::O200k => "o200k",
            TokenizerKind::Cl100k => "cl100k",
            TokenizerKind::Heuristic => "heuristic",
        }
    }
}

/// Options controlling the HTTP fetching behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchOptions {
    /// Custom User-Agent header string.
    pub user_agent: Option<String>,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Connection handshake timeout in seconds.
    pub connect_timeout_secs: u64,
    /// Number of exponential backoff retry attempts on 5xx or network errors.
    pub retries: u32,
    /// Maximum allowed download size in bytes.
    pub max_bytes: usize,
    /// Behavior when response size exceeds max_bytes.
    pub on_too_large: OnTooLarge,
    /// Additional custom HTTP headers.
    pub headers: Vec<(String, String)>,
    /// Cookie header string.
    pub cookie: Option<String>,
    /// Path to cookie credential file (supports multi-platform prefix format, e.g. bilibili: ..., zhihu: ...).
    pub cookie_file: Option<String>,
    /// Authorized initial target host for unprefixed CLI cookies to prevent cross-host leaks.
    pub initial_target_host: Option<String>,
    /// Allow fetching private IP addresses, localhost, and internal network endpoints.
    pub allow_private_addresses: bool,
    /// Do not fail with an error on HTTP 4xx or 5xx responses.
    pub allow_http_errors: bool,
    /// Optional HTTP/HTTPS proxy URL (e.g. 'http://127.0.0.1:7890').
    pub proxy: Option<String>,
    /// Whether to emulate a mobile device User-Agent to trigger lightweight mobile layouts and strip desktop clutter.
    pub mobile: bool,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            user_agent: Some(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 PageClaw/0.1"
                    .to_string(),
            ),
            timeout_secs: 15,
            connect_timeout_secs: 10,
            retries: 0,
            max_bytes: 10 * 1024 * 1024, // 10MB
            on_too_large: OnTooLarge::default(),
            headers: Vec::new(),
            cookie: None,
            cookie_file: None,
            initial_target_host: None,
            allow_private_addresses: false,
            allow_http_errors: false,
            proxy: None,
            mobile: false,
        }
    }
}

/// Options controlling content parsing, cleaning, and formatting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionOptions {
    /// Content extraction mode.
    pub mode: ExtractionMode,
    /// Target output format.
    pub format: OutputFormat,
    /// Maximum number of tokens before smart truncation.
    pub max_tokens: Option<usize>,
    /// Tokenizer encoding scheme used for token counting and truncation.
    pub tokenizer: TokenizerKind,
    /// Whether to extract OpenGraph, Twitter, and meta tags.
    pub include_metadata: bool,
    /// Whether to extract JSON-LD structured blocks.
    pub include_json_ld: bool,
    /// Whether to strip images in Markdown output.
    pub strip_images: bool,
    /// Whether to strip link URLs in Markdown output (keeping anchor text).
    pub strip_links: bool,
    /// Whether to prepend YAML frontmatter in Markdown output.
    pub frontmatter: bool,
    /// Disable platform adapters (force standard HTML scraping).
    pub no_adapter: bool,
    /// Request platform recommendations or feed stream.
    pub feed: bool,
    /// CSS selector for targeted DOM extraction (e.g. 'article.content, #readme').
    pub selector: Option<String>,
    /// Whether to extract structured links list.
    pub extract_links: bool,
    /// Whether to extract structured images list.
    pub extract_images: bool,
    /// Whether to extract structured tables.
    pub extract_tables: bool,
    /// Whether to fetch video subtitles.
    pub subtitles: bool,
    /// Whether to fetch video timestamped danmaku.
    pub danmaku: bool,
    /// Maximum number of danmaku entries to extract.
    pub danmaku_limit: Option<usize>,
    /// Whether to fetch video top comments.
    pub comments: bool,
    /// Maximum number of top comments to extract.
    pub comments_limit: Option<usize>,
    /// Whether to fetch official AI video conclusion and outline summary.
    pub conclusion: bool,
    /// Optional Chrome DevTools Protocol endpoint (e.g. 'http://127.0.0.1:9222' or 'ws://...') for JS-rendered extraction.
    pub cdp_endpoint: Option<String>,
    /// Optional wait time in milliseconds for CDP DOM hydration.
    pub cdp_wait_ms: Option<u64>,
    /// Optional schema definition (JSON string or file path) for structured entity extraction.
    pub schema: Option<String>,
    /// Whether to segment the output Markdown into semantic chunks for RAG and vector database ingestion.
    pub chunk: bool,
    /// Target maximum token size per chunk (default: 500).
    pub chunk_size: Option<usize>,
    /// Token overlap between consecutive chunks (default: 50).
    pub chunk_overlap: Option<usize>,
    /// Whether to download referenced remote images locally and rewrite Markdown image links.
    pub download_images: bool,
    /// Destination directory for downloaded offline image assets (default: "./assets").
    pub assets_dir: Option<String>,
    /// Whether to output only the extracted tables, skipping the main body content.
    pub tables_only: bool,
    /// Presentation format for table export: markdown (default), csv, tsv, json.
    pub table_format: TableExportFormat,
    /// Whether to measure and compute detailed telemetry and token economy metrics.
    pub stats: bool,
}

impl Default for ExtractionOptions {
    fn default() -> Self {
        Self {
            mode: ExtractionMode::default(),
            format: OutputFormat::default(),
            max_tokens: None,
            tokenizer: TokenizerKind::default(),
            include_metadata: true,
            include_json_ld: true,
            strip_images: false,
            strip_links: false,
            frontmatter: false,
            no_adapter: false,
            feed: false,
            selector: None,
            extract_links: false,
            extract_images: false,
            extract_tables: false,
            subtitles: false,
            danmaku: false,
            danmaku_limit: None,
            comments: false,
            comments_limit: None,
            conclusion: false,
            cdp_endpoint: None,
            cdp_wait_ms: None,
            schema: None,
            chunk: false,
            chunk_size: None,
            chunk_overlap: None,
            download_images: false,
            assets_dir: None,
            tables_only: false,
            table_format: TableExportFormat::default(),
            stats: false,
        }
    }
}

/// Extracted metadata from document head and meta tags.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageMetadata {
    pub description: Option<String>,
    pub keywords: Vec<String>,
    pub author: Option<String>,
    pub canonical_url: Option<String>,
    pub published_time: Option<String>,
    pub cover_image: Option<String>,
    pub favicon: Option<String>,
    pub open_graph: HashMap<String, String>,
    pub twitter_card: HashMap<String, String>,
}

/// Desired video quality for stream downloading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoQuality {
    /// 720P High Definition (Bilibili qn=64, highest unauthenticated quality)
    #[default]
    Q720p,
    /// 480P Clear (Bilibili qn=32)
    Q480p,
    /// 360P Smooth (Bilibili qn=16)
    Q360p,
}

impl VideoQuality {
    pub fn qn(&self) -> u32 {
        match self {
            VideoQuality::Q720p => 64,
            VideoQuality::Q480p => 32,
            VideoQuality::Q360p => 16,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            VideoQuality::Q720p => "720p",
            VideoQuality::Q480p => "480p",
            VideoQuality::Q360p => "360p",
        }
    }
}

/// Individual line of video subtitle with timeline range.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubtitleLine {
    /// Start timestamp in seconds.
    pub from: f64,
    /// End timestamp in seconds.
    pub to: f64,
    /// Subtitle content text.
    pub content: String,
}

/// Timestamped video danmaku (bullet comment) entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DanmakuEntry {
    /// Playback appearance timestamp in seconds.
    pub time: f64,
    /// Danmaku text content.
    pub text: String,
}

/// Video top comment entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommentEntry {
    /// Comment author username.
    pub user: String,
    /// Comment upvote/like count.
    pub like: u64,
    /// Comment text message.
    pub content: String,
    /// Formatted publication date/time string.
    pub time: String,
}

/// Extracted hyperlink asset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ExtractedLink {
    /// Link anchor text.
    pub text: String,
    /// Resolved absolute URL.
    pub url: String,
}

/// Extracted image asset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ExtractedImage {
    /// Image alternative text.
    pub alt: String,
    /// Resolved absolute image source URL.
    pub src: String,
}

/// Export format for extracted structured tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableExportFormat {
    /// GitHub Flavored Markdown table format (default).
    #[default]
    Markdown,
    /// Comma-Separated Values (RFC 4180).
    Csv,
    /// Tab-Separated Values.
    Tsv,
    /// Structured JSON records.
    Json,
}

impl TableExportFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            TableExportFormat::Markdown => "markdown",
            TableExportFormat::Csv => "csv",
            TableExportFormat::Tsv => "tsv",
            TableExportFormat::Json => "json",
        }
    }
}

/// Structured representation of an HTML table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtractedTable {
    /// Table column headers.
    pub headers: Vec<String>,
    /// Table raw data rows (2D array).
    pub rows: Vec<Vec<String>>,
    /// Table row records as key-value dictionaries.
    pub records: Vec<HashMap<String, String>>,
}

impl ExtractedTable {
    /// Formats the table into RFC 4180 compliant CSV text.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        if !self.headers.is_empty() {
            out.push_str(
                &self
                    .headers
                    .iter()
                    .map(|h| csv_escape(h))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push('\n');
        }
        for row in &self.rows {
            out.push_str(
                &row.iter()
                    .map(|cell| csv_escape(cell))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push('\n');
        }
        out
    }

    /// Formats the table into TSV text.
    pub fn to_tsv(&self) -> String {
        let mut out = String::new();
        if !self.headers.is_empty() {
            out.push_str(
                &self
                    .headers
                    .iter()
                    .map(|h| h.replace('\t', " ").replace(['\r', '\n'], " "))
                    .collect::<Vec<_>>()
                    .join("\t"),
            );
            out.push('\n');
        }
        for row in &self.rows {
            out.push_str(
                &row.iter()
                    .map(|cell| cell.replace('\t', " ").replace(['\r', '\n'], " "))
                    .collect::<Vec<_>>()
                    .join("\t"),
            );
            out.push('\n');
        }
        out
    }

    /// Formats the table into a clean GitHub Flavored Markdown table.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let col_count = self
            .headers
            .len()
            .max(self.rows.iter().map(|r| r.len()).max().unwrap_or(0));
        if col_count == 0 {
            return out;
        }

        let headers: Vec<String> = if self.headers.is_empty() {
            (1..=col_count).map(|i| format!("Col {i}")).collect()
        } else {
            self.headers.iter().map(|h| sanitize_md_cell(h)).collect()
        };

        out.push_str("| ");
        out.push_str(&headers.join(" | "));
        out.push_str(" |\n| ");
        out.push_str(&vec!["---"; col_count].join(" | "));
        out.push_str(" |\n");

        for row in &self.rows {
            let mut cells = Vec::new();
            for i in 0..col_count {
                let text = row.get(i).map(|s| sanitize_md_cell(s)).unwrap_or_default();
                cells.push(text);
            }
            out.push_str("| ");
            out.push_str(&cells.join(" | "));
            out.push_str(" |\n");
        }
        out
    }

    /// Formats multiple extracted tables into the requested export format.
    pub fn format_tables(tables: &[ExtractedTable], format: TableExportFormat) -> String {
        if tables.is_empty() {
            return match format {
                TableExportFormat::Json => "[]".to_string(),
                _ => "*(页面未检测到有效数据表格)*".to_string(),
            };
        }

        match format {
            TableExportFormat::Json => serde_json::to_string_pretty(tables).unwrap_or_default(),
            TableExportFormat::Csv => tables
                .iter()
                .enumerate()
                .map(|(idx, t)| {
                    if tables.len() > 1 {
                        format!("# Table {}\n{}", idx + 1, t.to_csv())
                    } else {
                        t.to_csv()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            TableExportFormat::Tsv => tables
                .iter()
                .enumerate()
                .map(|(idx, t)| {
                    if tables.len() > 1 {
                        format!("# Table {}\n{}", idx + 1, t.to_tsv())
                    } else {
                        t.to_tsv()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            TableExportFormat::Markdown => tables
                .iter()
                .enumerate()
                .map(|(idx, t)| {
                    if tables.len() > 1 {
                        format!(
                            "### 表格 {} (Table {})\n\n{}",
                            idx + 1,
                            idx + 1,
                            t.to_markdown()
                        )
                    } else {
                        t.to_markdown()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }
}

fn csv_escape(s: &str) -> String {
    let clean = s.replace('\r', "").replace('\n', " ");
    if clean.contains(',') || clean.contains('"') {
        format!("\"{}\"", clean.replace('"', "\"\""))
    } else {
        clean
    }
}

fn sanitize_md_cell(s: &str) -> String {
    s.replace('|', "\\|")
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string()
}

/// Official or AI-generated video conclusion and outline summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoConclusion {
    /// Comprehensive summary overview.
    pub summary: String,
    /// Timestamped chapter and key-point outline.
    pub outline: Vec<ConclusionChapter>,
}

/// Chapter entry in video conclusion outline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConclusionChapter {
    pub title: String,
    pub timestamp: u64,
    pub parts: Vec<ConclusionKeypoint>,
}

/// Sub-point entry within a conclusion chapter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConclusionKeypoint {
    pub timestamp: u64,
    pub content: String,
}

/// A semantic chunk of a cleaned document for RAG and vector database ingestion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentChunk {
    /// Zero-based chunk sequence index
    pub index: usize,
    /// Heading hierarchy path (e.g. ["Getting Started", "Installation", "Cargo"])
    pub heading_path: Vec<String>,
    /// Primary section heading title
    pub title: Option<String>,
    /// Chunk text content in clean Markdown
    pub content: String,
    /// Estimated token count of this chunk
    pub token_count: usize,
}

/// Telemetry performance and token economy metrics for a document extraction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtractionMetrics {
    /// Time spent fetching remote HTML over network in milliseconds
    pub fetch_time_ms: u64,
    /// Time spent cleaning, converting to markdown, and extracting metadata in milliseconds
    pub clean_time_ms: u64,
    /// Overall pipeline duration in milliseconds
    pub total_time_ms: u64,
    /// Raw HTML response payload size in bytes
    pub raw_bytes: usize,
    /// Final cleaned document content size in bytes
    pub cleaned_bytes: usize,
    /// Estimated token count of the raw unmodified HTML
    pub raw_token_estimate: usize,
    /// Final token count of the cleaned extracted content
    pub final_token_count: usize,
    /// Percentage of tokens saved compared to raw HTML (0.0 to 100.0)
    pub token_savings_percent: f64,
    /// Percentage of byte noise removed (0.0 to 100.0)
    pub noise_reduction_percent: f64,
}

/// The complete structured result of fetching and extracting a webpage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageDocument {
    /// Requested URL.
    pub url: String,
    /// Final resolved URL after HTTP redirects.
    pub final_url: String,
    /// HTTP response status code.
    pub status: u16,
    /// Extracted page or article title.
    pub title: Option<String>,
    /// Extracted author/byline.
    pub byline: Option<String>,
    /// Primary extracted content in the requested format.
    pub content: String,
    /// Extracted page metadata.
    pub metadata: PageMetadata,
    /// Extracted JSON-LD structured objects.
    pub json_ld: Vec<serde_json::Value>,
    /// Estimated token count of the content.
    pub token_count: usize,
    /// Tokenizer model name used for token counting.
    pub tokenizer: String,
    /// Whether content was truncated due to max_tokens limit or response size limit.
    pub truncated: bool,
    /// Extracted video subtitles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitles: Option<Vec<SubtitleLine>>,
    /// Extracted video timestamped danmaku entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub danmaku: Option<Vec<DanmakuEntry>>,
    /// Extracted video top comments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<CommentEntry>>,
    /// Official AI video conclusion and chapter outline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<VideoConclusion>,
    /// Extracted page hyperlinks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<ExtractedLink>>,
    /// Extracted page image assets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ExtractedImage>>,
    /// Extracted structured tables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tables: Option<Vec<ExtractedTable>>,
    /// Extracted structured entity according to schema definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<serde_json::Value>,
    /// Extracted semantic chunks for RAG / vector search ingestion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<DocumentChunk>>,
    /// 64-bit hexadecimal SimHash content fingerprint for near-duplicate detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// Telemetry performance and token economy metrics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<ExtractionMetrics>,
}
