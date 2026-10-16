use crate::error::PageclawError;
use crate::model::SearchEngineKind;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use tracing::{debug, warn};

// Bing Selectors
static BING_ITEM_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("li.b_algo, .b_algo").unwrap());
static BING_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h2 a, .b_title a").unwrap());
static BING_SNIPPET_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".b_caption p, .b_algoSlug, .b_snippet, .b_lineclamp2, .b_lineclamp3, .b_lineclamp4, .b_paractl").unwrap()
});

// Google Selectors
static GOOGLE_ITEM_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("div.g, div.tF2Cxc, div.MjjYud, .MjjYud > div").unwrap());
static GOOGLE_TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("h3").unwrap());
static GOOGLE_LINK_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a[href]").unwrap());
static GOOGLE_SNIPPET_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("div[data-sncf], .VwiC3b, .yXK7lf, .s3v9rd, .MUxGbd").unwrap()
});

/// Structured web search result item with provider attribution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResultItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub engine: String,
}

/// Executes a web search using the specified engine (Bing, Google, or Auto failover).
pub async fn web_search_with_engine(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
    engine: SearchEngineKind,
) -> Result<Vec<SearchResultItem>, PageclawError> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Ok(Vec::new());
    }

    match engine {
        SearchEngineKind::Bing => search_bing(client, clean_query, limit).await,
        SearchEngineKind::Google => search_google(client, clean_query, limit).await,
        SearchEngineKind::Auto => {
            debug!(
                query = clean_query,
                "Executing Auto search (trying Bing first)"
            );
            match search_bing(client, clean_query, limit).await {
                Ok(results) if !results.is_empty() => Ok(results),
                Ok(_) | Err(_) => {
                    warn!(
                        query = clean_query,
                        "Bing search returned empty or failed, falling back to Google"
                    );
                    search_google(client, clean_query, limit).await
                }
            }
        }
    }
}

