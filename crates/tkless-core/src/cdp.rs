//! Chrome DevTools Protocol (CDP) lightweight headless browser sidecar.
//!
//! Connects to an existing Chrome/Chromium/Edge browser instance running with `--remote-debugging-port=9222`
//! (or custom CDP WebSocket/HTTP endpoint), navigates to the target page, executes JavaScript,
//! waits for DOM hydration / anti-bot challenges to settle, and extracts the fully-rendered `outerHTML`.

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info};
use url::Url;

use crate::error::TklessError;

mod browser;
mod preflight;
mod steps;
pub(crate) use steps::validate as validate_steps;
pub use steps::BrowserStep;

pub(crate) use preflight::check as preflight_search;

/// Globally managed headless browser daemon child process.
static MANAGED_BROWSER_PROCESS: LazyLock<Mutex<Option<Child>>> = LazyLock::new(|| Mutex::new(None));

/// Searches the system for installed Chrome, Chromium, or Edge browser executables.
pub fn locate_system_browser() -> Option<PathBuf> {
    // 1. Platform-specific known installation paths
    #[cfg(target_os = "windows")]
    {
        let mut candidates = vec![
            PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
            PathBuf::from(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"),
        ];
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(&local_app_data)
                    .join("Google")
                    .join("Chrome")
                    .join("Application")
                    .join("chrome.exe"),
            );
            candidates.push(
                PathBuf::from(&local_app_data)
                    .join("Microsoft")
                    .join("Edge")
                    .join("Application")
                    .join("msedge.exe"),
            );
        }
        for path in candidates {
            if path.is_file() {
                return Some(path);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let candidates = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ];
        for path_str in candidates {
            let p = PathBuf::from(path_str);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let candidates = [
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/microsoft-edge",
            "/usr/bin/microsoft-edge-stable",
            "/usr/bin/brave-browser",
        ];
        for path_str in candidates {
            let p = PathBuf::from(path_str);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    // 2. Search PATH environment variable
    let bin_names = if cfg!(windows) {
        vec!["chrome.exe", "msedge.exe", "chromium.exe"]
    } else {
        vec![
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "msedge",
        ]
    };

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for bin in &bin_names {
                let candidate = dir.join(bin);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

/// Ensures the target CDP endpoint is reachable. If it points to local loopback (127.0.0.1/localhost)
/// and is not currently listening, automatically launches a managed headless Chrome/Edge daemon.
pub async fn ensure_cdp_daemon_ready(endpoint: &str) -> Result<String, TklessError> {
    let (base_endpoint, is_direct_ws) = normalize_cdp_endpoint(endpoint);
    if is_direct_ws {
        return Ok(base_endpoint);
    }

    // Check if endpoint is local loopback
    let is_local = base_endpoint.contains("127.0.0.1") || base_endpoint.contains("localhost");
    if !is_local {
        return Ok(base_endpoint);
    }

    let port: u16 = if let Ok(parsed) = Url::parse(&base_endpoint) {
        parsed.port().unwrap_or(9222)
    } else {
        9222
    };

    let probe_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(500))
        .timeout(Duration::from_millis(500))
        .build()
        .map_err(|e| TklessError::CdpError(e.to_string()))?;

    let version_url = format!("http://127.0.0.1:{port}/json/version");

    // 1. Probe if existing CDP endpoint is already alive
    if let Ok(resp) = probe_client.get(&version_url).send().await {
        if resp.status().is_success() {
            debug!(port = port, "CDP 远程调试端点已就绪 (现有服务复用)");
            return Ok(base_endpoint);
        }
    }

    // 2. Launch auto-managed browser daemon
    let browser_bin = locate_system_browser().ok_or_else(|| {
        TklessError::CdpError(
            "未检测到本地安装的 Chrome、Edge 或 Chromium 浏览器，无法自动拉起 CDP 无头守护进程。请安装 Chrome/Edge，或手动指定 `--cdp` 远程端点。".to_string(),
        )
    })?;

    info!(
        browser = %browser_bin.display(),
        port = port,
        "正在自动启动系统内置无头浏览器 CDP 守护进程..."
    );

    let profile_dir = std::env::temp_dir().join(format!("tkless_cdp_profile_{port}"));
    let _ = std::fs::create_dir_all(&profile_dir);

    let mut cmd = Command::new(&browser_bin);
    cmd.arg("--headless=new")
        .arg(format!("--remote-debugging-port={port}"))
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = cmd.spawn().map_err(|e| {
        TklessError::CdpError(format!(
            "无法启动无头浏览器进程 ({}): {e}",
            browser_bin.display()
        ))
    })?;

    if let Ok(mut lock) = MANAGED_BROWSER_PROCESS.lock() {
        *lock = Some(child);
    }

    // 3. Poll until port is alive (up to 8 seconds)
    let start_wait = std::time::Instant::now();
    let max_wait = Duration::from_secs(8);

    while start_wait.elapsed() < max_wait {
        tokio::time::sleep(Duration::from_millis(150)).await;
        if let Ok(resp) = probe_client.get(&version_url).send().await {
            if resp.status().is_success() {
                info!(port = port, "无头浏览器 CDP 守护进程已成功启动并就绪");
                return Ok(base_endpoint);
            }
        }
    }

    Err(TklessError::CdpError(format!(
        "无头浏览器 CDP 守护进程启动超时 (端口 {port} 未在 6 秒内响应)"
    )))
}

/// Options controlling CDP browser interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpOptions {
    #[serde(default)]
    pub steps: Vec<BrowserStep>,
    #[serde(default)]
    pub navigation_origin: Option<String>,
    #[serde(default)]
    pub read_only_requests: bool,
    /// Permit private network requests from the browser. Disabled by default.
    pub allow_private_addresses: bool,
    /// DevTools HTTP or WebSocket endpoint, e.g. "http://127.0.0.1:9222" or "ws://127.0.0.1:9222/devtools/..."
    pub endpoint: String,
    /// Optional additional wait time in milliseconds for dynamic JS rendering/hydration.
    pub wait_ms: Option<u64>,
    /// Overall page load and extraction timeout in milliseconds (default: 30000).
    pub timeout_ms: Option<u64>,
    /// Custom User-Agent to emulate in the browser session.
    pub user_agent: Option<String>,
    /// Explicit Accept-Language header; native browser defaults are retained when omitted.
    pub accept_language: Option<String>,
    /// Cookie header string or platform cookies to inject into the browser target.
    pub cookie: Option<String>,
    /// Whether to block heavy resources (images, media, fonts, tracker scripts) to boost speed by 3-5x. Default: true for HTML extraction.
    pub block_resources: Option<bool>,
    /// Whether to automatically scroll down the page to trigger lazy loading and dynamic DOM hydration. Default: false.
    pub auto_scroll: Option<bool>,
    /// Maximum scroll increments for auto-scroll. Default: 5.
    pub scroll_count: Option<u32>,
    /// Include open shadow roots in the extracted snapshot. Default: false.
    #[serde(default)]
    pub include_shadow_dom: bool,
    /// Inline same-origin iframe content in the extracted snapshot. Default: false.
    #[serde(default)]
    pub include_frames: bool,
    /// Opt-in capture of XHR/fetch JSON responses. Default: disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<NetworkCapture>,
}

impl Default for CdpOptions {
    fn default() -> Self {
        Self {
            steps: vec![],
            navigation_origin: None,
            read_only_requests: false,
            allow_private_addresses: false,
            endpoint: "http://127.0.0.1:9222".to_string(),
            wait_ms: Some(1000),
            timeout_ms: Some(30_000),
            user_agent: None,
            accept_language: None,
            cookie: None,
            block_resources: Some(true),
            auto_scroll: Some(false),
            scroll_count: Some(5),
            include_shadow_dom: false,
            include_frames: false,
            capture: None,
        }
    }
}

/// Opt-in, bounded capture of XHR/fetch JSON responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkCapture {
    /// MIME prefixes to keep, e.g. `["application/json"]`.
    pub mime_prefixes: Vec<String>,
    pub max_responses: usize,
    pub max_body_bytes: usize,
    /// Restrict capture to the navigation origin. Default: true.
    pub same_origin_only: bool,
}

impl Default for NetworkCapture {
    fn default() -> Self {
        Self {
            mime_prefixes: vec!["application/json".into()],
            max_responses: crate::limits::BROWSER_CAPTURE_DEFAULT_RESPONSES,
            max_body_bytes: crate::limits::BROWSER_CAPTURE_DEFAULT_BODY_BYTES,
            same_origin_only: true,
        }
    }
}

impl NetworkCapture {
    pub fn validate(&self) -> Result<(), TklessError> {
        if self.mime_prefixes.is_empty()
            || self.mime_prefixes.len() > crate::limits::BROWSER_CAPTURE_MAX_MIME_PREFIXES
            || self
                .mime_prefixes
                .iter()
                .any(|prefix| prefix.trim().is_empty() || prefix.len() > 64)
        {
            return Err(TklessError::InvalidArgument(
                "capture.mime_prefixes supports 1..8 non-empty entries of at most 64 bytes".into(),
            ));
        }
        if !(1..=crate::limits::BROWSER_CAPTURE_MAX_RESPONSES).contains(&self.max_responses) {
            return Err(TklessError::InvalidArgument(format!(
                "capture.max_responses must be 1..{}",
                crate::limits::BROWSER_CAPTURE_MAX_RESPONSES
            )));
        }
        if !(crate::limits::BROWSER_CAPTURE_MIN_BODY_BYTES
            ..=crate::limits::BROWSER_CAPTURE_MAX_BODY_BYTES)
            .contains(&self.max_body_bytes)
        {
            return Err(TklessError::InvalidArgument(format!(
                "capture.max_body_bytes must be {}..={}",
                crate::limits::BROWSER_CAPTURE_MIN_BODY_BYTES,
                crate::limits::BROWSER_CAPTURE_MAX_BODY_BYTES
            )));
        }
        Ok(())
    }

    fn matches_mime(&self, mime_type: &str) -> bool {
        let mime_type = mime_type.to_ascii_lowercase();
        self.mime_prefixes
            .iter()
            .any(|prefix| mime_type.starts_with(&prefix.trim().to_ascii_lowercase()))
    }
}

/// A single XHR/fetch response captured during a browser session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedResponse {
    pub url: String,
    pub status: u16,
    pub mime_type: String,
    pub body: String,
    pub truncated: bool,
}

/// Target description returned by Chrome DevTools HTTP `/json/new` or `/json/list`.
#[derive(Debug, Clone, Deserialize)]
struct DevToolsTarget {
    id: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: Option<String>,
}

/// Normalizes user-specified endpoint string to HTTP and WebSocket base URLs.
fn normalize_cdp_endpoint(endpoint: &str) -> (String, bool) {
    let trimmed = endpoint.trim();
    if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        (trimmed.to_string(), true)
    } else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        (trimmed.trim_end_matches('/').to_string(), false)
    } else if let Ok(port) = trimmed.parse::<u16>() {
        (format!("http://127.0.0.1:{port}"), false)
    } else {
        (format!("http://{}", trimmed.trim_end_matches('/')), false)
    }
}

