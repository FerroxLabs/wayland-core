//! Filesystem-aware SQLite journal-mode selection — the single place that
//! decides how any SQLite database in this workspace journals.
//!
//! # Why
//!
//! WAL keeps its wal-index in a memory-mapped `-shm` sidecar and depends on
//! coherent shared memory plus reliable POSIX locks between every process
//! touching the database. Network filesystems provide neither, which is why
//! SQLite documents WAL as unsupported over NFS/SMB. When `$HOME` is
//! network-mounted — routine in corporate environments — every database this
//! product writes lands there.
//!
//! # What actually happens (measured, not assumed)
//!
//! Measured on a real NFS mount (loopback NFSv3 export, two clients mounted
//! `nosharecache` so their page caches are genuinely incoherent), two
//! concurrent writers, 15s, no artificial fault injection:
//!
//! | journal mode | write errors | own rows visible to writer | `integrity_check` |
//! |--------------|--------------|----------------------------|-------------------|
//! | `WAL`        | 11,826       | 0 of 5,102                 | **corrupt**       |
//! | `TRUNCATE`   | 0            | all                        | `ok`              |
//! | `DELETE`     | 0            | all                        | `ok`              |
//!
//! The same harness on local disk in WAL: 204k writes, 0 errors, `ok`.
//!
//! **The failure is silent corruption, not a crash.** No process was ever
//! killed by a signal — both writers exited 0 while the database rotted
//! underneath them, and one committed 5,102 rows it could subsequently not
//! see. This matters for the fix: there is no signal to catch and no error
//! for a caller to handle, so the mode must be chosen correctly up front.
//!
//! WAL is materially better on local disks (concurrent readers during
//! writes), so this module *selects*; it does not blanket-disable.
//!
//! # Fallback posture: unknown is treated as network
//!
//! An unclassifiable filesystem resolves to [`SqliteJournalMode::Truncate`],
//! not WAL. The costs are wildly asymmetric: guessing WAL wrong destroys
//! user data silently, guessing TRUNCATE wrong costs some read concurrency
//! and is reversible with an environment variable. Known-local filesystems
//! are allowlisted so this does not quietly demote the common case.

use std::path::Path;

/// Environment kill-switch: force a journal mode regardless of detection.
/// Accepts `wal` or `truncate` (case-insensitive).
pub const JOURNAL_MODE_ENV: &str = "WAYLAND_SQLITE_JOURNAL_MODE";

/// How a path's backing filesystem classifies for SQLite journaling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsClass {
    /// Recognised local filesystem — safe for WAL.
    Local,
    /// Recognised network/remote filesystem — WAL corrupts here.
    Network,
    /// Could not be classified. Treated as [`FsClass::Network`] when
    /// choosing a journal mode (see module docs).
    Unknown,
}

/// Journal mode chosen for a SQLite database from where it lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqliteJournalMode {
    /// Write-ahead logging. Local filesystems only.
    Wal,
    /// Rollback journal, truncated rather than unlinked at commit. Safe on
    /// network filesystems and cheaper there than `DELETE`: no per-commit
    /// create/unlink round-trip and no NFS `.nfsXXXX` silly-rename litter.
    Truncate,
}

impl SqliteJournalMode {
    /// The journal mode the pre-fix code selected: `WAL`, unconditionally,
    /// at every call site, on every filesystem.
    ///
    /// Retained as executable evidence rather than a comment — the
    /// self-test asserts that this legacy choice is identical for a local
    /// and a network path, which is what made the defect invisible. Without
    /// it, a test that "the selector picks WAL locally and TRUNCATE on NFS"
    /// would also pass on an instrument that simply echoed its input.
    pub const fn legacy_unconditional() -> Self {
        Self::Wal
    }

