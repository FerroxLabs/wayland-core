//! #693 — the command floor, asked of the SECOND shell path.
//!
//! `wcore_tools::bash::command_floor` put a floor under `BashTool`. It did not
//! put one under `wcore_skills::shell::execute_shell_commands`, which is a
//! second, independent `sh -c` surface reached from skill content.
//!
//! That gap is not theoretical, and it is not merely "one more path to
//! harden" — it is a complete, two-step bypass of the floor that was just
//! built, and both steps are ordinary agent actions:
//!
//!   1. `BashTool` writes `<WAYLAND_HOME>/skills/x/SKILL.md`. The floor does
//!      NOT refuse this: `skills` is not one of `AUTHORITY_LEAVES`, and the
//!      floor matches a protected root only by EXACT equality, never as a
//!      prefix. Verified by `bash_may_still_author_a_user_skill` in
//!      `wcore-tools/tests/command_floor_test.rs`.
//!   2. The next session loads that skill and `execute_shell_commands` runs
//!      its `` !`…` `` directive under `sh -c` with no floor at all — reaching
//!      exactly the authority state step 1 was not allowed to touch directly.
//!
//! So the floor's own rule 2 (the agent may not read or write its own
//! authority state) is enforced on one shell path and silently revoked on the
//! other. These tests grade the second one.

use wcore_skills::shell::{ShellExecutionError, execute_shell_commands};
use wcore_skills::types::LoadedFrom;

/// A path token naming the agent's own learned-grant store. `~` is expanded by
/// the floor exactly as a shell would expand it, so this is the same spelling
/// a model actually emits.
const AUTHORITY_TOKEN: &str = "~/.wayland/permissions.toml";

/// Assert the directive did NOT run.
///
/// Refusal is an `Err`: `execute_shell_commands` is fail-fast, so a refused
/// directive fails the whole skill body rather than silently returning content
/// with the directive stripped. That is the fail-CLOSED shape — a skill whose
/// shell-out was refused must not load looking as though it succeeded.
fn assert_refused(result: &Result<String, ShellExecutionError>) {
    match result {
        Err(_) => {}
        Ok(out) => panic!(
            "the skill `!shell:` directive was EXECUTED — the floor does not \
             cover this path. Output: {out:?}"
        ),
    }
}

#[tokio::test]
async fn a_skill_shell_directive_may_not_name_the_agents_own_authority_state() {
    let td = tempfile::tempdir().unwrap();
    let cwd = td.path().to_str().unwrap();

    // `echo` rather than `cat`: side-effect free, and it proves EXECUTION
    // (the shell expands `~` and prints the path) without reading the real
    // store. The floor does not inspect the verb, only the path tokens, so
    // this is the same question `cat`/`>>` would ask.
    let content = format!("before !`echo {AUTHORITY_TOKEN}` after");

    let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
    assert_refused(&result);
}

#[tokio::test]
async fn a_skill_shell_block_may_not_name_a_repository_control_surface() {
    let td = tempfile::tempdir().unwrap();
    let cwd = td.path().to_str().unwrap();

    // The block form (```!\n…\n```), which is the other pattern
    // `extract_shell_matches` accepts — a floor on only the inline form would
    // be no floor.
    let content = "```!\necho hi > .git/hooks/pre-commit\n```";

    let result = execute_shell_commands(content, LoadedFrom::Skills, cwd).await;
    assert_refused(&result);
}

/// The other direction: the floor must not cost the ordinary use of the
/// feature. A skill that shells out for something innocuous still works.
#[tokio::test]
async fn an_ordinary_skill_shell_directive_still_runs() {
    let td = tempfile::tempdir().unwrap();
    let cwd = td.path().to_str().unwrap();

    let content = "version: !`echo 1.2.3`";
    let out = execute_shell_commands(content, LoadedFrom::Skills, cwd)
        .await
        .expect("an innocuous directive must still run");
    assert!(
        out.contains("1.2.3"),
        "the floor refused ordinary work: {out:?}"
    );
    assert!(
        !out.contains("!`"),
        "the directive was not substituted: {out:?}"
    );
}

// ---------------------------------------------------------------------------
// Non-bypassable: every hatch open
// ---------------------------------------------------------------------------

