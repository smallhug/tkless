#[path = "../../tkless-core/tests/support/batch_site.rs"]
pub mod support;
use std::{
    io::BufRead,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};
use support::{Site, TempDir};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn command(dir: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tkless"));
    command.current_dir(&dir.0).env("NO_COLOR", "1");
    command
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_jsonl_is_live_and_killed_batches_resume_without_duplicate_fetches() {
    let site = Site::start().await;
    let dir = TempDir::default();
    let urls = dir.file("urls.txt");
    let checkpoint = dir.file("job.jsonl");
    std::fs::write(&urls, site.urls(&["/held", "/fast"]).join("\n")).unwrap();
    let args = vec![
        "--allow-private-addresses".into(),
        "batch".into(),
        urls.to_string_lossy().into_owned(),
        "-f".into(),
        "jsonl".into(),
        "--order".into(),
        "completion".into(),
        "--checkpoint".into(),
        checkpoint.to_string_lossy().into_owned(),
    ];
    let mut child = ChildGuard(
        command(&dir)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let first: serde_json::Value =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(10)).unwrap()).unwrap();
    assert_eq!(first["index"], 1);
    assert!(child.0.try_wait().unwrap().is_none());
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    reader.join().unwrap();
    site.release();
    let output = command(&dir).args(&args).arg("--resume").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let mut indices = rows
        .iter()
        .map(|item| item["index"].as_u64().unwrap())
        .collect::<Vec<_>>();
    indices.sort();
    assert_eq!(indices, [0, 1]);
    assert_eq!(site.count("/fast"), 1);
    assert!(String::from_utf8_lossy(&output.stderr).contains("1 restored, 1 fetched"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_retry_failed_preserves_successes_and_reports_partial_failure() {
    let site = Site::start().await;
    let dir = TempDir::default();
    let urls = dir.file("urls.txt");
    let checkpoint = dir.file("job.jsonl");
    std::fs::write(&urls, site.urls(&["/ok", "/flaky"]).join("\n")).unwrap();
    let args = vec![
        "--allow-private-addresses".into(),
        "batch".into(),
        urls.to_string_lossy().into_owned(),
        "-f".into(),
        "json".into(),
        "--checkpoint".into(),
        checkpoint.to_string_lossy().into_owned(),
    ];
    let first = command(&dir).args(&args).output().unwrap();
    assert!(!first.status.success());
    let first: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["failed"], 1);
    let second = command(&dir)
        .args(&args)
        .args(["--resume", "--retry-failed"])
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second["failed"], 0);
    assert_eq!(second["resumed"], 1);
    assert_eq!(second["fetched"], 1);
    assert_eq!((site.count("/ok"), site.count("/flaky")), (1, 2));
}

#[test]
fn cli_rejects_checkpoint_output_aliases_without_truncating_files() {
    let dir = TempDir::default();
    let urls = dir.file("urls.txt");
    let checkpoint = dir.file("job.jsonl");
    std::fs::write(&urls, "invalid-url\n").unwrap();
    let args = vec![
        "batch".into(),
        urls.to_string_lossy().into_owned(),
        "-f".into(),
        "jsonl".into(),
        "--checkpoint".into(),
        checkpoint.to_string_lossy().into_owned(),
        "--allow-partial".into(),
    ];
    let first = command(&dir).args(&args).output().unwrap();
    assert!(first.status.success());
    let bytes = std::fs::read(&checkpoint).unwrap();
    let second = command(&dir)
        .args(&args)
        .args(["--resume", "-o"])
        .arg(&checkpoint)
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(2));
    assert_eq!(std::fs::read(&checkpoint).unwrap(), bytes);
    let source = std::fs::read(&urls).unwrap();
    let third = command(&dir)
        .args(["batch"])
        .arg(&urls)
        .args(["-f", "jsonl", "-o"])
        .arg(&urls)
        .output()
        .unwrap();
    assert_eq!(third.status.code(), Some(2));
    assert_eq!(std::fs::read(&urls).unwrap(), source);
}
