//! FerroxLabs/wayland-core#376 c1 — measure the per-operation cost of a
//! `SecretDenyFs` guard on an ORDINARY (non-secret, non-store) path.
//!
//! Run as `cargo run --release --example secret_deny_cost -- <ops>`. Prints one
//! line per VFS operation with the mean wall time per call. The `<ops>`
//! argument exists so the same binary can be straced twice with different
//! counts: the DIFFERENCE in syscall counts divided by the difference in ops is
//! the per-guard syscall figure, with all harness/one-off cost cancelled out.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

fn main() {
    let ops: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    // Which loop to run. One loop per process so a differential strace
    // attributes every syscall to a single operation.
    let only = std::env::args().nth(2).unwrap_or_else(|| "all".to_string());

    let dir = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(dir.path()).expect("canon");
    // A realistic workspace: a real .git with a loose object, plus the ordinary
    // file every measured op touches.
    std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
    std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
    std::fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
    std::fs::write(root.join(".git/objects/ab/cdef"), b"x").unwrap();
    std::fs::create_dir_all(root.join("src/deep/deeper")).unwrap();
    let ordinary: PathBuf = root.join("src/deep/deeper/main.rs");
    std::fs::write(&ordinary, b"fn main() {}\n").unwrap();

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let stack = SandboxedFs::new(SecretDenyFs::new(RealFs, Arc::clone(&policy)), root.clone());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");

    rt.block_on(async {
        // Warm: the first touch pays page-cache and dentry cost that is not
        // the guard's.
        let _ = stack.exists(&ordinary).await;

        for (name, kind) in [("read", 0u8), ("exists", 1), ("metadata", 2), ("list", 3)] {
            if only != "all" && only != name {
                continue;
            }
            let target = if kind == 3 {
                root.join("src/deep/deeper")
            } else {
                ordinary.clone()
            };
            let start = Instant::now();
            for _ in 0..ops {
                match kind {
                    0 => {
                        stack.read(&target).await.unwrap();
                    }
                    1 => {
                        stack.exists(&target).await.unwrap();
                    }
                    2 => {
                        stack.metadata(&target).await.unwrap();
                    }
                    _ => {
                        stack.list(&target).await.unwrap();
                    }
                }
            }
            let elapsed = start.elapsed();
            println!(
                "{name:9} ops={ops} total={elapsed:?} per_op={:.3}us",
                elapsed.as_secs_f64() * 1e6 / ops as f64
            );
        }

        // Isolate the two predicates from the surrounding VFS work.
        for (name, which) in [("is_project_secret", 0u8), ("is_vcs_content_store", 1u8)] {
            if only != "all" && only != name {
                continue;
            }
            let start = Instant::now();
            let mut acc = 0usize;
            for _ in 0..ops {
                let hit = if which == 0 {
                    policy.is_project_secret(&ordinary)
                } else {
                    policy.is_vcs_content_store(&ordinary)
                };
                acc += usize::from(hit);
            }
            let elapsed = start.elapsed();
            println!(
                "{name:22} ops={ops} hits={acc} total={elapsed:?} per_call={:.3}us",
                elapsed.as_secs_f64() * 1e6 / ops as f64
            );
        }
    });
}
