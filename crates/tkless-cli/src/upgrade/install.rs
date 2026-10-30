//! Verified installation with an exclusive lock and recoverable replacement.
use super::verify_sha256;
use anyhow::{bail, Context, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub(super) const MAX_BINARY_BYTES: usize = 100 * 1024 * 1024;

pub(super) struct Stage(PathBuf);
impl Stage {
    pub(super) fn new(parent: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".tkless-upgrade-{}-{nonce}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e).context("无法创建独立升级目录"),
            }
        }
        bail!("无法分配升级目录");
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        // Never remove a retained backup or recursively traverse a mounted DMG.
        let _ = fs::remove_file(self.0.join("candidate.exe"));
        let _ = fs::remove_file(self.0.join("download.dmg"));
        let _ = fs::remove_dir(self.0.join("mount"));
        let _ = fs::remove_dir(&self.0);
    }
}

fn acquire_lock(current: &Path) -> Result<File> {
    let mut name = current.as_os_str().to_owned();
    name.push(".upgrade.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(PathBuf::from(name))
        .context("无法打开升级锁文件")?;
    file.try_lock()
        .context("无法取得升级锁，可能已有其他升级进程")?;
    Ok(file)
}

pub(super) fn validate_binary(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 64 || bytes.len() > MAX_BINARY_BYTES {
        bail!("升级文件为空、过短或超过大小限制，拒绝替换");
    }
    let valid = if cfg!(target_os = "windows") {
        let offset = u32::from_le_bytes(bytes[60..64].try_into()?) as usize;
        bytes.starts_with(b"MZ")
            && offset.checked_add(6).is_some_and(|end| {
                end <= bytes.len()
                    && &bytes[offset..offset + 4] == b"PE\0\0"
                    && u16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]) == 0x8664
            })
    } else if cfg!(target_os = "linux") {
        let machine = if cfg!(target_arch = "aarch64") {
            183
        } else {
            62
        };
        bytes.starts_with(b"\x7fELF")
            && bytes[4] == 2
            && bytes[5] == 1
            && u16::from_le_bytes([bytes[18], bytes[19]]) == machine
    } else if cfg!(target_os = "macos") {
        let cpu = if cfg!(target_arch = "aarch64") {
            0x0100000c
        } else {
            0x01000007
        };
        bytes.starts_with(b"\xcf\xfa\xed\xfe") && u32::from_le_bytes(bytes[4..8].try_into()?) == cpu
    } else {
        false
    };
    if !valid {
        bail!("升级文件不是当前平台的可执行程序，拒绝替换");
    }
    Ok(())
}

fn verify_file(path: &Path, bytes: &[u8], expected: &str) -> Result<()> {
    let file = File::open(path).context("无法读取落盘后的升级文件")?;
    if file.metadata()?.len() != bytes.len() as u64 {
        bail!("升级文件落盘后大小不符");
    }
    let mut actual = Vec::new();
    file.take(bytes.len() as u64 + 1).read_to_end(&mut actual)?;
    if actual.len() != bytes.len() {
        bail!("升级文件在校验期间发生变化");
    }
    verify_sha256(&actual, expected)
}

fn probe_version(path: &Path, expected: &str) -> Result<()> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("新程序无法启动，升级未完成")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                match result {
                    Err(error) => return Err(error).context("检查新程序状态失败"),
                    _ => bail!("新程序版本检查超时"),
                }
            }
        }
    };
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .context("缺少版本输出")?
        .take(4096)
        .read_to_string(&mut stdout)?;
    let mut words = stdout.split_whitespace();
    if !status.success() || words.next() != Some("tkless") || words.next() != Some(expected) {
        bail!(
            "新程序版本检查失败：预期 tkless {expected}，实际 {}",
            stdout.trim()
        );
    }
    Ok(())
}

/// Returns a retained backup when Windows still holds the running old executable open.
pub(super) fn install(
    current: &Path,
    bytes: &[u8],
    expected: &str,
    version: &str,
) -> Result<Option<PathBuf>> {
    install_with_probe(current, bytes, expected, |path| {
        probe_version(path, version)
    })
}

fn install_with_probe(
    current: &Path,
    bytes: &[u8],
    expected: &str,
    mut probe: impl FnMut(&Path) -> Result<()>,
) -> Result<Option<PathBuf>> {
    validate_binary(bytes)?;
    verify_sha256(bytes, expected)?;
    let current = fs::canonicalize(current).context("无法定位待替换程序")?;
    let _lock = acquire_lock(&current)?;
    let stage = Stage::new(current.parent().context("程序路径缺少父目录")?)?;
    let candidate = stage.path().join("candidate.exe");
    let backup = stage.path().join("previous.exe");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&candidate)?;
    file.write_all(bytes).context("写入升级文件失败")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    file.sync_all().context("升级文件持久化失败")?;
    drop(file);
    verify_file(&candidate, bytes, expected)?;
    probe(&candidate)?;
    // Probe or another process must not silently change the verified candidate.
    verify_file(&candidate, bytes, expected)?;

    #[cfg(windows)]
    fs::rename(&current, &backup).context("无法备份运行中的程序，原文件未替换")?;
    #[cfg(not(windows))]
    {
        fs::copy(&current, &backup).context("无法备份当前程序")?;
        File::open(&backup)?.sync_all()?;
    }

    let replacement = (|| {
        fs::rename(&candidate, &current).context("替换新程序失败")?;
        verify_file(&current, bytes, expected)?;
        probe(&current)?;
        verify_file(&current, bytes, expected)
    })();
    if let Err(error) = replacement {
        // The original is never discarded until the installed executable passes validation.
        #[cfg(windows)]
        if current.exists() {
            fs::rename(&current, &candidate).with_context(|| {
                format!(
                    "升级失败（{error:#}），且无法移开损坏文件；原程序备份位于 {}",
                    backup.display()
                )
            })?;
        }
        fs::rename(&backup, &current).with_context(|| {
            format!(
                "升级失败（{error:#}），回滚也失败；原程序备份位于 {}",
                backup.display()
            )
        })?;
        bail!("升级失败，已恢复原程序：{error:#}");
    }
    match fs::remove_file(&backup) {
        Ok(()) => Ok(None),
        Err(_) => Ok(Some(backup)),
    }
}

#[cfg(test)]
mod tests;
