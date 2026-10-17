use crate::error::TklessError;
use crate::fetcher::Fetcher;
use reqwest::header::{HeaderValue, CONTENT_RANGE, COOKIE, RANGE};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{debug, warn};
use url::Url;

static MD_IMAGE_REGEX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"!\[(.*?)\]\(((?:https?://)[^\s\)]+)\)").unwrap());

/// Trait for observing file download progress events.
pub trait DownloadProgressReporter: Send + Sync {
    /// Called when the download transfer begins.
    fn on_start(&self, _total_bytes: Option<u64>) {}

    /// Called periodically as chunks arrive.
    fn on_progress(&self, _downloaded_bytes: u64, _total_bytes: Option<u64>) {}

    /// Called when the download transfer completes successfully.
    fn on_finish(&self, _total_bytes: u64) {}
}

/// Configuration options for downloading a remote media file.
pub struct DownloadOptions<'a> {
    pub url: &'a str,
    pub destination: &'a Path,
    pub referer: Option<&'a str>,
    pub cookie: Option<&'a str>,
    pub reporter: Option<&'a dyn DownloadProgressReporter>,
    pub concurrency: Option<usize>,
}

/// Downloads a remote file with optional multi-threaded Range chunking and progress reporting.
pub async fn download_file(
    client: &reqwest::Client,
    options: DownloadOptions<'_>,
) -> Result<u64, TklessError> {
    debug!(
        url = options.url,
        dest = %options.destination.display(),
        concurrency = ?options.concurrency,
        "Starting file download"
    );

    // Ensure parent directory exists
    if let Some(parent) = options.destination.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let concurrency = options.concurrency.unwrap_or(1);

    // If concurrency > 1, attempt multi-connection HTTP Range chunked download
    if concurrency > 1 {
        if let Some(total_bytes) =
            probe_range_support(client, options.url, options.referer, options.cookie).await
        {
            // Only perform multi-threaded range downloads for files >= 4MB
            if total_bytes >= 4 * 1024 * 1024 {
                debug!(
                    total_bytes = total_bytes,
                    concurrency = concurrency,
                    "Target supports HTTP Range; launching multi-threaded concurrent download"
                );
                match download_file_concurrent(client, &options, total_bytes, concurrency).await {
                    Ok(bytes) => return Ok(bytes),
                    Err(e) => {
                        warn!(
                            error = %e,
                            "Multi-threaded range download failed; falling back to single-stream download"
                        );
                    }
                }
            }
        }
    }

    // Default single-stream download
    download_file_single_stream(client, options).await
}

/// Probes whether the remote server supports HTTP 206 Partial Content and returns the total file size.
async fn probe_range_support(
    client: &reqwest::Client,
    url: &str,
    referer: Option<&str>,
    cookie: Option<&str>,
) -> Option<u64> {
    let mut req = client.get(url).timeout(Duration::from_secs(15));
    if let Some(ref_url) = referer {
        req = req.header("Referer", ref_url);
    }
    if let Some(cookie_str) = cookie {
        if !cookie_str.trim().is_empty() {
            if let Ok(hval) = HeaderValue::from_str(cookie_str.trim()) {
                req = req.header(COOKIE, hval);
            }
        }
    }
    req = req.header(RANGE, "bytes=0-0");

    let resp = req.send().await.ok()?;
    if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        if let Some(cr) = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
        {
            if let Some(slash_pos) = cr.rfind('/') {
                let total_str = &cr[slash_pos + 1..];
                if let Ok(total) = total_str.parse::<u64>() {
                    return Some(total);
                }
            }
        }
    }
    None
}

