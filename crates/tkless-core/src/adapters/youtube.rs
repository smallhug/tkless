//! YouTube video metadata, chapters, and native transcript/captions extractor.
//!
//! Intercepts `youtube.com/watch?v=...`, `youtu.be/...`, and `youtube.com/shorts/...` URLs,
//! extracting video title, channel author, duration, publication date, view count,
//! structured chapters, and native timed subtitles (timedtext) without headless browser overhead.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static TIMEDTEXT_XML_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<text\s+start="([\d.]+)"(?:\s+dur="([\d.]+)")?[^>]*>([\s\S]*?)</text>"#).unwrap()
});

static CHAPTER_TIMESTAMP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*(?:(\d{1,2}:)?\d{1,2}:\d{2})\s*[-–—:]?\s*(.+)$"#).unwrap()
});

static YT_PLAYER_RESPONSE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"ytInitialPlayerResponse\s*=\s*(\{[\s\S]+?\});(?:\s*var\s+|\s*</script>)"#)
        .unwrap()
});

fn unescape_xml_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

pub struct YoutubeAdapter;

impl Default for YoutubeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl YoutubeAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Extracts YouTube Video ID from standard watch URL, shorts URL, or youtu.be shortlink.
    pub fn extract_video_id(url: &Url) -> Option<String> {
        let host = url.host_str().unwrap_or("").to_lowercase();
        let path = url.path().trim_start_matches('/');

        if crate::fetcher::is_subdomain_or_exact(&host, "youtu.be") {
            let id = path.split('/').next().unwrap_or("").trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }

        if crate::fetcher::is_subdomain_or_exact(&host, "youtube.com") {
            if let Some((_, val)) = url.query_pairs().find(|(k, _)| k == "v") {
                if !val.trim().is_empty() {
                    return Some(val.trim().to_string());
                }
            }

            if let Some(rest) = path.strip_prefix("shorts/") {
                let id = rest.split('/').next().unwrap_or("").trim();
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }

            if let Some(rest) = path.strip_prefix("embed/") {
                let id = rest.split('/').next().unwrap_or("").trim();
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }

        None
    }

    /// Formats seconds into a human-readable duration string (`mm:ss` or `hh:mm:ss`).
    pub fn format_duration(total_secs: u64) -> String {
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let secs = total_secs % 60;
        if hours > 0 {
            format!("{hours:02}:{mins:02}:{secs:02}")
        } else {
            format!("{mins:02}:{secs:02}")
        }
    }

    /// Parses YouTube timedtext XML format into formatted timestamped subtitle lines.
    pub fn parse_timedtext_xml(xml: &str) -> Vec<(String, String)> {
        let mut lines = Vec::new();
        for cap in TIMEDTEXT_XML_RE.captures_iter(xml) {
            let start_f: f64 = cap[1].parse().unwrap_or(0.0);
            let time_str = Self::format_duration(start_f as u64);
            let raw_text = &cap[3];
            let clean_text = unescape_xml_entities(raw_text)
                .replace('\n', " ")
                .trim()
                .to_string();
            if !clean_text.is_empty() {
                lines.push((time_str, clean_text));
            }
        }
        lines
    }

    /// Parses YouTube timedtext JSON3 format into formatted timestamped subtitle lines.
    pub fn parse_timedtext_json3(json_str: &str) -> Vec<(String, String)> {
        let mut lines = Vec::new();
        if let Ok(v) = serde_json::from_str::<Value>(json_str) {
            if let Some(events) = v.get("events").and_then(|e| e.as_array()) {
                for ev in events {
                    let start_ms = ev.get("tStartMs").and_then(|ms| ms.as_u64()).unwrap_or(0);
                    let time_str = Self::format_duration(start_ms / 1000);
                    if let Some(segs) = ev.get("segs").and_then(|s| s.as_array()) {
                        let mut seg_text = String::new();
                        for seg in segs {
                            if let Some(utf8) = seg.get("utf8").and_then(|u| u.as_str()) {
                                seg_text.push_str(utf8);
                            }
                        }
                        let clean = seg_text.replace('\n', " ").trim().to_string();
                        if !clean.is_empty() {
                            lines.push((time_str, clean));
                        }
                    }
                }
            }
        }
        lines
    }

    /// Extracts video chapters from description text if timestamp markers are present.
    pub fn extract_chapters_from_description(desc: &str) -> Vec<(String, String)> {
        let mut chapters = Vec::new();
        for line in desc.lines() {
            let trimmed = line.trim();
            if CHAPTER_TIMESTAMP_RE.is_match(trimmed) {
                if let Some(first_match) = trimmed.split_whitespace().next() {
                    let ts = first_match.trim_matches(['-', '–', '—', ':']).trim();
                    let title = trimmed[first_match.len()..]
                        .trim()
                        .trim_start_matches(['-', '–', '—', ':', '|'])
                        .trim();
                    if !ts.is_empty() && !title.is_empty() {
                        chapters.push((ts.to_string(), title.to_string()));
                    }
                }
            }
        }
        chapters
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing YouTube adapter extraction");

        let video_id = Self::extract_video_id(url).ok_or_else(|| {
            TklessError::InvalidArgument(format!("Invalid YouTube video URL: {url}"))
        })?;

        let watch_url_str = format!("https://www.youtube.com/watch?v={video_id}&hl=zh-CN");
        let watch_parsed_url = Url::parse(&watch_url_str)
            .map_err(|e| TklessError::InvalidArgument(format!("Failed to parse URL: {e}")))?;

        let resp = fetcher
            .get(&watch_parsed_url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
            )
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .send()
            .await?;

        let html = resp.text().await?;

        // 1. Extract ytInitialPlayerResponse JSON payload
        let player_response: Option<Value> = YT_PLAYER_RESPONSE_RE
            .captures(&html)
            .and_then(|cap| cap.get(1))
            .and_then(|m| serde_json::from_str(m.as_str()).ok());

        let video_details = player_response
            .as_ref()
            .and_then(|pr| pr.get("videoDetails"));
        let microformat = player_response
            .as_ref()
            .and_then(|pr| pr.get("microformat"))
            .and_then(|m| m.get("playerMicroformatRenderer"));

        let title = video_details
            .and_then(|vd| vd.get("title"))
            .and_then(|t| t.as_str())
            .unwrap_or("YouTube Video")
            .to_string();

        let author = video_details
            .and_then(|vd| vd.get("author"))
            .and_then(|a| a.as_str())
            .unwrap_or("")
            .to_string();

        let view_count = video_details
            .and_then(|vd| vd.get("viewCount"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let length_secs: u64 = video_details
            .and_then(|vd| vd.get("lengthSeconds"))
            .and_then(|l| l.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let duration_str = Self::format_duration(length_secs);

        let description = video_details
            .and_then(|vd| vd.get("shortDescription"))
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim();

        let published_time = microformat
            .and_then(|mf| mf.get("publishDate").or_else(|| mf.get("uploadDate")))
            .and_then(|p| p.as_str())
            .map(|s| s.to_string());

        // 2. Extract chapters from description
        let chapters = Self::extract_chapters_from_description(description);

        // 3. Extract captions / subtitles
        let mut subtitle_lines = Vec::new();
        let caption_tracks = player_response
            .as_ref()
            .and_then(|pr| pr.get("captions"))
            .and_then(|c| c.get("playerCaptionsTracklistRenderer"))
            .and_then(|r| r.get("captionTracks"))
            .and_then(|t| t.as_array());

        if let Some(tracks) = caption_tracks {
            // Priority: zh (Chinese) -> en (English) -> first available
            let target_track = tracks
                .iter()
                .find(|t| {
                    t.get("languageCode")
                        .and_then(|l| l.as_str())
                        .map(|l| l.starts_with("zh"))
                        .unwrap_or(false)
                })
                .or_else(|| {
                    tracks.iter().find(|t| {
                        t.get("languageCode")
                            .and_then(|l| l.as_str())
                            .map(|l| l.starts_with("en"))
                            .unwrap_or(false)
                    })
                })
                .or_else(|| tracks.first());

            if let Some(track) = target_track {
                if let Some(base_url) = track.get("baseUrl").and_then(|u| u.as_str()) {
                    let sub_fetch_url = if base_url.contains("fmt=") {
                        base_url.to_string()
                    } else {
                        format!("{base_url}&fmt=json3")
                    };

                    if let Ok(parsed_sub_url) = Url::parse(&sub_fetch_url) {
                        if let Ok(sub_resp) = fetcher.get(&parsed_sub_url).send().await {
                            if let Ok(sub_text) = sub_resp.text().await {
                                if sub_text.trim_start().starts_with('{') {
                                    subtitle_lines = Self::parse_timedtext_json3(&sub_text);
                                } else {
                                    subtitle_lines = Self::parse_timedtext_xml(&sub_text);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 4. Construct high-fidelity Markdown
        let mut content = String::new();

        if options.frontmatter && options.format == OutputFormat::Markdown {
            content.push_str("---\n");
            content.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
            if !author.is_empty() {
                content.push_str(&format!("channel: \"{}\"\n", author.replace('"', "\\\"")));
            }
            if let Some(ref pub_time) = published_time {
                content.push_str(&format!("date: \"{pub_time}\"\n"));
            }
            content.push_str(&format!("duration: \"{duration_str}\"\n"));
            content.push_str(&format!(
                "url: \"https://www.youtube.com/watch?v={video_id}\"\n"
            ));
            content.push_str("platform: \"youtube\"\n");
            content.push_str("---\n\n");
        }

        content.push_str(&format!("# 🎬 {title}\n\n"));
        content.push_str(&format!(
            "> 👤 **频道**: {} | ⏱️ **时长**: {} | 👁️ **播放量**: {}\n\n",
            if author.is_empty() { "未知" } else { &author },
            duration_str,
            if view_count.is_empty() {
                "未知"
            } else {
                view_count
            }
        ));

        if !chapters.is_empty() {
            content.push_str("### 📑 视频章节目录 (Chapters)\n\n");
            content.push_str("| 时间戳 | 章节名称 |\n");
            content.push_str("|:---|:---|\n");
            for (ts, ch_title) in &chapters {
                content.push_str(&format!("| `{ts}` | {ch_title} |\n"));
            }
            content.push_str("\n---\n\n");
        }

        if !description.is_empty() {
            content.push_str("### 📖 视频简介 (Description)\n\n");
            content.push_str(description);
            content.push_str("\n\n---\n\n");
        }

        if !subtitle_lines.is_empty() {
            content.push_str("### 📝 视频字幕与实录 (Transcript)\n\n");
            for (time_label, line_text) in &subtitle_lines {
                content.push_str(&format!("- **`{time_label}`** {line_text}\n"));
            }
            content.push('\n');
        }

        let token_count = count_tokens(&content, options.tokenizer);

        let metadata = PageMetadata {
            author: if author.is_empty() {
                None
            } else {
                Some(author.clone())
            },
            published_time,
            canonical_url: Some(format!("https://www.youtube.com/watch?v={video_id}")),
            description: if description.is_empty() {
                None
            } else {
                Some(description.lines().next().unwrap_or("").to_string())
            },
            ..Default::default()
        };

        Ok(PageDocument {
            url: url.to_string(),
            final_url: format!("https://www.youtube.com/watch?v={video_id}"),
            status: 200,
            title: Some(title),
            byline: if author.is_empty() {
                None
            } else {
                Some(author)
            },
            content,
            metadata,
            json_ld: Vec::new(),
            token_count,
            tokenizer: options.tokenizer.as_str().to_string(),
            truncated: false,
            subtitles: None,
            danmaku: None,
            comments: None,
            conclusion: None,
            links: None,
            images: None,
            tables: None,
            entity: None,
            chunks: None,
            content_hash: None,
            metrics: None,
        })
    }
}

impl PlatformAdapter for YoutubeAdapter {
    fn name(&self) -> &'static str {
        "youtube"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("").to_lowercase();
        crate::fetcher::is_subdomain_or_exact(&host, "youtube.com")
            || crate::fetcher::is_subdomain_or_exact(&host, "youtu.be")
    }

    fn extract<'a>(
        &'a self,
        fetcher: &'a crate::fetcher::Fetcher,
        url: &'a Url,
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<PageDocument, TklessError>> {
        Box::pin(self.do_extract(fetcher, url, options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_youtube_video_id_extraction() {
        let u1 = Url::parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap();
        assert_eq!(
            YoutubeAdapter::extract_video_id(&u1),
            Some("dQw4w9WgXcQ".to_string())
        );

        let u2 = Url::parse("https://youtu.be/dQw4w9WgXcQ?t=42").unwrap();
        assert_eq!(
            YoutubeAdapter::extract_video_id(&u2),
            Some("dQw4w9WgXcQ".to_string())
        );

        let u3 = Url::parse("https://www.youtube.com/shorts/dQw4w9WgXcQ").unwrap();
        assert_eq!(
            YoutubeAdapter::extract_video_id(&u3),
            Some("dQw4w9WgXcQ".to_string())
        );

        let u4 = Url::parse("https://www.youtube.com/embed/dQw4w9WgXcQ").unwrap();
        assert_eq!(
            YoutubeAdapter::extract_video_id(&u4),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_youtube_format_duration() {
        assert_eq!(YoutubeAdapter::format_duration(45), "00:45");
        assert_eq!(YoutubeAdapter::format_duration(125), "02:05");
        assert_eq!(YoutubeAdapter::format_duration(3665), "01:01:05");
    }

    #[test]
    fn test_youtube_parse_timedtext_xml() {
        let xml = r#"
        <transcript>
            <text start="0.0" dur="2.5">Hello &amp; welcome to the channel!</text>
            <text start="2.5" dur="3.0">Today we discuss Rust programming.</text>
        </transcript>
        "#;
        let lines = YoutubeAdapter::parse_timedtext_xml(xml);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].0, "00:00");
        assert_eq!(lines[0].1, "Hello & welcome to the channel!");
        assert_eq!(lines[1].0, "00:02");
        assert_eq!(lines[1].1, "Today we discuss Rust programming.");
    }

    #[test]
    fn test_youtube_parse_timedtext_json3() {
        let json = r#"{
            "events": [
                {
                    "tStartMs": 1500,
                    "segs": [{"utf8": "First "}, {"utf8": "segment"}]
                },
                {
                    "tStartMs": 65000,
                    "segs": [{"utf8": "One minute later"}]
                }
            ]
        }"#;
        let lines = YoutubeAdapter::parse_timedtext_json3(json);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].0, "00:01");
        assert_eq!(lines[0].1, "First segment");
        assert_eq!(lines[1].0, "01:05");
        assert_eq!(lines[1].1, "One minute later");
    }

    #[test]
    fn test_youtube_extract_chapters() {
        let desc = "Check out our video:\n00:00 - Introduction\n01:45 Architecture Overview\n10:20 - Summary and Benchmark";
        let chapters = YoutubeAdapter::extract_chapters_from_description(desc);
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].0, "00:00");
        assert_eq!(chapters[0].1, "Introduction");
        assert_eq!(chapters[1].0, "01:45");
        assert_eq!(chapters[1].1, "Architecture Overview");
        assert_eq!(chapters[2].0, "10:20");
        assert_eq!(chapters[2].1, "Summary and Benchmark");
    }
}
