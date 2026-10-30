//! Markdown knowledge-base bundles stored in a small uncompressed ZIP container.
//!
//! The writer is intentionally minimal (stored entries only, no compression) so
//! the project does not pull in an archive dependency. It is not a general ZIP
//! implementation and refuses inputs outside the documented limits.
use crate::crawler::{CrawledPage, SiteCrawlReport};
use crate::TklessError;
use std::path::Path;

pub const MAX_BUNDLE_ENTRIES: usize = 512;
pub const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;

/// Writes a crawl report as `README.md` plus one Markdown file per page.
pub fn write_markdown_bundle(report: &SiteCrawlReport, path: &Path) -> Result<usize, TklessError> {
    let mut entries = Vec::with_capacity(report.pages.len() + 1);
    let mut index = String::new();
    index.push_str("# Site bundle\n\n");
    index.push_str(&format!("- seed: {}\n", report.seed_url));
    index.push_str(&format!("- pages: {}\n", report.pages.len()));
    index.push_str(&format!("- complete: {}\n", report.complete));
    index.push_str(&format!("- stop_reason: {}\n\n", report.stop_reason));
    index.push_str("## Pages\n\n");

    for (position, page) in report.pages.iter().enumerate() {
        let file = format!(
            "pages/{:03}-{}.md",
            position + 1,
            slugify(&page.title, position + 1)
        );
        index.push_str(&format!("- [{}]({})\n", page.title, file));
        entries.push((file, page_markdown(page)));
    }
    entries.insert(0, ("README.md".to_string(), index.into_bytes()));

    write_stored_zip(path, &entries)?;
    Ok(entries.len())
}

/// Writes a crawl report as a minimal, dependency-free EPUB 3 book.
///
/// The container reuses the stored ZIP writer, so `mimetype` is the first entry and no
/// compression is applied. Page bodies are emitted as escaped `<pre>` blocks rather than
/// rendered Markdown, which keeps the conversion lossless without a Markdown engine.
pub fn write_epub(report: &SiteCrawlReport, path: &Path) -> Result<usize, TklessError> {
    if report.pages.is_empty() {
        return Err(TklessError::InvalidArgument(
            "EPUB export requires at least one crawled page".into(),
        ));
    }
    let max_pages = MAX_BUNDLE_ENTRIES - 4;
    if report.pages.len() > max_pages {
        return Err(TklessError::InvalidArgument(format!(
            "EPUB export supports at most {max_pages} pages"
        )));
    }

    let book_title = format!("TkLess crawl: {}", report.seed_url);
    let mut entries = Vec::with_capacity(report.pages.len() + 4);
    entries.push(("mimetype".to_string(), b"application/epub+zip".to_vec()));
    entries.push((
        "META-INF/container.xml".to_string(),
        CONTAINER_XML.as_bytes().to_vec(),
    ));

    let mut manifest = String::new();
    let mut spine = String::new();
    let mut nav_items = String::new();
    for (position, page) in report.pages.iter().enumerate() {
        let index = position + 1;
        let file = format!("pages/{:03}-{}.xhtml", index, slugify(&page.title, index));
        let title = display_title(page);
        manifest.push_str(&format!(
            "    <item id=\"page-{index}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>\n",
            escape_xml(&file)
        ));
        spine.push_str(&format!("    <itemref idref=\"page-{index}\"/>\n"));
        nav_items.push_str(&format!(
            "      <li><a href=\"{}\">{}</a></li>\n",
            escape_xml(&file),
            escape_xml(&title)
        ));
        entries.push((
            format!("OEBPS/{file}"),
            page_xhtml(&title, page).into_bytes(),
        ));
    }

    let modified = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\">\n\
  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
    <dc:identifier id=\"bookid\">{}</dc:identifier>\n\
    <dc:title>{}</dc:title>\n\
    <dc:language>und</dc:language>\n\
    <meta property=\"dcterms:modified\">{modified}</meta>\n\
  </metadata>\n\
  <manifest>\n\
    <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n\
{manifest}\
\
  </manifest>\n\
  <spine>\n\
{spine}\
\
  </spine>\n\
</package>\n",
        escape_xml(&report.seed_url),
        escape_xml(&book_title)
    );
    entries.push(("OEBPS/content.opf".to_string(), opf.into_bytes()));

    let nav = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"und\">\n\
<head><meta charset=\"utf-8\"/><title>Contents</title></head>\n\
<body>\n\
  <nav epub:type=\"toc\" id=\"toc\">\n\
    <h1>Contents</h1>\n\
    <ol>\n\
{nav_items}\
\
    </ol>\n\
  </nav>\n\
</body>\n\
</html>\n"
    );
    entries.push(("OEBPS/nav.xhtml".to_string(), nav.into_bytes()));

    write_stored_zip(path, &entries)?;
    Ok(entries.len())
}

