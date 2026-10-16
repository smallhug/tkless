use crate::error::PageclawError;
use htmd::HtmlToMarkdown;
use regex::Regex;
use std::sync::LazyLock;

static IMAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[(.*?)\]\(.*?\)").unwrap());
static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^!])\[(.*?)\]\(.*?\)").unwrap());
static BLANK_LINES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// HTML to clean, readable Markdown converter.
pub struct MarkdownConverter {
    converter: HtmlToMarkdown,
}

impl Default for MarkdownConverter {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for MarkdownConverter {
    fn clone(&self) -> Self {
        Self::new()
    }
}

impl MarkdownConverter {
    pub fn new() -> Self {
        Self {
            converter: HtmlToMarkdown::new(),
        }
    }

    /// Converts cleaned HTML into GitHub Flavored Markdown with optional token-saving filters.
    pub fn convert(
        &self,
        html: &str,
        strip_images: bool,
        strip_links: bool,
    ) -> Result<String, PageclawError> {
        let mut md = self
            .converter
            .convert(html)
            .map_err(|e| PageclawError::ConversionError(e.to_string()))?;

        if strip_images {
            md = remove_markdown_images(&md);
        }

        if strip_links {
            md = remove_markdown_links(&md);
        }

        // Normalize excessive consecutive blank lines
        let normalized = normalize_blank_lines(&md);

        Ok(normalized)
    }
}

/// Replaces `![alt](url)` with `[Image: alt]` or strips it if alt is empty.
fn remove_markdown_images(md: &str) -> String {
    IMAGE_RE
        .replace_all(md, |caps: &regex::Captures| {
            let alt = caps.get(1).map_or("", |m| m.as_str()).trim();
            if alt.is_empty() {
                "".to_string()
            } else {
                format!("[Image: {alt}]")
            }
        })
        .to_string()
}

/// Replaces `[text](url)` with `text`.
fn remove_markdown_links(md: &str) -> String {
    LINK_RE
        .replace_all(md, |caps: &regex::Captures| {
            let prefix = caps.get(1).map_or("", |m| m.as_str());
            let text = caps.get(2).map_or("", |m| m.as_str());
            format!("{prefix}{text}")
        })
        .to_string()
}

/// Collapses 3 or more consecutive newlines into double newlines.
fn normalize_blank_lines(text: &str) -> String {
    BLANK_LINES_RE.replace_all(text.trim(), "\n\n").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_markdown_basic() {
        let converter = MarkdownConverter::new();
        let html = "<h1>Title</h1><p>Hello <strong>World</strong>!</p>";
        let md = converter.convert(html, false, false).unwrap();
        assert!(md.contains("# Title"));
        assert!(md.contains("**World**"));
    }

    #[test]
    fn test_strip_images_and_links() {
        let converter = MarkdownConverter::new();
        let html = r#"<p><a href="https://example.com">Visit site</a> <img src="cat.png" alt="Cute Cat" /></p>"#;
        let md = converter.convert(html, true, true).unwrap();
        assert!(!md.contains("https://example.com"));
        assert!(md.contains("Visit site"));
        assert!(md.contains("[Image: Cute Cat]"));
        assert!(!md.contains("cat.png"));
    }
}
