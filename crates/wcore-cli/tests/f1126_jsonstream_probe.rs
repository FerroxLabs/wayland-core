//! FerroxLabs/wayland#1126 — LANE-ONLY DIAGNOSTIC. NOT FOR MERGE.
//!
//! Is the #1126 wedge reachable OUTSIDE the TUI?
//!
//! The TUI probe established that a turn can stop dead after the provider has
//! already answered. If that is an engine property it reaches `--json-stream`
//! too, which is the surface the Wayland desktop host drives — a much larger
//! blast radius than one quarantined test. If it is a TUI property, json-stream
//! is clean and the finding stays inside `wcore-cli/src/tui/**`.
//!
//! This drives the SAME scenario over `--json-stream`: same mock script, same
//! out-of-workspace `Read`, the boundary gate answered approve-once
//! (`{"type":"tool_approve","call_id":…}` — `scope` serde-defaults to `Once`).
//!
//! Headless (`--no-tui`) is deliberately NOT the third arm: it installs no
//! approval manager, so `needs_approval` never consults `path_boundary` and the
//! gate is not forced there at all. A "no gate appeared" result on that surface
//! would be ordinary behaviour, not a reproduction.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;

use support::pty::{harden_child_env, write_config};

const OUTSIDE_TOKEN: &str = "WAYLAND_OUTSIDE_FILE_CONTENT_OK";
const DONE_TOKEN: &str = "WAYLAND_BOUNDARY_TURN_DONE";

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_wayland-core"))
}

fn outside_file() -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let reports = dir.path().join("reports");
    std::fs::create_dir_all(&reports).expect("create reports dir");
    let file = reports.join("q3.md");
    std::fs::write(&file, format!("{OUTSIDE_TOKEN}\n")).expect("write outside file");
    let root = reports.canonicalize().expect("canonicalize reports dir");
    let file = file.canonicalize().expect("canonicalize outside file");
    (dir, root, file)
}

fn tool_result_text(bodies: &[Value]) -> String {
    let mut out = String::new();
    for body in bodies {
        let Some(messages) = body.get("messages").and_then(Value::as_array) else {
            continue;
        };
        for message in messages {
            let Some(blocks) = message.get("content").and_then(Value::as_array) else {
                continue;
            };
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                    out.push_str(
                        &block
                            .get("content")
                            .map(Value::to_string)
                            .unwrap_or_default(),
                    );
                    out.push('\n');
                }
            }
        }
    }
    out
}

#[test]
fn f1126_probe_the_same_turn_over_json_stream() {
    let home = TempDir::new().expect("tempdir");
    let (_outside, _root, file) = outside_file();
    let file_arg = file.to_str().expect("utf-8 path").to_string();

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(
        support::mock_llm::MockLlm::new()
            .tool_use("Read", serde_json::json!({ "file_path": file_arg }))
            .text(DONE_TOKEN)
            .start(),
    );
    write_config(
        home.path(),
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(&server.uri()),
    );

    let mut command = Command::new(binary());
    command
        .args(["--json-stream", "--provider", "anthropic"])
        .current_dir(home.path());
    harden_child_env(&mut command, home.path());
    command.env(
        "RUST_LOG",
        "info,wcore_agent=trace,wcore_providers=trace,wcore_cli=trace",
    );
    // Without an ephemeral encrypted vault the binary refuses to start a
    // session under a hermetic WAYLAND_HOME, so every observation would be an
    // absence rather than a verdict.
    let guard = support::vault::configure_process(&mut command);
    let spawned = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    drop(guard);
    let mut child = spawned.expect("the shipped binary must spawn");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            // stderr is NOT a terminal the subject reads; this is a plain pipe.
            eprintln!("[child stderr] {line}");
        }
    });

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let _ = writeln!(
        stdin,
        "{{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"read the quarterly report\"}}"
    );

    let started = Instant::now();
    let budget = Duration::from_secs(120);
    let mut frames: Vec<String> = Vec::new();
    let mut approved_at: Option<Duration> = None;
    let mut done_at: Option<Duration> = None;
    let mut saw_escalation = false;

    while started.elapsed() < budget {
        let Ok(line) = rx.recv_timeout(Duration::from_millis(500)) else {
            continue;
        };
        frames.push(line.clone());
        let parsed: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind = parsed.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "tool_request"
            && parsed
                .pointer("/tool/escalation/kind")
                .and_then(Value::as_str)
                == Some("path_boundary")
        {
            saw_escalation = true;
        }
        if kind == "approval_required" && approved_at.is_none() {
            let call_id = parsed
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            // Approve ONCE — `scope` omitted serde-defaults to Once, which is
            // the json-stream spelling of the TUI's `y`.
            let _ = writeln!(
                stdin,
                "{{\"type\":\"tool_approve\",\"call_id\":\"{call_id}\"}}"
            );
            approved_at = Some(started.elapsed());
        }
        if line.contains(DONE_TOKEN) {
            done_at = Some(started.elapsed());
            break;
        }
    }

    let bodies: Vec<Value> = rt
        .block_on(async {
            tokio::time::timeout(
                Duration::from_secs(5),
                support::mock_llm::received_requests(&server),
            )
            .await
        })
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.body)
        .collect();

    println!("=== f1126 json-stream: approved_at={approved_at:?} done_at={done_at:?} ===");
    println!("path_boundary escalation seen on the wire: {saw_escalation}");
    println!("mock provider received {} request(s)", bodies.len());
    println!("--- frames ({}) ---", frames.len());
    for f in &frames {
        println!("{}", &f[..f.len().min(400)]);
    }
    println!("--- tool_results ---\n{}", tool_result_text(&bodies));
    let log_path = home.path().join("logs").join("wayland-core.log");
    match std::fs::read_to_string(&log_path) {
        Ok(text) => println!("--- child log ({} bytes) ---\n{text}", text.len()),
        Err(e) => println!("--- child log UNREADABLE: {e} ---"),
    }

    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        approved_at.is_some(),
        "the boundary gate never reached the host over json-stream"
    );
    assert!(
        done_at.is_some(),
        "the closing turn never reached the host over json-stream within {budget:?} \
         — the #1126 wedge is NOT confined to the TUI"
    );
    let results = tool_result_text(&bodies);
    assert!(
        results.contains("outside sandbox"),
        "approve-once mints no grant, so the read must still be refused; got:\n{results}"
    );
    assert!(
        !results.contains(OUTSIDE_TOKEN),
        "approve-once must NOT return the file; got:\n{results}"
    );
}
