//! The JSON-stream handshake contract: `ready` is the FIRST frame.
//!
//! A host reads line 1 of stdout as the handshake — `release_binary_smoke`
//! does, `harness_regression::r012` does, and the Desktop app contract implies
//! it. On Windows that contract was broken on every single session: the
//! `windows_job_object` local-shell notice reached `emit_info` inside
//! `AgentBootstrap::build` before `ready` existed, so line 1 was an `info`
//! frame and three release tests read a diagnostic as their handshake.
//!
//! # Why this file exists next to the sink's own unit tests
//!
//! `protocol_sink.rs` proves the GATE holds frames correctly. It cannot prove
//! the json-stream entry point actually ARMS it — a unit-tested guard that no
//! call site invokes is the failure mode this repo has shipped before. This
//! test spawns the real binary and reads real stdout, so it grades the wiring.
//!
//! It runs against the debug binary Cargo wires through `CARGO_BIN_EXE_`, so it
//! is part of every `cargo nextest run` and needs no release pre-build.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

/// Drive `--json-stream` and return the first `frame_count` stdout lines,
/// parsed. Mirrors the isolation recipe in `release_binary_smoke.rs`:
/// `HOME` alone does NOT isolate on Windows (`dirs::home_dir()` reads
/// `USERPROFILE` there), so `WAYLAND_HOME` — the crate's canonical hermetic
/// override — is set too.
fn first_frames(frame_count: usize) -> Vec<serde_json::Value> {
    let tmp = TempDir::new().expect("create tmp workspace");

    let mut child = OwnedTree::new(
        Command::new(env!("CARGO_BIN_EXE_wayland-core"))
            .args([
                "--json-stream",
                "--provider",
                "anthropic",
                "--api-key",
                "test-key-not-used-because-we-stop-before-message",
            ])
            .current_dir(tmp.path())
            .env("HOME", tmp.path())
            .env("WAYLAND_HOME", tmp.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn wayland-core --json-stream"),
    );

    let mut stdout = child.stdout.take().expect("capture stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(&mut stdout);
        let mut lines = Vec::new();
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    lines.push(l);
                    if lines.len() >= frame_count {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(lines);
    });

    let lines = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("wayland-core did not produce stdout within 60s");

    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                break;
            }
        }
    }

    assert!(
        !lines.is_empty(),
        "wayland-core --json-stream closed stdout without emitting a single frame"
    );
    lines
        .iter()
        .map(|l| {
            serde_json::from_str(l)
                .unwrap_or_else(|e| panic!("stdout line was not JSON ({e}): {l:?}"))
        })
        .collect()
}

/// The contract, graded on the real process on whatever platform runs it.
///
/// Red on Windows before the `deferring_info_until_ready` gate landed: the
/// local-shell notice took line 1. Green on Linux/macOS both before and after,
/// where the shipping backends (bwrap / sandbox_exec) enforce read-deny so that
/// notice never fires — this is the guard that keeps them that way, and the one
/// that catches the next boot-time emitter that tries to speak before the
/// handshake.
#[test]
fn ready_is_the_first_frame_of_the_json_stream() {
    let frames = first_frames(1);

    assert_eq!(
        frames[0]["type"], "ready",
        "the host reads frame 1 as the handshake; got: {}",
        frames[0]
    );
}

/// A diagnostic emitted during bootstrap must be DEFERRED, not DROPPED.
///
/// The cheapest wrong fix for the ordering bug is to stop emitting the notice.
/// That would pass the test above and silently delete a security-relevant
/// warning, so the frames after the handshake are graded too: whatever this
/// platform's bootstrap wanted to say still reaches the host.
///
/// Deliberately asserted as "the stream continues past `ready` and every frame
/// is well-formed" rather than pinning a specific notice — the notice set is
/// platform-dependent (Windows emits the `windows_job_object` one, Linux and
/// macOS emit none), and pinning it would make this test assert nothing on two
/// of the three platforms.
#[test]
fn frames_after_the_handshake_are_still_delivered() {
    let frames = first_frames(2);

    assert_eq!(frames[0]["type"], "ready");
    assert!(
        frames.len() >= 2,
        "the stream must continue past the handshake; got only: {frames:?}"
    );
    assert!(
        frames[1]["type"].is_string(),
        "frame 2 must be a well-formed protocol event: {}",
        frames[1]
    );
    assert_ne!(
        frames[1]["type"], "ready",
        "the handshake must be emitted exactly once"
    );
}
