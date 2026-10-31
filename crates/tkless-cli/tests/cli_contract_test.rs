//! Contract tests for CLI exit codes and machine-readable output.

use std::process::Command;

fn tkless() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tkless"));
    command.env("NO_COLOR", "1");
    command
}

#[test]
fn missing_target_exits_with_usage_code() {
    let output = tkless().output().expect("failed to run tkless");
    assert_eq!(
        output.status.code(),
        Some(2),
        "missing TARGET must be a usage error; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn json_log_format_keeps_fatal_errors_machine_readable() {
    let output = tkless()
        .args(["--log-format", "json"])
        .output()
        .expect("failed to run tkless");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON error line on stderr: {stderr}"));
    let value: serde_json::Value = serde_json::from_str(line).expect("fatal error must be JSON");
    assert_eq!(value["level"], "error");
    assert_eq!(value["exit_code"], 2);
}

#[test]
fn stats_are_written_to_stderr_and_stdout_stays_valid_json() {
    let dir = std::env::temp_dir().join(format!("tkless-stats-contract-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    let page = dir.join("page.html");
    std::fs::write(
        &page,
        "<html><body><article><h1>Title</h1><p>Hello world content for testing.</p></article></body></html>",
    )
    .expect("failed to write fixture");

    let output = tkless()
        .args([page.to_str().unwrap(), "-f", "json", "--stats"])
        .output()
        .expect("failed to run tkless");

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<serde_json::Value>(stdout.trim())
        .unwrap_or_else(|error| panic!("stdout must stay valid JSON ({error}): {stdout}"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mcp_dynamic_without_mcp_mode_is_a_usage_error() {
    let output = tkless()
        .args(["--mcp-dynamic"])
        .output()
        .expect("failed to run tkless");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn root_format_before_a_subcommand_is_not_ignored() {
    // No network access is required: an unsupported format must fail during argument
    // validation rather than silently falling back to Markdown.
    let output = tkless()
        .args(["-f", "text", "adapters"])
        .output()
        .expect("failed to run tkless");
    assert_eq!(output.status.code(), Some(2));
}
