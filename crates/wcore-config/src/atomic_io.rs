//! Atomic file write helpers.
//!
//! `std::fs::write()` lacks two guarantees we need for durable state:
//!
//! 1. **Atomicity** — a crash mid-write leaves a truncated file. The
//!    next read sees garbage, and code that assumed "file exists
//!    therefore content is valid" panics or silently corrupts.
//!
//! 2. **Durability** — written bytes can sit in the OS page cache for
//!    seconds before reaching the disk. A power loss after `write()`
//!    returned `Ok` can still lose the write.
//!
//! [`atomic_write`] gives both: write to a sibling tempfile, fsync the
//! data, then rename into place. The rename is atomic on POSIX and
//! NTFS; the prior fsync ensures the bytes are on platter before the
//! rename commits, so a crash anywhere in the sequence leaves either
//! the old contents or the new contents — never a half-written file.
//!
//! Used for: auth credentials, memory store entries, memory index
//! files — anywhere a partial write would leave the system in a
//! corrupt state.

use std::io::Write;
use std::path::Path;

/// Write `contents` to `path` atomically and durably.
///
/// 1. Creates a tempfile in the same directory as `path` (so the
///    rename is same-filesystem and therefore atomic).
/// 2. Writes `contents`, then `sync_all()`s the tempfile so its
///    bytes are on platter before the rename.
/// 3. Renames the tempfile over `path`. POSIX guarantees `rename(2)`
///    is atomic; NTFS provides the same guarantee for files on the
///    same volume.
///
/// On any error before the rename, the original `path` is untouched.
///
/// Does NOT fsync the parent directory after the rename. On most
/// modern filesystems (ext4 with `data=ordered`, xfs, apfs, ntfs)
/// the rename is journalled so the new dentry survives a crash; the
/// extra `fsync(parent_dir)` would block the call by ~1ms for a
/// guarantee the journal already provides.
pub fn atomic_write<P: AsRef<Path>>(path: P, contents: &[u8]) -> std::io::Result<()> {
    let path = path.as_ref();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;

    // `persist()` does the atomic rename. `PersistError` wraps both
    // the underlying io::Error and the un-renamed temp file; we only
    // care about the io::Error for callers using `?`.
    tmp.persist(path).map(|_| ()).map_err(|e| e.error)
}

/// Build a path under `base` whose total length exceeds Windows' `MAX_PATH`
/// (260), using nesting rather than one long component so that every individual
/// component stays inside the 255-byte per-component limit every filesystem
/// enforces.
///
/// Shared by the tests below and by `wcore-cli`'s backup long-path proof, so
/// both measure the same shape rather than two hand-rolled approximations that
/// could drift apart and disagree about what "deep" means.
#[doc(hidden)]
pub fn deep_path_over_max_path(base: &Path, leaf: &str) -> std::path::PathBuf {
    const WINDOWS_MAX_PATH: usize = 260;
    let mut p = base.to_path_buf();
    // 8 components of 40 chars each = 328 characters of nesting on top of
    // `base`, which is comfortably past 260 even for a short base.
    while p.as_os_str().len() + leaf.len() + 1 <= WINDOWS_MAX_PATH + 40 {
        p.push("d234567890123456789012345678901234567890");
    }
    p.push(leaf);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F26-03-D. The diagnostic that discriminates between the three candidate
    /// causes of the Windows `os error 3` seen when restoring a deep tree:
    /// whether `create_dir_all` fails, whether plain `fs::write` fails, or
    /// whether only `atomic_write` fails. Only the third would point at the
    /// tempfile round trip rather than at `std`'s own long-path handling.
    ///
    /// It prints every leg so a single Windows run yields the whole picture,
    /// and it ASSERTS the property that matters: `atomic_write` must work at a
    /// path past `MAX_PATH`, because that is the call the restore write loop
    /// makes for every payload.
    ///
    /// On unix this is already green against the untouched tree — `PATH_MAX` is
    /// 4096, so a 300-character path was never the constraint there. It is
    /// recorded as a gate for Windows only; the unix run proves the harness
    /// builds and the fixture is well formed, not that the defect is absent.
    #[test]
    fn atomic_write_survives_a_path_past_windows_max_path() {
        let dir = tempfile::tempdir().unwrap();
        // An ABSOLUTE base: Windows can only rewrite a path to extended-length
        // (`\\?\`) form when it is absolute, so a relative base would measure a
        // different thing than the restore path does.
        let base = dir.path().canonicalize().unwrap();
        let deep = deep_path_over_max_path(&base, "payload.txt");
        let len = deep.as_os_str().len();
        assert!(
            len > 260,
            "fixture is too shallow to reach the defect: {len} chars"
        );

        let parent = deep.parent().unwrap();
        let mkdir = std::fs::create_dir_all(parent);
        println!("LONGPATH-LEN: {len}");
        println!("LONGPATH-CREATE-DIR-ALL: {:?}", mkdir.as_ref().err());
        mkdir.expect("create_dir_all failed past MAX_PATH");

        // Leg 2: plain `std::fs::write`, which goes through std's own
        // `maybe_verbatim` long-path handling.
        let plain = parent.join("plain.txt");
        let plain_res = std::fs::write(&plain, b"plain");
        println!("LONGPATH-STD-WRITE: {:?}", plain_res.as_ref().err());

        // Leg 3: the call the restore loop actually makes.
        let atomic_res = atomic_write(&deep, b"atomic");
        println!("LONGPATH-ATOMIC-WRITE: {:?}", atomic_res.as_ref().err());

        atomic_res.expect("atomic_write failed at a path past MAX_PATH");
        assert_eq!(std::fs::read(&deep).unwrap(), b"atomic");
        // And it must replace, not only create — persist() over an existing
        // long path is a separate Win32 call shape from creating one.
        atomic_write(&deep, b"atomic-2").unwrap();
        assert_eq!(std::fs::read(&deep).unwrap(), b"atomic-2");
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("target.txt");
        std::fs::write(&path, b"old contents").unwrap();

        atomic_write(&path, b"new contents").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new contents");
    }

    #[test]
    fn atomic_write_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        atomic_write(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn atomic_write_failure_leaves_original_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readonly_dir").join("file.txt");
        // Parent doesn't exist — write must fail without affecting any
        // other file. We can't easily simulate a mid-write crash in a
        // unit test, but a missing parent directory exercises the
        // pre-rename error path.
        let result = atomic_write(&path, b"contents");
        assert!(result.is_err());
    }
}
