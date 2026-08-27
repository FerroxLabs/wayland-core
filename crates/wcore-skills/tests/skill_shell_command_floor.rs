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
//! So the floor's own rule 2 (the agent may not AUTHOR its own authority
//! state, and may not read the credential stores inside it) is enforced on one
//! shell path and silently revoked on the other. These tests grade the second
//! one.

use wcore_skills::shell::{ShellExecutionError, execute_shell_commands};
use wcore_skills::types::LoadedFrom;

/// A command naming the agent's own credential store, in the READ direction.
///
/// `~` is expanded by the floor exactly as a shell would expand it, so this is
/// the same spelling a model actually emits — and it names the operator's REAL
/// profile. A read, deliberately: these directives EXECUTE if the floor lets
/// them through, and a test may not be able to author a live profile the moment
/// the floor regresses. The credential leaves are the one part of the profile
/// the floor read-denies, mirroring `workspace_policy`'s `fs_read_deny`.
const AUTHORITY_READ_CMD: &str = "cat ~/.wayland/credentials.toml";

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
async fn a_skill_shell_directive_may_not_read_the_agents_own_credential_store() {
    let td = tempfile::tempdir().unwrap();
    let cwd = td.path().to_str().unwrap();

    let content = format!("before !`{AUTHORITY_READ_CMD}` after");

    let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
    assert_refused(&result);
}

/// The write direction of rule 2 on this path, asked against a RELOCATED
/// profile so the command that would run if the floor failed lands in a
/// tempdir rather than in the operator's live store.
#[tokio::test]
#[serial_test::serial]
async fn a_skill_shell_directive_may_not_write_the_agents_own_authority_state() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("WAYLAND_HOME", home.path());
    }
    let td = tempfile::tempdir().unwrap();
    let cwd = td.path().to_str().unwrap();

    let store = home.path().join("permissions.toml");
    let content = format!("!`echo '[[rules]]' >> {}`", store.display());
    let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
    assert_refused(&result);
    assert!(
        !store.exists(),
        "the refusal must mean NOTHING RAN, not merely that the result said so"
    );

    unsafe {
        std::env::remove_var("WAYLAND_HOME");
    }
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

    // READ — the credential leaves only. `permissions.toml`, `config.toml` and
    // `workspace-trust.json` are NOT here, deliberately: the floor is a write
    // deny, and its one read carry-over is `fs_read_deny`, which names exactly
    // these. They are asserted to PASS in
    // `ordinary_skill_directives_with_real_paths_are_untouched`.
    for token in [
        "~/.wayland/credentials.toml",
        "~/.wayland/credentials.enc",
        "~/.wayland/oauth/anthropic.json",
    ] {
        let content = format!("!`cat {token}`");
        let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
        assert_refused(&result);
    }

    // WRITE — the rest of the floor.
    for cmd in [
        "echo x > .git/hooks/pre-commit",
        "echo x >> .git/config",
        "echo x > .wayland-core/skills/evil/SKILL.md",
        "rm -rf .wayland-core/skills",
        "cp /tmp/x .git/hooks/pre-push",
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
    let content = format!("!`{AUTHORITY_READ_CMD}`");
    let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
    assert_refused(&result);

    // And the relocated root, which must also be protected. A write, because
    // it lands in a tempdir and rule 2's own direction is the write one.
    let moved = elsewhere.path().join("permissions.toml");
    let content = format!("!`echo x >> {}`", moved.display());
    let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
    assert_refused(&result);
    assert!(!moved.exists(), "nothing ran");

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
        "!`{AUTHORITY_READ_CMD}`\nand also !`touch {}`",
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

/// The other direction, with real PATH TOKENS rather than a bare `echo`.
///
/// `an_ordinary_skill_shell_directive_still_runs` proves the feature still
/// works; it does not prove the floor is narrow, because its command has no
/// path in it to match. These do. A floor that refused these would have broken
/// the skills feature rather than floored it.
///
/// The `MEASURED_COST` rows are the eight commands the first revision of the
/// floor refused, verbatim, on BOTH shell surfaces. They are graded here as
/// well as in `wcore-tools/tests/command_floor_test.rs` because the skill path
/// is where `cat .wayland-core/skills/x/SKILL.md` is not a hypothetical: a
/// skill that shells out to read its own sibling data hits it every load.
#[tokio::test]
#[serial_test::serial]
async fn ordinary_skill_directives_with_real_paths_are_untouched() {
    open_every_hatch();
    // A relocated profile, so the non-credential authority rows below name a
    // tempdir rather than the operator's real store — they are asserted to RUN.
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("WAYLAND_HOME", home.path());
    }
    std::fs::write(home.path().join("permissions.toml"), "p\n").unwrap();
    std::fs::write(home.path().join("config.toml"), "c\n").unwrap();

    let td = tempfile::tempdir().unwrap();
    std::fs::write(td.path().join("README.md"), "hello\n").unwrap();
    std::fs::create_dir_all(td.path().join("src")).unwrap();
    std::fs::create_dir_all(td.path().join(".git/hooks")).unwrap();
    std::fs::write(
        td.path().join(".git/config"),
        "[core]\n\trepositoryformatversion = 0\n",
    )
    .unwrap();
    std::fs::write(td.path().join(".git/hooks/pre-commit"), "#!/bin/sh\n").unwrap();
    std::fs::create_dir_all(td.path().join(".wayland-core/skills/x")).unwrap();
    // Contains the word `wayland` on purpose: `grep -rn wayland .wayland-core`
    // below exits 1 with EMPTY output when it matches nothing, and this path
    // reports that as a command failure — which would make the row red for a
    // reason that has nothing to do with the floor.
    std::fs::write(
        td.path().join(".wayland-core/skills/x/SKILL.md"),
        "wayland demo\n",
    )
    .unwrap();
    std::fs::write(td.path().join(".wayland-core.toml"), "t\n").unwrap();
    let cwd = td.path().to_str().unwrap();

    let profile_reads = [
        format!("cat {}", home.path().join("permissions.toml").display()),
        format!("cat {}", home.path().join("config.toml").display()),
    ];

    let measured_cost = [
        r#"git commit -m "fix .git/config parsing""#.to_string(),
        "grep -rn wayland .wayland-core".to_string(),
        "ls .wayland-core/skills".to_string(),
        "cat .wayland-core/skills/x/SKILL.md".to_string(),
        "git config --file .git/config --list".to_string(),
        "cat .git/hooks/pre-commit".to_string(),
        "ls -la .git/hooks".to_string(),
        "echo see .wayland-core.toml for config".to_string(),
    ];

    let ordinary = [
        "cat README.md".to_string(),
        "ls .".to_string(),
        "ls src".to_string(),
        "git status --porcelain".to_string(),
        "cat ./README.md".to_string(),
        // Not `.wayland-core`: a component-wise match must not fire on a name
        // that merely CONTAINS the protected one.
        "echo mywayland-core-notes".to_string(),
        // Nor on `.gitignore`, which shares a prefix with `.git` but is not
        // the `.git` DIRECTORY.
        "echo .gitignore".to_string(),
        // Ordinary writes, inside the workspace and outside every protected
        // surface — the floor must not have become a blanket write deny either.
        "echo hi > notes.txt".to_string(),
        "rm -rf src".to_string(),
    ];

    for cmd in ordinary
        .iter()
        .chain(measured_cost.iter())
        .chain(profile_reads.iter())
    {
        let content = format!("!`{cmd}`");
        let result = execute_shell_commands(&content, LoadedFrom::Skills, cwd).await;
        assert!(
            result.is_ok(),
            "the floor refused ordinary skill work `{cmd}` — that breaks the \
             feature rather than flooring it: {:?}",
            result.err()
        );
    }

    // Known-positive control in the SAME run: the floor is still installed on
    // this path, so the clean sweep above is narrowness and not absence.
    let refused = execute_shell_commands(
        "!`echo x > .wayland-core/skills/x/SKILL.md`",
        LoadedFrom::Skills,
        cwd,
    )
    .await;
    assert_refused(&refused);
    assert_eq!(
        std::fs::read_to_string(td.path().join(".wayland-core/skills/x/SKILL.md")).unwrap(),
        "wayland demo\n",
        "the control command RAN — the refusal above was some other error, so \
         the clean sweep proves nothing"
    );

    unsafe {
        std::env::remove_var("WAYLAND_HOME");
    }
}

