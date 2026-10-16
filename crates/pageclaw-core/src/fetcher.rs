use crate::error::PageclawError;
use crate::model::{FetchOptions, OnTooLarge};
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, COOKIE, USER_AGENT,
};
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
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(ua).map_err(|e| PageclawError::InvalidArgument(e.to_string()))?,
        );
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
                HeaderValue::from_str(cookie_str)
                    .map_err(|e| PageclawError::InvalidArgument(e.to_string()))?,
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
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self { client, options })
    }

    /// Access the underlying reqwest HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Fetch a webpage by URL with retry backoff, full-chain SSRF validation across redirects,
    /// and decode its content into a clean string.
    pub async fn fetch(&self, target_url: &str) -> Result<FetchedResponse, PageclawError> {
        let mut current_url = Url::parse(target_url)?;
        let mut redirect_count = 0;
        const MAX_REDIRECTS: usize = 10;
        let connect_timeout = Duration::from_secs(self.options.connect_timeout_secs);

        let mut response = loop {
            // Strictly validate SSRF on EVERY hop (initial request and all redirect targets)
            self.validate_ssrf(&current_url).await?;
            debug!(url = %current_url, redirect_hop = redirect_count, "Fetching URL with SSRF guard");

            let mut attempt = 0;
            let resp = loop {
                let send_fut = self.client.get(current_url.clone()).send();
                match tokio::time::timeout(connect_timeout, send_fut).await {
                    Ok(Ok(r)) => {
                        let status = r.status();
                        if status.is_server_error() && attempt < self.options.retries {
                            attempt += 1;
                            let delay = Duration::from_millis(200 * (1 << (attempt - 1)))
                                .min(Duration::from_secs(5));
                            warn!(
                                url = %current_url,
                                status = status.as_u16(),
                                attempt = attempt,
                                retries = self.options.retries,
                                delay = ?delay,
                                "Server returned 5xx error, retrying with exponential backoff..."
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        break r;
                    }
                    Ok(Err(err)) => {
                        if attempt < self.options.retries {
                            attempt += 1;
                            let delay = Duration::from_millis(200 * (1 << (attempt - 1)))
                                .min(Duration::from_secs(5));
                            warn!(
                                url = %current_url,
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
                            let delay = Duration::from_millis(200 * (1 << (attempt - 1)))
                                .min(Duration::from_secs(5));
                            warn!(
                                url = %current_url,
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
                            "Connection handshake to '{current_url}' timed out after {:?}",
                            connect_timeout
                        )));
                    }
                }
            };

            // Check if this response is an HTTP redirect (301, 302, 303, 307, 308)
            let status = resp.status();
            if status.is_redirection() {
                if let Some(loc_header) = resp.headers().get(reqwest::header::LOCATION) {
                    if let Ok(loc_str) = loc_header.to_str() {
                        let next_url = current_url.join(loc_str.trim()).map_err(|e| {
                            PageclawError::InvalidArgument(format!(
                                "Malformed redirect Location header '{loc_str}': {e}"
                            ))
                        })?;

                        redirect_count += 1;
                        if redirect_count > MAX_REDIRECTS {
                            return Err(PageclawError::InvalidArgument(format!(
                                "Too many redirects (exceeded maximum limit of {MAX_REDIRECTS})"
                            )));
                        }

                        debug!(
                            from = %current_url,
                            to = %next_url,
                            redirect_count = redirect_count,
                            "Following HTTP redirect with strict SSRF re-validation"
                        );
                        current_url = next_url;
                        continue;
                    }
                }
            }

            // Not a redirect (or terminal response): break and process body
            break resp;
        };

        let status = response.status();
        let status_u16 = status.as_u16();
        let final_url = current_url.to_string();

        if !self.options.allow_http_errors && (status.is_client_error() || status.is_server_error())
        {
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
            return Err(PageclawError::SsrfBlocked(format!(
                "Access to '{host}' is restricted"
            )));
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

        // Hostname DNS resolution check: validate ALL resolved IP addresses (prevents multi-A-record bypass)
        let port = url.port_or_known_default().unwrap_or(80);
        let host_with_port = format!("{host}:{port}");
        if let Ok(addrs) = tokio::net::lookup_host(&host_with_port).await {
            for addr in addrs {
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
            let oct = ipv4.octets();
            ipv4.is_loopback()
                || ipv4.is_private()
                || ipv4.is_link_local()
                || ipv4.is_broadcast()
                || ipv4.is_unspecified()
                || oct[0] == 0
                || (oct[0] == 100 && (oct[1] & 0xc0) == 64) // 100.64.0.0/10 (CGNAT)
                || (oct[0] == 192 && oct[1] == 0 && oct[2] == 0) // 192.0.0.0/24 (IETF Protocol Assignments)
                || (oct[0] == 192 && oct[1] == 0 && oct[2] == 2) // 192.0.2.0/24 (TEST-NET-1)
                || (oct[0] == 198 && (oct[1] & 0xfe) == 18) // 198.18.0.0/15 (Benchmarking)
                || (oct[0] == 198 && oct[1] == 51 && oct[2] == 100) // 198.51.100.0/24 (TEST-NET-2)
                || (oct[0] == 203 && oct[1] == 0 && oct[2] == 113) // 203.0.113.0/24 (TEST-NET-3)
                || (oct[0] & 0xf0) == 240 // 240.0.0.0/4 (Reserved)
        }
        std::net::IpAddr::V6(ipv6) => {
            // Check IPv4-mapped IPv6 (e.g. ::ffff:127.0.0.1 or ::ffff:169.254.169.254)
            if let Some(ipv4) = ipv6.to_ipv4_mapped() {
                return is_private_ip(std::net::IpAddr::V4(ipv4));
            }
            ipv6.is_loopback()
                || ipv6.is_unspecified()
                || (ipv6.segments()[0] & 0xfe00) == 0xfc00 // Unique Local Address fc00::/7
                || (ipv6.segments()[0] & 0xffc0) == 0xfe80 // Link-Local Address fe80::/10
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
                warn!(
                    charset = name,
                    "Malformed characters encountered while decoding"
                );
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
            let val = trimmed["charset=".len()..]
                .trim()
                .trim_matches('"')
                .trim_matches('\'');
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

/// Strictly checks whether a host matches an apex domain or is a valid subdomain.
/// Prevents typosquatting / domain spoofing vulnerabilities (e.g. `evilzhihu.com`, `fakebilibili.com`).
pub fn is_subdomain_or_exact(host: &str, apex_domain: &str) -> bool {
    let host_lower = host.trim().to_lowercase();
    let apex_lower = apex_domain.trim().to_lowercase();

    if host_lower == apex_lower {
        return true;
    }

    if let Some(prefix) = host_lower.strip_suffix(&apex_lower) {
        return prefix.ends_with('.');
    }

    false
}

/// Detects platform identifier from a URL string (e.g. "bilibili", "zhihu", "wechat").
/// Uses strict domain boundary matching to prevent cross-site cookie leaks to spoofed domains.
pub fn detect_platform_from_url(url_str: &str) -> Option<&'static str> {
    let parsed = Url::parse(url_str).ok()?;
    let host = parsed.host_str()?.to_lowercase();

    if is_subdomain_or_exact(&host, "bilibili.com") || is_subdomain_or_exact(&host, "b23.tv") {
        Some("bilibili")
    } else if is_subdomain_or_exact(&host, "zhihu.com") {
        Some("zhihu")
    } else if is_subdomain_or_exact(&host, "qq.com")
        || is_subdomain_or_exact(&host, "weixin.qq.com")
    {
        Some("wechat")
    } else if is_subdomain_or_exact(&host, "weibo.com") || is_subdomain_or_exact(&host, "weibo.cn")
    {
        Some("weibo")
    } else if is_subdomain_or_exact(&host, "xiaohongshu.com") {
        Some("xiaohongshu")
    } else {
        None
    }
}

/// Loads cookie credentials from file tailored for the specified target URL or platform.
/// Prevents cross-site cookie collisions by extracting credentials matching the platform prefix.
pub fn load_cookie_from_file_for_url(
    path: &Path,
    target_url: Option<&str>,
) -> Result<String, PageclawError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        PageclawError::InvalidArgument(format!("Cannot read cookie file '{}': {e}", path.display()))
    })?;

    let platform = target_url.and_then(detect_platform_from_url);
    Ok(parse_cookie_content_for_platform(&content, platform))
}

/// Loads and parses cookie credentials from a file supporting multi-platform prefixes,
/// Netscape format, and standard HTTP header format.
pub fn load_cookie_from_file(path: &Path) -> Result<String, PageclawError> {
    load_cookie_from_file_for_url(path, None)
}

/// Saves platform-specific cookies to file with mandatory platform prefix (e.g. `bilibili: SESSDATA=...`).
/// Automatically preserves existing platform credentials to prevent accidental overwrite.
pub fn save_platform_cookies_to_file(
    path: &Path,
    platform: &str,
    new_cookie: &str,
) -> Result<(), PageclawError> {
    let clean_platform = platform.trim().to_lowercase();
    let clean_cookie = new_cookie.trim();

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut platform_map: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    if path.is_file() {
        if let Ok(existing_content) = std::fs::read_to_string(path) {
            let mut current_section: Option<String> = None;

            for line in existing_content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }

                // INI section: [platform]
                if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() > 2 {
                    let section_name = trimmed[1..trimmed.len() - 1].trim().to_lowercase();
                    current_section = Some(section_name);
                    continue;
                }

                // Prefix format: platform: cookie
                if let Some((p, c)) = trimmed.split_once(':') {
                    let p = p.trim().to_lowercase();
                    let c = c.trim();
                    if !p.contains(' ') && c.contains('=') {
                        platform_map.insert(p, c.to_string());
                        continue;
                    }
                }

                // Line under [section]
                if let Some(ref sec) = current_section {
                    if trimmed.contains('=') {
                        let existing = platform_map.entry(sec.clone()).or_default();
                        if existing.is_empty() {
                            *existing = trimmed.to_string();
                        } else {
                            *existing = format!("{existing}; {trimmed}");
                        }
                    }
                }
            }
        }
    }

    // Insert or update target platform cookie
    platform_map.insert(clean_platform, clean_cookie.to_string());

    // Generate clean output with platform prefixes
    let mut out = String::new();
    out.push_str("# PageClaw Platform Credentials\n");
    out.push_str("# Format: <platform>: <cookies>\n\n");

    for (p, c) in &platform_map {
        out.push_str(&format!("{p}: {c}\n"));
    }

    std::fs::write(path, out.trim_end()).map_err(PageclawError::Io)?;
    Ok(())
}

