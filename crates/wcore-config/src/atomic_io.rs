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
use std::path::{Path, PathBuf};

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
/// # Long destination paths on Windows (F26-03-D)
///
/// The tempfile round trip below reaches Win32 (`MoveFileExW`) without
/// `std`'s long-path handling, so a destination past `MAX_PATH` (260)
/// fails with `ERROR_PATH_NOT_FOUND` even where `std::fs::write` to the
/// very same path succeeds. Measured on Windows 11 26200 at a 320-char
/// plain `C:\...` path: `create_dir_all` OK, `fs::write` OK,
/// `atomic_write` **os error 3**.
///
/// [`long_path_safe_dest`] resolves the parent to its extended-length
/// (`\\?\`) form before the round trip, which lifts the limit at the
/// Win32 layer regardless of the machine's `LongPathsEnabled` setting.
pub fn atomic_write<P: AsRef<Path>>(path: P, contents: &[u8]) -> std::io::Result<()> {
    let path = path.as_ref();
    let dest = long_path_safe_dest(path)?;
    let dest = dest.as_ref();
    let tmp = staged_temp_file(dest, contents)?;
    tmp.persist(dest).map_err(|e| e.error)
}

/// [`atomic_write`], for a caller that may only publish over a destination
/// whose current contents it has already judged.
///
/// `accept` is handed the bytes the destination held **at the instant the new
/// bytes were published** — `None` if it held nothing — and returning
/// `Err(why)` retracts the publish. `Ok(Err(why))` therefore means the
/// destination is exactly as it was and `why` says what was found instead.
///
/// # Why an exchange and not a re-check
///
/// The obvious shape is to re-read the destination and then rename over it,
/// and that is what this did. It cannot work: the re-read and the rename are
/// two operations, and an editor saving between them is overwritten. Narrowing
/// the gap only lowers the rate — measured at ~6.5% with the check moved as
/// late as it can go, immediately before the rename (#1155).
///
/// [`exchange`] closes it instead of narrowing it. `RENAME_EXCHANGE` swaps the
/// two names in one atomic step, so the bytes that arrive in the temp file
/// *are* the bytes the destination held at the moment of publication — there
/// is no second observation to be stale. A save that lost the race is then
/// detected with certainty and put back by a second exchange.
///
/// The cost is a bounded window, between the exchange and the verdict, in
/// which a crash would leave the new contents published where a re-check would
/// have refused. The destination is never torn — it holds either the old bytes
/// or the new ones at every instant — and the window is the same read the
/// re-check performed anyway, so this is a change of which whole state
/// survives a crash, not of whether a whole state does.
///
/// Where no exchange primitive exists (see [`exchange`]) this falls back to the
/// re-check, which is racy but no worse than what it replaced.
pub fn atomic_write_checked<P: AsRef<Path>>(
    path: P,
    contents: &[u8],
    accept: impl FnOnce(Option<&[u8]>) -> Result<(), String>,
) -> std::io::Result<Result<(), String>> {
    let path = path.as_ref();
    let dest = long_path_safe_dest(path)?;
    let dest = dest.as_ref();
    let tmp = staged_temp_file(dest, contents)?;

    match publish_displacing(&tmp, dest)? {
        Swap::Displaced(displaced) => {
            // `dest` now names `contents`; `displaced` names what `dest` held
            // at the instant of publication.
            let verdict = match std::fs::read(&displaced) {
                Ok(observed) => accept(Some(&observed)),
                Err(e) => Err(format!("it could no longer be read ({e})")),
            };
            if let Err(why) = verdict {
                // Retract, by the inverse of the step that published.
                if let Err(e) = restore(&displaced, dest) {
                    // The publish stands and the displaced file is the only
                    // copy of what it replaced, so it must not be unlinked.
                    let kept = keep_displaced(tmp, &displaced)?;
                    return Err(std::io::Error::other(format!(
                        "{why}, and the original could not be put back ({e}); \
                         it is preserved at {}",
                        kept.display()
                    )));
                }
                discard_displaced(tmp, &displaced);
                return Ok(Err(why));
            }
            discard_displaced(tmp, &displaced);
            Ok(Ok(()))
        }
        // No exchange to make, or none available. Both fall back to reading the
        // destination and then renaming over it, which is racy — see above.
        Swap::Vacant | Swap::Unsupported => {
            let observed = match std::fs::read(dest) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            if let Err(why) = accept(observed.as_deref()) {
                return Ok(Err(why));
            }
            tmp.persist(dest).map(Ok).map_err(|e| e.error)
        }
    }
}

