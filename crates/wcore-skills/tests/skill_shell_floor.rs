//! #693 — the command floor on the SECOND shell surface.
//!
//! `wcore_skills::shell::execute_shell_commands` spawns a shell through
//! `wcore_config::shell::shell_command_builder` with no denylist, no OS
//! sandbox and no approval manager: it is not the `BashTool`, so none of those
//! guards are on this path at all. The layers that DO gate it — skill
//! permissions and workspace trust — are both waivable, which is exactly the
//! layer a floor sits under.
//!
//! `wcore-skills` does not depend on `wcore-tools`, which is why the floor
//! lives in `wcore-config`: the lowest crate BOTH shell surfaces already
//! depend on.

use wcore_skills::shell::{ShellExecutionError, execute_shell_commands};
use wcore_skills::types::LoadedFrom;

/// A skill body may not rewrite the learned-grant store.
#[tokio::test]
#[serial_test::serial]
async fn a_skill_body_cannot_reach_the_grant_store() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();

    // Point HOME at a throwaway directory so that if the floor ever regresses
    // this test damages a temp dir and not the machine's real grant store.
    // SAFETY: test-only env mutation; `#[serial]` prevents env races.
    let prior_home = std::env::var_os("HOME");
    unsafe { std::env::set_var("HOME", dir.path()) };

    // Known-positive control FIRST: an ordinary substitution on this surface,
    // in this directory, really does run. Without it, a refusal below could be
    // any unrelated failure of the shell path.
    let ok = execute_shell_commands(
        "before !`echo floor_control_ok` after",
        LoadedFrom::Skills,
        &cwd,
    )
    .await
    .expect("an ordinary skill substitution must still run");
    assert!(
        ok.contains("floor_control_ok"),
        "control substitution produced no output: {ok:?}"
    );

    for body in [
        "grants: !`echo x >> ~/.wayland/permissions.toml`",
        "grants: !`echo x >> $HOME/.wayland/permissions.toml`",
        "hook: !`printf '#!/bin/sh\\nid\\n' > .git/hooks/pre-commit`",
        "cfg: !`echo x >> .git/config`",
    ] {
        let err = execute_shell_commands(body, LoadedFrom::Skills, &cwd)
            .await
            .expect_err("the floor must refuse this skill body");
        assert!(
            matches!(err, ShellExecutionError::FloorRefused { .. }),
            "must be the floor refusal, not an incidental failure: {err:?} for {body:?}"
        );
    }

    unsafe {
        match prior_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// The wrong-refusal arm on this surface: ordinary skill shell expansion is
/// untouched.
#[tokio::test]
#[serial_test::serial]
async fn ordinary_skill_shell_expansion_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();

    let out = execute_shell_commands(
        "branch: !`echo main`\nfiles: !`echo Cargo.toml`",
        LoadedFrom::Skills,
        &cwd,
    )
    .await
    .expect("ordinary substitutions must still run");
    assert!(out.contains("main"), "got {out:?}");
    assert!(out.contains("Cargo.toml"), "got {out:?}");
}
