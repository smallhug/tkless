use anyhow::{Context, Result};
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use tkless_core::model::{OutputFormat, PageDocument};

/// Renders the extracted document according to the requested format and flags,
/// writing the payload to stdout or a designated output file.
pub fn render_output(
    doc: &PageDocument,
    format: OutputFormat,
    json_ld_only: bool,
    output_path: Option<&Path>,
    stream: bool,
) -> Result<()> {
    let payload = if json_ld_only {
        serde_json::to_string_pretty(&doc.json_ld)
            .context("Failed to serialize JSON-LD structured data")?
    } else {
        match format {
            OutputFormat::Json => {
                serde_json::to_string_pretty(doc).context("Failed to serialize document to JSON")?
            }
            OutputFormat::Markdown => {
                if let Some(ref chunks) = doc.chunks {
                    format_chunks_markdown(doc, chunks)
                } else {
                    doc.content.clone()
                }
            }
            OutputFormat::Text | OutputFormat::Raw | OutputFormat::Html => doc.content.clone(),
        }
    };

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
        "# ✂️ 语义切块输出: {} (共 {} 块)\n\n",
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
