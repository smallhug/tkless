use crate::error::TklessError;
use crate::model::ExtractionOptions;
use crate::TklessEngine;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use tracing::{debug, warn};
mod diagnostics;
#[cfg(test)]
mod tests_reliability;
use diagnostics::ParsedPage;
pub use diagnostics::{SearchPageDiagnostics, SearchReport, SearchStatus};

/// Optional, best-effort search filters. Parameters are undocumented Bing
/// query parameters; callers must treat results as unverified.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchFilters {
    /// Inclusive lower bound (`YYYY-MM-DD`).
    pub since: Option<String>,
    /// Inclusive upper bound (`YYYY-MM-DD`).
    pub until: Option<String>,
    /// Result language, e.g. `zh-CN` or `en`.
    pub language: Option<String>,
    /// Region/country code, e.g. `cn` or `us`.
    pub region: Option<String>,
}

impl SearchFilters {
    pub fn is_empty(&self) -> bool {
        self.since.is_none()
            && self.until.is_none()
            && self.language.is_none()
            && self.region.is_none()
    }

    pub fn validate(&self) -> Result<(), TklessError> {
        let start = self.since.as_deref().map(parse_filter_date).transpose()?;
        let end = self.until.as_deref().map(parse_filter_date).transpose()?;
        if let (Some(start), Some(end)) = (start, end) {
            if start > end {
                return Err(TklessError::InvalidArgument(
                    "search --since must not be later than --until".into(),
                ));
            }
        }
        if let Some(language) = &self.language {
            let valid = (2..=16).contains(&language.len())
                && language
                    .bytes()
                    .all(|byte| byte.is_ascii_alphabetic() || byte == b'-');
            if !valid {
                return Err(TklessError::InvalidArgument(
                    "search language must be 2..16 ASCII letters or hyphens".into(),
                ));
            }
        }
        if let Some(region) = &self.region {
            if region.len() != 2 || !region.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                return Err(TklessError::InvalidArgument(
                    "search region must be a two-letter country code".into(),
                ));
            }
        }
        Ok(())
    }

    fn apply(&self, url: &mut url::Url) -> Result<(), TklessError> {
        self.validate()?;
        let mut pairs = url.query_pairs_mut();
        if let Some(language) = &self.language {
            pairs.append_pair("setlang", &language.to_ascii_lowercase());
        }
        if let Some(region) = &self.region {
            pairs.append_pair("cc", &region.to_ascii_lowercase());
        }
        if self.since.is_some() || self.until.is_some() {
            let start = match &self.since {
                Some(value) => parse_filter_date(value)?,
                None => parse_filter_date("1970-01-01")?,
            };
            let end = match &self.until {
                Some(value) => parse_filter_date(value)?,
                None => chrono::Utc::now().date_naive(),
            };
            let epoch = parse_filter_date("1970-01-01")?;
            let start_days = (start - epoch).num_days();
            let end_days = (end - epoch).num_days();
            pairs.append_pair("filters", &format!("ex1:\"ez5_{start_days}_{end_days}\""));
        }
        Ok(())
    }
}

fn parse_filter_date(value: &str) -> Result<chrono::NaiveDate, TklessError> {
    let date = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
        TklessError::InvalidArgument(format!(
            "Invalid search date '{value}'; expected YYYY-MM-DD"
        ))
    })?;
    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("static epoch");
    if date < epoch {
        return Err(TklessError::InvalidArgument(
            "Search dates must be on or after 1970-01-01".into(),
        ));
    }
    Ok(date)
}

/// Current search configuration; all calls return SearchReport.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub scope: Option<String>,
    pub limit: usize,
    pub cdp: Option<crate::CdpOptions>,
    pub auto_cdp: bool,
    pub filters: SearchFilters,
}
impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            scope: None,
            limit: 10,
            cdp: None,
            auto_cdp: false,
            filters: SearchFilters::default(),
        }
    }
}

// Bing Selectors
static BING_ITEM_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("li.b_algo, .b_algo").unwrap());
static BING_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h2 a, .b_title a").unwrap());
static BING_SNIPPET_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".b_caption p, .b_algoSlug, .b_snippet, .b_lineclamp2, .b_lineclamp3, .b_lineclamp4, .b_paractl").unwrap()
});
static BING_NEXT_PAGE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"href="(/search\?[^"]*first=11[^"]*)""#).unwrap());

/// Structured web search result item with provider attribution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResultItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub engine: String,
}

/// Detailed search result containing concurrently fetched and cleaned page content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFetchItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_details: Option<crate::error::ErrorDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction: Option<crate::model::ExtractionProvenance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<crate::model::DiagnosticWarning>,
    pub rank: usize,
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub engine: String,
    pub content: Option<String>,
    pub token_count: usize,
    pub status: u16,
    pub error: Option<String>,
}

/// Fetch a supplied result list without repeating the search request.
pub async fn fetch_search_results(
    engine: &TklessEngine,
    search_results: Vec<SearchResultItem>,
    base_options: &ExtractionOptions,
    max_tokens_per_page: Option<usize>,
    dedup: bool,
) -> Result<Vec<SearchFetchItem>, TklessError> {
    base_options.validate()?;
    if search_results.is_empty() {
        return Ok(Vec::new());
    }

    debug!(
        count = search_results.len(),
        "Concurrently fetching search result contents"
    );

    // 2. Parallel fetch phase
    let per_page_tokens = max_tokens_per_page.unwrap_or(1500);
    let mut page_options = base_options.clone();
    page_options.max_tokens = Some(per_page_tokens);

    // Pre-filter duplicate URLs before parallel network fetching
    let mut seen_fetch_urls = std::collections::HashSet::new();
    let mut unique_results = Vec::new();
    for item in search_results {
        let norm = normalize_url_for_dedup(&item.url);
        if seen_fetch_urls.insert(norm) {
            unique_results.push(item);
        }
    }

    let mut join_set = tokio::task::JoinSet::new();

    for (idx, item) in unique_results.into_iter().enumerate() {
        let rank = idx + 1;
        let item_url = item.url.clone();
        let item_title = item.title.clone();
        let item_snippet = item.snippet.clone();
        let item_engine = item.engine.clone();
        let opts = page_options.clone();
        let eng = engine.clone();

        join_set.spawn(async move {
            match eng.process_url(&item_url, &opts).await {
                Ok(doc) => SearchFetchItem {
                    error_details: None,
                    extraction: Some(doc.extraction),
                    warnings: doc.warnings,
                    rank,
                    title: if item_title.is_empty() {
                        doc.title.unwrap_or_else(|| "网页内容".into())
                    } else {
                        item_title
                    },
                    url: item_url,
                    snippet: item_snippet,
                    engine: item_engine,
                    content: Some(doc.content),
                    token_count: doc.token_count,
                    status: doc.status,
                    error: None,
                },
                Err(err) => {
                    warn!(url = %item_url, error = %err, "Failed to fetch search result item content");
                    SearchFetchItem {
                    error_details: Some(err.details()),
                    extraction: None,
                    warnings: Vec::new(),
                        rank,
                        title: item_title,
                        url: item_url,
                        snippet: item_snippet,
                        engine: item_engine,
                        content: None,
                        token_count: 0,
                        status: 0,
                        error: Some(err.to_string()),
                    }
                }
            }
        });
    }

    let mut fetched_items = Vec::new();
    while let Some(res) = join_set.join_next().await {
        if let Ok(item) = res {
            fetched_items.push(item);
        }
    }

    // Sort by original search rank to preserve ranking order
    fetched_items.sort_by_key(|item| item.rank);

    if dedup {
        let mut seen_simhashes: Vec<u64> = Vec::new();
        let mut deduplicated = Vec::new();
        for item in fetched_items {
            if let Some(ref text) = item.content {
                let h = crate::dedup::simhash(text);
                if seen_simhashes
                    .iter()
                    .any(|&prev| crate::dedup::is_near_duplicate(prev, h, 3))
                {
                    debug!(url = %item.url, "Filtered out near-duplicate search result via SimHash");
                    continue;
                }
                seen_simhashes.push(h);
            }
            deduplicated.push(item);
        }
        return Ok(deduplicated);
    }

    Ok(fetched_items)
}

