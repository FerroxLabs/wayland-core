//! FerroxLabs/wayland#1126 — LANE-ONLY DIAGNOSTIC. NOT FOR MERGE.
//!
//! A copy of `path_boundary_tui_pty::approving_once_leaves_the_read_refused_by_the_sandbox`
//! whose final wait is replaced by a bounded OBSERVATION loop: it keeps polling
//! for the closing token for 120 s, and at four points along the way it records
//!
//!   * what the mock provider has actually been sent (bounded by its own 5 s
//!     timeout, so a starved wiremock runtime is itself a reading rather than a
//!     hang in the instrument), and
//!   * an OS-level snapshot of the child `wayland-core` process AND of this test
//!     process: `ps`, every thread, a `sample(1)` stack report, and open files.
//!
//! Everything is written to this test's own stdout. NOTHING is written to the
//! pty the subject is painting into — an `eprintln!` there would corrupt the
//! very screen the harness reads, in every arm including the control.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;

use support::pty::{Pty, write_config};

const OUTSIDE_TOKEN: &str = "WAYLAND_OUTSIDE_FILE_CONTENT_OK";
const DONE_TOKEN: &str = "WAYLAND_BOUNDARY_TURN_DONE";

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

/// What the mock has been sent — with a hard 5 s ceiling on the read itself.
///
/// The ceiling is the point: `received_requests` is served by a task on the
/// runtime wiremock was started on. If that runtime is not being scheduled,
/// an unbounded `block_on` here would hang the instrument and destroy the
/// measurement. A timeout turns that failure mode into a finding.
fn provider_traffic(rt: &tokio::runtime::Runtime, server: &wiremock::MockServer) -> String {
    let recorded = rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            support::mock_llm::received_requests(server),
        )
        .await
    });
    let bodies: Vec<Value> = match recorded {
        Ok(reqs) => reqs.into_iter().map(|r| r.body).collect(),
        Err(_) => {
            return "PROVIDER TRAFFIC UNREADABLE: wiremock::received_requests did not answer \
                    within 5s. The mock server's runtime in THIS process is not being \
                    scheduled.\n"
                .to_string();
        }
    };
    let with_tool_result = bodies
        .iter()
        .filter(|b| {
            b.get("messages")
                .and_then(Value::as_array)
                .is_some_and(|messages| {
                    messages.iter().any(|m| {
                        m.get("content")
                            .and_then(Value::as_array)
                            .is_some_and(|blocks| {
                                blocks.iter().any(|blk| {
                                    blk.get("type").and_then(Value::as_str) == Some("tool_result")
                                })
                            })
                    })
                })
        })
        .count();
    format!(
        "mock provider received {} request(s); {} of them carried a tool_result.\n",
        bodies.len(),
        with_tool_result
    )
}

fn run_cmd(program: &str, args: &[String]) -> String {
    let shown = format!("$ {program} {}", args.join(" "));
    match std::process::Command::new(program).args(args).output() {
        Ok(o) => format!(
            "{shown}\n[status {:?}]\n{}{}\n",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => format!("{shown}\n[spawn failed: {e}]\n"),
    }
}

/// An OS-level snapshot of one process: state, threads, stacks, open files.
fn os_probe(label: &str, pid: Option<u32>) -> String {
    let mut out = format!("\n========== PROBE {label} ==========\n");
    let Some(pid) = pid else {
        out.push_str("no pid available\n");
        return out;
    };
    let p = pid.to_string();
    out.push_str(&run_cmd(
        "/bin/ps",
        &[
            "-o".into(),
            "pid,ppid,stat,%cpu,rss,command".into(),
            "-p".into(),
            p.clone(),
        ],
    ));
    out.push_str(&run_cmd("/bin/ps", &["-M".into(), "-p".into(), p.clone()]));
    let sample_file = std::env::temp_dir().join(format!("f1126-sample-{label}-{p}.txt"));
    out.push_str(&run_cmd(
        "/usr/bin/sample",
        &[
            p.clone(),
            "2".into(),
            "-mayDie".into(),
            "-file".into(),
            sample_file.display().to_string(),
        ],
    ));
    match std::fs::read_to_string(&sample_file) {
        Ok(s) => {
            out.push_str("--- sample(1) report ---\n");
            out.push_str(&s);
            out.push('\n');
        }
        Err(e) => out.push_str(&format!("[sample file unreadable: {e}]\n")),
    }
    out.push_str(&run_cmd(
        "/usr/sbin/lsof",
        &["-nP".into(), "-p".into(), p.clone()],
    ));
    out
}

/// Everything the engine wrote under `WAYLAND_HOME`, and the contents of the
/// session journal.
///
/// The journal is the engine's own record of provider attempts and stream
/// lifecycle. When the process is idle and the provider has already answered,
/// it is the only artifact that can say how far the turn got.
fn durable_state_dump(home: &std::path::Path) -> String {
    fn walk(dir: &std::path::Path, depth: usize, out: &mut Vec<(std::path::PathBuf, u64)>) {
        if depth > 6 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => walk(&path, depth + 1, out),
                Ok(_) => {
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    out.push((path, size));
                }
                Err(_) => {}
            }
        }
    }
    let mut files = Vec::new();
    walk(home, 0, &mut files);
    files.sort();
    let mut out = String::from("\n--- WAYLAND_HOME tree ---\n");
    for (path, size) in &files {
        out.push_str(&format!(
            "{:>10}  {}\n",
            size,
            path.strip_prefix(home).unwrap_or(path).display()
        ));
    }
    for (path, size) in &files {
        let name = path.to_string_lossy();
        let interesting = name.contains("journal")
            || name.contains("session")
            || name.ends_with(".jsonl")
            || name.ends_with(".wal");
        if !interesting || *size > 400_000 {
            continue;
        }
        match std::fs::read(path) {
            Ok(bytes) => out.push_str(&format!(
                "\n--- {} ({size} bytes) ---\n{}\n",
                path.strip_prefix(home).unwrap_or(path).display(),
                String::from_utf8_lossy(&bytes)
            )),
            Err(e) => out.push_str(&format!("\n--- {} UNREADABLE: {e} ---\n", path.display())),
        }
    }
    out
}

