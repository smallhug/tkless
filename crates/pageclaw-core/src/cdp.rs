//! Chrome DevTools Protocol (CDP) lightweight headless browser sidecar.
//!
//! Connects to an existing Chrome/Chromium/Edge browser instance running with `--remote-debugging-port=9222`
//! (or custom CDP WebSocket/HTTP endpoint), navigates to the target page, executes JavaScript,
//! waits for DOM hydration / anti-bot challenges to settle, and extracts the fully-rendered `outerHTML`.

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::debug;
use url::Url;

use crate::error::PageclawError;

/// Options controlling CDP browser interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpOptions {
    /// DevTools HTTP or WebSocket endpoint, e.g. "http://127.0.0.1:9222" or "ws://127.0.0.1:9222/devtools/..."
    pub endpoint: String,
    /// Optional additional wait time in milliseconds for dynamic JS rendering/hydration.
    pub wait_ms: Option<u64>,
    /// Overall page load and extraction timeout in milliseconds (default: 30000).
    pub timeout_ms: Option<u64>,
    /// Custom User-Agent to emulate in the browser session.
    pub user_agent: Option<String>,
    /// Cookie header string or platform cookies to inject into the browser target.
    pub cookie: Option<String>,
}

impl Default for CdpOptions {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:9222".to_string(),
            wait_ms: Some(1000),
            timeout_ms: Some(30_000),
            user_agent: None,
            cookie: None,
        }
    }
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
pub async fn fetch_html_via_cdp(url: &str, options: &CdpOptions) -> Result<String, PageclawError> {
    let (base_endpoint, is_direct_ws) = normalize_cdp_endpoint(&options.endpoint);
    let total_timeout = Duration::from_millis(options.timeout_ms.unwrap_or(30_000));

    timeout(total_timeout, async {
        if is_direct_ws {
            connect_and_extract_html(&base_endpoint, url, options, None).await
        } else {
            // 1. Create a new target tab via Chrome DevTools HTTP API
            let req_timeout = Duration::from_millis(options.timeout_ms.unwrap_or(30_000).min(5000));
            let client = reqwest::Client::builder()
                .connect_timeout(req_timeout)
                .timeout(req_timeout)
                .build()
                .map_err(|e| PageclawError::CdpError(format!("创建 HTTP 客户端失败: {e}")))?;

            let new_target_url = format!("{}/json/new?{}", base_endpoint, urlencoding(url));
            debug!(endpoint = %new_target_url, "通过 CDP HTTP 接口创建新页面 Tab");

            let resp = match client.put(&new_target_url).send().await {
                Ok(r) if r.status().is_success() => Ok(r),
                _ => client.get(&new_target_url).send().await,
            }
            .map_err(|err| {
                PageclawError::CdpError(format!(
                    "无法连接到 Chrome CDP 端点 '{base_endpoint}': {err}。请确保已通过 `google-chrome --remote-debugging-port=9222` 或 `msedge --remote-debugging-port=9222` 启动浏览器远程调试。"
                ))
            })?;

            if !resp.status().is_success() {
                return Err(PageclawError::CdpError(format!(
                    "CDP 端点返回错误状态码 {}: {}",
                    resp.status(),
                    base_endpoint
                )));
            }

            let target: DevToolsTarget = resp.json().await.map_err(|err| {
                PageclawError::CdpError(format!("解析 CDP 目标元数据失败: {err}"))
            })?;

            let ws_url = target.web_socket_debugger_url.ok_or_else(|| {
                PageclawError::CdpError("CDP 目标未返回有效的 webSocketDebuggerUrl".to_string())
            })?;

            let target_id = target.id.clone();
            let result = connect_and_extract_html(&ws_url, url, options, Some(&base_endpoint)).await;

            // Close tab after extraction to prevent tab leak
            let close_url = format!("{}/json/close/{}", base_endpoint, target_id);
            let _ = client.get(&close_url).send().await;

            result
        }
    })
    .await
    .map_err(|_| {
        PageclawError::ConnectionTimeout(format!(
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
    _http_base: Option<&str>,
) -> Result<String, PageclawError> {
    debug!(ws_url = %ws_url, "连接 CDP WebSocket 调试会话");

    let (mut ws_stream, _) = connect_async(ws_url).await.map_err(|err| {
        PageclawError::CdpError(format!("无法建立 CDP WebSocket 连接 ({ws_url}): {err}"))
    })?;

    let mut message_id = 1u64;

    // 1. Enable Page and Runtime domains
    send_cdp_cmd(&mut ws_stream, message_id, "Page.enable", json!({})).await?;
    message_id += 1;

    send_cdp_cmd(&mut ws_stream, message_id, "Runtime.enable", json!({})).await?;
    message_id += 1;

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
        while let Some(msg_result) = ws_stream.next().await {
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
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        }
    }

    // 7. Extract document.documentElement.outerHTML
    let eval_cmd_id = message_id;
    send_cdp_cmd(
        &mut ws_stream,
        eval_cmd_id,
        "Runtime.evaluate",
        json!({
            "expression": "document.documentElement.outerHTML",
            "returnByValue": true,
            "awaitPromise": true,
        }),
    )
    .await?;

    let eval_timeout = Duration::from_secs(10);
    let html = timeout(eval_timeout, async {
        while let Some(msg_result) = ws_stream.next().await {
            if let Ok(Message::Text(text)) = msg_result {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if v.get("id").and_then(|id| id.as_u64()) == Some(eval_cmd_id) {
                        if let Some(res_val) = v
                            .get("result")
                            .and_then(|r| r.get("result"))
                            .and_then(|r| r.get("value"))
                            .and_then(|val| val.as_str())
                        {
                            return Ok(res_val.to_string());
                        }
                    }
                }
            }
        }
        Err(PageclawError::CdpError(
            "未能从 CDP Runtime.evaluate 获取 outerHTML".to_string(),
        ))
    })
    .await
    .map_err(|_| PageclawError::ConnectionTimeout("CDP Runtime.evaluate 执行超时".to_string()))??;

    Ok(html)
}

/// Helper function to send a JSON-RPC command over a WebSocket stream.
async fn send_cdp_cmd<S: SinkExt<Message> + Unpin>(
    ws: &mut S,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), PageclawError>
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
        .map_err(|err| PageclawError::CdpError(format!("发送 CDP 指令失败 ({method}): {err}")))?;
    Ok(())
}

/// Captures a rendered visual screenshot of a given URL via Chrome DevTools Protocol.
/// Returns raw image bytes (PNG or JPEG).
pub async fn capture_screenshot_via_cdp(
    url: &str,
    options: &CdpOptions,
    format: Option<&str>,
) -> Result<Vec<u8>, PageclawError> {
    let (base_endpoint, is_direct_ws) = normalize_cdp_endpoint(&options.endpoint);
    let total_timeout = Duration::from_millis(options.timeout_ms.unwrap_or(30_000));

    timeout(total_timeout, async {
        if is_direct_ws {
            connect_and_capture_screenshot(&base_endpoint, url, options, format).await
        } else {
            let req_timeout = Duration::from_millis(options.timeout_ms.unwrap_or(30_000).min(5000));
            let client = reqwest::Client::builder()
                .connect_timeout(req_timeout)
                .timeout(req_timeout)
                .build()
                .map_err(|e| PageclawError::CdpError(format!("创建 HTTP 客户端失败: {e}")))?;

            let new_target_url = format!("{}/json/new?{}", base_endpoint, urlencoding(url));
            debug!(endpoint = %new_target_url, "通过 CDP HTTP 接口创建新页面 Tab (截图)");

            let resp = match client.put(&new_target_url).send().await {
                Ok(r) if r.status().is_success() => Ok(r),
                _ => client.get(&new_target_url).send().await,
            }
            .map_err(|err| {
                PageclawError::CdpError(format!(
                    "无法连接到 Chrome CDP 端点 '{base_endpoint}': {err}。请确保已通过 `google-chrome --remote-debugging-port=9222` 或 `msedge --remote-debugging-port=9222` 启动浏览器远程调试。"
                ))
            })?;

            if !resp.status().is_success() {
                return Err(PageclawError::CdpError(format!(
                    "CDP 端点返回错误状态码 {}: {}",
                    resp.status(),
                    base_endpoint
                )));
            }

            let target: DevToolsTarget = resp.json().await.map_err(|err| {
                PageclawError::CdpError(format!("解析 CDP 目标元数据失败: {err}"))
            })?;

            let ws_url = target.web_socket_debugger_url.ok_or_else(|| {
                PageclawError::CdpError("CDP 目标未返回有效的 webSocketDebuggerUrl".to_string())
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
        PageclawError::ConnectionTimeout(format!(
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
) -> Result<Vec<u8>, PageclawError> {
    debug!(ws_url = %ws_url, "连接 CDP WebSocket 截取屏幕快照");

    let (mut ws_stream, _) = connect_async(ws_url).await.map_err(|err| {
        PageclawError::CdpError(format!("无法建立 CDP WebSocket 连接 ({ws_url}): {err}"))
    })?;

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
        while let Some(msg_result) = ws_stream.next().await {
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
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        }
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
        while let Some(msg_result) = ws_stream.next().await {
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
        Err(PageclawError::CdpError(
            "未能从 CDP Page.captureScreenshot 获取图像数据".to_string(),
        ))
    })
    .await
    .map_err(|_| {
        PageclawError::ConnectionTimeout("CDP Page.captureScreenshot 执行超时".to_string())
    })??;

    decode_base64(&b64_str)
        .ok_or_else(|| PageclawError::CdpError("解析 CDP 截图 Base64 数据失败".to_string()))
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
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);

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
/// (e.g. Cloudflare Turnstile, 5-second shield, Zhihu captcha, WeChat verification, DDOS-GUARD, WAF block).
pub fn is_anti_scraping_challenge(html: &str, status: u16) -> bool {
    if status == 403 || status == 503 {
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
/// 2. `PAGECLAW_CDP_ENDPOINT` or `CDP_ENDPOINT` environment variables
/// 3. Default fallback "http://127.0.0.1:9222" if auto-cdp requested
pub fn resolve_cdp_endpoint(configured: Option<&str>, auto_cdp: bool) -> Option<String> {
    if let Some(endpoint) = configured {
        let trimmed = endpoint.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Ok(env_ep) = std::env::var("PAGECLAW_CDP_ENDPOINT") {
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
}