fn apply_search_scope(query: &str, scope: Option<&str>) -> Result<String, TklessError> {
    let domain = match scope {
        None | Some("all") => None,
        Some("wechat") => Some("mp.weixin.qq.com"),
        Some("baike") => Some("baike.baidu.com"),
        Some("zhihu") => Some("zhihu.com"),
        Some("github") => Some("github.com"),
        Some("juejin") => Some("juejin.cn"),
        Some("csdn") => Some("blog.csdn.net"),
        Some("medium") => Some("medium.com"),
        Some("substack") => Some("substack.com"),
        Some("arxiv") => Some("arxiv.org"),
        Some("huggingface") => Some("huggingface.co"),
        Some("wikipedia") => Some("wikipedia.org"),
        Some("reddit") => Some("reddit.com"),
        Some("stackoverflow") => Some("stackoverflow.com"),
        Some("v2ex") => Some("v2ex.com"),
        Some(_) => {
            return Err(TklessError::InvalidArgument(
                "Unsupported search scope".into(),
            ))
        }
    };
    Ok(match domain {
        Some(domain) if !query.is_empty() && tokenize_query(query).domain_filters.is_empty() => {
            format!("site:{domain} {query}")
        }
        _ => query.to_owned(),
    })
}

fn matches_search_domains(raw_url: &str, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let Ok(url) = url::Url::parse(raw_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    filters.iter().any(|filter| {
        let absolute = if filter.contains("://") {
            filter.clone()
        } else {
            format!("https://{filter}")
        };
        let Ok(filter_url) = url::Url::parse(&absolute) else {
            return false;
        };
        let Some(domain) = filter_url.host_str() else {
            return false;
        };
        let prefix = filter_url.path().trim_end_matches('/');
        crate::fetcher::is_subdomain_or_exact(host, domain)
            && (prefix.is_empty()
                || url.path() == prefix
                || url.path().starts_with(&format!("{prefix}/")))
    })
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    #[test]
    fn platform_domains_are_enforced_before_limit_and_fallback_scoring() {
        let urls = [
            "https://tokio.rs/",
            "https://evilgithub.com/tokio",
            "https://github.com.evil.example/tokio",
            "https://example.com/?next=github.com",
            "https://github.com@evil.example/tokio",
            "https://github.com/tokio-rs/tokio",
            "https://docs.github.com/tokio",
        ];
        let html = urls.iter().map(|url| format!("<li class='b_algo'><h2><a href='{url}'>Tokio</a></h2><div class='b_caption'><p>Tokio Rust runtime</p></div></li>")).collect::<String>();
        let results = parse_bing_html_with_query(&html, "site:github.com Tokio", 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://github.com/tokio-rs/tokio");
        assert!(matches_search_domains(
            "https://docs.github.com/tokio",
            &["github.com".into()]
        ));
        assert!(!matches_search_domains(
            "https://github.com/tokio-rs-fake/repo",
            &["github.com/tokio-rs".into()]
        ));
        assert!(matches_search_domains(
            "https://github.com/tokio-rs/tokio",
            &["github.com/tokio-rs".into()]
        ));
    }
    #[test]
    fn all_platform_filters_share_the_same_bing_query_logic() {
        for (scope, domain) in [
            ("wechat", "mp.weixin.qq.com"),
            ("baike", "baike.baidu.com"),
            ("github", "github.com"),
            ("zhihu", "zhihu.com"),
            ("juejin", "juejin.cn"),
            ("csdn", "blog.csdn.net"),
            ("medium", "medium.com"),
            ("substack", "substack.com"),
            ("arxiv", "arxiv.org"),
            ("huggingface", "huggingface.co"),
            ("wikipedia", "wikipedia.org"),
            ("reddit", "reddit.com"),
            ("stackoverflow", "stackoverflow.com"),
            ("v2ex", "v2ex.com"),
        ] {
            assert_eq!(
                apply_search_scope("Rust", Some(scope)).unwrap(),
                format!("site:{domain} Rust")
            );
        }
        assert_eq!(
            apply_search_scope("site:example.com Rust", Some("github")).unwrap(),
            "site:example.com Rust"
        );
        assert_eq!(apply_search_scope("", Some("github")).unwrap(), "");
        assert!(apply_search_scope("Rust", Some("unknown")).is_err());
    }
}

/// Tokenized query elements separated into core entity tokens, temporal tokens, domain filters, and exact phrases.
#[derive(Debug, Clone, Default)]
pub struct QueryTokens {
    /// Non-temporal core content tokens (e.g. "特斯拉", "上海工厂", "产量")
    pub core_tokens: Vec<String>,
    /// Temporal modifier tokens (e.g. "2026", "2025")
    pub temporal_tokens: Vec<String>,
    /// All raw tokens
    pub all_tokens: Vec<String>,
    /// Extracted domain constraints (e.g. "zhuanlan.zhihu.com", "zhihu.com")
    pub domain_filters: Vec<String>,
    /// Exact phrase constraints extracted from quotes (e.g. "small language model")
    pub phrase_filters: Vec<String>,
}

/// Tokenizes a user query into core semantic tokens, temporal modifiers, domain filters, and exact phrases.
static STOPWORDS: &[&str] = &[
    "vs", "and", "or", "in", "on", "at", "to", "for", "of", "with", "by", "from", "the", "a", "an",
    "is", "it", "as", "be", "are", "was", "were", "that", "this",
];

pub fn tokenize_query(query: &str) -> QueryTokens {
    let mut phrase_filters = Vec::new();

    // 1. Extract quoted phrases ("..." or “...”)
    let mut in_quote = false;
    let mut current_phrase = String::new();
    for c in query.chars() {
        if c == '"' || c == '“' || c == '”' {
            if in_quote {
                let trimmed = current_phrase.trim().to_lowercase();
                if !trimmed.is_empty() {
                    phrase_filters.push(trimmed);
                }
                current_phrase.clear();
                in_quote = false;
            } else {
                in_quote = true;
            }
        } else if in_quote {
            current_phrase.push(c);
        }
    }

    let mut core_tokens = Vec::new();
    let mut temporal_tokens = Vec::new();
    let mut all_tokens = Vec::new();
    let mut domain_filters = Vec::new();

    let raw_tokens: Vec<&str> = query.split_whitespace().collect();

    for raw_t in raw_tokens {
        // Strip quotation marks and punctuation edges
        let t = raw_t.trim_matches(|c: char| {
            c == '"'
                || c == '\''
                || c == '“'
                || c == '”'
                || c == '‘'
                || c == '’'
                || c == '('
                || c == ')'
                || c == '['
                || c == ']'
                || c == ','
                || c == ';'
        });
        if t.is_empty() {
            continue;
        }

        let lower = t.to_lowercase();
        all_tokens.push(lower.clone());

        // Only domain operators constrain URL hosts. Preserve case-sensitive path prefixes.
        if let Some(prefix) = ["site:", "site：", "domain:"]
            .into_iter()
            .find(|prefix| lower.starts_with(prefix))
        {
            let domain = &t[prefix.len()..];
            let clean_domain = domain
                .trim()
                .trim_start_matches("http://")
                .trim_start_matches("https://");
            if !clean_domain.is_empty() {
                domain_filters.push(clean_domain.to_string());
            }
            continue; // Do not add search operator to core_tokens!
        }

        // Exclusions and other Bing operators must not become positive relevance terms.
        if t.starts_with('-')
            || ["inurl:", "intitle:", "filetype:"]
                .iter()
                .any(|prefix| lower.starts_with(prefix))
        {
            continue;
        }

        let t = lower.as_str();

        // Skip English stopwords for core token matching
        if STOPWORDS.contains(&t) {
            continue;
        }

        // Check if 4-digit year (e.g. 2024, 2025, 2026, 2027) or year prefix (e.g. 2026年).
        // Work on character boundaries: `t[..4]` can split a multi-byte CJK
        // character (for example the query `20年`) and panic.
        if t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit()) {
            temporal_tokens.push(t.to_string());
        } else if t.len() > 4 && (t.starts_with("19") || t.starts_with("20")) {
            let first_four: String = t.chars().take(4).collect();
            if first_four.len() == 4 && first_four.bytes().all(|b| b.is_ascii_digit()) {
                temporal_tokens.push(first_four);
                let boundary = t
                    .char_indices()
                    .nth(4)
                    .map(|(index, _)| index)
                    .unwrap_or(t.len());
                let rem = t[boundary..].trim_start_matches('年').trim();
                if !rem.is_empty() {
                    core_tokens.push(rem.to_string());
                    if rem.chars().any(|c| (c as u32) >= 0x4E00) && rem.chars().count() >= 4 {
                        let chars: Vec<char> = rem.chars().collect();
                        let half = chars.len() / 2;
                        let s1: String = chars[..half].iter().collect();
                        let s2: String = chars[half..].iter().collect();
                        if !core_tokens.contains(&s1) {
                            core_tokens.push(s1);
                        }
                        if !core_tokens.contains(&s2) {
                            core_tokens.push(s2);
                        }
                    }
                }
            } else {
                core_tokens.push(t.to_string());
            }
        } else {
            core_tokens.push(t.to_string());
            if t.chars().any(|c| (c as u32) >= 0x4E00) && t.chars().count() >= 4 {
                let chars: Vec<char> = t.chars().collect();
                let half = chars.len() / 2;
                let s1: String = chars[..half].iter().collect();
                let s2: String = chars[half..].iter().collect();
                if !core_tokens.contains(&s1) {
                    core_tokens.push(s1);
                }
                if !core_tokens.contains(&s2) {
                    core_tokens.push(s2);
                }
            }
        }
    }

    QueryTokens {
        core_tokens,
        temporal_tokens,
        all_tokens,
        domain_filters,
        phrase_filters,
    }
}

static AD_CONTAINER_CLASSES: &[&str] = &[
    "b_ad",
    "b_algo_ad",
    "b_promoted",
    "b_sponsored",
    "b_adbottom",
    "b_adtop",
    "b_adtopcontent",
    "b_ans_ad",
    "ads",
];

static PROMO_DOMAINS: &[&str] = &[
    "taobao.com",
    "douyin.com",
    "taptap.cn",
    "alipan.com",
    "pinduoduo.com",
    "yangkeduo.com",
    "poki.com",
    "poki.cn",
    "youtube.com",
    "tiktok.com",
    "missav",
    "kohi-den",
    "e621.net",
    "patreon.com",
    "52pojie.cn",
];

/// Identifies whether a Bing SERP element is a commercial advertisement or disguised promotional card.
pub fn is_bing_ad_or_promotion(
    element: &scraper::ElementRef,
    title: &str,
    _snippet: &str,
    resolved_url: &str,
    query_tokens: &QueryTokens,
) -> bool {
    // 1. Check element class attribute
    if let Some(cls) = element.value().attr("class") {
        let cls_lower = cls.to_lowercase();
        for ad_cls in AD_CONTAINER_CLASSES {
            if cls_lower.contains(ad_cls) {
                return true;
            }
        }
    }

    // 2. Check inner elements for ad indicators (b_adlabel, ad_label, etc.)
    let inner_html = element.html().to_lowercase();
    if inner_html.contains("b_adlabel")
        || inner_html.contains("ad_label")
        || inner_html.contains("class=\"b_ad\"")
        || inner_html.contains("class=\"ad\"")
        || inner_html.contains("class=\"ads\"")
    {
        return true;
    }

    // 3. Check for text prefixes explicitly indicating advertising
    let trimmed_title = title.trim();
    if trimmed_title.starts_with("广告")
        || trimmed_title.starts_with("推广")
        || trimmed_title.starts_with("赞助")
        || trimmed_title.starts_with("Ad ")
        || trimmed_title.starts_with("Sponsored")
    {
        return true;
    }

    // 4. Commercial promo landing domains heuristic
    // If the URL is a commercial landing domain and the user's query did not mention that domain/brand,
    // and neither title nor snippet contains any core query tokens, it is a disguised promotion.
    let url_lower = resolved_url.to_lowercase();
    for promo_domain in PROMO_DOMAINS {
        if url_lower.contains(promo_domain) {
            let query_mentions_domain = query_tokens.all_tokens.iter().any(|t| {
                promo_domain.contains(t)
                    || (t.contains("淘宝") && *promo_domain == "taobao.com")
                    || (t.contains("抖音") && *promo_domain == "douyin.com")
                    || (t.contains("阿里") && *promo_domain == "alipan.com")
                    || (t.contains("taptap") && *promo_domain == "taptap.cn")
            });
            if !query_mentions_domain {
                return true;
            }
        }
    }

    // 5. Local store / offline storefront cards heuristic (e.g. "XX餐厅 (蓝色港湾店)", "XX酒店 (国贸店)")
    if (trimmed_title.contains("店）") || trimmed_title.contains("店)"))
        && !query_tokens.all_tokens.iter().any(|t| t.contains("店"))
    {
        return true;
    }

    false
}

/// Checks if token matches inside haystack, enforcing word boundaries for short ASCII tokens (<= 3 chars).
pub fn is_token_matched(haystack: &str, token: &str) -> bool {
    if token.is_empty() || haystack.is_empty() {
        return false;
    }
    let h_lower = haystack.to_lowercase();
    let t_lower = token.to_lowercase();

    if token.is_ascii() && token.len() <= 3 {
        let bytes = h_lower.as_bytes();
        let t_bytes = t_lower.as_bytes();
        let t_len = t_bytes.len();

        let mut start = 0;
        while let Some(pos) = h_lower[start..].find(&t_lower) {
            let abs_pos = start + pos;
            let left_ok = abs_pos == 0 || !bytes[abs_pos - 1].is_ascii_alphanumeric();
            let right_ok =
                abs_pos + t_len >= bytes.len() || !bytes[abs_pos + t_len].is_ascii_alphanumeric();
            if left_ok && right_ok {
                return true;
            }
            start = abs_pos + 1;
            if start >= h_lower.len() {
                break;
            }
        }
        false
    } else {
        h_lower.contains(&t_lower)
    }
}

/// Returns variations and stems of a word (e.g. "troubleshooting" -> ["troubleshooting", "troubleshoot"]).
pub fn get_word_stems(token: &str) -> Vec<&str> {
    let mut stems = vec![token];
    if token.len() >= 6 && token.is_ascii() {
        if let Some(s) = token.strip_suffix("ing") {
            if s.len() >= 3 {
                stems.push(s);
            }
        }
        if let Some(s) = token.strip_suffix("tion") {
            if s.len() >= 3 {
                stems.push(s);
            }
        }
        if let Some(s) = token.strip_suffix("es") {
            if s.len() >= 3 {
                stems.push(s);
            }
        } else if let Some(s) = token.strip_suffix('s') {
            if s.len() >= 3 {
                stems.push(s);
            }
        }
        if let Some(s) = token.strip_suffix("ed") {
            if s.len() >= 3 {
                stems.push(s);
            }
        }
    }
    stems
}

/// Evaluates query relevance and scores a candidate result with full URL and domain awareness.
/// Returns None if the candidate is completely unrelated or a severe first-token-only fallback.
pub fn calculate_relevance_full(
    title: &str,
    snippet: &str,
    resolved_url: &str,
    query_tokens: &QueryTokens,
) -> Option<i32> {
    if query_tokens.core_tokens.is_empty() && query_tokens.phrase_filters.is_empty() {
        return Some(10);
    }

    let title_lower = title.to_lowercase();
    let snippet_lower = snippet.to_lowercase();
    let url_lower = resolved_url.to_lowercase();

    // Compact forms with spaces and separators removed for entity folding (e.g. "hugging face" == "huggingface")
    let title_compact = title_lower.replace([' ', '-', '_', '·', '|', '—'], "");
    let snippet_compact = snippet_lower.replace([' ', '-', '_', '·', '|', '—'], "");
    let url_compact = url_lower.replace([' ', '-', '_', '/', '.'], "");

    let mut score = 0;
    let mut domain_matched = false;

    // 1. Domain filter matching (e.g. site:zhuanlan.zhihu.com or site:zhihu.com)
    if !query_tokens.domain_filters.is_empty() {
        for df in &query_tokens.domain_filters {
            if url_lower.contains(df) {
                domain_matched = true;
                score += 35;
                break;
            }
        }
    }

    // 2. Exact phrase matching (e.g. "small language model")
    for phrase in &query_tokens.phrase_filters {
        let phrase_compact = phrase.replace(' ', "");
        if title_lower.contains(phrase) || title_compact.contains(&phrase_compact) {
            score += 30;
        } else if snippet_lower.contains(phrase) || snippet_compact.contains(&phrase_compact) {
            score += 15;
        }
    }

    // 3. Core token matching
    let mut matched_core = Vec::new();
    for (idx, token) in query_tokens.core_tokens.iter().enumerate() {
        let token_compact = token.replace([' ', '-', '_'], "");
        let stems = get_word_stems(token);
        let in_title = stems.iter().any(|st| is_token_matched(title, st))
            || (!token_compact.is_empty() && title_compact.contains(&token_compact));
        let in_snippet = stems.iter().any(|st| is_token_matched(snippet, st))
            || (!token_compact.is_empty() && snippet_compact.contains(&token_compact));
        let in_url = !resolved_url.is_empty()
            && (stems.iter().any(|st| is_token_matched(resolved_url, st))
                || (!token_compact.is_empty()
                    && token.len() >= 4
                    && url_compact.contains(&token_compact)));

        if in_title || in_snippet || in_url {
            matched_core.push(idx);
            if in_title {
                score += 20;
            }
            if in_snippet {
                score += 8;
            }
            if in_url {
                score += 10;
            }
        } else if token.is_ascii() && token.len() >= 5 {
            // Partial prefix matching for technical terms (e.g. "smollm" in "smollm2" or vice versa)
            let prefix = &token[..token.len() - 1];
            if title_lower.contains(prefix)
                || snippet_lower.contains(prefix)
                || url_lower.contains(prefix)
            {
                matched_core.push(idx);
                score += 8;
            }
        } else if token.chars().any(|c| (c as u32) >= 0x4E00) && token.chars().count() >= 3 {
            // CJK compound word n-gram matching
            let chars: Vec<char> = token.chars().collect();
            let mut cjk_sub_matches = 0;
            for window in chars.windows(2) {
                let bi_gram: String = window.iter().collect();
                if title_lower.contains(&bi_gram) || snippet_lower.contains(&bi_gram) {
                    cjk_sub_matches += 1;
                }
            }
            if cjk_sub_matches >= 2 || (chars.len() <= 3 && cjk_sub_matches >= 1) {
                matched_core.push(idx);
                score += 15 + cjk_sub_matches * 3;
            }
        }
    }

    // Hard Filter 1: Zero Core Tokens Matched AND No Domain Matched AND No Phrase Matched
    if matched_core.is_empty() && !domain_matched && score == 0 {
        return None;
    }

    // Hard Filter 2: First-Token-Only Fallback (Bing Term Drop)
    // If user asked a multi-token query (>=2 core tokens), but the result ONLY matches token 0,
    // and matches NONE of the other 1..N core tokens (e.g. "Python asyncio", "Rust vs Go", "北京 小学 入学 政策"),
    // reject this term-drop casualty so Stage 1d can retry with quoted discriminating keywords.
    if query_tokens.core_tokens.len() >= 2 && !domain_matched && matched_core == vec![0] {
        return None;
    }

    // Check temporal tokens (e.g. 2026, 2025)
    for year in &query_tokens.temporal_tokens {
        if title_lower.contains(year) {
            score += 12;
        } else if snippet_lower.contains(year) {
            score += 4;
        }
    }

    Some(score)
}

/// Evaluates relaxed query relevance for graceful fallback when strict gatekeeping yields 0 results.
pub fn calculate_relaxed_relevance(
    title: &str,
    snippet: &str,
    resolved_url: &str,
    query_tokens: &QueryTokens,
) -> Option<i32> {
    if query_tokens.core_tokens.is_empty() && query_tokens.phrase_filters.is_empty() {
        return Some(5);
    }

    let title_lower = title.to_lowercase();
    let snippet_lower = snippet.to_lowercase();
    let url_lower = resolved_url.to_lowercase();

    let title_compact = title_lower.replace([' ', '-', '_', '·', '|', '—'], "");
    let snippet_compact = snippet_lower.replace([' ', '-', '_', '·', '|', '—'], "");
    let url_compact = url_lower.replace([' ', '-', '_', '/', '.'], "");

    let mut score = 0;

    // Domain filter matching
    if !query_tokens.domain_filters.is_empty() {
        for df in &query_tokens.domain_filters {
            if url_lower.contains(df) {
                score += 35;
                break;
            }
        }
    }

    // Exact phrase matching
    for phrase in &query_tokens.phrase_filters {
        let phrase_compact = phrase.replace(' ', "");
        if title_lower.contains(phrase) || title_compact.contains(&phrase_compact) {
            score += 25;
        } else if snippet_lower.contains(phrase) || snippet_compact.contains(&phrase_compact) {
            score += 10;
        }
    }

    // Core tokens matching (at least 1 required)
    let mut any_match = false;
    for token in &query_tokens.core_tokens {
        let token_compact = token.replace([' ', '-', '_'], "");
        let stems = get_word_stems(token);
        let in_title = stems.iter().any(|st| is_token_matched(title, st))
            || (!token_compact.is_empty() && title_compact.contains(&token_compact));
        let in_snippet = stems.iter().any(|st| is_token_matched(snippet, st))
            || (!token_compact.is_empty() && snippet_compact.contains(&token_compact));
        let in_url = !resolved_url.is_empty()
            && (stems.iter().any(|st| is_token_matched(resolved_url, st))
                || (!token_compact.is_empty()
                    && token.len() >= 4
                    && url_compact.contains(&token_compact)));

        if in_title || in_snippet || in_url {
            any_match = true;
            if in_title {
                score += 15;
            }
            if in_snippet {
                score += 6;
            }
            if in_url {
                score += 8;
            }
        } else if token.is_ascii() && token.len() >= 5 {
            let prefix = &token[..token.len() - 1];
            if title_lower.contains(prefix) || snippet_lower.contains(prefix) {
                any_match = true;
                score += 5;
            }
        } else if token.chars().any(|c| (c as u32) >= 0x4E00) && token.chars().count() >= 3 {
            let chars: Vec<char> = token.chars().collect();
            let mut cjk_sub_matches = 0;
            for window in chars.windows(2) {
                let bi_gram: String = window.iter().collect();
                if title_lower.contains(&bi_gram) || snippet_lower.contains(&bi_gram) {
                    cjk_sub_matches += 1;
                }
            }
            if cjk_sub_matches >= 1 {
                any_match = true;
                score += 8 + cjk_sub_matches * 2;
            }
        }
    }

    if !any_match && score == 0 {
        return None;
    }

    // Check temporal tokens
    for year in &query_tokens.temporal_tokens {
        if title_lower.contains(year) {
            score += 8;
        } else if snippet_lower.contains(year) {
            score += 3;
        }
    }

    Some(score)
}

struct ScoredBingItem {
    item: SearchResultItem,
    score: i32,
}

async fn prepare_search_transport(
    client: &reqwest::Client,
    fetcher: &crate::fetcher::Fetcher,
    origin: &str,
    configured: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
) -> Result<Option<crate::cdp::CdpOptions>, TklessError> {
    if let Some(options) = configured {
        return Ok(Some(options.clone()));
    }
    match crate::search_preflight::http(client, origin, Some(fetcher.search_preflight())).await {
        Ok(()) => Ok(None),
        Err(TklessError::SearchConnectionError(_)) if auto_cdp => {
            warn!(
                origin,
                "HTTP connection preflight failed; trying the browser's network path"
            );
            Ok(Some(crate::cdp::CdpOptions {
                allow_private_addresses: fetcher.options().allow_private_addresses,
                endpoint: crate::cdp::resolve_cdp_endpoint(None, true)
                    .unwrap_or_else(|| "http://127.0.0.1:9222".into()),
                ..Default::default()
            }))
        }
        Err(error) => Err(error),
    }
}

/// Canonical search API. Operators, dates and punctuation are preserved.
pub async fn web_search(
    engine: &TklessEngine,
    query: &str,
    options: &SearchOptions,
) -> Result<SearchReport, TklessError> {
    let effective_query = apply_search_scope(query.trim(), options.scope.as_deref())?;
    options.filters.validate()?;
    let mut report = search_bing_report_impl(
        engine.client(),
        engine.fetcher(),
        &effective_query,
        options.limit,
        options.cdp.as_ref(),
        options.auto_cdp,
        &options.filters,
    )
    .await?;
    if !options.filters.is_empty() {
        report.warnings.push(crate::model::DiagnosticWarning::new(
            "search_filters_best_effort",
            "Date, language and region filters rely on undocumented Bing parameters; upstream may ignore them. Verify results before relying on them.",
        ));
    }
    report.query = query.trim().into();
    Ok(report)
}

async fn search_bing_report_impl(
    client: &reqwest::Client,
    fetcher: &crate::fetcher::Fetcher,
    query: &str,
    limit: usize,
    cdp_options: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
    filters: &SearchFilters,
) -> Result<SearchReport, TklessError> {
    if limit == 0 {
        return Err(TklessError::InvalidArgument(
            "search limit must be positive".into(),
        ));
    }
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Err(TklessError::InvalidArgument(
            "search query must not be empty".into(),
        ));
    }

    let prepared = prepare_search_transport(
        client,
        fetcher,
        "https://www.bing.com",
        cdp_options,
        auto_cdp,
    )
    .await?;
    let cdp_options = prepared.as_ref();
    if let Some(options) = cdp_options {
        let endpoint = crate::cdp::ensure_cdp_daemon_ready(&options.endpoint).await?;
        crate::cdp::preflight_search(&endpoint, "https://www.bing.com").await?;
    }

    let outcome = fetch_and_parse_bing(
        fetcher,
        clean_query,
        clean_query,
        limit,
        cdp_options,
        auto_cdp,
        filters,
    )
    .await;
    if cdp_options.is_none() {
        if let Err(error) = &outcome {
            fetcher
                .search_preflight()
                .observe_error("https://www.bing.com", error)
                .await;
        }
    }
    outcome
}