/// A sibling temp file holding `contents`, fsynced, and already wearing the
/// destination's permission bits so that publishing it never redefines them.
fn staged_temp_file(dest: &Path, contents: &[u8]) -> std::io::Result<tempfile::TempPath> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;
    // B6 — carry the destination's own mode onto the temp file BEFORE it is
    // published, so the name is never republished with the tempfile's 0600.
    carry_destination_mode(&tmp, dest);
    Ok(tmp.into_temp_path())
}

/// What an attempt to publish-and-displace did.
///
/// Where there is no primitive that hands the displaced bytes back,
/// [`publish_displacing`] can only ever answer `Unsupported` and the other two
/// are unconstructible. They are still the right shape for the outcome, so the
/// enum is kept whole rather than split into a platform-specific one.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", windows)),
    allow(dead_code)
)]
enum Swap {
    /// The new bytes are published at the destination, and the payload names a
    /// file holding exactly what the destination held at that instant.
    ///
    /// On the exchange platforms that file IS the staged temp file, which now
    /// names the pre-image. On Windows it is a separate sibling backup, which
    /// is why this carries the path instead of the caller assuming it.
    Displaced(PathBuf),
    /// The destination does not exist, so there was nothing to displace.
    Vacant,
    /// This platform, kernel or filesystem has no such primitive.
    Unsupported,
}

/// Put `displaced` back at `dest`, undoing a [`Swap::Displaced`] publish.
///
/// The inverse of whichever primitive published, so it inherits that
/// primitive's atomicity: a second `RENAME_EXCHANGE` / `RENAME_SWAP` where the
/// publish was one, and a replacing rename on Windows where the publish was
/// `ReplaceFileW`.
///
/// #1155 residual: the discriminant is load-bearing and must not be discarded.
/// Only [`Swap::Displaced`] put the pre-image back. [`Swap::Vacant`] means the
/// destination name has disappeared since the publish (an external `rm`, a
/// `git checkout`, an editor that unlinks before it writes) and
/// [`Swap::Unsupported`] means the primitive refused; in both, NOTHING was
/// exchanged, the caller's new bytes stand published, and `displaced` holds
/// the only surviving copy of what the check refused to replace. Answering
/// `Ok` there sent control on to `discard_displaced`, which unlinked that
/// copy, and `atomic_write_checked` then returned `Ok(Err(why))` -- whose
/// contract is "the destination is exactly as it was" -- over published data
/// loss. Both must be errors, so the `keep_displaced` preservation path runs
/// and the bytes are named to the user.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn restore(displaced: &Path, dest: &Path) -> std::io::Result<()> {
    match publish_displacing(displaced, dest)? {
        Swap::Displaced(_) => Ok(()),
        Swap::Vacant => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the destination no longer exists, so the pre-image could not be exchanged back",
        )),
        Swap::Unsupported => Err(std::io::Error::other(
            "the filesystem refused the exchange, so the pre-image could not be put back",
        )),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn restore(displaced: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::rename(displaced, dest)
}

/// Drop the leftovers of a completed publish.
///
/// `tmp` guards the staged file. On the exchange platforms `displaced` IS that
/// path (the exchange moved the pre-image into it), so dropping the guard is
/// the whole job. On Windows the staged file was consumed by `ReplaceFileW`
/// and `displaced` is a separate backup this module created, so it has to be
/// unlinked explicitly — the `TempPath` drop then finds nothing and is a no-op.
fn discard_displaced(tmp: tempfile::TempPath, displaced: &Path) {
    if displaced != &*tmp {
        let _ = std::fs::remove_file(displaced);
    }
    drop(tmp);
}