/// Fetches rendered HTML for a given URL via Chrome DevTools Protocol.
pub async fn fetch_html_via_cdp(url: &str, options: &CdpOptions) -> Result<String, TklessError> {
    Ok(fetch_rendered_page(url, options).await?.html)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedPage {
    pub html: String,
    pub final_url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<CapturedResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_stopped_reason: Option<String>,
}

pub async fn fetch_rendered_page(
    url: &str,
    options: &CdpOptions,
) -> Result<RenderedPage, TklessError> {
    steps::validate(&options.steps)?;
    let (raw_base_endpoint, is_direct_ws) = normalize_cdp_endpoint(&options.endpoint);
    let base_endpoint = if !is_direct_ws {
        ensure_cdp_daemon_ready(&raw_base_endpoint).await?
    } else {
        raw_base_endpoint
    };
    let total_timeout = Duration::from_millis(options.timeout_ms.unwrap_or(30_000));

    let started = std::time::Instant::now();
    timeout(total_timeout + Duration::from_secs(3), async {
        if is_direct_ws {
            connect_and_extract_html(&base_endpoint, url, options).await
        } else {
            // 1. Create a new target tab via Chrome DevTools HTTP API
            let req_timeout = Duration::from_millis(options.timeout_ms.unwrap_or(30_000).min(2000));
            let client = reqwest::Client::builder()
                .connect_timeout(req_timeout)
                .timeout(req_timeout)
                .build()
                .map_err(|e| TklessError::CdpError(format!("创建 HTTP 客户端失败: {e}")))?;

            let new_target_url = format!("{}/json/new?{}", base_endpoint, urlencoding("about:blank"));
            debug!(endpoint = %new_target_url, "通过 CDP HTTP 接口创建新页面 Tab");

            let resp = match client.put(&new_target_url).send().await {
                Ok(r) if r.status().is_success() => Ok(r),
                _ => client.get(&new_target_url).send().await,
            }
            .map_err(|err| {
                TklessError::CdpError(format!(
                    "无法连接到 Chrome CDP 端点 '{base_endpoint}': {err}。请确保已通过 `google-chrome --remote-debugging-port=9222` 或 `msedge --remote-debugging-port=9222` 启动浏览器远程调试。"
                ))
            })?;

            if !resp.status().is_success() {
                return Err(TklessError::CdpError(format!(
                    "CDP 端点返回错误状态码 {}: {}",
                    resp.status(),
                    base_endpoint
                )));
            }

            let target: DevToolsTarget = resp.json().await.map_err(|err| {
                TklessError::CdpError(format!("解析 CDP 目标元数据失败: {err}"))
            })?;

            let ws_url = target.web_socket_debugger_url.ok_or_else(|| {
                TklessError::CdpError("CDP 目标未返回有效的 webSocketDebuggerUrl".to_string())
            })?;

            let target_id = target.id.clone();
            // Keep cleanup outside the extraction timeout so challenge failures do not leak tabs.
            let remaining = total_timeout.saturating_sub(started.elapsed()).saturating_sub(Duration::from_millis(250));
            let result = timeout(remaining, connect_and_extract_html(&ws_url, url, options)).await
                .unwrap_or_else(|_| Err(TklessError::ConnectionTimeout("CDP page extraction timed out".into())));

            // Close tab after extraction to prevent tab leak
            let close_url = format!("{}/json/close/{}", base_endpoint, target_id);
            let _ = client.get(&close_url).send().await;

            result
        }
    })
    .await
    .map_err(|_| {
        TklessError::ConnectionTimeout(format!(
            "CDP 页面加载超时 ({}ms): {url}",
            options.timeout_ms.unwrap_or(30_000)
        ))
    })?
}

/// Connects to a CDP WebSocket URL, navigates, waits for DOM ready, and evaluates `outerHTML`.
async fn connect_and_extract_html(
    ws_url: &str,
    target_url: &str,
    options: &CdpOptions,
) -> Result<RenderedPage, TklessError> {
    debug!(ws_url = %ws_url, "连接 CDP WebSocket 调试会话");

    let (mut ws_stream, _) = connect_async(ws_url).await.map_err(|err| {
        TklessError::CdpError(format!("无法建立 CDP WebSocket 连接 ({ws_url}): {err}"))
    })?;

    let mut guard = BrowserRequestGuard::enable(&mut ws_stream, target_url, options).await?;

    let mut message_id = 1u64;

    // 1. Enable Page, Runtime, and Network domains
    send_cdp_cmd(&mut ws_stream, message_id, "Page.enable", json!({})).await?;
    message_id += 1;

    send_cdp_cmd(&mut ws_stream, message_id, "Runtime.enable", json!({})).await?;
    message_id += 1;

    send_cdp_cmd(&mut ws_stream, message_id, "Network.enable", json!({})).await?;
    message_id += 1;

    // Block heavy static assets (images, fonts, media, analytics) if enabled (default true for HTML)
    if options.block_resources.unwrap_or(true) {
        debug!("CDP 启用网络层静态重资产与跟踪器拦截 (提速 3-5x)");
        send_cdp_cmd(
            &mut ws_stream,
            message_id,
            "Network.setBlockedURLs",
            json!({
                "urls": [
                    "*.png", "*.jpg", "*.jpeg", "*.gif", "*.webp", "*.svg", "*.ico",
                    "*.woff", "*.woff2", "*.ttf", "*.eot", "*.otf",
                    "*.mp4", "*.mp3", "*.avi", "*.webm", "*.flv", "*.mov", "*.m3u8",
                    "*google-analytics.com*", "*googletagmanager.com*", "*doubleclick.net*",
                    "*hm.baidu.com*", "*cnzz.com*", "*clarity.ms*", "*facebook.net*",
                    "*beacon*", "*telemetry*"
                ]
            }),
        )
        .await?;
        message_id += 1;
    }

    // 2. Set custom User-Agent if provided
    if let Some(ref ua) = options.user_agent {
        send_cdp_cmd(
            &mut ws_stream,
            message_id,
            "Network.setUserAgentOverride",
            json!({ "userAgent": ua }),
        )
        .await?;
        message_id += 1;
    }

    if let Some(language) = &options.accept_language {
        send_cdp_cmd(
            &mut ws_stream,
            message_id,
            "Network.setExtraHTTPHeaders",
            json!({"headers": {"Accept-Language": language}}),
        )
        .await?;
        message_id += 1;
    }

    // 3. Set Cookie if provided
    if let Some(ref raw_cookie) = options.cookie {
        if let Ok(parsed_url) = Url::parse(target_url) {
            let host = parsed_url.host_str().unwrap_or_default();
            for item in raw_cookie.split(';') {
                let trimmed = item.trim();
                if let Some((k, v)) = trimmed.split_once('=') {
                    send_cdp_cmd(
                        &mut ws_stream,
                        message_id,
                        "Network.setCookie",
                        json!({
                            "name": k.trim(),
                            "value": v.trim(),
                            "domain": host,
                            "path": "/",
                        }),
                    )
                    .await?;
                    message_id += 1;
                }
            }
        }
    }

    // 4. Navigate to URL
    send_cdp_cmd(
        &mut ws_stream,
        message_id,
        "Page.navigate",
        json!({ "url": target_url }),
    )
    .await?;
    let navigate_cmd_id = message_id;
    message_id += 1;

    // 5. Wait for page load event or navigation response
    let nav_timeout = Duration::from_secs(15);
    let _ = timeout(nav_timeout, async {
        while let Some(msg_result) = guard.next(&mut ws_stream).await {
            if let Ok(Message::Text(text)) = msg_result {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
                        if method == "Page.loadEventFired" || method == "Page.domContentEventFired"
                        {
                            debug!("CDP 捕获到页面加载完成事件: {method}");
                            break;
                        }
                    }
                    if let Some(id) = v.get("id").and_then(|id| id.as_u64()) {
                        if id == navigate_cmd_id {
                            debug!("CDP Page.navigate 指令已接收响应");
                        }
                    }
                }
            }
        }
    })
    .await;

    // 6. Optional additional hydration wait time
    if let Some(wait_ms) = options.wait_ms {
        if wait_ms > 0 {
            let _ = timeout(Duration::from_millis(wait_ms), async {
                while guard.next(&mut ws_stream).await.is_some() {}
            })
            .await;
        }
    }

    // 6.5 Auto-scroll for lazy-loading / infinite scroll pages
    if options.auto_scroll.unwrap_or(false) {
        let max_scrolls = options.scroll_count.unwrap_or(5);
        debug!(
            max_scrolls = max_scrolls,
            "CDP 执行平滑自动下滚以触发延迟渲染"
        );
        let scroll_js = format!(
            r#"
            (function() {{
                return new Promise((resolve) => {{
                    let totalHeight = 0;
                    let distance = 400;
                    let count = 0;
                    let maxCount = {};
                    let timer = setInterval(() => {{
                        window.scrollBy(0, distance);
                        totalHeight += distance;
                        count++;
                        if (count >= maxCount || (window.innerHeight + window.scrollY) >= (document.body ? document.body.scrollHeight : 0)) {{
                            clearInterval(timer);
                            window.scrollTo(0, 0);
                            resolve(true);
                        }}
                    }}, 120);
                }});
            }})()
            "#,
            max_scrolls
        );

        let scroll_cmd_id = message_id;
        message_id += 1;
        send_cdp_cmd(
            &mut ws_stream,
            scroll_cmd_id,
            "Runtime.evaluate",
            json!({
                "expression": scroll_js,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
        .await?;

        let scroll_timeout = Duration::from_millis(max_scrolls as u64 * 180 + 1500);
        let _ = timeout(scroll_timeout, async {
            while let Some(msg_result) = guard.next(&mut ws_stream).await {
                if let Ok(Message::Text(text)) = msg_result {
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        if v.get("id").and_then(|id| id.as_u64()) == Some(scroll_cmd_id) {
                            debug!("CDP 自动滚动完成");
                            break;
                        }
                    }
                }
            }
        })
        .await;
    }

    let scroll_reason =
        steps::execute(&mut ws_stream, &mut guard, &mut message_id, &options.steps).await?;
    // 7. Capture content and its actual browser URL in the same evaluation.
    let expression = snapshot_expression(options.include_shadow_dom, options.include_frames);
    let eval_cmd_id = message_id;
    send_cdp_cmd(
        &mut ws_stream,
        eval_cmd_id,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
        }),
    )
    .await?;

    let eval_timeout = Duration::from_secs(10);
    let html = timeout(eval_timeout, async {
        while let Some(msg_result) = guard.next(&mut ws_stream).await {
            if let Ok(Message::Text(text)) = msg_result {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if v.get("id").and_then(|id| id.as_u64()) == Some(eval_cmd_id) {
                        if let Some(res_val) = v
                            .get("result")
                            .and_then(|r| r.get("result"))
                            .and_then(|r| r.get("value"))
                        {
                            return serde_json::from_value::<RenderedPage>(res_val.clone())
                                .map_err(|e| TklessError::CdpError(e.to_string()));
                        }
                    }
                }
            }
        }
        Err(TklessError::CdpError(
            "未能从 CDP Runtime.evaluate 获取 outerHTML".to_string(),
        ))
    })
    .await
    .map_err(|_| TklessError::ConnectionTimeout("CDP Runtime.evaluate 执行超时".to_string()))??;

    if let Some(url) = guard.blocked_navigation {
        return Err(TklessError::CdpError(format!(
            "Blocked browser navigation: {url}"
        )));
    }
    if html.html.len() > crate::limits::BROWSER_MAX_SNAPSHOT_BYTES {
        return Err(TklessError::ResponseTooLarge {
            limit: crate::limits::BROWSER_MAX_SNAPSHOT_BYTES,
            received: html.html.len(),
        });
    }
    let mut page = html;
    if options.capture.is_some() {
        let _ = timeout(Duration::from_millis(800), async {
            while guard.capture_in_flight() {
                if guard.next(&mut ws_stream).await.is_none() {
                    break;
                }
            }
        })
        .await;
    }
    page.network = guard.take_captures();
    page.scroll_stopped_reason = scroll_reason;
    Ok(page)
}