fn bing_search_url_with(query: &str, filters: &SearchFilters) -> Result<String, TklessError> {
    let mut url = url::Url::parse("https://www.bing.com/search").expect("static Bing URL");
    url.query_pairs_mut().append_pair("q", query);
    filters.apply(&mut url)?;
    Ok(url.into())
}

fn check_bing_http_status(status: u16, html: &str, url: &str) -> Result<(), TklessError> {
    if !(200..300).contains(&status)
        && !is_bing_verification_page(&Html::parse_document(html), html)
    {
        return Err(TklessError::HttpStatusError {
            status,
            url: url.into(),
        });
    }
    Ok(())
}

fn is_bing_verification_page(doc: &Html, html: &str) -> bool {
    // Search snippets and scripts may mention verification; readable result items take priority.
    let has_results = doc.select(&BING_ITEM_SELECTOR).any(|item| {
        item.select(&BING_TITLE_SELECTOR).any(|title| {
            title.value().attr("href").is_some_and(|href| {
                url::Url::parse(href).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
            })
        })
    });
    !has_results && crate::cdp::is_anti_scraping_challenge(html, 200)
}

async fn fetch_and_parse_bing(
    fetcher: &crate::fetcher::Fetcher,
    search_query: &str,
    relevance_query: &str,
    limit: usize,
    cdp_options: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
    filters: &SearchFilters,
) -> Result<SearchReport, TklessError> {
    let client = fetcher.client();
    let search_url = bing_search_url_with(search_query, filters)?;
    let mut report = SearchReport::new(search_query);

    debug!(query = search_query, url = %search_url, "Executing Bing web search");

    // If explicit CDP options provided, fetch directly via headless Chrome
    if let Some(cdp_opts) = cdp_options {
        let html = crate::cdp::fetch_html_via_cdp(&search_url, cdp_opts).await?;
        let page = parse_bing_page(&html, relevance_query, limit)?;
        report.append(page, 1, "cdp", limit);
        return Ok(report);
    }

    let is_cjk = search_query.chars().any(|c| (c as u32) >= 0x4E00);
    // Use worldwide market en-ww instead of zh-cn to avoid severely crippled regional term drop
    let cookie_val = if is_cjk {
        "SRCHD=AF=NOFORM; SRCHHPGUSR=SRCHLANG=zh-Hans; _EDGE_S=mkt=en-ww; _SS=SID=1;"
    } else {
        "SRCHD=AF=NOFORM; SRCHHPGUSR=SRCHLANG=en; _EDGE_S=mkt=en-us; _SS=SID=1;"
    };
    let accept_lang = if is_cjk {
        "zh-CN,zh;q=0.9,en;q=0.8"
    } else {
        "en-US,en;q=0.9,zh-CN;q=0.8"
    };

    let resp = client
        .get(&search_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 Edg/133.0.0.0",
        )
        .header("Referer", "https://www.bing.com/")
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
        .header("Accept-Language", accept_lang)
        .header("Cookie", cookie_val)
        .header(
            "Sec-Ch-Ua",
            "\"Not(A:Brand\";v=\"99\", \"Microsoft Edge\";v=\"133\", \"Chromium\";v=\"133\"",
        )
        .header("Sec-Ch-Ua-Mobile", "?0")
        .header("Sec-Ch-Ua-Platform", "\"Windows\"")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "same-origin")
        .header("Sec-Fetch-User", "?1")
        .header("Upgrade-Insecure-Requests", "1")
        .send()
        .await?;

    let mut page2_cookie = cookie_val.to_string();
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(crate::search_preflight::rate_limit_error(
            "https://www.bing.com",
            resp.headers(),
        ));
    }
    for val in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        if let Ok(s) = val.to_str() {
            if let Some(cookie_pair) = s.split(';').next() {
                page2_cookie.push_str("; ");
                page2_cookie.push_str(cookie_pair.trim());
            }
        }
    }

    let status = resp.status();
    let html = fetcher.read_text(resp).await?;
    check_bing_http_status(status.as_u16(), &html, &search_url)?;
    let mut transport = "http";
    let initial_page = match parse_bing_page(&html, relevance_query, limit) {
        Err(TklessError::SearchAntiScraping(ref err_msg)) if auto_cdp => {
            warn!(
                error = %err_msg,
                "Encountered Bing anti-scraping challenge, auto-falling back to CDP browser rendering"
            );
            let fallback_cdp = crate::cdp::CdpOptions {
                allow_private_addresses: fetcher.options().allow_private_addresses,
                endpoint: crate::cdp::resolve_cdp_endpoint(None, true)
                    .unwrap_or_else(|| "http://127.0.0.1:9222".into()),
                wait_ms: Some(1500),
                timeout_ms: Some(30_000),
                ..Default::default()
            };
            let endpoint = crate::cdp::ensure_cdp_daemon_ready(&fallback_cdp.endpoint).await?;
            crate::cdp::preflight_search(&endpoint, "https://www.bing.com").await?;
            let cdp_html = crate::cdp::fetch_html_via_cdp(&search_url, &fallback_cdp).await?;
            transport = "cdp";
            report.warnings.push(crate::model::DiagnosticWarning::new(
                "http_challenge_fallback",
                "HTTP search required verification; results were obtained through CDP",
            ));
            parse_bing_page(&cdp_html, relevance_query, limit)?
        }
        other => other?,
    };

    report.append(initial_page, 1, transport, limit);

    // Pagination support: if user requested limit > 10 and we have >= 8 results on page 1, fetch page 2 (&first=11 with FPIG token)
    if limit > 10 && report.results.len() >= 8 && transport == "http" {
        let page2_url = if let Some(cap) = BING_NEXT_PAGE_RE.captures(&html) {
            let rel = cap[1].replace("&amp;", "&");
            format!("https://www.bing.com{rel}")
        } else {
            format!("{search_url}&first=11")
        };
        debug!(url = %page2_url, "Fetching Bing page 2 for pagination");
        let page2 = client
            .get(&page2_url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 Edg/133.0.0.0",
            )
            .header("Referer", "https://www.bing.com/")
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            )
            .header("Accept-Language", accept_lang)
            .header("Cookie", page2_cookie)
            .header(
                "Sec-Ch-Ua",
                "\"Not(A:Brand\";v=\"99\", \"Microsoft Edge\";v=\"133\", \"Chromium\";v=\"133\"",
            )
            .header("Sec-Ch-Ua-Mobile", "?0")
            .header("Sec-Ch-Ua-Platform", "\"Windows\"")
            .send()
            .await;
        match page2 {
            Ok(response) => {
                let status = response.status();
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    let error = crate::search_preflight::rate_limit_error(
                        "https://www.bing.com",
                        response.headers(),
                    );
                    return Err(error);
                }
                if !status.is_success() {
                    report.warnings.push(crate::model::DiagnosticWarning::new(
                        "pagination_http_error",
                        format!("Page 2 returned HTTP {status}; page 1 results retained"),
                    ));
                } else {
                    match fetcher.read_text(response).await {
                        Ok(html) => match parse_bing_page(&html, relevance_query, limit) {
                            Ok(page) => report.append(page, 2, "http", limit),
                            Err(error) => {
                                report.warnings.push(crate::model::DiagnosticWarning::new(
                                    "pagination_parse_error",
                                    error.to_string(),
                                ))
                            }
                        },
                        Err(error) => report.warnings.push(crate::model::DiagnosticWarning::new(
                            "pagination_read_error",
                            error.to_string(),
                        )),
                    }
                }
            }
            Err(error) => report.warnings.push(crate::model::DiagnosticWarning::new(
                "pagination_connection_error",
                error.to_string(),
            )),
        }
    }

    debug!(
        final_bing_count = report.results.len(),
        "fetch_and_parse_bing done"
    );
    Ok(report)
}

