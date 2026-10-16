use clap::{Parser, Subcommand, ValueEnum};
use pageclaw_core::model::{ExtractionMode, OutputFormat};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliFormat {
    /// GitHub Flavored Markdown (default)
    Markdown,
    /// Complete structured JSON with metadata, content, and token counts
    Json,
    /// Clean plain text stripped of HTML/Markdown tags
    Text,
    /// Sanitized HTML content
    Html,
    /// Raw unmodified HTML
    Raw,
}

impl From<CliFormat> for OutputFormat {
    fn from(format: CliFormat) -> Self {
        match format {
            CliFormat::Markdown => OutputFormat::Markdown,
            CliFormat::Json => OutputFormat::Json,
            CliFormat::Text => OutputFormat::Text,
            CliFormat::Html => OutputFormat::Html,
            CliFormat::Raw => OutputFormat::Raw,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliMode {
    /// Readability-based article body extraction (default)
    Article,
    /// Sanitized full page body with scripts and styles removed
    Full,
    /// Raw unparsed HTML
    Raw,
}

impl From<CliMode> for ExtractionMode {
    fn from(mode: CliMode) -> Self {
        match mode {
            CliMode::Article => ExtractionMode::Article,
            CliMode::Full => ExtractionMode::Full,
            CliMode::Raw => ExtractionMode::Raw,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum CliQuality {
    #[default]
    #[value(name = "720p")]
    Q720p,
    #[value(name = "480p")]
    Q480p,
    #[value(name = "360p")]
    Q360p,
}

impl From<CliQuality> for pageclaw_core::model::VideoQuality {
    fn from(q: CliQuality) -> Self {
        match q {
            CliQuality::Q720p => pageclaw_core::model::VideoQuality::Q720p,
            CliQuality::Q480p => pageclaw_core::model::VideoQuality::Q480p,
            CliQuality::Q360p => pageclaw_core::model::VideoQuality::Q360p,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum CliOnTooLarge {
    /// Return ResponseTooLarge error when response exceeds max-size (default)
    #[default]
    Error,
    /// Truncate downloaded body at max-size limit and continue processing
    Truncate,
}

impl From<CliOnTooLarge> for pageclaw_core::model::OnTooLarge {
    fn from(v: CliOnTooLarge) -> Self {
        match v {
            CliOnTooLarge::Error => pageclaw_core::model::OnTooLarge::Error,
            CliOnTooLarge::Truncate => pageclaw_core::model::OnTooLarge::Truncate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum CliTokenizer {
    /// OpenAI o200k_base (GPT-4o, Claude 3.5, DeepSeek default standard)
    #[default]
    #[value(name = "o200k")]
    O200k,
    /// OpenAI cl100k_base (GPT-4 standard)
    #[value(name = "cl100k")]
    Cl100k,
    /// Fast heuristic character/word counter
    #[value(name = "heuristic")]
    Heuristic,
}

impl From<CliTokenizer> for pageclaw_core::model::TokenizerKind {
    fn from(t: CliTokenizer) -> Self {
        match t {
            CliTokenizer::O200k => pageclaw_core::model::TokenizerKind::O200k,
            CliTokenizer::Cl100k => pageclaw_core::model::TokenizerKind::Cl100k,
            CliTokenizer::Heuristic => pageclaw_core::model::TokenizerKind::Heuristic,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "pageclaw",
    version = concat!(env!("CARGO_PKG_VERSION"), " (built ", env!("PAGECLAW_BUILD_DATE"), ")"),
    about = "High-performance, token-efficient web content extraction CLI for AI agents",
    long_about = "PageClaw fetches web pages, removes navigation/ads/boilerplate using Readability heuristics, \
and converts the core content into LLM-optimized Markdown or structured JSON. \
Designed for sub-second agent tool calls with strict stdout/stderr stream separation.",
    subcommand_precedence_over_arg = true
)]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Option<SubCommand>,

    /// Target webpage URL or '-' to read HTML from standard input (stdin)
    #[arg(value_name = "TARGET", help_heading = "通用提取选项 (General Options)")]
    pub target: Option<String>,

    /// Output presentation format
    #[arg(short = 'f', long = "format", value_enum, default_value_t = CliFormat::Markdown, help_heading = "通用提取选项 (General Options)")]
    pub format: CliFormat,

    /// Content extraction mode
    #[arg(short = 'm', long = "mode", value_enum, default_value_t = CliMode::Article, help_heading = "通用提取选项 (General Options)")]
    pub mode: CliMode,

    /// Maximum token budget (content will be safely truncated at paragraph/sentence boundaries)
    #[arg(short = 't', long = "max-tokens", value_name = "N", help_heading = "通用提取选项 (General Options)")]
    pub max_tokens: Option<usize>,

    /// Prepend YAML frontmatter with title, URL, date, and metadata (Markdown format only)
    #[arg(long = "frontmatter", help_heading = "通用提取选项 (General Options)")]
    pub frontmatter: bool,

    /// Extract and output only JSON-LD structured data blocks
    #[arg(long = "json-ld", help_heading = "通用提取选项 (General Options)")]
    pub json_ld_only: bool,

    /// CSS selector for targeted DOM extraction (e.g. 'article.content, #readme')
    #[arg(long = "selector", value_name = "CSS_SELECTOR", help_heading = "通用提取选项 (General Options)")]
    pub selector: Option<String>,

    /// Extract and structure all hyperlinks from the document
    #[arg(long = "extract-links", help_heading = "通用提取选项 (General Options)")]
    pub extract_links: bool,

    /// Extract and structure all image assets from the document
    #[arg(long = "extract-images", help_heading = "通用提取选项 (General Options)")]
    pub extract_images: bool,

    /// Extract and structure all tables into 2D arrays and row dictionary objects
    #[arg(long = "extract-tables", help_heading = "通用提取选项 (General Options)")]
    pub extract_tables: bool,

    /// Strip images from Markdown to conserve token budget
    #[arg(long = "no-images", help_heading = "通用提取选项 (General Options)")]
    pub no_images: bool,

    /// Strip link URLs from Markdown, keeping only the anchor text
    #[arg(long = "no-links", help_heading = "通用提取选项 (General Options)")]
    pub no_links: bool,

    /// Do not extract document metadata (OpenGraph, Twitter Cards, description)
    #[arg(long = "no-metadata", help_heading = "通用提取选项 (General Options)")]
    pub no_metadata: bool,

    /// Disable platform-specific fast API adapters and force standard HTML scraping
    #[arg(long = "no-adapter", help_heading = "通用提取选项 (General Options)")]
    pub no_adapter: bool,

    // =========================================================================
    // 哔哩哔哩专属选项 (Bilibili Options)
    // =========================================================================

    /// 提取 B站 视频官方/AI 生成的 CC 字幕轨
    #[arg(
        long = "bili-sub",
        alias = "bili-subtitles",
        alias = "subtitles",
        alias = "sub",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_sub: bool,

    /// 提取 B站 官方 AI 视频总结核心摘要与章节大纲
    #[arg(
        long = "bili-sum",
        alias = "bili-summary",
        alias = "summary",
        alias = "bili-conclusion",
        alias = "conclusion",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_sum: bool,

    /// 提取 B站 视频时间轴高能弹幕 (按出现时间升序排序为 GFM 表格)
    #[arg(
        long = "bili-dm",
        alias = "bili-danmaku",
        alias = "danmaku",
        alias = "dm",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_dm: bool,

    /// 限制提取的时间轴弹幕最大条数
    #[arg(
        long = "bili-dm-limit",
        alias = "bili-danmaku-limit",
        alias = "danmaku-limit",
        value_name = "N",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_dm_limit: Option<usize>,

    /// 提取 B站 视频高赞精选热评 (包含用户名、点赞数、评论内容与发布时间)
    #[arg(
        long = "bili-cm",
        alias = "bili-comments",
        alias = "comments",
        alias = "cm",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_cm: bool,

    /// 限制提取的高赞热评最大条数 [默认: 20] (支持自动跨页获取)
    #[arg(
        long = "bili-cm-limit",
        alias = "bili-comments-limit",
        alias = "comments-limit",
        value_name = "N",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_cm_limit: Option<usize>,

    /// 拉取 B站 首页推荐视频流表格
    #[arg(
        long = "bili-feed",
        alias = "feed",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_feed: bool,

    /// 按关键词搜索 B站 视频
    #[arg(
        long = "bili-search",
        alias = "search",
        value_name = "KEYWORD",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_search: Option<String>,

    /// 搜索结果页码或多 P 视频分 P 编号 (1-indexed)
    #[arg(
        long = "bili-page",
        alias = "page",
        value_name = "PAGE",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_page: Option<u32>,

    /// 搜索排序规则: totalrank (默认), click (播放), pubdate (最新), dm (弹幕), stow (收藏)
    #[arg(
        long = "bili-order",
        alias = "order",
        value_name = "ORDER",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_order: Option<String>,

    /// 仅输出 B站 视频高清封面图片 URL
    #[arg(
        long = "bili-cover",
        alias = "cover",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_cover: bool,

    /// 下载 B站 视频封面图片到本地 (默认: {bvid}_cover.jpg)
    #[arg(
        long = "bili-dl-cover",
        alias = "bili-download-cover",
        alias = "download-cover",
        value_name = "PATH",
        num_args = 0..=1,
        default_missing_value = "",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_dl_cover: Option<String>,

    /// 下载 B站 渐进式 MP4 视频流 (默认: {title}.mp4)
    #[arg(
        long = "bili-dl",
        alias = "bili-dl-video",
        alias = "bili-download",
        alias = "bili-download-video",
        alias = "download-video",
        alias = "download",
        value_name = "PATH",
        num_args = 0..=1,
        default_missing_value = "",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_dl: Option<String>,

    /// B站 视频清晰度选择: 720p (默认), 480p, 360p
    #[arg(
        long = "bili-quality",
        alias = "bili-qn",
        alias = "quality",
        value_enum,
        default_value_t = CliQuality::Q720p,
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_quality: CliQuality,

    // =========================================================================
    // 网络与身份认证 (Network & Auth)
    // =========================================================================

    /// Cookie header string
    #[arg(short = 'c', long = "cookie", value_name = "COOKIE", help_heading = "网络与身份认证 (Network & Auth)")]
    pub cookie: Option<String>,

    /// Path to cookie credential file (supports Netscape and standard Header formats)
    #[arg(long = "cookie-file", value_name = "PATH", help_heading = "网络与身份认证 (Network & Auth)")]
    pub cookie_file: Option<PathBuf>,

    /// Custom User-Agent header
    #[arg(short = 'u', long = "user-agent", value_name = "UA", help_heading = "网络与身份认证 (Network & Auth)")]
    pub user_agent: Option<String>,

    /// Custom HTTP request headers in "Key: Value" format (can be specified multiple times)
    #[arg(short = 'H', long = "header", value_name = "KEY:VALUE", help_heading = "网络与身份认证 (Network & Auth)")]
    pub headers: Vec<String>,

    /// Network request timeout in seconds
    #[arg(long = "timeout", default_value_t = 15, value_name = "SECS", help_heading = "网络与身份认证 (Network & Auth)")]
    pub timeout_secs: u64,

    /// Network connection handshake timeout in seconds
    #[arg(long = "connect-timeout", default_value_t = 10, value_name = "SECS", help_heading = "网络与身份认证 (Network & Auth)")]
    pub connect_timeout: u64,

    /// Number of retry attempts with exponential backoff on 5xx server errors or network drops
    #[arg(long = "retries", default_value_t = 0, value_name = "N", help_heading = "网络与身份认证 (Network & Auth)")]
    pub retries: u32,

    /// Maximum response download size in megabytes
    #[arg(long = "max-size", default_value_t = 10, value_name = "MB", help_heading = "网络与身份认证 (Network & Auth)")]
    pub max_size_mb: usize,

    /// Behavior when response size exceeds max-size: 'error' (default) or 'truncate'
    #[arg(long = "on-too-large", value_enum, default_value_t = CliOnTooLarge::Error, help_heading = "网络与身份认证 (Network & Auth)")]
    pub on_too_large: CliOnTooLarge,

    /// Allow fetching private IP addresses, localhost, and internal network endpoints (bypasses SSRF guard)
    #[arg(long = "allow-private-addresses", help_heading = "网络与身份认证 (Network & Auth)")]
    pub allow_private_addresses: bool,

    /// Do not fail with non-zero exit code on HTTP 4xx or 5xx error responses (returns body with exit 0)
    #[arg(long = "allow-http-errors", alias = "no-fail", help_heading = "网络与身份认证 (Network & Auth)")]
    pub allow_http_errors: bool,

    /// Base URL to resolve relative links when reading HTML from stdin
    #[arg(long = "base-url", value_name = "URL", help_heading = "网络与身份认证 (Network & Auth)")]
    pub base_url: Option<String>,

    // =========================================================================
    // 缓存与运行诊断 (Cache & Diagnostics)
    // =========================================================================

    /// Run as native Model Context Protocol (MCP) server over stdio
    #[arg(long = "mcp", help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub mcp: bool,

    /// Enable local response caching
    #[arg(long = "cache", help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub cache: bool,

    /// Explicitly disable local response caching
    #[arg(long = "no-cache", conflicts_with = "cache", help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub no_cache: bool,

    /// Cache TTL in seconds (default: 900 = 15 minutes)
    #[arg(long = "cache-ttl", default_value_t = 900, value_name = "SECS", help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub cache_ttl: u64,

    /// Custom directory for persistent file cache
    #[arg(long = "cache-dir", value_name = "DIR", help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub cache_dir: Option<PathBuf>,

    /// Tokenizer model encoding scheme for token counting and truncation
    #[arg(long = "tokenizer", value_enum, default_value_t = CliTokenizer::O200k, help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub tokenizer: CliTokenizer,

    /// Write output payload to file instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE", help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub output_file: Option<std::path::PathBuf>,

    /// Verbose diagnostic output on stderr (-v for debug, -vv for trace)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub verbose: u8,

    /// Silence all diagnostic logs and progress messages on stderr
    #[arg(short = 'q', long = "quiet", help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub quiet: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum SubCommand {
    /// Run as native Model Context Protocol (MCP) server over stdio
    Mcp(McpArgs),

    /// Launch browser login to extract Cookie credentials and save to file
    Login(LoginArgs),

    /// Perform a zero-API-key web search (DuckDuckGo HTML) and output structured results
    Search(SearchSubArgs),

    /// Concurrently fetch and extract a batch of URLs from a file or stdin
    Batch(BatchArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct SearchSubArgs {
    /// Search query keyword or question
    #[arg(value_name = "QUERY")]
    pub query: String,

    /// Maximum number of search results to return [default: 5]
    #[arg(short = 'l', long = "limit", default_value_t = 5, value_name = "N")]
    pub limit: usize,

    /// Output presentation format [default: markdown]
    #[arg(short = 'f', long = "format", value_enum, default_value_t = CliFormat::Markdown)]
    pub format: CliFormat,

    /// Write search results to file instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct BatchArgs {
    /// Path to file containing URLs (one per line) or '-' to read from standard input
    #[arg(value_name = "FILE")]
    pub file: String,

    /// Concurrency limit for simultaneous network requests [default: 5]
    #[arg(short = 'c', long = "concurrency", default_value_t = 5, value_name = "N")]
    pub concurrency: usize,

    /// Output presentation format [default: markdown]
    #[arg(short = 'f', long = "format", value_enum, default_value_t = CliFormat::Markdown)]
    pub format: CliFormat,

    /// Maximum token budget per article
    #[arg(short = 't', long = "max-tokens", value_name = "N")]
    pub max_tokens: Option<usize>,

    /// CSS selector for targeted DOM extraction
    #[arg(long = "selector", value_name = "CSS_SELECTOR")]
    pub selector: Option<String>,

    /// Extract and structure hyperlinks
    #[arg(long = "extract-links")]
    pub extract_links: bool,

    /// Extract and structure tables
    #[arg(long = "extract-tables")]
    pub extract_tables: bool,

    /// Strip images from Markdown output
    #[arg(long = "no-images")]
    pub no_images: bool,

    /// Strip links from Markdown output
    #[arg(long = "no-links")]
    pub no_links: bool,

    /// Write batch results to file instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct LoginArgs {
    /// Target platform to log in (e.g. 'bilibili')
    #[arg(value_name = "PLATFORM")]
    pub platform: String,

    /// Destination file path to save extracted cookies [default: ./cookies.txt]
    #[arg(short = 'o', long = "output", default_value = "./cookies.txt", value_name = "FILE")]
    pub output: PathBuf,

    /// Do not launch default web browser automatically (print terminal QR code and URL only)
    #[arg(long = "no-browser")]
    pub no_browser: bool,

    /// Maximum waiting timeout in seconds [default: 180]
    #[arg(long = "timeout", default_value_t = 180, value_name = "SECS")]
    pub timeout_secs: u64,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct McpArgs {
    /// Disable response caching in MCP server
    #[arg(long = "no-cache")]
    pub no_cache: bool,

    /// Cache TTL in seconds (default: 900 = 15 minutes)
    #[arg(long = "cache-ttl", default_value_t = 900, value_name = "SECS")]
    pub cache_ttl: u64,

    /// Custom directory for persistent file cache
    #[arg(long = "cache-dir", value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,
}
