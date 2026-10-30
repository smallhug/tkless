use crate::error::TklessError;
use crate::model::ExtractionMode;
use scraper::{Html, Selector};
use std::io::Cursor;
use std::sync::LazyLock;
use tracing::{debug, warn};
use url::Url;

static FULL_PAGE_REMOVE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "script, style, noscript, svg, math, canvas, iframe, object, embed, base, meta, link, form, input, textarea, button, dialog, nav, footer, aside",
    )
    .unwrap()
});

static TARGETED_REMOVE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "script, style, noscript, svg, math, canvas, iframe, object, embed, base, meta, link, form, input, textarea, button, dialog",
    )
    .unwrap()
});

static AD_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "[role='banner'], [role='navigation'], [aria-hidden='true'], .advertisement, .ads, .ad-banner",
    )
    .unwrap()
});

static BODY_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("body").unwrap());
static TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("title").unwrap());
static BASE_TAG_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("base[href]").unwrap());

/// Cleaned HTML representation ready for conversion or downstream processing.
#[derive(Debug, Clone)]
pub struct CleanedContent {
    pub title: Option<String>,
    pub byline: Option<String>,
    pub html: String,
    pub is_article_fallback: bool,
}

/// Prunes noise, ads, scripts, and extracts main article or cleaned full-page HTML.
pub fn clean_html(
    raw_html: &str,
    base_url_str: Option<&str>,
    mode: ExtractionMode,
) -> Result<CleanedContent, TklessError> {
    match mode {
        ExtractionMode::Raw => Ok(CleanedContent {
            title: extract_title_simple(raw_html),
            byline: None,
            html: raw_html.to_string(),
            is_article_fallback: false,
        }),
        ExtractionMode::Article => {
            // Attempt Readability extraction first
            let base_url = base_url_str
                .and_then(|u| Url::parse(u).ok())
                .unwrap_or_else(|| Url::parse("https://example.com").unwrap());

            let mut cursor = Cursor::new(raw_html.as_bytes());
            match readability::extractor::extract(&mut cursor, &base_url) {
                Ok(product) if !product.content.trim().is_empty() => {
                    debug!("Readability extraction succeeded");
                    let title = if product.title.trim().is_empty() {
                        extract_title_simple(raw_html)
                    } else {
                        Some(product.title)
                    };
                    Ok(CleanedContent {
                        title,
                        byline: None,
                        html: clean_full_page(&product.content, Some(base_url.as_str()))?.html,
                        is_article_fallback: false,
                    })
                }
                Ok(_) | Err(_) => {
                    warn!("Readability extraction returned empty or failed, falling back to full-page cleaning");
                    let mut cleaned = clean_full_page(raw_html, base_url_str)?;
                    cleaned.is_article_fallback = true;
                    Ok(cleaned)
                }
            }
        }
        ExtractionMode::Full => clean_full_page(raw_html, base_url_str),
    }
}

/// Resolves effective base URL, prioritizing `<base href="...">` tag in `<head>` if present.
pub fn resolve_effective_base_url(doc: &Html, fallback_base_url: Option<&str>) -> Option<Url> {
    let parsed_fallback = fallback_base_url.and_then(|u| Url::parse(u).ok());

    if let Some(base_el) = doc.select(&BASE_TAG_SELECTOR).next() {
        if let Some(href) = base_el.value().attr("href") {
            let trimmed = href.trim();
            if !trimmed.is_empty() {
                if let Ok(abs_base) = Url::parse(trimmed) {
                    return Some(abs_base);
                } else if let Some(ref parent_base) = parsed_fallback {
                    if let Ok(joined) = parent_base.join(trimmed) {
                        return Some(joined);
                    }
                }
            }
        }
    }

    parsed_fallback
}