/// Parses Bing search results HTML into structured items with query-aware relevance gatekeeping.
pub fn parse_bing_html_with_query(
    html: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    Ok(parse_bing_page(html, query, limit)?.results)
}

fn parse_bing_page(html: &str, query: &str, limit: usize) -> Result<ParsedPage, TklessError> {
    let doc = Html::parse_document(html);
    // 0. Active Anti-Scraping / Captcha Challenge Detection
    if is_bing_verification_page(&doc, html) {
        return Err(TklessError::SearchAntiScraping(
            "Bing requires human verification; retry later or use an authorized browser session. Browser rendering does not guarantee verification succeeds.".into(),
        ));
    }

    let mut diagnostics = SearchPageDiagnostics::default();
    let mut scored_results: Vec<ScoredBingItem> = Vec::new();
    let mut fallback_results: Vec<ScoredBingItem> = Vec::new();
    let mut raw_non_ad_items: Vec<ScoredBingItem> = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    let query_tokens = tokenize_query(query);

    for element in doc.select(&BING_ITEM_SELECTOR) {
        diagnostics.candidates += 1;
        let title_el = match element.select(&BING_TITLE_SELECTOR).next() {
            Some(el) => el,
            None => {
                diagnostics.invalid += 1;
                continue;
            }
        };

        let raw_title = title_el
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string();
        let raw_href = title_el.value().attr("href").unwrap_or("").trim();

        if raw_title.is_empty() || raw_href.is_empty() {
            diagnostics.invalid += 1;
            continue;
        }

        // Filter Bing internal links or invalid protocols
        if !raw_href.starts_with("http://") && !raw_href.starts_with("https://") {
            diagnostics.invalid += 1;
            continue;
        }

        let resolved_url = unwrap_bing_url(raw_href);
        if !url::Url::parse(&resolved_url)
            .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        {
            diagnostics.invalid += 1;
            continue;
        }
        if !matches_search_domains(&resolved_url, &query_tokens.domain_filters) {
            diagnostics.domain_filtered += 1;
            continue;
        }
        if !seen_urls.insert(resolved_url.clone()) {
            diagnostics.duplicates += 1;
            continue;
        }

        let snippet = element
            .select(&BING_SNIPPET_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_default();

        // 1. Filter out ads and commercial promotion widgets
        if is_bing_ad_or_promotion(&element, &raw_title, &snippet, &resolved_url, &query_tokens) {
            diagnostics.ads_filtered += 1;
            continue;
        }

        raw_non_ad_items.push(ScoredBingItem {
            item: SearchResultItem {
                title: raw_title.clone(),
                url: resolved_url.clone(),
                snippet: snippet.clone(),
                engine: "bing".to_string(),
            },
            score: 1,
        });

        // 2. Strict evaluation (Tier 1)
        if let Some(score) =
            calculate_relevance_full(&raw_title, &snippet, &resolved_url, &query_tokens)
        {
            scored_results.push(ScoredBingItem {
                item: SearchResultItem {
                    title: raw_title.clone(),
                    url: resolved_url.clone(),
                    snippet: snippet.clone(),
                    engine: "bing".to_string(),
                },
                score,
            });
        }

        // 3. Relaxed evaluation (Tier 2 Fallback)
        if let Some(score) =
            calculate_relaxed_relevance(&raw_title, &snippet, &resolved_url, &query_tokens)
        {
            fallback_results.push(ScoredBingItem {
                item: SearchResultItem {
                    title: raw_title,
                    url: resolved_url,
                    snippet,
                    engine: "bing".to_string(),
                },
                score,
            });
        }
    }

    if diagnostics.candidates == 0 {
        let empty_selector =
            Selector::parse("#b_results .b_no, #b_results .b_noResults, #b_results .b_no_results")
                .unwrap();
        let explicit_empty = doc
            .select(&empty_selector)
            .any(|element| element.text().any(|text| !text.trim().is_empty()));
        if !explicit_empty {
            return Err(TklessError::SearchParseError("No recognized result items or explicit no-results marker; the page may be incomplete or its structure has changed".into()));
        }
    } else if diagnostics.invalid == diagnostics.candidates {
        return Err(TklessError::SearchParseError(
            "Result containers were found but no valid title/link could be parsed".into(),
        ));
    }
    diagnostics.strict_matches = scored_results.len();
    diagnostics.relaxed_matches = fallback_results.len();
    let mut final_scored = if !scored_results.is_empty() {
        diagnostics.selection = "strict".into();
        scored_results
    } else if !fallback_results.is_empty() {
        diagnostics.selection = "relaxed".into();
        fallback_results
    } else {
        diagnostics.selection = "unranked".into();
        raw_non_ad_items
    };
    if diagnostics.candidates == 0 {
        diagnostics.selection = "no_results".into();
    }
    diagnostics.selected = final_scored.len();

    // Sort by relevance score descending so the most relevant results come first
    final_scored.sort_by_key(|a| std::cmp::Reverse(a.score));

    let results: Vec<SearchResultItem> = final_scored
        .into_iter()
        .take(limit)
        .map(|sr| sr.item)
        .collect();

    diagnostics.returned = results.len();
    Ok(ParsedPage {
        results,
        diagnostics,
    })
}

/// Unwraps Bing tracking redirect URLs like `https://www.bing.com/ck/a?!&&...&u=a1aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw...`
pub fn unwrap_bing_url(raw_href: &str) -> String {
    if let Some(pos) = raw_href.find("&u=") {
        let after_u = &raw_href[pos + 3..];
        let encoded = after_u.split('&').next().unwrap_or("");
        // Bing ck URLs prefix base64 with "a1"
        let b64_part = if let Some(stripped) = encoded.strip_prefix("a1") {
            stripped
        } else {
            encoded
        };

        if let Some(decoded_bytes) = decode_base64_url_safe(b64_part) {
            if let Ok(decoded_str) = String::from_utf8(decoded_bytes) {
                if decoded_str.starts_with("http://") || decoded_str.starts_with("https://") {
                    return decoded_str;
                }
            }
        }
    }

    raw_href.to_string()
}

/// Decodes standard and URL-safe Base64 strings.
fn decode_base64_url_safe(input: &str) -> Option<Vec<u8>> {
    let clean = input.trim_end_matches('=');
    let mut bits = 0u32;
    let mut bit_count = 0;
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);

    for b in clean.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        bits = (bits << 6) | (val as u32);
        bit_count += 6;

        if bit_count >= 8 {
            bit_count -= 8;
            out.push((bits >> bit_count) as u8);
            bits &= (1 << bit_count) - 1;
        }
    }

    Some(out)
}

