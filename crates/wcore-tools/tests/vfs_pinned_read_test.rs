//! Deterministic per-guard tests for the handle-pinned read (#1105).
//!
//! `path_grant_race_test.rs` proves the window is real and that the fix closes
//! it, but a wall-clock race cannot say WHICH guard did the closing. Each test
//! here targets exactly one refusal in `vfs_pinned.rs` and carries a positive
//! control through the ordinary path-based `RealFs::read` on the same fixture,
//! so a refusal is the guard talking and never a broken fixture.

use wcore_tools::vfs::{RealFs, VfsError, VirtualFs};

/// GUARD: `O_NOFOLLOW` on the leaf `openat` (unix) / `FILE_OPEN_REPARSE_POINT`
/// plus the `FILE_ATTRIBUTE_REPARSE_POINT` refusal (windows).
///
/// This is the single guard the race in `path_grant_race_test.rs` depends on:
/// the swapped-in object there is a symlink, and refusing to follow it at open
/// time is what stops the bytes coming from outside the grant.
#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_leaf_is_refused_by_the_pinned_read() {
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let real = dir.join("real.txt");
    std::fs::write(&real, b"real bytes").unwrap();
    let link = dir.join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    // Positive control: the fixture is sound and the bytes are reachable by
    // name, so the refusal below is a decision and not an absent file.
    assert_eq!(
        RealFs.read(&link).await.unwrap(),
        b"real bytes".to_vec(),
        "positive control: the ordinary path-based read follows the symlink"
    );

    let error = RealFs
        .read_pinned(&link)
        .await
        .expect_err("a pinned read must not follow a symlink at the leaf");
    assert!(
        matches!(&error, VfsError::PathRaced { .. }),
        "the refusal must name the identity mismatch, got: {error}"
    );

    // And the guard is narrow: the real file still reads.
    assert_eq!(
        RealFs.read_pinned(&real).await.unwrap(),
        b"real bytes".to_vec()
    );
}

/// GUARD: `O_DIRECTORY | O_NOFOLLOW` on the parent open (unix).
///
/// The leaf guard alone is not enough — swapping the DIRECTORY the leaf lives
/// in redirects the read just as effectively, and it is a swap the leaf's own
/// `O_NOFOLLOW` cannot see.
#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_parent_is_refused_by_the_pinned_read() {
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let real_dir = dir.join("realdir");
    std::fs::create_dir(&real_dir).unwrap();
    std::fs::write(real_dir.join("file.txt"), b"in the real dir").unwrap();
    let link_dir = dir.join("linkdir");
    std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();

    let through_link = link_dir.join("file.txt");
    assert_eq!(
        RealFs.read(&through_link).await.unwrap(),
        b"in the real dir".to_vec(),
        "positive control: the ordinary path-based read traverses the linked dir"
    );

    let error = RealFs
        .read_pinned(&through_link)
        .await
        .expect_err("a pinned read must not traverse a symlinked parent");
    assert!(
        matches!(&error, VfsError::PathRaced { .. }),
        "the refusal must name the identity mismatch, got: {error}"
    );

    assert_eq!(
        RealFs
            .read_pinned(&real_dir.join("file.txt"))
            .await
            .unwrap(),
        b"in the real dir".to_vec(),
        "the same bytes through the real directory are still readable"
    );
}

/// GUARD: the ABSENCE of `observe_unix_file`'s `metadata.nlink() != 1`
/// refusal.
///
/// That rule belongs to compare-exchange, where a second name for the same
/// inode defeats the rename-based commit. It has no business on an ordinary
/// read: hardlinks are everywhere in `.git` object stores and package caches,
/// and copying the CAS refusal across would break reading them. This test is
/// what stops that copy-paste.
#[cfg(unix)]
#[tokio::test]
async fn a_hardlinked_file_is_still_readable_through_the_pinned_read() {
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let original = dir.join("original.txt");
    std::fs::write(&original, b"two names, one inode").unwrap();
    let second = dir.join("second.txt");
    std::fs::hard_link(&original, &second).unwrap();
    assert_eq!(
        std::fs::metadata(&original)
            .map(|m| std::os::unix::fs::MetadataExt::nlink(&m))
            .unwrap(),
        2,
        "fixture check: the file really does have two links"
    );

    for name in [&original, &second] {
        assert_eq!(
            RealFs.read_pinned(name).await.unwrap(),
            b"two names, one inode".to_vec(),
            "a hardlinked regular file is an ordinary readable file"
        );
    }
}

/// GUARD: the `metadata.is_file()` refusal, taken from the OPEN descriptor,
/// plus `O_NONBLOCK` on the `openat`.
///
/// A FIFO planted where a regular file was is not an escape — it is a wedge.
/// Without `O_NONBLOCK` the `open` itself blocks until someone writes, which
/// hangs the turn with no error to report; without `is_file` the read fails
/// with an incidental `EWOULDBLOCK` rather than the honest refusal. Both
/// mutations fail this test.
#[cfg(unix)]
#[tokio::test]
async fn a_fifo_is_refused_instead_of_hanging_the_turn() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let fifo = dir.join("pipe");
    let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: `name` is a live NUL-terminated path inside a fresh temp dir.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);

    // Positive control: an ordinary regular file in the same directory reads,
    // so the refusal below is about the object's type and not the directory.
    std::fs::write(dir.join("plain.txt"), b"plain").unwrap();
    assert_eq!(
        RealFs.read_pinned(&dir.join("plain.txt")).await.unwrap(),
        b"plain".to_vec()
    );

    let outcome = tokio::time::timeout(Duration::from_secs(5), RealFs.read_pinned(&fifo))
        .await
        .expect("a pinned read of a FIFO must return, not block waiting for a writer");
    let error = outcome.expect_err("a FIFO is not a readable regular file");
    assert!(
        matches!(&error, VfsError::PathRaced { .. }),
        "the refusal must name the type mismatch, got: {error}"
    );
}

