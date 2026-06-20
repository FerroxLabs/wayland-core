//! Integration: the Contained jail denies secret + escape writes, allows source.
use std::sync::Arc;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs, VfsError, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

#[tokio::test]
async fn contained_jail_denies_secret_and_escape_allows_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let policy = Arc::new(WorkspacePolicy::contained(&root));
    // SandboxedFs OUTER, SecretDenyFs INNER — same layering apply_posture installs.
    let jail = SandboxedFs::new(SecretDenyFs::new(RealFs, Arc::clone(&policy)), root.clone());

    jail.write(&root.join("main.rs"), b"fn main(){}")
        .await
        .unwrap();
    assert_eq!(
        jail.read(&root.join("main.rs")).await.unwrap(),
        b"fn main(){}"
    );

    assert!(matches!(
        jail.write(&root.join(".env"), b"T=1").await,
        Err(VfsError::SecretDenied { .. })
    ));
    let outside = std::env::temp_dir().join("wcore-escape-probe.txt");
    assert!(matches!(
        jail.write(&outside, b"x").await,
        Err(VfsError::OutsideSandbox { .. })
    ));
}
