use crate::error::PageclawError;
use crate::model::{FetchOptions, OnTooLarge};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, COOKIE, USER_AGENT};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use tracing::{debug, warn};
use url::Url;

/// The raw fetched response from the web server.
#[derive(Debug, Clone)]
pub struct FetchedResponse {
    pub requested_url: String,
    pub final_url: String,
    pub status: u16,
    pub html: String,
    pub truncated: bool,
}

pub struct Fetcher {
    client: reqwest::Client,
    options: FetchOptions,
}

impl Fetcher {
    /// Create a new fetcher instance with custom options.
    pub fn new(options: FetchOptions) -> Result<Self, PageclawError> {
        let mut headers = HeaderMap::new();

        let ua = options.user_agent.as_deref().unwrap_or(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 PageClaw/0.1",
        );
        headers.insert(USER_AGENT, HeaderValue::from_str(ua).map_err(|e| PageclawError::InvalidArgument(e.to_string()))?);
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"),
        );
        headers.insert(
            ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.9,zh-CN;q=0.8,zh;q=0.7"),
        );

        // Resolve effective cookies: merge file cookies with command-line cookies
        let mut effective_cookie = options.cookie.clone();
        if let Some(ref cf) = options.cookie_file {
            let path = Path::new(cf);
            let file_cookie = load_cookie_from_file(path)?;
            effective_cookie = match effective_cookie {
                Some(cli_cookie) => Some(merge_cookies(&file_cookie, &cli_cookie)),
                None => Some(file_cookie),
            };
        }

        if let Some(ref cookie_str) = effective_cookie {
            headers.insert(
                COOKIE,
                HeaderValue::from_str(cookie_str).map_err(|e| PageclawError::InvalidArgument(e.to_string()))?,
            );
        }

