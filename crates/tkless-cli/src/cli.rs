use clap::builder::TypedValueParser;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use tkless_core::model::{ExtractionMode, OutputFormat};

#[cfg(test)]
mod workflow_contract_tests {
    use super::*;

    #[test]
    fn sync_budget_matches_core_limits() {
        for value in [0, tkless_core::limits::SYNC_MAX_STATE_MB + 1, u64::MAX] {
            assert!(CliArgs::try_parse_from([
                "tkless",
                "sync",
                "--config",
                "config.json",
                "--state",
                "state",
                "--max-state-mb",
                &value.to_string(),
            ])
            .is_err());
            assert!(tkless_core::limits::sync_state_bytes(value).is_err());
        }
        for value in [1, tkless_core::limits::SYNC_MAX_STATE_MB] {
            assert!(CliArgs::try_parse_from([
                "tkless",
                "sync",
                "--config",
                "config.json",
                "--state",
                "state",
                "--max-state-mb",
                &value.to_string(),
            ])
            .is_ok());
            assert!(tkless_core::limits::sync_state_bytes(value).is_ok());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum CliLogFormat {
    #[default]
    Plain,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum CliFormat {
    /// GitHub Flavored Markdown (default)
    #[default]
    Markdown,
    /// Complete structured JSON with metadata, content, and token counts
    Json,
    /// Clean plain text stripped of HTML/Markdown tags
    Text,
    /// Sanitized HTML content
    Html,
    /// Raw unmodified HTML
    Raw,
    /// Line-delimited JSON (JSONL / ndjson) for streaming RAG vector database ingestion
    Jsonl,
}

impl From<CliFormat> for OutputFormat {
    fn from(format: CliFormat) -> Self {
        match format {
            CliFormat::Markdown => OutputFormat::Markdown,
            CliFormat::Json => OutputFormat::Json,
            CliFormat::Text => OutputFormat::Text,
            CliFormat::Html => OutputFormat::Html,
            CliFormat::Raw => OutputFormat::Raw,
            CliFormat::Jsonl => OutputFormat::Json,
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

impl From<CliQuality> for tkless_core::model::VideoQuality {
    fn from(q: CliQuality) -> Self {
        match q {
            CliQuality::Q720p => tkless_core::model::VideoQuality::Q720p,
            CliQuality::Q480p => tkless_core::model::VideoQuality::Q480p,
            CliQuality::Q360p => tkless_core::model::VideoQuality::Q360p,
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

impl From<CliOnTooLarge> for tkless_core::model::OnTooLarge {
    fn from(v: CliOnTooLarge) -> Self {
        match v {
            CliOnTooLarge::Error => tkless_core::model::OnTooLarge::Error,
            CliOnTooLarge::Truncate => tkless_core::model::OnTooLarge::Truncate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum CliTokenizer {
    /// OpenAI o200k_base (GPT-4o, Claude 3.5, DeepSeek standard)
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

impl From<CliTokenizer> for tkless_core::model::TokenizerKind {
    fn from(t: CliTokenizer) -> Self {
        match t {
            CliTokenizer::O200k => tkless_core::model::TokenizerKind::O200k,
            CliTokenizer::Cl100k => tkless_core::model::TokenizerKind::Cl100k,
            CliTokenizer::Heuristic => tkless_core::model::TokenizerKind::Heuristic,
        }
    }
}

/// Platform restriction for Bing search. Explicit site: filters in the query take priority.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliSearchScope {
    All,
    Wechat,
    Zhihu,
    Github,
    Juejin,
    Csdn,
    Medium,
    Substack,
    Arxiv,
    Huggingface,
    Wikipedia,
    Reddit,
    Stackoverflow,
    V2ex,
    Baike,
}
impl CliSearchScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Wechat => "wechat",
            Self::Zhihu => "zhihu",
            Self::Github => "github",
            Self::Juejin => "juejin",
            Self::Csdn => "csdn",
            Self::Medium => "medium",
            Self::Substack => "substack",
            Self::Arxiv => "arxiv",
            Self::Huggingface => "huggingface",
            Self::Wikipedia => "wikipedia",
            Self::Reddit => "reddit",
            Self::Stackoverflow => "stackoverflow",
            Self::V2ex => "v2ex",
            Self::Baike => "baike",
        }
    }
}

fn positive_search_count(raw: &str) -> Result<usize, String> {
    raw.parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "value must be a positive integer".into())
}

#[cfg(test)]
mod skills_argument_tests {
    use super::*;

    #[test]
    fn skills_catalog_defaults_and_bounds_are_explicit() {
        let args = CliArgs::try_parse_from(["tkless", "https://skills.sh/"]).unwrap();
        assert_eq!(args.skills_pages, 3);
        assert_eq!(args.skills_limit, 1000);
        for (flag, value) in [
            ("--skills-pages", "0"),
            ("--skills-pages", "51"),
            ("--skills-limit", "0"),
            ("--skills-limit", "10001"),
        ] {
            assert!(
                CliArgs::try_parse_from(["tkless", "https://skills.sh/", flag, value]).is_err()
            );
        }
        let args = CliArgs::try_parse_from([
            "tkless",
            "https://skills.sh/trending",
            "--skills-pages",
            "5",
            "--skills-limit",
            "800",
        ])
        .unwrap();
        assert_eq!(args.skills_pages, 5);
        assert_eq!(args.skills_limit, 800);
    }
}

fn search_output_formats(jsonl: bool) -> clap::builder::ValueParser {
    let formats = if jsonl {
        vec!["markdown", "json", "jsonl"]
    } else {
        vec!["markdown", "json"]
    };
    clap::builder::PossibleValuesParser::new(formats)
        .map(|format| match format.as_str() {
            "json" => CliFormat::Json,
            "jsonl" => CliFormat::Jsonl,
            _ => CliFormat::Markdown,
        })
        .into()
}

#[cfg(test)]
mod search_argument_tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn defaults_and_platform_scope_are_unambiguous() {
        let args =
            CliArgs::try_parse_from(["tkless", "search", "Rust", "--scope", "github"]).unwrap();
        let Some(SubCommand::Search(search)) = args.command else {
            panic!("Expected search");
        };
        assert_eq!(search.limit, 10);
        assert_eq!(search.scope.unwrap().as_str(), "github");
        assert!(
            CliArgs::try_parse_from(["tkless", "search", "Rust", "--scope", "unknown"]).is_err()
        );
        assert!(CliArgs::try_parse_from(["tkless", "search", "Rust", "--limit", "0"]).is_err());
        assert!(CliArgs::try_parse_from(["tkless", "search", "Rust", "--dedup"]).is_err());
        assert!(CliArgs::try_parse_from([
            "tkless",
            "search",
            "Rust",
            "--max-tokens-per-page",
            "100"
        ])
        .is_err());
        assert!(CliArgs::try_parse_from([
            "tkless",
            "search",
            "Rust",
            "--fetch-content",
            "--dedup"
        ])
        .is_ok());
        assert!(CliArgs::try_parse_from(["tkless", "search", "Rust", "-f", "text"]).is_err());
        assert!(CliArgs::try_parse_from(["tkless", "research", "Rust", "-f", "jsonl"]).is_err());
    }
    #[test]
    fn removed_engine_options_fail_before_network_access() {
        for command in ["search", "research"] {
            assert!(
                CliArgs::try_parse_from(["tkless", command, "Rust", "--engine", "bing"]).is_err()
            );
        }
        for flag in [
            "--yandex-region",
            "--searxng-url",
            "--brave-api-key",
            "--wechat",
        ] {
            assert!(CliArgs::try_parse_from(["tkless", "search", "Rust", flag, "1"]).is_err());
        }
    }

    #[test]
    fn strict_schema_requires_a_schema_and_search_diagnostics_are_opt_in() {
        assert!(CliArgs::try_parse_from(["tkless", "page.html", "--schema-strict"]).is_err());
        let args = CliArgs::try_parse_from([
            "tkless",
            "page.html",
            "--schema",
            "schema.json",
            "--schema-strict",
        ])
        .unwrap();
        assert!(args.schema_strict);
        let args = CliArgs::try_parse_from([
            "tkless",
            "batch",
            "urls.txt",
            "--schema",
            "schema.json",
            "--schema-strict",
        ])
        .unwrap();
        assert!(matches!(
            args.command,
            Some(SubCommand::Batch(BatchArgs {
                schema_strict: true,
                ..
            }))
        ));
        let args = CliArgs::try_parse_from(["tkless", "search", "Rust", "--diagnostics"]).unwrap();
        assert!(matches!(
            args.command,
            Some(SubCommand::Search(SearchSubArgs {
                diagnostics: true,
                ..
            }))
        ));
    }

    #[test]
    fn batch_resume_requirements_and_canonical_names_are_enforced() {
        for args in [
            vec!["tkless", "batch", "urls.txt", "--resume"],
            vec!["tkless", "batch", "urls.txt", "--retry-failed"],
            vec!["tkless", "batch", "urls.txt", "--concurrency", "0"],
            vec![
                "tkless",
                "batch",
                "urls.txt",
                "--per-host-concurrency",
                "65",
            ],
            vec!["tkless", "update", "--check"],
            vec!["tkless", "mcp", "--dynamic-tools"],
            vec!["tkless", "page.html", "--exclude-selector", ".ad"],
            vec!["tkless", "page.html", "--bili-sub"],
        ] {
            assert!(CliArgs::try_parse_from(args.clone()).is_err(), "{args:?}");
        }
        assert!(CliArgs::try_parse_from([
            "tkless",
            "batch",
            "urls.txt",
            "--checkpoint",
            "job.jsonl",
            "--resume",
            "--retry-failed",
            "--order",
            "completion"
        ])
        .is_ok());
        let args = CliArgs::try_parse_from([
            "tkless",
            "BV1xx411c7mD",
            "--subtitles",
            "--comments",
            "--download-video",
            "video.mp4",
        ])
        .unwrap();
        assert!(args.bili_sub && args.bili_cm && args.bili_dl.is_some());
    }
}

/// Table export format choices.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CliTableFormat {
    /// GitHub Flavored Markdown table format (default)
    #[default]
    Markdown,
    /// Comma-Separated Values (RFC 4180)
    Csv,
    /// Tab-Separated Values
    Tsv,
    /// Structured JSON records
    Json,
    /// ANSI/SQLite-compatible SQL DDL and INSERT statements
    Sql,
}

impl From<CliTableFormat> for tkless_core::model::TableExportFormat {
    fn from(f: CliTableFormat) -> Self {
        match f {
            CliTableFormat::Markdown => tkless_core::model::TableExportFormat::Markdown,
            CliTableFormat::Csv => tkless_core::model::TableExportFormat::Csv,
            CliTableFormat::Tsv => tkless_core::model::TableExportFormat::Tsv,
            CliTableFormat::Json => tkless_core::model::TableExportFormat::Json,
            CliTableFormat::Sql => tkless_core::model::TableExportFormat::Sql,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "tkless",
    version = concat!(env!("CARGO_PKG_VERSION"), " (built ", env!("TKLESS_BUILD_DATE"), ")"),
    about = "TkLess: Spend less tokens, perceive more web.",
    long_about = "TkLess — Spend less tokens, perceive more web.\n\
专为大语言模型与 AI Agent 量身定制的高性能网页抓取、降噪清洗与搜索提取引擎。\n\
自动剥离导航、脚本与广告噪音，将正文转化为 LLM 友好的 Markdown 或结构化 JSON，支持段落断句平滑截断与 Token 预算控制。\n\n\
【已深度原生适配的 22 主流平台 (Built-in Adapters)】\n\
• 视频与图文社区: B站 (字幕/AI大纲/弹幕/评论/封面)、小红书 (无水印图集/标签)\n\
• 技术与开源生态: GitHub (README/Issue/Trending热榜)、Skills.sh (技能正文/仓库列表/榜单)、Hugging Face (模型卡/论文)、Stack Overflow (采纳答案/得分)、V2EX (主楼/附言/楼层)、掘金/CSDN (外链解包/代码提纯)\n\
• 学术与知识百科: 维基百科/百度百科 (信息盒/表格/去引用)、ArXiv (论文/PDF/Atom订阅)\n\
• 深度专栏与讨论: 微信公众号 (懒加载图片/防封)、Medium/Substack (去付费遮罩)、Reddit (帖子/评论树)\n\n\
运行 'tkless adapters' 可查看各平台完整能力矩阵与匹配域名清单。",
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
    #[arg(
        short = 't',
        long = "max-tokens",
        value_name = "N",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub max_tokens: Option<usize>,

    /// Prepend YAML frontmatter with title, URL, date, and metadata (Markdown format only)
    #[arg(long = "frontmatter", help_heading = "通用提取选项 (General Options)")]
    pub frontmatter: bool,

    /// Extract and output only JSON-LD structured data blocks
    #[arg(long = "json-ld", help_heading = "通用提取选项 (General Options)")]
    pub json_ld_only: bool,

    /// CSS selector for targeted DOM extraction (e.g. 'article.content, #readme')
    #[arg(
        long = "selector",
        value_name = "CSS_SELECTOR",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub selector: Option<String>,

    /// CSS 排除选择器: 剔除指定匹配的 DOM 元素容器 (例如 '.ads, .footer, #comments, .author-bio')
    #[arg(
        short = 'R',
        long = "remove-selector",
        value_name = "CSS_SELECTOR",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub remove_selector: Option<String>,

    /// Extract and structure all hyperlinks from the document
    #[arg(
        long = "extract-links",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub extract_links: bool,

    /// Extract and structure all image assets from the document
    #[arg(
        long = "extract-images",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub extract_images: bool,

    /// Extract and structure all tables into 2D arrays and row dictionary objects
    #[arg(
        long = "extract-tables",
        help_heading = "通用提取选项 (General Options)"
    )]
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

    /// Skills.sh 榜单自动翻页上限（包含首页）
    #[arg(long = "skills-pages", default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=50), help_heading = "Skills.sh")]
    pub skills_pages: u32,

    /// Skills.sh 去重结果上限；站内搜索的上游上限为 200
    #[arg(long = "skills-limit", default_value_t = 1000, value_parser = clap::value_parser!(u32).range(1..=10000), help_heading = "Skills.sh")]
    pub skills_limit: u32,

    /// 结构化实体提取 Schema (支持内联 JSON 或 JSON 模式文件路径，直接抽取所需字段并削减 Token)
    #[arg(
        long = "schema",
        value_name = "JSON_OR_PATH",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub schema: Option<String>,

    /// Fail when Schema parsing or extraction fails; requires --schema
    #[arg(long = "schema-strict", requires = "schema")]
    pub schema_strict: bool,

    /// 分析目标页面 DOM 结构骨架，自动探测重复列表容器并生成建议的 Schema 抽取模版 (节省 Agent Token)
    #[arg(long = "scaffold", help_heading = "通用提取选项 (General Options)")]
    pub scaffold: bool,

    /// 启用语义感知切块 (Semantic Chunking，按标题层级与段落切分，保护代码块与表格，供 RAG 向量入库)
    #[arg(long = "chunk", help_heading = "通用提取选项 (General Options)")]
    pub chunk: bool,

    /// 目标切块 Token 大小 [默认: 500]
    #[arg(
        long = "chunk-size",
        value_name = "N",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub chunk_size: Option<usize>,

    /// 相邻切块间的 Token 重叠量 [默认: 50]
    #[arg(
        long = "chunk-overlap",
        value_name = "N",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub chunk_overlap: Option<usize>,

    /// 下载 Markdown 中的全部远程图片到本地资产目录并自动回写本地相对链接
    #[arg(
        long = "download-images",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub download_images: bool,

    /// 本地图片资产保存目录 [默认: ./assets]
    #[arg(
        long = "assets-dir",
        value_name = "DIR",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub assets_dir: Option<String>,

    /// 仅提取并输出页面中的全部数据表格，跳过正文与噪音
    #[arg(
        short = 'T',
        long = "tables-only",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub tables_only: bool,

    /// 表格导出呈现格式: markdown (默认), csv, tsv, json
    #[arg(
        long = "table-format",
        value_enum,
        default_value_t = CliTableFormat::Markdown,
        help_heading = "通用提取选项 (General Options)"
    )]
    pub table_format: CliTableFormat,

    /// 启用终端流式按行输出 (实时 flush 降低首字延迟)
    #[arg(long = "stream", help_heading = "通用提取选项 (General Options)")]
    pub stream: bool,

    /// 启用本地启发式提取式摘要 (生成 3-5 条核心观点速览，无需外部大模型 API Key)
    #[arg(long = "summarize", help_heading = "通用提取选项 (General Options)")]
    pub summarize: bool,

    /// 将提取的 Markdown 按标题层级自动拆分为多文件目录并生成 index.md 导航索引
    #[arg(
        long = "out-dir",
        value_name = "DIR",
        help_heading = "通用提取选项 (General Options)"
    )]
    pub out_dir: Option<PathBuf>,

    // =========================================================================
    // 哔哩哔哩专属选项 (Bilibili Options)
    // =========================================================================
    /// 提取 B站或 YouTube 提供的可访问字幕轨
    #[arg(long = "subtitles", help_heading = "通用提取选项 (General Options)")]
    pub bili_sub: bool,

    /// 提取 B站 官方 AI 视频总结核心摘要与章节大纲
    #[arg(long = "bili-sum", help_heading = "哔哩哔哩专属 (Bilibili)")]
    pub bili_sum: bool,

    /// 提取 B站 视频时间轴高能弹幕 (按出现时间升序排序为 GFM 表格)
    #[arg(long = "danmaku", help_heading = "哔哩哔哩专属 (Bilibili)")]
    pub bili_dm: bool,

    /// 限制提取的时间轴弹幕最大条数
    #[arg(
        long = "danmaku-limit",
        value_name = "N",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_dm_limit: Option<usize>,

    /// 提取 B站 视频高赞精选热评 (包含用户名、点赞数、评论内容与发布时间)
    #[arg(long = "comments", help_heading = "哔哩哔哩专属 (Bilibili)")]
    pub bili_cm: bool,

    /// 限制提取的高赞热评最大条数 [默认: 20] (支持自动跨页获取)
    #[arg(
        long = "comments-limit",
        value_name = "N",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_cm_limit: Option<usize>,

    /// 拉取 B站 首页推荐视频流表格
    #[arg(long = "feed", help_heading = "哔哩哔哩专属 (Bilibili)")]
    pub bili_feed: bool,

    /// 按关键词搜索 B站 视频
    #[arg(
        long = "bili-search",
        value_name = "KEYWORD",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_search: Option<String>,

    /// 搜索结果页码或多 P 视频分 P 编号 (1-indexed)
    #[arg(
        long = "bili-page",
        value_name = "PAGE",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_page: Option<u32>,

    /// 搜索排序规则: totalrank (默认), click (播放), pubdate (最新), dm (弹幕), stow (收藏)
    #[arg(
        long = "bili-order",
        value_name = "ORDER",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_order: Option<String>,

    /// 仅输出 B站 视频高清封面图片 URL
    #[arg(long = "cover", help_heading = "哔哩哔哩专属 (Bilibili)")]
    pub bili_cover: bool,

    /// 下载 B站 视频封面图片到本地 (默认: {bvid}_cover.jpg)
    #[arg(
        long = "download-cover",
        value_name = "PATH",
        num_args = 0..=1,
        default_missing_value = "",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_dl_cover: Option<String>,

    /// 下载 B站 渐进式 MP4 视频流 (默认: {title}.mp4)
    #[arg(
        long = "download-video",
        value_name = "PATH",
        num_args = 0..=1,
        default_missing_value = "",
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_dl: Option<String>,

    /// B站 视频清晰度选择: 720p (默认), 480p, 360p
    #[arg(
        long = "quality",
        value_enum,
        default_value_t = CliQuality::Q720p,
        help_heading = "哔哩哔哩专属 (Bilibili)"
    )]
    pub bili_quality: CliQuality,

    // =========================================================================
    // 网络与身份认证 (Network & Auth)
    // =========================================================================
    /// Cookie header string
    #[arg(
        short = 'c',
        long = "cookie",
        value_name = "COOKIE",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub cookie: Option<String>,

    /// Path to cookie credential file (supports multi-platform prefix format, e.g. bilibili: ..., zhihu: ...)
    #[arg(
        long = "cookie-file",
        value_name = "PATH",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub cookie_file: Option<PathBuf>,

    /// Custom User-Agent header
    #[arg(
        short = 'u',
        long = "user-agent",
        value_name = "UA",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub user_agent: Option<String>,

    /// Custom HTTP request headers in "Key: Value" format (can be specified multiple times)
    #[arg(
        short = 'H',
        long = "header",
        value_name = "KEY:VALUE",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub headers: Vec<String>,

    /// Network request timeout in seconds
    #[arg(
        long = "timeout",
        default_value_t = 15,
        value_name = "SECS",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub timeout_secs: u64,

    /// Network connection handshake timeout in seconds
    #[arg(
        long = "connect-timeout",
        default_value_t = 10,
        value_name = "SECS",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub connect_timeout: u64,

    /// Number of retry attempts with exponential backoff on 5xx server errors or network drops
    #[arg(
        long = "retries",
        default_value_t = 0,
        value_name = "N",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub retries: u32,

    /// Shared cap on total retries across the run (all hosts); omitted means unlimited
    #[arg(
        long = "retry-budget",
        value_name = "N",
        value_parser = clap::value_parser!(u32).range(1..=1000),
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub retry_budget: Option<u32>,

    /// 日志输出格式：plain（默认）或 json（每行一个 JSON 对象）
    #[arg(
        long = "log-format",
        value_enum,
        default_value_t = CliLogFormat::Plain,
        global = true,
        help_heading = "缓存与运行诊断 (Cache & Diagnostics)"
    )]
    pub log_format: CliLogFormat,

    /// Maximum response download size in megabytes
    #[arg(
        long = "max-size",
        default_value_t = 10,
        value_name = "MB",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub max_size_mb: usize,

    /// Behavior when response size exceeds max-size: 'error' (default) or 'truncate'
    #[arg(long = "on-too-large", value_enum, default_value_t = CliOnTooLarge::Error, help_heading = "网络与身份认证 (Network & Auth)")]
    pub on_too_large: CliOnTooLarge,

    /// Allow fetching private IP addresses, localhost, and internal network endpoints (bypasses SSRF guard)
    #[arg(
        long = "allow-private-addresses",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub allow_private_addresses: bool,

    /// Chrome DevTools Protocol (CDP) 远程调试端点 (如 http://127.0.0.1:9222 或 ws://...)，用于 JS 动态渲染或防爬挑战绕过
    #[arg(
        long = "cdp",
        value_name = "ENDPOINT",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub cdp: Option<String>,

    /// CDP 页面加载后额外等待 DOM 动态渲染完成的时间 (毫秒) [默认: 1000]
    #[arg(
        long = "cdp-wait",
        value_name = "MS",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub cdp_wait_ms: Option<u64>,

    /// 自动探测反爬挑战并使用 CDP 浏览器渲染
    #[arg(
        long = "auto-cdp",
        global = true,
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub auto_cdp: bool,

    /// 启用 CDP 页面平滑自动下滚以触发延迟渲染和无限滚动列表加载 (默认: 关闭)
    #[arg(
        long = "cdp-auto-scroll",
        global = true,
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub cdp_auto_scroll: bool,

    /// 禁用 CDP 静态重资产拦截 (默认开启拦截以减少资源加载；开启此开关将恢复加载图片、字体与媒体)
    #[arg(
        long = "cdp-no-block-resources",
        global = true,
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub cdp_no_block_resources: bool,

    /// 通过 CDP 无头浏览器截取渲染后的页面高清快照并保存为 PNG/JPEG 图像 (供多模态模型分析)
    #[arg(
        long = "screenshot",
        value_name = "PATH",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub screenshot: Option<PathBuf>,

    /// 将渲染后的页面打印为 PDF 并保存（需要本地 Chrome/Edge 或 CDP 服务）
    #[arg(
        long = "pdf",
        value_name = "PATH",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub pdf: Option<PathBuf>,

    /// Do not fail with non-zero exit code on HTTP 4xx or 5xx error responses (returns body with exit 0)
    #[arg(
        long = "allow-http-errors",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub allow_http_errors: bool,

    /// HTTP/HTTPS 代理服务器地址 (例如 http://127.0.0.1:7890)
    #[arg(
        short = 'p',
        long = "proxy",
        value_name = "PROXY_URL",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub proxy: Option<String>,

    /// 模拟移动端设备 (iPhone / Safari) 访问以触发轻量移动端排版，有效减少桌面端冗余噪音
    #[arg(long = "mobile", help_heading = "网络与身份认证 (Network & Auth)")]
    pub mobile: bool,

    /// Base URL to resolve relative links when reading HTML from stdin
    #[arg(
        long = "base-url",
        value_name = "URL",
        help_heading = "网络与身份认证 (Network & Auth)"
    )]
    pub base_url: Option<String>,

    // =========================================================================
    // 缓存与运行诊断 (Cache & Diagnostics)
    // =========================================================================
    /// Run as native Model Context Protocol (MCP) server over stdio
    #[arg(long = "mcp", help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub mcp: bool,

    /// 启用 MCP 两阶段动态工具发现模式 (极简常驻上下文，减少初始工具声明)
    #[arg(
        long = "mcp-dynamic",
        help_heading = "缓存与运行诊断 (Cache & Diagnostics)"
    )]
    pub mcp_dynamic: bool,

    /// 启用本地两级响应缓存加速 (默认关闭；开启后默认 900 秒 TTL，支持 HTTP 304 条件式重新验证)
    #[arg(long = "cache", help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub cache: bool,

    /// Explicitly disable local response caching
    #[arg(
        long = "no-cache",
        conflicts_with = "cache",
        help_heading = "缓存与运行诊断 (Cache & Diagnostics)"
    )]
    pub no_cache: bool,

    /// Cache TTL in seconds (default: 900 = 15 minutes)
    #[arg(
        long = "cache-ttl",
        default_value_t = 900,
        value_name = "SECS",
        help_heading = "缓存与运行诊断 (Cache & Diagnostics)"
    )]
    pub cache_ttl: u64,

    /// Custom directory for persistent file cache
    #[arg(
        long = "cache-dir",
        value_name = "DIR",
        help_heading = "缓存与运行诊断 (Cache & Diagnostics)"
    )]
    pub cache_dir: Option<PathBuf>,

    /// Tokenizer model encoding scheme for token counting and truncation
    #[arg(long = "tokenizer", value_enum, default_value_t = CliTokenizer::O200k, help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub tokenizer: CliTokenizer,

    /// Write output payload to file instead of stdout
    #[arg(
        short = 'o',
        long = "output",
        value_name = "FILE",
        help_heading = "缓存与运行诊断 (Cache & Diagnostics)"
    )]
    pub output_file: Option<std::path::PathBuf>,

    /// Verbose diagnostics on stderr (-v warnings, -vv connection info, -vvv debug, -vvvv trace)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true, help_heading = "缓存与运行诊断 (Cache & Diagnostics)")]
    pub verbose: u8,

    /// Silence all diagnostic logs and progress messages on stderr
    #[arg(
        short = 'q',
        long = "quiet",
        global = true,
        help_heading = "缓存与运行诊断 (Cache & Diagnostics)"
    )]
    pub quiet: bool,

    /// 输出详细的 Token 经济性与管道耗时基准遥测报告 (显示 Token 节省比例与降噪率)
    #[arg(
        long = "stats",
        global = true,
        help_heading = "缓存与运行诊断 (Cache & Diagnostics)"
    )]
    pub stats: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum SubCommand {
    /// 获取公开微信公众号文章、关联合集和合集分页列表
    Wechat(WechatArgs),
    /// 按配置进行通用分页和列表详情采集，输出 JSON 报告
    Collect(CollectionArgs),
    /// 保存采集快照，输出新增、更新和删除事件
    Sync(SyncArgs),
    /// 管理本地凭据存储（set 从 stdin 读取 Cookie）
    Auth(AuthArgs),
    /// 执行一轮或多轮采集并输出变更事件（可配合 cron 定时执行）
    Watch(WatchArgs),
    /// 执行受限浏览器步骤后提取网页，输出 PageDocument JSON
    Browse(BrowseArgs),
    /// 启动原生 Model Context Protocol (MCP) stdio 服务端 (供 Claude Desktop / Cursor 直连)
    #[command(
        name = "mcp",
        about = "启动原生 Model Context Protocol (MCP) 服务端 (供 Claude Desktop / Cursor 直连)"
    )]
    Mcp(McpArgs),

    /// 交互式登录并提取保存平台凭据 (自动带平台前缀隔离存储至 cookies.txt，防覆盖)
    #[command(
        name = "login",
        about = "交互式登录并提取保存平台凭据 (自动带平台前缀隔离存储至 cookies.txt，多网站互不覆盖)"
    )]
    Login(LoginArgs),

    /// 免 API Key 全网搜索与生态定向检索 (支持 Bing 一键 --fetch-content 搜索即抓取)
    #[command(
        name = "search",
        about = "免 API Key 全网搜索与生态定向检索 (支持 Bing 一键 --fetch-content 搜索即抓取)"
    )]
    Search(SearchSubArgs),

    /// 并发抓取与批量 URL 提取流水线 (多协程并发池高效处理文件或 stdin 列表)
    #[command(
        name = "batch",
        about = "并发抓取与批量 URL 提取流水线 (多协程并发池高效处理文件或 stdin 列表)"
    )]
    Batch(BatchArgs),

    /// 整站文档递归爬取与知识库汇编 (自动构建带 TOC 目录索引的单文件 Markdown 知识库)
    #[command(
        name = "crawl",
        about = "整站文档递归爬取与知识库汇编 (自动构建带 TOC 目录索引的单文件 Markdown 知识库)"
    )]
    Crawl(CrawlArgs),

    /// 本地响应缓存查看与清理维护 (查看磁盘占用/条目统计，或清空缓存)
    #[command(
        name = "cache",
        about = "本地响应缓存查看与清理维护 (查看磁盘占用/条目统计，或清空缓存)"
    )]
    Cache(CacheSubArgs),

    /// 启动交互式 REPL 探索会话命令行 (支持 fetch/search/cache/links 交互式快速调研)
    #[command(
        name = "shell",
        about = "启动交互式 REPL 会话命令行 (支持 fetch/search/cache/links 交互式快速调研)"
    )]
    Shell(ShellArgs),

    /// 对比两个文档/页面版本，输出行级变更分析与 Diff 报表
    #[command(
        name = "diff",
        about = "对比两个文本文件或网页版本，输出统一 Diff 格式与行级变更比例"
    )]
    Diff(DiffArgs),

    /// 提取网页或 HTML 文件中的全部表格，并导出为 SQL、CSV、TSV、JSON 或 Markdown
    #[command(
        name = "tables",
        about = "提取网页或 HTML 中的全部数据表格，并直接导出为 SQL DDL/INSERT、CSV、TSV 或 JSON"
    )]
    Tables(TablesArgs),

    /// 生成 Shell (Bash, Zsh, Fish, PowerShell, Elvish) 终端自动补全脚本
    #[command(
        name = "completions",
        about = "生成 Shell (Bash, Zsh, Fish, PowerShell, Elvish) 终端自动补全脚本"
    )]
    Completions(CompletionsArgs),

    /// 列出支持的全部 22 平台原生适配器、匹配域名与专属优化能力
    #[command(
        name = "adapters",
        about = "列出支持的全部 22 平台原生适配器、匹配域名与专属优化能力"
    )]
    Adapters(AdaptersArgs),

    /// 检查并自动升级 TkLess 二进制至 GitHub 最新版本 (类似 bun upgrade / rustup update)
    #[command(
        name = "upgrade",
        about = "检查并自动升级 TkLess 二进制至 GitHub 最新版本 (类似 bun upgrade)"
    )]
    Upgrade(UpgradeArgs),

    /// 多跳网络调研流水线 (拆解维度、Bing 检索、跟踪引链、汇编摘录报告)
    #[command(
        name = "research",
        about = "多跳网络调研流水线 (拆解维度、Bing 检索、跟踪引链、汇编摘录报告)"
    )]
    Research(ResearchSubArgs),

    /// 诊断并自检当前运行环境 (CDP 浏览器就绪、网络核心服务连通性、缓存读写权限)
    #[command(
        name = "doctor",
        about = "诊断并自检当前运行环境 (CDP 浏览器就绪、网络核心服务连通性、缓存读写权限)"
    )]
    Doctor(DoctorArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct CollectionArgs {
    /// JSON 采集配置文件
    #[arg(long, short = 'c')]
    pub config: PathBuf,
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,
    /// 不完整结果仍返回成功退出码，报告保留停止原因
    #[arg(long)]
    pub allow_partial: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct SyncArgs {
    #[command(flatten)]
    pub collection: CollectionArgs,
    /// 快照与增量清单目录，同一目录绑定同一采集配置
    #[arg(long)]
    pub state: PathBuf,
    #[arg(long, default_value_t = tkless_core::limits::SYNC_DEFAULT_STATE_MB, value_parser = clap::value_parser!(u64).range(1..=tkless_core::limits::SYNC_MAX_STATE_MB))]
    pub max_state_mb: u64,
}

#[derive(clap::Args, Debug, Clone)]
pub struct AuthArgs {
    /// set、list 或 remove
    #[arg(value_name = "ACTION")]
    pub action: String,
    /// 平台名称（set / remove 必填）
    #[arg(value_name = "PLATFORM")]
    pub platform: Option<String>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct WatchArgs {
    /// JSON 监控配置文件（jobs、diff 与通知目标）
    #[arg(long, short = 'c')]
    pub config: PathBuf,
    /// 快照与事件状态目录，每个 job 使用独立子目录
    #[arg(long)]
    pub state: PathBuf,
    /// 前台循环执行间隔秒数；不指定则只执行一轮
    #[arg(long, value_parser = clap::value_parser!(u64).range(tkless_core::limits::WATCH_MIN_INTERVAL_SECS..=86400))]
    pub interval: Option<u64>,
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,
    /// 允许部分结果并返回成功退出码，报告保留停止原因
    #[arg(long)]
    pub allow_partial: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct BrowseArgs {
    pub url: String,
    /// BrowserPlan JSON 文件（endpoint、timeout_ms、steps）
    #[arg(long)]
    pub plan: PathBuf,
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct WechatArgs {
    /// 公众号显示名称、文章 URL 或公开合集 URL
    pub source: String,
    #[arg(long, short = 'l', default_value_t = tkless_core::wechat::DEFAULT_LIMIT)]
    pub limit: usize,
    #[arg(long, default_value_t = tkless_core::wechat::DEFAULT_PAGES)]
    pub max_pages: usize,
    #[arg(long, default_value_t = 300)]
    pub delay_ms: u64,
    /// 获取列表中每篇文章的正文
    #[arg(long)]
    pub fetch_content: bool,
    /// 仅接受指定 __biz 的公众号文章
    #[arg(long)]
    pub expected_biz: Option<String>,
    /// 文章入口仅识别身份和合集，不继续抓取合集列表
    #[arg(long)]
    pub no_follow_albums: bool,
    #[arg(long, default_value_t = 4000, value_parser = clap::value_parser!(u32).range(1..=100000))]
    pub max_tokens_per_article: u32,
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,
    /// 接受搜索发现、预算中断等非完整列表，返回报告中的明确状态
    #[arg(long)]
    pub allow_partial: bool,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct UpgradeArgs {
    /// 仅检查最新版本发布情况，不执行下载或替换
    #[arg(long = "check")]
    pub check: bool,

    /// 即使当前已是最新版本，亦强制重新下载并覆盖安装
    #[arg(short = 'f', long = "force")]
    pub force: bool,

    /// 自定义 GitHub 仓库所有者与项目 (默认: 'smallhug/tkless')
    #[arg(long = "repo", default_value = "smallhug/tkless")]
    pub repo: String,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct DoctorArgs {
    /// HTTP/HTTPS 代理服务器地址 (例如 http://127.0.0.1:7890)
    #[arg(short = 'p', long = "proxy", value_name = "PROXY_URL")]
    pub proxy: Option<String>,

    /// Write output payload to file instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<std::path::PathBuf>,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct AdaptersArgs {
    /// Output presentation format [default: markdown]
    #[arg(short = 'f', long = "format", value_enum, default_value_t = CliFormat::Markdown)]
    pub format: CliFormat,

    /// Write output payload to file instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<std::path::PathBuf>,
}

/// Command-line arguments for autonomous multi-hop deep research.
#[derive(clap::Args, Clone, Debug)]
pub struct ResearchSubArgs {
    /// Research question, thesis, or technical topic
    #[arg(value_name = "TOPIC")]
    pub topic: String,

    /// Research hop depth (1: single-hop broad search, 2: two-hop citation tracking)
    #[arg(short = 'd', long = "depth", default_value_t = 2, value_name = "HOPS")]
    pub depth: usize,

    /// Number of research dimensions/sub-queries to explore (2-5)
    #[arg(short = 'b', long = "breadth", default_value_t = 3, value_name = "N")]
    pub breadth: usize,

    /// Maximum number of total pages to analyze (3-20)
    #[arg(short = 'm', long = "max-pages", default_value_t = 8, value_name = "N")]
    pub max_pages: usize,

    /// Maximum tokens extracted per webpage
    #[arg(
        long = "max-tokens-per-page",
        default_value_t = 1500,
        value_name = "TOKENS"
    )]
    pub max_tokens_per_page: usize,

    /// Overall token budget for the compiled research report
    #[arg(
        short = 't',
        long = "max-tokens",
        default_value_t = 8000,
        value_name = "TOKENS"
    )]
    pub max_tokens: usize,

    /// Output presentation format
    #[arg(short = 'f', long = "format", default_value = "markdown", value_parser = search_output_formats(false))]
    pub format: CliFormat,

    /// Write compiled research report to file instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Disable SimHash deduplication
    #[arg(long = "no-dedup")]
    pub no_dedup: bool,

    /// HTTP/HTTPS 代理服务器地址 (例如 http://127.0.0.1:7890)
    #[arg(short = 'p', long = "proxy", value_name = "PROXY_URL")]
    pub proxy: Option<String>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct CompletionsArgs {
    /// Target shell family (bash, zsh, fish, powershell, elvish)
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct ShellArgs {}

#[derive(clap::Args, Debug, Clone)]
pub struct DiffArgs {
    /// Original (old) text file path or URL
    #[arg(value_name = "OLD_TARGET")]
    pub old_target: String,

    /// Updated (new) text file path or URL
    #[arg(value_name = "NEW_TARGET")]
    pub new_target: String,

    /// Output presentation format [default: markdown]
    #[arg(short = 'f', long = "format", value_enum, default_value_t = CliFormat::Markdown)]
    pub format: CliFormat,

    /// Write output payload to file instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<std::path::PathBuf>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct TablesArgs {
    /// Target webpage URL, path to local HTML file, or '-' to read from stdin
    #[arg(value_name = "TARGET")]
    pub target: String,

    /// Table export presentation format [default: sql]
    #[arg(
        short = 'f',
        long = "format",
        value_enum,
        default_value_t = CliTableFormat::Sql
    )]
    pub format: CliTableFormat,

    /// Custom SQL table name identifier when format is 'sql' [default: 'extracted_table']
    #[arg(long = "table-name", default_value = "extracted_table")]
    pub table_name: String,

    /// Optional 0-indexed specific table to extract
    #[arg(long = "index")]
    pub table_index: Option<usize>,

    /// Write output payload to file instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<std::path::PathBuf>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct CacheSubArgs {
    /// 显示当前本地缓存的容量、条目总数与存储路径
    #[arg(long = "stats", short = 's')]
    pub stats: bool,

    /// 清理并删除全部本地磁盘与内存缓存条目
    #[arg(long = "clear", short = 'c')]
    pub clear: bool,

    /// 自定义缓存存储目录路径
    #[arg(long = "cache-dir")]
    pub cache_dir: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct SearchSubArgs {
    /// Show detailed diagnostics in Markdown; JSON/JSONL always contain a report
    #[arg(long = "diagnostics")]
    pub diagnostics: bool,
    /// Search query keyword or question
    #[arg(value_name = "QUERY")]
    pub query: String,

    /// Maximum number of search results to return
    #[arg(short = 'l', long = "limit", default_value_t = 10, value_name = "N", value_parser = positive_search_count)]
    pub limit: usize,

    /// Output presentation format
    #[arg(short = 'f', long = "format", default_value = "markdown", value_parser = search_output_formats(true))]
    pub format: CliFormat,

    /// Write search results to file instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Restrict Bing search to a platform (for example github, zhihu, arxiv)
    #[arg(long = "scope", value_enum, value_name = "PLATFORM")]
    pub scope: Option<CliSearchScope>,

    /// Fetch and extract full cleaned content for each search result in parallel
    #[arg(
        long = "fetch-content",
        help = "Concurrently fetch and extract clean Markdown content for each search result"
    )]
    pub fetch_content: bool,

    /// 启用 64-bit SimHash 汉明距离去重，自动过滤高度雷同的转载/镜像文章 (仅当 fetch-content 时生效)
    #[arg(
        long = "dedup",
        requires = "fetch_content",
        help = "Filter near-duplicate pages via 64-bit SimHash; requires --fetch-content"
    )]
    pub dedup: bool,

    /// Maximum tokens extracted per webpage during search & fetch
    #[arg(
        long = "max-tokens-per-page",
        value_name = "N",
        requires = "fetch_content",
        value_parser = positive_search_count,
        help = "Maximum tokens extracted per webpage during search & fetch [default: 1500]"
    )]
    pub max_tokens_per_page: Option<usize>,

    /// Chrome DevTools Protocol (CDP) 远程调试端点 (如 http://127.0.0.1:9222)
    #[arg(long = "cdp", value_name = "ENDPOINT")]
    pub cdp: Option<String>,

    /// CDP DOM 加载额外等待时间 (毫秒)，要求显式 --cdp 端点
    #[arg(long = "cdp-wait", value_name = "MS")]
    pub cdp_wait_ms: Option<u64>,

    /// HTTP/HTTPS 代理服务器地址 (例如 http://127.0.0.1:7890)
    #[arg(short = 'p', long = "proxy", value_name = "PROXY_URL")]
    pub proxy: Option<String>,

    /// 模拟移动端设备 User-Agent
    #[arg(long = "mobile")]
    pub mobile: bool,

    /// 仅返回该日期之后（含）的结果，格式 YYYY-MM-DD
    #[arg(long = "since", value_name = "DATE")]
    pub since: Option<String>,

    /// 仅返回该日期之前（含）的结果，格式 YYYY-MM-DD
    #[arg(long = "until", value_name = "DATE")]
    pub until: Option<String>,

    /// 结果语言，例如 zh-CN 或 en
    #[arg(long = "lang", value_name = "LANG")]
    pub language: Option<String>,

    /// 地区/国家代码，例如 cn 或 us
    #[arg(long = "region", value_name = "CC")]
    pub region: Option<String>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct BatchArgs {
    /// Output order for per-item results
    #[arg(long = "order", value_enum, default_value_t = CliBatchOrder::Input)]
    pub order: CliBatchOrder,
    /// Durable checkpoint file for this batch (must not already exist)
    #[arg(long = "checkpoint", value_name = "FILE")]
    pub checkpoint: Option<PathBuf>,
    /// Resume an existing checkpoint; completed results are replayed
    #[arg(long = "resume", requires = "checkpoint")]
    pub resume: bool,
    /// Re-fetch failed items when resuming; successful items remain saved
    #[arg(long = "retry-failed", requires = "resume")]
    pub retry_failed: bool,
    /// Maximum simultaneous page tasks for a single host
    #[arg(long = "per-host-concurrency", default_value_t = 2, value_parser = batch_concurrency)]
    pub per_host_concurrency: usize,
    /// Minimum interval between page-task starts for a single host
    #[arg(long = "per-host-delay-ms", default_value_t = 0)]
    pub per_host_delay_ms: u64,
    /// Return success when some URLs fail; per-URL errors remain in the output
    #[arg(long = "allow-partial")]
    pub allow_partial: bool,
    /// Path to file containing URLs (one per line) or '-' to read from standard input
    #[arg(value_name = "FILE")]
    pub file: String,

    /// Concurrency limit for simultaneous network requests [default: 5]
    #[arg(
        short = 'c',
        long = "concurrency",
        default_value_t = 5,
        value_name = "N",
        value_parser = batch_concurrency
    )]
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

    /// 结构化实体提取 Schema (支持内联 JSON 或 JSON 模式文件路径)
    #[arg(long = "schema", value_name = "JSON_OR_PATH")]
    pub schema: Option<String>,

    /// Fail when Schema parsing or extraction fails; requires --schema
    #[arg(long = "schema-strict", requires = "schema")]
    pub schema_strict: bool,

    /// Chrome DevTools Protocol (CDP) 远程调试端点 (如 http://127.0.0.1:9222)
    #[arg(long = "cdp", value_name = "ENDPOINT")]
    pub cdp: Option<String>,

    /// CDP 页面加载后额外等待 DOM 动态渲染完成的时间 (毫秒)
    #[arg(long = "cdp-wait", value_name = "MS")]
    pub cdp_wait_ms: Option<u64>,

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

    /// HTTP/HTTPS 代理服务器地址 (例如 http://127.0.0.1:7890)
    #[arg(short = 'p', long = "proxy", value_name = "PROXY_URL")]
    pub proxy: Option<String>,

    /// 模拟移动端设备 User-Agent
    #[arg(long = "mobile")]
    pub mobile: bool,

    /// 全局页面任务最小启动间隔 (毫秒)
    #[arg(long = "delay-ms", default_value_t = 0, value_name = "MS")]
    pub delay_ms: u64,
}