/// Downloads a remote file concurrently in N byte ranges and stitches the parts sequentially.
async fn download_file_concurrent(
    client: &reqwest::Client,
    options: &DownloadOptions<'_>,
    total_bytes: u64,
    concurrency: usize,
) -> Result<u64, TklessError> {
    let threads = concurrency.clamp(2, 16);
    let chunk_size = total_bytes.div_ceil(threads as u64);

    if let Some(reporter) = options.reporter {
        reporter.on_start(Some(total_bytes));
    }

    let progress_counter = Arc::new(AtomicU64::new(0));
    let mut part_files = Vec::new();
    let mut join_set = JoinSet::new();

    for i in 0..threads {
        let start = i as u64 * chunk_size;
        let end = std::cmp::min((i as u64 + 1) * chunk_size - 1, total_bytes - 1);
        let part_path = PathBuf::from(format!("{}.part{}", options.destination.display(), i));
        part_files.push(part_path.clone());

        let client = client.clone();
        let url = options.url.to_string();
        let referer = options.referer.map(|s| s.to_string());
        let cookie = options.cookie.map(|s| s.to_string());
        let progress = Arc::clone(&progress_counter);

        join_set.spawn(async move {
            let mut req = client.get(&url).timeout(Duration::from_secs(1800));
            if let Some(ref r) = referer {
                req = req.header("Referer", r);
            }
            if let Some(ref c) = cookie {
                if !c.trim().is_empty() {
                    if let Ok(hval) = HeaderValue::from_str(c.trim()) {
                        req = req.header(COOKIE, hval);
                    }
                }
            }
            req = req.header(RANGE, format!("bytes={start}-{end}"));

            let mut resp = req.send().await.map_err(TklessError::Network)?;
            let status = resp.status();
            if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(TklessError::AdapterError {
                    platform: "downloader",
                    message: format!("HTTP {} error for range {start}-{end}", status),
                });
            }

            let file = File::create(&part_path).map_err(TklessError::Io)?;
            let mut writer = BufWriter::with_capacity(512 * 1024, file);

            while let Some(chunk) = resp.chunk().await.map_err(TklessError::Network)? {
                writer.write_all(&chunk).map_err(TklessError::Io)?;
                progress.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
            writer.flush().map_err(TklessError::Io)?;
            Ok::<(), TklessError>(())
        });
    }

    // Monitor progress reporting while tasks are downloading
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(())) => {
                if let Some(reporter) = options.reporter {
                    let cur = progress_counter.load(Ordering::Relaxed);
                    reporter.on_progress(cur, Some(total_bytes));
                }
            }
            Ok(Err(err)) => {
                cleanup_part_files(&part_files);
                return Err(err);
            }
            Err(join_err) => {
                cleanup_part_files(&part_files);
                return Err(TklessError::AdapterError {
                    platform: "downloader",
                    message: format!("Concurrent download task panicked: {join_err}"),
                });
            }
        }
    }

    // Stitch all part files sequentially into the final destination
    let final_file = File::create(options.destination)?;
    let mut final_writer = BufWriter::with_capacity(1024 * 1024, final_file);
    let mut assembled_bytes: u64 = 0;
    let mut buffer = vec![0u8; 512 * 1024];

    for part_path in &part_files {
        let part_file = File::open(part_path)?;
        let mut part_reader = BufReader::new(part_file);
        loop {
            let n = part_reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            final_writer.write_all(&buffer[..n])?;
            assembled_bytes += n as u64;
        }
    }
    final_writer.flush()?;

    // Clean up temporary part files
    cleanup_part_files(&part_files);

    if let Some(reporter) = options.reporter {
        reporter.on_finish(assembled_bytes);
    }

    Ok(assembled_bytes)
}

fn cleanup_part_files(part_files: &[PathBuf]) {
    for part in part_files {
        let _ = std::fs::remove_file(part);
    }
}

/// Fallback single-stream file download.
async fn download_file_single_stream(
    client: &reqwest::Client,
    options: DownloadOptions<'_>,
) -> Result<u64, TklessError> {
    let mut request = client.get(options.url).timeout(Duration::from_secs(1800));
    if let Some(ref_url) = options.referer {
        request = request.header("Referer", ref_url);
    }
    if let Some(cookie_str) = options.cookie {
        if !cookie_str.trim().is_empty() {
            if let Ok(hval) = HeaderValue::from_str(cookie_str.trim()) {
                request = request.header(COOKIE, hval);
            }
        }
    }

    let mut response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(TklessError::AdapterError {
            platform: "downloader",
            message: format!("HTTP {} error while downloading {}", status, options.url),
        });
    }

    let total_bytes = response.content_length();
    if let Some(reporter) = options.reporter {
        reporter.on_start(total_bytes);
    }

    let file = File::create(options.destination)?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);
    let mut downloaded_bytes: u64 = 0;

    let download_result: Result<(), TklessError> = async {
        while let Some(chunk) = response.chunk().await? {
            writer.write_all(&chunk)?;
            downloaded_bytes += chunk.len() as u64;

            if let Some(reporter) = options.reporter {
                reporter.on_progress(downloaded_bytes, total_bytes);
            }
        }
        writer.flush()?;
        Ok(())
    }
    .await;

    match download_result {
        Ok(()) => {
            if let Some(reporter) = options.reporter {
                reporter.on_finish(downloaded_bytes);
            }
            Ok(downloaded_bytes)
        }
        Err(err) => {
            drop(writer);
            let _ = std::fs::remove_file(options.destination);
            Err(err)
        }
    }
}

