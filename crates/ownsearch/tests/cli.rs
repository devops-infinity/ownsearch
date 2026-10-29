#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod common;

use std::process::Command;

use common::temp_dir;

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
    assert!(text.contains("schema version: 4"));
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
fn db_verify_creates_and_seeds_a_missing_database() {
    let dir = temp_dir("cli-db-init");
    let mut command = Command::new(binary());
    command.args(["db", "verify"]);
    for (key, value) in env_dir(&dir) {
        command.env(key, value);
    }
    let output = command.output().expect("run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("instances: 67"), "{text}");
    assert!(dir.join("state").join("instances.db").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_state_dir_flag_is_accepted_after_a_subcommand() {
    let dir = temp_dir("cli-global-dir");
    let target = dir.join("elsewhere");
    let output = Command::new(binary())
        .args(["instances", "list", "--state-dir"])
        .arg(&target)
        .env("HOME", "/home/tester")
        .env("OWNSEARCH_STATE_DIR", dir.join("ignored"))
        .env("OWNSEARCH_LEDGER_DIR", dir.join("ledger"))
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(target.join("instances.db").exists());
    assert!(!dir.join("ignored").join("instances.db").exists());
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
    let list = run(&["instances", "list", "--limit", "500"]);
    assert!(list.status.success());
    let text = String::from_utf8_lossy(&list.stdout);
    assert!(
        text.contains("67 rows"),
        "a fresh database must seed the embedded fleet: {text}"
    );
    let json = run(&["instances", "list", "--json"]);
    assert!(json.status.success());
    let show_missing = run(&["instances", "show", "9999"]);
    assert!(!show_missing.status.success());
    let pin_missing = run(&["instances", "pin", "9999"]);
    assert!(!pin_missing.status.success());
    let add = run(&["instances", "add", "http://127.0.0.1:8888/", "--json"]);
    assert!(add.status.success());
    let added: serde_json::Value = serde_json::from_slice(&add.stdout).expect("json");
    let added_id = added
        .get("instanceId")
        .and_then(serde_json::Value::as_i64)
        .expect("instance id")
        .to_string();
    let retire = run(&["instances", "retire", &added_id, "--reason", "tidy up"]);
    assert!(retire.status.success());
    let shown = run(&["instances", "show", &added_id, "--json"]);
    let row: serde_json::Value = serde_json::from_slice(&shown.stdout).expect("json");
    assert_eq!(
        row.get("status").and_then(serde_json::Value::as_str),
        Some("retired")
    );
    let restore = run(&["instances", "restore", &added_id]);
    assert!(restore.status.success());
    let shown = run(&["instances", "show", &added_id, "--json"]);
    let row: serde_json::Value = serde_json::from_slice(&shown.stdout).expect("json");
    assert_eq!(
        row.get("status").and_then(serde_json::Value::as_str),
        Some("candidate")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn release_server(body: &'static str) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

#[test]
fn the_update_command_reports_a_newer_release() {
    let dir = temp_dir("cli-update");
    let (base, server) = release_server(r#"{"tag_name":"v99.0.0","assets":[]}"#);
    let json = Command::new(binary())
        .args(["update", "--check", "--json"])
        .env("OWNSEARCH_UPDATE_API_BASE", &base)
        .envs(env_dir(&dir))
        .output()
        .expect("run");
    let _ = server.join();
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).expect("json");
    assert_eq!(
        value.get("latest").and_then(serde_json::Value::as_str),
        Some("99.0.0")
    );
    assert_eq!(
        value.get("available").and_then(serde_json::Value::as_bool),
        Some(true)
    );

    let (base, server) = release_server(r#"{"tag_name":"v99.0.0","assets":[]}"#);
    let text = Command::new(binary())
        .args(["update", "--check"])
        .env("OWNSEARCH_UPDATE_API_BASE", &base)
        .envs(env_dir(&dir))
        .output()
        .expect("run");
    let _ = server.join();
    assert!(text.status.success());
    assert!(
        String::from_utf8_lossy(&text.stdout).contains("99.0.0 is available"),
        "{}",
        String::from_utf8_lossy(&text.stdout)
    );
    let (base, server) = release_server(r#"{"tag_name":"v0.0.1","assets":[]}"#);
    let current = Command::new(binary())
        .args(["update", "--check"])
        .env("OWNSEARCH_UPDATE_API_BASE", &base)
        .envs(env_dir(&dir))
        .output()
        .expect("run");
    let _ = server.join();
    assert!(current.status.success());
    assert!(
        String::from_utf8_lossy(&current.stdout).contains("is the latest release"),
        "{}",
        String::from_utf8_lossy(&current.stdout)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn update_run(dir: &std::path::Path, args: &[&str], base: &str) -> std::process::Output {
    Command::new(binary())
        .args(args)
        .env("OWNSEARCH_UPDATE_API_BASE", base)
        .envs(env_dir(dir))
        .output()
        .expect("run")
}

#[test]
fn the_update_command_refuses_an_api_error() {
    let dir = temp_dir("cli-update-403");
    let (base, server) = release_server_error(403);
    let output = update_run(&dir, &["update", "--check"], &base);
    let _ = server.join();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("GH_TOKEN"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_update_command_refuses_a_malformed_release() {
    let dir = temp_dir("cli-update-html");
    let (base, server) = release_server("<html>not json</html>");
    let output = update_run(&dir, &["update", "--check"], &base);
    let _ = server.join();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("did not parse"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_update_command_refuses_a_release_without_this_platform_asset() {
    let dir = temp_dir("cli-update-noasset");
    let (base, server) = release_server(r#"{"tag_name":"v99.0.0","assets":[]}"#);
    let output = update_run(&dir, &["update"], &base);
    let _ = server.join();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("releases/latest"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn release_server_error(status: u16) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "HTTP/1.1 {status} Error\r\ncontent-type: application/json\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

#[test]
fn the_allow_and_events_commands_show_the_policy_trail() {
    let dir = temp_dir("cli-instances-policy");
    let run = |args: &[&str]| {
        Command::new(binary())
            .args(args)
            .envs(env_dir(&dir))
            .output()
            .expect("run")
    };
    let add = run(&["instances", "add", "http://127.0.0.1:8888/", "--json"]);
    let added: serde_json::Value = serde_json::from_slice(&add.stdout).expect("json");
    let id = added
        .get("instanceId")
        .and_then(serde_json::Value::as_i64)
        .expect("id")
        .to_string();
    assert!(run(&["instances", "deny", &id]).status.success());
    let allowed = run(&["instances", "allow", &id]);
    assert!(allowed.status.success());
    assert!(
        String::from_utf8_lossy(&allowed.stdout).contains("scanning is enabled"),
        "{}",
        String::from_utf8_lossy(&allowed.stdout)
    );
    let shown = run(&["instances", "show", &id, "--json"]);
    let row: serde_json::Value = serde_json::from_slice(&shown.stdout).expect("json");
    assert_eq!(
        row.get("denied").and_then(serde_json::Value::as_bool),
        Some(false)
    );
    let typed = run(&["instances", "set-type", &id, "tor"]);
    assert!(typed.status.success());
    assert!(
        String::from_utf8_lossy(&typed.stdout).contains("excluded"),
        "{}",
        String::from_utf8_lossy(&typed.stdout)
    );
    let back = run(&["instances", "set-type", &id, "normal"]);
    assert!(back.status.success());
    assert!(
        String::from_utf8_lossy(&back.stdout).contains("candidate"),
        "{}",
        String::from_utf8_lossy(&back.stdout)
    );
    let events = run(&["instances", "events", &id, "--limit", "5"]);
    assert!(events.status.success());
    let text = String::from_utf8_lossy(&events.stdout);
    assert!(text.contains("policy_changed"), "{text}");
    assert!(text.contains("events"), "{text}");
    let json = run(&["instances", "events", &id, "--json"]);
    assert!(json.status.success());
    let rows: serde_json::Value = serde_json::from_slice(&json.stdout).expect("json");
    assert!(rows.as_array().is_some_and(|items| !items.is_empty()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_update_command_refuses_a_package_managed_copy() {
    let dir = temp_dir("cli-update-brew");
    let target = dir
        .join("homebrew")
        .join("Cellar")
        .join("ownsearch")
        .join("0.0.0")
        .join("bin");
    std::fs::create_dir_all(&target).expect("dir");
    let copied = target.join("ownsearch");
    std::fs::copy(binary(), &copied).expect("copy");
    let (base, server) = release_server(r#"{"tag_name":"v99.0.0","assets":[]}"#);
    let check = Command::new(&copied)
        .args(["update", "--check"])
        .env("OWNSEARCH_UPDATE_API_BASE", &base)
        .envs(env_dir(&dir))
        .output()
        .expect("run");
    let _ = server.join();
    assert!(check.status.success());
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("brew upgrade ownsearch"),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
    let apply = Command::new(&copied)
        .args(["update"])
        .env("OWNSEARCH_UPDATE_API_BASE", &base)
        .envs(env_dir(&dir))
        .output()
        .expect("run");
    assert!(!apply.status.success());
    assert!(
        String::from_utf8_lossy(&apply.stderr).contains("brew upgrade ownsearch"),
        "{}",
        String::from_utf8_lossy(&apply.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_update_command_sends_a_github_token_when_one_is_set() {
    let dir = temp_dir("cli-update-token");
    let (base, server) = release_server(r#"{"tag_name":"v0.0.1","assets":[]}"#);
    let output = Command::new(binary())
        .args(["update", "--check", "--json"])
        .env("OWNSEARCH_UPDATE_API_BASE", &base)
        .env("GH_TOKEN", "test-token")
        .envs(env_dir(&dir))
        .output()
        .expect("run");
    let _ = server.join();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_audit_rotate_force_archives_a_chain_it_cannot_verify() {
    let dir = temp_dir("cli-audit-force");
    let ledger_dir = dir.join("ledger");
    std::fs::create_dir_all(&ledger_dir).expect("dir");
    std::fs::write(ledger_dir.join("calls.jsonl"), "{\"row\":1}\n").expect("write");
    let output = Command::new(binary())
        .args(["audit", "rotate", "--force"])
        .envs(env_dir(&dir))
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("without verification"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!ledger_dir.join("calls.jsonl").exists());
    let archived = std::fs::read_dir(&ledger_dir)
        .expect("dir")
        .filter_map(|entry| entry.ok())
        .any(|entry| entry.file_name().to_string_lossy().contains("-unverified"));
    assert!(archived, "no unverified archive was written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_db_restore_command_replaces_a_corrupt_database() {
    let dir = temp_dir("cli-db-restore");
    let run = |args: &[&str]| {
        Command::new(binary())
            .args(args)
            .envs(env_dir(&dir))
            .output()
            .expect("run")
    };
    assert!(run(&["instances", "list"]).status.success());
    let backup = dir.join("backup.db");
    let made = Command::new(binary())
        .args(["db", "backup", "--out"])
        .arg(&backup)
        .envs(env_dir(&dir))
        .output()
        .expect("run");
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let live = dir.join("state").join("instances.db");
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", live.display()));
    }
    std::fs::write(&live, b"not a database").expect("corrupt");
    assert!(!run(&["db", "verify"]).status.success());
    let restored = Command::new(binary())
        .args(["db", "restore", "--from"])
        .arg(&backup)
        .envs(env_dir(&dir))
        .output()
        .expect("run");
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    let verify = run(&["db", "verify"]);
    assert!(verify.status.success());
    assert!(
        String::from_utf8_lossy(&verify.stdout).contains("instances: 67"),
        "{}",
        String::from_utf8_lossy(&verify.stdout)
    );
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
    let settings: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert!(settings["scan_concurrency"].is_number(), "{text}");
    assert!(settings["http"].is_boolean(), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_state_dir_flag_is_read_before_the_subcommand() {
    let dir = temp_dir("cli-flag-order");
    let state = dir.join("state");
    let output = {
        let mut command = Command::new(binary());
        command.args(["--state-dir", &state.display().to_string(), "db", "path"]);
        command.env("OWNSEARCH_OFFLINE", "true");
        command.output().expect("run")
    };
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains(&state.display().to_string()), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_ledger_is_reported_as_empty() {
    let dir = temp_dir("cli-audit");
    let mut command = Command::new(binary());
    command.args(["audit", "verify"]);
    for (key, value) in env_dir(&dir) {
        command.env(key, value);
    }
    let output = command.output().expect("run");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("no ledger yet"));
    assert!(text.contains("nothing to verify"));
    assert!(!dir.join("ledger").join("args.key").exists());
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
fn an_instance_can_be_added_by_url_and_the_add_is_idempotent() {
    let dir = temp_dir("instance-add");
    let run = || {
        Command::new(binary())
            .args(["instances", "add", "http://127.0.0.1:8888/", "--json"])
            .envs(env_dir(&dir))
            .output()
            .expect("run")
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let row: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(
        row.get("url").and_then(serde_json::Value::as_str),
        Some("http://127.0.0.1:8888/")
    );
    assert_eq!(
        row.get("status").and_then(serde_json::Value::as_str),
        Some("candidate")
    );
    let again = run();
    assert!(again.status.success());
    let second: serde_json::Value = serde_json::from_slice(&again.stdout).expect("json");
    assert_eq!(
        second.get("instanceId").and_then(serde_json::Value::as_i64),
        row.get("instanceId").and_then(serde_json::Value::as_i64)
    );
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
