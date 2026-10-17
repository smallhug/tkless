use crate::error::TklessError;
use qrcode::render::svg;
use qrcode::QrCode;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Response payload from Bilibili qrcode generate API.
#[derive(Debug, Clone, Deserialize)]
pub struct BiliQrGenerateData {
    pub url: String,
    pub qrcode_key: String,
}

#[derive(Deserialize)]
struct BiliApiResponse<T> {
    code: i32,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

#[derive(Deserialize)]
struct BiliPollData {
    #[serde(default)]
    url: String,
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: String,
}

/// Status of the Bilibili QR code polling check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrPollStatus {
    /// QR code has not been scanned yet.
    NotScanned,
    /// QR code has been scanned, waiting for user confirmation on mobile app.
    Scanned,
    /// QR code has expired.
    Expired,
    /// Login successfully confirmed by user; contains complete Cookie header string.
    Success { cookies: String },
}

/// Configuration options for the interactive login flow.
#[derive(Debug, Clone)]
pub struct LoginOptions {
    /// Destination file path to save extracted cookies.
    pub output_path: PathBuf,
    /// Whether to disable automatic browser launching.
    pub no_browser: bool,
    /// Overall login timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self {
            output_path: PathBuf::from("./cookies.txt"),
            no_browser: false,
            timeout_secs: 180,
        }
    }
}

/// Requests a new QR code URL and key from Bilibili passport API.
pub async fn generate_qrcode(client: &reqwest::Client) -> Result<BiliQrGenerateData, TklessError> {
    let api_url = "https://passport.bilibili.com/x/passport-login/web/qrcode/generate";
    let resp: BiliApiResponse<BiliQrGenerateData> = client
        .get(api_url)
        .header("Referer", "https://passport.bilibili.com/")
        .send()
        .await?
        .json()
        .await?;

    if resp.code != 0 {
        return Err(TklessError::AdapterError {
            platform: "bilibili",
            message: format!(
                "Failed to generate QR code (code={}): {}",
                resp.code, resp.message
            ),
        });
    }

    resp.data.ok_or_else(|| TklessError::AdapterError {
        platform: "bilibili",
        message: "Empty QR code generate payload received".to_string(),
    })
}

/// Polls Bilibili passport API for QR code scan status.
pub async fn poll_qrcode_status(
    client: &reqwest::Client,
    qrcode_key: &str,
) -> Result<QrPollStatus, TklessError> {
    let api_url = format!(
        "https://passport.bilibili.com/x/passport-login/web/qrcode/poll?qrcode_key={qrcode_key}"
    );

    let resp = client
        .get(&api_url)
        .header("Referer", "https://passport.bilibili.com/")
        .send()
        .await?;

    // Capture Set-Cookie headers in case of success
    let set_cookie_string = extract_set_cookie_headers(resp.headers());

    let poll_resp: BiliApiResponse<BiliPollData> = resp.json().await?;

    if poll_resp.code != 0 {
        return Err(TklessError::AdapterError {
            platform: "bilibili",
            message: format!(
                "QR poll API returned error (code={}): {}",
                poll_resp.code, poll_resp.message
            ),
        });
    }

    let data = poll_resp.data.ok_or_else(|| TklessError::AdapterError {
        platform: "bilibili",
        message: "Empty QR code poll payload".to_string(),
    })?;

    match data.code {
        86101 => Ok(QrPollStatus::NotScanned),
        86090 => Ok(QrPollStatus::Scanned),
        86038 => Ok(QrPollStatus::Expired),
        0 => {
            // Success! Combine cookies from Set-Cookie headers and data.url query params
            let cross_domain_cookies = extract_cookies_from_url(&data.url);
            let final_cookies = match (
                set_cookie_string.is_empty(),
                cross_domain_cookies.is_empty(),
            ) {
                (false, false) => {
                    crate::fetcher::merge_cookies(&cross_domain_cookies, &set_cookie_string)
                }
                (false, true) => set_cookie_string,
                (true, false) => cross_domain_cookies,
                (true, true) => String::new(),
            };

            if final_cookies.is_empty() {
                warn!("Login poll succeeded but no cookies were extracted");
            }

            Ok(QrPollStatus::Success {
                cookies: final_cookies,
            })
        }
        other => {
            warn!(code = other, message = %data.message, "Unknown QR code poll status");
            Ok(QrPollStatus::NotScanned)
        }
    }
}

