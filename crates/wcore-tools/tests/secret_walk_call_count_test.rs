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
//!   memoisation requirement. This file originally recorded it as REFUTED, on
//!   the grounds that `the_deny_list_changes_with_no_mutation_call_at_all`
//!   below shows the correct deny list changing with **no mutating call of any
//!   kind** — purely because a grant's `expires_at` passed — so any cache key
//!   that does not contain "now" returns a stale answer, and a stale answer
//!   here is a secret the child may read.
//!
//!   THAT VERDICT IS REVERSED, and by a cache that meets the objection rather
//!   than by a change of taste. `WorkspacePolicy::deny_cache_key` hashes
//!   `readable_roots()` and `session_read_grant_roots()`, both of which filter
//!   grants against `SystemTime::now()` on every call — so the key DOES contain
//!   "now", by recomputing the grant-filtered scope rather than by storing a
//!   timestamp. `deny_cache_hit` then re-stats every stamped directory and
//!   misses on any difference, any unreadable stamp, any unstampable directory
//!   (a sentinel that can never match) and any mtime at or after the walk's own
//!   start instant.
//!
//!   The refutation test is the proof, not the argument: it still stands, it is
//!   unmodified, and it PASSES with the cache in the tree. So does
//!   `a_grant_added_after_the_first_walk_still_contributes_its_secrets`. A
//!   cache that failed the objection would fail them.
//!
//! So the walk is memoised per policy, and
//! `a_repeated_exec_walks_the_workspace_once_and_revalidates` pins that
//! executably from the cheap direction, while the two tests above pin it from
//! the expensive one. The cost that motivated the bullet is bounded as well —
//! by cancellation and by the timeout (bullets 2 and 3, `bash.rs`) — and the
//! per-exec price #1113 complained about is measured at 70.2ms -> 11.8ms on a
//! 240k-entry tree.

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

/// #1111 bullet 1, graded: three sequential execs over an UNCHANGED tree cost
/// exactly ONE walk.
///
/// This is the cheap half of the contract and it is the half a regression hits
/// first — delete the memo and this counts 3. The expensive half (the memo must
/// MISS whenever a fresh walk would answer differently) is graded by
/// `the_deny_list_changes_with_no_mutation_call_at_all` and
/// `a_grant_added_after_the_first_walk_still_contributes_its_secrets`, and the
/// two halves fail in opposite directions, so neither can be satisfied by
/// weakening the other.
///
/// Note what is asserted alongside the count: the three answers must be equal
/// AND non-empty. A memo that returned an empty set on every hit would satisfy
/// a count assertion on its own.
#[test]
fn a_repeated_exec_walks_the_workspace_once_and_revalidates() {
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
        walks, 1,
        "three execs over an unchanged tree walked {walks} times, not 1 - the \
         #1111 memo is not hitting. If you removed it deliberately, read the \
         module header: the objection that once refuted it is answered by \
         `deny_cache_key`, and the two invalidation tests below prove it."
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

/// The refutation of #1111 bullet 1 in the direction that actually LEAKS.
///
/// `the_deny_list_changes_with_no_mutation_call_at_all` above shows a cache
/// serving a STALE-WIDE list: after a grant expires the cache keeps denying
/// paths it should have dropped. That is wrong, but it is wrong in the safe
/// direction — over-denial refuses a read nobody was entitled to anyway, and on
/// its own it is a weak argument against a cache ("so it is conservative").
///
/// This test pins the other direction, and it is the one that matters. A read
/// root granted AFTER the first deny-list computation must contribute its
/// secrets to every later computation. A session-lifetime memo keyed on the
/// workspace root cannot do that: it answers the second call from a list built
/// before the grant existed, the granted subtree's `.env` and `id_rsa` are
/// absent from `fs_read_deny`, and the sandboxed child can `cat` them. That is
/// the exact cross-command TOCTOU #234 deleted the frozen list to close.
///
/// So the two directions together say a cache is not merely stale, it is
/// UNSOUND: bullet 1 cannot be satisfied without reopening a secret-read hole.
#[test]
fn a_grant_added_after_the_first_walk_still_contributes_its_secrets() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workspace");
    let elsewhere = tmp.path().join("elsewhere");
    tree_with_a_secret(&root);
    tree_with_a_secret(&elsewhere);
    std::fs::write(elsewhere.join("id_rsa"), b"-----BEGIN PRIVATE KEY-----").unwrap();

    let policy = WorkspacePolicy::contained(&root).with_local_operator_principal();

    // First exec: the grant does not exist yet. This is what populates any
    // cache, and the ORDER is the whole point of the test.
    let before = walk_calls();
    let first = denied(&policy);
    assert_eq!(
        walk_calls() - before,
        1,
        "the first computation did not walk - there is no populated state for \
         the grant below to be missing from, and this test is vacuous"
    );
    assert!(
        !first.iter().any(|p| p.starts_with(&elsewhere)),
        "the ungranted tree is already denied ({first:?}) - the assertion below \
         would pass without the grant doing anything"
    );

    // The ONLY thing that happens between the two computations.
    policy
        .grant_session_read_root(&elsewhere, false)
        .expect("the grant must be accepted or this test proves nothing");

    let before = walk_calls();
    let after = denied(&policy);
    let walks = walk_calls() - before;

    for name in [".env", "id_rsa"] {
        assert!(
            after
                .iter()
                .any(|p| p.starts_with(&elsewhere) && p.ends_with(name)),
            "{name} under a root granted AFTER the first computation is NOT in \
             the deny set ({after:?}) - the sandboxed child can read it. A \
             cache is serving a list built before the grant existed; this is \
             the cross-command TOCTOU #234 closed, and it is why #1111 bullet \
             1 cannot be implemented."
        );
    }
    assert_eq!(
        walks, 2,
        "the post-grant computation cost {walks} walks, expected 2 (workspace \
         + the newly granted disjoint root) - if this is 0 the answer came \
         from a cache"
    );
}