/// Normalizes a URL for deduplication for search results.
pub fn normalize_url_for_dedup(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw.trim()) else {
        return raw.trim().to_owned();
    };
    if url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
    if let Some(host) = url
        .host_str()
        .and_then(|host| host.strip_prefix("www."))
        .map(str::to_owned)
    {
        let _ = url.set_host(Some(&host));
    }
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    let value = url.to_string();
    if url.query().is_none() {
        value.trim_end_matches('/').to_owned()
    } else {
        value
    }
}

/// Formats search results as a clean GitHub Flavored Markdown document with engine source attribution.
pub fn format_search_results_markdown(query: &str, results: &[SearchResultItem]) -> String {
    let mut out = String::new();
    let engine_name = "bing";

    out.push_str(&format!("# 网页搜索 ({engine_name}): \"{query}\"\n\n"));

    if results.is_empty() {
        out.push_str("未返回搜索结果；结果数量可能受查询、平台范围和页面过滤影响。\n");
        out.push_str("> 建议操作：1. 简化关键词；2. 去除年份等强时效限定；3. 指定平台定向参数（如 --scope zhihu, --scope baike, --scope stackoverflow）。\n");
        return out;
    }

    for (idx, item) in results.iter().enumerate() {
        out.push_str(&format!("{}. [{}]({})\n", idx + 1, item.title, item.url));
        if !item.snippet.is_empty() {
            out.push_str(&format!("   > {}\n", item.snippet));
        }
        out.push('\n');
    }

    out
}

