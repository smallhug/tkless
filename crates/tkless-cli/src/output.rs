use crate::cli::CliFormat;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use tkless_core::model::PageDocument;

/// Renders the extracted document according to the requested format and flags,
/// writing the payload to stdout or a designated output file.
pub fn render_output(
    doc: &PageDocument,
    format: CliFormat,
    json_ld_only: bool,
    output_path: Option<&Path>,
    stream: bool,
    options: &tkless_core::ExtractionOptions,
) -> Result<()> {
    let mut payload = if json_ld_only {
        serialize_json(&doc.json_ld, format)?
    } else {
        match format {
            CliFormat::Jsonl => {
                serde_json::to_string(doc).context("Failed to serialize JSONL document")?
            }
            CliFormat::Json => {
                serde_json::to_string_pretty(doc).context("Failed to serialize document to JSON")?
            }
            CliFormat::Markdown => {
                let mut text = if let Some(ref chunks) = doc.chunks {
                    format_chunks_markdown(doc, chunks)
                } else {
                    doc.content.clone()
                };

                if !text.contains("## Links") {
                    if let Some(ref link_list) = doc.links {
                        text.push_str(&tkless_core::format_links_markdown(link_list));
                    }
                }

                if !text.contains("## Images") {
                    if let Some(ref img_list) = doc.images {
                        text.push_str(&tkless_core::format_images_markdown(img_list));
                    }
                }

                text
            }
            CliFormat::Text | CliFormat::Raw | CliFormat::Html => doc.content.clone(),
        }
    };

    if !json_ld_only && matches!(format, CliFormat::Markdown | CliFormat::Text) {
        if let Some(limit) = options.max_tokens {
            payload = tkless_core::token::truncate_to_tokens(&payload, limit, options.tokenizer).0;
        }
    }

    write_payload_streaming(&payload, output_path, stream)
}

/// Renders cover image URL directly to stdout or output file.
pub fn render_cover(cover_url: &str, output_path: Option<&Path>) -> Result<()> {
    write_payload(cover_url, output_path)
}

/// Writes arbitrary payload text to stdout or a designated output file.
pub fn write_payload(payload: &str, output_path: Option<&Path>) -> Result<()> {
    write_payload_streaming(payload, output_path, false)
}

/// Writes arbitrary payload text to stdout or a designated output file with optional real-time line streaming.
pub fn write_payload_streaming(
    payload: &str,
    output_path: Option<&Path>,
    stream: bool,
) -> Result<()> {
    if let Some(path) = output_path {
        let mut file = File::create(path)
            .with_context(|| format!("Failed to create output file: {}", path.display()))?;
        file.write_all(payload.as_bytes())
            .with_context(|| format!("Failed to write output to file: {}", path.display()))?;
        file.write_all(b"\n")?;
    } else if stream {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        for line in payload.split_inclusive('\n') {
            handle.write_all(line.as_bytes())?;
            handle.flush()?;
        }
        if !payload.ends_with('\n') {
            handle.write_all(b"\n")?;
            handle.flush()?;
        }
    } else {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        handle.write_all(payload.as_bytes())?;
        handle.write_all(b"\n")?;
        handle.flush()?;
    }

    Ok(())
}

/// Formats extracted document chunks into a human and agent readable Markdown document.
fn format_chunks_markdown(
    doc: &PageDocument,
    chunks: &[tkless_core::model::DocumentChunk],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# 语义切块输出: {} (共 {} 块)\n\n",
        doc.title.as_deref().unwrap_or(&doc.url),
        chunks.len()
    ));

    for chunk in chunks {
        let path_str = if chunk.heading_path.is_empty() {
            "根节点".to_string()
        } else {
            chunk.heading_path.join(" > ")
        };

        out.push_str(&format!(
            "### [Chunk {}/{}] (Tokens: {}) | 标题路径: {}\n\n",
            chunk.index + 1,
            chunks.len(),
            chunk.token_count,
            path_str
        ));
        out.push_str(chunk.content.trim());
        out.push_str("\n\n---\n\n");
    }

    out
}

pub fn serialize_json<T: serde::Serialize>(value: &T, format: CliFormat) -> Result<String> {
    Ok(if matches!(format, CliFormat::Jsonl) {
        serde_json::to_string(value)?
    } else {
        serde_json::to_string_pretty(value)?
    })
}

pub fn serialize_rows<T: serde::Serialize>(values: &[T], format: CliFormat) -> Result<String> {
    if matches!(format, CliFormat::Jsonl) {
        Ok(values
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .join("\n"))
    } else {
        Ok(serde_json::to_string_pretty(values)?)
    }
}

pub fn apply_budget(
    payload: String,
    format: CliFormat,
    limit: Option<usize>,
    tokenizer: tkless_core::TokenizerKind,
) -> Result<String> {
    let Some(limit) = limit else {
        return Ok(payload);
    };
    if tkless_core::token::count_tokens(&payload, tokenizer) <= limit {
        return Ok(payload);
    }
    if matches!(format, CliFormat::Json | CliFormat::Jsonl) {
        return Err(tkless_core::TklessError::InvalidArgument("Serialized JSON exceeds the total token budget; increase the budget or reduce the page/result limit".into()).into());
    }
    Ok(tkless_core::token::truncate_to_tokens(&payload, limit, tokenizer).0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jsonl_has_one_valid_object_per_line() {
        let items = vec![
            serde_json::json!({"text":"line one\nline two"}),
            serde_json::json!({"ok":true}),
        ];
        let output = serialize_rows(&items, CliFormat::Jsonl).unwrap();
        let decoded: Vec<serde_json::Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(decoded, items);
    }

    #[test]
    fn structured_budget_never_returns_corrupted_json() {
        assert!(apply_budget(
            "{\"text\":\"too long\"}".into(),
            CliFormat::Json,
            Some(1),
            tkless_core::TokenizerKind::O200k
        )
        .is_err());
    }
}