fn display_title(page: &CrawledPage) -> String {
    let title = page.title.trim();
    if title.is_empty() {
        page.url.clone()
    } else {
        title.to_string()
    }
}

fn page_xhtml(title: &str, page: &CrawledPage) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "<h1>{}</h1>\n<p class=\"source\"><a href=\"{}\">{}</a></p>\n",
        escape_xml(title),
        escape_xml(&page.url),
        escape_xml(&page.url)
    ));
    if let Some(error) = &page.error {
        body.push_str(&format!(
            "<p class=\"error\">Error: {}</p>\n",
            escape_xml(error)
        ));
    }
    body.push_str(&format!("<pre>{}</pre>\n", escape_xml(&page.content)));
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"und\">\n\
<head><meta charset=\"utf-8\"/><title>{}</title></head>\n\
<body>\n{body}</body>\n\
</html>\n",
        escape_xml(title)
    )
}

fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

const CONTAINER_XML: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
  <rootfiles>\n\
    <rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/>\n\
  </rootfiles>\n\
</container>\n";

fn page_markdown(page: &CrawledPage) -> Vec<u8> {
    let mut content = String::new();
    content.push_str("---\n");
    content.push_str(&format!(
        "url: {}\n",
        serde_json::to_string(&page.url).unwrap_or_else(|_| "\"\"".into())
    ));
    content.push_str(&format!(
        "title: {}\n",
        serde_json::to_string(&page.title).unwrap_or_else(|_| "\"\"".into())
    ));
    content.push_str(&format!("rank: {}\n", page.rank));
    content.push_str(&format!("depth: {}\n", page.depth));
    content.push_str(&format!("status: {}\n", page.status));
    content.push_str(&format!("tokens: {}\n", page.token_count));
    content.push_str("---\n\n");
    if let Some(error) = &page.error {
        content.push_str(&format!("> error: {error}\n\n"));
    }
    content.push_str(&page.content);
    content.push('\n');
    content.into_bytes()
}

fn slugify(title: &str, fallback: usize) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        format!("page-{fallback}")
    } else {
        slug
    }
}

