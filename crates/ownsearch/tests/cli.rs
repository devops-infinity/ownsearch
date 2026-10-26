#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod support;

use std::process::Command;

use support::temp_dir;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_ownsearch")
}

fn env_dir(dir: &std::path::Path) -> Vec<(String, String)> {
    vec![
        (
            "OWNSEARCH_STATE_DIR".to_owned(),
            dir.join("state").display().to_string(),
        ),
        (
            "OWNSEARCH_LEDGER_DIR".to_owned(),
            dir.join("ledger").display().to_string(),
        ),
        ("OWNSEARCH_OFFLINE".to_owned(), "true".to_owned()),
    ]
}

#[test]
fn the_help_lists_every_command() {
    let output = Command::new(binary()).arg("--help").output().expect("run");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    for command in [
        "serve",
        "doctor",
        "scan",
        "instances",
        "db",
        "config",
        "audit",
        "completions",
        "man",
    ] {
        assert!(text.contains(command), "missing {command}");
    }
}

#[test]
fn the_version_flag_prints_the_package_version() {
    let output = Command::new(binary())
        .arg("--version")
        .output()
        .expect("run");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn doctor_does_not_create_the_database() {
    let dir = temp_dir("cli-doctor");
    let mut command = Command::new(binary());
    command.arg("doctor");
    for (key, value) in env_dir(&dir) {
        command.env(key, value);
    }
    let output = command.output().expect("run");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("does not exist yet"));
    assert!(!dir.join("state").join("instances.db").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn db_path_prints_the_resolved_database() {
    let dir = temp_dir("cli-db-path");
    let mut command = Command::new(binary());
    command.args(["db", "path"]);
    for (key, value) in env_dir(&dir) {
        command.env(key, value);
    }
    let output = command.output().expect("run");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        text.trim(),
        dir.join("state").join("instances.db").display().to_string()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_status_is_refused_with_a_remedy() {
    let dir = temp_dir("cli-status");
    let mut command = Command::new(binary());
    command.args(["instances", "list", "--status", "nonsense"]);
    for (key, value) in env_dir(&dir) {
        command.env(key, value);
    }
    let output = command.output().expect("run");
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("unknown status"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn completions_render_for_bash() {
    let output = Command::new(binary())
        .args(["completions", "bash"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("ownsearch"));
}

#[test]
fn the_manual_page_renders() {
    let output = Command::new(binary()).arg("man").output().expect("run");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.to_ascii_uppercase().contains("OWNSEARCH"));
}

#[test]
fn the_database_commands_work_on_a_fresh_state() {
    let dir = temp_dir("cli-db");
    let run = |args: &[&str]| {
        let mut command = Command::new(binary());
        command.args(args);
        for (key, value) in env_dir(&dir) {
            command.env(key, value);
        }
        command.output().expect("run")
    };
    let created = run(&["instances", "list"]);
    assert!(created.status.success());
    let verify = run(&["db", "verify"]);
    assert!(verify.status.success());
    let text = String::from_utf8_lossy(&verify.stdout);
    assert!(text.contains("schema version: 1"));
    let counts = run(&["db", "counts", "--json"]);
    assert!(counts.status.success());
    let export = run(&["db", "export"]);
    assert!(export.status.success());
    assert!(dir.join("state").join("instances.snapshot.json").exists());
    let maintenance = run(&["db", "maintenance"]);
    assert!(maintenance.status.success());
    let backup = run(&["db", "backup"]);
    assert!(backup.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_instance_commands_report_and_change_state() {
    let dir = temp_dir("cli-instances");
    let run = |args: &[&str]| {
        let mut command = Command::new(binary());
        command.args(args);
        for (key, value) in env_dir(&dir) {
            command.env(key, value);
        }
        command.output().expect("run")
    };
    let list = run(&["instances", "list"]);
    assert!(list.status.success());
    let text = String::from_utf8_lossy(&list.stdout);
    assert!(text.contains("0 rows"));
    let json = run(&["instances", "list", "--json"]);
    assert!(json.status.success());
    let show_missing = run(&["instances", "show", "99"]);
    assert!(!show_missing.status.success());
    let pin_missing = run(&["instances", "pin", "99"]);
    assert!(!pin_missing.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn config_show_masks_every_secret() {
    let dir = temp_dir("cli-config");
    let mut command = Command::new(binary());
    command.args(["config", "show", "--json"]);
    for (key, value) in env_dir(&dir) {
        command.env(key, value);
    }
    command.env("OWNSEARCH_BEARER_TOKENS", "a".repeat(40));
    let output = command.output().expect("run");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("bearer_tokens"));
    assert!(!text.contains(&"a".repeat(40)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audit_reports_a_missing_ledger() {
    let dir = temp_dir("cli-audit");
    let mut command = Command::new(binary());
    command.args(["audit", "verify"]);
    for (key, value) in env_dir(&dir) {
        command.env(key, value);
    }
    let output = command.output().expect("run");
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("cannot read"));
    let rotate = {
        let mut command = Command::new(binary());
        command.args(["audit", "rotate"]);
        for (key, value) in env_dir(&dir) {
            command.env(key, value);
        }
        command.output().expect("run")
    };
    assert!(!rotate.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn db_backup_refuses_a_missing_database() {
    let dir = temp_dir("cli-backup-missing");
    let mut command = Command::new(binary());
    command.args(["db", "backup"]);
    for (key, value) in env_dir(&dir) {
        command.env(key, value);
    }
    let output = command.output().expect("run");
    assert!(!output.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bearer_token_with_a_comma_is_refused() {
    use std::io::Write as _;
    use std::process::Stdio;
    let dir = temp_dir("cli-token");
    let mut command = Command::new(binary());
    command.args(["config", "set-bearer-token"]);
    for (key, value) in env_dir(&dir) {
        command.env(key, value);
    }
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn");
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(format!("{},{}", "a".repeat(40), "b".repeat(40)).as_bytes())
            .expect("write");
    }
    let output = child.wait_with_output().expect("wait");
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("comma"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dry_run_scan_reports_without_persisting_a_run() {
    let dir = temp_dir("scan-dry");
    let output = Command::new(binary())
        .args(["scan", "--dry-run", "--limit", "1"])
        .envs(env_dir(&dir))
        .env(
            "OWNSEARCH_DISCOVERY_URL",
            "http://127.0.0.1:1/instances.json",
        )
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("dry run"), "{text}");
    assert!(text.contains("discovered"), "{text}");
    assert!(text.contains("fleet:"), "{text}");
    assert!(text.contains("elapsed:"), "{text}");
}

#[test]
fn a_json_scan_prints_a_report() {
    let dir = temp_dir("scan-json");
    let output = Command::new(binary())
        .args(["scan", "--dry-run", "--limit", "1", "--json"])
        .envs(env_dir(&dir))
        .env(
            "OWNSEARCH_DISCOVERY_URL",
            "http://127.0.0.1:1/instances.json",
        )
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is json");
    assert!(parsed.get("discovered").is_some(), "{parsed}");
    assert!(parsed.get("probed").is_some(), "{parsed}");
}