/// Formats combined search & fetch results into an integrated research report for LLMs.
pub fn format_search_fetch_markdown(query: &str, items: &[SearchFetchItem]) -> String {
    let mut out = String::new();
    let engine_name = "bing";

    let total_fetched = items.iter().filter(|i| i.content.is_some()).count();
    let total_tokens: usize = items.iter().map(|i| i.token_count).sum();

    out.push_str(&format!(
        "# 搜索与正文集成调研 ({engine_name}): \"{query}\"\n\n> 检索到 {} 条结果 | 成功提取 {} 篇正文 | 累计估算 Tokens: {}\n\n---\n\n",
        items.len(),
        total_fetched,
        total_tokens
    ));

    if items.is_empty() {
        out.push_str("未检索到相关网页结果。\n");
        return out;
    }

    for item in items {
        out.push_str(&format!(
            "## {}. [{}]({})\n\n",
            item.rank, item.title, item.url
        ));

        if !item.snippet.is_empty() {
            out.push_str(&format!("> **摘要**: {}\n\n", item.snippet));
        }

        if let Some(ref content) = item.content {
            out.push_str(&format!("### 正文速览 (Tokens: {}):\n\n", item.token_count));
            out.push_str(content.trim());
            out.push_str("\n\n---\n\n");
        } else if let Some(ref err) = item.error {
            out.push_str(&format!("> *[正文拉取异常: {err}]*\n\n---\n\n"));
        } else {
            out.push_str("> *[未获取到正文内容]*\n\n---\n\n");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bing_html_sample() {
        let sample = r#"
            <ol id="b_results">
                <li class="b_algo">
                    <h2><a href="https://www.rust-lang.org/">Rust Programming Language</a></h2>
                    <div class="b_caption">
                        <p>A language empowering everyone to build reliable and efficient software.</p>
                    </div>
                </li>
                <li class="b_algo">
                    <h2><a href="https://doc.rust-lang.org/book/">The Rust Programming Language - Book</a></h2>
                    <div class="b_caption">
                        <p>The Book is the primary official resource for learning Rust.</p>
                    </div>
                </li>
            </ol>
        "#;

        let results = parse_bing_html_with_query(sample, "", 5).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert!(results[0].snippet.contains("empowering everyone"));
        assert_eq!(results[0].engine, "bing");
    }

    #[test]
    fn test_unwrap_bing_url() {
        let bing_url =
            "https://www.bing.com/ck/a?!&&p=abc&u=a1aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw&ntb=1";
        let unwrapped = unwrap_bing_url(bing_url);
        assert_eq!(unwrapped, "https://rust-lang.org/");

        let direct_url = "https://example.com/rust";
        assert_eq!(unwrap_bing_url(direct_url), "https://example.com/rust");
    }

    #[test]
    fn test_format_search_fetch_markdown() {
        let items = vec![
            SearchFetchItem {
                error_details: None,
                extraction: None,
                warnings: Vec::new(),
                rank: 1,
                title: "Rust Guide".to_string(),
                url: "https://www.rust-lang.org/".to_string(),
                snippet: "A language empowering everyone".to_string(),
                engine: "bing".to_string(),
                content: Some("# Rust\nFast and reliable.".to_string()),
                token_count: 50,
                status: 200,
                error: None,
            },
            SearchFetchItem {
                error_details: None,
                extraction: None,
                warnings: Vec::new(),
                rank: 2,
                title: "Failed Page".to_string(),
                url: "https://example.com/404".to_string(),
                snippet: "Missing".to_string(),
                engine: "bing".to_string(),
                content: None,
                token_count: 0,
                status: 404,
                error: Some("HTTP error 404".to_string()),
            },
        ];

        let md = format_search_fetch_markdown("Rust", &items);
        assert!(md.contains("# 搜索与正文集成调研 (bing): \"Rust\""));
        assert!(md.contains("成功提取 1 篇正文"));
        assert!(md.contains("## 1. [Rust Guide](https://www.rust-lang.org/)"));
        assert!(md.contains("Fast and reliable."));
        assert!(md.contains("HTTP error 404"));
    }

    #[test]
    fn test_search_simhash_deduplication() {
        let text_orig = r#"
            TkLess is a high-performance, token-efficient web content extraction engine designed specifically
            for Large Language Models and AI Agents. It removes navigation bars, scripts, advertisements, and
            boilerplate using heuristic algorithms, converting the core content into LLM-optimized Markdown.
            Native adapters for Bilibili, WeChat, Zhihu, and GitHub are supported out of the box with zero overhead.
            By eliminating aggressive noise, TkLess delivers significant token savings across production AI pipelines.
        "#;
        let text_mirror = r#"
            TkLess is a high-performance, token-efficient web content extraction engine designed specifically
            for Large Language Models and AI Agents. It removes navigation bars, scripts, advertisements, and
            boilerplate using heuristic algorithms, converting the core content into LLM-optimized Markdown.
            Native adapters for Bilibili, WeChat, Zhihu, and GitHub are supported out of the box with zero overhead.
            By eliminating aggressive noise, TkLess delivers significant token savings across production AI pipelines.
            Reposted with permission.
        "#;
        let text_diff = "Tokio is an asynchronous runtime for the Rust programming language, providing event-driven non-blocking I/O.";

        let items = vec![
            SearchFetchItem {
                error_details: None,
                extraction: None,
                warnings: Vec::new(),
                rank: 1,
                title: "Original Post".to_string(),
                url: "https://example.com/original".to_string(),
                snippet: "Snippet 1".to_string(),
                engine: "bing".to_string(),
                content: Some(text_orig.to_string()),
                token_count: 50,
                status: 200,
                error: None,
            },
            SearchFetchItem {
                error_details: None,
                extraction: None,
                warnings: Vec::new(),
                rank: 2,
                title: "Mirror Site Copy".to_string(),
                url: "https://mirror.example/repost".to_string(),
                snippet: "Snippet 2".to_string(),
                engine: "bing".to_string(),
                content: Some(text_mirror.to_string()),
                token_count: 50,
                status: 200,
                error: None,
            },
            SearchFetchItem {
                error_details: None,
                extraction: None,
                warnings: Vec::new(),
                rank: 3,
                title: "Different Subject".to_string(),
                url: "https://tokio.rs/overview".to_string(),
                snippet: "Snippet 3".to_string(),
                engine: "bing".to_string(),
                content: Some(text_diff.to_string()),
                token_count: 40,
                status: 200,
                error: None,
            },
        ];

        let mut seen = Vec::new();
        let mut deduped = Vec::new();
        for item in items {
            if let Some(ref text) = item.content {
                let h = crate::dedup::simhash(text);
                if seen
                    .iter()
                    .any(|&prev| crate::dedup::is_near_duplicate(prev, h, 3))
                {
                    continue;
                }
                seen.push(h);
            }
            deduped.push(item);
        }

        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].url, "https://example.com/original");
        assert_eq!(deduped[1].url, "https://tokio.rs/overview");
    }

    #[test]
    fn test_tokenize_query_and_temporal() {
        let tokens = tokenize_query("北京 小学 入学 政策 2026");
        assert_eq!(tokens.core_tokens, vec!["北京", "小学", "入学", "政策"]);
        assert_eq!(tokens.temporal_tokens, vec!["2026"]);

        let tokens_en = tokenize_query("Tesla Model 3 Production 2025");
        assert_eq!(
            tokens_en.core_tokens,
            vec!["tesla", "model", "3", "production"]
        );
        assert_eq!(tokens_en.temporal_tokens, vec!["2025"]);
    }

    #[test]
    fn tokenize_query_never_slices_mid_character() {
        for query in ["20年", "20日", "19号", "2026年", "2026年政策"] {
            let tokens = tokenize_query(query);
            assert!(!tokens.all_tokens.is_empty(), "query: {query}");
        }

        let tokens = tokenize_query("2026年政策");
        assert!(tokens.temporal_tokens.contains(&"2026".to_string()));
        assert!(tokens.core_tokens.contains(&"政策".to_string()));

        let tokens = tokenize_query("20年");
        assert!(tokens.core_tokens.contains(&"20年".to_string()));
        assert!(tokens.temporal_tokens.is_empty());
    }

    #[test]
    fn test_relevance_first_token_drop_and_zero_relevance() {
        let query_tokens = tokenize_query("北京 小学 入学 政策 2026");

        // 1. First-token-only fallback (Term Drop): matches ONLY "北京", misses "小学", "入学", "政策"
        let fallback_title = "北京市 - 百度百科";
        let fallback_snip = "北京市，简称“京”，是中华人民共和国首都、直辖市、国家中心城市。";
        let fallback_score =
            calculate_relevance_full(fallback_title, fallback_snip, "", &query_tokens);
        assert!(
            fallback_score.is_none(),
            "First-token-only fallback (北京市百科) must be rejected"
        );

        // 2. Completely unrelated item (Zero Core Tokens matched, e.g. 阿里云盘 or 淘宝)
        let ad_title = "阿里云盘 - 备份您的照片与文件";
        let ad_snip = "不限速的高速云存储网盘，支持大文件快速上传下载。";
        let ad_score = calculate_relevance_full(ad_title, ad_snip, "", &query_tokens);
        assert!(
            ad_score.is_none(),
            "Completely unrelated promotion item must be rejected"
        );

        // 3. Genuine relevant result: matches "北京", "小学", "入学", "政策", "2026"
        let good_title = "2026年北京各区小学入学政策与信息采集指南公布";
        let good_snip =
            "北京市教委正式发布2026年义务教育阶段小学入学政策，明确报名流程与居住证要求。";
        let good_score = calculate_relevance_full(good_title, good_snip, "", &query_tokens);
        assert!(
            good_score.is_some(),
            "Genuine relevant policy article must be accepted"
        );
        assert!(good_score.unwrap() >= 40);
    }

    #[test]
    fn test_parse_bing_html_with_ad_and_relevance_filters() {
        let sample = r#"
            <ol id="b_results">
                <!-- 1. Disguised promotion / ad item -->
                <li class="b_algo b_ad">
                    <h2><a href="https://www.douyin.com/">抖音-记录美好生活</a></h2>
                    <div class="b_caption">
                        <p>抖音让每一个人看见并连接更大的世界，鼓励表达、沟通和记录。</p>
                    </div>
                </li>
                <!-- 2. Term Drop fallback: only matches "北京" -->
                <li class="b_algo">
                    <h2><a href="https://baike.baidu.com/item/%E5%8C%97%E4%BA%AC%E5%B8%82">北京市 - 百度百科</a></h2>
                    <div class="b_caption">
                        <p>北京市，简称“京”，是中华人民共和国首都、国家中心城市。</p>
                    </div>
                </li>
                <!-- 3. Genuine relevant result -->
                <li class="b_algo">
                    <h2><a href="https://jw.beijing.gov.cn/policy/2026">2026年北京小学入学政策官方解答</a></h2>
                    <div class="b_caption">
                        <p>北京市教委发布2026小学入学政策，详述幼升小网上信息采集时间与入学顺位要求。</p>
                    </div>
                </li>
            </ol>
        "#;

        let query = "北京 小学 入学 政策 2026";
        let results = parse_bing_html_with_query(sample, query, 5).unwrap();

        // Must filter out the Douyin ad and the Beijing encyclopedia fallback
        assert_eq!(
            results.len(),
            1,
            "Only the genuine relevant policy result should survive"
        );
        assert_eq!(results[0].title, "2026年北京小学入学政策官方解答");
        assert_eq!(results[0].url, "https://jw.beijing.gov.cn/policy/2026");
    }

    #[test]
    fn test_normalize_url_for_dedup() {
        assert_eq!(
            normalize_url_for_dedup("https://www.rust-lang.org/"),
            "https://rust-lang.org"
        );
        assert_eq!(
            normalize_url_for_dedup("http://rust-lang.org"),
            "https://rust-lang.org"
        );
        assert_eq!(
            normalize_url_for_dedup("HTTPS://RUST-LANG.ORG/"),
            "https://rust-lang.org"
        );
    }

    #[test]
    fn test_tokenize_query_domain_and_phrase_extraction() {
        let tokens = tokenize_query("site:zhuanlan.zhihu.com DeepSeek 深度解析");
        assert_eq!(tokens.domain_filters, vec!["zhuanlan.zhihu.com"]);
        assert_eq!(
            tokens.core_tokens,
            vec!["deepseek", "深度解析", "深度", "解析"]
        );
        assert!(tokens.phrase_filters.is_empty());

        let tokens_quote = tokenize_query("\"small language model\" release");
        assert_eq!(tokens_quote.phrase_filters, vec!["small language model"]);
        assert_eq!(
            tokens_quote.core_tokens,
            vec!["small", "language", "model", "release"]
        );
        // Ensure no quotes polluted the individual core tokens
        for t in &tokens_quote.core_tokens {
            assert!(!t.contains('"'), "Token {t} must not contain quotes");
        }
    }

    #[test]
    fn test_relevance_space_insensitive_and_domain_boost() {
        let tokens_hf = tokenize_query("huggingface trending");
        let title_hf = "Trending Papers - Hugging Face";
        let snip_hf = "Explore community trending models and papers.";
        let score_hf = calculate_relevance_full(
            title_hf,
            snip_hf,
            "https://huggingface.co/papers/trending",
            &tokens_hf,
        );
        assert!(
            score_hf.is_some(),
            "Hugging Face with spaces must match huggingface token"
        );
        assert!(score_hf.unwrap() >= 25);

        let tokens_zhihu = tokenize_query("site:zhuanlan.zhihu.com DeepSeek 深度解析");
        let title_zhihu = "DeepSeek 核心架构与注意力机制详解";
        let snip_zhihu = "深入分析 DeepSeek 的工程实践。";
        let url_zhihu = "https://zhuanlan.zhihu.com/p/12345678";
        let score_zhihu =
            calculate_relevance_full(title_zhihu, snip_zhihu, url_zhihu, &tokens_zhihu);
        assert!(
            score_zhihu.is_some(),
            "Matching domain filter must grant score boost and survive"
        );
        assert!(score_zhihu.unwrap() >= 40);
    }

    #[test]
    fn test_relevance_relaxed_fallback() {
        let tokens_llama = tokenize_query("llama 3.2 1b 3b");
        let title_llama = "Llama · Your AI, on your computer";
        let snip_llama = "Open source artificial intelligence models by Meta.";
        let url_llama = "https://www.llama.com/";
        let relaxed_score =
            calculate_relaxed_relevance(title_llama, snip_llama, url_llama, &tokens_llama);
        assert!(
            relaxed_score.is_some(),
            "Llama query must pass relaxed fallback scoring"
        );
    }
}

