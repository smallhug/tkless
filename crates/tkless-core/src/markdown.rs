use crate::error::TklessError;
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
static LINE_NUM_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*\d+\s*([|│:：]|\s{2,})\s*"#).unwrap());

static KATEX_DISPLAY_DIV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<div[^>]*class\s*=\s*["'][^"']*\bkatex-display\b[^"']*["'][^>]*>[\s\S]*?<annotation\s+encoding\s*=\s*["']application/x-tex["']>([\s\S]*?)</annotation>[\s\S]*?</div>"#).unwrap()
});
static KATEX_DISPLAY_SPAN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<span[^>]*class\s*=\s*["'][^"']*\bkatex-display\b[^"']*["'][^>]*>[\s\S]*?<annotation\s+encoding\s*=\s*["']application/x-tex["']>([\s\S]*?)</annotation>[\s\S]*?</span>"#).unwrap()
});
static KATEX_INLINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<span[^>]*class\s*=\s*["'][^"']*\bkatex\b[^"']*["'][^>]*>[\s\S]*?<annotation\s+encoding\s*=\s*["']application/x-tex["']>([\s\S]*?)</annotation>[\s\S]*?</span>"#).unwrap()
});
static MATHJAX_DISPLAY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?is)<script[^>]*type\s*=\s*["']math/tex;\s*mode=display["'][^>]*>([\s\S]*?)</script>"#,
    )
    .unwrap()
});
static MATHJAX_INLINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<script[^>]*type\s*=\s*["']math/tex["'][^>]*>([\s\S]*?)</script>"#).unwrap()
});
static MATH_DISPLAY_DIV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<(?:div|p)[^>]*class\s*=\s*["'][^"']*\bmath\s+display\b[^"']*["'][^>]*>\s*\\\[([\s\S]*?)\\\]\s*</(?:div|p)>"#).unwrap()
});
static MATH_INLINE_SPAN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<span[^>]*class\s*=\s*["'][^"']*\bmath\s+inline\b[^"']*["'][^>]*>\s*\\\(([\s\S]*?)\\\)\s*</span>"#).unwrap()
});

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
    ) -> Result<String, TklessError> {
        let mut math_protector = MathPlaceholderProtector::new();
        let with_math = math_protector.protect(html);
        let preprocessed = normalize_code_language_tags(&with_math);

        let mut md = self
            .converter
            .convert(&preprocessed)
            .map_err(|e| TklessError::ConversionError(e.to_string()))?;

        md = math_protector.restore(&md);

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

/// Helper for protecting math formulas using alphanumeric placeholders before conversion,
/// and restoring them unescaped to guarantee 100% LaTeX syntax fidelity.
struct MathPlaceholderProtector {
    formulas: Vec<(bool, String)>,
}

impl MathPlaceholderProtector {
    fn new() -> Self {
        Self {
            formulas: Vec::new(),
        }
    }

    fn protect(&mut self, html: &str) -> String {
        // 1. KaTeX display div blocks
        let s1 = KATEX_DISPLAY_DIV_RE.replace_all(html, |caps: &regex::Captures| {
            let formula = clean_tex_code(&caps[1]);
            let id = self.formulas.len();
            self.formulas.push((true, formula));
            format!("<p>TKLESSMATHBLOCK{id}</p>")
        });

        // 2. KaTeX display span blocks
        let s2 = KATEX_DISPLAY_SPAN_RE.replace_all(&s1, |caps: &regex::Captures| {
            let formula = clean_tex_code(&caps[1]);
            let id = self.formulas.len();
            self.formulas.push((true, formula));
            format!("<p>TKLESSMATHBLOCK{id}</p>")
        });

        // 3. MathJax display formula blocks
        let s3 = MATHJAX_DISPLAY_RE.replace_all(&s2, |caps: &regex::Captures| {
            let formula = clean_tex_code(&caps[1]);
            let id = self.formulas.len();
            self.formulas.push((true, formula));
            format!("<p>TKLESSMATHBLOCK{id}</p>")
        });

        // 4. Math display divs (\\[ ... \\])
        let s4 = MATH_DISPLAY_DIV_RE.replace_all(&s3, |caps: &regex::Captures| {
            let formula = clean_tex_code(&caps[1]);
            let id = self.formulas.len();
            self.formulas.push((true, formula));
            format!("<p>TKLESSMATHBLOCK{id}</p>")
        });

        // 5. KaTeX inline formula spans
        let s5 = KATEX_INLINE_RE.replace_all(&s4, |caps: &regex::Captures| {
            let formula = clean_tex_code(&caps[1]);
            let id = self.formulas.len();
            self.formulas.push((false, formula));
            format!(" TKLESSMATHINLINE{id} ")
        });

        // 6. MathJax inline formula scripts
        let s6 = MATHJAX_INLINE_RE.replace_all(&s5, |caps: &regex::Captures| {
            let formula = clean_tex_code(&caps[1]);
            let id = self.formulas.len();
            self.formulas.push((false, formula));
            format!(" TKLESSMATHINLINE{id} ")
        });

        // 7. Math inline spans (\\( ... \\))
        let s7 = MATH_INLINE_SPAN_RE.replace_all(&s6, |caps: &regex::Captures| {
            let formula = clean_tex_code(&caps[1]);
            let id = self.formulas.len();
            self.formulas.push((false, formula));
            format!(" TKLESSMATHINLINE{id} ")
        });

        s7.to_string()
    }

    fn restore(&self, markdown: &str) -> String {
        let mut result = markdown.to_string();
        for (id, (is_block, formula)) in self.formulas.iter().enumerate() {
            if *is_block {
                let token = format!("TKLESSMATHBLOCK{id}");
                let replacement = format!("\n\n$$\n{formula}\n$$\n\n");
                result = result.replace(&token, &replacement);
            } else {
                let token = format!("TKLESSMATHINLINE{id}");
                let replacement = format!("${formula}$");
                result = result.replace(&token, &replacement);
            }
        }
        result
    }
}

/// Preprocesses HTML to normalize KaTeX and MathJax math formulas into standard Markdown `$math$` and `$$math$$` blocks.
/// Standalone library helper; full conversion protects formulas until after HTML-to-Markdown conversion.
pub fn normalize_math_formulas(html: &str) -> String {
    let mut protector = MathPlaceholderProtector::new();
    let protected = protector.protect(html);
    protector.restore(&protected)
}

fn clean_tex_code(raw: &str) -> String {
    raw.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
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

/// Preprocesses HTML to guarantee code blocks receive standardized `<code class="language-<canonical>">` tags
/// and strips distracting line numbering prefixes (e.g. `1 | `, `01: `).
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

            let raw_body = if let Some(ref c_cap) = code_match {
                &c_cap[2]
            } else {
                inner_content
            };
            let cleaned_body = strip_code_line_numbers(raw_body);

            if let Some(lang) = detected_lang {
                format!(
                    "<pre{pre_attrs}><code class=\"language-{lang}\">{cleaned_body}</code></pre>"
                )
            } else if let Some(ref c_cap) = code_match {
                let code_attrs = &c_cap[1];
                format!("<pre{pre_attrs}><code{code_attrs}>{cleaned_body}</code></pre>")
            } else {
                format!("<pre{pre_attrs}><code>{cleaned_body}</code></pre>")
            }
        })
        .to_string()
}

