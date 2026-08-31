//! SECOND-INSTRUMENT probe (not a gate): what does the core#406 c1 post-walk
//! freshness check cost when a witness is CHURNING, i.e. when a vendored
//! checkout is being written to while the VFS guard runs?
//!
//! The lane states the closure`s price as "+2 probes per nested checkout, +0
//! per workspace directory". This measures the same quantity with the vendored
//! `.git` directory`s mtime moving between guards, which is what every `git`
//! command in that checkout does (index.lock create/remove).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

const SETTLE: Duration = Duration::from_millis(60);

fn stack(policy: &Arc<WorkspacePolicy>, root: &Path) -> SandboxedFs<SecretDenyFs<RealFs>> {
    SandboxedFs::new(
        SecretDenyFs::new(RealFs, Arc::clone(policy)),
        root.to_path_buf(),
    )
}

fn build(checkouts: usize, extra_dirs: usize) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("workspace");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
    std::fs::write(root.join(".git/objects/ab/cdef"), b"x").unwrap();
    std::fs::create_dir_all(root.join("src/deep/deeper")).unwrap();
    std::fs::write(root.join("src/deep/deeper/main.rs"), b"fn main() {}\n").unwrap();
    for i in 0..checkouts {
        let git = root.join(format!("vendor/pkg{i}/.git"));
        std::fs::create_dir_all(git.join("objects/ab")).unwrap();
        std::fs::write(git.join("HEAD"), b"ref: refs/heads/main").unwrap();
    }
    for i in 0..extra_dirs {
        std::fs::create_dir_all(root.join(format!("pkg{i}/src"))).unwrap();
    }
    (dir, root)
}

/// probes-per-guard and walks-per-guard on an ORDINARY (admitted) path while
/// the vendored checkout`s control directory is touched between guards.
async fn churned(checkouts: usize, extra_dirs: usize) -> (u64, u64) {
    let (_dir, root) = build(checkouts, extra_dirs);
    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    let ordinary = root.join("src/deep/deeper/main.rs");
    tokio::time::sleep(SETTLE).await;
    fs.exists(&ordinary).await.expect("ordinary readable");

    let (_, _, before) = policy.guard_cost();
    let walks_before = policy.nested_walk_count();
    const N: u64 = 20;
    for i in 0..N {
        if checkouts > 0 {
            // exactly what `git` does: create then remove a lock file inside
            // the vendored control directory. Moves ONLY that directory`s mtime.
            let lock = root.join("vendor/pkg0/.git").join(format!("index.lock{i}"));
            std::fs::write(&lock, b"").unwrap();
            std::fs::remove_file(&lock).unwrap();
        }
        fs.exists(&ordinary).await.expect("ordinary readable");
    }
    let (_, _, after) = policy.guard_cost();
    (
        (after - before) / N,
        (policy.nested_walk_count() - walks_before) / N,
    )
}

#[tokio::test]
async fn i2_probe_churned_witness_cost_vs_directory_count() {
    // control: no checkout at all, so no witness can churn.
    let none_small = churned(0, 4).await;
    let none_large = churned(0, 44).await;
    // the measurement: one vendored checkout, churning.
    let one_small = churned(1, 4).await;
    let one_large = churned(1, 44).await;
    println!(
        "I2-CHURN  none(4dirs)={none_small:?} none(44dirs)={none_large:?}  \
         one(4dirs)={one_small:?} one(44dirs)={one_large:?}"
    );
    let slope = (one_large.0 as f64 - one_small.0 as f64) / 40.0;
    println!("I2-CHURN slope_probes_per_extra_directory={slope:.3}");
    assert_eq!(
        one_small.0, one_large.0,
        "PER-GUARD PROBE COUNT SCALES WITH THE TREE under witness churn: \
         {} probes at 4 extra dirs vs {} at 44 (slope {slope:.3}/dir), walks/guard \
         {} vs {}",
        one_small.0, one_large.0, one_small.1, one_large.1
    );
}

/// How long does ONE write into a vendored control directory keep the whole
/// workspace re-walking? Amplification of a single mutation.
#[tokio::test]
async fn i2_probe_single_touch_amplification() {
    let (_dir, root) = build(1, 44);
    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    let ordinary = root.join("src/deep/deeper/main.rs");
    tokio::time::sleep(SETTLE).await;
    fs.exists(&ordinary).await.expect("readable");
    tokio::time::sleep(SETTLE).await;
    let (_, _, before) = policy.guard_cost();
    let w0 = policy.nested_walk_count();
    // ONE lock file, as `git status` writes and removes.
    let lock = root.join("vendor/pkg0/.git/index.lock");
    std::fs::write(&lock, b"").unwrap();
    std::fs::remove_file(&lock).unwrap();
    const N: u64 = 50;
    for _ in 0..N {
        fs.exists(&ordinary).await.expect("readable");
    }
    let (_, _, after) = policy.guard_cost();
    println!(
        "I2-SINGLE-TOUCH walks_for_{N}_guards={} probes_total={}",
        policy.nested_walk_count() - w0,
        after - before
    );
    assert_eq!(
        policy.nested_walk_count() - w0,
        1,
        "one mutation should cost at most ONE re-walk"
    );
}

/// WRONG-REFUSAL control, written by the second instrument rather than the
/// lane: after the freshness check fires and re-walks, does ordinary traffic
/// still get through?
#[tokio::test]
async fn i2_wrong_refusal_control_after_a_refire() {
    let (_dir, root) = build(1, 6);
    // legitimate content INSIDE the vendored checkout`s working tree, and a
    // directory whose name is store-ish but which is not a store.
    std::fs::write(root.join("vendor/pkg0/README.md"), b"hello").unwrap();
    std::fs::create_dir_all(root.join("modules/vpc")).unwrap();
    std::fs::write(root.join("modules/vpc/main.tf"), b"# tf").unwrap();
    std::fs::create_dir_all(root.join("assets/objects")).unwrap();
    std::fs::write(root.join("assets/objects/logo.png"), b"png").unwrap();
    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    tokio::time::sleep(SETTLE).await;
    let legit = [
        root.join("src/deep/deeper/main.rs"),
        root.join("vendor/pkg0/README.md"),
        root.join("modules/vpc/main.tf"),
        root.join("assets/objects/logo.png"),
    ];
    for p in &legit {
        assert!(fs.exists(p).await.is_ok(), "before churn: {p:?}");
    }
    let w0 = policy.nested_walk_count();
    let lock = root.join("vendor/pkg0/.git/index.lock");
    std::fs::write(&lock, b"").unwrap();
    std::fs::remove_file(&lock).unwrap();
    for p in &legit {
        let r = fs.read(p).await;
        assert!(r.is_ok(), "WRONG REFUSAL after the freshness check refired: {p:?} -> {r:?}");
    }
    // and the real store is still refused (known-positive control)
    let obj = root.join("vendor/pkg0/.git/objects/ab");
    std::fs::write(root.join("vendor/pkg0/.git/objects/ab/cd"), b"secret").unwrap();
    let _ = obj;
    let denied = fs.read(&root.join("vendor/pkg0/.git/objects/ab/cd")).await;
    assert!(denied.is_err(), "known-positive control: the store must stay refused, got {denied:?}");
    println!("I2-WRONG-REFUSAL ok; refires={}", policy.nested_walk_count() - w0);
}
