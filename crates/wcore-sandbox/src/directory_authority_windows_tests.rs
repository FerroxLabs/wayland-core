//! Windows retained-directory authority tests.
//!
//! Mounted as `directory_authority::tests` (not `directory_authority::windows::
//! tests`) so the required proof identities are exactly
//! `directory_authority::tests::windows_*`. Because this module is a sibling of
//! `windows` rather than its child, it reaches the retained-handle internals
//! through `super::windows::*` (bumped to `pub(super)`) and imports the raw
//! `windows-sys` layout types it needs directly.

use super::windows::*;
use super::*;
use crate::error::SandboxError;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
// Both raw layout types these tests measure — `FILE_ID_BOTH_DIR_INFORMATION`
// and `FILE_RENAME_INFORMATION` — live in the Wdk namespace in windows-sys 0.59
// (feature `Wdk_Storage_FileSystem`), matching production `directory_authority_
// windows.rs`. The rename type is the NT one because the handle-relative rename
// is an `NtSetInformationFile` call: the Win32 `FILE_RENAME_INFO` wrapper class
// rejects a HANDLE in `RootDirectory` (os error 87) and is no longer used
// anywhere in this crate.
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_ID_BOTH_DIR_INFORMATION, FILE_RENAME_INFORMATION,
};
// The attribute constants the fabricated parser fixtures write. Production
// reaches these through its own module-level import; a glob over `super::
// windows::*` does not re-export another module's private `use`, so they are
// imported here directly.
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY,
};

fn inject_create_failure(stage: Option<CreateValidationStage>) {
    CREATE_VALIDATION_FAILURE.with(|failure| failure.set(stage));
}

/// Fabricate one `FILE_ID_BOTH_DIR_INFORMATION` record.
///
/// `attributes` is written EXPLICITLY rather than left to the buffer's zero
/// fill, so the parser's attribute read is genuinely exercised by these
/// hand-written buffers and a regression that read the wrong offset would show
/// up here rather than only in a live enumeration.
fn write_directory_entry(
    buffer: &mut [u8],
    start: usize,
    next: u32,
    name: &str,
    attributes: u32,
) -> usize {
    let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
    let name_length_offset = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileNameLength);
    let attributes_offset = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileAttributes);
    let wide = name.encode_utf16().collect::<Vec<_>>();
    let name_bytes = wide.len() * std::mem::size_of::<u16>();
    let length = header + name_bytes;
    assert!(buffer.len() >= start + length);
    unsafe {
        buffer
            .as_mut_ptr()
            .add(start)
            .cast::<u32>()
            .write_unaligned(next);
        buffer
            .as_mut_ptr()
            .add(start + attributes_offset)
            .cast::<u32>()
            .write_unaligned(attributes);
        buffer
            .as_mut_ptr()
            .add(start + name_length_offset)
            .cast::<u32>()
            .write_unaligned(name_bytes as u32);
        for (index, code_unit) in wide.into_iter().enumerate() {
            buffer
                .as_mut_ptr()
                .add(start + header + index * std::mem::size_of::<u16>())
                .cast::<u16>()
                .write_unaligned(code_unit);
        }
    }
    length
}

fn set_directory_case_sensitive(handle: &File) {
    // `as_raw_handle` is an extension-trait method on `std::fs::File`; the trait
    // must be in scope at the call site below. Kept local to this fn (rather
    // than at module scope) because this is its only use.
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileCaseSensitiveInfo, SetFileInformationByHandle,
    };

    let info = DirectoryCaseSensitiveInfo {
        flags: FILE_CS_FLAG_CASE_SENSITIVE_DIR,
    };
    assert_ne!(
        unsafe {
            SetFileInformationByHandle(
                handle.as_raw_handle().cast(),
                FileCaseSensitiveInfo,
                std::ptr::addr_of!(info).cast(),
                std::mem::size_of::<DirectoryCaseSensitiveInfo>() as u32,
            )
        },
        0,
        "native Windows proof requires case-sensitive directory support: {}",
        std::io::Error::last_os_error()
    );
}

#[test]
fn retained_parent_routes_children_after_path_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("original");
    let moved = temp.path().join("moved");
    std::fs::create_dir(&original).unwrap();
    let authority = DirectoryAuthority::open(&original).unwrap();

    std::fs::rename(&original, &moved).unwrap();
    std::fs::create_dir(&original).unwrap();
    authority.create_child_file("proof", b"retained").unwrap();

    assert_eq!(std::fs::read(moved.join("proof")).unwrap(), b"retained");
    assert!(!original.join("proof").exists());
}

