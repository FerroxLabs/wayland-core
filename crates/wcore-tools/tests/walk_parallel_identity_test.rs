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
//! * `deny_set_is_complete_..._on_the_serial_arm` / `..._on_the_parallel_arm` —
//!   the IDENTITY contract, asserted ONCE PER ARM. A parallel walk that returns
//!   a different set is a security regression, not an optimisation. Each pins
//!   membership against a hand-enumerated nasty tree (gitignored secret, secret
//!   under `node_modules/`, secret under `target/`, benign-named symlink to a
//!   secret, secret at depth 8, symlink loop, unreadable directory) and pins
//!   run-to-run identity across 25 walks. Each carries its own negative
//!   control: a benign file must be ABSENT, so an assertion that accepted
//!   everything would fail.
//!
//!   The split is load-bearing, not cosmetic. `project_committed_secrets` walks
//!   serially until a tree exceeds `SERIAL_WALK_BUDGET` and only then starts a
//!   thread pool, so a fixture under that budget grades the arm this lane did
//!   NOT change and leaves the new one untested. Each test therefore asserts
//!   which side of the budget its own fixture is on, BY COUNT.
//!
//! * `the_parallel_arm_returns_exactly_what_the_serial_arm_returns` — the
//!   cross-arm equality the two tests above cannot state on their own. The same
//!   nasty tree is built twice, once under the budget and once padded over it
//!   with benign files, and the two deny sets are compared relative to their
//!   roots. Padding is `.txt` only, so the sole difference between the fixtures
//!   is which arm walks them. Carries a positive control: the compared sets
//!   must be non-empty.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wcore_tools::workspace_policy::{SERIAL_WALK_BUDGET, WorkspacePolicy};

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

/// Benign padding, so a fixture can be pushed past [`SERIAL_WALK_BUDGET`]
/// without adding a single path to the deny set. `.txt` files only: the two
/// fixtures below must differ ONLY in which walk arm they take.
fn pad_tree(root: &Path, files: usize) {
    for i in 0..files {
        let d = root.join(format!("pad/d{}", i / 100));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("f{i}.txt")), b"x").unwrap();
    }
}

/// Entries under `dir`, counted WITHOUT following symlinks (the fixture holds a
/// loop). Only ever called AFTER the deny set has been taken — these tests do
/// no timing, but a sizing walk still has no business running first.
fn count_entries(dir: &Path) -> usize {
    let mut n = 1;
    let Ok(read) = std::fs::read_dir(dir) else {
        return n;
    };
    for entry in read.flatten() {
        match std::fs::symlink_metadata(entry.path()) {
            Ok(meta) if meta.is_dir() => n += count_entries(&entry.path()),
            _ => n += 1,
        }
    }
    n
}

/// Hand a 0o000 fixture directory back so `TempDir::drop` can remove it on a
/// non-root runner (its Drop swallows the error and leaks the tree instead).
fn unlock(root: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o700));
    }
    let _ = root;
}

/// Deny-set entries that live under `root`, as paths RELATIVE to it, so two
/// fixtures built in different temp dirs can be compared directly. Everything
/// outside the fixture (system credential stores, the runner's own home) is
/// dropped by the `strip_prefix`, because it is not what these tests are about.
fn relative_set(root: &Path, set: &[PathBuf]) -> BTreeSet<PathBuf> {
    let canon = std::fs::canonicalize(root).unwrap();
    set.iter()
        .filter_map(|p| p.strip_prefix(&canon).ok())
        .map(Path::to_path_buf)
        .collect()
}