pub(crate) fn write_stored_zip(
    path: &Path,
    entries: &[(String, Vec<u8>)],
) -> Result<(), TklessError> {
    if entries.is_empty() || entries.len() > MAX_BUNDLE_ENTRIES {
        return Err(TklessError::InvalidArgument(format!(
            "Bundle requires 1..{MAX_BUNDLE_ENTRIES} entries"
        )));
    }
    let total: usize = entries.iter().map(|(_, data)| data.len()).sum();
    if total > MAX_BUNDLE_BYTES {
        return Err(TklessError::InvalidArgument(format!(
            "Bundle exceeds {MAX_BUNDLE_BYTES} bytes"
        )));
    }

    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    let mut central = Vec::new();
    let mut offset = 0u32;

    for (name, data) in entries {
        if name.len() > 255 || name.is_empty() {
            return Err(TklessError::InvalidArgument(
                "Bundle entry names must be 1..255 bytes".into(),
            ));
        }
        let crc = crc32(data);
        let size = data.len() as u32;

        let mut local = Vec::with_capacity(30 + name.len() + data.len());
        push_u32(&mut local, 0x0403_4b50);
        push_u16(&mut local, 20);
        push_u16(&mut local, 0x0800); // UTF-8 names
        push_u16(&mut local, 0); // stored
        push_u16(&mut local, 0); // time
        push_u16(&mut local, 0); // date
        push_u32(&mut local, crc);
        push_u32(&mut local, size);
        push_u32(&mut local, size);
        push_u16(&mut local, name.len() as u16);
        push_u16(&mut local, 0); // extra
        local.extend_from_slice(name.as_bytes());
        local.extend_from_slice(data);
        file.write_all(&local)?;

        push_u32(&mut central, 0x0201_4b50);
        push_u16(&mut central, 20); // version made by
        push_u16(&mut central, 20); // version needed
        push_u16(&mut central, 0x0800);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, crc);
        push_u32(&mut central, size);
        push_u32(&mut central, size);
        push_u16(&mut central, name.len() as u16);
        push_u16(&mut central, 0); // extra
        push_u16(&mut central, 0); // comment
        push_u16(&mut central, 0); // disk
        push_u16(&mut central, 0); // internal attrs
        push_u32(&mut central, 0); // external attrs
        push_u32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());

        offset = offset
            .checked_add(local.len() as u32)
            .ok_or_else(|| TklessError::InvalidArgument("Bundle offset overflow".into()))?;
    }

    let central_offset = offset;
    file.write_all(&central)?;
    let mut end = Vec::with_capacity(22);
    push_u32(&mut end, 0x0605_4b50);
    push_u16(&mut end, 0);
    push_u16(&mut end, 0);
    push_u16(&mut end, entries.len() as u16);
    push_u16(&mut end, entries.len() as u16);
    push_u32(&mut end, central.len() as u32);
    push_u32(&mut end, central_offset);
    push_u16(&mut end, 0);
    file.write_all(&end)?;
    file.flush()?;
    Ok(())
}

