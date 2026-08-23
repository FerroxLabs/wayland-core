//! #1099 REACHABILITY — the path-boundary card, driven on a real TUI.
//!
//! ## What this file is for
//!
//! Core learned to force the approval gate when a read names a path outside
//! every root the session can reach, and to hand the host the folder a grant
//! would open (`ToolEscalation::PathBoundary`). Nothing in
//! `crates/wcore-cli/src/tui/**` could answer it: `ApprovalScope::AlwaysPath`
//! appeared ZERO times there. So the shipped terminal offered the user a
//! prompt with no working answer — `y` interrupted them and then failed with
//! the very `path ... is outside sandbox root ...` error the feature exists to
//! remove, and `a` re-asked on every subsequent call forever, because the
//! boundary check forces the gate past a tool-name grant.
//!
//! A unit test on the key handler would restate the fix, not close it. This
//! drives the SHIPPED BINARY on a pseudo-terminal: a scripted model asks to
//! `Read` a file outside the workspace, the card renders, a human presses one
//! key, and the assertion is that the READ RETURNED THE FILE — read back off
//! the tool_result the engine POSTs to the mock provider, not off a log line.
//!
//! ## Why `#![cfg(unix)]`
//!
//! Same as every other PTY smoke in this crate: `portable_pty`'s ConPTY
//! backend on a headless Windows runner does not surface the child's stdout to
//! the master end, so the vt100 grid stays empty and every wait times out. The
//! Windows terminal leg of this behaviour is NOT measured here.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;

use support::pty::{Pty, write_config};

/// The unique line inside the out-of-workspace file. It can only reach the
/// provider if the `Read` actually ran and returned content.
const OUTSIDE_TOKEN: &str = "WAYLAND_OUTSIDE_FILE_CONTENT_OK";

/// The scripted closing turn, served only after a tool_result is POSTed back.
const DONE_TOKEN: &str = "WAYLAND_BOUNDARY_TURN_DONE";

/// Create the file the model will ask for, in a directory that is NOT the
/// session workspace and NOT `$HOME` (a grant on `$HOME` is refused by the
/// policy, which would make this prove the wrong thing).
fn outside_file() -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    // A nested folder so `suggested_root` is a real containing directory
    // rather than the tempdir root itself.
    let reports = dir.path().join("reports");
    std::fs::create_dir_all(&reports).expect("create reports dir");
    let file = reports.join("q3.md");
    std::fs::write(&file, format!("{OUTSIDE_TOKEN}\n")).expect("write outside file");
    // Canonicalized, because that is the form Core classifies and echoes back
    // as `suggested_root` — on macOS `/var` is a symlink to `/private/var`.
    let root = reports.canonicalize().expect("canonicalize reports dir");
    let file = file.canonicalize().expect("canonicalize outside file");
    (dir, root, file)
}

/// Every tool_result body the engine POSTed back to the provider, flattened to
/// text. This is where "did the read succeed" is answered: the transcript can
/// show a card and a continuation for a FAILED tool just as readily.
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

/// What the mock provider actually received, in one line, for the moment a
/// wait gives up.
///
/// FerroxLabs/wayland#1109: `approving_once_leaves_the_read_refused_by_the_sandbox`
/// timed out at 30s on macOS inside the full suite (3 of the last 7 legs), and
/// every investigation of it stalled on the same ambiguity — the harness could
/// see the terminal and nothing else, so a timeout could not say whether the
/// engine never dispatched the closing turn or whether it dispatched, was
/// answered, and the answer never reached the screen. Those are different
/// components with different fixes. This is the fact that separates them, and
/// it is read out of the mock server itself rather than off a log line.
fn provider_traffic(rt: &tokio::runtime::Runtime, server: &wiremock::MockServer) -> String {
    let bodies: Vec<Value> = rt
        .block_on(support::mock_llm::received_requests(server))
        .into_iter()
        .map(|r| r.body)
        .collect();
    let with_tool_result = bodies
        .iter()
        .filter(|body| {
            body.get("messages")
                .and_then(Value::as_array)
                .is_some_and(|messages| {
                    messages.iter().any(|message| {
                        message
                            .get("content")
                            .and_then(Value::as_array)
                            .is_some_and(|blocks| {
                                blocks.iter().any(|block| {
                                    block.get("type").and_then(Value::as_str) == Some("tool_result")
                                })
                            })
                    })
                })
        })
        .count();
    format!(
        "--- provider traffic ---\n\
         the mock provider received {} request(s); {} of them carried a tool_result.\n\
         0 requests                       = the engine never dispatched a turn at all.\n\
         1 request, no tool_result        = turn 1 went out and the tool has not been answered.\n\
         a request carrying a tool_result = the CLOSING turn was dispatched, the mock answered it \
         (its script replays the last turn forever), and the answer is what failed to reach the \
         screen.\n\
         --- end ---",
        bodies.len(),
        with_tool_result
    )
}

