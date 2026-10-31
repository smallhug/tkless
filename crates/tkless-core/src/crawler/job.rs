use super::*;
use crate::{
    collection::{canonical_url, digest},
    state_file, DocumentCache,
};
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
struct State {
    version: u32,
    identity: String,
    queue: VecDeque<(String, usize)>,
    visited: HashSet<String>,
    pages: Vec<CrawledPage>,
    processed: usize,
    next_rank: usize,
}

async fn save(
    path: Option<&std::path::Path>,
    lock: &Option<std::sync::Arc<state_file::StateLock>>,
    state: &State,
    pending: &BTreeMap<usize, (String, usize)>,
) -> Result<(), TklessError> {
    if let Some(path) = path {
        let mut snapshot = state.clone();
        for entry in pending.values().rev() {
            snapshot.queue.push_front(entry.clone());
        }
        let path = path.to_path_buf();
        let lock = lock.clone();
        state_file::blocking(move || {
            let _lock = lock;
            state_file::write(&path, &snapshot)
        })
        .await?;
    }
    Ok(())
}

pub(super) async fn run<F>(
    engine: &TklessEngine,
    seed_url: &str,
    options: &CrawlOptions,
    extraction: &ExtractionOptions,
    mut progress: Option<F>,
) -> Result<SiteCrawlReport, TklessError>
where
    F: FnMut(usize, usize, &str) + Send,
{
    let seed = canonical_url(seed_url)?;
    if options.max_pages == 0
        || options.max_pages > 100_000
        || options.max_depth > 1000
        || options.delay_ms > 3_600_000
        || options.concurrency == 0
        || options.concurrency > 32
        || options.max_pending_urls == 0
        || options.max_seconds == Some(0)
        || options.max_seconds.is_some_and(|s| s > 86400)
        || options.max_output_bytes == Some(0)
        || (options.resume && options.checkpoint.is_none())
        || (options.retry_failed && !options.resume)
        || options.max_pending_urls > 100_000
    {
        return Err(TklessError::InvalidArgument(
            "Invalid crawl budget, concurrency, or resume without checkpoint".into(),
        ));
    }
    let path = options.checkpoint.as_deref();

    let mut identity_options = options.clone();
    identity_options.checkpoint = None;
    identity_options.resume = false;
    identity_options.max_pages = 20;
    identity_options.max_seconds = None;
    identity_options.max_output_bytes = None;
    identity_options.concurrency = 1;
    identity_options.max_pending_urls = 10_000;
    identity_options.retry_failed = false;
    identity_options.delay_ms = 0;
    let identity = digest(serde_json::to_vec(&(
        seed.as_str(),
        identity_options,
        engine.cache_key(&DocumentCache::memory_only(1), seed.as_str(), extraction),
    ))?);
    let owned_path = options.checkpoint.clone();
    let resume = options.resume;
    let seed_copy = seed.clone();
    let (mut state, lock) = state_file::blocking(move || {
        let path = owned_path.as_deref();
        let lock = path
            .map(state_file::StateLock::acquire)
            .transpose()?
            .map(std::sync::Arc::new);
        let seed = seed_copy;
        let state = if resume {
            let old: State = state_file::read(path.ok_or_else(|| {
                TklessError::InvalidArgument("Resume requires a checkpoint".into())
            })?)?;
            if old.version != 1 || old.identity != identity {
                return Err(TklessError::InvalidArgument(
                    "Checkpoint configuration or request identity mismatch".into(),
                ));
            }
            old
        } else {
            if path.is_some_and(|p| p.exists()) {
                return Err(TklessError::InvalidArgument(
                    "Checkpoint already exists; use resume or a new path".into(),
                ));
            }
            State {
                version: 1,
                identity,
                queue: VecDeque::from([(seed.to_string(), 0)]),
                visited: HashSet::from([normalize_url(&seed)]),
                pages: vec![],
                processed: 0,
                next_rank: 0,
            }
        };
        Ok((state, lock))
    })
    .await?;
    if options.retry_failed {
        let failed: Vec<_> = state
            .pages
            .iter()
            .filter(|p| p.error.is_some())
            .map(|p| (p.url.clone(), p.depth))
            .collect();
        state.processed = state.processed.saturating_sub(failed.len());
        state.pages.retain(|p| p.error.is_none());
        state.queue.extend(failed);
    }
    let exclude: Vec<_> = options
        .exclude_patterns
        .iter()
        .map(|p| {
            regex::Regex::new(p)
                .map_err(|e| TklessError::InvalidArgument(format!("Invalid exclude pattern: {e}")))
        })
        .collect::<Result<_, _>>()?;
    let mut pending = BTreeMap::new();
    save(path, &lock, &state, &pending).await?;
    let deadline = options
        .max_seconds
        .map(|seconds| tokio::time::Instant::now() + Duration::from_secs(seconds));
    let mut tasks = tokio::task::JoinSet::new();
    let robots_cache =
        std::sync::Arc::new(tokio::sync::Mutex::new(BTreeMap::<String, String>::new()));
    let mut next_launch = tokio::time::Instant::now();
    let mut reason = crate::workflow::StopReason::Exhausted;
    let mut output_bytes: u64 = state
        .pages
        .iter()
        .map(|p| serde_json::to_vec(p).map(|bytes| bytes.len() as u64))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .sum();
    let mut maps_seen = HashSet::new();
    'crawl: loop {
        if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
            reason = crate::workflow::StopReason::TimeBudget;
            break;
        }
        while tasks.len() < options.concurrency
            && state.processed + pending.len() < options.max_pages
        {
            let Some((url, depth)) = state.queue.pop_front() else {
                break;
            };
            state.next_rank += 1;
            let rank = state.next_rank;
            pending.insert(rank, (url.clone(), depth));
            let engine = engine.clone();
            let extraction = extraction.clone();
            let robots = options.respect_robots;
            let robots_cache = robots_cache.clone();
            let start = next_launch.max(tokio::time::Instant::now());
            next_launch = start + Duration::from_millis(options.delay_ms);
            tasks.spawn(async move {
                tokio::time::sleep_until(start).await;
                let result = async {
                    let parsed = canonical_url(&url)?;
                    if robots {
                        check_robots(&engine, &parsed, &robots_cache).await?;
                    }
                    fetch_raw(&engine, &url, &extraction).await
                }
                .await;
                (rank, url, depth, result)
            });
        }
        if tasks.is_empty() {
            if !state.queue.is_empty() {
                reason = crate::workflow::StopReason::PageLimit;
            }
            break;
        }
        let result = if let Some(deadline) = deadline {
            match tokio::time::timeout_at(deadline, tasks.join_next()).await {
                Ok(value) => value,
                Err(_) => {
                    reason = crate::workflow::StopReason::TimeBudget;
                    break;
                }
            }
        } else {
            tasks.join_next().await
        };
        let (rank, url, depth, fetched) = result.unwrap().map_err(TklessError::TaskFailed)?;
        let mut page = CrawledPage {
            rank,
            url: url.clone(),
            title: url.clone(),
            depth,
            content: String::new(),
            token_count: 0,
            status: 0,
            error: None,
        };
        let mut children = Vec::new();
        let mut is_map = false;
        match fetched {
            Ok((html, final_url, status)) => {
                page.status = status;
                let final_url = canonical_url(&final_url)?;
                if (options.sitemap || seed.path().ends_with(".xml"))
                    && (html.contains("<sitemapindex") || html.contains("<urlset"))
                {
                    is_map = true;
                    if maps_seen.len() >= 10_000 {
                        reason = crate::workflow::StopReason::SitemapLimit;
                        break;
                    }
                    maps_seen.insert(url.clone());
                    children.extend(
                        parse_sitemap_urls(&html)
                            .into_iter()
                            .filter_map(|s| canonical_url(&s).ok())
                            .map(|u| (u, 0)),
                    );
                } else {
                    match engine.process_html(&html, Some(final_url.as_str()), extraction) {
                        Ok(doc) if !doc.truncated && (200..300).contains(&status) => {
                            page.title = doc.title.unwrap_or_else(|| url.clone());
                            page.content = doc.content;
                            page.token_count = doc.token_count;
                            if depth < options.max_depth {
                                children.extend(
                                    extract_discovered_links(&html, &final_url)
                                        .into_iter()
                                        .map(|u| (u, depth + 1)),
                                );
                            }
                        }
                        Ok(_) => page.error = Some("Incomplete or unsuccessful page".into()),
                        Err(error) => page.error = Some(error.to_string()),
                    }
                }
            }
            Err(error) => page.error = Some(error.to_string()),
        }
        let mut new_children = Vec::new();
        let mut unique = HashSet::new();
        for (mut child, depth) in children {
            child.set_fragment(None);
            if should_enqueue_url(
                &child,
                seed.host_str().unwrap_or(""),
                seed.path(),
                options,
                &exclude,
            ) && !state.visited.contains(&normalize_url(&child))
                && unique.insert(normalize_url(&child))
            {
                new_children.push((child.to_string(), depth));
            }
        }
        let queue_exceeded =
            state.queue.len() + pending.len().saturating_sub(1) + new_children.len()
                > options.max_pending_urls;
        let size = if is_map {
            0
        } else {
            serde_json::to_vec(&page)?.len() as u64
        };
        let output_exceeded = options
            .max_output_bytes
            .is_some_and(|limit| output_bytes.saturating_add(size) > limit);
        if queue_exceeded {
            reason = crate::workflow::StopReason::QueueBudget;
        }
        if output_exceeded {
            reason = crate::workflow::StopReason::OutputBudget;
        }
        let stop_after_page = queue_exceeded || output_exceeded;

        // Commit the page that was already fetched before honoring the stop.
        // Breaking earlier discarded it and caused a refetch on resume.
        if !queue_exceeded {
            for (child, depth) in new_children {
                state.visited.insert(normalize_url(&canonical_url(&child)?));
                state.queue.push_back((child, depth));
            }
        }
        pending.remove(&rank);
        if !is_map {
            state.processed += 1;
            let duplicate = options.dedup
                && page.error.is_none()
                && state.pages.iter().any(|p| {
                    p.error.is_none()
                        && crate::dedup::is_near_duplicate(
                            crate::dedup::simhash(&p.content),
                            crate::dedup::simhash(&page.content),
                            3,
                        )
                });
            if !duplicate {
                output_bytes += size;
                state.pages.push(page);
            }
        }
        save(path, &lock, &state, &pending).await?;
        if let Some(cb) = &mut progress {
            cb(state.pages.len(), options.max_pages, &url);
        }
        if stop_after_page {
            break 'crawl;
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    save(path, &lock, &state, &pending).await?;
    state.pages.sort_by_key(|p| p.rank);
    let remaining = state.queue.len() + pending.len();
    let complete = remaining == 0 && state.pages.iter().all(|p| p.error.is_none());
    if reason == crate::workflow::StopReason::Exhausted && !complete {
        reason = crate::workflow::StopReason::PageErrors;
    }
    Ok(SiteCrawlReport {
        complete,
        stop_reason: reason,
        pending_urls: remaining,
        seed_url: seed.to_string(),
        total_pages: state.pages.len(),
        total_tokens: state.pages.iter().map(|p| p.token_count).sum(),
        pages: state.pages,
    })
}

