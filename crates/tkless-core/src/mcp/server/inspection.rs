use super::{Arc, McpServer, McpToolCallResult, TklessError, Value};
use url::Url;

impl McpServer {
    pub(super) async fn call_fetch_feed(&self, args: &Value) -> McpToolCallResult {
        let channel = if let Some(raw_xml) = args.get("xml").and_then(|v| v.as_str()) {
            crate::feed::parse_feed(raw_xml)
        } else if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
            let parsed_url = match url::Url::parse(url) {
                Ok(u) => u,
                Err(e) => return McpToolCallResult::error(format!("Invalid URL: {e}")),
            };
            if let Err(e) = self.engine.fetcher().validate_ssrf(&parsed_url).await {
                return McpToolCallResult::error(format!("SSRF blocked: {e}"));
            }
            match self.engine.fetcher().fetch(url).await {
                Ok(resp) => crate::feed::parse_feed(&resp.html),
                Err(e) => return McpToolCallResult::error(format!("Failed to fetch feed: {e}")),
            }
        } else {
            return McpToolCallResult::error(
                "Either 'url' or 'xml' parameter must be provided to fetch_feed",
            );
        };

        let mut feed_channel = match channel {
            Ok(c) => c,
            Err(e) => {
                return McpToolCallResult::error(format!("Failed to parse syndication feed: {e}"))
            }
        };

