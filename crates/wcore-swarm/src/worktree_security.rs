//! Filesystem and identifier guards for Swarm worktree creation.

use std::path::Path;

use wcore_config::profile::validate_profile_name;
use wcore_sandbox::DirectoryAuthority as SandboxDirectoryAuthority;
use wcore_sandbox::DirectoryAuthorityIdentity as SandboxDirectoryAuthorityIdentity;

use crate::error::{Result, SwarmError};

use super::normalized_root;

#[derive(Clone, Debug)]
pub(super) struct DirectoryAuthority(SandboxDirectoryAuthority);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DirectoryAuthorityIdentity(SandboxDirectoryAuthorityIdentity);

impl DirectoryAuthority {
    pub(super) fn from_sandbox(authority: SandboxDirectoryAuthority) -> Self {
        Self(authority)
    }

    pub(super) fn to_sandbox(&self) -> SandboxDirectoryAuthority {
        self.0.clone()
    }

    pub(super) fn identity_token(&self) -> DirectoryAuthorityIdentity {
        DirectoryAuthorityIdentity(self.0.identity_token())
    }

    pub(super) fn open(path: &Path) -> Result<Self> {
        SandboxDirectoryAuthority::open(path)
            .map(Self)
            .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))
    }

    /// Acquire an identity-witness authority that requests no delete access.
    /// The rationale lives at `wcore_sandbox::DirectoryAuthority::open_observational`.
    pub(super) fn open_observational(path: &Path) -> Result<Self> {
        SandboxDirectoryAuthority::open_observational(path)
            .map(Self)
            .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))
    }

    pub(super) fn validate_path(&self, path: &Path) -> Result<()> {
        self.0
            .validate_path(path)
            .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))
    }

    /// Open an EXISTING regular-file advisory-lock target beneath this retained
    /// directory. The rationale lives at
    /// `wcore_sandbox::DirectoryAuthority::open_child_lock_file`.
    pub(super) fn open_child_lock_file(
        &self,
        name: &str,
    ) -> Result<wcore_sandbox::DirectoryHandleLoan> {
        self.0.open_child_lock_file(name).map_err(lock_file_error)
    }

    /// Open-or-create a regular-file advisory-lock target beneath this retained
    /// directory. The rationale lives at
    /// `wcore_sandbox::DirectoryAuthority::open_or_create_child_lock_file`.
    pub(super) fn open_or_create_child_lock_file(
        &self,
        name: &str,
    ) -> Result<wcore_sandbox::DirectoryHandleLoan> {
        // NOT `lock_file_error`. That helper deliberately preserves the raw
        // `io::Error` so the PROBE path above can tell "no lock file, so nobody
        // holds the lease" from a real failure — see its doc comment. This is
        // the ACQUIRE path, where nobody handles `NotFound` and there is no
        // benign reading of it: open-or-create only fails that way when the
        // directory that should contain the lock is gone.
        //
        // wayland#1025. Sharing the helper let that ENOENT escape as a bare
        // `SwarmError::Io`, which renders as `io: No such file or directory
        // (os error 2)` with no path and no probe. It bypassed every named
        // site in this crate because it is minted in the sandbox layer, which
        // is why the macOS admission flake stayed unmeasurable across several
        // release trains: its CI output contained nothing to look at.
        self.0
            .open_or_create_child_lock_file(name)
            .map_err(|error| match error {
                wcore_sandbox::SandboxError::Io(error) => SwarmError::WorktreeIo(format!(
                    "open-or-create of the advisory lock file {name:?} \
                     (its parent directory is missing or unusable): {error}"
                )),
                other => SwarmError::DispatchAdmission(other.to_string()),
            })
    }

    pub(super) fn has_outstanding_loans(&self) -> bool {
        self.0.has_outstanding_handle_loans()
    }

    pub(super) fn open_or_create_child_directory(&self, name: &str) -> Result<Self> {
        self.0
            .open_or_create_child_directory(name)
            .map(Self)
            .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))
    }

    pub(super) fn remove_open_dir_all(self) -> std::result::Result<(), (SwarmError, Self)> {
        self.0.remove_open_dir_all().map_err(|boxed| {
            let (error, authority) = *boxed;
            (SwarmError::WorktreeIo(error.to_string()), Self(authority))
        })
    }

    /// Rename the exact held Windows directory object beneath a retained
    /// destination parent.
    ///
    /// DO NOT DELETE THIS AS DEAD CODE. Until 20-75 it had no live caller for
    /// one reason only: the primitive beneath it — the crate's sole
    /// handle-relative rename in `wcore-sandbox` — had NEVER worked on Windows
    /// (`SetFileInformationByHandle` + `FileRenameInfo` rejects a HANDLE in
    /// `RootDirectory` with os error 87). Its dead-code appearance was a SYMPTOM
    /// of that defect, not a reason to remove it. 20-75 repaired the primitive
    /// against `NtSetInformationFile`; this is the swarm-side API that repair
    /// restores, and deleting it would destroy the surface the fix exists to
    /// make usable.
    ///
    /// 20A-02 measured the follow-through and it is NOT there: at this SHA the
    /// repaired primitive still has no swarm-side caller, so `-D warnings` on
    /// the Windows leg turns this into a hard build failure. `expect` rather
    /// than `allow` is deliberate — `unfulfilled_lint_expectation` fires the
    /// moment a caller lands, forcing this attribute to be deleted then instead
    /// of silently outliving the gap. Recorded as a finding; wiring the caller
    /// is a behaviour change and belongs to the plan that needs the surface.
    #[cfg(windows)]
    #[expect(
        dead_code,
        reason = "20-75 restored this Windows-only API; its caller was never wired. Remove this attribute with the caller."
    )]
    pub(super) fn rename_into(
        &self,
        destination_parent: &Self,
        child_name: &str,
        replace: bool,
    ) -> Result<()> {
        self.0
            .rename_into(&destination_parent.0, child_name, replace)
            .map_err(|error| SwarmError::WorktreeIo(error.to_string()))
    }
}