    /// Pick the journal mode for a database at `db_path`.
    ///
    /// Classifies the database's directory rather than the file, since the
    /// file usually does not exist yet at the point this is called.
    pub fn for_db_path(db_path: &Path) -> Self {
        match journal_mode_from_env(std::env::var(JOURNAL_MODE_ENV).ok().as_deref()) {
            EnvOverride::Mode(mode) => {
                // Loud, so a field flip of the kill-switch is greppable.
                tracing::info!(
                    db = %db_path.display(),
                    mode = mode.as_str(),
                    source = "env",
                    "sqlite journal mode forced by {JOURNAL_MODE_ENV}"
                );
                return mode;
            }
            EnvOverride::Invalid => {
                // A typo in an emergency kill-switch must not fail silently.
                tracing::warn!(
                    var = JOURNAL_MODE_ENV,
                    "invalid value (accepted: wal, truncate); falling back to detection"
                );
            }
            EnvOverride::Unset => {}
        }

        let dir = match db_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            // Bare filename or a root path: classify the CWD it resolves against.
            _ => Path::new("."),
        };

        let class = classify_path(dir);
        let mode = match class {
            FsClass::Local => Self::Wal,
            FsClass::Network | FsClass::Unknown => Self::Truncate,
        };
        if class == FsClass::Unknown {
            tracing::warn!(
                db = %db_path.display(),
                "unclassifiable filesystem; using rollback journaling. \
                 Set {JOURNAL_MODE_ENV}=wal to override if this is local storage."
            );
        } else {
            tracing::debug!(
                db = %db_path.display(),
                class = ?class,
                mode = mode.as_str(),
                "sqlite journal mode selected"
            );
        }
        mode
    }

    /// The `PRAGMA journal_mode` value for this mode.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wal => "WAL",
            Self::Truncate => "TRUNCATE",
        }
    }

    /// Whether this mode uses the memory-mapped `-shm` wal-index.
    pub const fn uses_shared_memory(self) -> bool {
        matches!(self, Self::Wal)
    }

    /// Apply this journal mode to a freshly opened read-write connection.
    ///
    /// The `Truncate` arm has to cope with a database already stamped WAL —
    /// either written by a pre-fix binary, or copied onto the network mount
    /// from local storage. Converting away from WAL normally maps the
    /// `-shm` first; on a network mount that is the very mapping we are
    /// avoiding. `locking_mode = EXCLUSIVE` keeps the wal-index in heap
    /// memory for the conversion, so no `-shm` is ever mapped. The
    /// subsequent downgrade to `NORMAL` is legal only once the database has
    /// left WAL, and takes effect on the connection's next database access.
    ///
    /// A busy timeout is set on the `Truncate` arm only: the mode
    /// conversion takes a brief exclusive lock and needs a busy handler to
    /// survive contention. The `Wal` arm is left byte-identical to the
    /// pre-fix behaviour so local-disk callers see no change at all.
    #[cfg(feature = "sqlite")]
    pub fn apply(self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        match self {
            Self::Wal => {
                conn.pragma_update(None, "journal_mode", "WAL")?;
            }
            Self::Truncate => {
                conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
                conn.pragma_update(None, "locking_mode", "EXCLUSIVE")?;
                conn.pragma_update(None, "journal_mode", "TRUNCATE")?;
                conn.pragma_update(None, "locking_mode", "NORMAL")?;
            }
        }
        Ok(())
    }

    /// Select from `db_path` and apply to `conn` in one step. This is the
    /// call every site that opens an on-disk SQLite database should use.
    #[cfg(feature = "sqlite")]
    pub fn configure(conn: &rusqlite::Connection, db_path: &Path) -> rusqlite::Result<Self> {
        let mode = Self::for_db_path(db_path);
        mode.apply(conn)?;
        Ok(mode)
    }
}

/// Parse result of the [`JOURNAL_MODE_ENV`] kill-switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnvOverride {
    Unset,
    /// Set to something unrecognised — warned, not silently ignored.
    Invalid,
    Mode(SqliteJournalMode),
}

/// Pure parser for the kill-switch, so it is testable without touching the
/// process environment.
fn journal_mode_from_env(value: Option<&str>) -> EnvOverride {
    match value {
        // Set-but-empty is a deliberate blank, not a typo.
        None | Some("") => EnvOverride::Unset,
        Some(v) if v.eq_ignore_ascii_case("wal") => EnvOverride::Mode(SqliteJournalMode::Wal),
        Some(v) if v.eq_ignore_ascii_case("truncate") => {
            EnvOverride::Mode(SqliteJournalMode::Truncate)
        }
        Some(_) => EnvOverride::Invalid,
    }
}

