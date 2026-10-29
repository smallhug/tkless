//! Binary self-upgrade mechanism from GitHub Releases.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use tkless_core::TklessEngine;

/// Automatically checks GitHub Releases and upgrades the current running executable (like bun upgrade).
pub async fn run_upgrade(engine: &TklessEngine, args: &crate::cli::UpgradeArgs) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let repo = &args.repo;
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");

    eprintln!("[TkLess] 正在检查 GitHub 最新版本发布信息 (https://github.com/{repo})...");

    let resp = engine
        .client()
        .get(&url)
        .header("User-Agent", format!("tkless/{current_version}"))
        .send()
        .await
        .with_context(|| format!("连接 GitHub API 失败: {url}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("GitHub API 返回异常状态码 {status}: {body}");
    }

    let release_json: serde_json::Value =
        resp.json().await.context("解析 GitHub Release 数据失败")?;
    let tag_name = release_json["tag_name"]
        .as_str()
        .context("Release 数据中未找到 tag_name 字段")?;
    let latest_version = tag_name.trim_start_matches('v');

    eprintln!("[TkLess] 当前版本: v{current_version} | 最新版本: {tag_name}");

    let current_semver = semver::Version::parse(current_version)?;
    let latest_semver =
        semver::Version::parse(latest_version).context("Release tag is not a semantic version")?;
    if latest_semver <= current_semver && !args.force {
        println!("TkLess 已经是最新版本 (v{current_version})，无需更新。");
        return Ok(());
    }

    if args.check {
        if latest_semver > current_semver {
            println!("检测到 TkLess 有新版本可用: {tag_name} (当前: v{current_version})");
            println!("   运行 'tkless upgrade' 即可一键自动升级。");
        } else {
            println!("当前已是最新版本 (v{current_version})。");
        }
        return Ok(());
    }

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let target_asset_name = match (os, arch) {
        ("windows", "x86_64") => "tkless-windows-amd64.exe",
        ("linux", "x86_64") => "tkless-linux-amd64",
        ("linux", "aarch64") => "tkless-linux-arm64",
        ("macos", "aarch64") => "tkless-macos-arm64.dmg",
        ("macos", "x86_64") => "tkless-macos-amd64.dmg",
        _ => bail!(
            "暂不支持在系统架构 {os}-{arch} 上执行自动替换更新，请从 GitHub Releases 手动下载。"
        ),
    };

    let assets = release_json["assets"]
        .as_array()
        .context("Release 中未发现可下载的产物列表 (assets)")?;

    let asset = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(target_asset_name))
        .with_context(|| {
            format!("未在 Release {tag_name} 中找到适用于当前平台的产物 '{target_asset_name}'")
        })?;

    let download_url = asset["browser_download_url"]
        .as_str()
        .context("下载链接缺失")?;

    eprintln!("[TkLess] 正在下载最新二进制: {download_url}...");
    let bytes = download_release_bytes(engine, download_url).await?;
    let expected = if let Some(digest) = asset
        .get("digest")
        .and_then(|v| v.as_str())
        .and_then(|s| s.strip_prefix("sha256:"))
    {
        digest.to_string()
    } else {
        let checksum_name = format!("{target_asset_name}.sha256");
        let checksum_asset = assets.iter().find(|a| a["name"].as_str() == Some(&checksum_name))
            .context("Release has no SHA-256 digest or checksum asset; refusing to replace the executable")?;
        let checksum_url = checksum_asset["browser_download_url"]
            .as_str()
            .context("Missing checksum download URL")?;
        let checksum_bytes = download_release_bytes(engine, checksum_url).await?;
        let checksum = std::str::from_utf8(&checksum_bytes).context("Invalid checksum encoding")?;
        checksum
            .split_whitespace()
            .next()
            .context("Empty checksum file")?
            .to_string()
    };
    verify_sha256(&bytes, &expected)?;

    let current_exe = std::env::current_exe().context("无法确定当前执行程序的物理路径")?;
    eprintln!("[TkLess] 当前执行程序路径: {}", current_exe.display());

    #[cfg(target_os = "macos")]
    {
        let tmp_dmg = std::env::temp_dir().join(format!("tkless_upgrade_{}.dmg", uuid_simple()));
        std::fs::write(&tmp_dmg, &bytes)?;
        let mount_point = std::env::temp_dir().join(format!("tkless_mnt_{}", uuid_simple()));
        let _ = std::fs::create_dir_all(&mount_point);

        let attach_status = std::process::Command::new("hdiutil")
            .args([
                "attach",
                tmp_dmg.to_str().unwrap(),
                "-mountpoint",
                mount_point.to_str().unwrap(),
                "-nobrowse",
                "-quiet",
            ])
            .status()?;
        if !attach_status.success() {
            let _ = std::fs::remove_file(&tmp_dmg);
            bail!("无法挂载下载的 DMG 镜像包");
        }

        let inner_bin = mount_point.join("tkless");
        let temp_exe = current_exe.with_extension("new.tmp");
        std::fs::copy(&inner_bin, &temp_exe)?;

        let _ = std::process::Command::new("hdiutil")
            .args(["detach", mount_point.to_str().unwrap(), "-quiet"])
            .status();
        let _ = std::fs::remove_file(&tmp_dmg);
        let _ = std::fs::remove_dir_all(&mount_point);

        std::fs::rename(&temp_exe, &current_exe)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&current_exe, std::fs::Permissions::from_mode(0o755));
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let temp_exe = current_exe.with_extension("new.tmp");
        std::fs::write(&temp_exe, &bytes)?;

        #[cfg(target_os = "windows")]
        {
            let old_exe = current_exe.with_extension("old.tmp");
            let _ = std::fs::remove_file(&old_exe);
            std::fs::rename(&current_exe, &old_exe)
                .context("重命名当前运行中程序失败，可能被杀毒软件或其它进程锁定")?;
            if let Err(e) = std::fs::rename(&temp_exe, &current_exe) {
                // Rollback on failure
                let _ = std::fs::rename(&old_exe, &current_exe);
                return Err(e).context("替换新版本二进制失败");
            }
            let _ = std::fs::remove_file(&old_exe);
        }

        #[cfg(not(target_os = "windows"))]
        {
            std::fs::rename(&temp_exe, &current_exe).context("替换新版本二进制失败")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&current_exe, std::fs::Permissions::from_mode(0o755));
            }
        }
    }

    println!("恭喜！TkLess 已成功自动升级至 {}！", tag_name);
    Ok(())
}

#[cfg(target_os = "macos")]
fn uuid_simple() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// Public release assets may redirect; validate each hop and bound the download.
async fn download_release_bytes(engine: &TklessEngine, url: &str) -> Result<Vec<u8>> {
    let mut current = url::Url::parse(url)?;
    for _ in 0..=10 {
        if current.scheme() != "https" {
            bail!("Release downloads require HTTPS");
        }
        engine.fetcher().validate_ssrf(&current).await?;
        let mut response = engine.client().get(current.clone()).send().await?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .context("Redirect has no Location")?
                .to_str()?;
            current = current.join(location)?;
            continue;
        }
        response.error_for_status_ref()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > 100 * 1024 * 1024 {
                bail!("Release asset exceeds 100 MB");
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(bytes);
    }
    bail!("Too many release download redirects")
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Invalid SHA-256 checksum");
    }
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("Release SHA-256 mismatch; executable was not replaced");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_corrupted_release_before_installation() {
        let digest = format!("{:x}", Sha256::digest(b"release"));
        assert!(verify_sha256(b"release", &digest).is_ok());
        assert!(verify_sha256(b"corrupted", &digest).is_err());
        assert!(verify_sha256(b"release", "bad").is_err());
    }
}
