//! Fingerprint-bound workspace trust stored outside repositories.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use wcore_types::workspace_trust::{
    AuthoritySource, EffectiveWorkspaceTrust, WorkspaceTrustInput, resolve_workspace_trust,
};

const STORE_SCHEMA: u32 = 1;
const MAX_EXECUTABLE_FILES: usize = 512;
const MAX_EXECUTABLE_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_EXECUTABLE_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const GIT_ROOT_DEPTH_CAP: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFingerprint {
    pub root: PathBuf,
    pub digest: String,
}

#[derive(Debug, Error)]
pub enum WorkspaceTrustError {
    #[error("workspace trust I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("workspace trust store is invalid: {0}")]
    InvalidStore(#[from] serde_json::Error),
    #[error("workspace trust store schema {0} is not supported")]
    UnsupportedSchema(u32),
    #[error("workspace root is not a directory: {0}")]
    InvalidRoot(PathBuf),
    #[error("executable repository content contains a symlink: {0}")]
    ExecutableSymlink(PathBuf),
    #[error("executable repository file exceeds {MAX_EXECUTABLE_FILE_BYTES} bytes: {0}")]
    FileTooLarge(PathBuf),
    #[error("executable repository surface exceeds the fingerprint limits")]
    SurfaceTooLarge,
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceTrustStoreFile {
    #[serde(default = "store_schema")]
    schema: u32,
    #[serde(default)]
    entries: BTreeMap<String, String>,
}

const fn store_schema() -> u32 {
    STORE_SCHEMA
}

#[derive(Debug, Clone)]
pub struct WorkspaceTrustStore {
    path: PathBuf,
}

impl WorkspaceTrustStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn for_current_home() -> Self {
        Self::new(crate::config::wayland_config_dir().join("workspace-trust.json"))
    }

    pub fn grant(&self, workspace: &Path) -> Result<WorkspaceFingerprint, WorkspaceTrustError> {
        let fingerprint = fingerprint_workspace(workspace)?;
        let mut file = self.load()?;
        file.entries.insert(
            fingerprint.root.to_string_lossy().into_owned(),
            fingerprint.digest.clone(),
        );
        self.save(&file)?;
        Ok(fingerprint)
    }

    pub fn revoke(&self, workspace: &Path) -> Result<bool, WorkspaceTrustError> {
        let root = canonical_workspace_root(workspace)?;
        let mut file = self.load()?;
        let removed = file
            .entries
            .remove(root.to_string_lossy().as_ref())
            .is_some();
        if removed {
            self.save(&file)?;
        }
        Ok(removed)
    }

    pub fn resolve(
        &self,
        workspace: &Path,
        local_session_grant: bool,
        strict_sources: impl IntoIterator<Item = AuthoritySource>,
    ) -> Result<EffectiveWorkspaceTrust, WorkspaceTrustError> {
        let fingerprint = fingerprint_workspace(workspace)?;
        let file = self.load()?;
        let stored = file
            .entries
            .get(fingerprint.root.to_string_lossy().as_ref())
            .is_some_and(|digest| digest == &fingerprint.digest);

        let mut inputs = Vec::new();
        if stored {
            inputs.push(WorkspaceTrustInput::grant(AuthoritySource::User));
        }
        if local_session_grant {
            inputs.push(WorkspaceTrustInput::grant(AuthoritySource::LocalSession));
        }
        inputs.extend(strict_sources.into_iter().map(WorkspaceTrustInput::narrow));
        Ok(resolve_workspace_trust(fingerprint.digest, inputs))
    }

    fn load(&self) -> Result<WorkspaceTrustStoreFile, WorkspaceTrustError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(WorkspaceTrustStoreFile {
                    schema: STORE_SCHEMA,
                    entries: BTreeMap::new(),
                });
            }
            Err(error) => return Err(error.into()),
        };
        let file: WorkspaceTrustStoreFile = serde_json::from_slice(&bytes)?;
        if file.schema != STORE_SCHEMA {
            return Err(WorkspaceTrustError::UnsupportedSchema(file.schema));
        }
        Ok(file)
    }

    fn save(&self, file: &WorkspaceTrustStoreFile) -> Result<(), WorkspaceTrustError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut bytes = serde_json::to_vec_pretty(file)?;
        bytes.push(b'\n');
        crate::atomic_write(&self.path, &bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

pub fn fingerprint_workspace(
    workspace: &Path,
) -> Result<WorkspaceFingerprint, WorkspaceTrustError> {
    let root = canonical_workspace_root(workspace)?;
    let skill_ancestors = executable_project_ancestors(&root);
    let scope_boundary = skill_ancestors
        .last()
        .cloned()
        .unwrap_or_else(|| root.clone());
    let mut candidates = Vec::new();
    for path in [
        root.join(".wayland-core.toml"),
        root.join(".wayland-core").join("config.toml"),
    ] {
        if path.exists() {
            candidates.push(path);
        }
    }
    for ancestor in skill_ancestors {
        for directory in [
            ancestor.join(".wayland-core").join("skills"),
            ancestor.join(".wayland-core").join("commands"),
        ] {
            collect_regular_files(&scope_boundary, &directory, &mut candidates)?;
        }
    }
    candidates.sort();
    candidates.dedup();
    if candidates.len() > MAX_EXECUTABLE_FILES {
        return Err(WorkspaceTrustError::SurfaceTooLarge);
    }

    let mut hasher = Sha256::new();
    hasher.update(b"wayland-workspace-executable-surface-v1\0");
    let mut total = 0_u64;
    for path in candidates {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(WorkspaceTrustError::ExecutableSymlink(path));
        }
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() > MAX_EXECUTABLE_FILE_BYTES {
            return Err(WorkspaceTrustError::FileTooLarge(path));
        }
        total = total
            .checked_add(metadata.len())
            .ok_or(WorkspaceTrustError::SurfaceTooLarge)?;
        if total > MAX_EXECUTABLE_TOTAL_BYTES {
            return Err(WorkspaceTrustError::SurfaceTooLarge);
        }
        let relative = path
            .strip_prefix(&scope_boundary)
            .map_err(|_| WorkspaceTrustError::InvalidRoot(root.clone()))?;
        let bytes = fs::read(&path)?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
        hasher.update([0]);
    }

    Ok(WorkspaceFingerprint {
        root,
        digest: format!("{:x}", hasher.finalize()),
    })
}

