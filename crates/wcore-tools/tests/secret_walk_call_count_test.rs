//! #1111 acceptance bullet 1, graded with the instrument the bullet named.
//!
//! The bullet reads: *"A repeated `echo` in a large workspace does not repeat
//! the walk (assert call count via an injected counter, not wall-clock)."*
//!
//! It has two halves and they do not share a verdict.
//!
//! * **The instrument** — *"assert call count via an injected counter, not
//!   wall-clock"* — is achievable and was missing. Before this file the only
//!   walk-count assertion in the tree was a wall-clock RATIO
//!   (`walk_parallel_identity_test::redundant_walk_root_is_not_walked_twice`),
//!   which is exactly the instrument the bullet rules out, and which on a busy
//!   96-core build host measures the scheduler as much as the walk.
//!   [`walk_calls`] is that counter and every test here reads it.
//!
//! * **The requirement** — *"a repeated exec does not repeat the walk"* — is a
//!   memoisation requirement, and it is REFUTED, not merely declined. The
//!   refutation is not a matter of taste and it is not left in a comment:
//!   `the_deny_list_changes_with_no_mutation_call_at_all` below is a red arm
//!   for the cache itself. It shows the correct deny list changing with **no
//!   mutating call of any kind** between the two reads — purely because a
//!   grant's `expires_at` passed — so any cache key that does not contain
//!   "now" returns a stale answer, and a stale answer here is a secret the
//!   child may read. A cache added to satisfy the bullet fails that test.
//!
//! So the walk IS repeated per exec, deliberately, and
//! `a_repeated_exec_walks_the_workspace_again` pins that decision executably.
//! The cost that motivated the bullet is bounded instead — by cancellation and
//! by the timeout (bullets 2 and 3, `bash.rs`) — and tracked as performance in
//! #1113.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use wcore_tools::workspace_policy::{WorkspacePolicy, walk_calls};

/// A tree with one committed secret in it, so a walk that runs has something
/// to find and an assertion on the result cannot be vacuous.
fn tree_with_a_secret(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
    std::fs::write(root.join(".env"), b"TOKEN=hunter2").unwrap();
}

fn denied(policy: &WorkspacePolicy) -> Vec<PathBuf> {
    policy.secret_deny_paths_for_backend(true)
}

/// POSITIVE CONTROL for the instrument itself.
///
/// Every other test in this file reads a difference in [`walk_calls`]. If the
/// counter were never incremented, every one of those differences would be 0
/// and each assertion would be satisfied by an instrument that measures
/// nothing. This test pins both polarities: an enforcing backend costs exactly
/// one walk, and the #922 non-enforcing gate costs exactly zero.
#[test]
fn the_walk_counter_counts_one_walk_per_enforcing_call() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    tree_with_a_secret(&root);
    let policy = WorkspacePolicy::contained(&root);

    let before = walk_calls();
    let deny = policy.secret_deny_paths_for_backend(true);
    let enforcing = walk_calls() - before;

    assert!(
        deny.iter().any(|p| p.ends_with(".env")),
        "the fixture secret is not in the deny set ({deny:?}) - this file's \
         other assertions would be grading an empty walk"
    );
    assert_eq!(
        enforcing, 1,
        "an enforcing backend must cost exactly one walk, counted {enforcing}"
    );

    // NEGATIVE POLARITY: the #922 gate declines to compute the list at all.
    let before = walk_calls();
    let _ = policy.secret_deny_paths_for_backend(false);
    let non_enforcing = walk_calls() - before;
    assert_eq!(
        non_enforcing, 0,
        "a backend that discards the read-deny list must not walk, counted \
         {non_enforcing} - if this is ever non-zero the counter is firing \
         somewhere other than the walk"
    );
}

/// #1111 bullet 1, graded — with the opposite verdict to the one the bullet
/// asks for, and on purpose.
///
/// Three sequential execs cost three walks. There is no memoisation and there
/// must not be one: see `the_deny_list_changes_with_no_mutation_call_at_all`
/// below, which is the executable reason. If you are here because you added a
/// cache and this went red, read that test before deciding this one is wrong.
#[test]
fn a_repeated_exec_walks_the_workspace_again() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    tree_with_a_secret(&root);
    let policy = WorkspacePolicy::contained(&root);

    let before = walk_calls();
    let first = denied(&policy);
    let second = denied(&policy);
    let third = denied(&policy);
    let walks = walk_calls() - before;

    assert!(
        !first.is_empty(),
        "deny set is empty - nothing was walked and this test is vacuous"
    );
    assert_eq!(first, second, "the answer must not vary between execs");
    assert_eq!(second, third, "the answer must not vary between execs");
    assert_eq!(
        walks, 3,
        "three execs walked {walks} times, not 3 - a cache has been added. \
         #1111 bullet 1 asked for exactly that and it is REFUTED: see \
         the_deny_list_changes_with_no_mutation_call_at_all"
    );
}