#[test]
fn root_mutation_authority_supports_directory_durability() {
    let temp = tempfile::tempdir().unwrap();
    let authority = DirectoryAuthority::open(temp.path()).unwrap();

    authority.sync().unwrap();
    authority.create_child_file("durable", b"bytes").unwrap();
    authority.sync().unwrap();

    assert_eq!(
        std::fs::read(temp.path().join("durable")).unwrap(),
        b"bytes"
    );
}

#[test]
fn cleanup_refuses_outstanding_handle_loan_then_retries_same_authority() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let authority = DirectoryAuthority::open(&root).unwrap();
    let loan = authority.try_clone_handle().unwrap();

    let (error, authority) = *authority.remove_open_dir_all().unwrap_err();
    assert!(error.to_string().contains("outstanding authority handles"));
    drop(loan);
    authority.remove_open_dir_all().unwrap();
    assert!(!root.exists());
}

#[test]
fn concurrent_atomic_write_exposes_only_whole_old_or_new_payloads() {
    let temp = tempfile::tempdir().unwrap();
    let authority = DirectoryAuthority::open(temp.path()).unwrap();
    let old = vec![b'o'; 1024 * 1024];
    let payload = vec![b'n'; 8 * 1024 * 1024];
    authority.atomic_write_child("state", &old).unwrap();

    let path = temp.path().join("state");
    let done = Arc::new(AtomicBool::new(false));
    let start = Arc::new(Barrier::new(2));
    let (replacement_started_tx, replacement_started_rx) = std::sync::mpsc::channel();
    let (observation_complete_tx, observation_complete_rx) = std::sync::mpsc::channel();
    let in_flight_observations = Arc::new(AtomicUsize::new(0));
    let observer_done = Arc::clone(&done);
    let observer_start = Arc::clone(&start);
    let observer_in_flight_observations = Arc::clone(&in_flight_observations);
    let observed_old = old.clone();
    let observed_new = payload.clone();
    let observer = std::thread::spawn(move || {
        let bytes = std::fs::read(&path).expect("initial atomic target must be readable");
        assert_eq!(bytes, observed_old);
        observer_start.wait();

        replacement_started_rx
            .recv()
            .expect("replacement must reach its atomic publication boundary");
        let bytes = std::fs::read(&path).expect("in-flight atomic target must remain readable");
        assert!(
            bytes == observed_old || bytes == observed_new,
            "observer saw a partial or foreign in-flight payload of {} bytes",
            bytes.len()
        );
        observer_in_flight_observations.fetch_add(1, Ordering::Release);
        observation_complete_tx
            .send(())
            .expect("replacement must wait for the in-flight observation");

        while !observer_done.load(Ordering::Acquire) {
            let bytes = std::fs::read(&path).expect("atomic target must remain readable");
            assert!(
                bytes == observed_old || bytes == observed_new,
                "observer saw a partial or foreign payload of {} bytes",
                bytes.len()
            );
        }
        let bytes = std::fs::read(&path).expect("final atomic target must be readable");
        assert_eq!(bytes, observed_new);
    });

    // Do not begin replacement until the observer has opened and verified the
    // old generation. This makes the visibility proof genuinely concurrent.
    start.wait();
    set_before_atomic_file_rename_hook(Some(Box::new(move || {
        replacement_started_tx
            .send(())
            .expect("observer must receive the publication-boundary signal");
        observation_complete_rx
            .recv()
            .expect("observer must complete an in-flight read before publication");
    })));
    authority.atomic_write_child("state", &payload).unwrap();
    done.store(true, Ordering::Release);
    observer.join().unwrap();

    assert_eq!(
        in_flight_observations.load(Ordering::Acquire),
        1,
        "the observer must read at least once while replacement is in flight"
    );

    let observed = std::fs::read(temp.path().join("state")).unwrap();
    assert_eq!(observed, payload);
}

#[test]
fn case_sensitive_parent_preserves_distinct_foo_and_uppercase_foo() {
    let temp = tempfile::tempdir().unwrap();
    let authority = DirectoryAuthority::open(temp.path()).unwrap();
    set_directory_case_sensitive(&authority.handle);

    authority.create_child_file("foo", b"lower").unwrap();
    authority.create_child_file("Foo", b"upper").unwrap();

    assert_eq!(
        authority.read_child_bounded("foo", 32).unwrap().unwrap(),
        b"lower"
    );
    assert_eq!(
        authority.read_child_bounded("Foo", 32).unwrap().unwrap(),
        b"upper"
    );
    assert_eq!(authority.child_names().unwrap(), ["Foo", "foo"]);
}

