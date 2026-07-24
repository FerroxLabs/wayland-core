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
// `FILE_ID_BOTH_DIR_INFORMATION` lives in the Wdk namespace in windows-sys 0.59
// (feature `Wdk_Storage_FileSystem`), matching production `directory_authority_
// windows.rs`; only `FILE_RENAME_INFO` is a Win32 Storage::FileSystem type.
use windows_sys::Wdk::Storage::FileSystem::FILE_ID_BOTH_DIR_INFORMATION;
use windows_sys::Win32::Storage::FileSystem::FILE_RENAME_INFO;

fn inject_create_failure(stage: Option<CreateValidationStage>) {
    CREATE_VALIDATION_FAILURE.with(|failure| failure.set(stage));
}

fn write_directory_entry(buffer: &mut [u8], start: usize, next: u32, name: &str) -> usize {
    let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
    let name_length_offset = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileNameLength);
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

#[test]
fn directory_information_larger_than_supplied_buffer_fails_closed() {
    assert!(checked_information_length(4097, 4096).is_err());
    assert_eq!(checked_information_length(4096, 4096).unwrap(), 4096);
}

#[test]
fn unaligned_directory_entry_is_copied_before_utf16_decoding() {
    let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
    let mut storage = vec![0_u8; 1 + header + 16];
    let returned = write_directory_entry(&mut storage, 1, 0, "proof");
    let mut names = Vec::new();

    parse_directory_entries(unsafe { storage.as_ptr().add(1) }, returned, &mut names).unwrap();

    assert_eq!(names, ["proof"]);
}

#[test]
fn misaligned_next_directory_entry_offset_fails_closed() {
    let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
    let mut storage = vec![0_u8; header * 2 + 32];
    let first_length = write_directory_entry(&mut storage, 0, 0, "first");
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
    write_directory_entry(&mut storage, 0, next as u32, "name-that-crosses-boundary");

    let mut names = Vec::new();
    assert!(parse_directory_entries(storage.as_ptr(), storage.len(), &mut names).is_err());
    assert!(
        names.is_empty(),
        "invalid entry must not produce partial names"
    );
}

#[test]
fn rename_buffer_includes_full_structure_and_rejects_overflow() {
    let name_bytes = 12;
    assert_eq!(
        rename_buffer_len(name_bytes).unwrap(),
        std::mem::size_of::<windows_sys::Win32::Storage::FileSystem::FILE_RENAME_INFO>()
            + name_bytes
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
