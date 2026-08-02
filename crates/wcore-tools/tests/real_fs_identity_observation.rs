//! Native proof for `RealFs::observe_file` — the primitive every durable
//! filesystem receipt, crash reconciliation and rollback authority is decided
//! from.
//!
//! Every case here runs on BOTH unix and Windows against the real host
//! filesystem, deliberately. The Windows half of `observe_real_file` is ~200
//! lines of `NtCreateFile` FFI whose identity token, parent binding and
//! refusal set had no test on any platform: a token that is subtly constant,
//! a parent token read from the wrong handle, or a followed reparse point all
//! pass a compile and then silently corrupt reconciliation in production.
//! These assertions are written against the contract, so the unix
//! implementation is held to exactly the same bar and neither platform can
//! drift away from the other unnoticed.
//!
//! The four properties, in the order the crash-recovery path needs them:
//!   1. the identity token is STABLE across an independent reopen, and stable
//!      across a content rewrite of the same object;
//!   2. the identity token DISTINGUISHES two objects with identical bytes;
//!   3. the parent token binds the observation to the exact parent directory
//!      object, so a substituted parent cannot answer for the recorded one;
//!   4. a directory, a reparse point and a multiply-linked file are refused
//!      outright rather than observed.
//!
//! Property 5 closes the loop at the level recovery actually uses: a real
//! receipt built from a real observation must reconcile to `NotStarted` when
//! nothing moved and to `Conflict` when the bytes still match but the object
//! or its parent does not.

use std::path::{Path, PathBuf};

use wcore_tools::effects::{
    FILESYSTEM_EFFECT_RECEIPT_VERSION, FILESYSTEM_EFFECT_RECONCILER, FilesystemContentIdentity,
    FilesystemEffectPrecondition, FilesystemEffectReceiptV1, FilesystemReconciliation,
};
use wcore_tools::vfs::{
    FileContentIdentity, FileObservation, IdentifiedFileObservation, RealFs, VirtualFs,
};

async fn observe(path: &Path) -> IdentifiedFileObservation {
    RealFs
        .observe_file(path)
        .await
        .unwrap_or_else(|error| panic!("observe {}: {error}", path.display()))
}

fn present(observation: FileObservation) -> FileContentIdentity {
    match observation {
        FileObservation::Present(identity) => identity,
        FileObservation::Absent => panic!("expected a present observation"),
    }
}

/// Build the durable receipt exactly as a preparing writer would: the
/// precondition and the prepared object come from a real observation, the
/// receipt path stays in the caller's own spelling (the canonical form is a
/// `\\?\` verbatim path on Windows, which `validate_user_path` refuses).
fn receipt_for(
    path: &Path,
    observed: &IdentifiedFileObservation,
    intended: &[u8],
) -> FilesystemEffectReceiptV1 {
    let (precondition, checkpoint_identity) = match observed.observation {
        FileObservation::Absent => (FilesystemEffectPrecondition::Absent, None),
        FileObservation::Present(identity) => {
            let identity = FilesystemContentIdentity::from(identity);
            (
                FilesystemEffectPrecondition::Present {
                    identity: identity.clone(),
                },
                Some(identity),
            )
        }
    };
    FilesystemEffectReceiptV1 {
        version: FILESYSTEM_EFFECT_RECEIPT_VERSION,
        reconciler: FILESYSTEM_EFFECT_RECONCILER.to_owned(),
        path: path.to_path_buf(),
        preparation_object: observed.object.clone(),
        precondition,
        checkpoint_identity,
        intended: FileContentIdentity::from_bytes(intended).into(),
    }
}

/// Replace `path` with a DIFFERENT filesystem object carrying `bytes`.
///
/// Deliberately not `remove_file` + `write`: a freshly recreated name can be
/// handed back the same MFT record / inode the deleted one had, which would
/// make "the token changed" flaky for reasons that have nothing to do with the
/// property. Renaming a sibling that existed at the same time guarantees a
/// distinct object on every platform.
fn substitute_object(path: &Path, bytes: &[u8]) {
    let replacement = path.with_extension("replacement");
    std::fs::write(&replacement, bytes).expect("write replacement object");
    std::fs::rename(&replacement, path).expect("rename replacement over target");
}

fn temp_root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let root = dir.path().to_path_buf();
    (dir, root)
}

#[tokio::test]
async fn identity_token_is_stable_across_an_independent_reopen() {
    let (_guard, root) = temp_root();
    let path = root.join("stable.txt");
    std::fs::write(&path, b"one").unwrap();

    let first = observe(&path).await;
    let second = observe(&path).await;

    assert_eq!(
        first.object, second.object,
        "two independent observations of one unmoved file must agree on \
         authority, resolved path, parent token and file token"
    );
    assert_eq!(first.observation, second.observation);
    assert!(
        first.object.file.is_some(),
        "a present file must carry a file identity token"
    );
    assert!(
        first.object.parent.is_some(),
        "an existing parent must carry a parent identity token"
    );
    assert!(
        !first.object.authority.is_empty(),
        "the observation must name the authority it was taken under"
    );
}