#[test]
fn windows_handle_relative_rename_stays_bound_to_target_parent() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source");
    let target_path = temp.path().join("target");
    let moved_target = temp.path().join("moved-target");
    std::fs::create_dir(&source_path).unwrap();
    std::fs::create_dir(&target_path).unwrap();
    let source = DirectoryAuthority::open(&source_path).unwrap();
    let target = DirectoryAuthority::open(&target_path).unwrap();

    std::fs::rename(&target_path, &moved_target).unwrap();
    std::fs::create_dir(&target_path).unwrap();
    source.rename_into(&target, "landed", false).unwrap();

    assert!(moved_target.join("landed").is_dir());
    assert!(!target_path.join("landed").exists());
}

/// The same held-handle guarantee as the sibling directory proof above, but for
/// the FILE publish path — which is the one `atomic_write_child` (and therefore
/// the production heartbeat status mirror, the swarm rename API and the archive
/// rollback) actually travels. The sibling test only covers directory sources,
/// so without this the production path had no anti-swap proof at all.
///
/// What it guards: the destination parent is resolved by the RETAINED HANDLE and
/// never by pathname. The target directory's pathname is renamed away and a
/// decoy is recreated at the original path BEFORE the publish, so a rename that
/// re-resolved the destination by name would land in the decoy. This test fails
/// the moment anyone reintroduces the `RootDirectory = NULL` + full-pathname
/// form that a probe showed "working" (see `rename_handle_into`).
///
/// Asserts on the invariant — WHERE the object landed — never on an OS error
/// code.
#[test]
fn windows_handle_relative_file_publish_stays_bound_to_target_parent() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source");
    let target_path = temp.path().join("target");
    let moved_target = temp.path().join("moved-target");
    std::fs::create_dir(&source_path).unwrap();
    std::fs::create_dir(&target_path).unwrap();
    let source_parent = DirectoryAuthority::open(&source_path).unwrap();
    let target = DirectoryAuthority::open(&target_path).unwrap();
    let payload = b"published-through-the-held-handle";
    let source = source_parent.create_child_file("pending", payload).unwrap();

    // Swap the destination's PATHNAME out from under the held handle and put a
    // decoy back at the original path.
    std::fs::rename(&target_path, &moved_target).unwrap();
    std::fs::create_dir(&target_path).unwrap();

    source.rename_into(&target, "landed", false).unwrap();

    assert_eq!(std::fs::read(moved_target.join("landed")).unwrap(), payload);
    assert!(!target_path.join("landed").exists());
    assert!(!source_path.join("pending").exists());
}

#[test]
fn windows_handle_relative_delete_rejects_same_path_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let authority = DirectoryAuthority::open(temp.path()).unwrap();
    let child = authority.create_child_directory("child").unwrap();
    child.create_child_file("kept", b"untouched").unwrap();

    let error = authority.remove_empty_child_directory("child").unwrap_err();
    assert!(error.to_string().contains("non-empty"));
    assert_eq!(
        std::fs::read(temp.path().join("child").join("kept")).unwrap(),
        b"untouched"
    );

    child.remove_descendants().unwrap();
    drop(child);
    authority.remove_empty_child_directory("child").unwrap();
    assert!(!temp.path().join("child").exists());
}

#[test]
fn file_symlink_is_rejected_without_following_it() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    let root = temp.path().join("root");
    std::fs::write(&outside, b"outside").unwrap();
    std::fs::create_dir(&root).unwrap();
    std::os::windows::fs::symlink_file(&outside, root.join("linked"))
        .expect("native Windows proof requires file-symlink creation authority");
    let authority = DirectoryAuthority::open(&root).unwrap();

    assert!(authority.open_child_file("linked").is_err());
    assert!(authority.remove_descendants().is_err());
    assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
}

#[test]
fn windows_handle_relative_enumeration_rejects_reparse_children() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside-dir");
    let root = temp.path().join("root");
    let linked = root.join("linked");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("kept"), b"outside").unwrap();
    std::fs::create_dir(&root).unwrap();
    std::os::windows::fs::symlink_dir(&outside, &linked)
        .expect("native Windows proof requires directory-reparse creation authority");
    let authority = DirectoryAuthority::open(&root).unwrap();

    assert!(authority.open_child_directory("linked").is_err());
    assert!(authority.remove_descendants().is_err());
    assert_eq!(std::fs::read(outside.join("kept")).unwrap(), b"outside");
}

