use super::*;

#[test]
fn duplicated_directory_handles_are_tracked_until_drop() {
    let fixture = tempfile::tempdir().unwrap();
    let authority = DirectoryAuthority::open(fixture.path()).unwrap();

    assert!(!authority.has_outstanding_handle_loans());
    let loan = authority.try_clone_handle().unwrap();
    assert!(authority.has_outstanding_handle_loans());
    drop(loan);
    assert!(!authority.has_outstanding_handle_loans());
}

#[test]
fn rejects_real_directory_swap_during_acquisition() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("root");
    let original = fixture.path().join("original");
    std::fs::create_dir(&root).unwrap();

    let error = DirectoryAuthority::open_inner(&root, || {
        std::fs::rename(&root, &original).unwrap();
        std::fs::create_dir(&root).unwrap();
    })
    .expect_err("same-path substitution was accepted during acquisition");

    assert!(error.to_string().contains("identity changed"), "{error}");
}

#[test]
fn rejects_real_directory_swap_during_validation() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("root");
    let original = fixture.path().join("original");
    std::fs::create_dir(&root).unwrap();
    let authority = DirectoryAuthority::open(&root).unwrap();

    let error = authority
        .validate_path_inner(&root, || {
            std::fs::rename(&root, &original).unwrap();
            std::fs::create_dir(&root).unwrap();
        })
        .expect_err("same-path substitution was accepted during validation");

    assert!(error.to_string().contains("identity changed"), "{error}");
}

#[test]
fn retained_regular_file_read_is_bounded_and_identity_checked() {
    let fixture = tempfile::tempdir().unwrap();
    let receipt = fixture.path().join("receipt");
    std::fs::write(&receipt, "123456789").unwrap();
    let authority = RegularFileAuthority::open(&receipt).unwrap();

    let error = authority
        .read_bounded_to_string(8)
        .expect_err("oversized authority file was read");
    assert!(error.to_string().contains("exceeds 8 bytes"), "{error}");

    let original = fixture.path().join("receipt-original");
    std::fs::rename(&receipt, &original).unwrap();
    std::fs::write(&receipt, "1").unwrap();
    let error = authority
        .validate_path(&receipt)
        .expect_err("same-path file replacement retained authority");
    assert!(error.to_string().contains("identity changed"), "{error}");
}

#[test]
fn retained_regular_file_detects_same_inode_rewrite() {
    use std::os::unix::fs::MetadataExt;

    let fixture = tempfile::tempdir().unwrap();
    let receipt = fixture.path().join("receipt");
    std::fs::write(&receipt, "8192").unwrap();
    let before = std::fs::metadata(&receipt).unwrap().ino();
    let authority = RegularFileAuthority::open(&receipt).unwrap();

    std::fs::write(&receipt, "1").unwrap();
    let after = std::fs::metadata(&receipt).unwrap().ino();
    assert_eq!(
        before, after,
        "fixture replaced the inode instead of rewriting it"
    );
    assert_eq!(authority.read_bounded_to_string(64).unwrap(), "1");
    assert_ne!(
        authority.read_bounded_to_string(64).unwrap(),
        "8192",
        "retained file authority hid a same-inode rewrite"
    );
}

#[test]
fn relative_creation_stays_bound_to_renamed_parent_object() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("root");
    let retained = fixture.path().join("retained");
    std::fs::create_dir(&root).unwrap();
    let authority = DirectoryAuthority::open(&root).unwrap();
    std::fs::rename(&root, &retained).unwrap();
    std::fs::create_dir(&root).unwrap();

    let child = authority.create_child_directory("child").unwrap();
    child.create_child_file("receipt", b"retained\n").unwrap();

    assert_eq!(
        std::fs::read_to_string(retained.join("child/receipt")).unwrap(),
        "retained\n"
    );
    assert!(!root.join("child").exists());
}

/// Native macOS proof that a retained directory authority keeps rename,
/// delete, enumeration, and command-cwd bound to the OS object it opened, even
/// after that object's pathname is swapped out and a decoy is planted at the
/// original path. macOS-only so it exercises the real Darwin filesystem object
/// identity rather than a foreign-platform substitute.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_retained_parent_rename_delete_enumeration_and_cwd_stay_handle_relative() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("root");
    let retained = fixture.path().join("retained");
    std::fs::create_dir(&root).unwrap();
    let authority = DirectoryAuthority::open(&root).unwrap();

    // Swap the pathname out from under the retained object and plant a decoy.
    std::fs::rename(&root, &retained).unwrap();
    std::fs::create_dir(&root).unwrap();

    // Create + enumerate stay bound to the retained object, never the decoy.
    let mover = authority.create_child_directory("mover").unwrap();
    authority.create_child_directory("victim").unwrap();
    authority
        .create_child_file("receipt", b"retained\n")
        .unwrap();
    assert_eq!(
        authority.child_names().unwrap(),
        ["mover", "receipt", "victim"]
    );
    assert!(
        std::fs::read_dir(&root).unwrap().next().is_none(),
        "operations leaked onto the decoy planted at the original path"
    );

    // Rename stays handle-relative: move `mover` to `landed` under the retained
    // parent. Delete stays handle-relative: remove the empty `victim` child.
    mover.rename_into(&authority, "landed", false).unwrap();
    authority.remove_empty_child_directory("victim").unwrap();
    assert_eq!(authority.child_names().unwrap(), ["landed", "receipt"]);

    // Command cwd binds to the retained object and resolves to its real path.
    let mut command = tokio::process::Command::new("pwd");
    authority.bind_command_cwd(&mut command).unwrap();
    let output = command.output().await.unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        retained.to_string_lossy()
    );

    // The retained object holds every result; the decoy path stays empty.
    assert!(retained.join("landed").is_dir());
    assert!(retained.join("receipt").is_file());
    assert!(!retained.join("victim").exists());
    assert!(!root.join("landed").exists());
}
