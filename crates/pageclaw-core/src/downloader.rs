use crate::error::PageclawError;
use reqwest::header::{HeaderValue, COOKIE};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Duration;
use tracing::debug;

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
}

/// Downloads a remote file in chunks with progress reporting and cleanup on failure.
pub async fn download_file(
    client: &reqwest::Client,
    options: DownloadOptions<'_>,
) -> Result<u64, PageclawError> {
    debug!(url = options.url, dest = %options.destination.display(), "Starting file download");

    let mut request = client.get(options.url).timeout(Duration::from_secs(1800)); // Allow up to 30 minutes for large media file transfers
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
        return Err(PageclawError::AdapterError {
            platform: "downloader",
            message: format!("HTTP {} error while downloading {}", status, options.url),
        });
    }

    let total_bytes = response.content_length();
    if let Some(reporter) = options.reporter {
        reporter.on_start(total_bytes);
    }

    // Ensure parent directory exists
    if let Some(parent) = options.destination.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let file = File::create(options.destination)?;
    let mut writer = BufWriter::new(file);
    let mut downloaded_bytes: u64 = 0;

    let download_result: Result<(), PageclawError> = async {
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
            // Remove incomplete file on error
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
}
