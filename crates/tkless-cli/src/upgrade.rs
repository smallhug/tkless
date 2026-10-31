//! Binary self-upgrade mechanism from GitHub Releases.

mod install;

use anyhow::{bail, Context, Result};
use minisign_verify::{PublicKey, Signature};
use sha2::{Digest, Sha256};
use tkless_core::TklessEngine;

const MANIFEST_ASSET: &str = "SHA256SUMS";
const MANIFEST_SIGNATURE_ASSET: &str = "SHA256SUMS.minisig";
const MANIFEST_MAX_BYTES: usize = 64 * 1024;
const SIGNATURE_MAX_BYTES: usize = 16 * 1024;

/// Public keys embedded at build time. The file may contain several Minisign
/// public key blocks; every base64 line is trusted. Keeping the current and next
/// key here allows a rotation release to be verified by both old and new binaries.
const EMBEDDED_UPGRADE_PUBLIC_KEYS: &str = include_str!("../assets/tkless-upgrade.pub");

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

    // The signature is the security boundary. Without a trusted manifest the
    // release JSON, asset URLs and GitHub-provided digests are only transport
    // hints and must not be used to authorize replacing the executable.
    let public_keys = embedded_upgrade_public_keys();
    if public_keys.is_empty() {
        bail!(
            "This build does not embed a TkLess upgrade public key; refusing to self-update. \
             Use an official release build or rebuild with a Minisign public key in \
             crates/tkless-cli/assets/tkless-upgrade.pub."
        );
    }
    let manifest =
        download_release_asset(engine, assets, MANIFEST_ASSET, MANIFEST_MAX_BYTES).await?;
    let signature = download_release_asset(
        engine,
        assets,
        MANIFEST_SIGNATURE_ASSET,
        SIGNATURE_MAX_BYTES,
    )
    .await?;
    let signature_text =
        std::str::from_utf8(&signature).context("Upgrade signature is not valid UTF-8")?;
    verify_manifest_signature(&manifest, signature_text, &public_keys)?;
    let manifest_text =
        std::str::from_utf8(&manifest).context("Upgrade manifest is not valid UTF-8")?;
    let expected = parse_signed_manifest(manifest_text, latest_version, target_asset_name)?;

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
    let expected_size = asset["size"]
        .as_u64()
        .context("Release 产物缺少 size 字段")?;
    if expected_size == 0 || expected_size > install::MAX_BINARY_BYTES as u64 {
        bail!("Release 产物为空或超过 100 MiB，拒绝更新");
    }
    let bytes = download_release_bytes(
        engine,
        download_url,
        install::MAX_BINARY_BYTES,
        Some(expected_size),
    )
    .await?;
    // The signed manifest, not the GitHub release metadata, supplies the hash.
    verify_sha256(&bytes, &expected)?;

    let current_exe = std::env::current_exe().context("无法确定当前执行程序的物理路径")?;
    eprintln!("[TkLess] 当前执行程序路径: {}", current_exe.display());

    let version = latest_version.to_owned();
    let backup = tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        let (bytes, expected) = {
            let binary = extract_dmg_binary(&bytes)?;
            let digest = format!("{:x}", Sha256::digest(&binary));
            (binary, digest)
        };
        install::install(&current_exe, &bytes, &expected, &version)
    })
    .await
    .context("升级安装任务异常")??;
    if let Some(path) = backup {
        eprintln!(
            "[TkLess] 原程序仍被进程占用，备份保留在：{}",
            path.display()
        );
    }
    println!("TkLess 已成功升级至 {tag_name}，文件完整性与启动版本检查均已通过。");
    Ok(())
}