/// Preserve the OS error kind when a lock-file open fails, so a caller probing
/// an absent lock file can still distinguish "not found" (nobody holds it) from
/// a real failure. Every other sandbox refusal keeps the admission shape the
/// sibling accessors use.
///
/// **Only the open-EXISTING probe may use this.** Preserving a bare
/// `SwarmError::Io` is safe exactly where a caller matches on
/// `ErrorKind::NotFound`; anywhere else it escapes as `io: <errno>` with no
/// path and no probe, defeating the naming discipline the rest of this crate
/// maintains through `io_at`. wayland#1025 was that leak on the acquire path.
fn lock_file_error(error: wcore_sandbox::SandboxError) -> SwarmError {
    match error {
        wcore_sandbox::SandboxError::Io(error) => SwarmError::Io(error),
        other => SwarmError::DispatchAdmission(other.to_string()),
    }
}

pub(super) fn reject_option_like_ref(kind: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.starts_with('-') {
        return Err(SwarmError::WorktreeIo(format!(
            "refused invalid {kind} ref {value:?}"
        )));
    }
    Ok(())
}

/// Validate a fully-qualified branch ref (`refs/heads/<name>`) for the parent
/// landing target. Rejects option-like, empty, traversal, trailing-slash, and
/// metacharacter-bearing names so the ref can never be mistaken for a flag or
/// escape the branch namespace. Lives here beside the other identifier guards
/// ([`validate_worker_id`], [`reject_option_like_ref`]) so every argv-facing
/// name check shares one home.
pub(super) fn validate_target_ref(target_ref: &str) -> Result<()> {
    let Some(name) = target_ref.strip_prefix("refs/heads/") else {
        return Err(SwarmError::WorktreeIo(format!(
            "target ref {target_ref:?} must be a fully-qualified branch ref"
        )));
    };
    if name.is_empty()
        || name.starts_with('-')
        || name.contains("..")
        || name.contains("//")
        || name.ends_with('/')
        || name.bytes().any(|byte| {
            byte.is_ascii_whitespace()
                || byte == b'~'
                || byte == b'^'
                || byte == b':'
                || byte == b'?'
                || byte == b'*'
                || byte == b'['
                || byte == b'\\'
                || byte < 0x20
        })
    {
        return Err(SwarmError::WorktreeIo(format!(
            "target ref {target_ref:?} is not a safe branch name"
        )));
    }
    Ok(())
}

/// Derive a filesystem-safe slug from a fully-qualified branch ref for
/// per-ref lock and quarantine-ref naming.
pub(super) fn ref_slug(target_ref: &str) -> String {
    let mut slug = String::with_capacity(target_ref.len());
    for byte in target_ref.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.' {
            slug.push(byte as char);
        } else {
            slug.push('_');
        }
    }
    slug
}

pub(super) fn validate_worker_id(worker_id: &str) -> Result<()> {
    let mut components = Path::new(worker_id).components();
    let exactly_one_normal = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if let Err(error) = validate_profile_name(worker_id) {
        return Err(SwarmError::WorktreeIo(format!(
            "refused invalid worker id {worker_id:?}: {error}"
        )));
    }
    if !exactly_one_normal {
        return Err(SwarmError::WorktreeIo(format!(
            "refused invalid worker id {worker_id:?}: expected one safe path component"
        )));
    }
    Ok(())
}