/// Builds the fixed snapshot expression. Shadow roots and iframes are opt-in
/// because inlining them changes the serialized DOM.
fn snapshot_expression(include_shadow_dom: bool, include_frames: bool) -> String {
    if !include_shadow_dom && !include_frames {
        return "({html: document.documentElement.outerHTML, final_url: location.href})".into();
    }
    format!(
        r#"(() => {{
            const includeShadow = {include_shadow_dom};
            const includeFrames = {include_frames};
            const VOID = new Set(['area','base','br','col','embed','hr','img','input','link','meta','param','source','track','wbr']);
            const escText = (s) => s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
            const escAttr = (s) => s.replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
            const out = [];
            const walk = (node) => {{
                if (node.nodeType === 3) {{ out.push(escText(node.nodeValue || '')); return; }}
                if (node.nodeType !== 1) {{ return; }}
                const tag = node.tagName.toLowerCase();
                if (includeFrames && tag === 'iframe') {{
                    let inlined = false;
                    try {{
                        const frame = node.contentDocument;
                        if (frame && frame.documentElement) {{
                            out.push('<div data-frame-url="' + escAttr(node.getAttribute('src') || '') + '">');
                            walk(frame.documentElement);
                            out.push('</div>');
                            inlined = true;
                        }}
                    }} catch (error) {{}}
                    if (inlined) {{ return; }}
                }}
                out.push('<' + tag);
                for (const attribute of node.attributes) {{
                    out.push(' ' + attribute.name + '="' + escAttr(attribute.value) + '"');
                }}
                out.push('>');
                if (VOID.has(tag)) {{ return; }}
                if (tag === 'script' || tag === 'style') {{
                    out.push(node.textContent || '');
                }} else if (includeShadow && node.shadowRoot) {{
                    for (const child of node.shadowRoot.childNodes) {{ walk(child); }}
                }} else {{
                    for (const child of node.childNodes) {{ walk(child); }}
                }}
                out.push('</' + tag + '>');
            }};
            walk(document.documentElement);
            return {{html: out.join(''), final_url: location.href}};
        }})()"#
    )
}

type BrowserSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct BrowserRequestGuard {
    navigation_origin: Option<String>,
    read_only_requests: bool,
    blocked_navigation: Option<String>,
    fetcher: crate::fetcher::Fetcher,
    next_id: u64,
    pending: Option<Value>,
    capture: Option<CaptureState>,
}

struct PendingCapture {
    url: String,
    status: u16,
    mime_type: String,
}

/// Bounded, opt-in accumulator for XHR/fetch responses.
#[derive(Default)]
struct CaptureState {
    policy: NetworkCapture,
    origin: Option<String>,
    sensitive: HashSet<String>,
    pending: HashMap<String, PendingCapture>,
    bodies: HashMap<u64, String>,
    responses: Vec<CapturedResponse>,
}

impl CaptureState {
    fn new(policy: NetworkCapture, origin: Option<String>) -> Self {
        Self {
            policy,
            origin,
            ..Default::default()
        }
    }

    fn observe_request(&mut self, params: &Value) {
        let Some(request_id) = params["requestId"].as_str() else {
            return;
        };
        let sensitive = params["request"]["headers"]
            .as_object()
            .is_some_and(|headers| {
                headers
                    .keys()
                    .any(|name| crate::fetcher::is_sensitive_header(name))
            });
        if sensitive {
            self.sensitive.insert(request_id.to_string());
        }
    }

