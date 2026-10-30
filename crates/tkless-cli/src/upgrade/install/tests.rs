use super::*;
use sha2::{Digest, Sha256};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tkless-install-test-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("tkless.exe"), b"original executable").unwrap();
        Self(dir)
    }
    fn executable(&self) -> PathBuf {
        self.0.join("tkless.exe")
    }
    fn assert_original(&self) {
        assert_eq!(fs::read(self.executable()).unwrap(), b"original executable");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn binary() -> Vec<u8> {
    // Minimal native header; launch behavior is separately controlled by the probe.
    let mut bytes = vec![0; 128];
    if cfg!(windows) {
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
    } else if cfg!(target_os = "linux") {
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        let machine: u16 = if cfg!(target_arch = "aarch64") {
            183
        } else {
            62
        };
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    } else {
        bytes[..4].copy_from_slice(b"\xcf\xfa\xed\xfe");
        let cpu: u32 = if cfg!(target_arch = "aarch64") {
            0x0100000c
        } else {
            0x01000007
        };
        bytes[4..8].copy_from_slice(&cpu.to_le_bytes());
    }
    bytes
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn rejects_empty_and_non_executable_even_with_matching_checksum() {
    let fixture = Fixture::new();
    for bytes in [vec![], vec![b'x'; 128]] {
        assert!(
            install_with_probe(&fixture.executable(), &bytes, &hash(&bytes), |_| Ok(())).is_err()
        );
        fixture.assert_original();
    }
}

#[test]
fn rejects_staging_truncation_before_replacing_original() {
    let fixture = Fixture::new();
    let bytes = binary();
    let error = install_with_probe(&fixture.executable(), &bytes, &hash(&bytes), |candidate| {
        fs::write(candidate, [])?;
        Ok(())
    })
    .unwrap_err();
    assert!(error.to_string().contains("大小不符"));
    fixture.assert_original();
}

#[test]
fn restores_original_when_installed_file_becomes_empty() {
    let fixture = Fixture::new();
    let bytes = binary();
    let mut probes = 0;
    let error = install_with_probe(&fixture.executable(), &bytes, &hash(&bytes), |path| {
        probes += 1;
        if probes == 2 {
            fs::write(path, [])?;
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(probes, 2);
    assert!(error.to_string().contains("已恢复原程序"));
    fixture.assert_original();
}

#[test]
fn failed_launch_after_installation_rolls_back() {
    let fixture = Fixture::new();
    let bytes = binary();
    let mut probes = 0;
    let result = install_with_probe(&fixture.executable(), &bytes, &hash(&bytes), |_| {
        probes += 1;
        if probes == 2 {
            bail!("模拟新程序启动失败");
        }
        Ok(())
    });
    assert!(result.is_err());
    fixture.assert_original();
}

#[test]
fn invalid_program_cannot_report_success() {
    let fixture = Fixture::new();
    let bytes = binary();
    assert!(install(&fixture.executable(), &bytes, &hash(&bytes), "0.5.0").is_err());
    fixture.assert_original();
}

#[test]
fn concurrent_installer_is_rejected_without_touching_destination() {
    let fixture = Fixture::new();
    let bytes = binary();
    let path = fs::canonicalize(fixture.executable()).unwrap();
    let _lock = acquire_lock(&path).unwrap();
    assert!(install_with_probe(&path, &bytes, &hash(&bytes), |_| Ok(())).is_err());
    fixture.assert_original();
}

#[test]
fn successful_install_checks_both_paths_and_ignores_shared_legacy_temp() {
    let fixture = Fixture::new();
    let bytes = binary();
    let mut probes = vec![];
    let legacy = fixture.executable().with_extension("new.tmp");
    fs::write(&legacy, b"another process").unwrap();
    install_with_probe(&fixture.executable(), &bytes, &hash(&bytes), |path| {
        probes.push(path.to_path_buf());
        Ok(())
    })
    .unwrap();
    assert_eq!(probes.len(), 2);
    assert_ne!(probes[0], legacy);
    assert_eq!(probes[1], fs::canonicalize(fixture.executable()).unwrap());
    assert_eq!(fs::read(fixture.executable()).unwrap(), bytes);
    assert_eq!(fs::read(legacy).unwrap(), b"another process");
}
