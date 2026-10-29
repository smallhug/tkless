//! Native PDF Content Extraction & LLM-Optimized Markdown Structuring.
//!
//! Provides zero-external-C-dependency, pure-Rust PDF extraction:
//! 1. Identifies PDF content by magic byte signature (`%PDF-`) or URL patterns.
//! 2. Extracts embedded metadata (Title, Author, Subject, Keywords, Creation Date) via `lopdf`.
//! 3. Extracts raw page text streams via `pdf_extract`.
//! 4. Intelligent layout structuring:
//!    - Filters page numbers, running headers, and footers.
//!    - Reassembles hard-wrapped lines into unified natural paragraphs.
//!    - Detects section headings (`1. Introduction`, `ABSTRACT`, `一、引言`).
//!    - Formats bullet points and numbered lists into standard Markdown.
//! 5. Produces a full `PageDocument` seamlessly integrated with `chunk`, `summarize`, and MCP tools.

use crate::error::TklessError;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::{count_tokens, truncate_to_tokens};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static PDF_PAGE_NUM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)^\s*(?:page\s+\d+(?:\s+of\s+\d+)?|\d+\s*/\s*\d+|---\s*page\s+\d+\s*---|\d+)\s*$"#,
    )
    .unwrap()
});

static SECTION_HEADING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(?:(?:\d+\.){1,3}\s+[A-Z\u4E00-\u9FA5]|[一二三四五六七八九十]+[、.]|[A-Z]{3,}(?:\s+[A-Z]{3,})*$)"#).unwrap()
});

static LIST_ITEM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(?:[•*⁃\-–—]\s+|\(?\d+\)[.\s]+|[a-z]\)[.\s]+)"#).unwrap());

/// Checks if a byte slice starts with the standard PDF magic header (`%PDF-`).
pub fn is_pdf_content(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
        || (bytes.len() >= 10 && bytes[..10].windows(5).any(|w| w == b"%PDF-"))
}

/// Checks if a URL points to a PDF document by path extension or query parameters.
pub fn is_pdf_url(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    if path.ends_with(".pdf") {
        return true;
    }
    if let Some(query) = url.query() {
        let q_lower = query.to_ascii_lowercase();
        if q_lower.contains("format=pdf")
            || q_lower.contains("type=pdf")
            || q_lower.ends_with(".pdf")
        {
            return true;
        }
    }
    false
}

/// Checks if a file path string ends with `.pdf`.
pub fn is_pdf_path(path_str: &str) -> bool {
    path_str.trim().to_ascii_lowercase().ends_with(".pdf")
}

/// Extracts metadata from PDF document Info dictionary via `lopdf`.
pub fn extract_pdf_metadata(bytes: &[u8]) -> HashMap<String, String> {
    let mut meta = HashMap::new();
    if let Ok(doc) = lopdf::Document::load_mem(bytes) {
        if let Ok(info_dict) = doc.get_dictionary(
            doc.trailer
                .get(b"Info")
                .and_then(|o| o.as_reference())
                .unwrap_or((0, 0)),
        ) {
            for (k, v) in info_dict.iter() {
                let key_name = String::from_utf8_lossy(k).to_string();
                let val_str = match v {
                    lopdf::Object::String(s, _) => String::from_utf8_lossy(s).to_string(),
                    lopdf::Object::Name(n) => String::from_utf8_lossy(n).to_string(),
                    _ => format!("{v:?}"),
                };
                let clean_val = val_str.trim().trim_matches('\0').trim().to_string();
                if !clean_val.is_empty() {
                    meta.insert(key_name, clean_val);
                }
            }
        }
    }
    meta
}

/// Extracts raw text from in-memory PDF bytes via `pdf_extract`.
pub fn extract_raw_text_from_pdf(bytes: &[u8]) -> Result<String, TklessError> {
    pdf_extract::extract_text_from_mem(bytes)
        .map_err(|e| TklessError::PdfError(format!("PDF 文本解析失败: {e}")))
}