/// Cleans full page by stripping boilerplate, navigation, scripts, styles, and normalizing URLs.
fn clean_full_page(
    raw_html: &str,
    base_url_str: Option<&str>,
) -> Result<CleanedContent, TklessError> {
    let document = Html::parse_document(raw_html);
    let title = extract_title_from_doc(&document);

    // Resolve effective base URL (auto-discovering <base href="..."> if present)
    let base_url = resolve_effective_base_url(&document, base_url_str);

    let body_element = document.select(&BODY_SELECTOR).next();

    let target_node = match body_element {
        Some(b) => b,
        None => {
            // If no <body>, fallback to entire doc
            return Ok(CleanedContent {
                title,
                byline: None,
                html: raw_html.to_string(),
                is_article_fallback: false,
            });
        }
    };

    // Serialize cleaned children
    let mut out_html = String::with_capacity(raw_html.len() / 2);
    render_cleaned_node(
        target_node.id(),
        &document,
        &FULL_PAGE_REMOVE_SELECTOR,
        Some(&AD_SELECTOR),
        base_url.as_ref(),
        &mut out_html,
    );

    Ok(CleanedContent {
        title,
        byline: None,
        html: out_html,
        is_article_fallback: false,
    })
}

/// Extracts only the DOM nodes matching a given CSS selector, cleans their inner trees,
/// and concatenates the resulting HTML fragments.
pub fn extract_selector(
    raw_html: &str,
    selector_str: &str,
    base_url_str: Option<&str>,
) -> Result<CleanedContent, TklessError> {
    let document = Html::parse_document(raw_html);
    let title = extract_title_from_doc(&document);

    let selector = Selector::parse(selector_str).map_err(|e| {
        TklessError::CleaningError(format!("Invalid CSS selector '{selector_str}': {e:?}"))
    })?;

    // Resolve effective base URL (auto-discovering <base href="..."> if present)
    let base_url = resolve_effective_base_url(&document, base_url_str);

    let mut out_html = String::new();
    let mut matched_count = 0;

    for element_ref in document.select(&selector) {
        matched_count += 1;
        render_cleaned_node(
            element_ref.id(),
            &document,
            &TARGETED_REMOVE_SELECTOR,
            Some(&AD_SELECTOR),
            base_url.as_ref(),
            &mut out_html,
        );
        out_html.push('\n');
    }

    debug!(
        selector = selector_str,
        matches = matched_count,
        "Targeted CSS selector extraction completed"
    );

    Ok(CleanedContent {
        title,
        byline: None,
        html: out_html.trim().to_string(),
        is_article_fallback: false,
    })
}

fn render_cleaned_node(
    node_id: ego_tree::NodeId,
    document: &Html,
    remove_selector: &Selector,
    ad_selector: Option<&Selector>,
    base_url: Option<&Url>,
    out: &mut String,
) {
    let node = match document.tree.get(node_id) {
        Some(n) => n,
        None => return,
    };

    let value = node.value();
    if let Some(element) = value.as_element() {
        let element_ref = scraper::ElementRef::wrap(node).unwrap();

        // Check if node matches removal selectors
        if remove_selector.matches(&element_ref) {
            return;
        }
        if let Some(ad_sel) = ad_selector {
            if ad_sel.matches(&element_ref) {
                return;
            }
        }

        // Start tag
        let tag_name = element.name();
        out.push('<');
        out.push_str(tag_name);

        // Copy and resolve attributes
        for (attr_name, attr_val) in element.attrs() {
            // Skip noisy inline event handlers (onclick, onload, etc.)
            if attr_name.starts_with("on") {
                continue;
            }
            // Skip inline style
            if matches!(attr_name, "style" | "srcdoc" | "srcset" | "ping") {
                continue;
            }

            let resolved = if matches!(
                attr_name,
                "href" | "src" | "action" | "formaction" | "poster" | "background" | "xlink:href"
            ) {
                let parsed = base_url
                    .and_then(|b| b.join(attr_val).ok())
                    .or_else(|| Url::parse(attr_val).ok());
                match parsed {
                    Some(url) if matches!(url.scheme(), "http" | "https" | "mailto" | "tel") => {
                        Some(url.to_string())
                    }
                    Some(_) => continue,
                    None if attr_val.contains(':') => continue,
                    None => None,
                }
            } else {
                None
            };

            out.push(' ');
            out.push_str(attr_name);
            out.push_str("=\"");

            escape_html(resolved.as_deref().unwrap_or(attr_val), out);
            out.push('"');
        }
        out.push('>');

        // Recurse children
        for child in node.children() {
            render_cleaned_node(
                child.id(),
                document,
                remove_selector,
                ad_selector,
                base_url,
                out,
            );
        }

        // Closing tag
        out.push_str("</");
        out.push_str(tag_name);
        out.push('>');
    } else if let Some(text) = value.as_text() {
        escape_html(text, out);
    } else {
        for child in node.children() {
            render_cleaned_node(
                child.id(),
                document,
                remove_selector,
                ad_selector,
                base_url,
                out,
            );
        }
    }
}

