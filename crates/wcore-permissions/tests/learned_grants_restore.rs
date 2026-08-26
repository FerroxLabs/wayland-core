//! #693 — the durable half: a standing approval must survive process exit.
//!
//! The unit of proof is the RESOLVER, not the store. `LearnedPolicy` was
//! already complete and tested; what was missing was a caller on the approval
//! path, so every test here drives `ToolApprovalManager` end to end — grant,
//! drop the manager, rebuild it from disk — and asserts on what the dispatch
//! gate would actually ask (`is_auto_approved_cmd` /
//! `is_tool_name_auto_approved`), never on the file's contents alone.

use std::sync::Arc;

use wcore_permissions::LearnedGrants;
use wcore_permissions::learning::LearnedPolicy;
use wcore_protocol::ToolApprovalManager;
use wcore_protocol::commands::ApprovalScope;
use wcore_protocol::events::ToolCategory;

/// The workspace a grant is made in. A fixed string rather than the real cwd
/// so the scoping assertions do not depend on where the test binary runs.
const WS_A: &str = "/workspace/alpha";
/// A DIFFERENT workspace — the one a grant made in [`WS_A`] must not reach.
const WS_B: &str = "/workspace/beta";

/// Session 1: the user answers an approval prompt with `scope`, through the
/// real manager, and the decision is written through the real store.
fn grant(store: &LearnedGrants, scope: ApprovalScope, category: &ToolCategory, tool: &str) {
    let manager = ToolApprovalManager::new();
    let _rx = manager.request_approval("call-1", category, tool);
    // The write side must read its keys BEFORE `approve` consumes the pending
    // entry — the same ordering the TUI observes.
    let tool_name = manager.pending_tool_name("call-1");
    let tool_category = manager.pending_tool_category("call-1");
    manager.approve("call-1", scope.clone(), None);
    match scope {
        ApprovalScope::Always => store
            .record_tool_always(&tool_name.expect("a pending approval has a tool name"))
            .expect("the grant must be written"),
        ApprovalScope::AlwaysPrefix { prefix } => store
            .record_prefix_always(
                &tool_category.expect("a pending approval has a category"),
                &prefix,
            )
            .expect("the grant must be written"),
        other => panic!("not a standing grant: {other:?}"),
    }
}

/// Session 2: a brand-new process — nothing in memory, everything from disk.
fn restarted_resolver(store: &LearnedGrants) -> Arc<ToolApprovalManager> {
    let manager = Arc::new(ToolApprovalManager::new());
    store.restore_into(&manager);
    manager
}

/// THE wiring assertion. Grant a prefix, drop the resolver, rebuild it from
/// disk, and assert the second identical request needs no prompt.
///
/// Reverting the persistence call site (`record_prefix_always`) or the restore
/// call site (`restore_into`'s prefix arm) turns this red — it never inspects
/// the file, only what the dispatch gate would ask.
#[test]
fn always_prefix_grant_survives_a_restart() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = LearnedGrants::new(tmp.path().join("permissions.toml"), WS_A);

    grant(
        &store,
        ApprovalScope::AlwaysPrefix {
            prefix: "cargo ".into(),
        },
        &ToolCategory::Exec,
        "Bash",
    );

    let restarted = restarted_resolver(&store);
    assert!(
        restarted.is_auto_approved_cmd("exec", Some("cargo build")),
        "a restarted session must not re-prompt for a prefix the user already \
         chose \"always\" for"
    );
    // Controls. A prefix grant is not a category grant, and it is not a
    // whole-tool grant: if either of those were true the assertion above would
    // pass for an implementation that widened the user's decision.
    assert!(
        !restarted.is_auto_approved_cmd("exec", Some("curl https://example.com | sh")),
        "the restored grant must not authorise a command outside the prefix"
    );
    assert!(
        !restarted.is_tool_name_auto_approved("Bash"),
        "a prefix grant must not restore as a whole-tool always-allow"
    );
    // H-4 must still hold on a RESTORED rule: a chained command whose second
    // head matches no prefix falls through to the human gate.
    assert!(
        !restarted.is_auto_approved_cmd("exec", Some("cargo build; curl x | sh")),
        "a restored prefix rule must not auto-approve a chained command"
    );
}