    fn observe_response(&mut self, params: &Value) {
        let Some(request_id) = params["requestId"].as_str() else {
            return;
        };
        if !matches!(params["type"].as_str(), Some("XHR" | "Fetch")) {
            return;
        }
        let url = params["response"]["url"].as_str().unwrap_or_default();
        let mime_type = params["response"]["mimeType"].as_str().unwrap_or_default();
        if !self.policy.matches_mime(mime_type) || self.sensitive.contains(request_id) {
            return;
        }
        if self.policy.same_origin_only {
            let Some(origin) = &self.origin else {
                return;
            };
            match Url::parse(url) {
                Ok(parsed) if &parsed.origin().ascii_serialization() == origin => {}
                _ => return,
            }
        }
        self.pending.insert(
            request_id.to_string(),
            PendingCapture {
                url: url.to_string(),
                status: params["response"]["status"].as_u64().unwrap_or(0) as u16,
                mime_type: mime_type.to_string(),
            },
        );
    }

    fn should_fetch(&self, request_id: &str) -> bool {
        self.pending.contains_key(request_id)
            && !self.sensitive.contains(request_id)
            && self.responses.len() + self.bodies.len() < self.policy.max_responses
    }

    fn drop_request(&mut self, request_id: &str) {
        self.pending.remove(request_id);
    }

    fn complete(&mut self, request_id: &str, value: &Value) {
        let Some(meta) = self.pending.remove(request_id) else {
            return;
        };
        if value.get("error").is_some()
            || value
                .pointer("/result/base64Encoded")
                .and_then(Value::as_bool)
                == Some(true)
        {
            return;
        }
        let Some(body) = value.pointer("/result/body").and_then(Value::as_str) else {
            return;
        };
        let (body, truncated) = truncate_utf8(body, self.policy.max_body_bytes);
        self.responses.push(CapturedResponse {
            url: meta.url,
            status: meta.status,
            mime_type: meta.mime_type,
            body,
            truncated,
        });
    }
}