/// Prunes and excludes DOM subtrees matching a specified CSS selector from raw HTML.
pub fn prune_unwanted_selectors(raw_html: &str, selector_str: &str) -> Result<String, TklessError> {
    let selector = Selector::parse(selector_str).map_err(|e| {
        TklessError::CleaningError(format!(
            "Invalid remove CSS selector '{selector_str}': {e:?}"
        ))
    })?;

    let document = Html::parse_document(raw_html);
    let mut out_html = String::with_capacity(raw_html.len());

    for child in document.tree.root().children() {
        render_pruned_node(child.id(), &document, &selector, &mut out_html);
    }

    Ok(out_html)
}

fn render_pruned_node(
    node_id: ego_tree::NodeId,
    document: &Html,
    remove_selector: &Selector,
    out: &mut String,
) {
    let node = match document.tree.get(node_id) {
        Some(n) => n,
        None => return,
    };

    let value = node.value();
    if let Some(element) = value.as_element() {
        let element_ref = scraper::ElementRef::wrap(node).unwrap();

        // Check if node matches the remove selector
        if remove_selector.matches(&element_ref) {
            return;
        }

        let tag_name = element.name();
        out.push('<');
        out.push_str(tag_name);

        for (attr_name, attr_val) in element.attrs() {
            out.push(' ');
            out.push_str(attr_name);
            out.push_str("=\"");
            escape_html(attr_val, out);
            out.push('"');
        }
        out.push('>');

        for child in node.children() {
            render_pruned_node(child.id(), document, remove_selector, out);
        }

        out.push_str("</");
        out.push_str(tag_name);
        out.push('>');
    } else if let Some(text) = value.as_text() {
        // Raw-text elements must retain their original parsing semantics during pruning.
        if node
            .parent()
            .and_then(|p| p.value().as_element())
            .is_some_and(|e| {
                matches!(
                    e.name(),
                    "script" | "style" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext"
                )
            })
        {
            out.push_str(text);
        } else {
            escape_html(text, out);
        }
    } else {
        for child in node.children() {
            render_pruned_node(child.id(), document, remove_selector, out);
        }
    }
}