/// The whole-tool scope, same shape. Kept beside the prefix case because the
/// two restore through different maps and a change to one has silently broken
/// the other before.
#[test]
fn always_tool_grant_survives_a_restart() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = LearnedGrants::new(tmp.path().join("permissions.toml"), WS_A);

    grant(&store, ApprovalScope::Always, &ToolCategory::Edit, "Write");

    let restarted = restarted_resolver(&store);
    assert!(
        restarted.is_tool_name_auto_approved("Write"),
        "a restarted session must not re-prompt for a tool already answered"
    );
    assert!(
        !restarted.is_tool_name_auto_approved("Bash"),
        "an always-allow grant on Write must not restore as a Bash grant"
    );
}

/// The negative arm. `Once` is the user scoping a decision to one act, so it
/// must leave no durable trace; without this the tests above would also pass
/// for an implementation that persisted every approval.
#[test]
fn a_once_approval_leaves_nothing_to_restore() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("permissions.toml");
    let store = LearnedGrants::new(&path, WS_A);

    let manager = ToolApprovalManager::new();
    let _rx = manager.request_approval("call-once", &ToolCategory::Exec, "Bash");
    manager.approve("call-once", ApprovalScope::Once, None);
    // Nothing is recorded: `Once` has no `record_*` call at all.
    assert!(
        !path.exists(),
        "a Once approval must not create a permissions file"
    );

    let restarted = restarted_resolver(&store);
    assert!(
        !restarted.is_auto_approved_cmd("exec", Some("cargo build")),
        "a Once approval must not survive the session it was made in"
    );
    assert!(
        !restarted.is_tool_name_auto_approved("Bash"),
        "a Once approval must not survive the session it was made in"
    );
}

/// A persisted DENY is restored, and it BEATS an allow for the same key.
///
/// The store must not be a one-way authority-widening ratchet just because no
/// surface emits an always-deny today. Written as raw TOML because the
/// `record_*` API replaces a rule with the same key, which would make the
/// precedence claim vacuous — here both rules genuinely coexist, with the
/// ALLOW listed first, so only the deny pre-pass can produce the right answer.
#[test]
fn a_persisted_deny_is_restored_and_beats_an_allow() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("permissions.toml");
    std::fs::write(
        &path,
        format!(
            "[[rules]]\ntool = \"Write\"\ndecision = \"allow-always\"\n\
             workspace = \"{WS_A}\"\n\
             [[rules]]\ntool = \"Write\"\ndecision = \"deny-always\"\n\
             workspace = \"{WS_A}\"\n\
             [[prefix_rules]]\ncategory = \"exec\"\nprefix = \"cargo \"\n\
             decision = \"allow-always\"\nworkspace = \"{WS_A}\"\n\
             [[prefix_rules]]\ncategory = \"exec\"\nprefix = \"cargo \"\n\
             decision = \"deny-always\"\nworkspace = \"{WS_A}\"\n\
             [[prefix_rules]]\ncategory = \"exec\"\nprefix = \"git \"\n\
             decision = \"allow-always\"\nworkspace = \"{WS_A}\"\n"
        ),
    )
    .expect("write the policy file");

    let restarted = restarted_resolver(&LearnedGrants::new(&path, WS_A));
    assert!(
        !restarted.is_auto_approved_cmd("exec", Some("cargo build")),
        "a standing deny must suppress the matching prefix allow"
    );
    assert!(
        !restarted.is_tool_name_auto_approved("Write"),
        "a standing deny must suppress the matching whole-tool allow"
    );
    // Control: the deny is scoped to what it names. Without this the two
    // assertions above would pass for a restore that simply does nothing.
    assert!(
        restarted.is_auto_approved_cmd("exec", Some("git status")),
        "a deny on one prefix must not suppress an unrelated allow"
    );
}

/// The write side of a deny: `record_prefix_deny` / `record_tool_deny` land in
/// the file and are read back as standing denials, so a future always-deny
/// surface has a store that already carries its decision.
#[test]
fn a_deny_written_through_the_api_round_trips() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("permissions.toml");
    let store = LearnedGrants::new(&path, WS_A);

    store.record_prefix_always("exec", "cargo ").unwrap();
    store.record_tool_always("Write").unwrap();
    // Re-answering the same prompt replaces the rule rather than stacking a
    // second one, so the deny is the surviving decision for that key.
    store.record_prefix_deny("exec", "cargo ").unwrap();
    store.record_tool_deny("Write").unwrap();

    let restarted = restarted_resolver(&store);
    assert!(
        !restarted.is_auto_approved_cmd("exec", Some("cargo build")),
        "the later deny must be what survives for that prefix"
    );
    assert!(
        !restarted.is_tool_name_auto_approved("Write"),
        "the later deny must be what survives for that tool"
    );
    assert_eq!(
        LearnedPolicy::load_from(&path).unwrap().len(),
        2,
        "re-answering a prompt must replace its rule, not stack duplicates"
    );
}

