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

/// #1155. This asserted the opposite until the atomic exchange landed:
/// `RealFs` returned `Unsupported`, on the reasoning that a host filesystem
/// cannot hold a pathname against a writer that never agreed to cooperate.
/// `renameat2(RENAME_EXCHANGE)` does exactly that, so the refusal is gone and
/// what is asserted now is that the compare-exchange actually discriminates.
#[tokio::test]
async fn real_host_files_compare_exchange_on_content() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("file.txt");

    let create = IntendedFileMutation::new(FilePrecondition::Absent, b"before".to_vec());
    assert!(matches!(
        RealFs.compare_exchange_file(&path, &create).await.unwrap(),
        FileMutationOutcome::Applied {
            previous: FileObservation::Absent,
            ..
        }
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"before");

    // Re-running the same create is a no-op, not a conflict.
    assert!(matches!(
        RealFs.compare_exchange_file(&path, &create).await.unwrap(),
        FileMutationOutcome::AlreadyApplied { .. }
    ));

    let update = IntendedFileMutation::new(present(b"before"), b"after".to_vec());
    assert!(matches!(
        RealFs.compare_exchange_file(&path, &update).await.unwrap(),
        FileMutationOutcome::Applied { .. }
    ));

    // The pre-image is gone, so the stale replacement must not land -- and
    // the bytes that displaced it must survive untouched.
    let stale = IntendedFileMutation::new(present(b"before"), b"stale".to_vec());
    assert!(matches!(
        RealFs.compare_exchange_file(&path, &stale).await.unwrap(),
        FileMutationOutcome::Conflict {
            current: FileObservation::Present(_),
            // #1248 c4: refused by the pre-flight classification, before
            // anything was published, so no save can have been displaced.
            intercepted_save: None,
        }
    ));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"after",
        "a refused compare-exchange left its bytes behind"
    );

    // And no temp file survives either arm.
    let strays: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n != "file.txt")
        .collect();
    assert!(strays.is_empty(), "left behind {strays:?}");
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

/// #1155, the sub-agent shape. `SandboxedFs` observes through the inner
/// backend and then REBINDS the mutation to the object it saw
/// (`vfs.rs`, `with_expected_object`) before delegating. Over `InMemoryFs`
/// that is exercised above; over `RealFs` it was not, and the identity a real
/// host observation carries is not the one a fixture carries. A decorator
/// that quietly conflicts on every real file would disable the guard for
/// every sub-agent while every unit test stayed green.
#[tokio::test]
async fn a_jailed_real_filesystem_compare_exchanges_inside_its_root() {
    let root = tempfile::tempdir().unwrap();
    let jail = SandboxedFs::new(RealFs, root.path());
    let path = root.path().join("draft.md");

    let create = IntendedFileMutation::new(FilePrecondition::Absent, b"before".to_vec());
    assert!(
        matches!(
            jail.compare_exchange_file(&path, &create).await.unwrap(),
            FileMutationOutcome::Applied { .. }
        ),
        "a jailed create was refused"
    );

    let update = IntendedFileMutation::new(present(b"before"), b"after".to_vec());
    assert!(
        matches!(
            jail.compare_exchange_file(&path, &update).await.unwrap(),
            FileMutationOutcome::Applied { .. }
        ),
        "a jailed update whose preimage still holds was refused"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"after");

    // An outsider save, then a replacement composed against what it replaced.
    std::fs::write(&path, b"user save").unwrap();
    let stale = IntendedFileMutation::new(present(b"after"), b"stale".to_vec());
    assert!(matches!(
        jail.compare_exchange_file(&path, &stale).await.unwrap(),
        FileMutationOutcome::Conflict { .. }
    ));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"user save",
        "a jailed compare-exchange overwrote a save it should have refused"
    );
}
