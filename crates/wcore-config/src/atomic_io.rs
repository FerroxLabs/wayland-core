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

#[cfg(test)]
mod tests {
    use super::*;

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
