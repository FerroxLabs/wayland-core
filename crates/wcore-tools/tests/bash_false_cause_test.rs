//! Foundation W2/W3 gate — the engine must not assert a false cause.
//!
//! When a sandboxed Bash command fails, the engine appends a banner. Measured
//! on macOS and Windows against d6f76c67, that banner said the network was OFF
//! "and that is why it failed" for failures that were purely filesystem or
//! process-launch, and then forbade the model from reporting anything else.
//!
//! These tests drive the REAL `BashTool` through the REAL platform sandbox
//! backend with a workspace policy that denies egress, and grade the annotation
//! the tool actually emitted — not a hand-built `ToolResult`.
//!
//! The negative leg uses a command that matches the product's network heuristic
//! (`git clone`) but whose source is a LOCAL PATH, so no socket is ever opened:
//! whatever fails, egress cannot be the cause.

use serde_json::json;
use serial_test::serial;
use std::sync::Arc;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

/// The claim that is only true when the command's output says the network
/// failed.
const EGRESS_CLAIM: &str = "that is why it failed";

/// Clauses that forbid the model from reporting a cause. None may ever ship.
const FORBIDDING_CLAUSES: &[&str] = &["do NOT claim", "do not invent any other", "Do NOT report"];

/// The marker the true-cause branch writes immediately before the line it
/// selected out of the command's output.
///
/// Its only reader is the Linux-gated quoted-cause leg below, so it carries the
/// same gate: ungated it is dead code on every other target and fails
/// `clippy -D warnings` there.
#[cfg(target_os = "linux")]
const QUOTED_CAUSE_MARKER: &str = "reports a different failure:\n";

/// The command must actually have reached a shell. A pre-exec refusal (the
/// exec-time capability gate) also produces `is_error` with no egress claim in
/// it, which would pass every assertion below while proving nothing.
fn assert_the_shell_actually_ran(content: &str) {
    assert!(
        content.starts_with("Exit code:"),
        "the leg is vacuous unless a shell really ran under the sandbox; got:\n{content}"
    );
}

fn assert_no_forbidding_clause(content: &str) {
    for clause in FORBIDDING_CLAUSES {
        assert!(
            !content.contains(clause),
            "the annotation must never forbid the model from reporting a cause \
             (found {clause:?}):\n{content}"
        );
    }
}

/// A command whose text matches the network heuristic but which never touches
/// the network, run under a real sandbox that denies egress. The failure is
/// filesystem-only, so the annotation must not assert egress as its cause.
#[tokio::test]
#[serial]
async fn filesystem_failure_under_the_real_sandbox_is_not_blamed_on_egress() {
    let dir = tempfile::tempdir().unwrap();
    let policy = Arc::new(WorkspacePolicy::contained(dir.path()));
    assert_eq!(
        policy.network(),
        wcore_sandbox::NetworkPolicy::Deny,
        "the leg is only meaningful with egress denied"
    );
    let ctx = ToolContext::test_default().with_workspace(policy);

    // A local-path clone: `git clone` matches the heuristic, the source is a
    // path outside every granted root, and no socket is opened.
    let result = BashTool
        .execute_with_ctx(
            json!({"command": "git clone /nonexistent-outside-root/repo.git out"}),
            &ctx,
        )
        .await;

    eprintln!("--- tool_result content ---\n{}\n---", result.content);
    assert_the_shell_actually_ran(&result.content);
    assert!(
        result.is_error,
        "the leg needs a FAILING command; got:\n{}",
        result.content
    );
    assert!(
        !result.content.contains(EGRESS_CLAIM),
        "no socket was opened, so egress cannot be asserted as the cause:\n{}",
        result.content
    );
    assert_no_forbidding_clause(&result.content);
}

/// Same, on the streaming path — the annotation is applied there too.
#[tokio::test]
#[serial]
async fn filesystem_failure_on_the_streaming_path_is_not_blamed_on_egress() {
    let dir = tempfile::tempdir().unwrap();
    let policy = Arc::new(WorkspacePolicy::contained(dir.path()));
    let ctx = ToolContext::test_default().with_workspace(policy);

    let result = BashTool
        .execute_streaming_with_ctx(
            json!({"command": "git clone /nonexistent-outside-root/repo.git out"}),
            &ctx,
            &wcore_tools::NullToolOutputSink,
        )
        .await;

    eprintln!(
        "--- streamed tool_result content ---\n{}\n---",
        result.content
    );
    assert_the_shell_actually_ran(&result.content);
    assert!(
        result.is_error,
        "the leg needs a FAILING command; got:\n{}",
        result.content
    );
    assert!(
        !result.content.contains(EGRESS_CLAIM),
        "no socket was opened, so egress cannot be asserted as the cause:\n{}",
        result.content
    );
    assert_no_forbidding_clause(&result.content);
}