#[cfg(target_os = "macos")]
fn extract_dmg_binary(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let stage = install::Stage::new(&std::env::temp_dir())?;
    let dmg = stage.path().join("download.dmg");
    let mount = stage.path().join("mount");
    std::fs::write(&dmg, bytes)?;
    std::fs::create_dir(&mount)?;
    let status = std::process::Command::new("hdiutil")
        .arg("attach")
        .arg(&dmg)
        .arg("-mountpoint")
        .arg(&mount)
        .args(["-readonly", "-nobrowse", "-quiet"])
        .status()?;
    if !status.success() {
        bail!("无法挂载下载的 DMG 镜像包");
    }
    let extracted = (|| -> Result<Vec<u8>> {
        let file = std::fs::File::open(mount.join("tkless"))?;
        let mut binary = Vec::new();
        file.take(install::MAX_BINARY_BYTES as u64 + 1)
            .read_to_end(&mut binary)?;
        install::validate_binary(&binary)?;
        Ok(binary)
    })();
    let detached = std::process::Command::new("hdiutil")
        .arg("detach")
        .arg(&mount)
        .arg("-quiet")
        .status()?;
    if !detached.success() {
        bail!("无法卸载 DMG，停止更新；挂载点：{}", mount.display());
    }
    extracted
}

fn embedded_upgrade_public_keys() -> Vec<&'static str> {
    EMBEDDED_UPGRADE_PUBLIC_KEYS
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && !line.starts_with('#') && !line.starts_with("untrusted comment:")
        })
        .collect()
}

async fn download_release_asset(
    engine: &TklessEngine,
    assets: &[serde_json::Value],
    name: &str,
    limit: usize,
) -> Result<Vec<u8>> {
    let asset = assets
        .iter()
        .find(|asset| asset["name"].as_str() == Some(name))
        .with_context(|| format!("Release is missing the signed asset '{name}'"))?;
    let url = asset["browser_download_url"]
        .as_str()
        .with_context(|| format!("Missing download URL for '{name}'"))?;
    download_release_bytes(engine, url, limit, None).await
}

fn verify_manifest_signature(
    manifest: &[u8],
    signature_text: &str,
    public_keys: &[&str],
) -> Result<()> {
    if public_keys.is_empty() {
        bail!("No upgrade public key is embedded in this build");
    }
    let normalized_signature = signature_text.replace("\r\n", "\n");
    let signature = Signature::decode(normalized_signature.trim())
        .context("Invalid Minisign signature file")?;
    let mut last_error = None;
    for key in public_keys {
        let public_key =
            PublicKey::from_base64(key).context("Invalid embedded Minisign public key")?;
        match public_key.verify(manifest, &signature, false) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    bail!(
        "Upgrade manifest signature verification failed: {}",
        last_error.unwrap_or_else(|| "no matching embedded key".into())
    )
}

fn parse_signed_manifest(
    manifest: &str,
    expected_version: &str,
    asset_name: &str,
) -> Result<String> {
    let mut version = None;
    let mut digests = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# version:") {
            version = Some(rest.trim());
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(digest) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else {
            continue;
        };
        let name = name.trim_start_matches('*').trim_start_matches("./");
        if name != asset_name {
            continue;
        }
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("Signed manifest contains an invalid SHA-256 for '{asset_name}'");
        }
        digests.push(digest.to_string());
    }
    let version = version.context("Signed manifest has no '# version:' header")?;
    if version != expected_version {
        bail!(
            "Signed manifest version '{version}' does not match release version '{expected_version}'"
        );
    }
    match digests.len() {
        1 => Ok(digests.remove(0)),
        0 => bail!("Signed manifest has no entry for '{asset_name}'"),
        _ => bail!("Signed manifest has duplicate entries for '{asset_name}'"),
    }
}

/// Public release assets may redirect; validate each hop and bound the download.
async fn download_release_bytes(
    engine: &TklessEngine,
    url: &str,
    limit: usize,
    expected_size: Option<u64>,
) -> Result<Vec<u8>> {
    let mut current = url::Url::parse(url)?;
    for _ in 0..=10 {
        if current.scheme() != "https" {
            bail!("Release downloads require HTTPS");
        }
        engine.fetcher().validate_ssrf(&current).await?;
        let mut response = engine
            .client()
            .get(current.clone())
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await?;
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
        if response.status() != reqwest::StatusCode::OK {
            bail!("Release 下载未返回完整的 HTTP 200 响应");
        }
        let content_length = response.content_length();
        if content_length.is_some_and(|size| size > limit as u64) {
            bail!("Release 响应超过大小限制");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > limit {
                bail!("Release 响应超过大小限制");
            }
            bytes.extend_from_slice(&chunk);
        }
        validate_download_size(bytes.len(), expected_size.or(content_length))?;
        return Ok(bytes);
    }
    bail!("Too many release download redirects")
}