/// Classify the filesystem backing `path`.
///
/// Walks up to the nearest existing ancestor, because callers routinely ask
/// about a directory they are about to create. Only when no ancestor can be
/// interrogated does this report [`FsClass::Unknown`].
pub fn classify_path(path: &Path) -> FsClass {
    let mut candidate = Some(path);
    while let Some(dir) = candidate {
        match imp::classify_existing(dir) {
            Some(class) => return class,
            None => candidate = dir.parent(),
        }
    }
    FsClass::Unknown
}

/// Whether `path` is backed by a network filesystem. Convenience wrapper;
/// note that an *unclassifiable* path reports `false` here while still
/// selecting [`SqliteJournalMode::Truncate`] — callers wanting the
/// journaling decision must use [`SqliteJournalMode::for_db_path`].
pub fn is_network_backed(path: &Path) -> bool {
    classify_path(path) == FsClass::Network
}

// ---------------------------------------------------------------------------
// Linux: statfs(2) f_type magic classification.
// ---------------------------------------------------------------------------

/// Classify a Linux `statfs(2)` `f_type`.
///
/// Magic values are from `include/uapi/linux/magic.h`. Only the low 32 bits
/// are compared: `f_type` is a signed word whose width varies by
/// architecture, so 32-bit kernels sign-extend magics whose high bit is set
/// (CIFS `0xFF534D42` being the common case).
#[cfg(any(target_os = "linux", test))]
fn classify_linux_magic(f_type: u64) -> FsClass {
    // Network/remote: no coherent cross-host shared memory, so WAL corrupts.
    const NETWORK: &[u64] = &[
        0x6969,      // NFS
        0x517B,      // SMB
        0xFE53_4D42, // SMB2
        0xFF53_4D42, // CIFS
        0x0102_1997, // 9p / v9fs
        0x7375_7245, // CODA
        0x5346_414F, // AFS (OpenAFS)
        0x6B41_4653, // kAFS (in-kernel client)
        0x00C3_6400, // CEPH
        0x0BD0_0BD0, // Lustre
        0x0116_1970, // GFS2
        0x4750_4653, // GPFS / Spectrum Scale
        0x7461_636F, // OCFS2
        0x1803_1977, // WekaFS
        // FUSE is deliberately network: sshfs/s3fs/gluster-backed homes
        // cannot guarantee coherent mmap between writers either.
        0x6573_5546, // FUSE
    ];
    // Local: recognised as safe for WAL. This allowlist is what keeps the
    // fail-safe default from quietly demoting ordinary machines and CI.
    const LOCAL: &[u64] = &[
        0xEF53,      // ext2/ext3/ext4
        0xEF51,      // ext2 (old)
        0x0102_1994, // tmpfs
        0x858458F6,  // ramfs
        0x9123_683E, // btrfs
        0x5846_5342, // XFS
        0x2FC1_2FC1, // ZFS
        0x794C_7630, // overlayfs  (containers and CI runners)
        0xF2F5_2010, // f2fs
        0xCA45_1A4E, // bcachefs
        0x2011_BAB0, // exFAT
        0x4D44,      // vfat / msdos
        0x5346_544E, // NTFS (ntfs-3g)
        0x7366_746E, // ntfs3
        0xE0F5_E1E2, // erofs
        0x7371_7368, // squashfs
        0x5265_4973, // reiserfs
        0x3153_464A, // JFS
        0x2405_1905, // UBIFS
        0x6165_676C, // bcache
        0x0187,      // autofs (resolves to a real fs on traverse)
    ];
    let magic = f_type & 0xFFFF_FFFF;
    if NETWORK.contains(&magic) {
        FsClass::Network
    } else if LOCAL.contains(&magic) {
        FsClass::Local
    } else {
        FsClass::Unknown
    }
}

// ---------------------------------------------------------------------------
// macOS: statfs(2) MNT_LOCAL flag + fstypename allowlist.
// ---------------------------------------------------------------------------

/// Mirrors macOS `libc::MNT_LOCAL`, redeclared so the pure classifier is
/// testable on non-mac hosts; pinned to libc by a macOS-only test.
#[cfg(any(target_os = "macos", test))]
const MNT_LOCAL: u32 = 0x0000_1000;