/// Renders a QR code as an SVG string for inline HTML embedding.
pub fn render_svg_qrcode(url: &str) -> Result<String, TklessError> {
    let code = QrCode::new(url.as_bytes())
        .map_err(|e| TklessError::InvalidArgument(format!("Failed to generate QR code: {e}")))?;
    let svg_str = code.render::<svg::Color>().min_dimensions(260, 260).build();
    Ok(svg_str)
}

/// Renders a QR code for terminal console display.
pub fn render_terminal_qrcode(url: &str) -> Result<String, TklessError> {
    let code = QrCode::new(url.as_bytes())
        .map_err(|e| TklessError::InvalidArgument(format!("Failed to generate QR code: {e}")))?;
    let rendered = code
        .render::<char>()
        .quiet_zone(true)
        .module_dimensions(2, 1)
        .build();
    Ok(rendered)
}

/// Launches the default system browser opening the given URL.
pub fn open_browser(url: &str) -> Result<(), TklessError> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
            .map_err(TklessError::Io)?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(TklessError::Io)?;
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(TklessError::Io)?;
    }

    Ok(())
}

/// Starts a lightweight local HTTP server displaying the login QR code and live status.
pub async fn start_login_server(
    qr_svg: String,
    status_rx: watch::Receiver<String>,
) -> Result<(u16, tokio::task::JoinHandle<()>), TklessError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(TklessError::Io)?;
    let port = listener.local_addr().map_err(TklessError::Io)?.port();

    let server_handle = tokio::spawn(async move {
        let html_content = format!(
            r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>TkLess - 哔哩哔哩登录</title>
  <style>
    * {{ box-sizing: border-box; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
      margin: 0; padding: 20px;
      display: flex; align-items: center; justify-content: center;
      min-height: 100vh; background: #f1f2f3;
    }}
    .card {{
      background: white; border-radius: 16px;
      padding: 36px 32px; width: 100%; max-width: 380px;
      box-shadow: 0 8px 30px rgba(0,0,0,0.06); text-align: center;
    }}
    .brand {{ display: inline-flex; align-items: center; gap: 8px; margin-bottom: 12px; }}
    .brand h1 {{ font-size: 20px; margin: 0; color: #18191c; font-weight: 600; }}
    .subtitle {{ color: #61666d; font-size: 13px; margin: 0 0 24px; }}
    .qr-container {{
      background: #fafafa; border: 1px solid #e3e5e7;
      border-radius: 12px; padding: 16px; display: inline-block;
      margin-bottom: 20px;
    }}
    .qr-container svg {{ display: block; max-width: 100%; height: auto; }}
    .status-badge {{
      display: inline-block; padding: 6px 16px; border-radius: 20px;
      font-size: 13px; font-weight: 500;
      background: #e8f4fc; color: #00aeec; margin-bottom: 12px;
      transition: all 0.3s ease;
    }}
    .status-badge.success {{ background: #eaf8ec; color: #16a34a; }}
    .status-badge.scanned {{ background: #fef3c7; color: #d97706; }}
    .status-badge.expired {{ background: #fee2e2; color: #dc2626; }}
    .tip {{ font-size: 12px; color: #9499a0; line-height: 1.5; margin: 0; }}
  </style>
</head>
<body>
  <div class="card">
    <div class="brand">
      <h1>TkLess</h1>
    </div>
    <p class="subtitle">请使用 <strong>哔哩哔哩 手机客户端</strong> 扫码登录</p>
    <div class="qr-container">
      {qr_svg}
    </div>
    <div>
      <div id="status" class="status-badge">等待扫码...</div>
    </div>
    <p class="tip">扫码并确认后，凭据将自动保存至本地文件。<br>登录完成后即可关闭此窗口。</p>
  </div>
  <script>
    async function checkStatus() {{
      try {{
        const res = await fetch('/status');
        const data = await res.json();
        const badge = document.getElementById('status');
        if (data.status === 'scanned') {{
          badge.textContent = '已扫码，请在手机上点击确认';
          badge.className = 'status-badge scanned';
        }} else if (data.status === 'success') {{
          badge.textContent = '登录成功！凭据已保存';
          badge.className = 'status-badge success';
        }} else if (data.status === 'expired') {{
          badge.textContent = '二维码已失效，请重新运行命令';
          badge.className = 'status-badge expired';
        }}
        if (data.status !== 'success' && data.status !== 'expired') {{
          setTimeout(checkStatus, 1000);
        }}
      }} catch (e) {{
        setTimeout(checkStatus, 1500);
      }}
    }}
    setTimeout(checkStatus, 1000);
  </script>
</body>
</html>"#
        );

        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            let mut buf = [0u8; 1024];
            let n = match socket.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => continue,
            };

            let req_str = String::from_utf8_lossy(&buf[..n]);
            let first_line = req_str.lines().next().unwrap_or("");

            if first_line.starts_with("GET /status") {
                let current_status = status_rx.borrow().clone();
                let json_body = format!("{{\"status\":\"{}\"}}", current_status);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    json_body.len(),
                    json_body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            } else {
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    html_content.len(),
                    html_content
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        }
    });

    Ok((port, server_handle))
}

/// Executes the full interactive login flow:
/// 1. Requests QR code and key from Bilibili API
/// 2. Starts local HTTP server and displays QR code in browser and console
/// 3. Polls scan status until success or timeout
/// 4. Saves extracted cookies to destination path
pub async fn execute_login_flow(
    client: Option<&reqwest::Client>,
    options: LoginOptions,
) -> Result<String, TklessError> {
    info!("Initializing Bilibili interactive QR code login...");

    let default_client;
    let client_ref = match client {
        Some(c) => c,
        None => {
            default_client = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .connect_timeout(Duration::from_secs(10))
                .build()?;
            &default_client
        }
    };

    // 1. Generate QR code
    let qr_data = generate_qrcode(client_ref).await?;
    debug!(qrcode_key = %qr_data.qrcode_key, "QR code generated successfully");

    // 2. Render SVG representation for web server
    let qr_svg = render_svg_qrcode(&qr_data.url)?;

    // 3. Start local web server
    let (status_tx, status_rx) = watch::channel("waiting".to_string());
    let (port, server_handle) = start_login_server(qr_svg, status_rx).await?;
    let local_url = format!("http://127.0.0.1:{port}");

    // 4. Output guidance to terminal
    if options.no_browser {
        eprintln!("[TkLess] 请使用 哔哩哔哩 手机客户端扫描下方二维码授权登录：");
        let terminal_qr = render_terminal_qrcode(&qr_data.url).unwrap_or_default();
        if !terminal_qr.is_empty() {
            eprintln!("{terminal_qr}");
        }
        eprintln!("或在浏览器中打开: {local_url}");
    } else {
        eprintln!("打开浏览器授权登录bilibili：{local_url}");
    }

    // 5. Open browser if enabled
    if !options.no_browser {
        if let Err(e) = open_browser(&local_url) {
            warn!(error = %e, "Failed to launch default browser automatically; please open the URL manually");
        }
    }

    // 6. Polling loop with timeout
    let start_time = std::time::Instant::now();
    let poll_interval = Duration::from_millis(1500);
    let mut last_status = "waiting";

    let final_cookies = loop {
        if start_time.elapsed().as_secs() > options.timeout_secs {
            let _ = status_tx.send("expired".to_string());
            server_handle.abort();
            return Err(TklessError::AdapterError {
                platform: "bilibili",
                message: format!(
                    "Login timed out after {} seconds without confirmation",
                    options.timeout_secs
                ),
            });
        }

        tokio::time::sleep(poll_interval).await;

        match poll_qrcode_status(client_ref, &qr_data.qrcode_key).await {
            Ok(QrPollStatus::NotScanned) => {
                if last_status != "waiting" {
                    last_status = "waiting";
                    let _ = status_tx.send("waiting".to_string());
                }
            }
            Ok(QrPollStatus::Scanned) => {
                if last_status != "scanned" {
                    last_status = "scanned";
                    let _ = status_tx.send("scanned".to_string());
                    eprintln!("[TkLess] 已扫码，等待手机端点击确认登录...");
                }
            }
            Ok(QrPollStatus::Expired) => {
                let _ = status_tx.send("expired".to_string());
                server_handle.abort();
                return Err(TklessError::AdapterError {
                    platform: "bilibili",
                    message: "二维码已过期失效，请重新执行登录命令".to_string(),
                });
            }
            Ok(QrPollStatus::Success { cookies }) => {
                let _ = status_tx.send("success".to_string());
                eprintln!("[TkLess] 扫码确认成功，已获取用户登录凭证！");
                break cookies;
            }
            Err(e) => {
                debug!(error = %e, "Transient error while polling QR status, retrying...");
            }
        }
    };

    // 7. Save to destination file with platform prefix to avoid overwriting other websites
    crate::fetcher::save_platform_cookies_to_file(
        &options.output_path,
        "bilibili",
        &final_cookies,
    )?;
    eprintln!(
        "[TkLess] bilibili 凭据已成功保存至: {} (已添加 bilibili: 前缀区分，多网站互不覆盖)",
        options.output_path.display()
    );
    eprintln!("========================================================");
    eprintln!();

    // Give browser a short window to fetch /status and show green success badge before shutdown
    tokio::time::sleep(Duration::from_millis(600)).await;
    server_handle.abort();

    Ok(final_cookies)
}

/// Saves cookie header string to disk.
pub fn save_cookies_to_file(path: &Path, cookies: &str) -> Result<(), TklessError> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, cookies.trim()).map_err(TklessError::Io)
}

fn extract_set_cookie_headers(headers: &reqwest::header::HeaderMap) -> String {
    let mut pairs = Vec::new();
    for header in headers.get_all(reqwest::header::SET_COOKIE) {
        if let Ok(val) = header.to_str() {
            if let Some(first_part) = val.split(';').next() {
                if let Some((k, v)) = first_part.split_once('=') {
                    let k = k.trim();
                    let v = v.trim();
                    if !k.is_empty() && !v.is_empty() {
                        pairs.push(format!("{k}={v}"));
                    }
                }
            }
        }
    }
    pairs.join("; ")
}

fn extract_cookies_from_url(url_str: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url_str) {
        let mut pairs = Vec::new();
        for (k, v) in parsed.query_pairs() {
            if k == "SESSDATA"
                || k == "bili_jct"
                || k == "DedeUserID"
                || k == "DedeUserID__ckMd5"
                || k == "sid"
            {
                pairs.push(format!("{k}={v}"));
            }
        }
        pairs.join("; ")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_svg_qrcode() {
        let url = "https://passport.bilibili.com/h5-app/passport/login/scan?qrcode_key=test123456";
        let svg = render_svg_qrcode(url).expect("SVG QR code generation should succeed");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
    }

    #[test]
    fn test_render_terminal_qrcode() {
        let url = "https://example.com/login";
        let rendered = render_terminal_qrcode(url).expect("Terminal QR generation should succeed");
        assert!(!rendered.is_empty());
    }

    #[test]
    fn test_extract_cookies_from_url() {
        let url = "https://passport.biligame.com/crossDomain?DedeUserID=12345678&DedeUserID__ckMd5=abc1234&Expires=15552000&SESSDATA=sess_xyz&bili_jct=token_jct";
        let cookies = extract_cookies_from_url(url);
        assert!(cookies.contains("DedeUserID=12345678"));
        assert!(cookies.contains("DedeUserID__ckMd5=abc1234"));
        assert!(cookies.contains("SESSDATA=sess_xyz"));
        assert!(cookies.contains("bili_jct=token_jct"));
    }
}
