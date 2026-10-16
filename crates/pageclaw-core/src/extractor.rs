use crate::model::{ExtractedImage, ExtractedLink, ExtractedTable, PageMetadata};
use scraper::{Html, Selector};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static META_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("meta").unwrap());
static CANONICAL_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("link[rel='canonical']").unwrap());
static JSON_LD_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("script[type='application/ld+json']").unwrap());
static LINK_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("a[href]").unwrap());
static IMAGE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("img[src]").unwrap());
static TABLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("table").unwrap());
static TR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("tr").unwrap());
static TH_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("th").unwrap());
static TD_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("td, th").unwrap());
static THEAD_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("thead").unwrap());

/// Extracts metadata and structured data (JSON-LD, OpenGraph, Twitter Card) from HTML.
pub fn extract_metadata(doc: &Html) -> PageMetadata {
    let mut metadata = PageMetadata::default();
    let mut og = HashMap::new();
    let mut twitter = HashMap::new();

    // Query all <meta> tags
    for element in doc.select(&META_SELECTOR) {
        let attrs = element.value().attrs().collect::<HashMap<_, _>>();

        // 1. OpenGraph: <meta property="og:..." content="...">
        if let Some(prop) = attrs.get("property") {
            if let Some(content) = attrs.get("content") {
                if prop.starts_with("og:") {
                    og.insert(prop.to_string(), content.to_string());
                }
            }
        }

        // 2. Name-based meta tags: <meta name="..." content="...">
        if let Some(name) = attrs.get("name") {
            if let Some(content) = attrs.get("content") {
                let name_lower = name.to_ascii_lowercase();
                match name_lower.as_str() {
                    "description" if metadata.description.is_none() => {
                        metadata.description = Some(content.to_string());
                    }
                    "keywords" => {
                        let kw = content
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        metadata.keywords = kw;
                    }
                    "author" if metadata.author.is_none() => {
                        metadata.author = Some(content.to_string());
                    }
                    _ if name_lower.starts_with("twitter:") => {
                        twitter.insert(name.to_string(), content.to_string());
                    }
                    _ if (name_lower.starts_with("article:published_time")
                        || name_lower == "pubdate"
                        || name_lower == "publishdate")
                        && metadata.published_time.is_none() =>
                    {
                        metadata.published_time = Some(content.to_string());
                    }
                    _ => {}
                }
            }
        }
    }

    // 3. Fallback for published time from og:article:published_time
    if metadata.published_time.is_none() {
        if let Some(pub_time) = og.get("og:article:published_time") {
            metadata.published_time = Some(pub_time.clone());
        }
    }

    // 4. Canonical link: <link rel="canonical" href="...">
    if let Some(link) = doc.select(&CANONICAL_SELECTOR).next() {
        if let Some(href) = link.value().attr("href") {
            metadata.canonical_url = Some(href.to_string());
        }
    }

    metadata.open_graph = og;
    metadata.twitter_card = twitter;

    metadata
}

/// Extracts all JSON-LD blocks (<script type="application/ld+json">) and normalizes into JSON values.
pub fn extract_json_ld(doc: &Html) -> Vec<serde_json::Value> {
    let mut results = Vec::new();

    for element in doc.select(&JSON_LD_SELECTOR) {
        let text = element.text().collect::<Vec<_>>().join("");
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(val) => {
                // If the top-level is an array, flatten elements
                if let serde_json::Value::Array(arr) = val {
                    results.extend(arr);
                } else if let Some(graph) = val.get("@graph").and_then(|g| g.as_array()) {
                    // If JSON-LD uses {"@context": "...", "@graph": [...]}, unpack @graph items
                    for item in graph {
                        results.push(item.clone());
                    }
                } else {
                    results.push(val);
                }
            }
            Err(e) => {
                debug!(error = %e, "Failed to parse JSON-LD block");
            }
        }
    }

    results
}

/// Extracts all hyperlinks from the document, resolving relative paths against `base_url`
/// and deduplicating by URL while preserving document appearance order.
pub fn extract_links(doc: &Html, base_url_str: Option<&str>) -> Vec<ExtractedLink> {
    let base_url = base_url_str.and_then(|u| Url::parse(u).ok());
    let mut results = Vec::new();
    let mut seen_urls = HashSet::new();

    for element in doc.select(&LINK_SELECTOR) {
        let href_raw = match element.value().attr("href") {
            Some(h) => h.trim(),
            None => continue,
        };
        if href_raw.is_empty() || href_raw.starts_with('#') || href_raw.starts_with("javascript:") {
            continue;
        }

        let resolved_url = if let Some(ref base) = base_url {
            if let Ok(joined) = base.join(href_raw) {
                joined.to_string()
            } else {
                href_raw.to_string()
            }
        } else {
            href_raw.to_string()
        };

        if seen_urls.insert(resolved_url.clone()) {
            let text = element.text().collect::<Vec<_>>().join(" ").trim().to_string();
            results.push(ExtractedLink {
                text,
                url: resolved_url,
            });
        }
    }

    results
}

