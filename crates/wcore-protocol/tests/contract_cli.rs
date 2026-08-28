//! The `wcore-contract` binary's advisory surface, exercised as CI invokes it.
//!
//! `preflight_notice` is unit-tested next to its definition; this file exists
//! because the unit test says nothing about the WIRING - whether the binary
//! reads the changed-file list at all, whether it prints the notice in a form
//! GitHub renders, and above all whether it stays non-gating. A hint that exits
//! non-zero is not a hint, and that is a property of the process, not of the
//! function.

use std::io::Write;
use std::process::{Command, Stdio};

use wcore_protocol::contract::SOURCE_INPUTS;

struct Output {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str], stdin: &str) -> Output {
    run_bytes(args, stdin.as_bytes())
}

/// CI pipes `gh api --jq '.[].filename'` straight into this binary, and nothing
/// in that pipeline promises UTF-8, so the changed-file list is bytes here.
fn run_bytes(args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wcore-contract"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the wcore-contract binary must launch");
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(stdin)
        .expect("the changed-file list must reach the child");
    let finished = child.wait_with_output().expect("the child must exit");
    Output {
        status: finished.status,
        stdout: String::from_utf8_lossy(&finished.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&finished.stderr).into_owned(),
    }
}

#[test]
fn preflight_warns_about_a_source_input_edit_without_ever_failing() {
    let output = run(
        &["preflight"],
        "crates/wcore-agent/src/engine.rs\ncrates/wcore-cli/src/main.rs\ndocs/tools.md\n",
    );
    assert!(
        output.status.success(),
        "the pre-flight hint must never fail the process: {:?}",
        output.status
    );
    assert!(
        output
            .stdout
            .starts_with("::notice title=Desktop contract corpus::"),
        "the hint must be emitted as a GitHub notice annotation: {:?}",
        output.stdout
    );
    assert!(
        output.stdout.contains("crates/wcore-agent/src/engine.rs")
            && output.stdout.contains("source_inputs_digest"),
        "the hint must name the file and the digest that will move: {:?}",
        output.stdout
    );
}

#[test]
fn preflight_is_silent_when_the_change_already_repins_the_corpus() {
    let output = run(
        &["preflight"],
        "crates/wcore-agent/src/engine.rs\ncrates/wcore-protocol/contracts/desktop/v1/manifest.json\n",
    );
    assert!(
        output.status.success(),
        "still non-gating: {:?}",
        output.status
    );
    assert_eq!(
        output.stdout, "",
        "a change that re-pins the corpus must produce no annotation"
    );
}

/// The control for the test above: same binary, same invocation, one path
/// removed. Without it "no annotation" would be satisfied by a `preflight`
/// that never prints anything at all.
#[test]
fn preflight_is_silent_for_a_change_that_touches_no_source_input() {
    let quiet = run(&["preflight"], "docs/tools.md\nREADME.md\n");
    assert_eq!(quiet.stdout, "");
    let loud = run(
        &["preflight"],
        "docs/tools.md\ncrates/wcore-agent/src/engine.rs\n",
    );
    assert!(
        loud.stdout.contains("::notice"),
        "the same invocation DOES annotate when a source input is present: {:?}",
        loud.stdout
    );
}

#[test]
fn source_inputs_prints_exactly_the_paths_the_generator_hashes() {
    let output = run(&["source-inputs"], "");
    assert!(output.status.success());
    assert_eq!(
        output.stdout.lines().collect::<Vec<_>>(),
        SOURCE_INPUTS.to_vec(),
        "CI sources the pre-flight path list from this subcommand; it must be the same list the \
         generator hashes, in the same order, so the list can never be re-hardcoded and go stale"
    );
}

/// ci.yml's step comment claims belt AND braces: `continue-on-error: true` on
/// the step, plus an arm that cannot fail. The second half was untrue when it
/// was written - both reads were `?`, so a single non-UTF-8 byte exited 1. Git
/// paths are not required to be UTF-8 and `gh` emits whatever bytes a filename
/// holds, so this is reachable, not theoretical.
#[test]
fn preflight_survives_a_non_utf8_changed_file_list() {
    let mut bytes = vec![0xff, 0xfe, 0x80];
    bytes.push(b'\n');
    bytes.extend_from_slice(b"crates/wcore-agent/src/engine.rs\n");
    let output = run_bytes(&["preflight"], &bytes);
    assert!(
        output.status.success(),
        "a changed-file list that is not valid UTF-8 must not fail the process - the hint is \
         non-gating by construction, not only because CI wraps it in `continue-on-error`: \
         {:?} stderr={:?}",
        output.status,
        output.stderr
    );
    // Surviving is not enough: "exit 0, print nothing" is also what a preflight
    // arm that had simply stopped working would do. The readable lines must
    // still be classified.
    assert!(
        output.stdout.contains("::notice")
            && output.stdout.contains("crates/wcore-agent/src/engine.rs"),
        "the readable part of the list must still produce the hint: {:?}",
        output.stdout
    );
}

/// The other former `?`: the FILE argument. An unreadable list means no hint,
/// never a failed process and never a wrong hint.
#[test]
fn preflight_survives_a_changed_file_list_that_cannot_be_read() {
    let dir = std::env::temp_dir().join(format!("wcore-contract-preflight-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory for the file list");
    let missing = dir.join("no-such-changed-file-list.txt");
    assert!(
        !missing.exists(),
        "the missing-file arm needs a missing file"
    );

    let absent = run_bytes(
        &["preflight", missing.to_str().expect("utf-8 temp path")],
        b"",
    );
    assert!(
        absent.status.success(),
        "an unreadable changed-file list must not fail the process: {:?} stderr={:?}",
        absent.status,
        absent.stderr
    );
    assert_eq!(
        absent.stdout, "",
        "no list means no hint, never a hint invented from an empty read"
    );
    assert!(
        absent.stderr.contains("no-such-changed-file-list.txt"),
        "the unreadable list must be named on stderr - otherwise a broken invocation is \
         indistinguishable from a clean PR: {:?}",
        absent.stderr
    );

    // Control, same arm and same invocation with one thing changed: a readable
    // file DOES annotate. Without it the assertions above would also hold for a
    // binary that ignored its FILE argument entirely.
    let present = dir.join("changed-file-list.txt");
    std::fs::write(&present, "crates/wcore-agent/src/engine.rs\n").expect("write the list");
    let readable = run_bytes(
        &["preflight", present.to_str().expect("utf-8 temp path")],
        b"",
    );
    assert!(readable.status.success(), "{:?}", readable.status);
    assert!(
        readable.stdout.contains("::notice"),
        "the FILE argument is genuinely read when it exists: {:?}",
        readable.stdout
    );
    std::fs::remove_dir_all(&dir).ok();
}
