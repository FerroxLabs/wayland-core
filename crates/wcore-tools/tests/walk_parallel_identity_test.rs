//! #1113 — the sub-agent secret-deny walk cost, and the identity contract any
//! cheaper walk must satisfy.
//!
//! Graded at the PRODUCTION entry point, `WorkspacePolicy::secret_deny_paths_dynamic()`
//! — the sole producer of `manifest.fs_read_deny` (`bash.rs`) — not at the private
//! `project_committed_secrets` helper, so a change that speeds the helper up while
//! leaving a caller on the slow path cannot pass.
//!
//! Three tests, deliberately of three different kinds:
//!
//! * `redundant_walk_root_is_not_walked_twice` — RED at the time of writing.
//!   A session read grant whose root is ALREADY covered by the workspace root
//!   walk costs an entire second walk of the same tree and produces a
//!   byte-identical deny set. Measured 88.5 ms -> 177.0 ms on a 90,313-entry
//!   tree, both returning the same 4 paths.
//!
//! * `a_disjoint_grant_really_does_cost_a_second_walk` — the paired NEGATIVE
//!   CONTROL for the test above. A grant on a genuinely separate tree IS new
//!   work and must still show the doubling. Without this, the ratio assertion
//!   above could be satisfied by an instrument that can no longer see a second
//!   walk at all, and would pass vacuously.
//!
//! * `deny_set_is_complete_and_identical_across_repeated_walks` — the IDENTITY
//!   contract. A parallel walk that returns a different set is a security
//!   regression, not an optimisation. Pins membership against a hand-enumerated
//!   nasty tree (gitignored secret, secret under `node_modules/`, secret under
//!   `target/`, benign-named symlink to a secret, secret at depth 8, symlink
//!   loop) and pins run-to-run identity across 25 walks. Carries its own
//!   negative control: a benign file must be ABSENT, so an assertion that
//!   accepted everything would fail.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wcore_tools::workspace_policy::WorkspacePolicy;

/// Dirs x files for the timing fixtures. Big enough that a duplicated walk is
/// unmistakable, small enough to build in well under a second.
const DIRS: usize = 150;
const FILES_PER_DIR: usize = 100;

/// Build a bulk tree of exactly `DIRS * FILES_PER_DIR` files in `DIRS` dirs.
/// The count is known BY CONSTRUCTION — never by walking the tree afterwards,
/// which would warm the page cache for the measurement that follows.
fn build_bulk_tree(root: &Path) {
    for i in 0..DIRS {
        let d = root.join(format!("d{i}"));
        std::fs::create_dir_all(&d).unwrap();
        for j in 0..FILES_PER_DIR {
            std::fs::write(d.join(format!("f{j}.txt")), b"x").unwrap();
        }
    }
    // One real secret so the walk has something to emit and the deny set is
    // not trivially empty (an empty result would make the identity half of the
    // timing tests vacuous).
    std::fs::write(root.join(".env"), b"TOKEN=1").unwrap();
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// Interleaved A/B timing. Both arms are measured inside the same loop so a
/// load ramp on a shared box lands on both arms equally instead of on whichever
/// one ran second.
fn interleaved_medians(
    a: &WorkspacePolicy,
    b: &WorkspacePolicy,
) -> (Duration, Duration, Vec<PathBuf>, Vec<PathBuf>) {
    // Warm through the SUBJECTS themselves. Never enumerate the tree separately
    // to "prepare" it: that walk warms the very cache the measurement reads.
    let mut last_a = a.secret_deny_paths_dynamic();
    let mut last_b = b.secret_deny_paths_dynamic();

    let mut ta = Vec::new();
    let mut tb = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        last_a = a.secret_deny_paths_dynamic();
        ta.push(t.elapsed());
        let t = Instant::now();
        last_b = b.secret_deny_paths_dynamic();
        tb.push(t.elapsed());
    }
    (median(ta), median(tb), last_a, last_b)
}