/// The pinned read is not allowed to be stricter than the ordinary read for
/// anything legitimate. Ordinary files, empty files, binary content and files
/// reached through a directory the caller named exactly all still work.
#[tokio::test]
async fn ordinary_files_read_identically_through_both_paths() {
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let cases: [(&str, &[u8]); 3] = [
        ("empty.txt", b""),
        ("text.txt", b"hello\nworld\n"),
        ("binary.bin", &[0_u8, 1, 2, 255, 254]),
    ];
    for (name, bytes) in cases {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(
            RealFs.read_pinned(&path).await.unwrap(),
            RealFs.read(&path).await.unwrap(),
            "{name}: the pinned read must return exactly what the ordinary read does"
        );
        assert_eq!(RealFs.read_pinned(&path).await.unwrap(), bytes.to_vec());
    }
}

/// A missing file must still be reported as missing. `FileHistory` treats
/// `ErrorKind::NotFound` as "no cursor yet"; if the pinned read reported an
/// absent file as some other error, that would become a hard failure instead
/// of a fresh start.
#[tokio::test]
async fn a_missing_file_is_still_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();

    let error = RealFs
        .read_pinned(&dir.join("nope.txt"))
        .await
        .expect_err("an absent file cannot be read");
    assert!(
        matches!(&error, VfsError::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
        "an absent leaf must be NotFound, got: {error}"
    );

    let error = RealFs
        .read_pinned(&dir.join("nodir").join("nope.txt"))
        .await
        .expect_err("an absent parent cannot be read either");
    assert!(
        matches!(&error, VfsError::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
        "an absent parent must be NotFound, got: {error}"
    );
}

/// REGRESSION: a decorator in the middle of the jail stack must FORWARD
/// `read_pinned`, not inherit the trait default.
///
/// The default refuses instead of falling back to `read`, deliberately, so the
/// TOCTOU window stays shut. The consequence is that any layer which forgets to
/// forward does not degrade the pin — it breaks EVERY read through the jail,
/// including ordinary project files that have nothing to do with that layer's
/// concern. `SecretDenyFs` carries a comment saying exactly this, and
/// `RepoControlDenyFs` was still added without the forward: the workspace
/// posture then failed `read inside the workspace root must succeed`.
///
/// Graded through `SandboxedFs::read`, which is the production entry point and
/// the thing that actually calls `inner.read_pinned`. The jail deliberately does
/// NOT implement `read_pinned` itself, so calling that on the jail would only
/// exercise the trait default and would fail whether or not the bug is present.
///
/// This grades the composed stack rather than one decorator, so the next layer
/// inserted here is covered without anyone remembering to extend the test.
#[tokio::test]
async fn the_composed_jail_stack_still_forwards_a_pinned_read() {
    use std::sync::Arc;
    use wcore_tools::vfs::{RepoControlDenyFs, SandboxedFs, SecretDenyFs};
    use wcore_tools::workspace_policy::WorkspacePolicy;

    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let ordinary = root.join("src/main.rs");
    std::fs::write(&ordinary, b"fn main() {}\n").unwrap();

    // Positive control: the fixture is readable by name, so a refusal below is
    // a decision by the stack and not a missing or unreadable file.
    assert_eq!(
        RealFs.read(&ordinary).await.unwrap(),
        b"fn main() {}\n".to_vec(),
        "control: the fixture must be readable through the plain path"
    );

    // The exact production composition from `channel_tools::apply_posture`.
    let policy = Arc::new(WorkspacePolicy::contained(root.clone()));
    let jail = SandboxedFs::new(
        RepoControlDenyFs::new(
            SecretDenyFs::new(RealFs, Arc::clone(&policy)),
            Arc::clone(&policy),
        ),
        root.clone(),
    );

    assert_eq!(
        jail.read(&ordinary).await.unwrap(),
        b"fn main() {}\n".to_vec(),
        "a jailed read of an ordinary project file must survive every decorator"
    );

    // The repo-control surface is WRITE-denied, never READ-denied, so a pinned
    // read of it must also succeed — guarding it here would be the opposite bug.
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let head = root.join(".git/HEAD");
    std::fs::write(&head, b"ref: refs/heads/main\n").unwrap();
    assert_eq!(
        jail.read(&head).await.unwrap(),
        b"ref: refs/heads/main\n".to_vec(),
        "the repo-control surface is write-denied, not read-denied"
    );

    // Control in the other direction: the layer is still doing its job.
    assert!(
        jail.write(&head, b"ref: refs/heads/owned\n").await.is_err(),
        "control: writes into the repo-control surface must still be refused"
    );
}