/// Extracts all image assets from the document, resolving relative paths against `base_url`
/// and deduplicating by source URL while preserving appearance order.
pub fn extract_images(doc: &Html, base_url_str: Option<&str>) -> Vec<ExtractedImage> {
    let base_url = base_url_str.and_then(|u| Url::parse(u).ok());
    let mut results = Vec::new();
    let mut seen_srcs = HashSet::new();

    for element in doc.select(&IMAGE_SELECTOR) {
        let src_raw = match element.value().attr("src") {
            Some(s) => s.trim(),
            None => continue,
        };
        if src_raw.is_empty() || src_raw.starts_with("data:") {
            continue;
        }

        let resolved_src = if let Some(ref base) = base_url {
            if let Ok(joined) = base.join(src_raw) {
                joined.to_string()
            } else {
                src_raw.to_string()
            }
        } else {
            src_raw.to_string()
        };

        if seen_srcs.insert(resolved_src.clone()) {
            let alt = element.value().attr("alt").unwrap_or("").trim().to_string();
            results.push(ExtractedImage {
                alt,
                src: resolved_src,
            });
        }
    }

    results
}

/// Extracts all <table> elements into structured 2D arrays and row dictionary records.
pub fn extract_tables(doc: &Html) -> Vec<ExtractedTable> {
    let mut tables = Vec::new();

    for table_el in doc.select(&TABLE_SELECTOR) {
        let mut headers = Vec::new();
        let mut rows = Vec::new();

        let has_thead = table_el.select(&THEAD_SELECTOR).next().is_some();
        if has_thead {
            if let Some(thead_el) = table_el.select(&THEAD_SELECTOR).next() {
                if let Some(first_tr) = thead_el.select(&TR_SELECTOR).next() {
                    for cell in first_tr.select(&TD_SELECTOR) {
                        let text = cell.text().collect::<Vec<_>>().join(" ").trim().to_string();
                        headers.push(text);
                    }
                }
            }

            for tr in table_el.select(&TR_SELECTOR) {
                let is_in_thead = {
                    let mut curr = tr.parent();
                    let mut in_th = false;
                    while let Some(parent_node) = curr {
                        if let Some(el) = parent_node.value().as_element() {
                            if el.name() == "thead" {
                                in_th = true;
                                break;
                            }
                            if el.name() == "table" {
                                break;
                            }
                        }
                        curr = parent_node.parent();
                    }
                    in_th
                };
                if is_in_thead {
                    continue;
                }

                let row: Vec<String> = tr
                    .select(&TD_SELECTOR)
                    .map(|cell| cell.text().collect::<Vec<_>>().join(" ").trim().to_string())
                    .collect();
                if !row.is_empty() {
                    rows.push(row);
                }
            }
        } else {
            let all_trs: Vec<_> = table_el.select(&TR_SELECTOR).collect();
            if let Some(first_tr) = all_trs.first() {
                let has_th = first_tr.select(&TH_SELECTOR).next().is_some();
                if has_th || all_trs.len() > 1 {
                    headers = first_tr
                        .select(&TD_SELECTOR)
                        .map(|cell| cell.text().collect::<Vec<_>>().join(" ").trim().to_string())
                        .collect();

                    for tr in all_trs.iter().skip(1) {
                        let row: Vec<String> = tr
                            .select(&TD_SELECTOR)
                            .map(|cell| cell.text().collect::<Vec<_>>().join(" ").trim().to_string())
                            .collect();
                        if !row.is_empty() {
                            rows.push(row);
                        }
                    }
                } else {
                    let row: Vec<String> = first_tr
                        .select(&TD_SELECTOR)
                        .map(|cell| cell.text().collect::<Vec<_>>().join(" ").trim().to_string())
                        .collect();
                    for i in 0..row.len() {
                        headers.push(format!("col_{}", i + 1));
                    }
                    if !row.is_empty() {
                        rows.push(row);
                    }
                }
            }
        }

        if headers.is_empty() {
            if let Some(first_row) = rows.first() {
                for i in 0..first_row.len() {
                    headers.push(format!("col_{}", i + 1));
                }
            }
        }

        let clean_headers: Vec<String> = headers
            .iter()
            .enumerate()
            .map(|(i, h)| {
                if h.trim().is_empty() {
                    format!("col_{}", i + 1)
                } else {
                    h.clone()
                }
            })
            .collect();

        let mut records = Vec::new();
        for row in &rows {
            let mut rec = HashMap::new();
            for (i, cell) in row.iter().enumerate() {
                let key = if i < clean_headers.len() {
                    clean_headers[i].clone()
                } else {
                    format!("col_{}", i + 1)
                };
                rec.insert(key, cell.clone());
            }
            records.push(rec);
        }

        tables.push(ExtractedTable {
            headers,
            rows,
            records,
        });
    }

    tables
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_metadata() {
        let html = r#"
            <html>
                <head>
                    <meta name="description" content="A page about Rust">
                    <meta name="keywords" content="rust, cli, agent">
                    <meta name="author" content="PageClaw">
                    <meta property="og:title" content="Rust CLI">
                    <meta property="og:article:published_time" content="2026-03-01T12:00:00Z">
                    <meta name="twitter:card" content="summary_large_image">
                    <link rel="canonical" href="https://example.com/canonical">
                </head>
                <body></body>
            </html>
        "#;

        let doc = Html::parse_document(html);
        let meta = extract_metadata(&doc);

        assert_eq!(meta.description.as_deref(), Some("A page about Rust"));
        assert_eq!(meta.keywords, vec!["rust", "cli", "agent"]);
        assert_eq!(meta.author.as_deref(), Some("PageClaw"));
        assert_eq!(meta.canonical_url.as_deref(), Some("https://example.com/canonical"));
        assert_eq!(meta.published_time.as_deref(), Some("2026-03-01T12:00:00Z"));
        assert_eq!(meta.open_graph.get("og:title").map(|s| s.as_str()), Some("Rust CLI"));
        assert_eq!(meta.twitter_card.get("twitter:card").map(|s| s.as_str()), Some("summary_large_image"));
    }

    #[test]
    fn test_extract_json_ld() {
        let html = r#"
            <html>
                <head>
                    <script type="application/ld+json">
                    {
                        "@context": "https://schema.org",
                        "@type": "Article",
                        "headline": "PageClaw Launch"
                    }
                    </script>
                </head>
                <body></body>
            </html>
        "#;

        let doc = Html::parse_document(html);
        let json_lds = extract_json_ld(&doc);

        assert_eq!(json_lds.len(), 1);
        assert_eq!(json_lds[0]["headline"], "PageClaw Launch");
        assert_eq!(json_lds[0]["@type"], "Article");
    }

    #[test]
    fn test_extract_links() {
        let html = r##"
            <html>
                <body>
                    <a href="/docs">Documentation</a>
                    <a href="https://example.com/about">About</a>
                    <a href="/docs">Documentation duplicate</a>
                    <a href="#section">Fragment</a>
                    <a href="javascript:void(0)">JS</a>
                </body>
            </html>
        "##;
        let doc = Html::parse_document(html);
        let links = extract_links(&doc, Some("https://example.com/base/"));

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].text, "Documentation");
        assert_eq!(links[0].url, "https://example.com/docs");
        assert_eq!(links[1].text, "About");
        assert_eq!(links[1].url, "https://example.com/about");
    }

    #[test]
    fn test_extract_images() {
        let html = r#"
            <html>
                <body>
                    <img src="img/logo.png" alt="Company Logo">
                    <img src="https://cdn.example.com/banner.jpg" alt="Banner">
                    <img src="img/logo.png" alt="Another Logo">
                    <img src="data:image/png;base64,abc==" alt="Inline">
                </body>
            </html>
        "#;
        let doc = Html::parse_document(html);
        let images = extract_images(&doc, Some("https://example.com/page/"));

        assert_eq!(images.len(), 2);
        assert_eq!(images[0].alt, "Company Logo");
        assert_eq!(images[0].src, "https://example.com/page/img/logo.png");
        assert_eq!(images[1].alt, "Banner");
        assert_eq!(images[1].src, "https://cdn.example.com/banner.jpg");
    }

    #[test]
    fn test_extract_tables() {
        let html = r#"
            <html>
                <body>
                    <table>
                        <thead>
                            <tr>
                                <th>Name</th>
                                <th>Role</th>
                                <th>Score</th>
                            </tr>
                        </thead>
                        <tbody>
                            <tr>
                                <td>Alice</td>
                                <td>Engineer</td>
                                <td>95</td>
                            </tr>
                            <tr>
                                <td>Bob</td>
                                <td>Designer</td>
                                <td>88</td>
                            </tr>
                        </tbody>
                    </table>
                </body>
            </html>
        "#;
        let doc = Html::parse_document(html);
        let tables = extract_tables(&doc);

        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.headers, vec!["Name", "Role", "Score"]);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0], vec!["Alice", "Engineer", "95"]);
        assert_eq!(t.rows[1], vec!["Bob", "Designer", "88"]);
        assert_eq!(t.records.len(), 2);
        assert_eq!(t.records[0].get("Name").map(|s| s.as_str()), Some("Alice"));
        assert_eq!(t.records[0].get("Score").map(|s| s.as_str()), Some("95"));
    }
}