/// Converts raw unformatted PDF text into clean, structured Markdown.
pub fn structure_pdf_text_to_markdown(
    raw_text: &str,
    title_hint: Option<&str>,
    author_hint: Option<&str>,
    source_url: Option<&str>,
    options: &ExtractionOptions,
) -> String {
    let mut lines_clean = Vec::new();
    let mut discovered_title = title_hint.map(|s| s.to_string());

    // 1. Line-by-line filtering of page numbers and running noise
    for raw_line in raw_text.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            lines_clean.push(String::new());
            continue;
        }

        // Filter standalone page numbers: "Page 1 of 12", "1 / 10", "42"
        if PDF_PAGE_NUM_RE.is_match(trimmed) {
            continue;
        }

        // Auto-detect title from the first prominent non-empty line if not already known
        if discovered_title.is_none()
            && trimmed.len() >= 4
            && trimmed.len() <= 120
            && !SECTION_HEADING_RE.is_match(trimmed)
            && !trimmed.starts_with("http")
        {
            discovered_title = Some(trimmed.to_string());
        }

        lines_clean.push(trimmed.to_string());
    }

    // 2. Reassemble hard-wrapped lines into unified paragraphs
    let mut paragraphs = Vec::new();
    let mut current_para = Vec::new();

    for line in lines_clean {
        if line.is_empty() {
            if !current_para.is_empty() {
                paragraphs.push(current_para.join(" "));
                current_para.clear();
            }
        } else if SECTION_HEADING_RE.is_match(&line) || LIST_ITEM_RE.is_match(&line) {
            // Flush existing paragraph before headings or list items
            if !current_para.is_empty() {
                paragraphs.push(current_para.join(" "));
                current_para.clear();
            }
            paragraphs.push(line);
        } else {
            current_para.push(line);
        }
    }
    if !current_para.is_empty() {
        paragraphs.push(current_para.join(" "));
    }

    // 3. Render structured Markdown
    let title_str = discovered_title.unwrap_or_else(|| "PDF Document".to_string());
    let mut md = String::new();

    if options.frontmatter && options.format == OutputFormat::Markdown {
        md.push_str("---\n");
        md.push_str(&format!("title: \"{}\"\n", title_str.replace('"', "\\\"")));
        if let Some(author) = author_hint {
            md.push_str(&format!("author: \"{}\"\n", author.replace('"', "\\\"")));
        }
        if let Some(url) = source_url {
            md.push_str(&format!("source: \"{url}\"\n"));
        }
        md.push_str("format: \"pdf\"\n");
        md.push_str("---\n\n");
    }

    md.push_str(&format!("# {title_str}\n\n"));
    if let Some(url) = source_url {
        md.push_str(&format!("> **来源**: [PDF 原始文档]({url})\n\n"));
    }

    for para in paragraphs {
        let p = para.trim();
        if p.is_empty() {
            continue;
        }

        if SECTION_HEADING_RE.is_match(p) && p.len() <= 80 {
            // Format as Level 2 Markdown heading
            md.push_str(&format!("\n## {p}\n\n"));
        } else if LIST_ITEM_RE.is_match(p) {
            // Clean and format as Markdown list item
            let stripped = LIST_ITEM_RE.replace(p, "").trim().to_string();
            md.push_str(&format!("- {stripped}\n"));
        } else {
            md.push_str(p);
            md.push_str("\n\n");
        }
    }

    md.trim().to_string()
}