fn push_u16(buffer: &mut Vec<u8>, value: u16) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::StopReason;

    fn page(title: &str, content: &str) -> CrawledPage {
        CrawledPage {
            rank: 1,
            url: "https://example.com/post".into(),
            title: title.into(),
            depth: 0,
            content: content.into(),
            token_count: 12,
            status: 200,
            error: None,
        }
    }

    struct ZipEntry {
        name: String,
        data: Vec<u8>,
    }

    fn parse_zip(bytes: &[u8]) -> Vec<ZipEntry> {
        let end = bytes.len() - 22;
        assert_eq!(&bytes[end..end + 4], &[0x50, 0x4b, 0x05, 0x06]);
        let count = u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]) as usize;
        let central_offset =
            u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap()) as usize;
        let mut cursor = central_offset;
        let mut entries = Vec::new();
        for _ in 0..count {
            assert_eq!(&bytes[cursor..cursor + 4], &[0x50, 0x4b, 0x01, 0x02]);
            let name_len = u16::from_le_bytes([bytes[cursor + 28], bytes[cursor + 29]]) as usize;
            let local_offset =
                u32::from_le_bytes(bytes[cursor + 42..cursor + 46].try_into().unwrap()) as usize;
            let name =
                String::from_utf8(bytes[cursor + 46..cursor + 46 + name_len].to_vec()).unwrap();
            assert_eq!(
                &bytes[local_offset..local_offset + 4],
                &[0x50, 0x4b, 0x03, 0x04]
            );
            let local_name_len =
                u16::from_le_bytes([bytes[local_offset + 26], bytes[local_offset + 27]]) as usize;
            let size = u32::from_le_bytes(
                bytes[local_offset + 18..local_offset + 22]
                    .try_into()
                    .unwrap(),
            ) as usize;
            let data_start = local_offset + 30 + local_name_len;
            entries.push(ZipEntry {
                name,
                data: bytes[data_start..data_start + size].to_vec(),
            });
            cursor += 46 + name_len;
        }
        entries
    }

    #[test]
    fn bundle_contains_index_and_page_markdown() {
        let path = std::env::temp_dir().join(format!(
            "tkless-bundle-{}-{}.zip",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let report = SiteCrawlReport {
            complete: true,
            stop_reason: StopReason::Exhausted,
            pending_urls: 0,
            seed_url: "https://example.com".into(),
            total_pages: 1,
            total_tokens: 12,
            pages: vec![page("Hello World", "# Hello\n\nBody.")],
        };
        assert_eq!(write_markdown_bundle(&report, &path).unwrap(), 2);

        let bytes = std::fs::read(&path).unwrap();
        let entries = parse_zip(&bytes);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "README.md");
        let index = String::from_utf8(entries[0].data.clone()).unwrap();
        assert!(index.contains("pages/001-hello-world.md"));
        let page_entry = entries
            .iter()
            .find(|entry| entry.name.ends_with("hello-world.md"))
            .expect("page entry");
        let markdown = String::from_utf8(page_entry.data.clone()).unwrap();
        assert!(markdown.contains("url: \"https://example.com/post\""));
        assert!(markdown.contains("# Hello"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn epub_contains_required_epub3_structure() {
        let path = std::env::temp_dir().join(format!(
            "tkless-epub-{}-{}.epub",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let report = SiteCrawlReport {
            complete: true,
            stop_reason: StopReason::Exhausted,
            pending_urls: 0,
            seed_url: "https://example.com/a?x=1&y=2".into(),
            total_pages: 2,
            total_tokens: 24,
            pages: vec![
                page("Hello & <World>", "<tag> & body"),
                page("", "second page"),
            ],
        };
        assert_eq!(write_epub(&report, &path).unwrap(), 6);

        let bytes = std::fs::read(&path).unwrap();
        let entries = parse_zip(&bytes);
        assert_eq!(entries[0].name, "mimetype");
        assert_eq!(entries[0].data, b"application/epub+zip");
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        for required in [
            "META-INF/container.xml",
            "OEBPS/content.opf",
            "OEBPS/nav.xhtml",
            "OEBPS/pages/001-hello-world.xhtml",
        ] {
            assert!(names.contains(&required), "missing {required} in {names:?}");
        }

        let entry = |name: &str| {
            String::from_utf8(
                entries
                    .iter()
                    .find(|entry| entry.name == name)
                    .unwrap_or_else(|| panic!("missing {name}"))
                    .data
                    .clone(),
            )
            .unwrap()
        };
        let opf = entry("OEBPS/content.opf");
        assert!(opf.contains("dcterms:modified"));
        assert!(opf.contains("href=\"pages/001-hello-world.xhtml\""));
        assert!(opf.contains("&amp;y=2"), "seed URL must be XML-escaped");
        let nav = entry("OEBPS/nav.xhtml");
        assert!(nav.contains("Hello &amp; &lt;World&gt;"));
        let page = entry("OEBPS/pages/001-hello-world.xhtml");
        assert!(page.contains("&lt;tag&gt; &amp; body"));
        assert!(!page.contains("<tag>"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn epub_rejects_empty_reports() {
        let path = std::env::temp_dir().join("tkless-empty.epub");
        let report = SiteCrawlReport {
            complete: true,
            stop_reason: StopReason::Exhausted,
            pending_urls: 0,
            seed_url: "https://example.com".into(),
            total_pages: 0,
            total_tokens: 0,
            pages: Vec::new(),
        };
        assert!(write_epub(&report, &path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bundle_rejects_oversized_inputs() {
        let path = std::env::temp_dir().join("tkless-bundle-invalid.zip");
        let entries = vec![("x".to_string(), vec![0u8; MAX_BUNDLE_BYTES + 1])];
        assert!(write_stored_zip(&path, &entries).is_err());
        let entries = vec![("".to_string(), b"data".to_vec())];
        assert!(write_stored_zip(&path, &entries).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