/// THE proof. A model asks to read a file outside the workspace; the card
/// offers the containing folder by name; `a` grants it; the read succeeds.
///
/// Every assertion here fails on the tree this test was written against:
/// the key row said "always in this workspace" (a false label — the path is
/// precisely not in this workspace), and `a` sent a bare `Always`, which the
/// boundary check overrides, so the tool never ran and no tool_result was ever
/// POSTed.
#[test]
fn a_on_the_boundary_card_grants_the_folder_and_the_read_succeeds() {
    let home = TempDir::new().expect("tempdir");
    let (_outside, root, file) = outside_file();
    let file_arg = file.to_str().expect("utf-8 path").to_string();
    let root_label = root.to_str().expect("utf-8 path").to_string();

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

    // 200 columns: the key row carries an absolute folder path, and a width
    // that clips it would turn a real acceptance into a timeout.
    let mut pty = Pty::spawn_with_env(home.path(), 40, 200, &[] as &[(&str, &str)]);
    pty.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(60),
        "TUI to render the chrome wordmark and Workspace tab",
    );

    pty.send(b"read the quarterly report\r");

    // 1. The gate is FORCED even though `Read` is on the default auto-approve
    //    allow-list — that allow-list grants the tool, not the path.
    pty.wait_for(
        |s| s.contains("approve") && s.contains("deny"),
        Duration::from_secs(30),
        "the approval card to render for the out-of-workspace Read",
    );

    // 2. The affordance under test: the card must name the folder `a` grants.
    //    A button labelled "always" that does not say what it opens is a
    //    button lying about its own scope.
    pty.wait_for(
        |s| s.contains(&format!("always allow {root_label}")),
        Duration::from_secs(10),
        "the card to offer the containing folder BY NAME",
    );
    let card_screen = pty.screen_text();
    println!("--- boundary card ---\n{card_screen}\n--- end ---");

    // 3. Answer it.
    pty.send(b"a");

    // 4. The turn continues — which only happens once a tool_result is POSTed.
    if let Err(report) = pty.try_wait_for(
        |s| s.contains(DONE_TOKEN),
        Duration::from_secs(30),
        "the turn to continue after the granted read",
    ) {
        let traffic = provider_traffic(&rt, &server);
        let alive = pty.child_is_running();
        panic!("{report}\nchild still running: {alive}\n{traffic}");
    }
    let after = pty.screen_text();
    println!("--- after the grant ---\n{after}\n--- end ---");
    pty.quit();

    // 5. HARD PROOF the read RETURNED THE FILE. A denied or refused tool also
    //    produces a tool_result and also lets the turn continue, so the
    //    continuation text alone proves nothing about the read.
    let bodies: Vec<Value> = rt
        .block_on(support::mock_llm::received_requests(&server))
        .into_iter()
        .map(|r| r.body)
        .collect();
    let results = tool_result_text(&bodies);
    assert!(
        results.contains(OUTSIDE_TOKEN),
        "the granted Read must return the file's contents; tool_results were:\n{results}"
    );
    assert!(
        !results.contains("outside sandbox"),
        "the granted Read must not still be refused by the jail; tool_results were:\n{results}"
    );
}

/// NEGATIVE CONTROL. Same harness, same file, same keystroke — but the user
/// answers `y` (approve once), which mints no grant.
///
/// Without this the test above would be evidence that the read works after an
/// approval, not that it works *because of the folder grant*. It also pins the
/// honesty of the docs: `once` does NOT run the call, and the protocol spec
/// must not promise that it does.
#[test]
fn approving_once_leaves_the_read_refused_by_the_sandbox() {
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

    let mut pty = Pty::spawn_with_env(home.path(), 40, 200, &[] as &[(&str, &str)]);
    pty.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(60),
        "TUI to render the chrome wordmark and Workspace tab",
    );
    pty.send(b"read the quarterly report\r");
    pty.wait_for(
        |s| s.contains("approve") && s.contains("deny"),
        Duration::from_secs(30),
        "the approval card to render for the out-of-workspace Read",
    );
    pty.send(b"y");
    if let Err(report) = pty.try_wait_for(
        |s| s.contains(DONE_TOKEN),
        Duration::from_secs(30),
        "the turn to continue after the once-approved read",
    ) {
        let traffic = provider_traffic(&rt, &server);
        let alive = pty.child_is_running();
        panic!("{report}\nchild still running: {alive}\n{traffic}");
    }
    println!(
        "--- after approve-once ---\n{}\n--- end ---",
        pty.screen_text()
    );
    pty.quit();

    let bodies: Vec<Value> = rt
        .block_on(support::mock_llm::received_requests(&server))
        .into_iter()
        .map(|r| r.body)
        .collect();
    let results = tool_result_text(&bodies);
    assert!(
        results.contains("outside sandbox"),
        "approve-once mints no grant, so the read must still be refused; \
         tool_results were:\n{results}"
    );
    assert!(
        !results.contains(OUTSIDE_TOKEN),
        "approve-once must NOT return the file; tool_results were:\n{results}"
    );
}