        for (key, val) in &options.headers {
            if let (Ok(hname), Ok(hval)) = (HeaderName::from_str(key), HeaderValue::from_str(val)) {
                headers.insert(hname, hval);
            }
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(options.timeout_secs))
            .connect_timeout(Duration::from_secs(options.connect_timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;

        Ok(Self { client, options })
    }

    /// Access the underlying reqwest HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Fetch a webpage by URL with retry backoff and decode its content into a clean string.
    pub async fn fetch(&self, target_url: &str) -> Result<FetchedResponse, PageclawError> {
        let parsed_url = Url::parse(target_url)?;
        self.validate_ssrf(&parsed_url).await?;
        debug!(url = target_url, "Fetching URL");

        let connect_timeout = Duration::from_secs(self.options.connect_timeout_secs);
        let mut attempt = 0;
        let mut response = loop {
            let send_fut = self.client.get(parsed_url.clone()).send();
            match tokio::time::timeout(connect_timeout, send_fut).await {
                Ok(Ok(resp)) => {
                    let status = resp.status();
                    if status.is_server_error() && attempt < self.options.retries {
                        attempt += 1;
                        let delay = Duration::from_millis(200 * (1 << (attempt - 1))).min(Duration::from_secs(5));
                        warn!(
                            url = target_url,
                            status = status.as_u16(),
                            attempt = attempt,
                            retries = self.options.retries,
                            delay = ?delay,
                            "Server returned 5xx error, retrying with exponential backoff..."
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    break resp;
                }
                Ok(Err(err)) => {
                    if attempt < self.options.retries {
                        attempt += 1;
                        let delay = Duration::from_millis(200 * (1 << (attempt - 1))).min(Duration::from_secs(5));
                        warn!(
                            url = target_url,
                            error = %err,
                            attempt = attempt,
                            retries = self.options.retries,
                            delay = ?delay,
                            "HTTP request failed, retrying with exponential backoff..."
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(PageclawError::Network(err));
                }
                Err(_) => {
                    if attempt < self.options.retries {
                        attempt += 1;
                        let delay = Duration::from_millis(200 * (1 << (attempt - 1))).min(Duration::from_secs(5));
                        warn!(
                            url = target_url,
                            attempt = attempt,
                            retries = self.options.retries,
                            delay = ?delay,
                            "Connection handshake timed out after {:?}, retrying...",
                            connect_timeout
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(PageclawError::ConnectionTimeout(format!(
                        "Connection handshake to '{target_url}' timed out after {:?}",
                        connect_timeout
                    )));
                }
            }
        };

        let status = response.status();
        let status_u16 = status.as_u16();
        let final_url = response.url().to_string();

        if !self.options.allow_http_errors && (status.is_client_error() || status.is_server_error()) {
            return Err(PageclawError::HttpStatusError {
                status: status_u16,
                url: final_url,
            });
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Stream body with strict or truncating size check
        let mut raw_bytes = Vec::new();
        let mut truncated = false;

        while let Some(chunk) = response.chunk().await? {
            if raw_bytes.len() + chunk.len() > self.options.max_bytes {
                match self.options.on_too_large {
                    OnTooLarge::Error => {
                        return Err(PageclawError::ResponseTooLarge {
                            limit: self.options.max_bytes,
                            received: raw_bytes.len() + chunk.len(),
                        });
                    }
                    OnTooLarge::Truncate => {
                        let remaining = self.options.max_bytes.saturating_sub(raw_bytes.len());
                        raw_bytes.extend_from_slice(&chunk[..remaining]);
                        truncated = true;
                        warn!(
                            limit = self.options.max_bytes,
                            "Response exceeded max-size, gracefully truncating body"
                        );
                        break;
                    }
                }
            }
            raw_bytes.extend_from_slice(&chunk);
        }

        // Decode character encoding (UTF-8, GBK, Shift-JIS, etc.)
        let html = decode_html(&raw_bytes, content_type.as_deref());

        Ok(FetchedResponse {
            requested_url: target_url.to_string(),
            final_url,
            status: status_u16,
            html,
            truncated,
        })
    }

    /// Validates that target URL does not point to internal/loopback/cloud-metadata addresses (SSRF guard).
    pub async fn validate_ssrf(&self, url: &Url) -> Result<(), PageclawError> {
        if self.options.allow_private_addresses {
            return Ok(());
        }

        let host = match url.host_str() {
            Some(h) => h,
            None => return Ok(()),
        };

        let host_lower = host.to_ascii_lowercase();
        if host_lower == "localhost" || host_lower.ends_with(".local") {
            return Err(PageclawError::SsrfBlocked(format!("Access to '{host}' is restricted")));
        }

        // Direct IP literal check
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            if is_private_ip(ip) {
                return Err(PageclawError::SsrfBlocked(format!(
                    "Access to private/loopback IP {ip} is restricted"
                )));
            }
            return Ok(());
        }

        // Hostname DNS resolution check
        let port = url.port_or_known_default().unwrap_or(80);
        let host_with_port = format!("{host}:{port}");
        if let Ok(mut addrs) = tokio::net::lookup_host(&host_with_port).await {
            if let Some(addr) = addrs.next() {
                if is_private_ip(addr.ip()) {
                    return Err(PageclawError::SsrfBlocked(format!(
                        "Host '{host}' resolved to restricted private IP {}",
                        addr.ip()
                    )));
                }
            }
        }

        Ok(())
    }
}

fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ipv4) => {
            ipv4.is_loopback()
                || ipv4.is_private()
                || ipv4.is_link_local()
                || ipv4.is_broadcast()
                || ipv4.is_unspecified()
                || ipv4.octets()[0] == 0
        }
        std::net::IpAddr::V6(ipv6) => {
            ipv6.is_loopback()
                || ipv6.is_unspecified()
                || (ipv6.segments()[0] & 0xfe00) == 0xfc00
                || (ipv6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Detect charset and decode raw bytes into UTF-8 string.
fn decode_html(raw_bytes: &[u8], content_type: Option<&str>) -> String {
    // 1. Try charset from Content-Type HTTP header
    let mut charset_name = content_type.and_then(extract_charset_from_content_type);

    // 2. If not found in headers, probe HTML <meta charset="..."> in the first 2KB
    if charset_name.is_none() {
        let probe_len = raw_bytes.len().min(2048);
        if let Ok(probe_str) = std::str::from_utf8(&raw_bytes[..probe_len]) {
            charset_name = extract_charset_from_html_meta(probe_str);
        }
    }

    // 3. Resolve encoding via encoding_rs
    if let Some(ref name) = charset_name {
        if let Some(encoding) = encoding_rs::Encoding::for_label(name.as_bytes()) {
            let (decoded, _, malformed) = encoding.decode(raw_bytes);
            if malformed {
                warn!(charset = name, "Malformed characters encountered while decoding");
            }
            return decoded.into_owned();
        }
    }

    // Default: try standard UTF-8, then fall back to lossy UTF-8
    match std::str::from_utf8(raw_bytes) {
        Ok(valid_utf8) => valid_utf8.to_string(),
        Err(_) => {
            let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(raw_bytes);
            decoded.into_owned()
        }
    }
}

fn extract_charset_from_content_type(content_type: &str) -> Option<String> {
    for part in content_type.split(';') {
        let trimmed = part.trim();
        if trimmed.to_ascii_lowercase().starts_with("charset=") {
            let val = trimmed["charset=".len()..].trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn extract_charset_from_html_meta(head_str: &str) -> Option<String> {
    let lower = head_str.to_ascii_lowercase();

    // Pattern 1: <meta charset="utf-8">
    if let Some(pos) = lower.find("charset=") {
        let after = &head_str[pos + 8..];
        let mut end = 0;
        let quote = after.chars().next();
        if let Some(q @ ('"' | '\'')) = quote {
            let inner = &after[1..];
            if let Some(close_pos) = inner.find(q) {
                return Some(inner[..close_pos].trim().to_string());
            }
        } else {
            for c in after.chars() {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    end += c.len_utf8();
                } else {
                    break;
                }
            }
            if end > 0 {
                return Some(after[..end].to_string());
            }
        }
    }
    None
}

/// Loads and parses cookie credentials from a file supporting both Netscape format
/// and standard HTTP header format.
pub fn load_cookie_from_file(path: &Path) -> Result<String, PageclawError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        PageclawError::InvalidArgument(format!("Cannot read cookie file '{}': {e}", path.display()))
    })?;
    Ok(parse_cookie_content(&content))
}

/// Parses raw cookie string content in Netscape or standard header format into "key1=val1; key2=val2".
pub fn parse_cookie_content(content: &str) -> String {
    let mut cookies: Vec<(String, String)> = Vec::new();

    for raw_line in content.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Handle Netscape HttpOnly prefix
        let (line, is_httponly) = if let Some(stripped) = trimmed.strip_prefix("#HttpOnly_") {
            (stripped.trim(), true)
        } else {
            (trimmed, false)
        };

        if !is_httponly && line.starts_with('#') {
            continue;
        }

        if line.contains('\t') {
            // Netscape tab-separated format: domain, flag, path, secure, expiration, name, value
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() >= 7 {
                let name = cols[5].trim();
                let val = cols[6].trim();
                if !name.is_empty() {
                    insert_or_update_cookie(&mut cookies, name, val);
                }
            } else if cols.len() == 6 {
                let name = cols[4].trim();
                let val = cols[5].trim();
                if !name.is_empty() {
                    insert_or_update_cookie(&mut cookies, name, val);
                }
            }
        } else {
            // Standard HTTP cookie header format: "key=val; key2=val2" or multiline
            let cleaned = if let Some(rest) = line.strip_prefix("Cookie:") {
                rest.trim()
            } else if let Some(rest) = line.strip_prefix("cookie:") {
                rest.trim()
            } else {
                line
            };

            for part in cleaned.split(';') {
                let part_trimmed = part.trim();
                if let Some((k, v)) = part_trimmed.split_once('=') {
                    let k = k.trim();
                    let v = v.trim();
                    if !k.is_empty() {
                        insert_or_update_cookie(&mut cookies, k, v);
                    }
                }
            }
        }
    }

    format_cookie_pairs(&cookies)
}

fn insert_or_update_cookie(cookies: &mut Vec<(String, String)>, name: &str, val: &str) {
    if let Some(existing) = cookies.iter_mut().find(|(k, _)| k == name) {
        existing.1 = val.to_string();
    } else {
        cookies.push((name.to_string(), val.to_string()));
    }
}

fn format_cookie_pairs(cookies: &[(String, String)]) -> String {
    cookies
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Merges cookies from two sources (e.g. cookie file and command line `--cookie`),
/// with command-line cookies taking precedence on key collision.
pub fn merge_cookies(file_cookie: &str, cli_cookie: &str) -> String {
    let mut cookies = Vec::new();

    // 1. Ingest file cookies
    for part in file_cookie.split(';') {
        let part_trimmed = part.trim();
        if let Some((k, v)) = part_trimmed.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            if !k.is_empty() {
                insert_or_update_cookie(&mut cookies, k, v);
            }
        }
    }

    // 2. Ingest CLI cookies (overwriting on collision)
    for part in cli_cookie.split(';') {
        let part_trimmed = part.trim();
        if let Some((k, v)) = part_trimmed.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            if !k.is_empty() {
                insert_or_update_cookie(&mut cookies, k, v);
            }
        }
    }

    format_cookie_pairs(&cookies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_charset_from_content_type() {
        assert_eq!(
            extract_charset_from_content_type("text/html; charset=UTF-8"),
            Some("UTF-8".to_string())
        );
        assert_eq!(
            extract_charset_from_content_type("text/html; charset=\"gb2312\""),
            Some("gb2312".to_string())
        );
        assert_eq!(extract_charset_from_content_type("text/html"), None);
    }

    #[test]
    fn test_decode_html_utf8() {
        let html_bytes = b"<html><head><meta charset=\"utf-8\"></head><body>\xE4\xBD\xA0\xE5\xA5\xBD</body></html>";
        let decoded = decode_html(html_bytes, None);
        assert!(decoded.contains("你好"));
    }

    #[test]
    fn test_parse_netscape_cookies() {
        let netscape = r#"
# Netscape HTTP Cookie File
# http://curl.haxx.se/rfc/cookie_spec.html
.bilibili.com	TRUE	/	FALSE	1745678900	SESSDATA	val123%2Cxyz
.bilibili.com	TRUE	/	FALSE	1745678900	bili_jct	token_abc
#HttpOnly_.bilibili.com	TRUE	/	TRUE	1745678900	DedeUserID	12345678
"#;
        let parsed = parse_cookie_content(netscape);
        assert!(parsed.contains("SESSDATA=val123%2Cxyz"));
        assert!(parsed.contains("bili_jct=token_abc"));
        assert!(parsed.contains("DedeUserID=12345678"));
    }

    #[test]
    fn test_parse_header_cookies() {
        let header = "Cookie: SESSDATA=foo; bili_jct=bar; DedeUserID=999";
        let parsed = parse_cookie_content(header);
        assert_eq!(parsed, "SESSDATA=foo; bili_jct=bar; DedeUserID=999");
    }

    #[test]
    fn test_merge_cookies_cli_priority() {
        let file_cookie = "SESSDATA=old_val; bili_jct=bar; DedeUserID=123";
        let cli_cookie = "SESSDATA=new_val; buvid3=xyz";
        let merged = merge_cookies(file_cookie, cli_cookie);

        assert!(merged.contains("SESSDATA=new_val"));
        assert!(merged.contains("bili_jct=bar"));
        assert!(merged.contains("DedeUserID=123"));
        assert!(merged.contains("buvid3=xyz"));
        assert!(!merged.contains("SESSDATA=old_val"));
    }
}