fn validate_download_size(actual: usize, expected: Option<u64>) -> Result<()> {
    if actual == 0 {
        bail!("下载结果为空，拒绝替换程序");
    }
    if expected.is_some_and(|size| size != actual as u64) {
        bail!(
            "下载文件大小不匹配：预期 {:?} 字节，实际 {actual} 字节",
            expected
        );
    }
    Ok(())
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    validate_download_size(bytes.len(), None)?;
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
        let empty_hash = format!("{:x}", Sha256::digest(b""));
        assert!(verify_sha256(b"", &empty_hash).is_err());
    }

    const TEST_PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\n\
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\n\
trusted comment: timestamp:1633700835\tfile:test\tprehashed\n\
wLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";

    #[test]
    fn verifies_a_minisign_manifest_signature() {
        verify_manifest_signature(b"test", TEST_SIGNATURE, &[TEST_PUBLIC_KEY]).unwrap();
        assert!(
            verify_manifest_signature(b"tampered", TEST_SIGNATURE, &[TEST_PUBLIC_KEY]).is_err()
        );
        assert!(verify_manifest_signature(b"test", TEST_SIGNATURE, &[]).is_err());
        let wrong_key = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO4";
        assert!(verify_manifest_signature(b"test", TEST_SIGNATURE, &[wrong_key]).is_err());
    }

    #[test]
    fn manifest_binds_version_asset_and_hash() {
        let digest = "a".repeat(64);
        let manifest = format!(
            "# version: 0.5.0\n# generated: 2026-10-31T00:00:00Z\n{digest}  *tkless-linux-amd64\n"
        );
        assert_eq!(
            parse_signed_manifest(&manifest, "0.5.0", "tkless-linux-amd64").unwrap(),
            digest
        );
        assert!(parse_signed_manifest(&manifest, "0.5.1", "tkless-linux-amd64").is_err());
        assert!(parse_signed_manifest(&manifest, "0.5.0", "tkless-windows-amd64.exe").is_err());
        let duplicate = format!(
            "# version: 0.5.0\n{digest}  tkless-linux-amd64\n{digest}  tkless-linux-amd64\n"
        );
        assert!(parse_signed_manifest(&duplicate, "0.5.0", "tkless-linux-amd64").is_err());
        let missing_version = format!("{digest}  tkless-linux-amd64\n");
        assert!(parse_signed_manifest(&missing_version, "0.5.0", "tkless-linux-amd64").is_err());
    }

    #[test]
    fn rejects_empty_and_incomplete_downloads() {
        assert!(validate_download_size(0, Some(0)).is_err());
        assert!(validate_download_size(64, Some(128)).is_err());
        assert!(validate_download_size(128, Some(64)).is_err());
        assert!(validate_download_size(128, Some(128)).is_ok());
    }

    #[tokio::test]
    async fn legacy_nonfollowing_client_receives_empty_redirect_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let received = stream.read(&mut request).await.unwrap();
            assert!(received > 0);
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://example.com/asset\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let engine = TklessEngine::with_options(tkless_core::FetchOptions {
            allow_private_addresses: true,
            ..Default::default()
        })
        .unwrap();
        let response = engine
            .client()
            .get(format!("http://{address}/asset"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        let bytes = response.bytes().await.unwrap();
        assert!(bytes.is_empty());
        assert!(validate_download_size(bytes.len(), Some(1024)).is_err());
        assert!(install::validate_binary(&bytes).is_err());
        server.await.unwrap();
    }
}
