//! A wire `set_mode` that asks for an auto-approving mode without the local
//! operator opt-in must be refused **observably** (wayland#1088).
//!
//! # Why this file exists
//!
//! `ToolApprovalManager::set_mode_from_wire` refuses `force`/`auto_edit` from
//! an un-opted-in wire peer (GHSA-8r7g — the remote-sandbox-bypass guard). The
//! refusal is correct and stays. What was wrong is that it was reported ONLY
//! as an `info` frame carrying English prose, so a host could not tell its
//! requested mode had been rejected: the session silently stayed in `default`,
//! where every category gates at once, and the host attributed the resulting
//! exec + info + restricted gate storm to the engine rather than to its own
//! un-applied mode.
//!
//! Unit tests of the gate itself cannot catch that — they grade the boolean,
//! not what reaches the host. So this test spawns the REAL binary, speaks the
//! real json-stream protocol at it, and reads real stdout, exactly as a host
//! does. It runs against the debug binary Cargo wires through `CARGO_BIN_EXE_`,
//! so it is part of every `cargo nextest run` and needs no release pre-build.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

/// Drive `--json-stream`, send `set_mode` with `mode`, then `stop`, and return
/// every frame the engine wrote.
///
/// `opt_in` sets `WAYLAND_ALLOW_WIRE_FORCE=1` — the local-operator opt-in — so
/// the same driver produces both arms.
fn frames_for_set_mode(mode: &str, opt_in: bool) -> Vec<serde_json::Value> {
    let tmp = TempDir::new().expect("create tmp workspace");

    let mut command = Command::new(env!("CARGO_BIN_EXE_wayland-core"));
    command
        .args([
            "--json-stream",
            "--provider",
            "anthropic",
            "--api-key",
            "test-key-not-used-because-we-never-send-a-message",
        ])
        .current_dir(tmp.path())
        // `HOME` alone does NOT isolate on Windows (`dirs::home_dir()` reads
        // `USERPROFILE` there), so set the crate's hermetic override too.
        .env("HOME", tmp.path())
        .env("WAYLAND_HOME", tmp.path())
        .env_remove("WAYLAND_ALLOW_WIRE_FORCE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if opt_in {
        command.env("WAYLAND_ALLOW_WIRE_FORCE", "1");
    }
    let mut child =
        OwnedTree::new(command.spawn().expect("spawn wayland-core --json-stream"));

    let mut stdin = child.stdin.take().expect("capture stdin");
    writeln!(stdin, "{{\"type\":\"set_mode\",\"mode\":\"{mode}\"}}").expect("write set_mode");
    writeln!(stdin, "{{\"type\":\"stop\"}}").expect("write stop");
    stdin.flush().expect("flush");
    drop(stdin);

    let mut stdout = child.stdout.take().expect("capture stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let lines = BufReader::new(&mut stdout)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<_>>();
        let _ = tx.send(lines);
    });
    let lines = rx
        .recv_timeout(Duration::from_secs(90))
        .expect("wayland-core did not close stdout within 90s");
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        !lines.is_empty(),
        "wayland-core --json-stream closed stdout without emitting a single frame"
    );
    lines
        .iter()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect()
}

/// The refusal must arrive as a TYPED frame a host can branch on, naming what
/// was asked for and what is actually in force.
#[test]
fn a_refused_wire_force_is_reported_as_a_typed_frame() {
    let frames = frames_for_set_mode("force", false);

    let refusal = frames
        .iter()
        .find(|frame| frame["type"] == "set_mode_refused")
        .unwrap_or_else(|| {
            panic!(
                "a host must be able to observe the refusal without parsing prose; frames: {}",
                serde_json::to_string(&frames).unwrap_or_default()
            )
        });
    assert_eq!(refusal["requested"], "force");
    assert_eq!(
        refusal["effective"], "default",
        "the refusal leaves the session on the mode it already had"
    );
    assert_eq!(
        refusal["reason"], "local_opt_in_required",
        "the reason must be a typed vocabulary term, not a sentence"
    );

    // The security posture is unchanged: no policy frame may say approvals
    // moved off `prompt`, and the refusal must not consume a revision.
    for frame in frames.iter().filter(|f| f["type"] == "execution_policy") {
        assert_eq!(
            frame["policy"]["approvals"], "prompt",
            "a refused escalation must not advance the execution policy: {frame}"
        );
        assert_eq!(
            frame["revision"], 0,
            "a refusal consumes no revision: {frame}"
        );
    }
}