fn escape_html(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

fn extract_title_from_doc(doc: &Html) -> Option<String> {
    let title_el = doc.select(&TITLE_SELECTOR).next()?;
    let t = title_el.text().collect::<Vec<_>>().join(" ");
    let trimmed = t.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn extract_title_simple(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title>")?;
    let end = lower[start + 7..].find("</title>")?;
    let title_raw = &html[start + 7..start + 7 + end];
    let trimmed = title_raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn encoded_markup_stays_text_and_unsafe_urls_are_removed() {
        let source = r#"<p>&lt;img src=x onerror=alert(1)&gt;<a href="java&#x09;script:alert(1)">bad</a><a href="/ok?a=1&amp;b=2" title="&quot; onmouseover=&quot;bad">ok</a></p>"#;
        let cleaned = super::extract_selector(source, "p", Some("https://example.com")).unwrap();
        let parsed = scraper::Html::parse_fragment(&cleaned.html);
        assert_eq!(
            parsed
                .select(&scraper::Selector::parse("img,[onerror],[onmouseover]").unwrap())
                .count(),
            0
        );
        let links: Vec<_> = parsed
            .select(&scraper::Selector::parse("a").unwrap())
            .collect();
        assert_eq!(links[0].value().attr("href"), None);
        assert_eq!(
            links[1].value().attr("href"),
            Some("https://example.com/ok?a=1&b=2")
        );
        assert!(cleaned.html.contains("&lt;img"));
        let pruned = super::prune_unwanted_selectors(source, ".unused").unwrap();
        assert!(!pruned.contains("<img"));
    }
    use super::*;

    #[test]
    fn test_clean_full_page_removes_scripts_and_styles() {
        let html = r#"
            <html>
                <head><title>Test Page</title></head>
                <body>
                    <nav><a href="/home">Home</a></nav>
                    <main>
                        <h1>Main Content</h1>
                        <p>This is genuine content.</p>
                        <script>console.log("bad");</script>
                        <style>.bad { color: red; }</style>
                    </main>
                    <footer>Footer info</footer>
                </body>
            </html>
        "#;

        let cleaned =
            clean_html(html, Some("https://example.com/sub/"), ExtractionMode::Full).unwrap();
        assert_eq!(cleaned.title.as_deref(), Some("Test Page"));
        assert!(cleaned.html.contains("Main Content"));
        assert!(cleaned.html.contains("This is genuine content."));
        assert!(!cleaned.html.contains("console.log"));
        assert!(!cleaned.html.contains(".bad { color: red; }"));
        assert!(!cleaned.html.contains("Footer info"));
    }

    #[test]
    fn test_resolve_relative_links() {
        let html = r#"
            <body>
                <a href="/about">About Us</a>
                <img src="pic.jpg">
            </body>
        "#;
        let cleaned = clean_html(
            html,
            Some("https://example.com/path/"),
            ExtractionMode::Full,
        )
        .unwrap();
        assert!(cleaned.html.contains("href=\"https://example.com/about\""));
        assert!(cleaned
            .html
            .contains("src=\"https://example.com/path/pic.jpg\""));
    }

    #[test]
    fn test_extract_selector() {
        let html = r#"
            <html>
                <head><title>Selector Test</title></head>
                <body>
                    <div class="sidebar">Sidebar junk</div>
                    <article class="content">
                        <h2>Article Heading</h2>
                        <p>Targeted paragraph.</p>
                        <script>alert(1);</script>
                    </article>
                    <div id="extra">Extra block</div>
                </body>
            </html>
        "#;

        let cleaned = extract_selector(html, "article.content", None).unwrap();
        assert_eq!(cleaned.title.as_deref(), Some("Selector Test"));
        assert!(cleaned.html.contains("Article Heading"));
        assert!(cleaned.html.contains("Targeted paragraph."));
        assert!(!cleaned.html.contains("Sidebar junk"));
        assert!(!cleaned.html.contains("Extra block"));
        assert!(!cleaned.html.contains("alert(1)"));
    }

    #[test]
    fn test_base_href_tag_resolution() {
        let html = r#"
            <!DOCTYPE html>
            <html>
                <head>
                    <base href="https://cdn.example.com/subfolder/">
                    <title>Base Tag Test</title>
                </head>
                <body>
                    <a href="page2.html">Relative Link</a>
                    <img src="logo.png" alt="Logo">
                </body>
            </html>
        "#;

        let cleaned = clean_full_page(html, None).unwrap();
        assert!(cleaned
            .html
            .contains("href=\"https://cdn.example.com/subfolder/page2.html\""));
        assert!(cleaned
            .html
            .contains("src=\"https://cdn.example.com/subfolder/logo.png\""));
    }

    #[test]
    fn test_prune_unwanted_selectors() {
        let html = r#"
            <article>
                <h1>Core Title</h1>
                <div class="promotional-banner">50% off discount!</div>
                <p>Real paragraph text.</p>
                <aside id="author-bio">Author bio</aside>
                <div class="comments">User comments</div>
            </article>
        "#;

        let pruned =
            prune_unwanted_selectors(html, ".promotional-banner, #author-bio, .comments").unwrap();
        assert!(pruned.contains("Core Title"));
        assert!(pruned.contains("Real paragraph text."));
        assert!(!pruned.contains("50% off discount"));
        assert!(!pruned.contains("Author bio"));
        assert!(!pruned.contains("User comments"));
    }
}
