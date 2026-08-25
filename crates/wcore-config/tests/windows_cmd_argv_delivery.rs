//! What `shell_command_argv` actually delivers to a real `cmd.exe`.
//!
//! The unit tests beside `push_argv` grade a MODEL of cmd's documented quote
//! handling. A model is only as good as the reading behind it, and the reading
//! behind #943 was wrong once already. This asks cmd itself, so the rule and
//! the program it describes cannot drift apart.
//!
//! Windows-only by necessity: `cmd.exe` is the subject. It therefore never runs
//! on the Linux or macOS legs, which is exactly why the model tests exist too —
//! neither file is sufficient alone.
//!
//! MEASURED on Windows 11 build 26200.9168 by driving `cmd.exe` with a
//! byte-exact command line, before any fix:
//!
//! ```text
//! cmd    /C "cmd /c echo NESTED"   ->  stdout `NESTED"`   <- stray 0x22
//! cmd /S /C "cmd /c echo NESTED"   ->  stdout `NESTED`
//! ```
//!
//! `shell_command_argv("cmd", ["/c", "cmd /c echo NESTED"])` produced the first
//! line, because it added the outer quote pair and no `/S` to strip it. That is
//! #943 reproduced through the argv helper, at the call sites `goal_cmd` and
//! `gateway` use.

#![cfg(windows)]

use wcore_config::shell::shell_command_argv;

/// Run an argv through the helper under test and return its stdout verbatim.
///
/// Spawned through the builder's inner `std::process::Command` rather than a
/// tokio runtime. The command LINE is the subject here, and both spawn paths
/// build it from the same argv the helper assembled — so this asks the question
/// without making the test depend on a runtime flavour.
fn stdout_of(program: &str, args: &[&str]) -> String {
    let mut cmd = shell_command_argv(program, args);
    let out = cmd
        .as_std_mut()
        .output()
        .expect("cmd.exe must spawn on Windows");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// THE DEFECT. A nested `cmd /c` payload is the shape whose first token names
/// a real program on every Windows host, which is the condition that sends cmd
/// down its quote-PRESERVING branch when `/S` is absent. The helper adds the
/// pair, so the helper must also carry the switch that removes it.
///
/// Asserted on the BYTES, not on a trimmed string: the whole defect is one
/// stray `0x22`, and `trim()` would delete the evidence.
#[test]
fn a_nested_cmd_payload_arrives_without_the_wrappers_quote() {
    let stdout = stdout_of("cmd", &["/c", "cmd /c echo NESTED"]);
    assert_eq!(
        stdout.as_bytes(),
        b"NESTED\r\n",
        "delivered stdout was {:?} ({:02X?}). A trailing 0x22 here is the \
         wrapper's own quote surviving into the child's argument (#943) — the \
         argv reached `wrap_cmd_payload` without a `/S` to strip the pair.",
        stdout,
        stdout.as_bytes()
    );
}

/// CONTROL 1 — a payload cmd already handled correctly before the fix, because
/// `&` disqualifies the preserving branch. It must still be correct, and its
/// correctness must not depend on the change: if this moves, the fix is doing
/// something broader than stripping one pair.
#[test]
fn a_payload_holding_a_metacharacter_is_unchanged() {
    let stdout = stdout_of("cmd", &["/c", "echo A & echo B"]);
    assert_eq!(stdout.as_bytes(), b"A \r\nB\r\n", "got {stdout:?}");
}

/// CONTROL 2 — an argv that is NOT a cmd payload invocation must not be
/// touched at all. `cmd_payload_index` returns `None`, so no pair is added and
/// no switch is inserted; a `/S` appearing here would prove the planner fired
/// on an argv it has no business rewriting.
#[test]
fn a_non_cmd_program_is_left_as_ordinary_argv() {
    let stdout = stdout_of("where.exe", &["cmd"]);
    assert!(
        stdout.to_ascii_lowercase().contains("cmd.exe"),
        "where.exe should have resolved cmd; got {stdout:?}"
    );
    assert!(
        !stdout.contains("/S"),
        "no switch may be introduced into a non-cmd argv; got {stdout:?}"
    );
}

/// A caller that already supplies `/S` must be byte-identical to one that does
/// not — that equality is the whole point of filling the switch in, and it is
/// what lets `BashTool` (which always sends `/S`) and an operator argv (which
/// never does) be reasoned about as one path.
#[test]
fn supplying_the_switch_explicitly_changes_nothing() {
    let implicit = stdout_of("cmd", &["/c", "cmd /c echo NESTED"]);
    let explicit = stdout_of("cmd", &["/S", "/c", "cmd /c echo NESTED"]);
    assert_eq!(
        implicit.as_bytes(),
        explicit.as_bytes(),
        "implicit {implicit:?} vs explicit {explicit:?}"
    );
}