#[tokio::test]
async fn identity_token_survives_a_content_rewrite_of_the_same_object() {
    let (_guard, root) = temp_root();
    let path = root.join("rewritten.txt");
    std::fs::write(&path, b"one").unwrap();
    let before = observe(&path).await;

    // In-place rewrite: same pathname, same underlying object, new bytes.
    std::fs::write(&path, b"two-different").unwrap();
    let after = observe(&path).await;

    assert_ne!(
        before.observation, after.observation,
        "the content identity must follow the bytes"
    );
    assert_eq!(
        before.object.file, after.object.file,
        "an ordinary write must NOT change the object identity token — on \
         Windows a write sets FILE_ATTRIBUTE_ARCHIVE and the search indexer \
         toggles FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, and folding either into \
         the token would turn routine background activity into a spurious \
         Conflict"
    );
    assert_eq!(before.object.parent, after.object.parent);
    assert_eq!(before.object.path, after.object.path);
}

#[tokio::test]
async fn identity_token_distinguishes_two_objects_with_identical_bytes() {
    let (_guard, root) = temp_root();
    let path = root.join("substituted.txt");
    std::fs::write(&path, b"identical").unwrap();
    let before = observe(&path).await;

    substitute_object(&path, b"identical");
    let after = observe(&path).await;

    assert_eq!(
        before.observation, after.observation,
        "the bytes are deliberately identical — this is the case where a \
         content hash alone cannot tell the two objects apart"
    );
    assert_eq!(before.object.path, after.object.path);
    assert_ne!(
        before.object.file, after.object.file,
        "the identity token must distinguish two different objects that \
         happen to hold the same bytes; a constant or always-None token \
         would pass every stability check and then silently resolve an \
         uncertain effect from matching bytes alone"
    );
}

#[tokio::test]
async fn parent_token_binds_the_observation_to_the_exact_parent_object() {
    let (_guard, root) = temp_root();
    let parent = root.join("parent");
    std::fs::create_dir(&parent).unwrap();
    let path = parent.join("bound.txt");
    std::fs::write(&path, b"identical").unwrap();
    let before = observe(&path).await;

    // Substitute the PARENT DIRECTORY, keeping the pathname and the leaf bytes
    // byte-for-byte identical. This is the window the retained parent handle
    // and the recorded parent token exist to close.
    let staging = root.join("staging");
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(staging.join("bound.txt"), b"identical").unwrap();
    std::fs::rename(&parent, root.join("displaced")).unwrap();
    std::fs::rename(&staging, &parent).unwrap();

    let after = observe(&path).await;

    assert_eq!(
        before.observation, after.observation,
        "same pathname, same bytes — only the parent object changed"
    );
    assert_eq!(before.object.path, after.object.path);
    assert_ne!(
        before.object.parent, after.object.parent,
        "the parent token must be read from the retained parent handle the \
         leaf was opened relative to; if it tracked the pathname instead, a \
         substituted parent would answer for the recorded one"
    );
}

#[tokio::test]
async fn absent_file_still_binds_its_existing_parent() {
    let (_guard, root) = temp_root();
    let parent = root.join("parent");
    std::fs::create_dir(&parent).unwrap();
    let path = parent.join("never-created.txt");

    let observed = observe(&path).await;

    assert_eq!(observed.observation, FileObservation::Absent);
    assert!(
        observed.object.file.is_none(),
        "an absent file has no file identity"
    );
    assert!(
        observed.object.parent.is_some(),
        "an EXISTING parent must still be identified, so a later observation \
         through a substituted parent cannot be mistaken for the same absence"
    );
    assert!(observed.object.path.ends_with("never-created.txt"));
}

#[tokio::test]
async fn absent_parent_yields_absent_without_a_parent_token() {
    let (_guard, root) = temp_root();
    let path = root.join("missing-parent").join("leaf.txt");

    let observed = observe(&path).await;

    assert_eq!(observed.observation, FileObservation::Absent);
    assert!(observed.object.file.is_none());
    assert!(
        observed.object.parent.is_none(),
        "a parent that does not exist has no identity to record"
    );
}

#[tokio::test]
async fn multiply_linked_file_is_refused_rather_than_observed() {
    let (_guard, root) = temp_root();
    let target = root.join("target.txt");
    std::fs::write(&target, b"linked").unwrap();
    std::fs::hard_link(&target, root.join("alias.txt")).expect("create hard link");

    let error = RealFs
        .observe_file(&target)
        .await
        .expect_err("a multiply-linked file must be refused");

    assert!(
        format!("{error}").contains("singly-linked"),
        "refusal must name the reason; got: {error}"
    );
}

#[tokio::test]
async fn directory_is_refused_rather_than_observed() {
    let (_guard, root) = temp_root();
    let directory = root.join("a-directory");
    std::fs::create_dir(&directory).unwrap();

    RealFs
        .observe_file(&directory)
        .await
        .expect_err("a directory must never be observed as a CAS target");
}