/// The identity contract itself, run against whichever arm the caller's fixture
/// size selects: every planted secret present, the benign control absent, and
/// 25 repeats byte-identical. Returns the deny set.
fn assert_identity_contract(root: &Path, expected: &[PathBuf], benign: &PathBuf) -> Vec<PathBuf> {
    let policy = WorkspacePolicy::contained(root);

    let first = policy.secret_deny_paths_dynamic();
    let first_set: BTreeSet<&PathBuf> = first.iter().collect();

    // POSITIVE: every planted secret is present.
    for want in expected {
        assert!(
            first_set.contains(want),
            "planted secret {} is missing from the deny set (set: {first:?})",
            want.display(),
        );
    }

    // A benign-NAMED symlink must not be usable as an unlisted alias: its
    // resolved target has to be denied.
    #[cfg(unix)]
    {
        let resolved = std::fs::canonicalize(root.join("links/notes.txt")).unwrap();
        assert!(
            first_set.contains(&resolved),
            "a benign-named symlink to a secret resolves to {} which is not denied",
            resolved.display(),
        );
    }

    // NEGATIVE CONTROL: a benign file must be absent. Without this, an
    // implementation that denied the entire workspace would satisfy every
    // positive assertion above.
    assert!(
        !first_set.contains(&benign),
        "the negative control {} was denied - this deny set does not discriminate",
        benign.display(),
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
    first
}

/// IDENTITY CONTRACT, SERIAL ARM. The nasty tree on its own is well under
/// `SERIAL_WALK_BUDGET`, so this is the arm that predates the lane.
#[test]
fn deny_set_is_complete_and_identical_on_the_serial_arm() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (expected, benign) = build_nasty_tree(&root);

    assert_identity_contract(&root, &expected, &benign);

    let entries = count_entries(&root);
    assert!(
        entries <= SERIAL_WALK_BUDGET,
        "this fixture has {entries} entries and the serial budget is {SERIAL_WALK_BUDGET} - \
         it takes the PARALLEL arm, so the serial arm is now ungraded"
    );
    unlock(&root);
}

/// IDENTITY CONTRACT, PARALLEL ARM. Same tree, padded past
/// `SERIAL_WALK_BUDGET` with benign files so the thread pool actually starts.
///
/// Without this test the arm this lane added is graded by nothing: a mutation
/// that drops symlink masking in the parallel closure alone passed all 1645
/// `wcore-tools` tests.
#[test]
fn deny_set_is_complete_and_identical_on_the_parallel_arm() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (expected, benign) = build_nasty_tree(&root);
    pad_tree(&root, SERIAL_WALK_BUDGET * 4);

    assert_identity_contract(&root, &expected, &benign);

    let entries = count_entries(&root);
    assert!(
        entries > SERIAL_WALK_BUDGET,
        "this fixture has only {entries} entries against a serial budget of \
         {SERIAL_WALK_BUDGET} - it never leaves the serial arm, so this test \
         grades the parallel walk not at all"
    );
    unlock(&root);
}

/// CROSS-ARM EQUALITY. A faster walk that returns a DIFFERENT set is a security
/// regression, and neither per-arm test above can see that on its own.
///
/// The same nasty tree is built twice — once under the budget, once padded over
/// it — and the deny sets are compared relative to their roots. The padding is
/// benign `.txt`, so the only difference between the two fixtures is which arm
/// walks them.
#[test]
fn the_parallel_arm_returns_exactly_what_the_serial_arm_returns() {
    let tmp = tempfile::tempdir().unwrap();
    let serial_root = tmp.path().join("serial");
    let parallel_root = tmp.path().join("parallel");
    std::fs::create_dir_all(&serial_root).unwrap();
    std::fs::create_dir_all(&parallel_root).unwrap();
    build_nasty_tree(&serial_root);
    build_nasty_tree(&parallel_root);
    pad_tree(&parallel_root, SERIAL_WALK_BUDGET * 4);

    let serial = WorkspacePolicy::contained(&serial_root).secret_deny_paths_dynamic();
    let parallel = WorkspacePolicy::contained(&parallel_root).secret_deny_paths_dynamic();

    let serial_rel = relative_set(&serial_root, &serial);
    let parallel_rel = relative_set(&parallel_root, &parallel);

    // POSITIVE CONTROL: an empty comparison would be satisfied by two walks
    // that both found nothing.
    assert!(
        !serial_rel.is_empty(),
        "the serial fixture denied nothing - this comparison is vacuous"
    );

    assert_eq!(
        serial_rel,
        parallel_rel,
        "the parallel arm returned a different deny set than the serial arm - \
         only in parallel: {:?}, only in serial: {:?}",
        parallel_rel.difference(&serial_rel).collect::<Vec<_>>(),
        serial_rel.difference(&parallel_rel).collect::<Vec<_>>(),
    );

    // The arms were really different arms, by count.
    let (n_serial, n_parallel) = (count_entries(&serial_root), count_entries(&parallel_root));
    assert!(
        n_serial <= SERIAL_WALK_BUDGET && n_parallel > SERIAL_WALK_BUDGET,
        "both fixtures took the same arm ({n_serial} and {n_parallel} entries \
         against a budget of {SERIAL_WALK_BUDGET}) - nothing was compared"
    );

    unlock(&serial_root);
    unlock(&parallel_root);
}