async fn fetch_raw(
    engine: &TklessEngine,
    url: &str,
    extraction: &ExtractionOptions,
) -> Result<(String, String, u16), TklessError> {
    if extraction.cdp_endpoint.is_none() {
        match engine.fetcher().fetch(url).await {
            Ok(raw)
                if !raw.truncated
                    && (!extraction.auto_cdp
                        || !crate::cdp::is_anti_scraping_challenge(&raw.html, raw.status)) =>
            {
                return Ok((raw.html, raw.final_url, raw.status))
            }
            Err(error) if !extraction.auto_cdp => return Err(error),
            Ok(_) if !extraction.auto_cdp => {
                return Err(TklessError::InvalidArgument(
                    "Truncated crawl response".into(),
                ))
            }
            _ => {}
        }
    }
    engine.fetcher().validate_ssrf(&canonical_url(url)?).await?;
    let endpoint =
        crate::cdp::resolve_cdp_endpoint(extraction.cdp_endpoint.as_deref(), extraction.auto_cdp)
            .ok_or_else(|| TklessError::InvalidArgument("Missing CDP endpoint".into()))?;
    let options = crate::CdpOptions {
        endpoint,
        allow_private_addresses: engine.fetcher().options().allow_private_addresses,
        wait_ms: extraction.cdp_wait_ms,
        user_agent: engine.fetcher().options().user_agent.clone(),
        cookie: engine
            .fetcher()
            .resolve_cookie_for_url(&canonical_url(url)?),
        ..Default::default()
    };
    let page = crate::cdp::fetch_rendered_page(url, &options).await?;
    Ok((page.html, page.final_url, 200))
}