/// The other half of the fix, driven end to end: when the OS sandbox really
/// does deny something, the annotation must QUOTE BACK the line the OS wrote.
///
/// Nothing here feeds the expected text in as input. The command asks for a
/// write the sandbox refuses; the kernel and `touch` author the denial line;
/// the assertion then requires that same line to appear inside the annotation,
/// and to have come from the command's own output rather than from the
/// annotation's static prose. Deleting the true-cause branch removes the
/// marker and this test panics.
///
/// LINUX-ONLY: the string a refused write produces is bwrap's ("Read-only file
/// system"); macOS seatbelt and Windows AppContainer word it differently and
/// are covered by the unit legs on their measured strings.
#[cfg(target_os = "linux")]
#[tokio::test]
#[serial]
async fn real_sandbox_write_denial_is_quoted_back_by_the_annotation() {
    if !wcore_tools::bash::platform_enforces_read_deny() {
        eprintln!(
            "SKIP real_sandbox_write_denial_is_quoted_back_by_the_annotation: \
             no enforcing OS sandbox on this host, the denial cannot be provoked"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let policy = Arc::new(WorkspacePolicy::contained(dir.path()));
    let ctx = ToolContext::test_default().with_workspace(policy);

    // `git clone` makes the command match the network heuristic; the local
    // source means no socket is opened; the `touch` provokes a real refusal
    // from the sandbox, which writes its own error text.
    let result = BashTool
        .execute_with_ctx(
            json!({"command":
                "git clone /nonexistent-outside-root/repo.git out; touch /usr/wl-false-cause-probe"}),
            &ctx,
        )
        .await;

    eprintln!("--- quoted-cause content ---\n{}\n---", result.content);
    assert_the_shell_actually_ran(&result.content);
    assert!(
        result.is_error,
        "the leg needs a FAILING command; got:\n{}",
        result.content
    );
    assert!(
        !result.content.contains(EGRESS_CLAIM),
        "no socket was opened, so egress cannot be asserted as the cause:\n{}",
        result.content
    );
    assert_no_forbidding_clause(&result.content);

    // The annotation must name the true cause, and that text must be the OS's,
    // not the annotation's own prose.
    let (before, after) = result
        .content
        .split_once(QUOTED_CAUSE_MARKER)
        .unwrap_or_else(|| {
            panic!(
                "the annotation must quote the cause the sandbox reported; got:\n{}",
                result.content
            )
        });
    let quoted = after.lines().next().unwrap_or_default().trim();
    assert!(
        quoted.contains("/usr/wl-false-cause-probe"),
        "the refused path must be in the quoted cause; quoted {quoted:?} in:\n{}",
        result.content
    );
    assert!(
        quoted.contains("Read-only file system") || quoted.contains("Permission denied"),
        "the quoted cause must be the sandbox's own denial text; quoted {quoted:?} in:\n{}",
        result.content
    );
    assert!(
        before.contains(quoted),
        "the quoted line must have been EXTRACTED from the command's output, \
         not composed by the annotation; quoted {quoted:?} in:\n{}",
        result.content
    );
}

/// POSITIVE CONTROL — a genuine egress denial under the same real sandbox must
/// still be named as the cause. Without this leg, deleting the banner outright
/// would "pass" the negative legs above.
///
/// It runs by default on Linux rather than sitting behind `#[ignore]`: a
/// control that never executes is not a control. `contained()` is
/// `NetworkPolicy::Deny`, so an enforcing bwrap gets `--unshare-net` and the
/// denial is guaranteed on any host where the sandbox spawns at all. The two
/// preconditions it cannot manufacture — an enforcing sandbox, and a `curl` to
/// run — are checked up front and reported loudly.
#[cfg(target_os = "linux")]
#[tokio::test]
#[serial]
async fn genuine_egress_denial_under_the_real_sandbox_is_still_named() {
    if !wcore_tools::bash::platform_enforces_read_deny() {
        eprintln!(
            "SKIP genuine_egress_denial_under_the_real_sandbox_is_still_named: \
             no enforcing OS sandbox on this host, egress cannot be denied"
        );
        return;
    }
    if std::process::Command::new("curl")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!(
            "SKIP genuine_egress_denial_under_the_real_sandbox_is_still_named: \
             no curl on this host, the control cannot provoke a named network failure"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let policy = Arc::new(WorkspacePolicy::contained(dir.path()));
    let ctx = ToolContext::test_default().with_workspace(policy);

    // -sS keeps curl's error message: a silent failure proves nothing here.
    let result = BashTool
        .execute_with_ctx(
            json!({"command": "curl -sS -m 10 https://example.com"}),
            &ctx,
        )
        .await;

    eprintln!("--- egress control content ---\n{}\n---", result.content);
    assert_the_shell_actually_ran(&result.content);
    assert!(
        result.is_error,
        "the control needs egress to actually be denied; got:\n{}",
        result.content
    );
    assert!(
        result.content.contains("network egress is OFF") && result.content.contains(EGRESS_CLAIM),
        "a real egress denial must still be named as the cause:\n{}",
        result.content
    );
    assert_no_forbidding_clause(&result.content);
}
