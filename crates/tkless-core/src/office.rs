//! Native DOCX/PPTX extraction without archive or XML dependencies.
//!
//! Office Open XML files are ZIP containers with a small set of XML parts. This module
//! implements a bounded stored/deflate ZIP reader and text-oriented XML scanning. It is
//! intentionally not a general Office renderer: formatting, images, charts and embedded
//! objects are dropped, while headings, lists and paragraph text are preserved.
use crate::error::TklessError;
use crate::model::{ExtractionOptions, PageDocument, PageMetadata};
use std::io::Read;

/// Supported Office Open XML document types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeKind {
    Docx,
    Pptx,
    Xlsx,
}

const MAX_ENTRIES: usize = 512;
const MAX_ENTRY_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const EOCD_SIGNATURE: &[u8] = b"PK\x05\x06";
const CENTRAL_SIGNATURE: &[u8] = b"PK\x01\x02";
const LOCAL_SIGNATURE: &[u8] = b"PK\x03\x04";

/// Detects an Office Open XML document from the content type, URL suffix and ZIP magic.
///
/// A plain ZIP archive is not treated as Office content: either the media type or the
/// final URL must identify the format.
pub fn detect_office(
    bytes: &[u8],
    content_type: Option<&str>,
    url: Option<&str>,
) -> Option<OfficeKind> {
    if !bytes.starts_with(LOCAL_SIGNATURE) {
        return None;
    }
    let media = content_type.unwrap_or_default().to_ascii_lowercase();
    let path = url
        .unwrap_or_default()
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if media.contains("wordprocessingml") || path.ends_with(".docx") {
        Some(OfficeKind::Docx)
    } else if media.contains("presentationml") || path.ends_with(".pptx") {
        Some(OfficeKind::Pptx)
    } else if media.contains("spreadsheetml") || path.ends_with(".xlsx") {
        Some(OfficeKind::Xlsx)
    } else {
        None
    }
}

/// Extracts a DOCX or PPTX document into Markdown content.
pub fn extract_office_to_document(
    bytes: &[u8],
    kind: OfficeKind,
    source_url: Option<&str>,
    options: &ExtractionOptions,
) -> Result<PageDocument, TklessError> {
    options.validate()?;
    if options.schema_strict {
        return Err(TklessError::SchemaError(
            "CSS Schema extraction is unavailable for Office documents".into(),
        ));
    }
    if kind == OfficeKind::Xlsx {
        return Err(TklessError::OfficeError(
            "XLSX extraction is not supported; export the sheet as CSV first".into(),
        ));
    }

    let archive = ZipArchive::parse(bytes)?;
    let (raw_text, title) = match kind {
        OfficeKind::Docx => extract_docx(&archive)?,
        OfficeKind::Pptx => extract_pptx(&archive)?,
        OfficeKind::Xlsx => unreachable!("rejected above"),
    };
    let content_hash = format!("{:016x}", crate::dedup::simhash(&raw_text));
    let mut content = raw_text;
    if options.summarize
        && matches!(
            options.format,
            crate::model::OutputFormat::Markdown | crate::model::OutputFormat::Json
        )
    {
        content = crate::summarizer::summarize_markdown(&content, 3);
    }

    let initial_tokens = crate::token::count_tokens(&content, options.tokenizer);
    let mut truncated = false;
    if let Some(limit) = options.max_tokens {
        if initial_tokens > limit {
            let (truncated_content, was_truncated) =
                crate::token::truncate_to_tokens(&content, limit, options.tokenizer);
            content = truncated_content;
            truncated = was_truncated;
        }
    }
    let final_tokens = if truncated {
        crate::token::count_tokens(&content, options.tokenizer)
    } else {
        initial_tokens
    };
    let title = title.or_else(|| {
        content
            .lines()
            .find(|line| line.starts_with("# "))
            .map(|line| line.trim_start_matches("# ").trim().to_string())
    });

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
            method: "office".into(),
            ..Default::default()
        },
        warnings: if options.schema.is_some() {
            vec![crate::model::DiagnosticWarning::new(
                "schema_unsupported",
                "CSS Schema extraction is unavailable for Office documents",
            )]
        } else {
            Vec::new()
        },
        url: source_url.unwrap_or("").to_string(),
        final_url: source_url.unwrap_or("").to_string(),
        status: 200,
        title,
        byline: None,
        content,
        metadata: PageMetadata {
            canonical_url: source_url.map(str::to_string),
            ..Default::default()
        },
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
        content_hash: Some(content_hash),
        metrics: None,
    })
}

