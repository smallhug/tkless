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
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub not_modified: bool,
}

#[derive(Clone)]
pub struct Fetcher {
    client: reqwest::Client,
    options: FetchOptions,
}

impl Fetcher {
    /// Create a new fetcher instance with custom options.
    pub fn new(options: FetchOptions) -> Result<Self, PageclawError> {
        let mut headers = HeaderMap::new();

        let default_desktop_ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 PageClaw/0.1";
        let default_mobile_ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1 PageClaw/0.1";

        let ua = match options.user_agent.as_deref() {
            Some(custom) => custom,
            None => {
                if options.mobile {
                    default_mobile_ua
                } else {
                    default_desktop_ua
                }
            }
        };
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

        for (key, val) in &options.headers {
            if let (Ok(hname), Ok(hval)) = (HeaderName::from_str(key), HeaderValue::from_str(val)) {
                headers.insert(hname, hval);
            }
        }

        // NOTE: We do NOT insert Cookie into client default_headers to prevent accidental
        // credential leakage to third-party hosts during searches, crawls, or cross-origin redirects.
        // Instead, cookies are strictly injected per-request based on the target URL's domain scope.

        let mut client_builder = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(options.timeout_secs))
            .connect_timeout(Duration::from_secs(options.connect_timeout_secs))
            .redirect(reqwest::redirect::Policy::none());

        if let Some(ref proxy_url) = options.proxy {
            let proxy = reqwest::Proxy::all(proxy_url)?;
            client_builder = client_builder.proxy(proxy);
        }

        let client = client_builder.build()?;

