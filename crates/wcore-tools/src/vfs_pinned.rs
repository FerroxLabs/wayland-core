//! Handle-pinned file read — the "use" half of `VirtualFs::read_pinned`
//! (FerroxLabs/wayland#1105).
//!
//! `SandboxedFs` decides whether a path is permitted by canonicalizing it and
//! comparing the result against its root and its standing read grants. That
//! decision is about an OBJECT, but what it can pass on is a NAME. If the
//! backend then resolves that name a second time, anything able to create a
//! file in the directory can swap the leaf in between, and the object that was
//! approved is not the object that gets read. Snyk demonstrated exactly this
//! against another agent with `renameat2(RENAME_EXCHANGE)` and noted that more
//! `lstat`/`realpath` checks cannot fix it, because the check stays split from
//! the use.
//!
//! This module removes the second resolution of the leaf. The parent directory
//! is opened once as a RETAINED handle with symlinks refused, the leaf is
//! opened RELATIVE to that handle with symlinks refused, its type is checked
//! from the open descriptor, and the bytes are read from that same descriptor.
//! There is no pathname left for anyone to redirect.
//!
//! **What this does NOT close, deliberately.** The parent is opened by
//! pathname, so the kernel still walks the components ABOVE it ambiently. An
//! attacker who can rename a directory higher up and drop a symlink in its
//! place still wins. Closing that needs a directory handle retained from the
//! moment the grant is made and `openat2(RESOLVE_BENEATH)` (Linux 5.6+)
//! beneath it, which changes the shape of a grant and is not this change.
//!
//! **Platform behaviour, recorded rather than assumed.**
//! * Unix (Linux, macOS): `O_NOFOLLOW` on both opens plus `O_DIRECTORY` on the
//!   parent. `O_NOFOLLOW` is POSIX and behaves identically on both.
//! * Windows: `O_NOFOLLOW` does not exist and would be a no-op if it did, so
//!   the equivalent is built from `NtCreateFile` with `RootDirectory` (the only
//!   `openat` Windows offers), `FILE_OPEN_REPARSE_POINT` to open a
//!   symlink/junction as ITSELF rather than traversing it, and an explicit
//!   `FILE_ATTRIBUTE_REPARSE_POINT` refusal afterwards. This reuses the exact
//!   primitives `vfs::observe_real_file_windows` already uses. It is
//!   compile-verified from Linux via `--target x86_64-pc-windows-gnu`; a
//!   cross-target lint sees `cfg(windows)` code and proves nothing about
//!   `NtCreateFile` runtime semantics.
//! * Anything else: `Unsupported`, which `SandboxedFs::read` turns into a
//!   refusal. There is no such CI target today; a fallback to a path-based read
//!   would be a silent downgrade and is worse than an honest error.

use std::io;
use std::path::Path;

use crate::vfs::VfsError;

/// Read `path`'s bytes with the leaf resolved exactly once, relative to a
/// retained parent-directory handle.
///
/// Blocking. Callers on an async runtime must wrap this in `spawn_blocking`.
pub(crate) fn pinned_read_bytes(path: &Path) -> Result<Vec<u8>, VfsError> {
    #[cfg(unix)]
    {
        pinned_read_unix(path)
    }
    #[cfg(windows)]
    {
        pinned_read_windows(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(VfsError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "handle-pinned reads are unavailable on this platform: {}",
                path.display()
            ),
        )))
    }
}

#[cfg(any(unix, windows))]
fn raced(path: &Path, reason: &str) -> VfsError {
    VfsError::PathRaced {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    }
}

/// Split into (parent directory, leaf name). A path with no file name — the
/// filesystem root, or one ending in `..` — names no readable object.
#[cfg(any(unix, windows))]
fn split_leaf(path: &Path) -> Result<(&Path, &std::ffi::OsStr), VfsError> {
    let leaf = path.file_name().ok_or_else(|| {
        VfsError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a pinned read requires a file name: {}", path.display()),
        ))
    })?;
    // `file_name()` returning `Some` guarantees a parent exists, but an empty
    // one ("x.txt" with no directory) is not something we can open as a dir.
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let parent = parent.ok_or_else(|| {
        VfsError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "a pinned read requires an anchored parent directory: {}",
                path.display()
            ),
        ))
    })?;
    Ok((parent, leaf))
}

#[cfg(unix)]
fn pinned_read_unix(path: &Path) -> Result<Vec<u8>, VfsError> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let (parent_path, leaf) = split_leaf(path)?;

    // The parent's own final component must not be a symlink and must be a
    // directory. Both are true of the canonical path `SandboxedFs` produced at
    // check time, so a violation here means the directory was replaced since.
    let parent = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(parent_path)
        .map_err(|error| match error.raw_os_error() {
            Some(libc::ELOOP) | Some(libc::ENOTDIR) => raced(
                path,
                "the parent directory was replaced by a symlink or a non-directory",
            ),
            _ => VfsError::Io(error),
        })?;

    // O_NOFOLLOW: a symlink at the leaf is ELOOP, never a traversal.
    // O_NONBLOCK: a FIFO opens immediately instead of blocking for a writer,
    // so a planted pipe cannot wedge the turn before the type check runs.
    let mut file = crate::vfs::openat_file(
        &parent,
        leaf,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        0,
    )
    .map_err(|error| match error.raw_os_error() {
        Some(libc::ELOOP) => raced(path, "a symlink was in place of the approved file"),
        _ => VfsError::Io(error),
    })?;

    // Type is read from the OPEN descriptor, not from the name, so it describes
    // the object the bytes will come from. Unlike `observe_unix_file` this does
    // NOT require `nlink == 1`: that is a compare-exchange rule, and an
    // ordinary hardlinked file (`.git` object stores, package caches) is
    // perfectly readable.
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(raced(
            path,
            "the approved name resolved to a directory, device or pipe rather \
             than a regular file",
        ));
    }

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(windows)]
fn pinned_read_windows(path: &Path) -> Result<Vec<u8>, VfsError> {
    use std::io::Read as _;

    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

    let (parent_path, leaf) = split_leaf(path)?;

    // `FILE_FLAG_OPEN_REPARSE_POINT` is what makes this the `O_NOFOLLOW`
    // equivalent for the parent: a junction or directory symlink planted here
    // opens as ITSELF, and the attribute check below refuses it instead of
    // silently traversing to wherever it points.
    let parent = crate::vfs::open_windows_directory(parent_path).map_err(VfsError::Io)?;
    let parent_identity = crate::vfs::windows_object_identity(&parent)?;
    if parent_identity.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || parent_identity.attributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err(raced(
            path,
            "the parent directory was replaced by a reparse point or a non-directory",
        ));
    }

    // `NtCreateFile` with `RootDirectory` resolves the name inside the retained
    // handle only — no pathname walk, so nothing to redirect. `FILE_NON_
    // DIRECTORY_FILE` refuses a directory at open time and
    // `FILE_OPEN_REPARSE_POINT` opens a symlink as itself.
    let mut file = crate::vfs::open_windows_child_no_follow(&parent, leaf).map_err(VfsError::Io)?;
    let identity = crate::vfs::windows_object_identity(&file)?;
    if identity.attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(raced(
            path,
            "the approved name resolved to a reparse point or a directory \
             rather than a regular file",
        ));
    }

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}