fn extract_docx(archive: &ZipArchive) -> Result<(String, Option<String>), TklessError> {
    let document = archive
        .read_text("word/document.xml")?
        .ok_or_else(|| TklessError::OfficeError("DOCX is missing word/document.xml".into()))?;
    let title = archive
        .read_text("docProps/core.xml")?
        .and_then(|core| element_text(&core, "dc:title"))
        .filter(|title| !title.is_empty());

    let paragraph_re = regex::Regex::new(r"(?s)<w:p(?:\s[^>]*)?>(.*?)</w:p>").expect("valid regex");
    let text_re = regex::Regex::new(r"(?s)<w:t(?:\s[^>]*)?>(.*?)</w:t>").expect("valid regex");
    let heading_re = regex::Regex::new(r#"w:val="Heading([1-6])""#).expect("valid regex");
    let mut blocks = Vec::new();
    for paragraph in paragraph_re.captures_iter(&document) {
        let body = &paragraph[1];
        let mut text = String::new();
        for run in text_re.captures_iter(body) {
            text.push_str(&decode_xml_text(&run[1]));
        }
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let heading = heading_re
            .captures(body)
            .and_then(|capture| capture[1].parse::<usize>().ok())
            .filter(|level| (1..=6).contains(level));
        let is_list = body.contains("<w:numPr");
        if let Some(level) = heading {
            blocks.push(format!("{} {}", "#".repeat(level), text));
        } else if is_list {
            blocks.push(format!("- {text}"));
        } else {
            blocks.push(text.to_string());
        }
    }
    Ok((blocks.join("\n\n"), title))
}

fn extract_pptx(archive: &ZipArchive) -> Result<(String, Option<String>), TklessError> {
    let mut slides = archive.slide_names()?;
    slides.sort_by_key(|(index, _)| *index);
    if slides.is_empty() {
        return Err(TklessError::OfficeError(
            "PPTX contains no readable slides".into(),
        ));
    }

    let paragraph_re = regex::Regex::new(r"(?s)<a:p>(.*?)</a:p>").expect("valid regex");
    let text_re = regex::Regex::new(r"(?s)<a:t(?:\s[^>]*)?>(.*?)</a:t>").expect("valid regex");
    let mut blocks = Vec::new();
    let mut title = None;
    for (index, name) in slides {
        let xml = archive.read_text(&name)?.ok_or_else(|| {
            TklessError::OfficeError(format!("PPTX slide '{name}' is unreadable"))
        })?;
        let mut lines = Vec::new();
        for paragraph in paragraph_re.captures_iter(&xml) {
            let mut text = String::new();
            for run in text_re.captures_iter(&paragraph[1]) {
                text.push_str(&decode_xml_text(&run[1]));
            }
            let text = text.trim();
            if !text.is_empty() {
                lines.push(text.to_string());
            }
        }
        if lines.is_empty() {
            continue;
        }
        if title.is_none() {
            title = Some(lines[0].clone());
        }
        blocks.push(format!("## Slide {index}\n\n{}", lines.join("\n\n")));
    }
    if blocks.is_empty() {
        return Err(TklessError::OfficeError(
            "PPTX contains no readable text".into(),
        ));
    }
    Ok((blocks.join("\n\n"), title))
}

fn element_text(xml: &str, tag: &str) -> Option<String> {
    let pattern = format!(r"(?s)<{tag}(?:\s[^>]*)?>(.*?)</{tag}>");
    let regex = regex::Regex::new(&pattern).ok()?;
    regex
        .captures(xml)
        .map(|capture| decode_xml_text(&capture[1]).trim().to_string())
}

fn decode_xml_text(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '&' {
            decoded.push(character);
            continue;
        }
        let mut entity = String::new();
        let mut terminated = false;
        while let Some(&next) = chars.peek() {
            if next == ';' {
                chars.next();
                terminated = true;
                break;
            }
            if entity.len() >= 10 || next.is_whitespace() || next == '&' || next == '<' {
                break;
            }
            entity.push(next);
            chars.next();
        }
        let replacement = if terminated {
            match entity.as_str() {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => {
                    entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                }
                _ => None,
            }
        } else {
            None
        };
        match replacement {
            Some(character) => decoded.push(character),
            None => {
                decoded.push('&');
                decoded.push_str(&entity);
                if terminated {
                    decoded.push(';');
                }
            }
        }
    }
    decoded
}

struct ZipEntry {
    name: String,
    method: u16,
    compressed_size: usize,
    uncompressed_size: usize,
    local_offset: usize,
}

struct ZipArchive<'a> {
    bytes: &'a [u8],
    entries: Vec<ZipEntry>,
}

