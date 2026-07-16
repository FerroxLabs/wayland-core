//! Filesystem and identifier guards for Swarm worktree creation.

use std::path::Path;

use wcore_config::profile::validate_profile_name;

use crate::error::{Result, SwarmError};

pub(super) fn reject_option_like_ref(kind: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.starts_with('-') {
        return Err(SwarmError::WorktreeIo(format!(
            "refused invalid {kind} ref {value:?}"
        )));
    }
    Ok(())
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
    validate_real_directory(path, &std::fs::symlink_metadata(path)?)?;
    let canonical = std::fs::canonicalize(path)?;
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
    let metadata = std::fs::symlink_metadata(path)?;
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
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
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
