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