/// The refutation of #1111 bullet 1, as a test rather than as a comment.
///
/// `readable_roots()` and `session_read_grant_roots()` both filter grants
/// against `SystemTime::now()`, so the correct deny list changes with **no
/// mutating call between the two reads**. Nothing is written, nothing is
/// revoked, no API is touched: a deadline simply passes. A memoised list keyed
/// on anything but "now" therefore keeps denying a path the grant no longer
/// covers, or worse keeps a granted subtree's secrets in a list the session has
/// stopped being entitled to compute.
///
/// This is the red arm for the cache #1111 bullet 1 requests: add a
/// session-lifetime memo and this test fails.
#[test]
fn the_deny_list_changes_with_no_mutation_call_at_all() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workspace");
    let elsewhere = tmp.path().join("elsewhere");
    tree_with_a_secret(&root);
    tree_with_a_secret(&elsewhere);
    std::fs::write(elsewhere.join("id_rsa"), b"-----BEGIN PRIVATE KEY-----").unwrap();

    let policy = WorkspacePolicy::contained(&root).with_local_operator_principal();
    let expiry = SystemTime::now() + Duration::from_millis(750);
    policy
        .grant_session_read_root_full(&elsewhere, false, Some("g1".into()), Some(expiry))
        .expect("the grant must be accepted or this test proves nothing");

    let with_live_grant = denied(&policy);
    let outside = |set: &[PathBuf]| set.iter().filter(|p| p.starts_with(&elsewhere)).count();
    assert!(
        outside(&with_live_grant) > 0,
        "the live grant contributed no denied path ({with_live_grant:?}) - \
         there is no difference for the expiry to remove"
    );

    // The ONLY thing that happens between the two reads.
    std::thread::sleep(Duration::from_millis(900));

    let after_expiry = denied(&policy);
    assert_eq!(
        outside(&after_expiry),
        0,
        "the expired grant still contributes denied paths ({after_expiry:?}) - \
         either expiry stopped being honoured, or a cache is serving a list \
         computed while the grant was live"
    );
    assert_ne!(
        with_live_grant, after_expiry,
        "the deny list did not change across a grant expiry with no mutating \
         call in between - if this ever holds, a cache key without \"now\" in \
         it would be sound and #1111 bullet 1 could be reconsidered"
    );
}

/// The walk-root dedup (`walk_root_is_covered`), asserted by COUNT.
///
/// `walk_parallel_identity_test::redundant_walk_root_is_not_walked_twice`
/// asserts the same property as a wall-clock ratio under 1.35x. That is the
/// instrument #1111 bullet 1 rules out and it is also the flaky one on a shared
/// build host. This is the same property, counted.
#[test]
fn a_redundant_grant_root_is_not_walked_twice() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    tree_with_a_secret(&root);

    let policy = WorkspacePolicy::contained(&root).with_local_operator_principal();
    policy
        .grant_session_read_root(&root, false)
        .expect("granting the workspace root must be accepted or nothing is deduped");
    assert_eq!(
        policy.session_read_grant_roots().len(),
        1,
        "the grant was not recorded - this test would pass on an empty grant list"
    );

    let before = walk_calls();
    let deny = denied(&policy);
    let walks = walk_calls() - before;

    assert!(
        !deny.is_empty(),
        "deny set is empty - the fixture emitted nothing and this is vacuous"
    );
    assert_eq!(
        walks, 1,
        "a grant already covered by the workspace-root walk cost {walks} walks \
         for a byte-identical deny set - the walk roots are not deduplicated"
    );
}

/// NEGATIVE CONTROL for `a_redundant_grant_root_is_not_walked_twice`.
///
/// A grant on a genuinely disjoint tree is real new work and must still cost a
/// second walk. Without this, a dedup that skipped EVERY grant would pass the
/// test above while silently narrowing the deny set.
#[test]
fn a_disjoint_grant_root_really_is_walked() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workspace");
    let elsewhere = tmp.path().join("elsewhere");
    tree_with_a_secret(&root);
    tree_with_a_secret(&elsewhere);

    let policy = WorkspacePolicy::contained(&root).with_local_operator_principal();
    policy
        .grant_session_read_root(&elsewhere, false)
        .expect("the disjoint grant must be accepted or this control proves nothing");

    let before = walk_calls();
    let deny = denied(&policy);
    let walks = walk_calls() - before;

    assert!(
        deny.iter().any(|p| p.starts_with(&elsewhere)),
        "the disjoint grant contributed nothing to the deny set ({deny:?}) - \
         a granted folder's secrets must be denied too"
    );
    assert_eq!(
        walks, 2,
        "a disjoint grant cost {walks} walks, expected 2 - if this drops to 1 \
         the dedup has started skipping grants it does not cover"
    );
}