/// Parses raw cookie string content into "key1=val1; key2=val2" strictly by platform prefix:
/// 1. Line prefix syntax: `platform: key1=val1; key2=val2`
/// 2. Section syntax: `[platform]\nkey1=val1; key2=val2`
/// If target platform is specified, strictly extracts only the credentials matching that platform.
pub fn parse_cookie_content_for_platform(content: &str, target_platform: Option<&str>) -> String {
    let clean_target = target_platform.map(|p| p.trim().to_lowercase());
    let mut platform_cookies: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();
    let mut current_section: Option<String> = None;

    for raw_line in content.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Section header: [bilibili]
        if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() > 2 {
            let section_name = trimmed[1..trimmed.len() - 1].trim().to_lowercase();
            current_section = Some(section_name);
            continue;
        }

        // Prefix syntax: `platform: key=val; ...`
        if let Some((p, rest)) = trimmed.split_once(':') {
            let p_clean = p.trim().to_lowercase();
            let rest_clean = rest.trim();
            if !p_clean.contains(' ') && rest_clean.contains('=') {
                let target_vec = platform_cookies.entry(p_clean).or_default();
                ingest_cookie_pairs(target_vec, rest_clean);
                continue;
            }
        }

        // Inside [section]
        if let Some(ref sec) = current_section {
            if trimmed.contains('=') {
                let target_vec = platform_cookies.entry(sec.clone()).or_default();
                ingest_cookie_pairs(target_vec, trimmed);
            }
        }
    }

    // Strict platform isolation:
    // If target platform is specified, strictly extract only matching credentials
    if let Some(ref tp) = clean_target {
        if let Some(pairs) = platform_cookies.get(tp) {
            return format_cookie_pairs(pairs);
        }
        return String::new();
    }

    // If no target platform requested, return empty or single platform if only one exists
    if platform_cookies.len() == 1 {
        if let Some((_, pairs)) = platform_cookies.into_iter().next() {
            return format_cookie_pairs(&pairs);
        }
    }

    String::new()
}