#[test]
fn windows_namespace_and_ads_names_fail_before_operations() {
    let ambiguous = [
        "file:stream",
        r"\\?\device",
        r"\\.\device",
        r"\??\device",
        "trailing.",
        "trailing ",
        "CON",
        "COM1.txt",
        "name<bad",
        "name>bad",
        "name\"bad",
        "name|bad",
        "name?bad",
        "name*bad",
        "name\u{1}bad",
        "name\u{1f}bad",
        "COM¹",
        "COM².log",
        "COM³",
        "LPT¹",
        "LPT².log",
        "LPT³",
    ];
    for name in ambiguous {
        assert!(
            validate_windows_child_name(name).is_err(),
            "accepted {name:?}"
        );
    }
    for codepoint in 0..=0x1f {
        let control = char::from_u32(codepoint).unwrap();
        let name = format!("prefix{control}suffix");
        assert!(
            validate_windows_child_name(&name).is_err(),
            "accepted Win32 control U+{codepoint:04X}"
        );
    }

    let temp = tempfile::tempdir().unwrap();
    let authority = DirectoryAuthority::open(temp.path()).unwrap();
    assert!(authority.create_child_file("file:stream", b"bad").is_err());
    assert!(authority.create_child_directory("dir:stream").is_err());
    assert!(authority.open_child_file("file:stream").is_err());
    let source = authority.create_child_file("source", b"safe").unwrap();
    assert!(
        source
            .rename_into(&authority, "target:stream", false)
            .is_err()
    );
    assert!(!temp.path().join("file").exists());
}

#[test]
fn read_only_child_open_does_not_require_or_receive_delete_authority() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("readable"), b"value").unwrap();
    let parent = DirectoryAuthority::open(temp.path()).unwrap();
    let file = parent.open_child_file("readable").unwrap();

    let error = mark_open_object_for_delete(&file.handle, file.display_path(), "file").unwrap_err();
    assert!(
        matches!(&error, SandboxError::ExecFailed(message) if message.contains("delete retained Windows file")),
        "unexpected error: {error}"
    );
    assert_eq!(file.read_bounded(64).unwrap(), b"value");
    assert!(temp.path().join("readable").exists());
}

#[test]
fn created_file_rolls_back_every_post_create_validation_failure() {
    let temp = tempfile::tempdir().unwrap();
    let parent = DirectoryAuthority::open(temp.path()).unwrap();
    for (index, stage) in [
        CreateValidationStage::Metadata,
        CreateValidationStage::Type,
        CreateValidationStage::Identity,
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("file-{index}");
        inject_create_failure(Some(stage));
        assert!(parent.create_child_file(&name, b"value").is_err());
        inject_create_failure(None);
        assert!(!temp.path().join(&name).exists());
        parent.create_child_file(&name, b"retry").unwrap();
        assert_eq!(std::fs::read(temp.path().join(&name)).unwrap(), b"retry");
    }
}

#[test]
fn created_directory_rolls_back_every_post_create_validation_failure() {
    let temp = tempfile::tempdir().unwrap();
    let parent = DirectoryAuthority::open(temp.path()).unwrap();
    for (index, stage) in [
        CreateValidationStage::Metadata,
        CreateValidationStage::Type,
        CreateValidationStage::Identity,
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("dir-{index}");
        inject_create_failure(Some(stage));
        assert!(parent.create_child_directory(&name).is_err());
        inject_create_failure(None);
        assert!(!temp.path().join(&name).exists());
        parent.create_child_directory(&name).unwrap();
        assert!(temp.path().join(&name).is_dir());
    }
}

/// A READ-ONLY FILE child must not block destructive removal.
///
/// This is the exact case the original diagnosis missed. Git writes loose
/// objects and packfiles at mode 444, so read-only children are the NORMAL case
/// in every checkout — not an edge case.
///
/// The bit whose loss this catches: `FILE_GENERIC_WRITE` appearing in the
/// `(File, Mutate)` access arm. A mode-444 child is refused AT THE OPEN with os
/// error 5 when write is requested, so widening that arm to match the directory
/// arm makes every read-only child un-removable. Windows-only because unix
/// removal goes through cap-std relative to the descriptor and has no per-handle
/// access mask, so this arm has no unix analogue.
///
/// Asserts on the invariant (removal succeeds, the tree is gone), never on an OS
/// error code.
#[test]
fn windows_destructive_removal_succeeds_through_a_read_only_file_child() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let child = root.join("packfile");
    std::fs::write(&child, b"read-only like git writes them").unwrap();
    let mut permissions = std::fs::metadata(&child).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&child, permissions).unwrap();
    assert!(
        std::fs::metadata(&child).unwrap().permissions().readonly(),
        "the fixture must actually be read-only for this proof to mean anything"
    );

    let authority = DirectoryAuthority::open(&root).unwrap();
    authority.remove_descendants().unwrap();

    assert!(!child.exists());
    assert!(authority.child_names().unwrap().is_empty());
}