/// RED ARM. A grant whose root is already covered by the workspace root walk
/// must not buy a second full walk of the same tree.
///
/// `grant_capacity` refuses a new grant that is UNDER an existing grant, but it
/// never compares the grant against `self.root` — so granting the workspace
/// root itself (or any ancestor of an existing grant) is recorded, and
/// `secret_deny_paths_dynamic` then walks the same tree twice for a
/// byte-identical answer.
#[test]
fn redundant_walk_root_is_not_walked_twice() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    build_bulk_tree(&root);

    // `contained` is the sub-agent posture (spawner hardcodes it); the local
    // operator flag is what makes grants grantable at all. Both policies see
    // the SAME tree, so any time difference is duplicated work, not a bigger
    // workload.
    let baseline = WorkspacePolicy::contained(&root).with_local_operator_principal();
    let subject = WorkspacePolicy::contained(&root).with_local_operator_principal();

    // The redundant grant: the workspace root, which is walked unconditionally.
    let granted = subject
        .grant_session_read_root(&root, false)
        .expect("granting the workspace root must be accepted for this test to mean anything");
    assert_eq!(
        subject.session_read_grant_roots(),
        vec![granted.clone()],
        "the grant must actually be recorded, or this test measures nothing"
    );

    let (t_base, t_subj, set_base, set_subj) = interleaved_medians(&baseline, &subject);

    // CORRECTNESS FIRST. The redundant walk changes nothing about the answer —
    // which is exactly why paying for it is pure waste.
    assert_eq!(
        set_base, set_subj,
        "a redundant grant must not change the deny set"
    );
    assert!(
        !set_base.is_empty(),
        "deny set is empty - the fixture emitted nothing and this test is vacuous"
    );

    let ratio = t_subj.as_secs_f64() / t_base.as_secs_f64();
    eprintln!(
        "redundant grant: baseline {:.3} ms, subject {:.3} ms, ratio {ratio:.2}x, {} paths",
        t_base.as_secs_f64() * 1000.0,
        t_subj.as_secs_f64() * 1000.0,
        set_base.len()
    );
    assert!(
        ratio < 1.35,
        "a grant already covered by the workspace root walk cost {ratio:.2}x \
         (baseline {:.3} ms, subject {:.3} ms) for a byte-identical deny set - \
         the walk roots are not deduplicated",
        t_base.as_secs_f64() * 1000.0,
        t_subj.as_secs_f64() * 1000.0,
    );
}

/// NEGATIVE CONTROL for `redundant_walk_root_is_not_walked_twice`.
///
/// A grant on a genuinely DISJOINT tree is real new work and must still cost
/// roughly a second walk. If this ever stops holding, the ratio instrument has
/// gone blind and the red arm above can pass without proving anything.
#[test]
fn a_disjoint_grant_really_does_cost_a_second_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("workspace");
    let other = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    build_bulk_tree(&root);
    build_bulk_tree(&other);

    let baseline = WorkspacePolicy::contained(&root).with_local_operator_principal();
    let subject = WorkspacePolicy::contained(&root).with_local_operator_principal();
    subject
        .grant_session_read_root(&other, false)
        .expect("a disjoint folder must be grantable");
    assert_eq!(subject.session_read_grant_roots().len(), 1);

    let (t_base, t_subj, set_base, set_subj) = interleaved_medians(&baseline, &subject);

    let ratio = t_subj.as_secs_f64() / t_base.as_secs_f64();
    eprintln!(
        "disjoint grant: baseline {:.3} ms, subject {:.3} ms, ratio {ratio:.2}x",
        t_base.as_secs_f64() * 1000.0,
        t_subj.as_secs_f64() * 1000.0,
    );

    // The disjoint tree's own `.env` must APPEAR — proof the second walk really
    // ran and really contributed, not just burned time.
    let other_env = std::fs::canonicalize(other.join(".env")).unwrap();
    assert!(
        !set_base.contains(&other_env),
        "control broken: the disjoint secret is in the baseline set"
    );
    assert!(
        set_subj.contains(&other_env),
        "the granted tree's secret must be denied - the second walk did not contribute"
    );

    assert!(
        ratio > 1.55,
        "a disjoint grant of an equally-sized tree only cost {ratio:.2}x - the timing \
         instrument can no longer see a second walk, so the redundant-grant test \
         above would pass vacuously"
    );
}