impl<'a> ZipArchive<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, TklessError> {
        let eocd = find_eocd(bytes).ok_or_else(|| {
            TklessError::OfficeError("not a ZIP archive (missing end of central directory)".into())
        })?;
        let count = read_u16(bytes, eocd + 10)? as usize;
        let central_offset = read_u32(bytes, eocd + 16)? as usize;
        if count == 0 || count > MAX_ENTRIES {
            return Err(TklessError::OfficeError(format!(
                "ZIP entry count {count} is outside 1..={MAX_ENTRIES}"
            )));
        }
        if central_offset == u32::MAX as usize {
            return Err(TklessError::OfficeError(
                "ZIP64 archives are not supported".into(),
            ));
        }

        let mut entries = Vec::with_capacity(count);
        let mut cursor = central_offset;
        let mut total = 0usize;
        for _ in 0..count {
            if slice(bytes, cursor, 4)? != CENTRAL_SIGNATURE {
                return Err(TklessError::OfficeError(
                    "corrupt ZIP central directory".into(),
                ));
            }
            let flags = read_u16(bytes, cursor + 8)?;
            let method = read_u16(bytes, cursor + 10)?;
            let compressed_size = read_u32(bytes, cursor + 20)? as usize;
            let uncompressed_size = read_u32(bytes, cursor + 24)? as usize;
            let name_len = read_u16(bytes, cursor + 28)? as usize;
            let extra_len = read_u16(bytes, cursor + 30)? as usize;
            let comment_len = read_u16(bytes, cursor + 32)? as usize;
            let local_offset = read_u32(bytes, cursor + 42)? as usize;
            let name_end = cursor
                .checked_add(46 + name_len)
                .ok_or_else(zip_offset_error)?;
            let name = String::from_utf8_lossy(slice(bytes, cursor + 46, name_len)?).into_owned();
            cursor = name_end
                .checked_add(extra_len + comment_len)
                .ok_or_else(zip_offset_error)?;

            if flags & 1 != 0 {
                return Err(TklessError::OfficeError(format!(
                    "encrypted ZIP entry '{name}' is not supported"
                )));
            }
            if compressed_size == u32::MAX as usize
                || uncompressed_size == u32::MAX as usize
                || local_offset == u32::MAX as usize
            {
                return Err(TklessError::OfficeError(
                    "ZIP64 archives are not supported".into(),
                ));
            }
            if uncompressed_size > MAX_ENTRY_BYTES {
                return Err(TklessError::OfficeError(format!(
                    "ZIP entry '{name}' exceeds {MAX_ENTRY_BYTES} bytes"
                )));
            }
            total = total
                .checked_add(uncompressed_size)
                .ok_or_else(zip_offset_error)?;
            if total > MAX_TOTAL_BYTES {
                return Err(TklessError::OfficeError(
                    "ZIP uncompressed content exceeds the extraction budget".into(),
                ));
            }
            entries.push(ZipEntry {
                name,
                method,
                compressed_size,
                uncompressed_size,
                local_offset,
            });
        }
        Ok(Self { bytes, entries })
    }

    fn read_text(&self, name: &str) -> Result<Option<String>, TklessError> {
        let Some(entry) = self.entries.iter().find(|entry| entry.name == name) else {
            return Ok(None);
        };
        if slice(self.bytes, entry.local_offset, 4)? != LOCAL_SIGNATURE {
            return Err(TklessError::OfficeError(format!(
                "corrupt ZIP local header for '{name}'"
            )));
        }
        let name_len = read_u16(self.bytes, entry.local_offset + 26)? as usize;
        let extra_len = read_u16(self.bytes, entry.local_offset + 28)? as usize;
        let start = entry
            .local_offset
            .checked_add(30 + name_len + extra_len)
            .ok_or_else(zip_offset_error)?;
        let compressed = slice(self.bytes, start, entry.compressed_size)?;
        let raw = match entry.method {
            0 => compressed.to_vec(),
            8 => {
                let mut decoded = Vec::with_capacity(entry.uncompressed_size);
                flate2::read::DeflateDecoder::new(compressed)
                    .take(MAX_ENTRY_BYTES as u64 + 1)
                    .read_to_end(&mut decoded)?;
                decoded
            }
            method => {
                return Err(TklessError::OfficeError(format!(
                    "unsupported ZIP compression method {method} for '{name}'"
                )))
            }
        };
        if raw.len() != entry.uncompressed_size {
            return Err(TklessError::OfficeError(format!(
                "ZIP entry '{name}' has an unexpected uncompressed size"
            )));
        }
        String::from_utf8(raw)
            .map(Some)
            .map_err(|_| TklessError::OfficeError(format!("ZIP entry '{name}' is not UTF-8 XML")))
    }

    fn slide_names(&self) -> Result<Vec<(usize, String)>, TklessError> {
        let regex = regex::Regex::new(r"^ppt/slides/slide(\d+)\.xml$").expect("valid regex");
        let mut slides = Vec::new();
        for entry in &self.entries {
            if let Some(capture) = regex.captures(&entry.name) {
                if let Ok(index) = capture[1].parse::<usize>() {
                    slides.push((index, entry.name.clone()));
                }
            }
        }
        Ok(slides)
    }
}