/// A DIRECTORY child must not block destructive removal either.
///
/// The bit whose loss this catches: `FILE_GENERIC_WRITE` disappearing from the
/// `(Directory, Mutate)` access arm. The cleanup recursion terminates in
/// `FlushFileBuffers` on the directory handle, which demands write access, so
/// narrowing that arm to match the file arm makes every directory child
/// un-removable. Together with the sibling read-only-file proof above, this
/// pins BOTH ends of the impossibility that forced the kind to become precise.
/// Windows-only for the same reason as its sibling.
#[test]
fn windows_destructive_removal_succeeds_through_a_directory_child() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("empty")).unwrap();
    std::fs::create_dir_all(root.join("nested").join("inner")).unwrap();
    std::fs::write(root.join("nested").join("inner").join("leaf"), b"leaf").unwrap();

    let authority = DirectoryAuthority::open(&root).unwrap();
    authority.remove_descendants().unwrap();

    assert!(!root.join("empty").exists());
    assert!(!root.join("nested").exists());
    assert!(authority.child_names().unwrap().is_empty());
}

#[test]
fn directory_information_larger_than_supplied_buffer_fails_closed() {
    assert!(checked_information_length(4097, 4096).is_err());
    assert_eq!(checked_information_length(4096, 4096).unwrap(), 4096);
}

#[test]
fn unaligned_directory_entry_is_copied_before_utf16_decoding() {
    let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
    let mut storage = vec![0_u8; 1 + header + 16];
    let returned = write_directory_entry(
        &mut storage,
        1,
        0,
        "proof",
        FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_READONLY,
    );
    let mut entries = Vec::new();

    parse_directory_entries(unsafe { storage.as_ptr().add(1) }, returned, &mut entries).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "proof");
    // The attribute word is read from an UNALIGNED record too, and it is the
    // field the cleanup walk derives each child's open kind from.
    assert_eq!(
        entries[0].attributes,
        FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_READONLY
    );
}

#[test]
fn misaligned_next_directory_entry_offset_fails_closed() {
    let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
    let mut storage = vec![0_u8; header * 2 + 32];
    let first_length = write_directory_entry(&mut storage, 0, 0, "first", FILE_ATTRIBUTE_NORMAL);
    let bad_next = (first_length.next_multiple_of(8) + 4) as u32;
    assert_eq!(bad_next % 8, 4);
    unsafe { storage.as_mut_ptr().cast::<u32>().write_unaligned(bad_next) };
    let returned = bad_next as usize + header;

    assert!(parse_directory_entries(storage.as_ptr(), returned, &mut Vec::new()).is_err());
}

#[test]
fn directory_name_cannot_cross_its_current_entry_boundary() {
    let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
    let next = (header + 8).next_multiple_of(8);
    let mut storage = vec![0_u8; next + header + 32];
    write_directory_entry(
        &mut storage,
        0,
        next as u32,
        "name-that-crosses-boundary",
        FILE_ATTRIBUTE_DIRECTORY,
    );

    let mut entries = Vec::new();
    assert!(parse_directory_entries(storage.as_ptr(), storage.len(), &mut entries).is_err());
    assert!(
        entries.is_empty(),
        "invalid entry must not produce partial names"
    );
}

#[test]
fn rename_buffer_includes_full_structure_and_rejects_overflow() {
    let name_bytes = 12;
    assert_eq!(
        rename_buffer_len(name_bytes).unwrap(),
        std::mem::size_of::<FILE_RENAME_INFORMATION>() + name_bytes
    );
    assert!(rename_buffer_len(usize::MAX).is_err());
}

#[test]
fn windows_command_cwd_stays_bound_to_renamed_directory_object() {
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("original");
    let moved = temp.path().join("moved");
    std::fs::create_dir(&original).unwrap();
    let authority = DirectoryAuthority::open(&original).unwrap();
    let mut command = tokio::process::Command::new("cmd.exe");

    let error = bind_command_cwd(&authority, &mut command).unwrap_err();
    assert!(error.to_string().contains("process-lifetime name lease"));

    std::fs::rename(&original, &moved).unwrap();
    std::fs::create_dir(&original).unwrap();
    assert!(moved.is_dir());
    assert!(original.is_dir());
}