/// Formats raw byte counts into human-readable strings (e.g. 15.20 MB).
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Concurrently downloads remote images referenced in a Markdown document into `assets_dir`,
/// and rewrites the image URLs in the Markdown to point to the local relative paths.
pub async fn download_and_rewrite_markdown_images(
    fetcher: &Fetcher,
    markdown: &str,
    assets_dir: &Path,
    _base_url: Option<&str>,
) -> Result<String, TklessError> {
    if !assets_dir.exists() {
        std::fs::create_dir_all(assets_dir)?;
    }

    // 1. Collect all unique remote image URLs from markdown
    let mut image_urls = Vec::new();
    for cap in MD_IMAGE_REGEX.captures_iter(markdown) {
        if let Some(matched) = cap.get(2) {
            let u = matched.as_str().to_string();
            if !image_urls.contains(&u) {
                image_urls.push(u);
            }
        }
    }

    if image_urls.is_empty() {
        return Ok(markdown.to_string());
    }

    // 2. Concurrently download images with SSRF protection
    let mut url_to_local_path: HashMap<String, String> = HashMap::new();

    for (idx, img_url) in image_urls.into_iter().enumerate() {
        let parsed_url = match Url::parse(&img_url) {
            Ok(u) => u,
            Err(_) => continue,
        };

        // Enforce SSRF check on image host
        if fetcher.validate_ssrf(&parsed_url).await.is_err() {
            continue;
        }

        // Fetch image bytes
        let resp = match fetcher.get(&parsed_url).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };

        // Determine extension from Content-Type or URL path
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let ext = if content_type.contains("png") {
            ".png"
        } else if content_type.contains("jpeg") || content_type.contains("jpg") {
            ".jpg"
        } else if content_type.contains("webp") {
            ".webp"
        } else if content_type.contains("svg") {
            ".svg"
        } else if content_type.contains("gif") {
            ".gif"
        } else if let Some(path_ext) = Path::new(parsed_url.path())
            .extension()
            .and_then(|e| e.to_str())
        {
            &format!(".{path_ext}")
        } else {
            ".png"
        };

        let file_name = format!("img_{:03}{}", idx + 1, ext);
        let dest_path = assets_dir.join(&file_name);

        if let Ok(bytes) = resp.bytes().await {
            if std::fs::write(&dest_path, &bytes).is_ok() {
                let folder_name = assets_dir
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("assets");
                let relative_url = format!("{folder_name}/{file_name}");
                url_to_local_path.insert(img_url, relative_url);
            }
        }
    }

    // 3. Rewrite markdown image links
    let rewritten = MD_IMAGE_REGEX.replace_all(markdown, |caps: &regex::Captures| {
        let alt = &caps[1];
        let original_url = &caps[2];
        if let Some(local_path) = url_to_local_path.get(original_url) {
            format!("![{alt}]({local_path})")
        } else {
            caps[0].to_string()
        }
    });

    Ok(rewritten.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024 * 5 + 512 * 1024), "5.50 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 2), "2.00 GB");
    }

    #[test]
    fn test_md_image_regex_matching() {
        let md = r#"
        # Article Title
        Here is an inline picture: ![Architecture Diagram](https://example.com/assets/arch.png)
        And another photo: ![Logo](https://cdn.example.com/images/logo.webp)
        "#;

        let mut matches = Vec::new();
        for cap in MD_IMAGE_REGEX.captures_iter(md) {
            matches.push((cap[1].to_string(), cap[2].to_string()));
        }

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].0, "Architecture Diagram");
        assert_eq!(matches[0].1, "https://example.com/assets/arch.png");
        assert_eq!(matches[1].0, "Logo");
        assert_eq!(matches[1].1, "https://cdn.example.com/images/logo.webp");

        // Verify replacement
        let replaced = MD_IMAGE_REGEX.replace_all(md, |caps: &regex::Captures| {
            let alt = &caps[1];
            format!("![{alt}](./local/assets/img.png)")
        });

        assert!(replaced.contains("![Architecture Diagram](./local/assets/img.png)"));
        assert!(replaced.contains("![Logo](./local/assets/img.png)"));
        assert!(!replaced.contains("https://example.com/assets/arch.png"));
    }
}