        if let Some(limit) = args.get("limit").and_then(|v| v.as_u64()) {
            feed_channel.items.truncate(limit as usize);
        }

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");
        match format {
            "json" => match serde_json::to_string_pretty(&feed_channel) {
                Ok(json_str) => McpToolCallResult::success(json_str),
                Err(e) => {
                    McpToolCallResult::error(format!("Failed to serialize feed to JSON: {e}"))
                }
            },
            _ => McpToolCallResult::success(crate::feed::format_feed_markdown(&feed_channel)),
        }
    }

    pub(super) async fn call_inspect_links(&self, args: &Value) -> McpToolCallResult {
        let urls = match args.get("urls").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>(),
            _ => return McpToolCallResult::error("Missing or empty 'urls' array"),
        };

        let concurrency = args
            .get("concurrency")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 30) as usize;

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 60);

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
        let client = self.engine.client().clone();
        let fetcher = self.engine.fetcher().clone();

        let mut join_set = tokio::task::JoinSet::new();

        for (idx, target_url) in urls.into_iter().enumerate() {
            let sem = semaphore.clone();
            let cli = client.clone();
            let parsed_res = Url::parse(&target_url);

            let ssrf_res = if let Ok(ref p_url) = parsed_res {
                fetcher.validate_ssrf(p_url).await
            } else {
                Err(TklessError::InvalidArgument(format!(
                    "Invalid URL: {target_url}"
                )))
            };

            join_set.spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let start = std::time::Instant::now();

                if let Err(e) = ssrf_res {
                    return (
                        idx,
                        target_url,
                        0u16,
                        start.elapsed().as_millis() as u64,
                        None::<String>,
                        Some(format!("SSRF Blocked: {e}")),
                    );
                }

                let req_timeout = std::time::Duration::from_secs(timeout_secs);
                let resp_res = cli.head(&target_url).timeout(req_timeout).send().await;

                let (status, final_url, err) = match resp_res {
                    Ok(resp) => {
                        let st = resp.status().as_u16();
                        let redirect_target = if (300..400).contains(&st) {
                            resp.headers()
                                .get(reqwest::header::LOCATION)
                                .and_then(|h| h.to_str().ok())
                                .and_then(|loc| {
                                    Url::parse(&target_url)
                                        .ok()
                                        .and_then(|base| base.join(loc).ok())
                                })
                                .map(|u| u.to_string())
                        } else {
                            None
                        };
                        let final_dest = redirect_target.or_else(|| {
                            if resp.url().as_str() != target_url {
                                Some(resp.url().to_string())
                            } else {
                                None
                            }
                        });
                        (st, final_dest, None)
                    }
                    Err(_) => {
                        let get_res = cli
                            .get(&target_url)
                            .header("Range", "bytes=0-100")
                            .timeout(req_timeout)
                            .send()
                            .await;

                        match get_res {
                            Ok(resp) => {
                                let st = resp.status().as_u16();
                                let redirect_target = if (300..400).contains(&st) {
                                    resp.headers()
                                        .get(reqwest::header::LOCATION)
                                        .and_then(|h| h.to_str().ok())
                                        .and_then(|loc| {
                                            Url::parse(&target_url)
                                                .ok()
                                                .and_then(|base| base.join(loc).ok())
                                        })
                                        .map(|u| u.to_string())
                                } else {
                                    None
                                };
                                let final_dest = redirect_target.or_else(|| {
                                    if resp.url().as_str() != target_url {
                                        Some(resp.url().to_string())
                                    } else {
                                        None
                                    }
                                });
                                (st, final_dest, None)
                            }
                            Err(e) => (0, None, Some(e.to_string())),
                        }
                    }
                };

                let latency = start.elapsed().as_millis() as u64;
                (idx, target_url, status, latency, final_url, err)
            });
        }

        let mut results = Vec::new();
        while let Some(res) = join_set.join_next().await {
            if let Ok(item) = res {
                results.push(item);
            }
        }
        results.sort_by_key(|(idx, ..)| *idx);

        if format == "json" {
            let json_records: Vec<_> = results
                .into_iter()
                .map(|(_, url, status, latency_ms, final_url, error)| {
                    serde_json::json!({
                        "url": url,
                        "status": status,
                        "latency_ms": latency_ms,
                        "is_reachable": (200..400).contains(&status),
                        "is_redirect": final_url.is_some(),
                        "final_url": final_url,
                        "error": error
                    })
                })
                .collect();

            match serde_json::to_string_pretty(&json_records) {
                Ok(s) => McpToolCallResult::success(s),
                Err(e) => McpToolCallResult::error(format!("Serialization error: {e}")),
            }
        } else {
            let mut md = String::new();
            md.push_str("# 链接健康度与状态巡检报告 (Link Health Inspection)\n\n");
            md.push_str("| # | 目标链接 (URL) | 状态码 (Status) | 耗时 (Latency) | 重定向跳转 (Redirect) | 结果 (Health) |\n");
            md.push_str("|---|---|---|---|---|---|\n");

            for (idx, url, status, latency, final_url, err) in results {
                let status_str = if status > 0 {
                    format!("{status}")
                } else {
                    "Error".to_string()
                };

                let redirect_str = if let Some(dest) = final_url {
                    format!("[跳转]({dest})")
                } else {
                    "-".to_string()
                };

                let health_str = if (200..400).contains(&status) {
                    "可访问 (OK)".to_string()
                } else if let Some(e) = err {
                    format!("异常 ({e})")
                } else {
                    format!("异常 ({status})")
                };

                md.push_str(&format!(
                    "| {} | `{url}` | {} | {}ms | {} | {} |\n",
                    idx + 1,
                    status_str,
                    latency,
                    redirect_str,
                    health_str
                ));
            }

            McpToolCallResult::success(md)
        }
    }

    pub(super) async fn call_capture_screenshot(&self, args: &Value) -> McpToolCallResult {
        let raw_url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) if !u.trim().is_empty() => u.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'url'"),
        };
        let parsed_url = match Url::parse(raw_url) {
            Ok(u) => u,
            Err(e) => return McpToolCallResult::error(format!("Invalid URL: {e}")),
        };

        if let Err(e) = self.engine.fetcher().validate_ssrf(&parsed_url).await {
            return McpToolCallResult::error(format!("SSRF blocked: {e}"));
        }

        let cdp_ep = crate::cdp::resolve_cdp_endpoint(
            args.get("cdp_endpoint").and_then(|v| v.as_str()),
            true,
        )
        .unwrap_or_else(|| "http://127.0.0.1:9222".to_string());

        let wait_ms = args
            .get("cdp_wait_ms")
            .and_then(|v| v.as_u64())
            .or(Some(1000));
        let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("png");
        let output_path = args.get("output_path").and_then(|v| v.as_str());

        let cdp_opts = crate::cdp::CdpOptions {
            allow_private_addresses: self.engine.fetcher().options().allow_private_addresses,
            endpoint: cdp_ep,
            wait_ms,
            timeout_ms: None,
            user_agent: self.engine.fetcher().options().user_agent.clone(),
            cookie: self.engine.fetcher().resolve_cookie_for_url(&parsed_url),
            block_resources: Some(false),
            ..Default::default()
        };

        match crate::cdp::capture_screenshot_via_cdp(raw_url, &cdp_opts, Some(format)).await {
            Ok(bytes) => {
                if let Some(dest) = output_path {
                    if let Err(e) = std::fs::write(dest, &bytes) {
                        return McpToolCallResult::error(format!(
                            "Failed to write screenshot file '{dest}': {e}"
                        ));
                    }
                    McpToolCallResult::success(format!(
                        "页面高清快照截图成功！已保存至 `{dest}` (容量: {})",
                        crate::downloader::format_bytes(bytes.len() as u64)
                    ))
                } else {
                    let b64 = crate::cdp::encode_base64(&bytes);
                    McpToolCallResult::success(format!("data:image/{format};base64,{b64}"))
                }
            }
            Err(e) => McpToolCallResult::error(format!("CDP 截取网页快照失败: {e}")),
        }
    }
}