async fn check_robots(
    engine: &TklessEngine,
    url: &Url,
    cache: &tokio::sync::Mutex<BTreeMap<String, String>>,
) -> Result<(), TklessError> {
    let robots = url.join("/robots.txt")?;
    {
        let cache = cache.lock().await;
        if let Some(text) = cache.get(robots.as_str()) {
            return if robots_allowed(text, url) {
                Ok(())
            } else {
                Err(TklessError::InvalidArgument(
                    "robots.txt disallows this URL for TkLess".into(),
                ))
            };
        }
    }
    // Do not hold the cache mutex across the network request: one slow hosts'
    // robots.txt must not serialize every other crawl task.
    let raw = match engine.fetcher().fetch_api(robots.as_str()).await {
        Err(TklessError::HttpStatusError {
            status: 404 | 410, ..
        }) => {
            let mut cache = cache.lock().await;
            if cache.len() >= 1000 {
                cache.pop_first();
            }
            cache.insert(robots.to_string(), String::new());
            return Ok(());
        }
        result => result?,
    };
    if matches!(raw.status, 404 | 410) {
        let mut cache = cache.lock().await;
        if cache.len() >= 1000 {
            cache.pop_first();
        }
        cache.insert(robots.to_string(), String::new());
        return Ok(());
    }
    if !(200..300).contains(&raw.status) {
        return Err(TklessError::InvalidArgument(format!(
            "robots.txt returned HTTP {}",
            raw.status
        )));
    }
    if raw.truncated {
        return Err(TklessError::InvalidArgument("Truncated robots.txt".into()));
    }
    {
        let mut cache = cache.lock().await;
        if cache.len() >= 1000 {
            cache.pop_first();
        }
        cache.insert(robots.to_string(), raw.html.clone());
    }
    if !robots_allowed(&raw.html, url) {
        return Err(TklessError::InvalidArgument(
            "robots.txt disallows this URL for TkLess".into(),
        ));
    }
    Ok(())
}