/// Current directory through the executable project boundary: nearest git
/// root, otherwise HOME when the workspace is below it, otherwise filesystem
/// root. Both trust fingerprinting and skill discovery use this function so
/// executable ancestor content cannot escape the approved surface.
pub fn executable_project_ancestors(root: &Path) -> Vec<PathBuf> {
    let boundary = nearest_workspace_git_root(root)
        .or_else(|| dirs::home_dir().filter(|home| root.starts_with(home)))
        .or_else(|| root.ancestors().last().map(Path::to_path_buf))
        .unwrap_or_else(|| root.to_path_buf());

    root.ancestors()
        .take_while(|ancestor| ancestor.starts_with(&boundary))
        .map(Path::to_path_buf)
        .collect()
}

pub fn nearest_workspace_git_root(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .take(GIT_ROOT_DEPTH_CAP)
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

fn canonical_workspace_root(workspace: &Path) -> Result<PathBuf, WorkspaceTrustError> {
    let root = fs::canonicalize(workspace)?;
    if !root.is_dir() {
        return Err(WorkspaceTrustError::InvalidRoot(root));
    }
    Ok(root)
}

fn collect_regular_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<PathBuf>,
) -> Result<(), WorkspaceTrustError> {
    if !directory.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(directory)?;
    if metadata.file_type().is_symlink() {
        return Err(WorkspaceTrustError::ExecutableSymlink(
            directory.to_path_buf(),
        ));
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(WorkspaceTrustError::ExecutableSymlink(path));
        }
        if metadata.is_dir() {
            collect_regular_files(root, &path, output)?;
        } else if metadata.is_file() {
            if !path.starts_with(root) {
                return Err(WorkspaceTrustError::InvalidRoot(path));
            }
            output.push(path);
            if output.len() > MAX_EXECUTABLE_FILES {
                return Err(WorkspaceTrustError::SurfaceTooLarge);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_types::workspace_trust::WorkspaceTrustLevel;

    #[test]
    fn trust_is_bound_to_executable_surface_fingerprint() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join(".wayland-core/skills/x")).unwrap();
        let skill = workspace.path().join(".wayland-core/skills/x/SKILL.md");
        fs::write(&skill, "safe v1").unwrap();
        let store = WorkspaceTrustStore::new(state.path().join("trust.json"));

        store.grant(workspace.path()).unwrap();
        assert_eq!(
            store.resolve(workspace.path(), false, []).unwrap().level(),
            WorkspaceTrustLevel::Trusted
        );

        fs::write(&skill, "changed executable surface").unwrap();
        let changed = store.resolve(workspace.path(), false, []).unwrap();
        assert_eq!(changed.level(), WorkspaceTrustLevel::Untrusted);
        assert_eq!(changed.source(), AuthoritySource::Default);
    }

    #[test]
    fn nested_workspace_trust_covers_executable_skills_loaded_from_ancestors() {
        let repository = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        fs::create_dir(repository.path().join(".git")).unwrap();
        fs::create_dir_all(repository.path().join(".wayland-core/skills/root-skill")).unwrap();
        let skill = repository
            .path()
            .join(".wayland-core/skills/root-skill/SKILL.md");
        fs::write(&skill, "executable ancestor v1").unwrap();
        let nested = repository.path().join("crates/component");
        fs::create_dir_all(&nested).unwrap();
        let store = WorkspaceTrustStore::new(state.path().join("trust.json"));

        store.grant(&nested).unwrap();
        assert!(store.resolve(&nested, false, []).unwrap().is_trusted());

        fs::write(&skill, "changed executable ancestor").unwrap();
        assert!(!store.resolve(&nested, false, []).unwrap().is_trusted());
    }

    #[test]
    fn remote_and_managed_constraints_override_a_current_store_grant() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = WorkspaceTrustStore::new(state.path().join("trust.json"));
        store.grant(workspace.path()).unwrap();

        for source in [AuthoritySource::Remote, AuthoritySource::Managed] {
            let decision = store.resolve(workspace.path(), false, [source]).unwrap();
            assert!(!decision.is_trusted());
            assert_eq!(decision.source(), source);
        }
    }

    #[test]
    fn unsupported_store_schema_fails_closed() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store_path = state.path().join("trust.json");
        fs::write(&store_path, r#"{"schema":99,"entries":{}}"#).unwrap();
        let store = WorkspaceTrustStore::new(store_path);

        assert!(matches!(
            store.resolve(workspace.path(), false, []),
            Err(WorkspaceTrustError::UnsupportedSchema(99))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn executable_surface_symlinks_fail_closed() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join(".wayland-core/skills")).unwrap();
        symlink(
            workspace.path().join("outside"),
            workspace.path().join(".wayland-core/skills/escape"),
        )
        .unwrap();
        assert!(matches!(
            fingerprint_workspace(workspace.path()),
            Err(WorkspaceTrustError::ExecutableSymlink(_))
        ));
    }
}