/// `auto_edit` is the other escalating mode and is refused on the same gate.
#[test]
fn a_refused_wire_auto_edit_is_reported_as_a_typed_frame() {
    let frames = frames_for_set_mode("auto_edit", false);
    let refusal = frames
        .iter()
        .find(|frame| frame["type"] == "set_mode_refused")
        .expect("auto_edit is escalating too, so its refusal is equally observable");
    assert_eq!(refusal["requested"], "auto_edit");
    assert_eq!(refusal["effective"], "default");
}

/// The control: with the local opt-in present the SAME request is APPLIED, and
/// no refusal frame appears. Without this arm the assertions above would pass
/// against a build that refuses unconditionally.
///
/// # Why "an execution_policy frame exists" was not the control
///
/// Every session emits an `execution_policy` frame at launch — `revision: 0`,
/// `reason: "launch"`, `approvals: "prompt"` — so "some policy frame arrived"
/// is true before the `set_mode` is even read. **Measured:** with
/// `apply_wire_mode_change` mutated to consume an accepted change, leave the
/// manager on `default`, and return `Unchanged` — the exact silent no-op
/// wayland#1088 is about, only now on the ACCEPTING side — all three tests in
/// this file passed.
///
/// So the control names the applied state: the frame must be the MODE CHANGE
/// (not the launch frame), it must advance the revision, and its policy must
/// actually say approvals moved to `bypass`. The launch frame is asserted
/// alongside it as the known-positive, so "no policy frames at all" cannot
/// satisfy this either.
#[test]
fn the_opted_in_force_is_applied_and_emits_no_refusal() {
    let frames = frames_for_set_mode("force", true);
    let rendered = serde_json::to_string(&frames).unwrap_or_default();

    assert!(
        !frames
            .iter()
            .any(|frame| frame["type"] == "set_mode_refused"),
        "with WAYLAND_ALLOW_WIRE_FORCE=1 the request is honoured; frames: {rendered}"
    );

    let policies: Vec<&serde_json::Value> = frames
        .iter()
        .filter(|frame| frame["type"] == "execution_policy")
        .collect();

    // Known-positive: the launch frame is present, and it is NOT the applied
    // change. Without this the assertion below could be satisfied by a harness
    // that stopped capturing policy frames entirely.
    assert!(
        policies.iter().any(|frame| {
            frame["reason"] == "launch"
                && frame["revision"] == 0
                && frame["policy"]["approvals"] == "prompt"
        }),
        "the launch policy frame (revision 0, approvals=prompt) is missing, so this test is \
         not reading the stream it thinks it is; frames: {rendered}"
    );

    let applied = policies
        .iter()
        .find(|frame| frame["reason"] == "mode_change")
        .unwrap_or_else(|| {
            panic!(
                "the opted-in set_mode published no mode_change policy frame: the command was \
                 accepted, refused nothing, and changed nothing - a silent no-op is exactly \
                 what wayland#1088 is about, on the accepting side; frames: {rendered}"
            )
        });
    assert_eq!(
        applied["policy"]["approvals"], "bypass",
        "`force` is the auto-approving mode: the published policy must SAY approvals moved to \
         bypass, or the host is told a mode was applied that was not; frame: {applied}"
    );
    assert!(
        applied["revision"].as_u64().is_some_and(|r| r >= 1),
        "an applied change must consume a revision - a refusal deliberately does not, so a \
         revision that never advances means nothing was applied; frame: {applied}"
    );
}