fn find_eocd(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 22 {
        return None;
    }
    let start = bytes.len().saturating_sub(22 + u16::MAX as usize);
    (start..=bytes.len() - 22)
        .rev()
        .find(|offset| &bytes[*offset..*offset + 4] == EOCD_SIGNATURE)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, TklessError> {
    let value = slice(bytes, offset, 2)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, TklessError> {
    let value = slice(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn slice(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8], TklessError> {
    let end = offset.checked_add(length).ok_or_else(zip_offset_error)?;
    bytes
        .get(offset..end)
        .ok_or_else(|| TklessError::OfficeError("truncated ZIP structure".into()))
}

fn zip_offset_error() -> TklessError {
    TklessError::OfficeError("ZIP offset overflow".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(entries: &[(&str, &str)]) -> Vec<u8> {
        let path = std::env::temp_dir().join(format!(
            "tkless-office-{}-{}.zip",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let entries: Vec<(String, Vec<u8>)> = entries
            .iter()
            .map(|(name, data)| ((*name).to_string(), data.as_bytes().to_vec()))
            .collect();
        crate::bundle::write_stored_zip(&path, &entries).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        bytes
    }

    fn options() -> ExtractionOptions {
        ExtractionOptions::default()
    }

    #[test]
    fn docx_extracts_headings_lists_and_decoded_text() {
        let document = r#"<w:document><w:body>
            <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Introduction</w:t></w:r></w:p>
            <w:p><w:r><w:t>Hello &lt;world&gt; &amp; friends</w:t></w:r></w:p>
            <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>First item</w:t></w:r></w:p>
        </w:body></w:document>"#;
        let core =
            r#"<cp:coreProperties><dc:title>Quarterly Report</dc:title></cp:coreProperties>"#;
        let bytes = archive(&[("word/document.xml", document), ("docProps/core.xml", core)]);
        assert_eq!(
            detect_office(&bytes, None, Some("https://example.com/report.docx")),
            Some(OfficeKind::Docx)
        );
        let doc = extract_office_to_document(
            &bytes,
            OfficeKind::Docx,
            Some("https://example.com/report.docx"),
            &options(),
        )
        .unwrap();
        assert_eq!(doc.extraction.method, "office");
        assert_eq!(doc.title.as_deref(), Some("Quarterly Report"));
        assert!(doc.content.contains("# Introduction"));
        assert!(doc.content.contains("Hello <world> & friends"));
        assert!(doc.content.contains("- First item"));
        assert!(doc.token_count > 0);
    }

    #[test]
    fn pptx_extracts_slides_in_numeric_order() {
        let slide = |text: &str| {
            format!("<p:sld><p:cSld><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:cSld></p:sld>")
        };
        let first = slide("Title slide");
        let second = slide("Second &amp; last");
        let bytes = archive(&[
            ("ppt/slides/slide10.xml", first.as_str()),
            ("ppt/slides/slide2.xml", second.as_str()),
        ]);
        let doc = extract_office_to_document(
            &bytes,
            OfficeKind::Pptx,
            Some("https://example.com/deck.pptx"),
            &options(),
        )
        .unwrap();
        assert_eq!(doc.title.as_deref(), Some("Second & last"));
        let second_position = doc.content.find("## Slide 2").unwrap();
        let tenth_position = doc.content.find("## Slide 10").unwrap();
        assert!(second_position < tenth_position);
        assert!(doc.content.contains("Second & last"));
    }

    #[test]
    fn office_requires_identifying_metadata_and_rejects_xlsx() {
        let bytes = archive(&[("word/document.xml", "<w:document/>")]);
        assert_eq!(
            detect_office(&bytes, None, Some("https://example.com/a.zip")),
            None
        );
        assert_eq!(
            detect_office(
                &bytes,
                Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
                None
            ),
            Some(OfficeKind::Xlsx)
        );
        let error =
            extract_office_to_document(&bytes, OfficeKind::Xlsx, None, &options()).unwrap_err();
        assert!(error.to_string().contains("XLSX"));
    }
}