fn robots_allowed(text: &str, url: &Url) -> bool {
    type RobotsGroup = (Vec<String>, Vec<(bool, String)>);
    let mut groups: Vec<RobotsGroup> = Vec::new();
    let mut rules_started = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "user-agent" => {
                if groups.is_empty() || rules_started {
                    groups.push((vec![], vec![]));
                }
                rules_started = false;
                groups
                    .last_mut()
                    .unwrap()
                    .0
                    .push(value.to_ascii_lowercase());
            }
            "allow" | "disallow" => {
                rules_started = true;
                if value.is_empty() {
                    continue;
                }
                if let Some(group) = groups.last_mut() {
                    group
                        .1
                        .push((key.trim().eq_ignore_ascii_case("allow"), value.into()));
                }
            }
            _ => {}
        }
    }
    let specific = groups.iter().any(|g| g.0.iter().any(|a| a == "tkless"));
    let target = format!(
        "{}{}",
        url.path(),
        url.query().map(|q| format!("?{q}")).unwrap_or_default()
    );
    let mut winner: Option<(usize, bool)> = None;
    for (agents, rules) in groups {
        if !agents
            .iter()
            .any(|a| if specific { a == "tkless" } else { a == "*" })
        {
            continue;
        }
        for (allow, pattern) in rules {
            let anchored = pattern.ends_with('$');
            let pattern = pattern.trim_end_matches('$');
            let regex = format!(
                "^{}{}",
                pattern
                    .split('*')
                    .map(regex::escape)
                    .collect::<Vec<_>>()
                    .join(".*"),
                if anchored { "$" } else { "" }
            );
            let length = pattern.replace('*', "").len();
            if regex::Regex::new(&regex).is_ok_and(|r| r.is_match(&target))
                && winner.is_none_or(|winner| length > winner.0 || length == winner.0 && allow)
            {
                winner = Some((length, allow));
            }
        }
    }
    winner.map(|w| w.1).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn robots_specific_groups_and_longest_allow_rule() {
        let url = Url::parse("https://example.com/private/public").unwrap();
        assert!(robots_allowed(
            "User-agent: *\nDisallow: /private\nAllow: /private/public$",
            &url
        ));
        assert!(!robots_allowed("User-agent: *\nDisallow: *", &url));
        assert!(robots_allowed(
            "User-agent: TkLess\nDisallow:\nUser-agent: *\nDisallow: /",
            &url
        ));
    }
}