/// Keep the displaced pre-image on disk and name it, for the one path where
/// the publish stands and it is the only surviving copy.
fn keep_displaced(tmp: tempfile::TempPath, displaced: &Path) -> std::io::Result<PathBuf> {
    if displaced == &*tmp {
        return Ok(tmp.keep()?);
    }
    drop(tmp);
    Ok(displaced.to_path_buf())
}

/// Publish `a` over `b` and hand back a name holding what `b` held at that
/// instant. The one place the platform difference lives.
///
/// - **Linux** — `renameat2(RENAME_EXCHANGE)`, since 3.15. Invoked as a raw
///   syscall rather than through the glibc wrapper, which arrived in 2.28 and
///   does not exist on musl at all. The displaced name is `a` itself.
/// - **macOS** — `renamex_np(RENAME_SWAP)`, since 10.12. APFS and HFS+ only.
///   The displaced name is `a` itself.
/// - **Windows** — `ReplaceFileW` with a backup name (#1155's second
///   residual). Win32 has no exchange, and this is not one: `ReplaceFileW`
///   renames the replaced file to `lpBackupFileName` and then renames the
///   replacement into its place, so unlike `RENAME_EXCHANGE` there is an
///   instant at which the destination name does not resolve. What it DOES
///   give is the property the verdict actually needs — the displaced bytes,
///   under a name this module chose — so the check becomes an observation
///   taken AFTER publication rather than a re-read taken before it, and the
///   check-then-write window closes the same way it does elsewhere. The
///   earlier reading, recorded in this file, that `ReplaceFile` "does not
///   publish the displaced file under a name the caller chooses" was simply
///   wrong about `lpBackupFileName`.
///
///   **Ungraded on Windows by the lane that wrote it.** It is reachable only
///   on a platform this workspace cannot execute from Linux, so it ships
///   verified by `cargo check --target x86_64-pc-windows-gnu` and by
///   [`tests::the_check_is_handed_the_bytes_the_publish_displaced`], whose
///   Windows arm now asserts the post-publication reading and will fail the
///   Windows CI job if any of the above is wrong.
///
/// - **Everything else** — [`Swap::Unsupported`], and the caller falls back to
///   re-check-then-rename.
///
/// Any failure other than a missing destination degrades to
/// [`Swap::Unsupported`] rather than failing the write: the fallback is what
/// every platform did before this existed, so the worst case of a primitive
/// that misbehaves is the previous behaviour, never a lost write.
#[cfg(target_os = "linux")]
fn publish_displacing(a: &Path, b: &Path) -> std::io::Result<Swap> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    fn c(p: &Path) -> std::io::Result<CString> {
        CString::new(p.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    }
    let (ca, cb) = (c(a)?, c(b)?);

    // SAFETY: both pointers are NUL-terminated C strings owned by locals that
    // outlive the call, and AT_FDCWD is the documented "resolve relative to the
    // working directory" sentinel for the *at family.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2 as libc::c_long,
            libc::AT_FDCWD,
            ca.as_ptr(),
            libc::AT_FDCWD,
            cb.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if rc == 0 {
        return Ok(Swap::Displaced(a.to_path_buf()));
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ENOENT) => Ok(Swap::Vacant),
        // ENOSYS: kernel older than 3.15. EINVAL / EOPNOTSUPP: the filesystem
        // does not implement the flag.
        Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP) => Ok(Swap::Unsupported),
        _ => Err(err),
    }
}

#[cfg(target_os = "macos")]
fn publish_displacing(a: &Path, b: &Path) -> std::io::Result<Swap> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    fn c(p: &Path) -> std::io::Result<CString> {
        CString::new(p.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    }
    let (ca, cb) = (c(a)?, c(b)?);

    // SAFETY: as above — both pointers are NUL-terminated C strings owned by
    // locals that outlive the call.
    let rc = unsafe { libc::renamex_np(ca.as_ptr(), cb.as_ptr(), libc::RENAME_SWAP) };
    if rc == 0 {
        return Ok(Swap::Displaced(a.to_path_buf()));
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ENOENT) => Ok(Swap::Vacant),
        // A volume without VOL_CAP_INT_RENAME_SWAP: FAT, SMB, and others.
        Some(libc::ENOTSUP) | Some(libc::EOPNOTSUPP) | Some(libc::EINVAL) => Ok(Swap::Unsupported),
        _ => Err(err),
    }
}

