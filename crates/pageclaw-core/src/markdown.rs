use crate::error::PageclawError;
use htmd::HtmlToMarkdown;
use regex::Regex;
use std::sync::LazyLock;

static IMAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[(.*?)\]\(.*?\)").unwrap());
static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^!])\[(.*?)\]\(.*?\)").unwrap());
static BLANK_LINES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

static PRE_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<pre([^>]*)>(.*?)</pre>"#).unwrap());
static CLASS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)class\s*=\s*["']([^"']+)["']"#).unwrap());
static DATA_LANG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)data-lang(?:uage)?\s*=\s*["']([^"']+)["']"#).unwrap());
static INNER_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<code([^>]*)>(.*?)</code>"#).unwrap());

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
        let preprocessed = normalize_code_language_tags(html);
        let mut md = self
            .converter
            .convert(&preprocessed)
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

/// Canonicalizes raw language name or alias into standardized lowercase identifier.
fn canonicalize_language(lang: &str) -> &'static str {
    match lang.to_ascii_lowercase().as_str() {
        "py" | "python" | "python3" => "python",
        "js" | "javascript" | "node" => "javascript",
        "ts" | "typescript" => "typescript",
        "rs" | "rust" => "rust",
        "go" | "golang" => "go",
        "java" => "java",
        "c" => "c",
        "cpp" | "c++" | "cc" | "cxx" => "cpp",
        "cs" | "csharp" | "c#" => "csharp",
        "rb" | "ruby" => "ruby",
        "php" => "php",
        "sh" | "bash" | "shell" | "zsh" => "bash",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" => "css",
        "json" => "json",
        "xml" => "xml",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "dockerfile" | "docker" => "dockerfile",
        _ => "",
    }
}

/// Extracts programming language identifier from element attributes.
fn extract_lang_from_attrs(attrs: &str) -> Option<&'static str> {
    // 1. Check data-lang="xxx" or data-language="xxx"
    if let Some(cap) = DATA_LANG_RE.captures(attrs) {
        if let Some(m) = cap.get(1) {
            let canon = canonicalize_language(m.as_str().trim());
            if !canon.is_empty() {
                return Some(canon);
            }
        }
    }

    // 2. Check class="... language-xxx ... lang-xxx ... highlight-source-xxx ... brush: xxx ..."
    if let Some(cap) = CLASS_RE.captures(attrs) {
        let class_val = cap.get(1).map_or("", |m| m.as_str());
        for part in class_val.split_whitespace() {
            let p_lower = part.to_ascii_lowercase();
            if let Some(lang) = p_lower.strip_prefix("language-") {
                let canon = canonicalize_language(lang);
                if !canon.is_empty() {
                    return Some(canon);
                }
            } else if let Some(lang) = p_lower.strip_prefix("lang-") {
                let canon = canonicalize_language(lang);
                if !canon.is_empty() {
                    return Some(canon);
                }
            } else if let Some(lang) = p_lower.strip_prefix("highlight-source-") {
                let canon = canonicalize_language(lang);
                if !canon.is_empty() {
                    return Some(canon);
                }
            } else if let Some(lang) = p_lower.strip_prefix("brush:") {
                let canon = canonicalize_language(lang.trim());
                if !canon.is_empty() {
                    return Some(canon);
                }
            } else {
                let canon = canonicalize_language(&p_lower);
                if !canon.is_empty() && canon != "c" {
                    return Some(canon);
                }
            }
        }
    }

    None
}

/// Preprocesses HTML to guarantee code blocks receive standardized `<code class="language-<canonical>">` tags.
pub fn normalize_code_language_tags(html: &str) -> String {
    PRE_TAG_RE
        .replace_all(html, |caps: &regex::Captures| {
            let pre_attrs = &caps[1];
            let inner_content = &caps[2];

            let pre_lang = extract_lang_from_attrs(pre_attrs);
            let mut detected_lang = pre_lang;

            let code_match = INNER_CODE_RE.captures(inner_content);
            if let Some(ref c_cap) = code_match {
                let code_attrs = &c_cap[1];
                if let Some(code_lang) = extract_lang_from_attrs(code_attrs) {
                    detected_lang = Some(code_lang);
                }
            }

            if let Some(lang) = detected_lang {
                if let Some(ref c_cap) = code_match {
                    let code_body = &c_cap[2];
                    format!("<pre{pre_attrs}><code class=\"language-{lang}\">{code_body}</code></pre>")
                } else {
                    format!("<pre{pre_attrs}><code class=\"language-{lang}\">{inner_content}</code></pre>")
                }
            } else {
                caps[0].to_string()
            }
        })
        .to_string()
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

    #[test]
    fn test_code_block_language_normalization() {
        let converter = MarkdownConverter::new();

        // 1. Test language-xxx on <pre>
        let html1 = r#"<pre class="language-rust"><code>fn main() {}</code></pre>"#;
        let md1 = converter.convert(html1, false, false).unwrap();
        assert!(md1.contains("```rust"));

        // 2. Test lang-py on <code>
        let html2 = r#"<pre><code class="lang-py">print("hello")</code></pre>"#;
        let md2 = converter.convert(html2, false, false).unwrap();
        assert!(md2.contains("```python"));

        // 3. Test data-lang on <pre>
        let html3 = r#"<pre data-lang="ts"><code>const x: number = 42;</code></pre>"#;
        let md3 = converter.convert(html3, false, false).unwrap();
        assert!(md3.contains("```typescript"));

        // 4. Test brush: java
        let html4 = r#"<pre class="brush: java">public class App {}</pre>"#;
        let md4 = converter.convert(html4, false, false).unwrap();
        assert!(md4.contains("```java"));
    }
}
