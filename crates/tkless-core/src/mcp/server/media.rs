use super::{
    format_doc_result, BilibiliAdapter, ExtractionMode, ExtractionOptions, McpServer,
    McpToolCallResult, OutputFormat, Path, TokenizerKind, Value, VideoQuality,
};

impl McpServer {
    pub(super) async fn call_fetch_bilibili_feed(&self, args: &Value) -> McpToolCallResult {
        let feed_type = args
            .get("feed_type")
            .and_then(|v| v.as_str())
            .unwrap_or("recommendations");
        let partition = args
            .get("partition")
            .and_then(|v| v.as_str())
            .unwrap_or("all");
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };
        let max_tokens = args
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let use_cache = args
            .get("use_cache")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let (target_url, feed) = match feed_type {
            "ranking" => {
                if partition == "all" {
                    (
                        "https://www.bilibili.com/v/popular/rank/all".to_string(),
                        false,
                    )
                } else {
                    (
                        format!("https://www.bilibili.com/v/popular/rank/{partition}"),
                        false,
                    )
                }
            }
            "popular" => ("https://www.bilibili.com/v/popular/all".to_string(), false),
            _ => ("https://www.bilibili.com/".to_string(), true),
        };

        let options = ExtractionOptions {
            mode: ExtractionMode::Article,
            format,
            max_tokens,
            tokenizer: TokenizerKind::default(),
            include_metadata: true,
            include_json_ld: matches!(format, OutputFormat::Json),
            strip_images: false,
            strip_links: false,
            frontmatter: false,
            no_adapter: false,
            feed,
            ..Default::default()
        };

        let cache_key = if use_cache {
            Some(self.engine.cache_key(&self.cache, &target_url, &options))
        } else {
            None
        };

        if let Some(ref key) = cache_key {
            if let Some(cached_doc) = self.cache.get(key) {
                return format_doc_result(&cached_doc, format);
            }
        }

        match self.engine.process_url(&target_url, &options).await {
            Ok(doc) => {
                if let Some(ref key) = cache_key {
                    let _ = self.cache.set(key, &doc, None);
                }
                format_doc_result(&doc, format)
            }
            Err(e) => McpToolCallResult::error(format!("Failed to fetch Bilibili feed: {e}")),
        }
    }

    pub(super) async fn call_search_bilibili(&self, args: &Value) -> McpToolCallResult {
        let keyword = match args.get("keyword").and_then(|v| v.as_str()) {
            Some(k) if !k.trim().is_empty() => k.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'keyword'"),
        };

        let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
        let order = args.get("order").and_then(|v| v.as_str()).unwrap_or("");
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };
        let use_cache = args
            .get("use_cache")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let encoded_kw: String = url::form_urlencoded::byte_serialize(keyword.as_bytes()).collect();
        let mut target_url =
            format!("https://search.bilibili.com/all?keyword={encoded_kw}&page={page}");
        if !order.is_empty() {
            target_url.push_str(&format!("&order={order}"));
        }

        let options = ExtractionOptions {
            mode: ExtractionMode::Article,
            format,
            max_tokens: None,
            tokenizer: TokenizerKind::default(),
            include_metadata: true,
            include_json_ld: matches!(format, OutputFormat::Json),
            strip_images: false,
            strip_links: false,
            frontmatter: false,
            no_adapter: false,
            feed: false,
            ..Default::default()
        };

        let cache_key = if use_cache {
            Some(self.engine.cache_key(&self.cache, &target_url, &options))
        } else {
            None
        };

        if let Some(ref key) = cache_key {
            if let Some(cached_doc) = self.cache.get(key) {
                return format_doc_result(&cached_doc, format);
            }
        }

        match self.engine.process_url(&target_url, &options).await {
            Ok(doc) => {
                if let Some(ref key) = cache_key {
                    let _ = self.cache.set(key, &doc, None);
                }
                format_doc_result(&doc, format)
            }
            Err(e) => McpToolCallResult::error(format!("Failed to search Bilibili: {e}")),
        }
    }

    pub(super) async fn call_download_media(&self, args: &Value) -> McpToolCallResult {
        let url_or_bvid = match args.get("url_or_bvid").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'url_or_bvid'"),
        };

        let media_type = match args.get("media_type").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => {
                return McpToolCallResult::error(
                    "Missing required parameter 'media_type' ('video' or 'cover')",
                )
            }
        };

        let destination = args
            .get("destination")
            .and_then(|v| v.as_str())
            .map(Path::new);
        let quality = match args.get("quality").and_then(|v| v.as_str()) {
            Some("480p") => VideoQuality::Q480p,
            Some("360p") => VideoQuality::Q360p,
            _ => VideoQuality::Q720p,
        };
        let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(1) as usize;

        let bvid = crate::adapters::bilibili::extract_video_info(url_or_bvid)
            .map(|(b, _)| b)
            .unwrap_or_else(|| url_or_bvid.to_string());

        match media_type {
            "cover" => {
                match BilibiliAdapter::download_cover(
                    self.engine.fetcher(),
                    &bvid,
                    destination,
                    None,
                )
                .await
                {
                    Ok(path) => McpToolCallResult::success(format!(
                        "Cover image saved successfully to: {}",
                        path.display()
                    )),
                    Err(e) => McpToolCallResult::error(format!("Failed to download cover: {e}")),
                }
            }
            "video" => {
                match BilibiliAdapter::download_video(
                    self.engine.fetcher(),
                    &bvid,
                    page,
                    quality,
                    destination,
                    None,
                )
                .await
                {
                    Ok(path) => McpToolCallResult::success(format!(
                        "Video stream saved successfully to: {}",
                        path.display()
                    )),
                    Err(e) => McpToolCallResult::error(format!("Failed to download video: {e}")),
                }
            }
            _ => McpToolCallResult::error(format!(
                "Invalid media_type '{media_type}'. Expected 'video' or 'cover'."
            )),
        }
    }
}