#[tokio::test]
async fn file_reparse_point_is_refused_and_its_target_is_not_read() {
    let (_guard, root) = temp_root();
    let target = root.join("outside.txt");
    std::fs::write(&target, b"secret-target-bytes").unwrap();
    let link = root.join("link.txt");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).expect("create file symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &link)
        .expect("native Windows proof requires file-symlink creation authority");

    let observed = RealFs.observe_file(&link).await;

    let error = match observed {
        Err(error) => error,
        Ok(observed) => panic!(
            "a reparse point must be refused, not followed; observed {:?}",
            observed.observation
        ),
    };
    assert_ne!(
        format!("{error}"),
        String::new(),
        "the refusal must carry a reason"
    );
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"secret-target-bytes",
        "the refusal must not have disturbed the link target"
    );
}

#[tokio::test]
async fn unmoved_target_reconciles_to_not_started() {
    let (_guard, root) = temp_root();
    let path = root.join("receipt.txt");
    std::fs::write(&path, b"preimage").unwrap();
    let prepared = observe(&path).await;
    let receipt = receipt_for(&path, &prepared, b"postimage");
    receipt.validate().expect("receipt must validate");

    let reconciliation = receipt.reconcile(&RealFs).await.unwrap();

    match reconciliation {
        FilesystemReconciliation::NotStarted { current } => {
            assert_eq!(present(current).len, b"preimage".len() as u64);
        }
        other => panic!("nothing moved, so the effect never started; got {other:?}"),
    }
}

#[tokio::test]
async fn applied_target_reconciles_to_already_applied() {
    let (_guard, root) = temp_root();
    let path = root.join("receipt.txt");
    std::fs::write(&path, b"preimage").unwrap();
    let prepared = observe(&path).await;
    let receipt = receipt_for(&path, &prepared, b"postimage");

    std::fs::write(&path, b"postimage").unwrap();

    match receipt.reconcile(&RealFs).await.unwrap() {
        FilesystemReconciliation::AlreadyApplied { current } => {
            assert_eq!(
                present(current),
                FileContentIdentity::from_bytes(b"postimage")
            );
        }
        other => panic!("the intended bytes are present; got {other:?}"),
    }
}

#[tokio::test]
async fn substituted_object_with_matching_preimage_reconciles_to_conflict() {
    let (_guard, root) = temp_root();
    let path = root.join("receipt.txt");
    std::fs::write(&path, b"preimage").unwrap();
    let prepared = observe(&path).await;
    let receipt = receipt_for(&path, &prepared, b"postimage");

    // The bytes still satisfy the recorded precondition exactly. ONLY the
    // object identity moved. Byte comparison alone would answer NotStarted
    // and hand a crashed turn permission to replay a write onto a file it
    // never prepared.
    substitute_object(&path, b"preimage");

    match receipt.reconcile(&RealFs).await.unwrap() {
        FilesystemReconciliation::Conflict { current } => {
            assert_eq!(
                present(current),
                FileContentIdentity::from_bytes(b"preimage")
            );
        }
        other => panic!(
            "the preimage bytes match but the object does not — this must be \
             a Conflict, never a replayable NotStarted; got {other:?}"
        ),
    }
}

#[tokio::test]
async fn substituted_parent_with_matching_preimage_reconciles_to_conflict() {
    let (_guard, root) = temp_root();
    let parent = root.join("parent");
    std::fs::create_dir(&parent).unwrap();
    let path = parent.join("receipt.txt");
    std::fs::write(&path, b"preimage").unwrap();
    let prepared = observe(&path).await;
    let receipt = receipt_for(&path, &prepared, b"postimage");

    let staging = root.join("staging");
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(staging.join("receipt.txt"), b"preimage").unwrap();
    std::fs::rename(&parent, root.join("displaced")).unwrap();
    std::fs::rename(&staging, &parent).unwrap();

    match receipt.reconcile(&RealFs).await.unwrap() {
        FilesystemReconciliation::Conflict { .. } => {}
        other => panic!(
            "the pathname and the bytes are unchanged but the parent \
             directory object is not the one that was prepared; got {other:?}"
        ),
    }
}

#[tokio::test]
async fn absent_prepared_target_reconciles_to_not_started_then_conflict() {
    let (_guard, root) = temp_root();
    let path = root.join("absent-receipt.txt");
    let prepared = observe(&path).await;
    assert_eq!(prepared.observation, FileObservation::Absent);
    let receipt = receipt_for(&path, &prepared, b"postimage");
    receipt
        .validate()
        .expect("absent-precondition receipt must validate");

    match receipt.reconcile(&RealFs).await.unwrap() {
        FilesystemReconciliation::NotStarted {
            current: FileObservation::Absent,
        } => {}
        other => panic!("still absent, so the effect never started; got {other:?}"),
    }

    // Some other writer created the file with bytes that are neither the
    // recorded absence nor the intended postimage.
    std::fs::write(&path, b"someone-else").unwrap();
    match receipt.reconcile(&RealFs).await.unwrap() {
        FilesystemReconciliation::Conflict { .. } => {}
        other => panic!("an unexpected object now occupies the path; got {other:?}"),
    }
}