/// Every escape hatch this product has, ON.
///
/// The skills shell path does not read any of these — that is the claim. A
/// test that asserts the refusal while they are set is what makes the claim
/// falsifiable rather than merely stated.
///
/// | hatch | value | what it would otherwise buy |
/// |-------|-------|------------------------------|
/// | `WAYLAND_SANDBOX` | `none` | no OS backend at all — the shape `--dangerously-skip-permissions-and-sandbox` produces |
/// | `WAYLAND_ALLOW_NO_SANDBOX` | `1` | the interlock that makes the line above take effect |
/// | `TIRITH_ENABLED` | `0` | the external pre-exec scanner off |
/// | `TIRITH_FAIL_OPEN` | `true` | and failing open if it were on |
fn open_every_hatch() {
    unsafe {
        std::env::set_var("WAYLAND_SANDBOX", "none");
        std::env::set_var("WAYLAND_ALLOW_NO_SANDBOX", "1");
        std::env::set_var("TIRITH_ENABLED", "0");
        std::env::set_var("TIRITH_FAIL_OPEN", "true");
    }
}

#[tokio::test]
#[serial_test::serial]
async fn the_floor_holds_on_the_skill_path_with_every_hatch_open() {
    open_every_hatch();
    let td = tempfile::tempdir().unwrap();
    let cwd = td.path().to_str().unwrap();

    for token in [
        "~/.wayland/permissions.toml",
        "~/.wayland/config.toml",
        "~/.wayland/credentials.toml",
        "~/.wayland/workspace-trust.json",
    ] {
        let content = format!("!`cat {token}`");
        let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
        assert_refused(&result);
    }

    for cmd in [
        "echo x > .git/hooks/pre-commit",
        "cat .git/config",
        "echo x > .wayland-core/skills/evil/SKILL.md",
    ] {
        let content = format!("!`{cmd}`");
        let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
        assert_refused(&result);
    }
}

/// Moving `WAYLAND_HOME` must not move the floor off the operator's real
/// store. An environment variable that relocates a floor is an environment
/// variable that disables it.
#[tokio::test]
#[serial_test::serial]
async fn moving_wayland_home_does_not_move_the_floor_on_the_skill_path() {
    open_every_hatch();
    let elsewhere = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("WAYLAND_HOME", elsewhere.path());
    }

    let td = tempfile::tempdir().unwrap();
    let cwd = td.path().to_str().unwrap();

    // The DEFAULT root, while WAYLAND_HOME points somewhere else entirely.
    let content = format!("!`echo {AUTHORITY_TOKEN}`");
    let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
    assert_refused(&result);

    // And the relocated root, which must also be protected.
    let moved = elsewhere.path().join("permissions.toml");
    let content = format!("!`echo {}`", moved.display());
    let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
    assert_refused(&result);

    unsafe {
        std::env::remove_var("WAYLAND_HOME");
    }
}

/// A refused directive must stop the WHOLE body, not merely itself.
///
/// The directives execute in parallel, so a floor asked only at each spawn
/// point would let a skill pair a refused directive with a side-effecting one
/// and still get the side effect. This asserts the side effect does not happen.
#[tokio::test]
async fn one_refused_directive_stops_the_whole_skill_body() {
    let td = tempfile::tempdir().unwrap();
    let cwd = td.path().to_str().unwrap();
    let canary = td.path().join("canary");

    let content = format!(
        "!`echo {AUTHORITY_TOKEN}`\nand also !`touch {}`",
        canary.display()
    );
    let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
    assert_refused(&result);

    assert!(
        !canary.exists(),
        "a refused body still ran its OTHER directive — the floor is asked \
         per-command but the body is admitted as a set, and this is the \
         difference"
    );
}

/// Structural: the skill shell path calls the floor, and calls it before it
/// can spawn anything.
///
/// Behaviour above proves the hatches we thought of do not open it. This
/// proves the call is actually there — a behavioural test cannot distinguish
/// "refused by the floor" from "failed for an unrelated reason", and a future
/// edit that deleted the call would be caught here even if some other error
/// happened to keep the tests red-looking-green.
///
/// Graded in both directions: the positive (both call sites are present) and
/// a known-positive that the scan read real code at all.
#[test]
fn the_skill_shell_path_calls_the_floor() {
    let raw = include_str!("../src/shell.rs");
    let code: String = raw
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.len() > raw.len() / 4,
        "comment strip removed almost everything ({} of {} bytes left); the \
         assertions below would then prove nothing",
        code.len(),
        raw.len()
    );
    // Known-positive control: the scan reached the code that spawns the shell.
    assert!(
        code.contains("shell_command_builder"),
        "positive control: no code was scanned"
    );
    assert_eq!(
        code.matches("check_command_floor").count(),
        2,
        "the skill shell path must ask the floor TWICE — once over the whole \
         set before anything runs, once at the spawn point so a future caller \
         that skips the first cannot skip both"
    );
}