fn truncate_utf8(value: &str, limit: usize) -> (String, bool) {
    if value.len() <= limit {
        return (value.to_string(), false);
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}

impl BrowserRequestGuard {
    async fn enable(
        ws: &mut BrowserSocket,
        target_url: &str,
        options: &CdpOptions,
    ) -> Result<Self, TklessError> {
        let fetcher = crate::fetcher::Fetcher::new(crate::FetchOptions {
            allow_private_addresses: options.allow_private_addresses,
            ..Default::default()
        })?;
        fetcher.validate_ssrf(&Url::parse(target_url)?).await?;
        // Intercept before navigation, including redirects and subresource requests.
        const ENABLE_ID: u64 = 1_000_000;
        send_cdp_cmd(
            ws,
            ENABLE_ID,
            "Network.setBypassServiceWorker",
            json!({"bypass":true}),
        )
        .await?;
        send_cdp_cmd(
            ws,
            ENABLE_ID + 1,
            "Fetch.enable",
            json!({"patterns":[{"urlPattern":"*","requestStage":"Request"}]}),
        )
        .await?;
        timeout(Duration::from_secs(5), async {
            while let Some(message) = ws.next().await {
                let message = message.map_err(|e| TklessError::CdpError(e.to_string()))?;
                if let Message::Text(text) = message {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        if value.get("id").and_then(Value::as_u64) == Some(ENABLE_ID + 1) {
                            if value.get("error").is_some() {
                                return Err(TklessError::CdpError(
                                    "Browser request interception could not be enabled".into(),
                                ));
                            }
                            return Ok(());
                        }
                    }
                }
            }
            Err(TklessError::CdpError(
                "Browser disconnected while enabling request interception".into(),
            ))
        })
        .await
        .map_err(|_| TklessError::CdpError("Browser request interception timed out".into()))??;
        Ok(Self {
            navigation_origin: options.navigation_origin.clone(),
            read_only_requests: options.read_only_requests,
            blocked_navigation: None,
            fetcher,
            next_id: ENABLE_ID + 2,
            pending: None,
            capture: options.capture.as_ref().map(|policy| {
                CaptureState::new(
                    policy.clone(),
                    Url::parse(target_url)
                        .ok()
                        .map(|url| url.origin().ascii_serialization()),
                )
            }),
        })
    }

    fn capture_in_flight(&self) -> bool {
        self.capture
            .as_ref()
            .is_some_and(|capture| !capture.bodies.is_empty())
    }

    fn take_captures(&mut self) -> Vec<CapturedResponse> {
        self.capture
            .as_mut()
            .map(|capture| std::mem::take(&mut capture.responses))
            .unwrap_or_default()
    }

    async fn next(
        &mut self,
        ws: &mut BrowserSocket,
    ) -> Option<Result<Message, tokio_tungstenite::tungstenite::Error>> {
        loop {
            let message = if let Some(value) = &self.pending {
                Ok(Message::Text(value.to_string().into()))
            } else {
                ws.next().await?
            };
            if let Ok(Message::Text(text)) = &message {
                if let Ok(value) = serde_json::from_str::<Value>(text) {
                    if let Some(id) = value.get("id").and_then(Value::as_u64) {
                        if let Some(capture) = &mut self.capture {
                            if let Some(request_id) = capture.bodies.remove(&id) {
                                capture.complete(&request_id, &value);
                                continue;
                            }
                        }
                    }
                    match value.get("method").and_then(Value::as_str) {
                        Some("Fetch.requestPaused") => {
                            self.pending = Some(value.clone());
                            let params = &value["params"];
                            let mut allowed = match params["request"]["url"]
                                .as_str()
                                .and_then(|s| Url::parse(s).ok())
                            {
                                Some(url) => matches!(
                                    timeout(
                                        Duration::from_secs(5),
                                        self.fetcher.validate_ssrf(&url)
                                    )
                                    .await,
                                    Ok(Ok(()))
                                ),
                                None => false,
                            };
                            if self.read_only_requests
                                && !matches!(
                                    params["request"]["method"].as_str(),
                                    Some("GET" | "HEAD" | "OPTIONS")
                                )
                            {
                                allowed = false;
                            }
                            if params["resourceType"] == "Document" {
                                if let Some(origin) = &self.navigation_origin {
                                    if params["request"]["url"]
                                        .as_str()
                                        .and_then(|s| Url::parse(s).ok())
                                        .is_none_or(|u| u.origin().ascii_serialization() != *origin)
                                    {
                                        allowed = false;
                                    }
                                }
                                if !allowed {
                                    self.blocked_navigation =
                                        params["request"]["url"].as_str().map(str::to_owned);
                                }
                            }
                            let (method, args) = if allowed {
                                (
                                    "Fetch.continueRequest",
                                    json!({"requestId":params["requestId"]}),
                                )
                            } else {
                                (
                                    "Fetch.failRequest",
                                    json!({"requestId":params["requestId"],"errorReason":"BlockedByClient"}),
                                )
                            };
                            self.next_id += 1;
                            if let Err(e) = send_cdp_cmd(ws, self.next_id, method, args).await {
                                return Some(Err(tokio_tungstenite::tungstenite::Error::Io(
                                    std::io::Error::other(e.to_string()),
                                )));
                            }
                            self.pending = None;
                            continue;
                        }
                        Some("Network.requestWillBeSent") => {
                            if let Some(capture) = &mut self.capture {
                                capture.observe_request(&value["params"]);
                            }
                        }
                        Some("Network.responseReceived") => {
                            if let Some(capture) = &mut self.capture {
                                capture.observe_response(&value["params"]);
                            }
                        }
                        Some("Network.loadingFinished") => {
                            let request_id =
                                value["params"]["requestId"].as_str().map(str::to_owned);
                            let should_fetch = match (&self.capture, &request_id) {
                                (Some(capture), Some(request_id)) => {
                                    capture.should_fetch(request_id)
                                }
                                _ => false,
                            };
                            if let (Some(request_id), true) = (request_id, should_fetch) {
                                self.next_id += 1;
                                let command_id = self.next_id;
                                if send_cdp_cmd(
                                    ws,
                                    command_id,
                                    "Network.getResponseBody",
                                    json!({"requestId": request_id}),
                                )
                                .await
                                .is_err()
                                {
                                    if let Some(capture) = &mut self.capture {
                                        capture.drop_request(&request_id);
                                    }
                                } else if let Some(capture) = &mut self.capture {
                                    capture.bodies.insert(command_id, request_id);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            return Some(message);
        }
    }
}

/// Helper function to send a JSON-RPC command over a WebSocket stream.
async fn send_cdp_cmd<S: SinkExt<Message> + Unpin>(
    ws: &mut S,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), TklessError>
where
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    let payload = json!({
        "id": id,
        "method": method,
        "params": params,
    });
    let text = payload.to_string();
    ws.send(Message::Text(text.into()))
        .await
        .map_err(|err| TklessError::CdpError(format!("发送 CDP 指令失败 ({method}): {err}")))?;
    Ok(())
}

/// Captures a rendered visual screenshot of a given URL via Chrome DevTools Protocol.
/// Returns raw image bytes (PNG or JPEG).
pub async fn capture_screenshot_via_cdp(
    url: &str,
    options: &CdpOptions,
    format: Option<&str>,
) -> Result<Vec<u8>, TklessError> {
    let (raw_base_endpoint, is_direct_ws) = normalize_cdp_endpoint(&options.endpoint);
    let base_endpoint = if !is_direct_ws {
        ensure_cdp_daemon_ready(&raw_base_endpoint).await?
    } else {
        raw_base_endpoint
    };
    let total_timeout = Duration::from_millis(options.timeout_ms.unwrap_or(30_000));

    timeout(total_timeout, async {
        if is_direct_ws {
            connect_and_capture_screenshot(&base_endpoint, url, options, format).await
        } else {
            let req_timeout = Duration::from_millis(options.timeout_ms.unwrap_or(30_000).min(2000));
            let client = reqwest::Client::builder()
                .connect_timeout(req_timeout)
                .timeout(req_timeout)
                .build()
                .map_err(|e| TklessError::CdpError(format!("创建 HTTP 客户端失败: {e}")))?;

            let new_target_url = format!("{}/json/new?{}", base_endpoint, urlencoding("about:blank"));
            debug!(endpoint = %new_target_url, "通过 CDP HTTP 接口创建新页面 Tab (截图)");

            let resp = match client.put(&new_target_url).send().await {
                Ok(r) if r.status().is_success() => Ok(r),
                _ => client.get(&new_target_url).send().await,
            }
            .map_err(|err| {
                TklessError::CdpError(format!(
                    "无法连接到 Chrome CDP 端点 '{base_endpoint}': {err}。请确保已通过 `google-chrome --remote-debugging-port=9222` 或 `msedge --remote-debugging-port=9222` 启动浏览器远程调试。"
                ))
            })?;

            if !resp.status().is_success() {
                return Err(TklessError::CdpError(format!(
                    "CDP 端点返回错误状态码 {}: {}",
                    resp.status(),
                    base_endpoint
                )));
            }

            let target: DevToolsTarget = resp.json().await.map_err(|err| {
                TklessError::CdpError(format!("解析 CDP 目标元数据失败: {err}"))
            })?;

            let ws_url = target.web_socket_debugger_url.ok_or_else(|| {
                TklessError::CdpError("CDP 目标未返回有效的 webSocketDebuggerUrl".to_string())
            })?;

            let target_id = target.id.clone();
            let result = connect_and_capture_screenshot(&ws_url, url, options, format).await;

            let close_url = format!("{}/json/close/{}", base_endpoint, target_id);
            let _ = client.get(&close_url).send().await;

            result
        }
    })
    .await
    .map_err(|_| {
        TklessError::ConnectionTimeout(format!(
            "CDP 页面加载与截图超时 ({}ms): {url}",
            options.timeout_ms.unwrap_or(30_000)
        ))
    })?
}

/// Connects to a CDP WebSocket URL, navigates, waits for DOM ready and hydration, and captures screenshot.
async fn connect_and_capture_screenshot(
    ws_url: &str,
    target_url: &str,
    options: &CdpOptions,
    format: Option<&str>,
) -> Result<Vec<u8>, TklessError> {
    debug!(ws_url = %ws_url, "连接 CDP WebSocket 截取屏幕快照");

    let (mut ws_stream, _) = connect_async(ws_url).await.map_err(|err| {
        TklessError::CdpError(format!("无法建立 CDP WebSocket 连接 ({ws_url}): {err}"))
    })?;

    let mut guard = BrowserRequestGuard::enable(&mut ws_stream, target_url, options).await?;

    let mut message_id = 1u64;

    send_cdp_cmd(&mut ws_stream, message_id, "Page.enable", json!({})).await?;
    message_id += 1;

    if let Some(ref ua) = options.user_agent {
        send_cdp_cmd(
            &mut ws_stream,
            message_id,
            "Network.setUserAgentOverride",
            json!({ "userAgent": ua }),
        )
        .await?;
        message_id += 1;
    }

    let navigate_cmd_id = message_id;
    send_cdp_cmd(
        &mut ws_stream,
        navigate_cmd_id,
        "Page.navigate",
        json!({ "url": target_url }),
    )
    .await?;
    message_id += 1;

    let nav_timeout = Duration::from_millis(options.timeout_ms.unwrap_or(30_000).min(15_000));
    let _ = timeout(nav_timeout, async {
        while let Some(msg_result) = guard.next(&mut ws_stream).await {
            if let Ok(Message::Text(text)) = msg_result {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
                        if method == "Page.loadEventFired" || method == "Page.domContentEventFired"
                        {
                            break;
                        }
                    }
                }
            }
        }
    })
    .await;

    if let Some(wait_ms) = options.wait_ms {
        if wait_ms > 0 {
            let _ = timeout(Duration::from_millis(wait_ms), async {
                while guard.next(&mut ws_stream).await.is_some() {}
            })
            .await;
        }
    }

    // Auto-scroll for lazy-loading before capturing screenshot
    if options.auto_scroll.unwrap_or(false) {
        let max_scrolls = options.scroll_count.unwrap_or(5);
        debug!(
            max_scrolls = max_scrolls,
            "CDP 截图前执行自动下滚以触发延迟渲染"
        );
        let scroll_js = format!(
            r#"
            (function() {{
                return new Promise((resolve) => {{
                    let totalHeight = 0;
                    let distance = 400;
                    let count = 0;
                    let maxCount = {};
                    let timer = setInterval(() => {{
                        window.scrollBy(0, distance);
                        totalHeight += distance;
                        count++;
                        if (count >= maxCount || (window.innerHeight + window.scrollY) >= (document.body ? document.body.scrollHeight : 0)) {{
                            clearInterval(timer);
                            window.scrollTo(0, 0);
                            resolve(true);
                        }}
                    }}, 120);
                }});
            }})()
            "#,
            max_scrolls
        );

        let scroll_cmd_id = message_id;
        message_id += 1;
        send_cdp_cmd(
            &mut ws_stream,
            scroll_cmd_id,
            "Runtime.evaluate",
            json!({
                "expression": scroll_js,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
        .await?;

        let scroll_timeout = Duration::from_millis(max_scrolls as u64 * 180 + 1500);
        let _ = timeout(scroll_timeout, async {
            while let Some(msg_result) = guard.next(&mut ws_stream).await {
                if let Ok(Message::Text(text)) = msg_result {
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        if v.get("id").and_then(|id| id.as_u64()) == Some(scroll_cmd_id) {
                            debug!("CDP 截图前自动滚动已完成");
                            break;
                        }
                    }
                }
            }
        })
        .await;
    }

    let img_format = match format {
        Some("jpeg") | Some("jpg") => "jpeg",
        _ => "png",
    };

    let screenshot_cmd_id = message_id;
    send_cdp_cmd(
        &mut ws_stream,
        screenshot_cmd_id,
        "Page.captureScreenshot",
        json!({
            "format": img_format,
            "quality": 85,
            "fromSurface": true,
            "captureBeyondViewport": true
        }),
    )
    .await?;

    let cap_timeout = Duration::from_secs(15);
    let b64_str = timeout(cap_timeout, async {
        while let Some(msg_result) = guard.next(&mut ws_stream).await {
            if let Ok(Message::Text(text)) = msg_result {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if v.get("id").and_then(|id| id.as_u64()) == Some(screenshot_cmd_id) {
                        if let Some(res_val) = v
                            .get("result")
                            .and_then(|r| r.get("data"))
                            .and_then(|val| val.as_str())
                        {
                            return Ok(res_val.to_string());
                        }
                    }
                }
            }
        }
        Err(TklessError::CdpError(
            "未能从 CDP Page.captureScreenshot 获取图像数据".to_string(),
        ))
    })
    .await
    .map_err(|_| {
        TklessError::ConnectionTimeout("CDP Page.captureScreenshot 执行超时".to_string())
    })??;

    decode_base64(&b64_str)
        .ok_or_else(|| TklessError::CdpError("解析 CDP 截图 Base64 数据失败".to_string()))
}

/// Decodes standard base64 string into raw binary bytes.
pub fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let clean = input.trim().trim_end_matches('=');
    let mut bits = 0u32;
    let mut bit_count = 0;
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);

    for b in clean.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        bits = (bits << 6) | (val as u32);
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push((bits >> bit_count) as u8);
        }
    }
    Some(out)
}