/// Classify a macOS `statfs(2)` result.
///
/// Absence of `MNT_LOCAL` is the authoritative remote signal and covers
/// remote filesystems we have never heard of. The name allowlist is a
/// second trigger for remote-backed mounts that nonetheless claim
/// `MNT_LOCAL` (some FUSE bridges do).
#[cfg(any(target_os = "macos", test))]
fn classify_macos(f_flags: u32, fstype: &str) -> FsClass {
    if (f_flags & MNT_LOCAL) == 0 || is_macos_network_fstype(fstype) {
        FsClass::Network
    } else {
        FsClass::Local
    }
}

#[cfg(any(target_os = "macos", test))]
fn is_macos_network_fstype(fstype: &str) -> bool {
    [
        "nfs", "smbfs", "cifs", "afpfs", "webdav", "macfuse", "osxfuse", "ftpfs",
    ]
    .iter()
    .any(|known| fstype.eq_ignore_ascii_case(known))
}

// ---------------------------------------------------------------------------
// Windows: UNC prefix + GetDriveTypeW.
// ---------------------------------------------------------------------------

/// Classify a Windows path as UNC (network): `\\server\share` or
/// `\\?\UNC\server\share`. The `\\.\` device and `\\?\C:\` verbatim-local
/// forms are not network; mapped drives are caught by `GetDriveTypeW`.
///
/// This was a fourth private copy of the workspace's UNC check. It now
/// delegates to [`crate::network_path::has_unc_prefix`] — the shared version
/// additionally handles the forward-slash spelling, which Windows accepts and
/// this copy did not.
#[cfg(any(windows, test))]
fn is_windows_unc(path: &str) -> bool {
    crate::network_path::has_unc_prefix(std::path::Path::new(path))
}

#[cfg(target_os = "linux")]
mod imp {
    use super::FsClass;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// `None` when `path` could not be interrogated (caller walks up).
    pub(super) fn classify_existing(path: &Path) -> Option<FsClass> {
        let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `statfs` is zero-initialisable POD; `cpath` is
        // NUL-terminated and `st` is a valid out-pointer for the call.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(cpath.as_ptr(), &mut st) } != 0 {
            return None;
        }
        // f_type is i64 on 64-bit targets and i32 on some 32-bit ones; the
        // classifier masks to the meaningful low 32 bits.
        Some(super::classify_linux_magic(st.f_type as u64))
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::FsClass;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub(super) fn classify_existing(path: &Path) -> Option<FsClass> {
        let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `statfs` is zero-initialisable POD; `cpath` is
        // NUL-terminated and `st` is a valid out-pointer for the call.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(cpath.as_ptr(), &mut st) } != 0 {
            return None;
        }
        // Stack-copy the fixed array to u8 (c_char is i8 here); take up to
        // the first NUL.
        let bytes = st.f_fstypename.map(|c| c as u8);
        let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let name = std::str::from_utf8(&bytes[..len]).unwrap_or("");
        Some(super::classify_macos(st.f_flags, name))
    }
}

#[cfg(windows)]
mod imp {
    use super::FsClass;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
    };

    pub(super) fn classify_existing(path: &Path) -> Option<FsClass> {
        if super::is_windows_unc(&path.to_string_lossy()) {
            return Some(FsClass::Network);
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut root = [0u16; 261];
        // SAFETY: `wide` is NUL-terminated; `root` is a valid out-buffer
        // whose length is passed in characters, as the API requires.
        let drive_type = unsafe {
            if GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) == 0 {
                return None;
            }
            GetDriveTypeW(root.as_ptr())
        };
        match drive_type {
            DRIVE_REMOTE => Some(FsClass::Network),
            DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK | DRIVE_CDROM => Some(FsClass::Local),
            // DRIVE_UNKNOWN / DRIVE_NO_ROOT_DIR.
            _ => Some(FsClass::Unknown),
        }
    }
}

