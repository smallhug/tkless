use super::{McpServer, McpToolCallResult, Value};

impl McpServer {
    pub(super) async fn call_login_platform(&self, args: &Value) -> McpToolCallResult {
        let platform = match args.get("platform").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.trim().to_ascii_lowercase(),
            _ => {
                return McpToolCallResult::error(
                    "Missing required parameter 'platform' ('bilibili' or 'zhihu')",
                )
            }
        };

        let output_file = args
            .get("output_cookie_file")
            .and_then(|v| v.as_str())
            .unwrap_or("./cookies.txt");

        let no_browser = args
            .get("no_browser")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(180);

        match platform.as_str() {
            "bilibili" | "bili" => {
                let login_opts = crate::auth::LoginOptions {
                    output_path: std::path::PathBuf::from(output_file),
                    no_browser,
                    timeout_secs,
                };
                match crate::auth::execute_login_flow(Some(self.engine.client()), login_opts).await
                {
                    Ok(cookie_str) => McpToolCallResult::success(format!(
                        "Bilibili 登录成功！凭据已自动以 'bilibili: ...' 格式保存至 `{output_file}`。\n凭据摘要: 长度 {} 字符",
                        cookie_str.len()
                    )),
                    Err(e) => McpToolCallResult::error(format!("Bilibili 扫码登录失败: {e}")),
                }
            }
            "zhihu" => {
                let msg = format!(
                    "知乎登录说明：\n1. 请在本地浏览器访问授权登录页：https://www.zhihu.com/signin\n2. 登录成功后，从浏览器开发者工具 (F12 -> Application -> Cookies) 复制 Cookie 字符串\n3. 将凭据以 'zhihu: ...' 前缀格式追加到 `{output_file}`：\n```\nzhihu: d_c0=\"...\"; z_c0=\"...\"\n```\n4. 保存后 TkLess 将自动识别知乎会话凭据进行鉴权访问。"
                );
                McpToolCallResult::success(msg)
            }
            other => McpToolCallResult::error(format!(
                "Unsupported platform '{other}'. Currently supported: 'bilibili', 'zhihu'"
            )),
        }
    }

    pub(super) async fn call_check_auth(&self, args: &Value) -> McpToolCallResult {
        let platform = match args.get("platform").and_then(|v| v.as_str()) {
            Some(p) => p.trim().to_lowercase(),
            None => return McpToolCallResult::error("Missing required parameter 'platform'"),
        };

        let cookie_file = args
            .get("cookie_file")
            .and_then(|v| v.as_str())
            .unwrap_or("./cookies.txt");

        let cli_cookie = args
            .get("cookie")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let target_test_url = match platform.as_str() {
            "bilibili" => "https://www.bilibili.com/",
            "zhihu" => "https://www.zhihu.com/",
            "github" => "https://github.com/",
            "wechat" => "https://mp.weixin.qq.com/",
            other => {
                return McpToolCallResult::error(format!(
                    "Unsupported platform '{other}'. Currently supported: 'bilibili', 'zhihu', 'github', 'wechat'"
                ))
            }
        };

        let resolved_cookie = if let Some(ref c) = cli_cookie {
            Some(c.clone())
        } else {
            let path = std::path::Path::new(cookie_file);
            crate::fetcher::load_cookie_from_file_for_url(path, Some(target_test_url)).ok()
        };

        let cookie_str = match resolved_cookie {
            Some(c) if !c.trim().is_empty() => c,
            _ => {
                return McpToolCallResult::success(format!(
                    "# [{}] 未检测到有效登录凭据 (No Credentials Found)\n\n\
                    - 凭据文件: `{cookie_file}`\n\
                    - 说明: 未在此文件中找到带 `{platform}:` 前缀的凭据记录。\n\
                    - 建议操作: 执行登录提取指令以生成凭据，例如在 CLI 运行 `tkless login {platform}`，或手动将凭据按 `{platform}: key=val; ...` 写入 `{cookie_file}`。",
                    platform.to_uppercase()
                ));
            }
        };

        match platform.as_str() {
            "bilibili" => {
                let nav_url = "https://api.bilibili.com/x/web-interface/nav";
                match self
                    .engine
                    .client()
                    .get(nav_url)
                    .header("Cookie", &cookie_str)
                    .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36")
                    .header("Referer", "https://www.bilibili.com/")
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(v) = resp.json::<Value>().await {
                            let is_login = v.pointer("/data/isLogin").and_then(|b| b.as_bool()).unwrap_or(false);
                            if is_login {
                                let uname = v.pointer("/data/uname").and_then(|u| u.as_str()).unwrap_or("未知");
                                let mid = v.pointer("/data/mid").and_then(|m| m.as_u64()).unwrap_or(0);
                                let level = v.pointer("/data/level_info/current_level").and_then(|l| l.as_u64()).unwrap_or(0);
                                let vip_type = v.pointer("/data/vipType").and_then(|t| t.as_u64()).unwrap_or(0);
                                let vip_str = if vip_type > 0 { "大会员 (Active)" } else { "普通用户 (Standard)" };

                                McpToolCallResult::success(format!(
                                    "# [BILIBILI] 身份凭据鉴权成功 (Authenticated)\n\n\
                                    - **用户昵称**: {uname}\n\
                                    - **UID (mid)**: `{mid}`\n\
                                    - **账号等级**: Lv{level}\n\
                                    - **会员状态**: {vip_str}\n\
                                    - **凭据作用域**: `bilibili.com` (已隔离存储)"
                                ))
                            } else {
                                McpToolCallResult::success(
                                    "# [BILIBILI] 凭据已失效或已过期 (Cookie Expired)\n\n\
                                    - Bilibili 鉴权接口返回 `isLogin: false`。\n\
                                    - 建议操作: 运行 `tkless login bilibili` 重新扫码更新凭据。"
                                        .to_string(),
                                )
                            }
                        } else {
                            McpToolCallResult::success(format!("已加载 Bilibili 凭据 (长度 {} 字符)，解析接口响应异常。", cookie_str.len()))
                        }
                    }
                    Ok(resp) => McpToolCallResult::error(format!("Bilibili 鉴权检查网络响应异常: {}", resp.status())),
                    Err(e) => McpToolCallResult::error(format!("Bilibili 鉴权检查请求失败: {e}")),
                }
            }
            "zhihu" | "github" | "wechat" => {
                match crate::auth::verify_web_session(self.engine.client(), &platform, &cookie_str)
                    .await
                {
                    Ok(status) => McpToolCallResult::success(status.to_string()),
                    Err(error) => McpToolCallResult::error(format!(
                        "Session verification failed; validity is unknown: {error}"
                    )),
                }
            }
            _ => McpToolCallResult::error(format!("Unsupported platform '{platform}'")),
        }
    }
}
