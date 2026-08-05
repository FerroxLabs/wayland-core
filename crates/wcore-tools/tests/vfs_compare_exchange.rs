use std::sync::Arc;

use wcore_tools::vfs::{
    FileContentIdentity, FileMutationOutcome, FileObservation, FilePrecondition, InMemoryFs,
    IntendedFileMutation, RealFs, SandboxedFs, SecretDenyFs, VfsError, VirtualFs,
};
use wcore_tools::workspace_policy::WorkspacePolicy;

fn present(bytes: &[u8]) -> FilePrecondition {
    FilePrecondition::Present(FileContentIdentity::from_bytes(bytes))
}

#[test]
fn file_content_identity_uses_sha256() {
    let identity = FileContentIdentity::from_bytes(b"abc");
    assert_eq!(identity.len, 3);
    assert_eq!(
        identity.sha256_hex(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[tokio::test]
async fn in_memory_compare_exchange_is_deterministic_for_fixture_backends() {
    let fs = InMemoryFs::new();
    let path = std::path::Path::new("/workspace/file.txt");
    let create = IntendedFileMutation::new(FilePrecondition::Absent, b"before".to_vec());

    assert!(matches!(
        fs.compare_exchange_file(path, &create).await.unwrap(),
        FileMutationOutcome::Applied {
            previous: FileObservation::Absent,
            ..
        }
    ));
    assert!(matches!(
        fs.compare_exchange_file(path, &create).await.unwrap(),
        FileMutationOutcome::AlreadyApplied { .. }
    ));

    let update = IntendedFileMutation::new(present(b"before"), b"after".to_vec());
    assert!(matches!(
        fs.compare_exchange_file(path, &update).await.unwrap(),
        FileMutationOutcome::Applied { .. }
    ));
    let stale = IntendedFileMutation::new(present(b"before"), b"stale".to_vec());
    assert!(matches!(
        fs.compare_exchange_file(path, &stale).await.unwrap(),
        FileMutationOutcome::Conflict { .. }
    ));
    assert_eq!(fs.read(path).await.unwrap(), b"after");
}

#[tokio::test]
async fn real_host_files_do_not_advertise_authoritative_compare_exchange() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("file.txt");
    let mutation = IntendedFileMutation::new(FilePrecondition::Absent, b"content".to_vec());

    let error = RealFs
        .compare_exchange_file(&path, &mutation)
        .await
        .expect_err("ordinary host paths have no non-cooperative pathname CAS");
    assert!(error.to_string().contains("unavailable"));
    assert!(!path.exists());
}

#[tokio::test]
async fn wrappers_preserve_containment_and_secret_denial() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let jail = SandboxedFs::new(InMemoryFs::new(), root.path());
    let create = IntendedFileMutation::new(FilePrecondition::Absent, b"escape".to_vec());

    assert!(matches!(
        jail.compare_exchange_file(&outside.path().join("escape.txt"), &create)
            .await,
        Err(VfsError::OutsideSandbox { .. })
    ));

    let secret = root.path().join(".env");
    let inner = InMemoryFs::new();
    inner.write(&secret, b"TOKEN=abc").await.unwrap();
    let policy = Arc::new(WorkspacePolicy::contained(root.path()));
    let denied = SecretDenyFs::new(inner, policy);
    let update = IntendedFileMutation::new(present(b"TOKEN=abc"), b"x".to_vec());
    assert!(matches!(
        denied.compare_exchange_file(&secret, &update).await,
        Err(VfsError::SecretDenied { .. })
    ));
}