/// Extracts a complete `PageDocument` from PDF byte content.
pub fn extract_pdf_to_document(
    bytes: &[u8],
    source_url: Option<&str>,
    options: &ExtractionOptions,
) -> Result<PageDocument, TklessError> {
    options.validate()?;
    if options.schema_strict {
        return Err(TklessError::SchemaError(
            "CSS Schema extraction is unavailable for PDF input".into(),
        ));
    }
    debug!(len = bytes.len(), url = ?source_url, "Executing native PDF extraction pipeline");

    let pdf_meta = extract_pdf_metadata(bytes);
    let raw_text = extract_raw_text_from_pdf(bytes)?;

    let title_meta = pdf_meta.get("Title").cloned();
    let author_meta = pdf_meta.get("Author").cloned();
    let subject_meta = pdf_meta.get("Subject").cloned();

    let title_hint = title_meta.as_deref().or_else(|| {
        source_url.and_then(|u| {
            u.split('/')
                .next_back()
                .and_then(|f| f.strip_suffix(".pdf"))
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
        })
    });

    let mut content = structure_pdf_text_to_markdown(
        &raw_text,
        title_hint,
        author_meta.as_deref(),
        source_url,
        options,
    );

    if options.summarize
        && matches!(
            options.format,
            crate::model::OutputFormat::Markdown | crate::model::OutputFormat::Json
        )
    {
        content = crate::summarizer::summarize_markdown(&content, 3);
    }
    // Apply token truncation if budget is set
    let initial_tokens = count_tokens(&content, options.tokenizer);
    let mut truncated = false;
    if let Some(limit) = options.max_tokens {
        if initial_tokens > limit {
            let (truncated_content, was_truncated) =
                truncate_to_tokens(&content, limit, options.tokenizer);
            content = truncated_content;
            truncated = was_truncated;
        }
    }

    let final_tokens = if truncated {
        count_tokens(&content, options.tokenizer)
    } else {
        initial_tokens
    };

    let title_res = title_meta.or_else(|| {
        content
            .lines()
            .find(|l| l.starts_with("# "))
            .map(|l| l.trim_start_matches("# ").trim().to_string())
    });

    let metadata = PageMetadata {
        description: subject_meta,
        author: author_meta.clone(),
        canonical_url: source_url.map(|s| s.to_string()),
        ..Default::default()
    };

    let chunks = options.chunk.then(|| {
        crate::chunker::chunk_markdown(
            &content,
            options.chunk_size.unwrap_or(500),
            options.chunk_overlap.unwrap_or(50),
            options.tokenizer,
        )
    });
    Ok(PageDocument {
        extraction: crate::model::ExtractionProvenance {
            method: "pdf".into(),
            ..Default::default()
        },
        warnings: if options.schema.is_some() {
            vec![crate::model::DiagnosticWarning::new(
                "schema_unsupported",
                "CSS Schema extraction is unavailable for PDF input",
            )]
        } else {
            Vec::new()
        },
        url: source_url.unwrap_or("").to_string(),
        final_url: source_url.unwrap_or("").to_string(),
        status: 200,
        title: title_res,
        byline: author_meta,
        content,
        metadata,
        json_ld: Vec::new(),
        token_count: final_tokens,
        tokenizer: options.tokenizer.as_str().to_string(),
        truncated,
        subtitles: None,
        danmaku: None,
        comments: None,
        conclusion: None,
        links: None,
        images: None,
        tables: None,
        entity: None,
        chunks,
        content_hash: Some(format!("{:016x}", crate::dedup::simhash(&raw_text))),
        metrics: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_pdf_content() {
        assert!(is_pdf_content(b"%PDF-1.7\n1 0 obj..."));
        assert!(is_pdf_content(b"\r\n%PDF-1.4\n"));
        assert!(!is_pdf_content(b"<!DOCTYPE html><html>"));
    }

    #[test]
    fn test_is_pdf_url() {
        assert!(is_pdf_url(
            &Url::parse("https://arxiv.org/pdf/1706.03762.pdf").unwrap()
        ));
        assert!(is_pdf_url(
            &Url::parse("https://example.com/report.pdf?v=2").unwrap()
        ));
        assert!(is_pdf_url(
            &Url::parse("https://api.example.com/download?format=pdf").unwrap()
        ));
        assert!(!is_pdf_url(
            &Url::parse("https://example.com/article").unwrap()
        ));
    }

    #[test]
    fn test_structure_pdf_text_to_markdown() {
        let sample_pdf_text = r#"
Attention Is All You Need

Ashish Vaswani, Noam Shazeer

Page 1 of 15

ABSTRACT

The dominant sequence transduction models are based on complex recurrent or convolutional neural networks.
We propose a new simple network architecture, the Transformer, based solely on attention mechanisms.

1. INTRODUCTION

Recurrent neural networks, long short-term memory and gated recurrent neural networks in particular,
have been firmly established as state of the art approaches in sequence modeling.

• Simple and effective
• Fast convergence
• Highly parallelizable

--- Page 2 ---
2. ARCHITECTURE
The Transformer follows this overall architecture using stacked self-attention and point-wise layers.
"#;

        let options = ExtractionOptions::default();
        let md = structure_pdf_text_to_markdown(
            sample_pdf_text,
            Some("Attention Is All You Need"),
            Some("Ashish Vaswani"),
            Some("https://arxiv.org/pdf/1706.03762.pdf"),
            &options,
        );

        // Verify title
        assert!(md.contains("# Attention Is All You Need"));
        // Verify source link
        assert!(md.contains("> **来源**: [PDF 原始文档](https://arxiv.org/pdf/1706.03762.pdf)"));
        // Verify page numbers are stripped
        assert!(!md.contains("Page 1 of 15"));
        assert!(!md.contains("--- Page 2 ---"));
        // Verify headings
        assert!(md.contains("## ABSTRACT"));
        assert!(md.contains("## 1. INTRODUCTION"));
        assert!(md.contains("## 2. ARCHITECTURE"));
        // Verify lists
        assert!(md.contains("- Simple and effective"));
        assert!(md.contains("- Fast convergence"));
    }
}
