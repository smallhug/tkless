use crate::error::PageclawError;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static DDG_RESULT_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".result, .results_links, .result__body").unwrap());
static DDG_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".result__a").unwrap());
static DDG_SNIPPET_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".result__snippet").unwrap());

/// Structured web search result item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResultItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Executes a zero-API-key web search using DuckDuckGo HTML endpoint.
pub async fn web_search(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, PageclawError> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Ok(Vec::new());
    }

    let encoded_q: String = url::form_urlencoded::byte_serialize(clean_query.as_bytes()).collect();
    let search_url = format!("https://html.duckduckgo.com/html/?q={encoded_q}");

    debug!(query = clean_query, url = %search_url, "Executing web search");

    let resp = client
        .get(&search_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        )
        .header("Referer", "https://html.duckduckgo.com/")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .send()
        .await?;

    let html = resp.text().await?;
    let doc = Html::parse_document(&html);

    let mut results = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    for element in doc.select(&DDG_RESULT_SELECTOR) {
        let title_el = match element.select(&DDG_TITLE_SELECTOR).next() {
            Some(el) => el,
            None => continue,
        };

        let raw_title = title_el.text().collect::<Vec<_>>().join(" ").trim().to_string();
        let raw_href = title_el.value().attr("href").unwrap_or("").trim();

        if raw_title.is_empty() || raw_href.is_empty() {
            continue;
        }

        let resolved_url = unwrap_duckduckgo_url(raw_href);
        if resolved_url.is_empty() || !seen_urls.insert(resolved_url.clone()) {
            continue;
        }

        let snippet = element
            .select(&DDG_SNIPPET_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_default();

        results.push(SearchResultItem {
            title: raw_title,
            url: resolved_url,
            snippet,
        });

        if results.len() >= limit {
            break;
        }
    }

    Ok(results)
}

/// Formats search results as a clean GitHub Flavored Markdown document.
pub fn format_search_results_markdown(query: &str, results: &[SearchResultItem]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# 网页搜索: \"{query}\"\n\n"));

    if results.is_empty() {
        out.push_str("未检索到相关网页结果。\n");
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

/// Unwraps DuckDuckGo redirect URLs like `//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&...`
pub fn unwrap_duckduckgo_url(raw_href: &str) -> String {
    let full_url = if raw_href.starts_with("//") {
        format!("https:{raw_href}")
    } else if raw_href.starts_with('/') {
        format!("https://duckduckgo.com{raw_href}")
    } else {
        raw_href.to_string()
    };

    if let Ok(parsed) = Url::parse(&full_url) {
        for (k, v) in parsed.query_pairs() {
            if k == "uddg" && !v.is_empty() {
                return v.to_string();
            }
        }
        if (full_url.starts_with("http://") || full_url.starts_with("https://"))
            && parsed.host_str() != Some("duckduckgo.com")
            && parsed.host_str() != Some("html.duckduckgo.com")
        {
            return full_url;
        }
    }

    raw_href.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unwrap_duckduckgo_url() {
        let raw = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&rut=abc1234";
        let unwrapped = unwrap_duckduckgo_url(raw);
        assert_eq!(unwrapped, "https://www.rust-lang.org/");

        let direct = "https://example.com/blog/test";
        assert_eq!(unwrap_duckduckgo_url(direct), "https://example.com/blog/test");
    }

    #[test]
    fn test_format_search_results_markdown() {
        let items = vec![
            SearchResultItem {
                title: "Rust Programming Language".to_string(),
                url: "https://www.rust-lang.org/".to_string(),
                snippet: "Empowering everyone to build reliable and efficient software.".to_string(),
            },
            SearchResultItem {
                title: "Tokio Async Runtime".to_string(),
                url: "https://tokio.rs/".to_string(),
                snippet: "An asynchronous runtime for the Rust programming language.".to_string(),
            },
        ];

        let md = format_search_results_markdown("Rust", &items);
        assert!(md.contains("# 网页搜索: \"Rust\""));
        assert!(md.contains("1. [Rust Programming Language](https://www.rust-lang.org/)"));
        assert!(md.contains("> Empowering everyone to build reliable"));
        assert!(md.contains("2. [Tokio Async Runtime](https://tokio.rs/)"));
    }
}