fn ingest_cookie_pairs(cookies: &mut Vec<(String, String)>, raw: &str) {
    let cleaned = if let Some(rest) = raw.strip_prefix("Cookie:") {
        rest.trim()
    } else if let Some(rest) = raw.strip_prefix("cookie:") {
        rest.trim()
    } else {
        raw
    };

    for part in cleaned.split(';') {
        let part_trimmed = part.trim();
        if let Some((k, v)) = part_trimmed.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            if !k.is_empty() {
                insert_or_update_cookie(cookies, k, v);
            }
        }
    }
}

/// Parses raw cookie string content in Netscape or standard header format into "key1=val1; key2=val2".
pub fn parse_cookie_content(content: &str) -> String {
    parse_cookie_content_for_platform(content, None)
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
    fn test_parse_prefix_cookies_single_platform() {
        let content = "bilibili: SESSDATA=foo; bili_jct=bar; DedeUserID=999";
        let parsed = parse_cookie_content_for_platform(content, Some("bilibili"));
        assert_eq!(parsed, "SESSDATA=foo; bili_jct=bar; DedeUserID=999");

        // Requesting zhihu should return empty
        let zhihu_parsed = parse_cookie_content_for_platform(content, Some("zhihu"));
        assert!(zhihu_parsed.is_empty());
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

    #[test]
    fn test_is_subdomain_or_exact() {
        // Exact matches
        assert!(is_subdomain_or_exact("zhihu.com", "zhihu.com"));
        assert!(is_subdomain_or_exact("bilibili.com", "bilibili.com"));

        // Valid subdomains
        assert!(is_subdomain_or_exact("zhuanlan.zhihu.com", "zhihu.com"));
        assert!(is_subdomain_or_exact("api.bilibili.com", "bilibili.com"));
        assert!(is_subdomain_or_exact("deep.sub.zhihu.com", "zhihu.com"));

        // Typosquatting / domain spoofing attempts MUST FAIL
        assert!(!is_subdomain_or_exact("evilzhihu.com", "zhihu.com"));
        assert!(!is_subdomain_or_exact("fake-bilibili.com", "bilibili.com"));
        assert!(!is_subdomain_or_exact("notzhihu.com", "zhihu.com"));
        assert!(!is_subdomain_or_exact("mybilibili.com", "bilibili.com"));
        assert!(!is_subdomain_or_exact("evilqq.com", "qq.com"));
    }

    #[test]
    fn test_detect_platform_from_url() {
        assert_eq!(
            detect_platform_from_url("https://www.bilibili.com/video/BV1xx411c7mD"),
            Some("bilibili")
        );
        assert_eq!(
            detect_platform_from_url("https://b23.tv/BV1xx"),
            Some("bilibili")
        );
        assert_eq!(
            detect_platform_from_url("https://zhuanlan.zhihu.com/p/123456"),
            Some("zhihu")
        );
        assert_eq!(
            detect_platform_from_url("https://www.zhihu.com/question/123"),
            Some("zhihu")
        );
        assert_eq!(
            detect_platform_from_url("https://mp.weixin.qq.com/s/abc"),
            Some("wechat")
        );

        // Security check: spoofed / typosquatted domains MUST NOT leak cookies!
        assert_eq!(
            detect_platform_from_url("https://evilzhihu.com/p/123"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://fake-bilibili.com/video/BV1"),
            None
        );
        assert_eq!(detect_platform_from_url("https://evilqq.com/s/abc"), None);
        assert_eq!(detect_platform_from_url("https://example.com/page"), None);
    }

    #[test]
    fn test_save_platform_cookies_prevents_overwrite() {
        let temp_dir = std::env::temp_dir().join(format!(
            "pageclaw_test_{}",
            std::time::SystemTime::now().elapsed().unwrap().as_nanos()
        ));
        let cookie_file = temp_dir.join("test_cookies.txt");

        // 1. Save bilibili cookies first
        save_platform_cookies_to_file(
            &cookie_file,
            "bilibili",
            "SESSDATA=bili_token_123; bili_jct=jct_abc",
        )
        .unwrap();

        let content1 = std::fs::read_to_string(&cookie_file).unwrap();
        assert!(content1.contains("bilibili: SESSDATA=bili_token_123; bili_jct=jct_abc"));

        // 2. Save zhihu cookies to the SAME file -> MUST NOT overwrite bilibili!
        save_platform_cookies_to_file(
            &cookie_file,
            "zhihu",
            "d_c0=\"zhihu_dc0_val\"; z_c0=\"zhihu_zc0_val\"",
        )
        .unwrap();

        let content2 = std::fs::read_to_string(&cookie_file).unwrap();
        assert!(content2.contains("bilibili: SESSDATA=bili_token_123; bili_jct=jct_abc"));
        assert!(content2.contains("zhihu: d_c0=\"zhihu_dc0_val\"; z_c0=\"zhihu_zc0_val\""));

        // 3. Update bilibili cookies in the SAME file -> zhihu MUST be preserved!
        save_platform_cookies_to_file(
            &cookie_file,
            "bilibili",
            "SESSDATA=bili_token_UPDATED; bili_jct=jct_new",
        )
        .unwrap();

        let content3 = std::fs::read_to_string(&cookie_file).unwrap();
        assert!(content3.contains("bilibili: SESSDATA=bili_token_UPDATED; bili_jct=jct_new"));
        assert!(content3.contains("zhihu: d_c0=\"zhihu_dc0_val\"; z_c0=\"zhihu_zc0_val\""));
        assert!(!content3.contains("bili_token_123"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_cookie_for_url_and_platform_isolation() {
        let multi_content = r#"
            # PageClaw Credentials
            bilibili: SESSDATA=bili_secret; bili_jct=bili_csrf
            zhihu: d_c0="zhihu_device"; z_c0="zhihu_auth"
            wechat: wx_token=wx_secret_999
        "#;

        // Bilibili URL should only receive bilibili cookies
        let bili_cookie = parse_cookie_content_for_platform(multi_content, Some("bilibili"));
        assert!(bili_cookie.contains("SESSDATA=bili_secret"));
        assert!(bili_cookie.contains("bili_jct=bili_csrf"));
        assert!(!bili_cookie.contains("zhihu_device"));
        assert!(!bili_cookie.contains("wx_secret_999"));

        // Zhihu URL should only receive zhihu cookies
        let zhihu_cookie = parse_cookie_content_for_platform(multi_content, Some("zhihu"));
        assert!(zhihu_cookie.contains("d_c0=\"zhihu_device\""));
        assert!(zhihu_cookie.contains("z_c0=\"zhihu_auth\""));
        assert!(!zhihu_cookie.contains("bili_secret"));
        assert!(!zhihu_cookie.contains("wx_secret_999"));

        // Unknown site should not receive sensitive platform-specific credentials
        let unknown_cookie = parse_cookie_content_for_platform(multi_content, Some("unknown_site"));
        assert!(unknown_cookie.is_empty());
    }

    #[test]
    fn test_ini_section_cookie_parsing() {
        let ini_content = r#"
            [bilibili]
            SESSDATA=sec_123; bili_jct=csrf_456

            [zhihu]
            d_c0="dc0_ini"
        "#;

        let bili_cookie = parse_cookie_content_for_platform(ini_content, Some("bilibili"));
        assert_eq!(bili_cookie, "SESSDATA=sec_123; bili_jct=csrf_456");

        let zhihu_cookie = parse_cookie_content_for_platform(ini_content, Some("zhihu"));
        assert_eq!(zhihu_cookie, "d_c0=\"dc0_ini\"");
    }

    #[test]
    fn test_is_private_ip_comprehensive() {
        use std::net::IpAddr;

        // IPv4 loopback & private
        assert!(is_private_ip("127.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("127.1.2.3".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("10.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("172.16.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("192.168.1.1".parse::<IpAddr>().unwrap()));

        // Cloud metadata & link-local
        assert!(is_private_ip("169.254.169.254".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("169.254.0.1".parse::<IpAddr>().unwrap()));

        // CGNAT (100.64.0.0/10)
        assert!(is_private_ip("100.64.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("100.127.255.255".parse::<IpAddr>().unwrap()));
        assert!(!is_private_ip("100.128.0.1".parse::<IpAddr>().unwrap()));

        // Reserved / IETF
        assert!(is_private_ip("0.0.0.0".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("192.0.2.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("198.51.100.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("203.0.113.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("240.0.0.1".parse::<IpAddr>().unwrap()));

        // Public IPs should NOT be marked as private
        assert!(!is_private_ip("1.1.1.1".parse::<IpAddr>().unwrap()));
        assert!(!is_private_ip("8.8.8.8".parse::<IpAddr>().unwrap()));
        assert!(!is_private_ip("104.16.132.229".parse::<IpAddr>().unwrap()));

        // IPv6 loopback, unspecified, ULA & link-local
        assert!(is_private_ip("::1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("::".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("fc00::1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip("fe80::1".parse::<IpAddr>().unwrap()));

        // IPv4-mapped IPv6 must be detected and inspected recursively
        assert!(is_private_ip("::ffff:127.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(is_private_ip(
            "::ffff:169.254.169.254".parse::<IpAddr>().unwrap()
        ));
        assert!(is_private_ip(
            "::ffff:192.168.1.10".parse::<IpAddr>().unwrap()
        ));
        assert!(!is_private_ip("::ffff:8.8.8.8".parse::<IpAddr>().unwrap()));
    }

    #[tokio::test]
    async fn test_validate_ssrf_rejects_localhost_and_metadata() {
        let fetcher = Fetcher::new(FetchOptions::default()).unwrap();

        // Localhost hostname check
        let localhost_url = Url::parse("http://localhost:8080/secret").unwrap();
        assert!(fetcher.validate_ssrf(&localhost_url).await.is_err());

        let dot_local_url = Url::parse("http://my-service.local/api").unwrap();
        assert!(fetcher.validate_ssrf(&dot_local_url).await.is_err());

        // Cloud metadata IP literal check
        let metadata_url = Url::parse("http://169.254.169.254/latest/meta-data").unwrap();
        assert!(fetcher.validate_ssrf(&metadata_url).await.is_err());

        // IPv6 loopback
        let ipv6_loopback = Url::parse("http://[::1]:3000/").unwrap();
        assert!(fetcher.validate_ssrf(&ipv6_loopback).await.is_err());

        // Allow private addresses override
        let allow_options = FetchOptions {
            allow_private_addresses: true,
            ..Default::default()
        };
        let allow_fetcher = Fetcher::new(allow_options).unwrap();
        assert!(allow_fetcher.validate_ssrf(&localhost_url).await.is_ok());
        assert!(allow_fetcher.validate_ssrf(&metadata_url).await.is_ok());
    }

    #[tokio::test]
    async fn test_ssrf_redirect_to_private_ip_blocked() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        // Start a mock server that returns HTTP 302 redirecting to cloud metadata
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let response = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data\r\nContent-Length: 0\r\n\r\n";
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });

        // Fetcher that allows initial local connection to the mock server, but strict SSRF is tested on redirect
        // We test that following redirect to 169.254.169.254 is intercepted by validate_ssrf
        let fetcher = Fetcher::new(FetchOptions::default()).unwrap();
        let target_url = format!("http://127.0.0.1:{port}/redirect");

        // Without allow_private_addresses, initial connection to 127.0.0.1 is blocked at first hop
        let res1 = fetcher.fetch(&target_url).await;
        assert!(res1.is_err());
        match res1 {
            Err(PageclawError::SsrfBlocked(msg)) => {
                assert!(msg.contains("restricted"));
            }
            _ => panic!("Expected SsrfBlocked error on first hop"),
        }
    }
}