        Ok(Self { client, options })
    }

    /// Access the underlying reqwest HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Access the underlying fetch options.
    pub fn options(&self) -> &FetchOptions {
        &self.options
    }

    /// Builds a GET request for target URL with domain-scoped cookies automatically attached.
    /// Strictly protects against cross-host credential leaks.
    pub fn get(&self, url: &Url) -> reqwest::RequestBuilder {
        let mut builder = self.client.get(url.clone());
        if let Some(cookie_str) = self.resolve_cookie_for_url(url) {
            if !cookie_str.trim().is_empty() {
                if let Ok(hval) = HeaderValue::from_str(cookie_str.trim()) {
                    builder = builder.header(COOKIE, hval);
                }
            }
        }
        builder
    }

    /// Builds a GET request for a URL string with domain-scoped cookies automatically attached.
    pub fn get_str(&self, url_str: &str) -> reqwest::RequestBuilder {
        if let Ok(url) = Url::parse(url_str) {
            self.get(&url)
        } else {
            self.client.get(url_str)
        }
    }

    /// Resolves the cookie header string scoped strictly to the target URL's domain.
    /// Prevents cross-host credential leakage by only returning credentials matching the platform host.
    pub fn resolve_cookie_for_url(&self, url: &Url) -> Option<String> {
        let mut resolved_file_cookie: Option<String> = None;

        // 1. Resolve from cookie_file using strict URL domain boundary matching
        if let Some(ref cf) = self.options.cookie_file {
            let path = Path::new(cf);
            if let Ok(c) = load_cookie_from_file_for_url(path, Some(url.as_str())) {
                if !c.trim().is_empty() {
                    resolved_file_cookie = Some(c);
                }
            }
        }

        // 2. Resolve from CLI explicit cookie
        let resolved_cli_cookie = if let Some(ref cli_cookie) = self.options.cookie {
            if let Some((p, rest)) = cli_cookie.split_once(':') {
                let p_clean = p.trim().to_lowercase();
                if let Some(target_p) = detect_platform_from_url(url.as_str()) {
                    if p_clean == target_p {
                        Some(rest.trim().to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                // Security Hardening: If it's an unprefixed cookie, authorize based on platform scope or apex domain.
                // Ensures all legitimate API subdomains of the same platform (e.g. api.bilibili.com for www.bilibili.com)
                // receive the credentials, while strictly stripping cookies on cross-origin redirects and third-party search hits.
                if let Some(ref auth_host) = self.options.initial_target_host {
                    let req_host = url.host_str().unwrap_or("").to_lowercase();
                    let current_platform = detect_platform_from_host(&req_host);
                    let initial_platform = detect_platform_from_host(auth_host);

                    let is_authorized = match (current_platform, initial_platform) {
                        (Some(cp), Some(ip)) if cp == ip => true,
                        _ => {
                            let apex_auth = extract_apex_domain(auth_host);
                            is_subdomain_or_exact(&req_host, apex_auth)
                        }
                    };

                    if is_authorized {
                        Some(cli_cookie.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        } else {
            None
        };

        match (resolved_file_cookie, resolved_cli_cookie) {
            (Some(fc), Some(cc)) => Some(merge_cookies(&fc, &cc)),
            (Some(fc), None) => Some(fc),
            (None, Some(cc)) => Some(cc),
            (None, None) => None,
        }
    }

    /// Fetch a webpage by URL with retry backoff, full-chain SSRF validation across redirects,
    /// and decode its content into a clean string.
    pub async fn fetch(&self, target_url: &str) -> Result<FetchedResponse, PageclawError> {
        self.fetch_with_revalidation(target_url, None, None).await
    }

    /// Fetches a webpage with conditional HTTP revalidation headers (ETag / If-None-Match, Last-Modified / If-Modified-Since).
    /// If the remote server responds with 304 Not Modified, returns zero-byte body payload with `not_modified = true`.
    pub async fn fetch_with_revalidation(
        &self,
        target_url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<FetchedResponse, PageclawError> {
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
                let mut req_builder = self.get(&current_url);
                // Attach conditional revalidation headers on the first request hop
                if redirect_count == 0 {
                    if let Some(et) = etag {
                        req_builder = req_builder.header(reqwest::header::IF_NONE_MATCH, et.trim());
                    }
                    if let Some(lm) = last_modified {
                        req_builder =
                            req_builder.header(reqwest::header::IF_MODIFIED_SINCE, lm.trim());
                    }
                }

                let send_fut = req_builder.send();
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

            // Check if server returned 304 Not Modified
            if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
                debug!(url = %current_url, "Server returned HTTP 304 Not Modified");
                let new_etag = resp
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
                    .or_else(|| etag.map(|s| s.to_string()));
                let new_lm = resp
                    .headers()
                    .get(reqwest::header::LAST_MODIFIED)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
                    .or_else(|| last_modified.map(|s| s.to_string()));

                return Ok(FetchedResponse {
                    requested_url: target_url.to_string(),
                    final_url: current_url.to_string(),
                    status: 304,
                    html: String::new(),
                    truncated: false,
                    etag: new_etag,
                    last_modified: new_lm,
                    not_modified: true,
                });
            }

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

        let etag_header = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let lm_header = response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
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
            etag: etag_header,
            last_modified: lm_header,
            not_modified: false,
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

/// Extracts the registrable apex domain from a host (e.g. `www.bilibili.com` -> `bilibili.com`).
pub fn extract_apex_domain(host: &str) -> &str {
    let clean = host.trim().trim_matches('.');
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() <= 2 {
        return clean;
    }

    // Handle common double TLDs like .co.uk, .com.cn, .net.cn, .org.cn
    let last = parts[parts.len() - 1];
    let second_last = parts[parts.len() - 2];
    if (second_last == "com"
        || second_last == "co"
        || second_last == "net"
        || second_last == "org"
        || second_last == "edu")
        && (last == "cn" || last == "uk" || last == "jp" || last == "au")
        && parts.len() >= 3
    {
        let offset = parts[..parts.len() - 3]
            .iter()
            .map(|p| p.len() + 1)
            .sum::<usize>();
        return &clean[offset..];
    }

    let offset = parts[..parts.len() - 2]
        .iter()
        .map(|p| p.len() + 1)
        .sum::<usize>();
    &clean[offset..]
}

/// Detects platform identifier from a host string (e.g. `api.bilibili.com` -> `bilibili`).
pub fn detect_platform_from_host(host: &str) -> Option<&'static str> {
    let host_lower = host.trim().to_lowercase();

    if is_subdomain_or_exact(&host_lower, "bilibili.com")
        || is_subdomain_or_exact(&host_lower, "b23.tv")
        || is_subdomain_or_exact(&host_lower, "bilivideo.com")
    {
        Some("bilibili")
    } else if is_subdomain_or_exact(&host_lower, "zhihu.com") {
        Some("zhihu")
    } else if is_subdomain_or_exact(&host_lower, "qq.com")
        || is_subdomain_or_exact(&host_lower, "weixin.qq.com")
    {
        Some("wechat")
    } else if is_subdomain_or_exact(&host_lower, "weibo.com")
        || is_subdomain_or_exact(&host_lower, "weibo.cn")
    {
        Some("weibo")
    } else if is_subdomain_or_exact(&host_lower, "xiaohongshu.com")
        || is_subdomain_or_exact(&host_lower, "xhslink.com")
    {
        Some("xiaohongshu")
    } else if is_subdomain_or_exact(&host_lower, "juejin.cn") {
        Some("juejin")
    } else if is_subdomain_or_exact(&host_lower, "github.com") {
        Some("github")
    } else if is_subdomain_or_exact(&host_lower, "csdn.net") {
        Some("csdn")
    } else if is_subdomain_or_exact(&host_lower, "medium.com") {
        Some("medium")
    } else if is_subdomain_or_exact(&host_lower, "substack.com") {
        Some("substack")
    } else if is_subdomain_or_exact(&host_lower, "arxiv.org") {
        Some("arxiv")
    } else if is_subdomain_or_exact(&host_lower, "huggingface.co")
        || is_subdomain_or_exact(&host_lower, "hf.co")
    {
        Some("huggingface")
    } else if is_subdomain_or_exact(&host_lower, "wikipedia.org") {
        Some("wikipedia")
    } else if is_subdomain_or_exact(&host_lower, "reddit.com")
        || is_subdomain_or_exact(&host_lower, "redd.it")
    {
        Some("reddit")
    } else if is_subdomain_or_exact(&host_lower, "stackoverflow.com")
        || is_subdomain_or_exact(&host_lower, "stackexchange.com")
        || is_subdomain_or_exact(&host_lower, "superuser.com")
        || is_subdomain_or_exact(&host_lower, "serverfault.com")
        || is_subdomain_or_exact(&host_lower, "askubuntu.com")
    {
        Some("stackoverflow")
    } else if is_subdomain_or_exact(&host_lower, "v2ex.com") {
        Some("v2ex")
    } else if is_subdomain_or_exact(&host_lower, "baike.baidu.com") {
        Some("baike")
    } else {
        None
    }
}

/// Detects platform identifier from a URL string (e.g. "bilibili", "zhihu", "wechat").
/// Uses strict domain boundary matching to prevent cross-site cookie leaks to spoofed domains.
pub fn detect_platform_from_url(url_str: &str) -> Option<&'static str> {
    let parsed = Url::parse(url_str).ok()?;
    let host = parsed.host_str()?;
    detect_platform_from_host(host)
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

    // Security Hardening (Finding 5 Fix):
    // If no target platform requested, strictly return EMPTY.
    // Never leak platform credentials (even if single platform) to unknown or third-party search result hosts.
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
        assert_eq!(
            detect_platform_from_url("https://blog.csdn.net/user/article/details/123"),
            Some("csdn")
        );
        assert_eq!(
            detect_platform_from_url("https://medium.com/@author/my-article"),
            Some("medium")
        );
        assert_eq!(
            detect_platform_from_url("https://newsletter.substack.com/p/my-post"),
            Some("substack")
        );
        assert_eq!(
            detect_platform_from_url("https://arxiv.org/abs/2401.12345"),
            Some("arxiv")
        );
        assert_eq!(
            detect_platform_from_url("https://huggingface.co/deepseek-ai/DeepSeek-R1"),
            Some("huggingface")
        );
        assert_eq!(
            detect_platform_from_url("https://hf.co/meta-llama/Llama-3.1-8B"),
            Some("huggingface")
        );
        assert_eq!(
            detect_platform_from_url("https://en.wikipedia.org/wiki/Rust_(programming_language)"),
            Some("wikipedia")
        );
        assert_eq!(
            detect_platform_from_url("https://zh.wikipedia.org/wiki/AI"),
            Some("wikipedia")
        );
        assert_eq!(
            detect_platform_from_url("https://www.reddit.com/r/rust/comments/123/rust_news/"),
            Some("reddit")
        );
        assert_eq!(
            detect_platform_from_url("https://redd.it/123"),
            Some("reddit")
        );
        assert_eq!(
            detect_platform_from_url(
                "https://stackoverflow.com/questions/21747136/how-to-read-a-file"
            ),
            Some("stackoverflow")
        );
        assert_eq!(
            detect_platform_from_url("https://serverfault.com/questions/123"),
            Some("stackoverflow")
        );
        assert_eq!(
            detect_platform_from_url("https://v2ex.com/t/1098765"),
            Some("v2ex")
        );
        assert_eq!(
            detect_platform_from_url("https://www.v2ex.com/t/123"),
            Some("v2ex")
        );
        assert_eq!(
            detect_platform_from_url("https://baike.baidu.com/item/Rust"),
            Some("baike")
        );

        // Security check: spoofed / typosquatted domains MUST NOT leak cookies!
        assert_eq!(
            detect_platform_from_url("https://evilzhihu.com/p/123"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://evilcsdn.net/article/123"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://evilmedium.com/story"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://evilsubstack.com/post"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://evilarxiv.org/abs/2401.12345"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://evilhuggingface.co/model"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://evilwikipedia.org/wiki/Fake"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://evilreddit.com/r/rust/comments/123"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://evilstackoverflow.com/questions/123"),
            None
        );
        assert_eq!(detect_platform_from_url("https://evilv2ex.com/t/123"), None);
        assert_eq!(
            detect_platform_from_url("https://evilbaike.baidu.com/item/1"),
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

        // Public IPs and Fake-IP proxy addresses should NOT be marked as private
        assert!(!is_private_ip("1.1.1.1".parse::<IpAddr>().unwrap()));
        assert!(!is_private_ip("8.8.8.8".parse::<IpAddr>().unwrap()));
        assert!(!is_private_ip("198.18.0.1".parse::<IpAddr>().unwrap()));
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

    #[test]
    fn test_finding_5_no_credential_leak_to_third_party() {
        // File containing ONLY Zhihu credentials
        let single_platform_content = "zhihu: d_c0=\"zhihu_secret_token\"; z_c0=\"zhihu_auth\"";

        // 1. Third-party URLs (e.g. search result items) MUST NOT receive Zhihu credentials!
        let third_party_cookie = parse_cookie_content_for_platform(
            single_platform_content,
            detect_platform_from_url("https://rust-lang.org/"),
        );
        assert!(third_party_cookie.is_empty());

        let attacker_cookie = parse_cookie_content_for_platform(
            single_platform_content,
            detect_platform_from_url("https://attacker.example/blog"),
        );
        assert!(attacker_cookie.is_empty());

        // 2. Unscoped query (e.g. CLI search command without target URL) MUST NOT return any credentials!
        let unscoped_cookie = parse_cookie_content_for_platform(single_platform_content, None);
        assert!(unscoped_cookie.is_empty());

        // 3. Legitimate Zhihu URL strictly receives Zhihu credentials
        let zhihu_cookie = parse_cookie_content_for_platform(
            single_platform_content,
            detect_platform_from_url("https://zhuanlan.zhihu.com/p/123456"),
        );
        assert_eq!(
            zhihu_cookie,
            "d_c0=\"zhihu_secret_token\"; z_c0=\"zhihu_auth\""
        );
    }

    #[test]
    fn test_fetcher_resolve_cookie_for_url_host_scoped() {
        let temp_dir = std::env::temp_dir().join(format!(
            "pageclaw_f5_{}",
            std::time::SystemTime::now().elapsed().unwrap().as_nanos()
        ));
        let cookie_file = temp_dir.join("cookies.txt");
        save_platform_cookies_to_file(&cookie_file, "zhihu", "d_c0=\"zh_sec\"").unwrap();

        let options = FetchOptions {
            cookie_file: Some(cookie_file.to_string_lossy().to_string()),
            ..Default::default()
        };
        let fetcher = Fetcher::new(options).unwrap();

        // Legitimate Zhihu host receives scoped cookie
        let zhihu_url = Url::parse("https://www.zhihu.com/question/1").unwrap();
        assert_eq!(
            fetcher.resolve_cookie_for_url(&zhihu_url),
            Some("d_c0=\"zh_sec\"".to_string())
        );

        // Third-party host (e.g. from search result) strictly receives NONE
        let third_party_url = Url::parse("https://rust-lang.org/").unwrap();
        assert_eq!(fetcher.resolve_cookie_for_url(&third_party_url), None);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_extract_apex_domain() {
        assert_eq!(extract_apex_domain("www.bilibili.com"), "bilibili.com");
        assert_eq!(extract_apex_domain("api.bilibili.com"), "bilibili.com");
        assert_eq!(extract_apex_domain("zhuanlan.zhihu.com"), "zhihu.com");
        assert_eq!(extract_apex_domain("bilibili.com"), "bilibili.com");
        assert_eq!(extract_apex_domain("docs.rs"), "docs.rs");
        assert_eq!(extract_apex_domain("sub.example.com.cn"), "example.com.cn");
    }

    #[test]
    fn test_unprefixed_cli_cookie_scoped_to_initial_host() {
        // 1. Real-world CLI scenario: user enters "https://www.bilibili.com/video/BV...",
        // resulting in exact initial_target_host = Some("www.bilibili.com").
        // Must authorize both the page and sibling API/CDN subdomains of the same platform!
        let options = FetchOptions {
            cookie: Some("SESSDATA=bili_val; bili_jct=csrf_val".to_string()),
            initial_target_host: Some("www.bilibili.com".to_string()),
            ..Default::default()
        };
        let fetcher = Fetcher::new(options).unwrap();

        // Exact host receives the cookie
        let www_bili_url = Url::parse("https://www.bilibili.com/video/BV1xx").unwrap();
        assert_eq!(
            fetcher.resolve_cookie_for_url(&www_bili_url),
            Some("SESSDATA=bili_val; bili_jct=csrf_val".to_string())
        );

        // Platform API subdomain (api.bilibili.com) MUST ALSO receive the cookie!
        let api_bili_url = Url::parse("https://api.bilibili.com/x/web-interface/view").unwrap();
        assert_eq!(
            fetcher.resolve_cookie_for_url(&api_bili_url),
            Some("SESSDATA=bili_val; bili_jct=csrf_val".to_string())
        );

        // Platform video stream CDN (bilivideo.com) receives the cookie
        let cdn_bili_url = Url::parse("https://upos-sz-mirror08c.bilivideo.com/video.mp4").unwrap();
        assert_eq!(
            fetcher.resolve_cookie_for_url(&cdn_bili_url),
            Some("SESSDATA=bili_val; bili_jct=csrf_val".to_string())
        );

        // Third-party search result item or cross-origin redirect MUST STRIP the cookie!
        let third_party = Url::parse("https://rust-lang.org/").unwrap();
        assert_eq!(fetcher.resolve_cookie_for_url(&third_party), None);

        let attacker = Url::parse("https://attacker.example/steal").unwrap();
        assert_eq!(fetcher.resolve_cookie_for_url(&attacker), None);

        // 2. Real-world Zhihu CLI scenario: initial target is "zhuanlan.zhihu.com"
        // Must authorize "www.zhihu.com/api/v4/..." but strip on external sites!
        let zhihu_options = FetchOptions {
            cookie: Some("d_c0=zh_sec".to_string()),
            initial_target_host: Some("zhuanlan.zhihu.com".to_string()),
            ..Default::default()
        };
        let zhihu_fetcher = Fetcher::new(zhihu_options).unwrap();

        let zhihu_api_url = Url::parse("https://www.zhihu.com/api/v4/answers/123").unwrap();
        assert_eq!(
            zhihu_fetcher.resolve_cookie_for_url(&zhihu_api_url),
            Some("d_c0=zh_sec".to_string())
        );
        assert_eq!(zhihu_fetcher.resolve_cookie_for_url(&third_party), None);

        // 3. Unprefixed CLI cookie WITHOUT initial_target_host (e.g. search --fetch-content command)
        let search_options = FetchOptions {
            cookie: Some("SESSDATA=bili_val".to_string()),
            initial_target_host: None,
            ..Default::default()
        };
        let search_fetcher = Fetcher::new(search_options).unwrap();

        // Any arbitrary host MUST NOT receive the unprefixed cookie!
        assert_eq!(search_fetcher.resolve_cookie_for_url(&api_bili_url), None);
        assert_eq!(search_fetcher.resolve_cookie_for_url(&third_party), None);
        assert_eq!(search_fetcher.resolve_cookie_for_url(&attacker), None);
    }
}