/// Encodes raw binary bytes to standard Base64 string.
pub fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }

        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }

    out
}

/// Detects whether an HTML response body or HTTP status indicates an active anti-scraping challenge
/// (e.g. Cloudflare Turnstile, 5-second shield, Zhihu captcha, WeChat verification, DDOS-GUARD, Baidu WAF, Sogou anti-spider).
pub fn is_anti_scraping_challenge(html: &str, status: u16) -> bool {
    if status == 403 || status == 503 || status == 429 {
        return true;
    }

    let lower = html.to_ascii_lowercase();

    // Cloudflare challenges
    if lower.contains("cf-browser-verification")
        || lower.contains("challenge-platform")
        || lower.contains("cf-turnstile")
        || lower.contains("_cf_chl_opt")
        || lower.contains("just a moment...")
    {
        return true;
    }

    // Baidu Baike / WAF / Paris anti-scraping challenge
    if lower.contains("__paris_challenge_start__")
        || lower.contains("__challenge_config__")
        || lower.contains("百度安全验证")
        || lower.contains("mkdjump")
        || lower.contains("dlswbr.baidu.com/heicha")
    {
        return true;
    }

    // Sogou anti-spider verification challenge
    if lower.contains("antispider.min.js")
        || lower.contains("static/js/antispider")
        || lower.contains("verify.css")
        || lower.contains("请输入验证码")
        || lower.contains("checksnuid")
    {
        return true;
    }

    // Bing captcha / bot challenge
    if lower.contains("最后一步 请解决以下难题以继续")
        || lower.contains("请解决以下难题以继续")
        || lower.contains("b_captcha")
        || lower.contains("please solve the challenge below")
    {
        return true;
    }

    // Google captcha / bot challenge
    if lower.contains("/httpservice/retry/enablejs")
        || lower.contains("异常流量")
        || lower.contains("unusual traffic")
    {
        return true;
    }

    // Zhihu security challenge
    if lower.contains("data-za-module=\"securitycaptcha\"")
        || lower.contains("知乎 - 安全验证")
        || (lower.contains("zhihu.com/signin?next=") && lower.contains("captcha"))
    {
        return true;
    }

    // WeChat verification template
    if lower.contains("weixin.qq.com/mp/readtemplate?t=pages/verify_")
        || lower.contains("环境异常")
        || lower.contains("当前访问环境存在安全风险")
    {
        return true;
    }

    // DDOS-GUARD and general WAF
    if lower.contains("ddos-guard")
        || lower.contains("waf-challenge")
        || lower.contains("data-security-challenge")
        || lower.contains("please enable javascript and cookies to continue")
    {
        return true;
    }

    false
}

