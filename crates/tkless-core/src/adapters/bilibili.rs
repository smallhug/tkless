use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::downloader::{download_file, DownloadOptions, DownloadProgressReporter};
use crate::error::TklessError;
use crate::model::{
    CommentEntry, ConclusionChapter, ConclusionKeypoint, DanmakuEntry, ExtractedImage,
    ExtractionOptions, OutputFormat, PageDocument, PageMetadata, SubtitleLine, VideoConclusion,
    VideoQuality,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use tracing::{debug, warn};
use url::Url;

static EM_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"</?em[^>]*>"#).unwrap());
static DANMAKU_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"<d\s+p="([^"]*)">([\s\S]*?)</d>"#).unwrap());

const WBI_MIXIN_TABLE: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

static WBI_CACHE: std::sync::RwLock<Option<(String, u64)>> = std::sync::RwLock::new(None);

/// Route variations supported by the Bilibili platform adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BilibiliRoute {
    /// Specific video detail page (e.g. /video/BV1xx411c7mD)
    Video(String),
    /// Partition or overall leaderboard rankings (e.g. /v/popular/rank/all)
    Ranking { rid: u32, category_name: String },
    /// Comprehensive popular videos stream (/v/popular/all)
    Popular,
    /// Homepage recommendation feed (https://www.bilibili.com/ or with --feed)
    Recommendations,
    /// Video search query (https://search.bilibili.com/all?keyword=...)
    Search {
        keyword: String,
        page: u32,
        order: Option<String>,
    },
}

pub struct BilibiliAdapter;

impl Default for BilibiliAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl BilibiliAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Analyzes the URL and extraction options to determine the appropriate Bilibili route.
    pub fn parse_route(url: &Url, options: &ExtractionOptions) -> Option<BilibiliRoute> {
        let host = url.host_str()?;
        if !crate::fetcher::is_subdomain_or_exact(host, "bilibili.com")
            && !crate::fetcher::is_subdomain_or_exact(host, "b23.tv")
        {
            return None;
        }

        let path = url.path().trim_end_matches('/');

        // 1. Single video route: /video/BV... or /video/av...
        if let Some(pos) = path.find("/video/") {
            let id_part = &path[pos + 7..];
            let bvid = id_part.split('/').next()?.split('?').next()?;
            if !bvid.is_empty() {
                return Some(BilibiliRoute::Video(bvid.to_string()));
            }
        }

        // 2. Rankings route: /v/popular/rank/...
        if path.starts_with("/v/popular/rank") {
            let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            let slug = segments.get(3).copied().unwrap_or("all");

            let mut rid = match slug {
                "all" => 0,
                "anime" | "douga" => 1,
                "guochuang" => 168,
                "music" => 3,
                "dance" => 129,
                "game" => 4,
                "tech" | "technology" | "digital" => 188,
                "knowledge" => 36,
                "car" => 223,
                "sports" => 234,
                "life" => 160,
                "food" => 211,
                "animal" => 217,
                "kichiku" => 119,
                "fashion" => 155,
                "ent" => 5,
                "cinephile" => 181,
                "origin" => 0,
                "rookie" => 0,
                _ => 0,
            };

            // Query parameter override: ?rid=188
            for (k, v) in url.query_pairs() {
                if k == "rid" {
                    if let Ok(parsed_rid) = v.parse::<u32>() {
                        rid = parsed_rid;
                    }
                }
            }

            let cat_name = rid_to_category_name(rid);
            return Some(BilibiliRoute::Ranking {
                rid,
                category_name: cat_name.to_string(),
            });
        }

        // 3. Search route: https://search.bilibili.com/all?keyword=... or /video?keyword=...
        let is_search_host = host == "search.bilibili.com";
        let is_search_path = path == "/all" || path == "/video";
        let has_keyword = url.query_pairs().any(|(k, _)| k == "keyword");

        if is_search_host || (is_search_path && has_keyword) {
            let mut keyword = None;
            let mut page = 1u32;
            let mut order = None;

            for (k, v) in url.query_pairs() {
                match k.as_ref() {
                    "keyword" => keyword = Some(v.to_string()),
                    "page" => {
                        if let Ok(p) = v.parse::<u32>() {
                            if p > 0 {
                                page = p;
                            }
                        }
                    }
                    "order" => {
                        let trimmed = v.trim();
                        if !trimmed.is_empty() {
                            order = Some(trimmed.to_string());
                        }
                    }
                    _ => {}
                }
            }

            if let Some(kw) = keyword {
                let trimmed_kw = kw.trim();
                if !trimmed_kw.is_empty() {
                    return Some(BilibiliRoute::Search {
                        keyword: trimmed_kw.to_string(),
                        page,
                        order,
                    });
                }
            }
        }

        // 4. Popular videos route: /v/popular/all or /v/popular
        if path == "/v/popular/all" || path == "/v/popular" {
            return Some(BilibiliRoute::Popular);
        }

        // 5. Homepage or Feed route
        if options.feed || path.is_empty() || path == "/" || path == "/index.html" {
            return Some(BilibiliRoute::Recommendations);
        }

        None
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        let route = Self::parse_route(url, options).ok_or_else(|| TklessError::AdapterError {
            platform: "bilibili",
            message: format!("Unsupported Bilibili URL route: {url}"),
        })?;

        debug!(route = ?route, "Executing Bilibili adapter extraction");

        match route {
            BilibiliRoute::Video(bvid) => self.extract_video(fetcher, &bvid, url, options).await,
            BilibiliRoute::Ranking { rid, category_name } => {
                self.extract_ranking(fetcher, rid, &category_name, url, options)
                    .await
            }
            BilibiliRoute::Popular => self.extract_popular(fetcher, url, options).await,
            BilibiliRoute::Recommendations => {
                self.extract_recommendations(fetcher, url, options).await
            }
            BilibiliRoute::Search {
                keyword,
                page,
                order,
            } => {
                self.extract_search(fetcher, &keyword, page, order.as_deref(), url, options)
                    .await
            }
        }
    }

    /// Fetches structured video metadata from Bilibili API.
    pub async fn fetch_video_details(
        fetcher: &crate::fetcher::Fetcher,
        bvid: &str,
    ) -> Result<VideoViewData, TklessError> {
        let api_url = format!("https://api.bilibili.com/x/web-interface/view?bvid={bvid}");

        let resp: BiliResponse<VideoViewData> = fetcher
            .read_json(
                fetcher
                    .get_str(&api_url)
                    .header("Referer", "https://www.bilibili.com/")
                    .send()
                    .await?,
            )
            .await?;

        if resp.code != 0 {
            return Err(TklessError::AdapterError {
                platform: "bilibili",
                message: format!("API error (code={}): {}", resp.code, resp.message),
            });
        }

        resp.data.ok_or_else(|| TklessError::AdapterError {
            platform: "bilibili",
            message: "Empty data payload received for video view".to_string(),
        })
    }

    /// Fetches official or AI CC subtitle lines for a given video CID and BVID.
    pub async fn fetch_subtitles(
        fetcher: &crate::fetcher::Fetcher,
        bvid: &str,
        cid: u64,
        view_data: &VideoViewData,
    ) -> Result<Vec<SubtitleLine>, TklessError> {
        // 1. Try subtitles from view API
        let mut subtitle_list = view_data
            .subtitle
            .as_ref()
            .map(|s| s.list.clone())
            .unwrap_or_default();

        // 2. Fallback to /x/player/v2 if view API returned empty subtitles
        if subtitle_list.is_empty() {
            let player_api = format!("https://api.bilibili.com/x/player/v2?cid={cid}&bvid={bvid}");
            if let Ok(resp) = fetcher
                .get_str(&player_api)
                .header("Referer", "https://www.bilibili.com/")
                .send()
                .await
            {
                if let Ok(bili_resp) = fetcher.read_json::<BiliResponse<PlayerV2Data>>(resp).await {
                    if let Some(pdata) = bili_resp.data {
                        if let Some(sub_wrap) = pdata.subtitle {
                            subtitle_list = sub_wrap.subtitles;
                        }
                    }
                }
            }
        }

        if subtitle_list.is_empty() {
            return Ok(Vec::new());
        }

        // 3. Select preferred subtitle (prefer Chinese zh/中, fallback to first)
        let selected = subtitle_list
            .iter()
            .find(|s| s.lan.starts_with("zh") || s.lan_doc.contains('中'))
            .or_else(|| subtitle_list.first());

        let target_sub = match selected {
            Some(s) if !s.subtitle_url.trim().is_empty() => s,
            _ => return Ok(Vec::new()),
        };

        let sub_url = normalize_cover_url(&target_sub.subtitle_url);

        // 4. Download and parse subtitle JSON
        let sub_resp: SubtitleFileResponse = fetcher
            .read_json(
                fetcher
                    .get_str(&sub_url)
                    .header("Referer", "https://www.bilibili.com/")
                    .send()
                    .await?,
            )
            .await?;

        let lines = sub_resp
            .body
            .into_iter()
            .map(|item| SubtitleLine {
                from: item.from,
                to: item.to,
                content: item.content,
            })
            .collect();

        Ok(lines)
    }

    /// Fetches timestamped video danmaku entries for a given video CID with optional limit.
    pub async fn fetch_danmaku(
        fetcher: &crate::fetcher::Fetcher,
        cid: u64,
        limit: Option<usize>,
    ) -> Result<Vec<DanmakuEntry>, TklessError> {
        let api_url = format!("https://comment.bilibili.com/{cid}.xml");

        let resp = fetcher
            .get_str(&api_url)
            .header("Referer", "https://www.bilibili.com/")
            .send()
            .await?;

        let bytes = fetcher.read_bytes(resp).await?;
        let xml_str = decode_xml_body_with_limit(&bytes, fetcher.options().max_bytes)?;
        let mut entries = parse_danmaku_xml(&xml_str);

        if let Some(lim) = limit {
            entries.truncate(lim);
        }

        Ok(entries)
    }

    /// Fetches top liked comments for a given video AID with optional limit and pagination.
    pub async fn fetch_comments(
        fetcher: &crate::fetcher::Fetcher,
        aid: u64,
        limit: Option<usize>,
    ) -> Result<Vec<CommentEntry>, TklessError> {
        let max_items = limit.unwrap_or(20);
        let mut comments = Vec::new();
        let mut page = 1u32;

        while comments.len() < max_items {
            let remaining = max_items - comments.len();
            let ps = remaining.clamp(1, 20);
            let api_url = format!(
                "https://api.bilibili.com/x/v2/reply?type=1&oid={aid}&sort=2&pn={page}&ps={ps}"
            );

            let resp: BiliResponse<ReplyData> = fetcher
                .read_json(
                    fetcher
                        .get_str(&api_url)
                        .header("Referer", "https://www.bilibili.com/")
                        .send()
                        .await?,
                )
                .await?;

            if resp.code != 0 {
                if comments.is_empty() {
                    return Err(TklessError::AdapterError {
                        platform: "bilibili",
                        message: format!(
                            "Comments API error (code={}): {}",
                            resp.code, resp.message
                        ),
                    });
                } else {
                    break;
                }
            }

            let data = match resp.data {
                Some(d) => d,
                None => break,
            };

            let raw_replies = data.replies.unwrap_or_default();
            if raw_replies.is_empty() {
                break;
            }

            for r in raw_replies {
                comments.push(CommentEntry {
                    user: r.member.uname,
                    like: r.like,
                    content: r.content.message,
                    time: format_timestamp(r.ctime).0,
                });
                if comments.len() >= max_items {
                    break;
                }
            }

            page += 1;
            // Cap at 10 pages maximum (up to 200 comments) to prevent runaway requests
            if page > 10 {
                break;
            }
        }

        Ok(comments)
    }

    /// Fetches official AI-generated video conclusion and outline summary.
    pub async fn fetch_conclusion(
        fetcher: &crate::fetcher::Fetcher,
        bvid: &str,
        cid: u64,
        up_mid: u64,
    ) -> Result<Option<VideoConclusion>, TklessError> {
        let mixin_key = match get_wbi_mixin_key(fetcher).await {
            Ok(k) => k,
            Err(e) => {
                debug!(error = %e, "Failed to obtain WBI mixin key for conclusion, skipping");
                return Ok(None);
            }
        };

        let wts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let params = vec![
            ("bvid".to_string(), bvid.to_string()),
            ("cid".to_string(), cid.to_string()),
            ("up_mid".to_string(), up_mid.to_string()),
            ("wts".to_string(), wts.to_string()),
        ];

        let signed_query = sign_wbi_params(&params, &mixin_key);
        let api_url =
            format!("https://api.bilibili.com/x/web-interface/view/conclusion/get?{signed_query}");

        let resp: ConclusionApiResponse = fetcher
            .read_json(
                fetcher
                    .get_str(&api_url)
                    .header("Referer", "https://www.bilibili.com/")
                    .send()
                    .await?,
            )
            .await?;

        if resp.code != 0 {
            debug!(code = resp.code, message = %resp.message, "Conclusion API returned non-zero code");
            return Ok(None);
        }

        let data = match resp.data {
            Some(d) if d.code == 0 => d,
            _ => return Ok(None),
        };

        let model_result = match data.model_result {
            Some(m) if !m.summary.trim().is_empty() || !m.outline.is_empty() => m,
            _ => return Ok(None),
        };

        let outline = model_result
            .outline
            .into_iter()
            .map(|item| ConclusionChapter {
                title: item.title,
                timestamp: item.timestamp,
                parts: item
                    .part_outline
                    .into_iter()
                    .map(|p| ConclusionKeypoint {
                        timestamp: p.timestamp,
                        content: p.content,
                    })
                    .collect(),
            })
            .collect();

        Ok(Some(VideoConclusion {
            summary: model_result.summary,
            outline,
        }))
    }

    /// Fetches the progressive MP4 stream URL for a given video CID and quality.
    pub async fn fetch_play_url(
        fetcher: &crate::fetcher::Fetcher,
        bvid: &str,
        cid: u64,
        quality: VideoQuality,
    ) -> Result<PlayUrlData, TklessError> {
        let qn = quality.qn();
        let api_url = format!(
            "https://api.bilibili.com/x/player/playurl?bvid={bvid}&cid={cid}&qn={qn}&fnval=0&fnver=0"
        );

        let resp: BiliResponse<PlayUrlData> = fetcher
            .read_json(
                fetcher
                    .get_str(&api_url)
                    .header("Referer", "https://www.bilibili.com/")
                    .send()
                    .await?,
            )
            .await?;

        if resp.code != 0 {
            return Err(TklessError::AdapterError {
                platform: "bilibili",
                message: format!("PlayURL API error (code={}): {}", resp.code, resp.message),
            });
        }

        resp.data.ok_or_else(|| TklessError::AdapterError {
            platform: "bilibili",
            message: "Empty data payload received for playurl".to_string(),
        })
    }

    /// Downloads the video cover image to the destination path.
    pub async fn download_cover(
        fetcher: &crate::fetcher::Fetcher,
        bvid: &str,
        dest_path: Option<&Path>,
        reporter: Option<&dyn DownloadProgressReporter>,
    ) -> Result<PathBuf, TklessError> {
        let details = Self::fetch_video_details(fetcher, bvid).await?;
        let cover_url = normalize_cover_url(&details.pic);
        fetcher.validate_ssrf(&Url::parse(&cover_url)?).await?;
        let default_name = format!("{bvid}_cover.jpg");
        let target_path = resolve_download_path(dest_path, &default_name);

        let cookie_str = Url::parse(&cover_url)
            .ok()
            .and_then(|u| fetcher.resolve_cookie_for_url(&u));

        download_file(
            fetcher.client(),
            DownloadOptions {
                url: &cover_url,
                destination: &target_path,
                referer: Some("https://www.bilibili.com/"),
                cookie: cookie_str.as_deref(),
                reporter,
                concurrency: None,
                allow_private_addresses: fetcher.options().allow_private_addresses,
            },
        )
        .await?;

        Ok(target_path)
    }

    /// Downloads the progressive MP4 video stream to the destination path.
    pub async fn download_video(
        fetcher: &crate::fetcher::Fetcher,
        bvid: &str,
        page_num: usize,
        quality: VideoQuality,
        dest_path: Option<&Path>,
        reporter: Option<&dyn DownloadProgressReporter>,
    ) -> Result<PathBuf, TklessError> {
        let details = Self::fetch_video_details(fetcher, bvid).await?;

        // Resolve target CID based on page_num (1-indexed)
        let cid = if page_num > 0 && page_num <= details.pages.len() {
            details.pages[page_num - 1].cid
        } else if details.cid > 0 {
            details.cid
        } else if let Some(first_page) = details.pages.first() {
            first_page.cid
        } else {
            return Err(TklessError::AdapterError {
                platform: "bilibili",
                message: "No valid video stream CID found".to_string(),
            });
        };

        let play_data = Self::fetch_play_url(fetcher, bvid, cid, quality).await?;
        let durl_item = play_data
            .durl
            .first()
            .ok_or_else(|| TklessError::AdapterError {
                platform: "bilibili",
                message: "No stream download URL found in playurl response".to_string(),
            })?;

        let clean_title = sanitize_filename(&details.title);
        let default_filename = if clean_title.is_empty() {
            format!("{bvid}.mp4")
        } else {
            format!("{clean_title}.mp4")
        };
        let target_path = resolve_download_path(dest_path, &default_filename);

        // Gather primary URL and all backup CDN mirror URLs
        let mut candidate_urls = Vec::new();
        candidate_urls.push(durl_item.url.clone());
        candidate_urls.extend(durl_item.backup_url.clone());

        // Sort candidate URLs by CDN quality:
        // Tier 0: High-speed backbone CDNs (Aliyun, Tencent, Baidu, Akamai)
        // Tier 1: General official CDN mirrors
        // Tier 2: Deprioritized PCDN/P2P/low-upload nodes (mcdn, mountaintoys, p2p, :4483)
        candidate_urls.sort_by_key(|u| {
            let u_lower = u.to_lowercase();
            if u_lower.contains("mountaintoys")
                || u_lower.contains("mcdn")
                || u_lower.contains("szbdyd")
                || u_lower.contains(":4483")
                || u_lower.contains("p2p")
            {
                2
            } else if u_lower.contains("mirrorali")
                || u_lower.contains("mirrortencent")
                || u_lower.contains("mirrorbos")
                || u_lower.contains("mirrorakam")
                || u_lower.contains("bilivideo.com")
            {
                0
            } else {
                1
            }
        });

        let mut last_err = None;
        let mut downloaded = false;

        for stream_url in &candidate_urls {
            if let Err(error) = fetcher.validate_ssrf(&Url::parse(stream_url)?).await {
                last_err = Some(error);
                continue;
            }
            debug!(url = %stream_url, "Attempting video stream download from candidate mirror");
            let cookie_str = Url::parse(stream_url)
                .ok()
                .and_then(|u| fetcher.resolve_cookie_for_url(&u));

            match download_file(
                fetcher.client(),
                DownloadOptions {
                    url: stream_url,
                    destination: &target_path,
                    referer: Some("https://www.bilibili.com/"),
                    cookie: cookie_str.as_deref(),
                    reporter,
                    concurrency: Some(4), // 4-thread concurrent HTTP Range download to saturate bandwidth
                    allow_private_addresses: fetcher.options().allow_private_addresses,
                },
            )
            .await
            {
                Ok(_) => {
                    downloaded = true;
                    break;
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        url = %stream_url,
                        "Video stream download from candidate mirror failed, falling back to next backup mirror..."
                    );
                    last_err = Some(e);
                }
            }
        }

        if !downloaded {
            return Err(last_err.unwrap_or_else(|| TklessError::AdapterError {
                platform: "bilibili",
                message: "All video stream download mirror URLs failed".to_string(),
            }));
        }

        Ok(target_path)
    }

    async fn extract_video(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        bvid: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        let data = Self::fetch_video_details(fetcher, bvid).await?;

        let title = data.title.clone();
        let author = data.owner.name.clone();
        let (pub_date_friendly, pub_date_rfc3339) = format_timestamp(data.pubdate);
        let cover_url = normalize_cover_url(&data.pic);

        // Metadata
        let mut metadata = PageMetadata {
            description: Some(data.desc.clone()),
            author: Some(author.clone()),
            canonical_url: Some(format!("https://www.bilibili.com/video/{bvid}")),
            published_time: Some(pub_date_rfc3339.clone()),
            cover_image: Some(cover_url.clone()),
            ..Default::default()
        };
        metadata.open_graph.insert("og:title".into(), title.clone());
        metadata
            .open_graph
            .insert("og:description".into(), data.desc.clone());
        metadata
            .open_graph
            .insert("og:image".into(), cover_url.clone());

        // JSON-LD structured VideoObject
        let json_ld_object = serde_json::json!({
            "@context": "https://schema.org",
            "@type": "VideoObject",
            "name": title,
            "description": data.desc,
            "thumbnailUrl": cover_url,
            "uploadDate": pub_date_rfc3339,
            "duration": format!("PT{}S", data.duration),
            "author": {
                "@type": "Person",
                "name": author
            },
            "interactionStatistic": [
                {
                    "@type": "InteractionCounter",
                    "interactionType": "https://schema.org/WatchAction",
                    "userInteractionCount": data.stat.view
                },
                {
                    "@type": "InteractionCounter",
                    "interactionType": "https://schema.org/LikeAction",
                    "userInteractionCount": data.stat.like
                }
            ]
        });

        // Format Markdown content
        let mut content = String::new();
        if options.frontmatter && options.format == OutputFormat::Markdown {
            content.push_str("---\n");
            content.push_str(&format!("title: \"{}\"\n", sanitize_quotes(&title)));
            content.push_str(&format!("author: \"{}\"\n", sanitize_quotes(&author)));
            content.push_str(&format!("url: \"https://www.bilibili.com/video/{bvid}\"\n"));
            content.push_str(&format!("cover: \"{cover_url}\"\n"));
            content.push_str(&format!("published: \"{pub_date_rfc3339}\"\n"));
            content.push_str(&format!("bvid: \"{bvid}\"\n"));
            content.push_str("---\n\n");
        }

        content.push_str(&format!("# {}\n\n", title));

        if !options.strip_images && !cover_url.is_empty() {
            content.push_str(&format!("![封面]({cover_url})\n\n"));
        }

        content.push_str(&format!(
            "- **UP主**: {} (UID: {})\n",
            author, data.owner.mid
        ));
        content.push_str(&format!("- **发布时间**: {}\n", pub_date_friendly));
        content.push_str(&format!("- **时长**: {}\n", format_duration(data.duration)));
        content.push_str(&format!(
            "- **数据统计**: 播放 {} | 弹幕 {} | 点赞 {} | 投币 {} | 收藏 {} | 转发 {}\n\n",
            format_count(data.stat.view),
            format_count(data.stat.danmaku),
            format_count(data.stat.like),
            format_count(data.stat.coin),
            format_count(data.stat.favorite),
            format_count(data.stat.share)
        ));

        if !data.desc.trim().is_empty() {
            content.push_str("### 视频简介\n\n");
            content.push_str(&data.desc);
            content.push_str("\n\n");
        }

        // Subtitles extraction
        let mut subtitles = None;
        if options.subtitles {
            match Self::fetch_subtitles(fetcher, bvid, data.cid, &data).await {
                Ok(lines) => {
                    if !lines.is_empty() {
                        content.push_str("### 视频字幕 (Subtitles)\n\n");
                        for line in &lines {
                            content.push_str(&format!(
                                "[{}] {}\n",
                                format_subtitle_range(line.from, line.to),
                                line.content.trim()
                            ));
                        }
                        content.push('\n');
                        subtitles = Some(lines);
                    } else {
                        tracing::info!(
                            bvid = bvid,
                            "No CC or AI subtitle track found for this video on Bilibili (video may use hardcoded/burned-in subtitles)"
                        );
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to fetch video subtitles");
                }
            }
        }

        // Danmaku extraction
        let mut danmaku = None;
        if options.danmaku {
            match Self::fetch_danmaku(fetcher, data.cid, options.danmaku_limit).await {
                Ok(entries) => {
                    if !entries.is_empty() {
                        content.push_str("### 视频时间轴弹幕 (Danmaku)\n\n");
                        content.push_str(&render_danmaku_table(&entries));
                        content.push('\n');
                        danmaku = Some(entries);
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to fetch video danmaku");
                }
            }
        }

        // Comments extraction
        let mut comments = None;
        if options.comments {
            match Self::fetch_comments(fetcher, data.aid, options.comments_limit).await {
                Ok(cmts) => {
                    if !cmts.is_empty() {
                        content.push_str("### 视频热门评论 (Comments)\n\n");
                        content.push_str(&render_comments_table(&cmts));
                        content.push('\n');
                        comments = Some(cmts);
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to fetch video comments");
                }
            }
        }

        // Official AI Video Conclusion extraction
        let mut conclusion = None;
        if options.conclusion {
            match Self::fetch_conclusion(fetcher, bvid, data.cid, data.owner.mid).await {
                Ok(Some(conc)) => {
                    content.push_str("### 视频 AI 总结与大纲 (AI Summary & Outline)\n\n");
                    if !conc.summary.trim().is_empty() {
                        content.push_str(&format!("**核心摘要**：{}\n\n", conc.summary.trim()));
                    }
                    if !conc.outline.is_empty() {
                        content.push_str("#### 章节提纲\n\n");
                        for ch in &conc.outline {
                            let time_str = format_duration(ch.timestamp);
                            content.push_str(&format!(
                                "- **[{}] {}**\n",
                                time_str,
                                ch.title.trim()
                            ));
                            for part in &ch.parts {
                                let part_time = format_duration(part.timestamp);
                                content.push_str(&format!(
                                    "  - [{}] {}\n",
                                    part_time,
                                    part.content.trim()
                                ));
                            }
                        }
                        content.push('\n');
                    }
                    conclusion = Some(conc);
                }
                Ok(None) => {
                    tracing::info!(
                        bvid = bvid,
                        "No official AI conclusion/summary available for this video"
                    );
                }
                Err(e) => {
                    warn!(error = %e, "Failed to fetch video AI conclusion");
                }
            }
        }

        let images = if options.extract_images && !cover_url.is_empty() {
            Some(vec![ExtractedImage {
                alt: "封面".to_string(),
                src: cover_url.clone(),
            }])
        } else {
            None
        };

        Ok(PageDocument {
            extraction: Default::default(),
            warnings: Vec::new(),
            url: url.to_string(),
            final_url: format!("https://www.bilibili.com/video/{bvid}"),
            status: 200,
            title: Some(title),
            byline: Some(author),
            content,
            metadata,
            json_ld: vec![json_ld_object],
            token_count: 0, // will be computed by engine
            tokenizer: options.tokenizer.as_str().to_string(),
            truncated: false,
            subtitles,
            danmaku,
            comments,
            conclusion,
            links: None,
            images,
            tables: None,
            entity: None,
            chunks: None,
            content_hash: None,
            metrics: None,
        })
    }

    async fn extract_ranking(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        rid: u32,
        category_name: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        let api_url =
            format!("https://api.bilibili.com/x/web-interface/ranking/v2?rid={rid}&type=all");

        let resp: BiliResponse<RankingData> = fetcher
            .read_json(
                fetcher
                    .get_str(&api_url)
                    .header("Referer", "https://www.bilibili.com/")
                    .send()
                    .await?,
            )
            .await?;

        if resp.code != 0 {
            warn!(
                code = resp.code,
                "Ranking API returned risk code or error ({}), falling back to popular video stream",
                resp.message
            );
            return self.extract_popular(fetcher, url, options).await;
        }

        let data = resp.data.ok_or_else(|| TklessError::AdapterError {
            platform: "bilibili",
            message: "Empty ranking list payload".to_string(),
        })?;

        let items: Vec<BiliVideoSummary> = data
            .list
            .into_iter()
            .map(|item| BiliVideoSummary {
                bvid: item.bvid,
                title: item.title,
                author: item.owner.name,
                views: item.stat.view,
                coins: item.stat.coin,
                duration: item.duration,
            })
            .collect();

        let title = format!("哔哩哔哩 - {}排行榜", category_name);
        self.build_feed_document(title, items, url, options, true)
    }

    async fn extract_popular(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        let api_url = "https://api.bilibili.com/x/web-interface/popular?pn=1&ps=20";

        let resp: BiliResponse<PopularData> = fetcher
            .read_json(
                fetcher
                    .get_str(api_url)
                    .header("Referer", "https://www.bilibili.com/")
                    .send()
                    .await?,
            )
            .await?;

        if resp.code != 0 {
            return Err(TklessError::AdapterError {
                platform: "bilibili",
                message: format!("Popular API error (code={}): {}", resp.code, resp.message),
            });
        }

        let data = resp.data.ok_or_else(|| TklessError::AdapterError {
            platform: "bilibili",
            message: "Empty popular feed payload".to_string(),
        })?;

        let items: Vec<BiliVideoSummary> = data
            .list
            .into_iter()
            .map(|item| BiliVideoSummary {
                bvid: item.bvid,
                title: item.title,
                author: item.owner.name,
                views: item.stat.view,
                coins: item.stat.coin,
                duration: item.duration,
            })
            .collect();

        let title = "哔哩哔哩 - 全站热门视频推荐".to_string();
        self.build_feed_document(title, items, url, options, true)
    }

    async fn extract_recommendations(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        let api_url = "https://api.bilibili.com/x/web-interface/index/top/feed/rcmd?ps=15";

        let resp: BiliResponse<RcmdData> = fetcher
            .read_json(
                fetcher
                    .get_str(api_url)
                    .header("Referer", "https://www.bilibili.com/")
                    .send()
                    .await?,
            )
            .await?;

        if resp.code != 0 {
            return Err(TklessError::AdapterError {
                platform: "bilibili",
                message: format!("Rcmd API error (code={}): {}", resp.code, resp.message),
            });
        }

        let data = resp.data.ok_or_else(|| TklessError::AdapterError {
            platform: "bilibili",
            message: "Empty recommendations feed payload".to_string(),
        })?;

        let items: Vec<BiliVideoSummary> = data
            .item
            .into_iter()
            .map(|item| BiliVideoSummary {
                bvid: item.bvid,
                title: item.title,
                author: item.owner.name,
                views: item.stat.view,
                coins: 0,
                duration: item.duration,
            })
            .collect();

        let title = "哔哩哔哩 - 首页个性化推荐视频".to_string();
        self.build_feed_document(title, items, url, options, false)
    }

    async fn extract_search(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        keyword: &str,
        page: u32,
        order: Option<&str>,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        let mut api_url =
            Url::parse("https://api.bilibili.com/x/web-interface/search/type").unwrap();
        api_url
            .query_pairs_mut()
            .append_pair("search_type", "video")
            .append_pair("keyword", keyword)
            .append_pair("page", &page.to_string());

        if let Some(ord) = order {
            if !ord.is_empty() {
                api_url.query_pairs_mut().append_pair("order", ord);
            }
        }

        let response = fetcher
            .get(&api_url)
            .header("Referer", "https://search.bilibili.com/")
            .header("Cookie", "buvid3=infoc;")
            .send()
            .await?;
        let resp: BiliResponse<SearchData> = fetcher.read_json(response).await?;

        if resp.code != 0 {
            return Err(TklessError::AdapterError {
                platform: "bilibili",
                message: format!("Search API error (code={}): {}", resp.code, resp.message),
            });
        }

        let data = resp.data.ok_or_else(|| TklessError::AdapterError {
            platform: "bilibili",
            message: "Empty search response data".to_string(),
        })?;

        let raw_items = data.result.unwrap_or_default();
        let items: Vec<SearchVideoItem> = raw_items
            .into_iter()
            .filter(|item| !item.bvid.trim().is_empty())
            .collect();

        let title = format!("哔哩哔哩 - 视频搜索: \"{}\" (第 {} 页)", keyword, page);
        let mut metadata = PageMetadata {
            description: Some(format!(
                "搜索关键词 \"{}\" 的结果列表，当前第 {} 页，共检索到 {} 条视频",
                keyword,
                page,
                items.len()
            )),
            canonical_url: Some(url.to_string()),
            ..Default::default()
        };
        metadata.open_graph.insert("og:title".into(), title.clone());

        let mut json_ld_list = Vec::new();
        for item in &items {
            let clean_title = clean_keyword_tags(&item.title);
            let clean_desc = clean_keyword_tags(&item.description);
            let dur_secs = parse_duration_str(&item.duration);
            let (_, pub_date_rfc3339) = format_timestamp(item.pubdate);

            json_ld_list.push(serde_json::json!({
                "@context": "https://schema.org",
                "@type": "VideoObject",
                "name": clean_title,
                "description": clean_desc,
                "identifier": item.bvid,
                "url": format!("https://www.bilibili.com/video/{}", item.bvid),
                "thumbnailUrl": normalize_cover_url(&item.pic),
                "uploadDate": pub_date_rfc3339,
                "duration": format!("PT{}S", dur_secs),
                "author": {
                    "@type": "Person",
                    "name": item.author
                },
                "interactionStatistic": [
                    {
                        "@type": "InteractionCounter",
                        "interactionType": "https://schema.org/WatchAction",
                        "userInteractionCount": item.play
                    },
                    {
                        "@type": "InteractionCounter",
                        "interactionType": "https://schema.org/CommentAction",
                        "userInteractionCount": item.danmaku
                    }
                ]
            }));
        }

        let mut content = String::new();
        if options.frontmatter && options.format == OutputFormat::Markdown {
            content.push_str("---\n");
            content.push_str(&format!("title: \"{}\"\n", sanitize_quotes(&title)));
            content.push_str(&format!("keyword: \"{}\"\n", sanitize_quotes(keyword)));
            content.push_str(&format!("page: {}\n", page));
            if let Some(ord) = order {
                content.push_str(&format!("order: \"{}\"\n", ord));
            }
            content.push_str(&format!("count: {}\n", items.len()));
            content.push_str("---\n\n");
        }

        content.push_str(&format!("# {}\n\n", title));
        content.push_str(&render_search_table(&items));

        Ok(PageDocument {
            extraction: Default::default(),
            warnings: Vec::new(),
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(title),
            byline: Some("bilibili".to_string()),
            content,
            metadata,
            json_ld: json_ld_list,
            token_count: 0,
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

    fn build_feed_document(
        &self,
        title: String,
        items: Vec<BiliVideoSummary>,
        url: &Url,
        options: &ExtractionOptions,
        include_coins: bool,
    ) -> Result<PageDocument, TklessError> {
        let mut metadata = PageMetadata {
            description: Some(format!("{}，共收录 {} 条视频条目", title, items.len())),
            canonical_url: Some(url.to_string()),
            ..Default::default()
        };
        metadata.open_graph.insert("og:title".into(), title.clone());

        let mut json_ld_list = Vec::new();
        for item in &items {
            json_ld_list.push(serde_json::json!({
                "@context": "https://schema.org",
                "@type": "VideoObject",
                "name": item.title,
                "identifier": item.bvid,
                "url": format!("https://www.bilibili.com/video/{}", item.bvid),
                "author": {
                    "@type": "Person",
                    "name": item.author
                },
                "interactionStatistic": {
                    "@type": "InteractionCounter",
                    "interactionType": "https://schema.org/WatchAction",
                    "userInteractionCount": item.views
                }
            }));
        }

        let mut content = String::new();
        if options.frontmatter && options.format == OutputFormat::Markdown {
            content.push_str("---\n");
            content.push_str(&format!("title: \"{}\"\n", sanitize_quotes(&title)));
            content.push_str(&format!("url: \"{url}\"\n"));
            content.push_str(&format!("count: {}\n", items.len()));
            content.push_str("---\n\n");
        }

        content.push_str(&format!("# {}\n\n", title));
        if include_coins {
            content.push_str(&render_video_table(&items));
        } else {
            content.push_str(&render_feed_table(&items));
        }

        Ok(PageDocument {
            extraction: Default::default(),
            warnings: Vec::new(),
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(title),
            byline: Some("bilibili".to_string()),
            content,
            metadata,
            json_ld: json_ld_list,
            token_count: 0,
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

impl PlatformAdapter for BilibiliAdapter {
    fn name(&self) -> &'static str {
        "bilibili"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "bilibili.com")
            || crate::fetcher::is_subdomain_or_exact(host, "b23.tv")
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

// -----------------------------------------------------------------------------
// Formatters and Renderers
// -----------------------------------------------------------------------------

struct BiliVideoSummary {
    bvid: String,
    title: String,
    author: String,
    views: u64,
    coins: u64,
    duration: u64,
}

fn render_video_table(items: &[BiliVideoSummary]) -> String {
    let mut table = String::new();
    table.push_str("| # | 视频标题 | BV号 | UP主 | 播放量 | 硬币 | 时长 | 链接 |\n");
    table.push_str("|---|---|---|---|---|---|---|---|\n");

    for (idx, item) in items.iter().enumerate() {
        let sanitized_title = sanitize_cell(&item.title);
        let sanitized_author = sanitize_cell(&item.author);
        let dur = format_duration(item.duration);
        let views = format_count(item.views);
        let coins = format_count(item.coins);
        let link = format!("https://www.bilibili.com/video/{}", item.bvid);

        table.push_str(&format!(
            "| {} | {} | `{}` | {} | {} | {} | {} | [观看]({}) |\n",
            idx + 1,
            sanitized_title,
            item.bvid,
            sanitized_author,
            views,
            coins,
            dur,
            link
        ));
    }

    table
}

fn render_feed_table(items: &[BiliVideoSummary]) -> String {
    let mut table = String::new();
    table.push_str("| # | 视频标题 | BV号 | UP主 | 播放量 | 时长 | 链接 |\n");
    table.push_str("|---|---|---|---|---|---|---|\n");

    for (idx, item) in items.iter().enumerate() {
        let sanitized_title = sanitize_cell(&item.title);
        let sanitized_author = sanitize_cell(&item.author);
        let dur = format_duration(item.duration);
        let views = format_count(item.views);
        let link = format!("https://www.bilibili.com/video/{}", item.bvid);

        table.push_str(&format!(
            "| {} | {} | `{}` | {} | {} | {} | [观看]({}) |\n",
            idx + 1,
            sanitized_title,
            item.bvid,
            sanitized_author,
            views,
            dur,
            link
        ));
    }

    table
}

fn render_search_table(items: &[SearchVideoItem]) -> String {
    let mut table = String::new();
    table.push_str("| # | 视频标题 | BV号 | UP主 | 播放量 | 弹幕 | 时长 | 发布时间 | 链接 |\n");
    table.push_str("|---|---|---|---|---|---|---|---|---|\n");

    for (idx, item) in items.iter().enumerate() {
        let clean_title = clean_keyword_tags(&item.title);
        let sanitized_title = sanitize_cell(&clean_title);
        let sanitized_author = sanitize_cell(&item.author);
        let dur_secs = parse_duration_str(&item.duration);
        let dur = format_duration(dur_secs);
        let views = format_count(item.play);
        let danmaku = format_count(item.danmaku);
        let (pub_date_friendly, _) = format_timestamp(item.pubdate);
        let link = format!("https://www.bilibili.com/video/{}", item.bvid);

        table.push_str(&format!(
            "| {} | {} | `{}` | {} | {} | {} | {} | {} | [观看]({}) |\n",
            idx + 1,
            sanitized_title,
            item.bvid,
            sanitized_author,
            views,
            danmaku,
            dur,
            pub_date_friendly,
            link
        ));
    }

    table
}

pub fn render_danmaku_table(entries: &[DanmakuEntry]) -> String {
    let mut table = String::new();
    table.push_str("| 出现时间 | 弹幕内容 |\n");
    table.push_str("|---|---|\n");
    for entry in entries {
        let time_str = format_seconds_friendly(entry.time);
        let text_clean = sanitize_cell(&entry.text);
        table.push_str(&format!("| {} | {} |\n", time_str, text_clean));
    }
    table
}

pub fn render_comments_table(comments: &[CommentEntry]) -> String {
    let mut table = String::new();
    table.push_str("| # | 评论用户 | 点赞数 | 评论内容 | 发布时间 |\n");
    table.push_str("|---|---|---|---|---|\n");
    for (idx, c) in comments.iter().enumerate() {
        let user = sanitize_cell(&c.user);
        let likes = format_count(c.like);
        let content = sanitize_cell(&c.content);
        table.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            idx + 1,
            user,
            likes,
            content,
            c.time
        ));
    }
    table
}

pub fn format_seconds_friendly(secs_f64: f64) -> String {
    let total_secs = secs_f64.max(0.0).round() as u64;
    format_duration(total_secs)
}

pub fn format_subtitle_range(from: f64, to: f64) -> String {
    format!(
        "{} - {}",
        format_seconds_friendly(from),
        format_seconds_friendly(to)
    )
}

pub fn decode_xml_body(bytes: &[u8]) -> Result<String, TklessError> {
    decode_xml_body_with_limit(bytes, 10 * 1024 * 1024)
}

fn decode_xml_body_with_limit(bytes: &[u8], limit: usize) -> Result<String, TklessError> {
    if bytes.len() > limit {
        return Err(TklessError::ResponseTooLarge {
            limit,
            received: bytes.len(),
        });
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        if s.trim_start().starts_with('<') {
            return Ok(s.to_string());
        }
    }
    use std::io::Read;
    let decoders: [Box<dyn Read + '_>; 3] = [
        Box::new(flate2::read::DeflateDecoder::new(bytes)),
        Box::new(flate2::read::ZlibDecoder::new(bytes)),
        Box::new(flate2::read::GzDecoder::new(bytes)),
    ];
    for decoder in decoders {
        let mut expanded = Vec::new();
        let result = decoder
            .take(limit.saturating_add(1) as u64)
            .read_to_end(&mut expanded);
        if expanded.len() > limit {
            return Err(TklessError::ResponseTooLarge {
                limit,
                received: expanded.len(),
            });
        }
        if result.is_ok() {
            if let Ok(text) = String::from_utf8(expanded) {
                if text.trim_start().starts_with('<') {
                    return Ok(text);
                }
            }
        }
    }
    Ok(String::from_utf8_lossy(bytes).to_string())
}

pub fn parse_danmaku_xml(xml: &str) -> Vec<DanmakuEntry> {
    let mut entries = Vec::new();

    for cap in DANMAKU_RE.captures_iter(xml) {
        let p_attr = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let text_raw = cap.get(2).map(|m| m.as_str()).unwrap_or("");

        let time = p_attr
            .split(',')
            .next()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        let clean_text = clean_keyword_tags(text_raw).trim().to_string();
        if !clean_text.is_empty() {
            entries.push(DanmakuEntry {
                time,
                text: clean_text,
            });
        }
    }

    entries.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    entries
}

pub fn clean_keyword_tags(text: &str) -> String {
    let stripped = EM_RE.replace_all(text, "").to_string();
    stripped
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

pub fn parse_duration_str(dur_str: &str) -> u64 {
    let parts: Vec<&str> = dur_str.trim().split(':').collect();
    match parts.len() {
        1 => parts[0].parse::<u64>().unwrap_or(0),
        2 => {
            let mins = parts[0].parse::<u64>().unwrap_or(0);
            let secs = parts[1].parse::<u64>().unwrap_or(0);
            mins * 60 + secs
        }
        3 => {
            let hours = parts[0].parse::<u64>().unwrap_or(0);
            let mins = parts[1].parse::<u64>().unwrap_or(0);
            let secs = parts[2].parse::<u64>().unwrap_or(0);
            hours * 3600 + mins * 60 + secs
        }
        _ => 0,
    }
}

pub fn format_count(count: u64) -> String {
    if count >= 100_000_000 {
        format!("{:.1}亿", count as f64 / 100_000_000.0)
    } else if count >= 10_000 {
        format!("{:.1}万", count as f64 / 10_000.0)
    } else {
        count.to_string()
    }
}

pub fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{:02}:{:02}", mins, secs)
    }
}

fn sanitize_cell(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.contains('|') && !trimmed.contains('\r') && !trimmed.contains('\n') {
        return trimmed.to_string();
    }
    let mut out = String::with_capacity(trimmed.len() + 4);
    for c in trimmed.chars() {
        match c {
            '|' => out.push_str("\\|"),
            '\r' | '\n' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

fn sanitize_quotes(text: &str) -> String {
    text.replace('"', "\\\"")
}

/// Formats unix timestamp into (human_readable_local, rfc3339_iso8601).
pub fn format_timestamp(ts: u64) -> (String, String) {
    if let Some(dt) = chrono::DateTime::from_timestamp(ts as i64, 0) {
        let local_str = dt.format("%Y-%m-%d %H:%M:%S").to_string();
        let rfc3339_str = dt.to_rfc3339();
        (local_str, rfc3339_str)
    } else {
        (ts.to_string(), ts.to_string())
    }
}

fn rid_to_category_name(rid: u32) -> &'static str {
    match rid {
        0 => "全站",
        1 => "动画",
        168 => "国创",
        3 => "音乐",
        129 => "舞蹈",
        4 => "游戏",
        188 => "科技数码",
        36 => "知识",
        223 => "汽车",
        234 => "运动",
        160 => "生活",
        211 => "美食",
        217 => "动物圈",
        119 => "鬼畜",
        155 => "时尚",
        5 => "娱乐",
        181 => "影视",
        _ => "分区",
    }
}

/// Normalizes protocol-relative URL (//i0.hdslb.com/...) or http:// into https://
pub fn normalize_cover_url(pic: &str) -> String {
    let trimmed = pic.trim();
    if trimmed.starts_with("//") {
        format!("https:{trimmed}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("https://{rest}")
    } else {
        trimmed.to_string()
    }
}

/// Normalizes a target string: if input is a bare BV id or av id, expands it to a full Bilibili URL.
pub fn normalize_bilibili_url(input: &str) -> String {
    let trimmed = input.trim();
    if (trimmed.starts_with("BV")
        || trimmed.starts_with("bv")
        || trimmed.starts_with("av")
        || trimmed.starts_with("AV"))
        && !trimmed.contains("://")
    {
        format!("https://www.bilibili.com/video/{trimmed}")
    } else {
        trimmed.to_string()
    }
}

/// Extracts BVID and optional page number (1-indexed) from a Bilibili video URL or identifier.
pub fn extract_video_info(input: &str) -> Option<(String, usize)> {
    let normalized = normalize_bilibili_url(input);
    let parsed = Url::parse(&normalized).ok()?;
    let path = parsed.path().trim_end_matches('/');

    let id_part = if let Some(pos) = path.find("/video/") {
        &path[pos + 7..]
    } else {
        path.trim_start_matches('/')
    };

    let bvid = id_part.split('/').next()?.split('?').next()?;
    if bvid.is_empty() {
        return None;
    }

    let mut page_num = 1;
    for (k, v) in parsed.query_pairs() {
        if k == "p" {
            if let Ok(p) = v.parse::<usize>() {
                page_num = p;
            }
        }
    }

    Some((bvid.to_string(), page_num))
}

/// Sanitizes filename for safe creation on Windows, macOS, and Linux filesystems.
pub fn sanitize_filename(name: &str) -> String {
    let invalid_chars = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    let mut sanitized = String::with_capacity(name.len());
    for c in name.chars() {
        if invalid_chars.contains(&c) || c.is_control() {
            sanitized.push('_');
        } else {
            sanitized.push(c);
        }
    }
    let trimmed = sanitized.trim().trim_matches('.');
    if trimmed.chars().count() > 100 {
        trimmed.chars().take(100).collect()
    } else {
        trimmed.to_string()
    }
}

/// Resolves destination download path: if user provided path is a directory,
/// append default_filename; otherwise use user path; if None, use current dir + default_filename.
pub fn resolve_download_path(user_path: Option<&Path>, default_filename: &str) -> PathBuf {
    match user_path {
        Some(p) => {
            if p.is_dir()
                || p.to_string_lossy().ends_with('/')
                || p.to_string_lossy().ends_with('\\')
            {
                p.join(default_filename)
            } else {
                p.to_path_buf()
            }
        }
        None => PathBuf::from(default_filename),
    }
}

/// Standard RFC 1321 MD5 hash implementation in pure Rust.
#[allow(clippy::chunks_exact_to_as_chunks)]
pub fn compute_md5(input: &[u8]) -> String {
    let s: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];

    let k: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let orig_len_bits = (input.len() as u64).wrapping_mul(8);
    let mut msg = input.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0x00);
    }
    msg.extend_from_slice(&orig_len_bits.to_le_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, c) in chunk.chunks_exact(4).enumerate() {
            m[i] = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        }

        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };

            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                (a.wrapping_add(f).wrapping_add(k[i]).wrapping_add(m[g])).rotate_left(s[i]),
            );
            a = temp;
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut result = [0u8; 16];
    result[0..4].copy_from_slice(&a0.to_le_bytes());
    result[4..8].copy_from_slice(&b0.to_le_bytes());
    result[8..12].copy_from_slice(&c0.to_le_bytes());
    result[12..16].copy_from_slice(&d0.to_le_bytes());

    result.iter().map(|b| format!("{b:02x}")).collect()
}

pub async fn get_wbi_mixin_key(fetcher: &crate::fetcher::Fetcher) -> Result<String, TklessError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if let Ok(guard) = WBI_CACHE.read() {
        if let Some((ref key, cached_at)) = *guard {
            if now < cached_at + 12 * 3600 {
                return Ok(key.clone());
            }
        }
    }

    let nav_url = "https://api.bilibili.com/x/web-interface/nav";
    let resp: BiliResponse<NavData> = fetcher
        .read_json(
            fetcher
                .get_str(nav_url)
                .header("Referer", "https://www.bilibili.com/")
                .send()
                .await?,
        )
        .await?;

    let wbi_img = resp
        .data
        .and_then(|d| d.wbi_img)
        .ok_or_else(|| TklessError::AdapterError {
            platform: "bilibili",
            message: "Failed to extract wbi_img keys from nav endpoint".to_string(),
        })?;

    let img_key = wbi_img
        .img_url
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("");
    let sub_key = wbi_img
        .sub_url
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("");
    let raw_key = format!("{img_key}{sub_key}");

    if raw_key.len() < 64 {
        return Err(TklessError::AdapterError {
            platform: "bilibili",
            message: "Invalid WBI raw key length".to_string(),
        });
    }

    let chars: Vec<char> = raw_key.chars().collect();
    let mixin_key: String = WBI_MIXIN_TABLE
        .iter()
        .take(32)
        .map(|&idx| chars[idx])
        .collect();

    if let Ok(mut guard) = WBI_CACHE.write() {
        *guard = Some((mixin_key.clone(), now));
    }

    Ok(mixin_key)
}

pub fn sign_wbi_params(params: &[(String, String)], mixin_key: &str) -> String {
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut query_parts = Vec::new();
    for (k, v) in &sorted {
        let clean_v: String = v.chars().filter(|c| !"!'()*".contains(*c)).collect();
        let enc_v: String = url::form_urlencoded::byte_serialize(clean_v.as_bytes()).collect();
        query_parts.push(format!("{k}={enc_v}"));
    }

    let query_str = query_parts.join("&");
    let sign_payload = format!("{query_str}{mixin_key}");
    let w_rid = compute_md5(sign_payload.as_bytes());

    format!("{query_str}&w_rid={w_rid}")
}

// -----------------------------------------------------------------------------
// Upstream API Payloads
// -----------------------------------------------------------------------------

#[derive(Deserialize)]
struct BiliResponse<T> {
    code: i32,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

#[derive(Deserialize, Clone)]
pub struct VideoViewData {
    #[serde(default)]
    pub aid: u64,
    pub bvid: String,
    pub title: String,
    pub pic: String,
    pub desc: String,
    pub duration: u64,
    pub pubdate: u64,
    pub owner: OwnerData,
    pub stat: StatData,
    #[serde(default)]
    pub cid: u64,
    #[serde(default)]
    pub pages: Vec<VideoPageItem>,
    #[serde(default)]
    pub subtitle: Option<BiliSubtitleViewWrapper>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct BiliSubtitleViewWrapper {
    #[serde(default)]
    pub list: Vec<BiliSubtitleItem>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct BiliSubtitleItem {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub lan: String,
    #[serde(default)]
    pub lan_doc: String,
    #[serde(default)]
    pub subtitle_url: String,
}

#[derive(Deserialize, Debug)]
struct SubtitleFileResponse {
    #[serde(default)]
    body: Vec<SubtitleRawItem>,
}

#[derive(Deserialize, Debug)]
struct SubtitleRawItem {
    #[serde(default)]
    from: f64,
    #[serde(default)]
    to: f64,
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct PlayerV2Data {
    subtitle: Option<PlayerV2SubtitleWrapper>,
}

#[derive(Deserialize)]
struct PlayerV2SubtitleWrapper {
    #[serde(default)]
    subtitles: Vec<BiliSubtitleItem>,
}

#[derive(Deserialize)]
struct ReplyData {
    #[serde(default)]
    replies: Option<Vec<ReplyItem>>,
}

#[derive(Deserialize)]
struct ReplyItem {
    #[serde(default)]
    ctime: u64,
    #[serde(default)]
    like: u64,
    member: ReplyMember,
    content: ReplyContent,
}

#[derive(Deserialize)]
struct ReplyMember {
    #[serde(default)]
    uname: String,
}

#[derive(Deserialize)]
struct ReplyContent {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize, Clone)]
pub struct VideoPageItem {
    pub cid: u64,
    pub page: u32,
    #[serde(default)]
    pub part: String,
    #[serde(default)]
    pub duration: u64,
}

#[derive(Deserialize)]
pub struct PlayUrlData {
    #[serde(default)]
    pub quality: u32,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub durl: Vec<PlayDurlItem>,
}

#[derive(Deserialize)]
pub struct PlayDurlItem {
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub length: u64,
    #[serde(default)]
    pub size: u64,
    pub url: String,
    #[serde(default)]
    pub backup_url: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct NavData {
    wbi_img: Option<NavWbiImg>,
}

#[derive(Deserialize, Debug)]
struct NavWbiImg {
    #[serde(default)]
    img_url: String,
    #[serde(default)]
    sub_url: String,
}

#[derive(Deserialize, Debug)]
struct ConclusionApiResponse {
    code: i32,
    #[serde(default)]
    message: String,
    data: Option<ConclusionApiData>,
}

#[derive(Deserialize, Debug)]
struct ConclusionApiData {
    #[serde(default)]
    code: i32,
    model_result: Option<ConclusionModelResult>,
}

#[derive(Deserialize, Debug)]
struct ConclusionModelResult {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    outline: Vec<ConclusionOutlineItem>,
}

#[derive(Deserialize, Debug)]
struct ConclusionOutlineItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    timestamp: u64,
    #[serde(default)]
    part_outline: Vec<ConclusionPartOutline>,
}

#[derive(Deserialize, Debug)]
struct ConclusionPartOutline {
    #[serde(default)]
    timestamp: u64,
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct RankingData {
    list: Vec<RankingItem>,
}

#[derive(Deserialize)]
struct RankingItem {
    bvid: String,
    title: String,
    duration: u64,
    owner: OwnerData,
    stat: StatData,
}

#[derive(Deserialize)]
struct PopularData {
    list: Vec<PopularItem>,
}

#[derive(Deserialize)]
struct PopularItem {
    bvid: String,
    title: String,
    duration: u64,
    owner: OwnerData,
    stat: StatData,
}

#[derive(Deserialize)]
struct RcmdData {
    item: Vec<RcmdItem>,
}

#[derive(Deserialize)]
struct RcmdItem {
    bvid: String,
    title: String,
    duration: u64,
    owner: OwnerData,
    stat: StatData,
}

#[derive(Deserialize, Clone)]
pub struct OwnerData {
    pub mid: u64,
    pub name: String,
}

#[derive(Deserialize, Clone)]
pub struct StatData {
    view: u64,
    #[serde(default)]
    danmaku: u64,
    #[serde(default)]
    like: u64,
    #[serde(default)]
    coin: u64,
    #[serde(default)]
    favorite: u64,
    #[serde(default)]
    share: u64,
}

#[derive(Deserialize)]
struct SearchData {
    #[serde(default)]
    result: Option<Vec<SearchVideoItem>>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SearchVideoItem {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub bvid: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub pic: String,
    #[serde(default, deserialize_with = "deserialize_u64_lenient")]
    pub play: u64,
    #[serde(default, deserialize_with = "deserialize_u64_lenient")]
    pub danmaku: u64,
    #[serde(default)]
    pub duration: String,
    #[serde(default, deserialize_with = "deserialize_u64_lenient")]
    pub pubdate: u64,
}

fn deserialize_u64_lenient<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = serde_json::Value::deserialize(deserializer)?;
    match val {
        serde_json::Value::Number(n) => Ok(n.as_u64().unwrap_or(0)),
        serde_json::Value::String(s) => Ok(s.trim().parse::<u64>().unwrap_or(0)),
        _ => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn compressed_xml_cannot_expand_past_response_budget() {
        use std::io::Write;
        let xml = format!("<i>{}</i>", "x".repeat(4096));
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(xml.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(matches!(
            super::decode_xml_body_with_limit(&compressed, 512),
            Err(crate::TklessError::ResponseTooLarge { .. })
        ));
        assert_eq!(
            super::decode_xml_body_with_limit(&compressed, 8192).unwrap(),
            xml
        );
    }
    use super::*;

    #[test]
    fn test_format_count() {
        assert_eq!(format_count(500), "500");
        assert_eq!(format_count(15000), "1.5万");
        assert_eq!(format_count(150000000), "1.5亿");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(45), "00:45");
        assert_eq!(format_duration(365), "06:05");
        assert_eq!(format_duration(3665), "01:01:05");
    }

    #[test]
    fn test_sanitize_cell() {
        assert_eq!(sanitize_cell("A | B | C"), "A \\| B \\| C");
        assert_eq!(sanitize_cell("Line1\nLine2"), "Line1 Line2");
    }

    #[test]
    fn test_parse_route_video() {
        let url = Url::parse("https://www.bilibili.com/video/BV1xx411c7mD?p=1").unwrap();
        let options = ExtractionOptions::default();
        let route = BilibiliAdapter::parse_route(&url, &options);
        assert_eq!(route, Some(BilibiliRoute::Video("BV1xx411c7mD".into())));
    }

    #[test]
    fn test_parse_route_rankings() {
        let url = Url::parse("https://www.bilibili.com/v/popular/rank/tech").unwrap();
        let options = ExtractionOptions::default();
        let route = BilibiliAdapter::parse_route(&url, &options);
        assert_eq!(
            route,
            Some(BilibiliRoute::Ranking {
                rid: 188,
                category_name: "科技数码".into()
            })
        );
    }

    #[test]
    fn test_parse_route_recommendations() {
        let url = Url::parse("https://www.bilibili.com/").unwrap();
        let options = ExtractionOptions::default();
        let route = BilibiliAdapter::parse_route(&url, &options);
        assert_eq!(route, Some(BilibiliRoute::Recommendations));
    }

    #[test]
    fn test_parse_route_search() {
        let url =
            Url::parse("https://search.bilibili.com/all?keyword=Rust&page=2&order=click").unwrap();
        let options = ExtractionOptions::default();
        let route = BilibiliAdapter::parse_route(&url, &options);
        assert_eq!(
            route,
            Some(BilibiliRoute::Search {
                keyword: "Rust".into(),
                page: 2,
                order: Some("click".into()),
            })
        );
    }

    #[test]
    fn test_clean_keyword_tags() {
        let raw = r#"<em class="keyword">Rust</em>编程语言&amp;入门"#;
        assert_eq!(clean_keyword_tags(raw), "Rust编程语言&入门");
    }

    #[test]
    fn test_parse_duration_str() {
        assert_eq!(parse_duration_str("45"), 45);
        assert_eq!(parse_duration_str("06:05"), 365);
        assert_eq!(parse_duration_str("01:01:05"), 3665);
    }

    #[test]
    fn test_format_subtitle_range() {
        assert_eq!(format_subtitle_range(85.0, 90.0), "01:25 - 01:30");
        assert_eq!(format_subtitle_range(3605.0, 3610.0), "01:00:05 - 01:00:10");
    }

    #[test]
    fn test_parse_danmaku_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<i>
    <chatserver>chat.bilibili.com</chatserver>
    <chatid>123456</chatid>
    <d p="85.45,1,25,16777215,1612345678,0,abc1234,12345678">前方高能！</d>
    <d p="12.30,1,25,16777215,1612345670,0,def5678,12345679">第一条弹幕&amp;测试</d>
</i>"#;
        let danmaku = parse_danmaku_xml(xml);
        assert_eq!(danmaku.len(), 2);
        // Ordered by time ascending
        assert_eq!(danmaku[0].time, 12.30);
        assert_eq!(danmaku[0].text, "第一条弹幕&测试");
        assert_eq!(danmaku[1].time, 85.45);
        assert_eq!(danmaku[1].text, "前方高能！");

        let rendered = render_danmaku_table(&danmaku);
        assert!(rendered.contains("| 00:12 | 第一条弹幕&测试 |"));
        assert!(rendered.contains("| 01:25 | 前方高能！ |"));
    }

    #[test]
    fn test_parse_subtitle_json() {
        let json_str = r#"{
            "body": [
                { "from": 1.5, "to": 4.8, "content": "大家好，欢迎来到本期视频。" },
                { "from": 5.2, "to": 9.0, "content": "今天我们将探讨 Rust 的并发特性。" }
            ]
        }"#;
        let sub_resp: SubtitleFileResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(sub_resp.body.len(), 2);
        assert_eq!(sub_resp.body[0].content, "大家好，欢迎来到本期视频。");
        assert_eq!(sub_resp.body[1].from, 5.2);
    }

    #[test]
    fn test_render_comments_table() {
        let comments = vec![CommentEntry {
            user: "Alice | Dev".to_string(),
            like: 15200,
            content: "Very informative video!\nThanks.".to_string(),
            time: "2026-03-01 12:00:00".to_string(),
        }];
        let rendered = render_comments_table(&comments);
        assert!(rendered.contains("| # | 评论用户 | 点赞数 | 评论内容 | 发布时间 |"));
        assert!(rendered.contains("Alice \\| Dev"));
        assert!(rendered.contains("1.5万"));
        assert!(rendered.contains("Very informative video! Thanks."));
    }
}