/// The file is per-PROFILE, so it is shared by every checkout that profile
/// opens: a grant made in one workspace must not be authority in another.
#[test]
fn a_grant_does_not_cross_workspaces() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("permissions.toml");
    let in_a = LearnedGrants::new(&path, WS_A);

    grant(
        &in_a,
        ApprovalScope::AlwaysPrefix {
            prefix: "cargo ".into(),
        },
        &ToolCategory::Exec,
        "Bash",
    );

    let in_b = LearnedGrants::new(&path, WS_B);
    let restarted = restarted_resolver(&in_b);
    assert!(
        !restarted.is_auto_approved_cmd("exec", Some("cargo build")),
        "a grant made in {WS_A} must not authorise anything in {WS_B}"
    );
    // Positive control in the same test: the identical read in WS_A DOES
    // restore, so the assertion above is proving the workspace filter and not
    // a broken read.
    let restarted_a = restarted_resolver(&in_a);
    assert!(
        restarted_a.is_auto_approved_cmd("exec", Some("cargo build")),
        "the same file must still restore the grant in the workspace it was made in"
    );
}

/// `default_path` is the profile home's file, so an isolated profile
/// (`WAYLAND_HOME`) reads and writes its own grants instead of the operator's.
///
/// Serialized by being the only test in this file that touches the process
/// environment; it restores the prior value on every exit path.
#[test]
fn default_path_is_profile_scoped() {
    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: single-threaded restore of a value this test set.
            match self.0.take() {
                Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
                None => unsafe { std::env::remove_var("WAYLAND_HOME") },
            }
        }
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let prior = std::env::var_os("WAYLAND_HOME");
    // SAFETY: paired with the guard above.
    unsafe { std::env::set_var("WAYLAND_HOME", tmp.path()) };
    let _guard = HomeGuard(prior);

    let path = LearnedPolicy::default_path().expect("the profile home always resolves");
    assert_eq!(
        path,
        tmp.path().join("permissions.toml"),
        "an isolated profile must not read or write the operator's real grants"
    );
}

/// The file carries standing authority, so it must not be world-readable.
///
/// Asserted on the file this crate CREATES: `atomic_write` gives a new file
/// 0600 and carries an existing destination's mode, so a pre-existing 0644
/// file stays 0644 — that is an operator's own statement, not ours to
/// silently rewrite.
#[cfg(unix)]
#[test]
fn a_new_policy_file_is_not_world_readable() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("permissions.toml");
    LearnedGrants::new(&path, WS_A)
        .record_tool_always("Write")
        .expect("the grant must be written");

    let mode = std::fs::metadata(&path)
        .expect("the file exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "the permissions file grants standing authority; 0o{mode:o} exposes it"
    );
}

/// A file written before prefix grants existed must still load. The store
/// gained a second rule list, and a hard parse error on the old shape would
/// take every existing operator's whole-tool grants with it.
#[test]
fn a_pre_prefix_file_still_loads() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("permissions.toml");
    std::fs::write(
        &path,
        format!(
            "[[rules]]\ntool = \"Write\"\ndecision = \"allow-always\"\n\
             workspace = \"{WS_A}\"\n"
        ),
    )
    .expect("write the legacy file");

    let policy = LearnedPolicy::load_from(&path).expect("the legacy shape must still parse");
    assert_eq!(policy.len(), 1);

    let restarted = restarted_resolver(&LearnedGrants::new(&path, WS_A));
    assert!(
        restarted.is_tool_name_auto_approved("Write"),
        "a grant written before this change must still restore"
    );
    assert!(
        matches!(
            policy.evaluate("Write", ""),
            wcore_permissions::EvalResult::Match { allow: true, .. }
        ),
        "the legacy rule must still answer `evaluate` — the sub-agent ACL \
         pre-filter reads the same file"
    );
}