fn batch_concurrency(raw: &str) -> Result<usize, String> {
    raw.parse::<usize>()
        .ok()
        .filter(|value| (1..=64).contains(value))
        .ok_or_else(|| "value must be between 1 and 64".into())
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CliBatchOrder {
    #[default]
    Input,
    Completion,
}

impl From<CliBatchOrder> for tkless_core::batch::BatchOrder {
    fn from(value: CliBatchOrder) -> Self {
        match value {
            CliBatchOrder::Input => Self::Input,
            CliBatchOrder::Completion => Self::Completion,
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
pub struct LoginArgs {
    /// Target platform to log in (e.g. 'bilibili')
    #[arg(value_name = "PLATFORM")]
    pub platform: String,

    /// Destination file path to save extracted cookies
    #[arg(
        short = 'o',
        long = "output",
        default_value = "./cookies.txt",
        value_name = "FILE"
    )]
    pub output: PathBuf,

    /// Do not launch default web browser automatically (print terminal QR code and URL only)
    #[arg(long = "no-browser")]
    pub no_browser: bool,

    /// Maximum waiting timeout in seconds
    #[arg(long = "timeout", default_value_t = 180, value_name = "SECS")]
    pub timeout_secs: u64,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct McpArgs {
    /// 启用两阶段动态工具发现 (仅暴露 2 核心工具 + 4 发现元工具，大幅降低 Token 开销)
    #[arg(long = "dynamic")]
    pub dynamic: bool,

    /// MCP 工具配置模式: 'full' (全量 27 工具) 或 'dynamic' (两阶段动态发现)
    #[arg(long = "profile", default_value = "full", value_name = "PROFILE")]
    pub profile: String,

    /// Disable response caching in MCP server
    #[arg(long = "no-cache")]
    pub no_cache: bool,

    /// Cache TTL in seconds (default: 900 = 15 minutes)
    #[arg(long = "cache-ttl", default_value_t = 900, value_name = "SECS")]
    pub cache_ttl: u64,

    /// Custom directory for persistent file cache
    #[arg(long = "cache-dir", value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,

    /// 通过 HTTP 提供 JSON-RPC（例如 127.0.0.1:8787）；省略时使用 stdio
    #[arg(long = "http", value_name = "ADDR")]
    pub http: Option<String>,

    /// 单次工具调用超时秒数
    #[arg(long = "tool-timeout-secs", default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub tool_timeout_secs: u64,

    /// 从环境变量读取 Bearer Token；非回环绑定必须设置
    #[arg(long = "auth-token-env", value_name = "NAME")]
    pub auth_token_env: Option<String>,

    /// 显式允许绑定非回环地址（必须同时提供 Token）
    #[arg(long = "allow-public-bind")]
    pub allow_public_bind: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct CrawlArgs {
    #[arg(long, requires = "resume")]
    pub retry_failed: bool,
    #[arg(long)]
    pub checkpoint: Option<PathBuf>,
    #[arg(long, requires = "checkpoint")]
    pub resume: bool,
    #[arg(long)]
    pub max_seconds: Option<u64>,
    #[arg(long)]
    pub max_output_bytes: Option<u64>,
    #[arg(long, default_value_t = 10000)]
    pub max_pending_urls: usize,
    /// 读取 robots.txt 并按 TkLess / * 规则过滤页面
    #[arg(long)]
    pub respect_robots: bool,
    /// Return success despite failed pages; errors remain in the report
    #[arg(long = "allow-partial")]
    pub allow_partial: bool,
    /// Seed URL to start deep site crawling
    #[arg(value_name = "URL")]
    pub url: String,

    /// Maximum link traversal depth [default: 2]
    #[arg(short = 'd', long = "max-depth", default_value_t = 2, value_name = "N")]
    pub max_depth: usize,

    /// Maximum number of unique pages to crawl [default: 20]
    #[arg(
        short = 'P',
        long = "max-pages",
        default_value_t = 20,
        value_name = "N"
    )]
    pub max_pages: usize,

    /// Concurrency limit for simultaneous network requests [default: 5]
    #[arg(
        short = 'c',
        long = "concurrency",
        default_value_t = 5,
        value_name = "N"
    )]
    pub concurrency: usize,

    /// Restrict crawling strictly to URLs sharing the same path prefix as seed URL
    #[arg(
        long = "same-path-prefix",
        help = "Restrict crawling strictly to URLs sharing the same path prefix as seed URL"
    )]
    pub same_path_prefix: bool,

    /// Output presentation format [default: markdown]
    #[arg(short = 'f', long = "format", value_enum, default_value_t = CliFormat::Markdown)]
    pub format: CliFormat,

    /// Write assembled crawl report to file instead of stdout
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Chrome DevTools Protocol (CDP) 远程调试端点 (如 http://127.0.0.1:9222)
    #[arg(long = "cdp", value_name = "ENDPOINT")]
    pub cdp: Option<String>,

    /// CDP 页面加载后额外等待 DOM 动态渲染完成的时间 (毫秒)
    #[arg(long = "cdp-wait", value_name = "MS")]
    pub cdp_wait_ms: Option<u64>,

    /// 强制以站点地图 (Sitemap XML) 模式解析种子 URL 或自动提取规范 URL 列表爬取
    #[arg(
        long = "sitemap",
        help = "Parse seed URL as XML sitemap to extract and crawl canonical URLs directly"
    )]
    pub sitemap: bool,

    /// HTTP/HTTPS 代理服务器地址 (例如 http://127.0.0.1:7890)
    #[arg(short = 'p', long = "proxy", value_name = "PROXY_URL")]
    pub proxy: Option<String>,

    /// 启用基于 64位 SimHash 网页指纹的内容去重 (自动跳过内容近似度高的重复镜像页面)
    #[arg(
        long = "dedup",
        help = "Automatically skip near-duplicate crawled pages based on 64-bit SimHash"
    )]
    pub dedup: bool,

    /// 模拟移动端设备 User-Agent
    #[arg(long = "mobile")]
    pub mobile: bool,

    /// 递归爬取单页面间歇延迟等待时间 (毫秒)，防止触发服务端 429 速率限制
    #[arg(long = "delay-ms", default_value_t = 0, value_name = "MS")]
    pub delay_ms: u64,

    /// 将抓取结果打包为 Markdown ZIP（README 索引 + 每页一个 .md）
    #[arg(long = "bundle", value_name = "FILE")]
    pub bundle: Option<PathBuf>,
}
