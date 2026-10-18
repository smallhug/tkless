//! Console download progress reporter for media assets.

use std::io::{self, IsTerminal, Write};
use std::sync::Mutex;
use std::time::Instant;
use tkless_core::downloader::{format_bytes, DownloadProgressReporter};

/// Real-time console progress reporter for media downloads writing strictly to stderr when attached to a TTY.
pub struct ConsoleProgressReporter {
    label: &'static str,
    start_time: Instant,
    last_update: Mutex<Instant>,
    quiet: bool,
}

impl ConsoleProgressReporter {
    pub fn new(label: &'static str, quiet: bool) -> Self {
        let now = Instant::now();
        Self {
            label,
            start_time: now,
            last_update: Mutex::new(now),
            quiet,
        }
    }
}

impl DownloadProgressReporter for ConsoleProgressReporter {
    fn on_start(&self, total_bytes: Option<u64>) {
        if self.quiet || !io::stderr().is_terminal() {
            return;
        }
        let total_str = match total_bytes {
            Some(t) => format!(" (total: {})", format_bytes(t)),
            None => String::new(),
        };
        eprintln!("[TkLess] Starting {} download{}...", self.label, total_str);
    }

    fn on_progress(&self, downloaded_bytes: u64, total_bytes: Option<u64>) {
        if self.quiet || !io::stderr().is_terminal() {
            return;
        }

        let mut last = self.last_update.lock().unwrap();
        let now = Instant::now();
        if now.duration_since(*last).as_millis() < 120 {
            return;
        }
        *last = now;

        let elapsed_secs = self.start_time.elapsed().as_secs_f64().max(0.001);
        let speed = downloaded_bytes as f64 / elapsed_secs;
        let speed_str = format!("{}/s", format_bytes(speed as u64));
        let downloaded_str = format_bytes(downloaded_bytes);

        if let Some(total) = total_bytes {
            let percent = if total > 0 {
                (downloaded_bytes as f64 / total as f64 * 100.0).min(100.0)
            } else {
                0.0
            };
            let total_str = format_bytes(total);
            eprint!(
                "\r[TkLess] Downloading {}: {:>5.1}% ({:>9} / {:>9}, {:>10})   ",
                self.label, percent, downloaded_str, total_str, speed_str
            );
        } else {
            eprint!(
                "\r[TkLess] Downloading {}: {:>9} ({:>10})   ",
                self.label, downloaded_str, speed_str
            );
        }
        let _ = io::stderr().flush();
    }

    fn on_finish(&self, total_bytes: u64) {
        if self.quiet || !io::stderr().is_terminal() {
            return;
        }
        let elapsed = self.start_time.elapsed();
        let speed = if elapsed.as_secs_f64() > 0.0 {
            format!(
                "{}/s",
                format_bytes((total_bytes as f64 / elapsed.as_secs_f64()) as u64)
            )
        } else {
            "-".to_string()
        };
        eprintln!(
            "\r[TkLess] {} download complete: {} in {:.2?} ({})                    ",
            self.label,
            format_bytes(total_bytes),
            elapsed,
            speed
        );
    }
}