/// Default web search implementation using the default configured engine (Bing / Auto).
pub async fn web_search(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, PageclawError> {
    web_search_with_engine(client, query, limit, SearchEngineKind::default()).await
}

/// Searches Microsoft Bing for the given query without API keys.
pub async fn search_bing(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, PageclawError> {
    let encoded_q: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let search_url = format!("https://www.bing.com/search?q={encoded_q}&setlang=zh-hans");

    debug!(query = query, url = %search_url, "Executing Bing web search");

    let resp = client
        .get(&search_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        )
        .header("Referer", "https://www.bing.com/")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .send()
        .await?;

    let html = resp.text().await?;
    parse_bing_html(&html, limit)
}

/// Parses Bing search results HTML into structured items.
pub fn parse_bing_html(html: &str, limit: usize) -> Result<Vec<SearchResultItem>, PageclawError> {
    let doc = Html::parse_document(html);
    let mut results = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    for element in doc.select(&BING_ITEM_SELECTOR) {
        let title_el = match element.select(&BING_TITLE_SELECTOR).next() {
            Some(el) => el,
            None => continue,
        };

        let raw_title = title_el
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string();
        let raw_href = title_el.value().attr("href").unwrap_or("").trim();

        if raw_title.is_empty() || raw_href.is_empty() {
            continue;
        }

        // Filter Bing internal links or invalid protocols
        if !raw_href.starts_with("http://") && !raw_href.starts_with("https://") {
            continue;
        }

        let resolved_url = unwrap_bing_url(raw_href);
        if !seen_urls.insert(resolved_url.clone()) {
            continue;
        }

        let snippet = element
            .select(&BING_SNIPPET_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_default();

        results.push(SearchResultItem {
            title: raw_title,
            url: resolved_url,
            snippet,
            engine: "bing".to_string(),
        });

        if results.len() >= limit {
            break;
        }
    }

    Ok(results)
}

/// Searches Google for the given query without API keys.
pub async fn search_google(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, PageclawError> {
    let encoded_q: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let search_url = format!("https://www.google.com/search?q={encoded_q}&hl=zh-CN&gl=cn");

    debug!(query = query, url = %search_url, "Executing Google web search");

    let resp = client
        .get(&search_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        )
        .header("Referer", "https://www.google.com/")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .send()
        .await?;

    let html = resp.text().await?;
    parse_google_html(&html, limit)
}

/// Parses Google search results HTML into structured items.
pub fn parse_google_html(html: &str, limit: usize) -> Result<Vec<SearchResultItem>, PageclawError> {
    let doc = Html::parse_document(html);
    let mut results = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    for element in doc.select(&GOOGLE_ITEM_SELECTOR) {
        let title_el = match element.select(&GOOGLE_TITLE_SELECTOR).next() {
            Some(el) => el,
            None => continue,
        };

        let raw_title = title_el
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string();
        if raw_title.is_empty() {
            continue;
        }

        // Link element: usually parent of h3 or first a[href] in item
        let raw_href = match element.select(&GOOGLE_LINK_SELECTOR).next() {
            Some(a) => a.value().attr("href").unwrap_or("").trim(),
            None => continue,
        };

        if raw_href.is_empty() {
            continue;
        }

        let resolved_url = unwrap_google_url(raw_href);
        if resolved_url.is_empty()
            || !resolved_url.starts_with("http")
            || resolved_url.contains("google.com/search")
            || !seen_urls.insert(resolved_url.clone())
        {
            continue;
        }

        let snippet = element
            .select(&GOOGLE_SNIPPET_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_default();

        results.push(SearchResultItem {
            title: raw_title,
            url: resolved_url,
            snippet,
            engine: "google".to_string(),
        });

        if results.len() >= limit {
            break;
        }
    }

    Ok(results)
}

/// Unwraps Bing tracking redirect URLs like `https://www.bing.com/ck/a?!&&...&u=a1aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw...`
/// into the direct destination URL (e.g. `https://rust-lang.org/`).
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

/// Unwraps Google redirect URLs like `/url?q=https%3A%2F%2Fexample.com&...`
pub fn unwrap_google_url(raw_href: &str) -> String {
    if let Some(pos) = raw_href.find("/url?q=") {
        let after_q = &raw_href[pos + 7..];
        let encoded = after_q.split('&').next().unwrap_or("");
        let decoded: String = url::form_urlencoded::byte_serialize(encoded.as_bytes()).collect();
        return url::form_urlencoded::parse(encoded.as_bytes())
            .map(|(k, _)| k.to_string())
            .next()
            .unwrap_or_else(|| decoded);
    }

    raw_href.to_string()
}

/// Formats search results as a clean GitHub Flavored Markdown document with engine source attribution.
pub fn format_search_results_markdown(query: &str, results: &[SearchResultItem]) -> String {
    let mut out = String::new();
    let engine_name = results
        .first()
        .map(|r| r.engine.as_str())
        .unwrap_or("search");

    out.push_str(&format!("# 网页搜索 ({engine_name}): \"{query}\"\n\n"));

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

        let results = parse_bing_html(sample, 5).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert!(results[0].snippet.contains("empowering everyone"));
        assert_eq!(results[0].engine, "bing");
    }

    #[test]
    fn test_parse_google_html_sample() {
        let sample = r#"
            <div class="g">
                <div class="tF2Cxc">
                    <a href="https://github.com/tokio-rs/tokio">
                        <h3>Tokio: An asynchronous runtime for Rust</h3>
                    </a>
                    <div class="VwiC3b">
                        A runtime for writing reliable, asynchronous, and slim applications.
                    </div>
                </div>
            </div>
        "#;

        let results = parse_google_html(sample, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Tokio: An asynchronous runtime for Rust");
        assert_eq!(results[0].url, "https://github.com/tokio-rs/tokio");
        assert!(results[0].snippet.contains("asynchronous, and slim"));
        assert_eq!(results[0].engine, "google");
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
    fn test_unwrap_google_url() {
        let google_url = "/url?q=https%3A%2F%2Fwww.rust-lang.org%2F&sa=U&ved=2ahUKEwj";
        let unwrapped = unwrap_google_url(google_url);
        assert_eq!(unwrapped, "https://www.rust-lang.org/");

        let direct_url = "https://example.com/page";
        assert_eq!(unwrap_google_url(direct_url), "https://example.com/page");
    }
}