/// Strips leading code line numbers (e.g. `1 | `, `01: `, `1 │ `) commonly injected into blog code blocks.
pub fn strip_code_line_numbers(code: &str) -> String {
    let lines: Vec<&str> = code.lines().collect();
    if lines.len() < 2 {
        return code.to_string();
    }

    let mut numbered_count = 0;
    let mut non_empty_count = 0;

    for line in &lines {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            non_empty_count += 1;
            if LINE_NUM_PREFIX_RE.is_match(line) {
                numbered_count += 1;
            }
        }
    }

    if non_empty_count >= 2 && (numbered_count as f64 / non_empty_count as f64) >= 0.7 {
        let mut cleaned_lines = Vec::with_capacity(lines.len());
        for line in lines {
            if let Some(mat) = LINE_NUM_PREFIX_RE.find(line) {
                cleaned_lines.push(&line[mat.end()..]);
            } else {
                cleaned_lines.push(line);
            }
        }
        cleaned_lines.join("\n")
    } else {
        code.to_string()
    }
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

    #[test]
    fn test_math_formula_normalization() {
        let converter = MarkdownConverter::new();

        // 1. KaTeX inline formula
        let katex_inline = r#"<p>The equation is <span class="katex"><span class="katex-mathml"><math><semantics><mrow><mi>E</mi><mo>=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></mrow><annotation encoding="application/x-tex">E = mc^2</annotation></semantics></math></span><span class="katex-html" aria-hidden="true">E=mc^2</span></span> in relativity.</p>"#;
        let md1 = converter.convert(katex_inline, false, false).unwrap();
        assert!(md1.contains("$E = mc^2$"));

        // 2. KaTeX display formula
        let katex_display = r#"<div class="katex-display"><span class="katex"><span class="katex-mathml"><annotation encoding="application/x-tex">\int_0^\infty e^{-x^2} dx = \frac{\sqrt{\pi}}{2}</annotation></span></span></div>"#;
        let md2 = converter.convert(katex_display, false, false).unwrap();
        assert!(md2.contains("$$\n\\int_0^\\infty e^{-x^2} dx = \\frac{\\sqrt{\\pi}}{2}\n$$"));

        // 3. MathJax script
        let mathjax_inline =
            r#"<p>Let <script type="math/tex">f(x) = x^2 + 1</script> be continuous.</p>"#;
        let md3 = converter.convert(mathjax_inline, false, false).unwrap();
        assert!(md3.contains("$f(x) = x^2 + 1$"));
    }

    #[test]
    fn test_strip_code_line_numbers() {
        let converter = MarkdownConverter::new();
        let sample = r#"
            <pre class="language-rust"><code>1 | fn main() {
2 |     println!("Hello, World!");
3 | }</code></pre>
        "#;
        let md = converter.convert(sample, false, false).unwrap();
        assert!(md.contains("```rust"));
        assert!(md.contains("fn main() {"));
        assert!(md.contains("println!(\"Hello, World!\");"));
        assert!(!md.contains("1 |"));
        assert!(!md.contains("2 |"));
        assert!(!md.contains("3 |"));
    }
}