#[test]
fn f1126_probe_the_approve_once_stall() {
    println!(
        "{}",
        run_cmd("/usr/sbin/sysctl", &["-n".into(), "hw.logicalcpu".into()])
    );
    println!("test process pid = {}", std::process::id());

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

    // The child's own tracing output. In TUI mode `wayland-core` routes every
    // record to `$WAYLAND_HOME/logs/wayland-core.log` and NOTHING to the
    // terminal (main.rs:1362 — the alt-screen owns stdio), so raising the
    // filter cannot corrupt the screen this harness reads. It is the one
    // channel that can say what the engine was doing when it went quiet.
    let mut pty = Pty::spawn_with_env(
        home.path(),
        40,
        200,
        &[
            (
                "RUST_LOG",
                "info,wcore_agent=trace,wcore_providers=trace,wcore_tools=debug",
            ),
            ("RUST_BACKTRACE", "1"),
        ],
    );
    println!("child pid = {:?}", pty.child_pid());
    pty.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(60),
        "TUI to render the chrome wordmark and Workspace tab",
    );
    pty.send(b"read the quarterly report\r");
    pty.wait_for_ctx(
        |s| s.contains("approve") && s.contains("deny"),
        Duration::from_secs(30),
        "the approval card to render for the out-of-workspace Read",
        || provider_traffic(&rt, &server),
    );
    pty.send(b"y");

    // The observation. 120 s, not 30 — whether this stall ENDS is itself the
    // fact that separates a permanent wedge from something timing out and
    // recovering.
    let started = Instant::now();
    let budget = Duration::from_secs(120);
    let mut probe_at: Vec<u64> = vec![12, 32, 62, 100];
    let mut log = String::new();
    let mut done_at: Option<Duration> = None;
    loop {
        if pty.screen_text().contains(DONE_TOKEN) {
            done_at = Some(started.elapsed());
            break;
        }
        let elapsed = started.elapsed();
        if elapsed >= budget {
            break;
        }
        let due = probe_at.first().copied();
        if let Some(t) = due {
            if elapsed.as_secs() >= t {
                probe_at.remove(0);
                // OS snapshot FIRST: it cannot be blocked by anything inside
                // this process, so it survives even when the reading below
                // cannot be taken.
                log.push_str(&os_probe(&format!("child@{t}s"), pty.child_pid()));
                log.push_str(&os_probe(&format!("test@{t}s"), Some(std::process::id())));
                log.push_str(&format!(
                    "\n[t={}s] {}",
                    elapsed.as_secs(),
                    provider_traffic(&rt, &server)
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    println!("=== f1126 OBSERVATION: DONE_TOKEN at {done_at:?} (budget {budget:?}) ===");
    println!("{log}");
    println!("--- final screen ---\n{}\n--- end ---", pty.screen_text());
    println!("--- final traffic ---\n{}", provider_traffic(&rt, &server));
    println!("{}", durable_state_dump(home.path()));
    let log_path = home.path().join("logs").join("wayland-core.log");
    match std::fs::read_to_string(&log_path) {
        Ok(text) => println!(
            "--- child log ({} bytes) {} ---\n{text}\n--- end child log ---",
            text.len(),
            log_path.display()
        ),
        Err(e) => println!(
            "--- child log UNREADABLE at {}: {e} ---",
            log_path.display()
        ),
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
    println!("--- tool_results ---\n{}", tool_result_text(&bodies));
    pty.quit();

    assert!(
        done_at.is_some(),
        "the closing token never reached the screen within {budget:?}"
    );
}