/// The cost the floor DOES impose on this path, pinned so it is a decision
/// rather than a surprise — and its exact boundary.
///
/// A project skill may READ its own `.wayland-core` tree from the shell; that
/// is ordinary work and the row above asserts it. What it may not do is AUTHOR
/// that tree, because those bytes are obeyed by the next session rather than
/// merely read. An earlier revision of this test asserted the opposite, which
/// is the defect this file was reworked to remove: it meant a project skill
/// could not shell out to read its own sibling data at all.
///
/// Measured cost of the write half at the time of writing: ZERO shipped skills
/// use a shell directive at all (`grep -rln '!`' crates/wcore-skills/src/bundled/`
/// is empty), so nothing in the product regresses.
#[tokio::test]
async fn a_skill_may_read_its_own_control_tree_but_not_author_it() {
    open_every_hatch();
    let td = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(td.path().join(".wayland-core/skills/demo")).unwrap();
    std::fs::write(
        td.path().join(".wayland-core/skills/demo/data.txt"),
        "SENTINEL\n",
    )
    .unwrap();
    let cwd = td.path().to_str().unwrap();

    let out = execute_shell_commands(
        "!`cat .wayland-core/skills/demo/data.txt`",
        LoadedFrom::Skills,
        cwd,
    )
    .await
    .expect("a skill must be able to read its own sibling data");
    assert!(
        out.contains("SENTINEL"),
        "the directive did not run: {out:?}"
    );

    let refused = execute_shell_commands(
        "!`echo pwned > .wayland-core/skills/demo/data.txt`",
        LoadedFrom::Skills,
        cwd,
    )
    .await;
    assert_refused(&refused);
    assert_eq!(
        std::fs::read_to_string(td.path().join(".wayland-core/skills/demo/data.txt")).unwrap(),
        "SENTINEL\n",
        "nothing ran"
    );
}