#[cfg(test)]
mod filter_tests {
    use super::*;

    #[test]
    fn date_language_and_region_map_to_bing_parameters() {
        let filters = SearchFilters {
            since: Some("2026-01-01".into()),
            until: Some("2026-01-31".into()),
            language: Some("zh-CN".into()),
            region: Some("CN".into()),
        };
        filters.validate().unwrap();
        let url = url::Url::parse(&bing_search_url_with("rust", &filters).unwrap()).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs.get("q").map(String::as_str), Some("rust"));
        assert_eq!(pairs.get("setlang").map(String::as_str), Some("zh-cn"));
        assert_eq!(pairs.get("cc").map(String::as_str), Some("cn"));
        let range = pairs.get("filters").expect("date range parameter");
        assert!(range.starts_with("ex1:\"ez5_"), "{range}");
    }

    #[test]
    fn invalid_filters_are_rejected_before_fetching() {
        for filters in [
            SearchFilters {
                since: Some("2026-02-01".into()),
                until: Some("2026-01-01".into()),
                ..Default::default()
            },
            SearchFilters {
                since: Some("2026-13-01".into()),
                ..Default::default()
            },
            SearchFilters {
                until: Some("1969-12-31".into()),
                ..Default::default()
            },
            SearchFilters {
                language: Some("toolonglanguagecode".into()),
                ..Default::default()
            },
            SearchFilters {
                region: Some("chn".into()),
                ..Default::default()
            },
        ] {
            assert!(filters.validate().is_err());
            assert!(bing_search_url_with("q", &filters).is_err());
        }
    }

    #[test]
    fn default_filters_leave_the_query_unchanged() {
        let url = url::Url::parse(&bing_search_url_with("a b", &SearchFilters::default()).unwrap())
            .unwrap();
        assert_eq!(url.query_pairs().count(), 1);
    }
}