/// Lay out a tree that exercises every shape the walk has to survive, and
/// return the canonical paths that MUST end up denied plus one that must not.
fn build_nasty_tree(root: &Path) -> (Vec<PathBuf>, PathBuf) {
    std::fs::write(root.join(".gitignore"), b".env\nnode_modules/\ntarget/\n").unwrap();

    // 1. gitignored secret at the top level.
    std::fs::write(root.join(".env"), b"TOKEN=1").unwrap();

    // 2. secret under node_modules/ - the directory a prune would skip.
    let nm = root.join("node_modules/vendor/deep");
    std::fs::create_dir_all(&nm).unwrap();
    std::fs::write(nm.join("client.pem"), b"-----BEGIN-----").unwrap();

    // 3. secret under target/ - the other prune candidate.
    let tg = root.join("target/debug/build/foo-123/out");
    std::fs::create_dir_all(&tg).unwrap();
    std::fs::write(tg.join("service-account.json"), b"{}").unwrap();

    // 4. secret at depth 8, to catch a walk that bounds its depth.
    let deep = root.join("a/b/c/d/e/f/g/h");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("id_rsa"), b"key").unwrap();

    // 5. a benign-NAMED symlink pointing at a secret. The link's own canonical
    //    path must be denied or the mask is trivially defeated by renaming.
    let links = root.join("links");
    std::fs::create_dir_all(&links).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join(".env"), links.join("notes.txt")).unwrap();
        // 6. a dangling symlink - canonicalize fails, the walk must not stop.
        std::os::unix::fs::symlink(root.join("does-not-exist"), links.join("dangling.pem"))
            .unwrap();
        // 7. a symlink LOOP. `follow_links(false)` means this must not hang;
        //    any replacement walk inherits that obligation.
        std::os::unix::fs::symlink(root.join("loop"), root.join("loop")).ok();
    }

    // 8. a directory the process may not be able to read. As root (CAP_DAC_OVERRIDE)
    //    this is descended anyway, so no assertion is made about its CONTENTS -
    //    that would be vacuous on this host. The assertion made is the one that
    //    holds either way: the walk completes and every other secret is still found.
    let locked = root.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("hidden.pem"), b"k").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    }

    // 9. NEGATIVE CONTROL: a plainly benign file that must never be denied.
    std::fs::write(root.join("README.md"), b"hello").unwrap();

    let mut expected = vec![
        std::fs::canonicalize(root.join(".env")).unwrap(),
        std::fs::canonicalize(nm.join("client.pem")).unwrap(),
        std::fs::canonicalize(tg.join("service-account.json")).unwrap(),
        std::fs::canonicalize(deep.join("id_rsa")).unwrap(),
    ];
    expected.sort();
    let benign = std::fs::canonicalize(root.join("README.md")).unwrap();
    (expected, benign)
}

/// IDENTITY CONTRACT. Whatever walks the tree, the answer must be the same set
/// every time and must contain every secret the file tools would refuse.
///
/// The parallel walk this issue sanctions changes the ORDER entries are
/// visited in and the thread they are canonicalized on. Neither may change
/// membership, and neither may make the result vary run to run.
#[test]
fn deny_set_is_complete_and_identical_across_repeated_walks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (expected, benign) = build_nasty_tree(&root);

    let policy = WorkspacePolicy::contained(&root);

    let first = policy.secret_deny_paths_dynamic();
    let first_set: BTreeSet<&PathBuf> = first.iter().collect();

    // POSITIVE: every planted secret is present.
    for want in &expected {
        assert!(
            first_set.contains(want),
            "planted secret {} is missing from the deny set (set: {:?})",
            want.display(),
            first
        );
    }

    // The benign-named symlink's own canonical path resolves to `.env`, which is
    // already asserted above; assert instead that the link cannot be used as an
    // unlisted alias by checking the resolved target is denied.
    #[cfg(unix)]
    {
        let link = root.join("links/notes.txt");
        let resolved = std::fs::canonicalize(&link).unwrap();
        assert!(
            first_set.contains(&resolved),
            "a benign-named symlink to a secret resolves to {} which is not denied",
            resolved.display()
        );
    }

    // NEGATIVE CONTROL: a benign file must be absent. Without this, an
    // implementation that denied the entire workspace would satisfy every
    // positive assertion above.
    assert!(
        !first_set.contains(&benign),
        "the negative control {} was denied - this deny set does not discriminate",
        benign.display()
    );

    // IDENTITY: 25 repeats must be byte-identical, order included. A walk whose
    // output order depends on thread scheduling fails here.
    for i in 1..25 {
        let again = policy.secret_deny_paths_dynamic();
        assert_eq!(
            first, again,
            "walk {i} returned a different deny set than walk 0 - the walk is not deterministic"
        );
    }

    // Hand the 0o000 directory back so `TempDir::drop` can remove it on a
    // non-root runner (its Drop swallows the error and leaks the tree instead).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o700));
    }
}