/// `ReplaceFileW(dest, replacement, backup)` — see [`publish_displacing`]'s
/// doc for why this is the Windows analogue and what it does not promise.
///
/// The backup name is derived from the staged temp file's own (already random,
/// already unique) name, so it is a sibling on the same volume — `ReplaceFileW`
/// requires that — without creating a second file to reserve a name.
#[cfg(windows)]
fn publish_displacing(a: &Path, b: &Path) -> std::io::Result<Swap> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
    };

    /// `ERROR_FILE_NOT_FOUND`. Spelled out rather than imported so the mapping
    /// to [`Swap::Vacant`] reads at the match arm.
    const ERROR_FILE_NOT_FOUND: i32 = 2;

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let Some(stem) = a.file_name() else {
        return Ok(Swap::Unsupported);
    };
    let mut backup_name = stem.to_os_string();
    backup_name.push(".wl-displaced");
    let backup = a.with_file_name(backup_name);
    // ReplaceFileW moves the replaced file ONTO this name; a stale file there
    // from a killed run must not be mistaken for the pre-image.
    let _ = std::fs::remove_file(&backup);

    // SAFETY: all three pointers are NUL-terminated UTF-16 buffers owned by
    // locals that outlive the call, and both reserved parameters are the
    // documented NULL.
    let ok = unsafe {
        ReplaceFileW(
            wide(b).as_ptr(),
            wide(a).as_ptr(),
            wide(&backup).as_ptr(),
            REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok != 0 {
        return Ok(Swap::Displaced(backup));
    }

    let err = std::io::Error::last_os_error();
    // ReplaceFileW documents partial failures in which the replaced file has
    // already been moved aside. Put it back before degrading, so the caller's
    // fallback publishes over the destination rather than over a hole.
    if backup.exists() && !b.exists() {
        let _ = std::fs::rename(&backup, b);
    }
    let _ = std::fs::remove_file(&backup);
    if err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) {
        return Ok(Swap::Vacant);
    }
    Ok(Swap::Unsupported)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn publish_displacing(_a: &Path, _b: &Path) -> std::io::Result<Swap> {
    Ok(Swap::Unsupported)
}

/// Copy an EXISTING destination's permission bits onto the temp file that is
/// about to replace it (B6).
///
/// `NamedTempFile` creates its file 0600 by design, and the rename carries
/// that mode onto the destination name. Every rewrite of an existing file
/// therefore redefined its permissions: a 0755 script edited by the agent came
/// back 0600 and the agent's own next turn got `Exit code: 126 … Permission
/// denied`; a 0644 file silently lost group/other read.
///
/// Only an existing destination is matched. A file this helper CREATES keeps
/// the private 0600 — credentials, the session mirror and the memory store are
/// all written through here, and widening a new file would be a security
/// change nobody asked for.
///
/// Best-effort: a failure to read or apply the mode must not fail the write,
/// which would turn a cosmetic permission problem into data loss.
#[cfg(unix)]
fn carry_destination_mode(tmp: &tempfile::NamedTempFile, dest: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(existing) = std::fs::metadata(dest) {
        // Mask to the permission/setuid/sticky bits; `mode()` also carries the
        // file-type bits, which are not the kernel's to take from a chmod.
        let bits = existing.permissions().mode() & 0o7777;
        let _ = tmp
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(bits));
    }
}

