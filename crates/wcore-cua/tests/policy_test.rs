//! F.6 — `CuaPolicy` integration test surface (TDD per the plan).
//! Inline unit tests in `policy.rs` cover the per-rule branches; this
//! file pins the high-level scenarios the design contract calls out.

use wcore_cua::{CuaOp, CuaPolicy, CuaPolicyOutcome};

#[test]
fn require_approval_for_sensitive_app_triggers_suspend() {
    let mut policy = CuaPolicy::permissive();
    policy.require_approval_for_app = vec!["Keychain Access".into(), "System Settings".into()];
    let outcome = policy.check_action(
        &CuaOp::LeftClick {
            x: 100,
            y: 100,
            button: Default::default(),
            mods: Default::default(),
        },
        "Keychain Access",
    );
    assert!(matches!(outcome, CuaPolicyOutcome::Suspend { .. }));
}

#[test]
fn forbidden_key_combo_rejected_outright() {
    let mut policy = CuaPolicy::permissive();
    policy.forbidden_key_combos = vec!["cmd+q+system".into()];
    let outcome = policy.check_action(
        &CuaOp::Key {
            keys: "cmd+q+system".into(),
            mods: Default::default(),
        },
        "Finder",
    );
    assert!(matches!(outcome, CuaPolicyOutcome::Reject { .. }));
}

#[test]
fn first_time_per_app_triggers_approval_then_allows_after_mark() {
    let mut policy = CuaPolicy::permissive();
    policy.first_time_per_app_approval = true;
    let click = CuaOp::LeftClick {
        x: 0,
        y: 0,
        button: Default::default(),
        mods: Default::default(),
    };
    let first = policy.check_action(&click, "TextEdit");
    assert!(matches!(first, CuaPolicyOutcome::Suspend { .. }));
    policy.mark_app_seen("TextEdit");
    let after = policy.check_action(&click, "TextEdit");
    assert!(matches!(after, CuaPolicyOutcome::Allow));
}

#[test]
fn forbidden_apps_block_every_op() {
    let mut policy = CuaPolicy::permissive();
    policy.forbidden_apps = vec!["1Password".into()];
    let outcome = policy.check_action(
        &CuaOp::Type {
            text: "secret".into(),
        },
        "1Password",
    );
    assert!(matches!(outcome, CuaPolicyOutcome::Reject { .. }));
}