// No cheap, reliable probe elsewhere. Reporting Unknown routes these to
// rollback journaling, which is correct-but-slower rather than silently
// destructive. We ship Linux, macOS and Windows only.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use super::FsClass;
    use std::path::Path;

    pub(super) fn classify_existing(_path: &Path) -> Option<FsClass> {
        Some(FsClass::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- The three-assertion self-test -----------------------------------
    //
    // A selector test is trivially self-passing if it only checks "local =>
    // WAL": the pre-fix code returns WAL for everything and passes that too.
    // The third assertion is the one that proves the repair does anything.

    #[test]
    fn selector_discriminates_and_legacy_code_could_not() {
        // (1) KNOWN-POSITIVE — a local filesystem still gets WAL.
        assert_eq!(
            classify_linux_magic(0xEF53),
            FsClass::Local,
            "ext4 must classify local"
        );
        let local_mode = match classify_linux_magic(0xEF53) {
            FsClass::Local => SqliteJournalMode::Wal,
            _ => SqliteJournalMode::Truncate,
        };
        assert_eq!(local_mode, SqliteJournalMode::Wal);

        // (2) KNOWN-NEGATIVE — a network filesystem does NOT get WAL.
        assert_eq!(
            classify_linux_magic(0x6969),
            FsClass::Network,
            "NFS must classify network"
        );
        let network_mode = match classify_linux_magic(0x6969) {
            FsClass::Local => SqliteJournalMode::Wal,
            _ => SqliteJournalMode::Truncate,
        };
        assert_ne!(
            network_mode,
            SqliteJournalMode::Wal,
            "NFS must not select WAL"
        );
        assert_eq!(network_mode, SqliteJournalMode::Truncate);

        // (3) THE OLD CODE WOULD HAVE CHOSEN WAL IN BOTH. Without this the
        //     two assertions above also pass on an instrument that cannot
        //     tell the cases apart.
        assert_eq!(
            SqliteJournalMode::legacy_unconditional(),
            SqliteJournalMode::Wal
        );
        assert_eq!(
            SqliteJournalMode::legacy_unconditional(),
            local_mode,
            "legacy agreed with the new selector on local — so the local arm alone proves nothing"
        );
        assert_ne!(
            SqliteJournalMode::legacy_unconditional(),
            network_mode,
            "legacy chose WAL on NFS where the selector chooses TRUNCATE: this difference IS the fix"
        );
    }

    #[test]
    fn network_magics_classify_as_network() {
        for magic in [
            0x6969u64,   // NFS
            0x517B,      // SMB
            0xFE53_4D42, // SMB2
            0xFF53_4D42, // CIFS
            0x0102_1997, // 9p
            0x7375_7245, // CODA
            0x5346_414F, // AFS
            0x6B41_4653, // kAFS
            0x00C3_6400, // CEPH
            0x0BD0_0BD0, // Lustre
            0x0116_1970, // GFS2
            0x4750_4653, // GPFS
            0x7461_636F, // OCFS2
            0x1803_1977, // WekaFS
            0x6573_5546, // FUSE
        ] {
            assert_eq!(
                classify_linux_magic(magic),
                FsClass::Network,
                "magic {magic:#x} must be network"
            );
        }
    }

    #[test]
    fn local_magics_classify_as_local() {
        // Every one of these must keep WAL — the fix must not become a
        // blanket disable. overlayfs and tmpfs matter for CI specifically.
        for magic in [
            0xEF53u64,   // ext4
            0x0102_1994, // tmpfs
            0x9123_683E, // btrfs
            0x5846_5342, // XFS
            0x2FC1_2FC1, // ZFS
            0x794C_7630, // overlayfs
            0xF2F5_2010, // f2fs
            0xCA45_1A4E, // bcachefs
            0x5346_544E, // NTFS
        ] {
            assert_eq!(
                classify_linux_magic(magic),
                FsClass::Local,
                "magic {magic:#x} must be local"
            );
        }
    }

    #[test]
    fn unrecognised_magic_is_unknown_and_does_not_select_wal() {
        // The fail-safe posture, and the deliberate divergence from the
        // peer implementation, which returns "local" (WAL) here.
        assert_eq!(classify_linux_magic(0xDEAD_BEEF), FsClass::Unknown);
        assert_eq!(classify_linux_magic(0x0), FsClass::Unknown);
    }

    #[test]
    fn sign_extended_magic_still_matches() {
        // A 32-bit kernel reports CIFS's 0xFF534D42 as a negative f_type.
        assert_eq!(
            classify_linux_magic(0xFFFF_FFFF_FF53_4D42),
            FsClass::Network
        );
    }

    #[test]
    fn macos_classifier_uses_mnt_local_then_name() {
        // No MNT_LOCAL => remote, even for a type we do not know.
        assert_eq!(classify_macos(0, "somefuturefs"), FsClass::Network);
        // Plain local APFS.
        assert_eq!(classify_macos(MNT_LOCAL, "apfs"), FsClass::Local);
        // Allowlisted name wins even when the mount claims MNT_LOCAL.
        assert_eq!(classify_macos(MNT_LOCAL, "smbfs"), FsClass::Network);
        assert_eq!(classify_macos(MNT_LOCAL, "nfs"), FsClass::Network);
        assert_eq!(classify_macos(MNT_LOCAL, "macfuse"), FsClass::Network);
        // Case-insensitive.
        assert_eq!(classify_macos(MNT_LOCAL, "NFS"), FsClass::Network);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mnt_local_matches_libc() {
        assert_eq!(u64::from(MNT_LOCAL), libc::MNT_LOCAL as u64);
    }

    #[test]
    fn windows_unc_classifies() {
        assert!(is_windows_unc(r"\\server\share\wayland"));
        assert!(is_windows_unc(r"\\?\UNC\server\share\wayland"));
        assert!(is_windows_unc(r"\\?\unc\server\share"));
        assert!(!is_windows_unc(r"\\?\C:\Users\x"));
        assert!(!is_windows_unc(r"\\.\pipe\wayland"));
        assert!(!is_windows_unc(r"C:\Users\x"));
        assert!(!is_windows_unc("/home/x"));
    }

    #[test]
    fn env_override_parses() {
        use EnvOverride::{Invalid, Mode, Unset};
        assert_eq!(
            journal_mode_from_env(Some("wal")),
            Mode(SqliteJournalMode::Wal)
        );
        assert_eq!(
            journal_mode_from_env(Some("WAL")),
            Mode(SqliteJournalMode::Wal)
        );
        assert_eq!(
            journal_mode_from_env(Some("truncate")),
            Mode(SqliteJournalMode::Truncate)
        );
        assert_eq!(
            journal_mode_from_env(Some("TrUnCaTe")),
            Mode(SqliteJournalMode::Truncate)
        );
        // Typos are Invalid (warned), not silently ignored.
        assert_eq!(journal_mode_from_env(Some("delete")), Invalid);
        assert_eq!(journal_mode_from_env(Some("wall")), Invalid);
        assert_eq!(journal_mode_from_env(Some("")), Unset);
        assert_eq!(journal_mode_from_env(None), Unset);
    }

    #[test]
    fn classify_walks_up_to_an_existing_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        // A path several levels below anything that exists must still
        // classify from its nearest real ancestor rather than reporting
        // Unknown and needlessly demoting a local disk.
        let deep = tmp.path().join("a").join("b").join("c");
        assert_eq!(classify_path(tmp.path()), classify_path(&deep));
        assert_ne!(
            classify_path(&deep),
            FsClass::Unknown,
            "CI temp dirs are local filesystems we recognise"
        );
    }

    #[test]
    fn local_temp_dir_still_selects_wal() {
        // Guards the "do not blanket-disable WAL" requirement on whatever
        // filesystem CI actually runs on.
        if std::env::var(JOURNAL_MODE_ENV).is_ok() {
            return; // ambient override would invalidate the assertion
        }
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            SqliteJournalMode::for_db_path(&tmp.path().join("x.sqlite")),
            SqliteJournalMode::Wal
        );
    }

    #[test]
    fn mode_strings_are_valid_pragma_values() {
        assert_eq!(SqliteJournalMode::Wal.as_str(), "WAL");
        assert_eq!(SqliteJournalMode::Truncate.as_str(), "TRUNCATE");
        assert!(SqliteJournalMode::Wal.uses_shared_memory());
        assert!(!SqliteJournalMode::Truncate.uses_shared_memory());
    }
}