/// Windows has no POSIX mode. The nearest analogue, the read-only ATTRIBUTE,
/// must NOT be copied onto the temp file: `MoveFileExW(REPLACE_EXISTING)`
/// refuses a read-only participant, so carrying it would turn today's silent
/// permission change into a failed write. Left as a no-op deliberately rather
/// than by omission.
#[cfg(not(unix))]
fn carry_destination_mode(_tmp: &tempfile::NamedTempFile, _dest: &Path) {}

/// Length past which a destination is rewritten to extended-length form on
/// Windows. Well below `MAX_PATH` (260) so there is headroom for the sibling
/// temp file's own name, which is what the round trip actually creates.
#[cfg(windows)]
const WINDOWS_LONG_PATH_THRESHOLD: usize = 200;

/// The single place platform long-path handling lives, per the
/// centralize-platform-differences rule. On unix this is the identity.
///
/// Only paths at or past the threshold are rewritten, so the ordinary short
/// path keeps exactly its present behavior and the change is bounded to the
/// case that is broken today.
#[cfg(windows)]
fn long_path_safe_dest(path: &Path) -> std::io::Result<std::borrow::Cow<'_, Path>> {
    use std::borrow::Cow;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let too_long = path.as_os_str().len() >= WINDOWS_LONG_PATH_THRESHOLD
        || parent.as_os_str().len() >= WINDOWS_LONG_PATH_THRESHOLD;
    if !too_long || path.to_string_lossy().starts_with(r"\\?\") {
        return Ok(Cow::Borrowed(path));
    }
    let Some(file_name) = path.file_name() else {
        return Ok(Cow::Borrowed(path));
    };
    // `canonicalize` is how std spells "extended-length form" on Windows. The
    // parent must already exist for the tempfile round trip to work at all, so
    // resolving it here costs one metadata call and no new failure mode: if it
    // cannot be resolved the original path is used and the caller sees the same
    // error it would have seen anyway.
    match std::fs::canonicalize(parent) {
        Ok(canon) => Ok(Cow::Owned(canon.join(file_name))),
        Err(_) => Ok(Cow::Borrowed(path)),
    }
}

#[cfg(not(windows))]
#[inline]
fn long_path_safe_dest(path: &Path) -> std::io::Result<std::borrow::Cow<'_, Path>> {
    // unix has no MAX_PATH equivalent at this scale (PATH_MAX is 4096) and no
    // extended-length form, so there is nothing to rewrite.
    Ok(std::borrow::Cow::Borrowed(path))
}