/// Resolves effective Chrome DevTools Protocol endpoint:
/// 1. Explicitly configured parameter
/// 2. `TKLESS_CDP_ENDPOINT`, `TKLESS_CDP_ENDPOINT`, or `CDP_ENDPOINT` environment variables
/// 3. Default fallback "http://127.0.0.1:9222" if auto-cdp requested
pub fn resolve_cdp_endpoint(configured: Option<&str>, auto_cdp: bool) -> Option<String> {
    if let Some(endpoint) = configured {
        let trimmed = endpoint.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Ok(env_ep) = std::env::var("TKLESS_CDP_ENDPOINT") {
        let trimmed = env_ep.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Ok(env_ep) = std::env::var("TKLESS_CDP_ENDPOINT") {
        let trimmed = env_ep.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Ok(env_ep) = std::env::var("CDP_ENDPOINT") {
        let trimmed = env_ep.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if auto_cdp {
        Some("http://127.0.0.1:9222".to_string())
    } else {
        None
    }
}

/// Helper function to URL-encode a string.
fn urlencoding(input: &str) -> String {
    url::form_urlencoded::byte_serialize(input.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn browser_guard_blocks_private_requests_before_continuing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            for _ in 0..2 {
                ws.next().await.unwrap().unwrap();
            }
            ws.send(Message::Text(
                json!({"id":1000001,"result":{}}).to_string().into(),
            ))
            .await
            .unwrap();
            for (url, method, resource, expected) in [
                (
                    "http://127.0.0.1/private",
                    "GET",
                    "Document",
                    "Fetch.failRequest",
                ),
                (
                    "https://1.1.1.1/public",
                    "GET",
                    "Document",
                    "Fetch.continueRequest",
                ),
                ("https://1.1.1.1/write", "POST", "XHR", "Fetch.failRequest"),
                (
                    "https://2.2.2.2/other",
                    "GET",
                    "Document",
                    "Fetch.failRequest",
                ),
            ] {
                ws.send(Message::Text(json!({"method":"Fetch.requestPaused","params":{"requestId":"test","resourceType":resource,"request":{"url":url,"method":method}}}).to_string().into())).await.unwrap();
                let reply = ws.next().await.unwrap().unwrap().into_text().unwrap();
                let reply: Value = serde_json::from_str(&reply).unwrap();
                assert_eq!(reply["method"], expected);
            }
            ws.send(Message::Text(
                json!({"method":"Page.loadEventFired"}).to_string().into(),
            ))
            .await
            .unwrap();
        });
        let (mut ws, _) = connect_async(format!("ws://{address}")).await.unwrap();
        let mut guard = BrowserRequestGuard::enable(
            &mut ws,
            "https://1.1.1.1/",
            &CdpOptions {
                read_only_requests: true,
                navigation_origin: Some("https://1.1.1.1".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let event = timeout(Duration::from_secs(3), guard.next(&mut ws))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(event.into_text().unwrap().contains("Page.loadEventFired"));
        peer.await.unwrap();
    }

    #[test]
    fn test_normalize_cdp_endpoint() {
        let (ep, is_ws) = normalize_cdp_endpoint("9222");
        assert_eq!(ep, "http://127.0.0.1:9222");
        assert!(!is_ws);

        let (ep, is_ws) = normalize_cdp_endpoint("localhost:9222");
        assert_eq!(ep, "http://localhost:9222");
        assert!(!is_ws);

        let (ep, is_ws) = normalize_cdp_endpoint("http://127.0.0.1:9222/");
        assert_eq!(ep, "http://127.0.0.1:9222");
        assert!(!is_ws);

        let (ep, is_ws) = normalize_cdp_endpoint("ws://127.0.0.1:9222/devtools/page/123");
        assert_eq!(ep, "ws://127.0.0.1:9222/devtools/page/123");
        assert!(is_ws);
    }

    #[test]
    fn test_locate_system_browser() {
        // Must execute without panic on any host system
        let browser = locate_system_browser();
        if let Some(ref path) = browser {
            assert!(path.is_file(), "Located browser binary must be a file");
        }
    }

    #[test]
    fn test_cdp_options_defaults_and_builder() {
        let default_opts = CdpOptions::default();
        assert_eq!(default_opts.block_resources, Some(true));
        assert_eq!(default_opts.auto_scroll, Some(false));
        assert_eq!(default_opts.scroll_count, Some(5));

        let custom_opts = CdpOptions {
            auto_scroll: Some(true),
            scroll_count: Some(10),
            block_resources: Some(false),
            ..Default::default()
        };
        assert_eq!(custom_opts.block_resources, Some(false));
        assert_eq!(custom_opts.auto_scroll, Some(true));
        assert_eq!(custom_opts.scroll_count, Some(10));
    }

    #[test]
    fn network_capture_bounds_are_enforced() {
        assert!(NetworkCapture::default().validate().is_ok());
        assert!(NetworkCapture {
            max_responses: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(NetworkCapture {
            max_body_bytes: 10,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(NetworkCapture {
            mime_prefixes: Vec::new(),
            ..Default::default()
        }
        .validate()
        .is_err());

        let capture = NetworkCapture {
            mime_prefixes: vec!["application/json".into(), "text/event-stream".into()],
            ..Default::default()
        };
        assert!(capture.validate().is_ok());
        assert!(capture.matches_mime("application/json; charset=utf-8"));
        assert!(!capture.matches_mime("text/html"));
    }

    #[test]
    fn capture_truncation_respects_utf8_boundaries() {
        assert_eq!(truncate_utf8("abcdef", 3), ("abc".to_string(), true));
        assert_eq!(truncate_utf8("a你", 2), ("a".to_string(), true));
        assert_eq!(truncate_utf8("abc", 3), ("abc".to_string(), false));
    }
}