/// Name the probe and the path in a filesystem error.
///
/// `SwarmError::Io` wraps a bare `std::io::Error`, so any failure in the
/// workspace-authority path reaches the operator as `io: No such file or
/// directory (os error 2)` and nothing else — not which path, not which probe.
/// That is exactly what the macOS leg of
/// `independent_cli_processes_cannot_overbook_shared_capacity` reduced to: the
/// losing process was refused, but for an unnamed filesystem reason instead of
/// the capacity verdict, and the failure carried nothing to look at. A subsystem
/// whose whole job is filesystem authority has to say which object it failed on.
pub(super) fn io_at(probe: &str, path: &Path, error: std::io::Error) -> SwarmError {
    SwarmError::WorktreeIo(format!("{probe} {}: {error}", path.display()))
}

pub(super) fn ensure_real_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => validate_real_directory(path, &metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            validate_real_directory(path, &std::fs::symlink_metadata(path)?)
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn ensure_unchanged_real_directory(path: &Path, parent: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| io_at("stat of the retained worktree root", path, error))?;
    validate_real_directory(path, &metadata)?;
    // Re-derive through the SHARED helper, not a bare canonicalize: `path` and
    // `parent` are the stored roots, which are produced by that same helper.
    // Both operands of a root comparison must come from one definition or this
    // check is unfalsifiable on Windows — a bare canonicalize yields a verbatim
    // `\\?\C:\...` path that never equals a plain stored root, so the swap
    // detection below would refuse every caller instead of only a real swap.
    let canonical = normalized_root(path)?;
    if canonical != path || canonical.parent() != Some(parent) {
        return Err(SwarmError::WorktreeIo(format!(
            "refused changed worktree root: {}",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn ensure_absent_destination(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(SwarmError::WorktreeIo(format!(
            "refused existing or linked worker destination: {}",
            path.display()
        ))),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn is_real_directory_entry(path: &Path) -> Result<bool> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| io_at("stat of the workspace entry", path, error))?;
    if is_symlink_or_reparse(&metadata) {
        return Err(SwarmError::WorktreeIo(format!(
            "refused linked cleanup entry: {}",
            path.display()
        )));
    }
    Ok(metadata.is_dir())
}

fn validate_real_directory(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    if !metadata.is_dir() || is_symlink_or_reparse(metadata) {
        return Err(SwarmError::WorktreeIo(format!(
            "refused non-directory or linked worktree root: {}",
            path.display()
        )));
    }
    Ok(())
}

fn is_symlink_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

pub(super) fn make_guard_dir_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub(super) fn write_empty_private_config(path: &Path) -> std::io::Result<()> {
    std::fs::File::create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod lock_file_error_tests {
    use super::*;

    /// wayland#1025. The acquire path must never hand back a bare
    /// `SwarmError::Io`. That renders as `io: No such file or directory
    /// (os error 2)` with no path and no probe, and because it is minted in
    /// the sandbox layer it bypasses every `io_at` site in this crate — which
    /// is exactly why the macOS admission flake survived several release
    /// trains with nothing in its CI output to look at.
    #[test]
    fn acquire_on_a_vanished_directory_names_itself_instead_of_leaking_io() {
        let dir = tempfile::tempdir().expect("tempdir");
        let authority = DirectoryAuthority::open(dir.path()).expect("open authority");
        // Remove the directory out from under the retained authority: this is
        // the shape the race produces, a lock target whose parent is gone.
        std::fs::remove_dir_all(dir.path()).expect("remove dir");

        let error = authority
            .open_or_create_child_lock_file("some.lock")
            .expect_err("acquiring a lock under a removed directory must fail");

        assert!(
            !matches!(error, SwarmError::Io(_)),
            "acquire must not leak a bare Io — that is the whole defect: {error:?}"
        );
        let rendered = error.to_string();
        assert!(
            rendered.contains("some.lock"),
            "the error must name the lock file it could not acquire: {rendered}"
        );
        assert!(
            rendered.contains("parent directory"),
            "the error must say why, not merely which: {rendered}"
        );
    }

    /// THE CONTROL, and the reason the two paths cannot simply share a helper.
    /// `transaction_is_active` probes for an ABSENT lease file and reads
    /// `Io(NotFound)` as "nobody holds the lease". If the fix above were
    /// applied to the probe too, every transaction would read as inactive and
    /// capacity accounting would silently stop counting active reservations —
    /// a far worse bug than the one being fixed.
    #[test]
    fn the_probe_path_still_reports_notfound_as_a_bare_io() {
        let dir = tempfile::tempdir().expect("tempdir");
        let authority = DirectoryAuthority::open(dir.path()).expect("open authority");

        let error = authority
            .open_child_lock_file("absent.lock")
            .expect_err("opening a non-existent lock file must fail");

        match error {
            SwarmError::Io(io) => assert_eq!(
                io.kind(),
                std::io::ErrorKind::NotFound,
                "the probe path's NotFound signal must survive"
            ),
            other => panic!(
                "probe path must keep returning a bare Io so callers can match \
                 NotFound; got {other:?}"
            ),
        }
    }
}