/// Build a path under `base` whose total length exceeds Windows' `MAX_PATH`
/// (260), using nesting rather than one long component so that every individual
/// component stays inside the 255-byte per-component limit every filesystem
/// enforces.
///
/// Shared by the tests below and by `wcore-cli`'s backup long-path proof, so
/// both measure the same shape rather than two hand-rolled approximations that
/// could drift apart and disagree about what "deep" means.
///
/// `sep` selects the separator used to build the RELATIVE part. This is the
/// variable under test, not a detail: a backup manifest records payload paths
/// `/`-separated (`path-norm=slash-relative`), so the restore reconstructs a
/// target path by joining ONE forward-slash string onto the target root. A
/// fixture built with `\` is a different shape and, measured, does not reach
/// the defect.
#[doc(hidden)]
pub fn deep_path_over_max_path(base: &Path, leaf: &str, sep: char) -> std::path::PathBuf {
    const WINDOWS_MAX_PATH: usize = 260;
    let mut rel = String::new();
    // Components of 40 chars each, each well inside the 255-byte per-component
    // limit every filesystem enforces, nested until the total is past 260.
    while base.as_os_str().len() + rel.len() + leaf.len() + 2 <= WINDOWS_MAX_PATH + 40 {
        rel.push_str("d234567890123456789012345678901234567890");
        rel.push(sep);
    }
    rel.push_str(leaf);
    // A single join of the whole relative string, exactly as the restore write
    // loop does it (`target.join(&entry.path)`).
    base.join(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F26-03-D. The Windows restore failed with `os error 3` past `MAX_PATH`
    /// while `create` accepted the same tree.
    ///
    /// The separator is the variable, and it is the whole point. A first
    /// version of this test built the deep path with `\` and PASSED on Windows
    /// at 306 characters — every leg green, defect untouched. The restore does
    /// not build paths that way: a manifest records payload paths
    /// `/`-separated, so the write loop joins one forward-slash string onto the
    /// target root. The `\` leg is kept as a CONTROL: it proves the harness,
    /// the box and the length regime are all fine, so a failure on the `/` leg
    /// isolates the separator rather than the depth.
    ///
    /// Each of the three calls the restore path makes is measured separately —
    /// `create_dir_all`, plain `fs::write`, `atomic_write` — because they fail
    /// for different reasons and a combined check would hide which.
    ///
    /// On unix this is already green against the untouched tree: `PATH_MAX` is
    /// 4096 and `/` is the native separator, so neither variable exists there.
    /// The unix run proves the fixture is well formed; only the Windows run is
    /// a gate.
    #[test]
    fn atomic_write_survives_a_path_past_windows_max_path() {
        // A PLAIN absolute base, deliberately NOT `canonicalize()`d. On Windows
        // `canonicalize()` returns an extended-length `\\?\` path, which is
        // already in the form that lifts the 260 limit — a base built that way
        // measures the working mode and can never reach the defect. A restore
        // target arrives from the command line as a plain `C:\...` path, so
        // that is what the fixture must use.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_path_buf();
        assert!(
            !base.to_string_lossy().starts_with(r"\\?\"),
            "fixture base is already verbatim, so it cannot reach the defect: {}",
            base.display()
        );

        // The control leg runs FIRST. If it fails, the box or the harness is
        // the problem and the `/` result would be uninterpretable.
        //
        // The control uses the platform's NATIVE separator, not a literal `\`.
        // On unix `\` is an ordinary filename character, so a `\`-built fixture
        // is one 300-byte component rather than a nested path, and it fails
        // with ENAMETOOLONG against `NAME_MAX` — a fixture defect that says
        // nothing about `atomic_write`. Measured: it did exactly that on Linux.
        // On unix the two legs are therefore the same shape, and the test
        // honestly degenerates to a single measurement there; the separator
        // distinction only exists on Windows.
        let native = std::path::MAIN_SEPARATOR;
        for (label, sep) in [
            ("control-native-sep", native),
            ("subject-forwardslash", '/'),
        ] {
            let root = base.join(label);
            std::fs::create_dir_all(&root).unwrap();
            let deep = deep_path_over_max_path(&root, "payload.txt", sep);
            let len = deep.as_os_str().len();
            assert!(
                len > 260,
                "[{label}] fixture too shallow to reach the defect: {len} chars"
            );

            let parent = deep.parent().unwrap();
            let mkdir = std::fs::create_dir_all(parent);
            println!("LONGPATH[{label}]-LEN: {len}");
            println!(
                "LONGPATH[{label}]-CREATE-DIR-ALL: {:?}",
                mkdir.as_ref().err()
            );
            mkdir.unwrap_or_else(|e| panic!("[{label}] create_dir_all failed past MAX_PATH: {e}"));

            // Plain `fs::write` goes through std's own long-path handling;
            // `atomic_write` additionally goes through the tempfile round trip.
            // Measuring both tells us which layer is responsible.
            let plain = parent.join("plain.txt");
            let plain_res = std::fs::write(&plain, b"plain");
            println!(
                "LONGPATH[{label}]-STD-WRITE: {:?}",
                plain_res.as_ref().err()
            );

            let atomic_res = atomic_write(&deep, b"atomic");
            println!(
                "LONGPATH[{label}]-ATOMIC-WRITE: {:?}",
                atomic_res.as_ref().err()
            );

            plain_res.unwrap_or_else(|e| panic!("[{label}] fs::write failed past MAX_PATH: {e}"));
            atomic_res
                .unwrap_or_else(|e| panic!("[{label}] atomic_write failed past MAX_PATH: {e}"));
            assert_eq!(std::fs::read(&deep).unwrap(), b"atomic");
            // It must REPLACE as well as create — persisting over an existing
            // long path is a different Win32 call shape from creating one.
            atomic_write(&deep, b"atomic-2").unwrap();
            assert_eq!(std::fs::read(&deep).unwrap(), b"atomic-2");
        }
    }

    /// #1155. The pre-image handed to the check is the one the publish
    /// DISPLACED, not one read back from the path afterwards, and the
    /// difference is observable: by the time the check runs, the destination
    /// already holds the new bytes.
    ///
    /// This is the whole mechanism, so it is asserted directly rather than
    /// only through the concurrent arm in `wcore-tools`, which can only ever
    /// sample the race.
    #[test]
    fn the_check_is_handed_the_bytes_the_publish_displaced() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"old").unwrap();

        let mut seen: Option<Vec<u8>> = None;
        let mut on_disk_during = Vec::new();
        let r = atomic_write_checked(&p, b"new", |observed| {
            seen = observed.map(<[u8]>::to_vec);
            on_disk_during = std::fs::read(&p).unwrap();
            Ok(())
        })
        .unwrap();

        assert!(r.is_ok());
        assert_eq!(seen.as_deref(), Some(&b"old"[..]), "displaced bytes");
        // The publish has ALREADY happened when the check runs, so the
        // destination reads as the new bytes and a refusal is a rollback.
        //
        // This assertion used to be split: Windows had no primitive that gave
        // the displaced bytes back, degraded to re-check-then-rename, and this
        // arm asserted `b"old"` -- i.e. it asserted that the #1155 race stayed
        // open there. `publish_displacing` now uses `ReplaceFileW` with a
        // backup name, so the property is the same on every platform and this
        // is one assertion again. It is ALSO the grading instrument for that
        // Windows path, which no Linux host can execute: if `ReplaceFileW`
        // does not behave as `publish_displacing`'s doc claims, this fails in
        // the Windows CI job.
        assert_eq!(
            on_disk_during, b"new",
            "the publish precedes the check, so the check is handed what it displaced"
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
    }

    /// A refused publish leaves the destination byte-identical. The publish has
    /// already happened when the check runs, so this exercises the rollback,
    /// not a skipped write.
    #[test]
    fn a_refused_publish_puts_the_destination_back() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"theirs").unwrap();
        let before = std::fs::metadata(&p).unwrap();

        let r = atomic_write_checked(&p, b"ours", |_| Err("changed".to_owned())).unwrap();

        assert_eq!(r, Err("changed".to_owned()));
        assert_eq!(std::fs::read(&p).unwrap(), b"theirs");
        // The rolled-back destination is the ORIGINAL file, not a copy of it:
        // an editor holding it open must not find itself writing to an inode
        // nothing points at any more.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().ino(),
                before.ino(),
                "the rollback published a different inode at the destination"
            );
        }
        let _ = before;
        // And no temp file is left behind.
        let strays: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != "f.txt")
            .collect();
        assert!(strays.is_empty(), "left behind {strays:?}");
    }

    /// An absent destination has nothing to exchange with, so the check is told
    /// `None` rather than being skipped.
    #[test]
    fn an_absent_destination_is_reported_as_no_pre_image() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("new.txt");

        let mut seen = Some(vec![0u8]);
        let r = atomic_write_checked(&p, b"body", |observed| {
            seen = observed.map(<[u8]>::to_vec);
            Ok(())
        })
        .unwrap();

        assert!(r.is_ok());
        assert_eq!(seen, None);
        assert_eq!(std::fs::read(&p).unwrap(), b"body");
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

    /// B6 — the rename-over-tempfile publish must not redefine the
    /// destination's permissions.
    ///
    /// `NamedTempFile` creates its file 0600 by design, and `persist()`
    /// carries that mode onto the destination name, so every rewrite of an
    /// existing file silently replaced its mode with 0600. Measured through
    /// the product: a 0755 script edited by the agent came back 0600 and the
    /// agent's own next turn got `Exit code: 126 … Permission denied`.
    ///
    /// Two files are seeded identically and only one is written. The
    /// untouched sibling is the CONTROL: if its mode moves too, the
    /// environment (umask, filesystem, test runner) is responsible and the
    /// subject measurement says nothing.
    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_the_destination_files_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let subject = dir.path().join("edited.sh");
        let control = dir.path().join("untouched.sh");
        for p in [&subject, &control] {
            std::fs::write(p, b"#!/bin/sh\necho HELLO\n").unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mode =
            |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        // The fixture itself must be real before anything is measured off it.
        assert_eq!(mode(&subject), 0o755, "fixture did not take 0755");

        atomic_write(&subject, b"#!/bin/sh\necho GOODBYE\n").unwrap();

        assert_eq!(
            mode(&control),
            0o755,
            "CONTROL: the untouched sibling lost its mode, so the environment \
             is stripping modes and the subject leg is uninterpretable"
        );
        assert_eq!(
            mode(&subject),
            0o755,
            "atomic_write redefined the destination's mode: 0o755 -> 0o{:o} \
             (an edited shell script stops being executable)",
            mode(&subject)
        );
        assert_eq!(
            std::fs::read(&subject).unwrap(),
            b"#!/bin/sh\necho GOODBYE\n"
        );
    }

    /// The other half of B6: a file the caller creates through this helper
    /// keeps the private-by-default 0600 it has always had. Credentials,
    /// session mirrors and the memory store are all written this way and
    /// several of them rely on it, so preservation must apply to an EXISTING
    /// destination only — never widen a new file.
    #[cfg(unix)]
    #[test]
    fn atomic_write_still_creates_new_files_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.json");
        atomic_write(&path, b"{}").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o600,
            "a newly created file must stay owner-only"
        );
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
    /// #1155 residual. A rollback that exchanged NOTHING must never be
    /// reported as a clean refusal.
    ///
    /// `restore` is the inverse exchange. It can answer three ways, and only
    /// one of them put the pre-image back. `Swap::Vacant` means the
    /// destination name has disappeared since the publish -- an external
    /// `rm`, a `git checkout`, an editor that unlinks before it writes --
    /// which is exactly the non-cooperating concurrent writer this module
    /// exists to survive. `Swap::Unsupported` means the primitive refused.
    /// In both, nothing was swapped: the caller's new bytes are still
    /// published (or the name is gone), and `displaced` holds the ONLY
    /// surviving copy of what the check refused to replace.
    ///
    /// Discarding the discriminant answered `Ok` for all three. Control then
    /// fell to `discard_displaced`, which unlinked that only copy, and
    /// `atomic_write_checked` returned `Ok(Err(why))` -- whose documented
    /// contract is "the destination is exactly as it was". The caller
    /// (`edit.rs`, `write.rs`) rendered `changed_under_write` and told the
    /// user nothing had been written, over published data loss.
    ///
    /// Only the exchange platforms are affected: the Windows arm is a
    /// replacing `fs::rename`, which reports the failure it had.
    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn a_rollback_that_exchanged_nothing_is_not_a_clean_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"the user's only copy").unwrap();

        let victim = p.clone();
        let outcome = atomic_write_checked(&p, b"ours", move |observed| {
            assert_eq!(observed, Some(&b"the user's only copy"[..]));
            // The concurrent writer unlinks the destination while we judge.
            std::fs::remove_file(&victim).unwrap();
            Err("changed under write".to_owned())
        });

        let err = match outcome {
            Err(e) => e,
            Ok(clean) => {
                let survivors: Vec<_> = std::fs::read_dir(dir.path())
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                panic!(
                    "a rollback that exchanged nothing was reported as {clean:?} \
                     -- the caller will tell the user the destination is untouched. \
                     Surviving files in the directory: {survivors:?}"
                )
            }
        };
        let msg = err.to_string();
        let kept = msg
            .split("it is preserved at ")
            .nth(1)
            .unwrap_or_else(|| panic!("the error must name where the bytes were kept: {msg}"))
            .trim();
        assert_eq!(
            std::fs::read(kept).unwrap(),
            b"the user's only copy",
            "the preserved file must hold the bytes the refusal was protecting"
        );
    }
}
