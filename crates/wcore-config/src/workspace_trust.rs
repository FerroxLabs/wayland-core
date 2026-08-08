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
    // (logical, resolved). They differ only for a symlink: `logical` is the
    // path as it appears under the workspace and names the entry in the hash,
    // `resolved` is where the bytes actually live and is what gets read.
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();
    for path in [
        root.join(".wayland-core.toml"),
        root.join(".wayland-core").join("config.toml"),
    ] {
        if path.exists() {
            candidates.push((path.clone(), path));
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
    for (logical, resolved) in candidates {
        // `metadata`, not `symlink_metadata`: a link has already been resolved
        // into `resolved` by the collector, which refuses anything that is not
        // a regular file. Reading through the link is the point — hashing the
        // link's own bytes would fingerprint a pointer, and what executes is
        // whatever the target holds.
        let metadata = fs::metadata(&resolved)?;
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() > MAX_EXECUTABLE_FILE_BYTES {
            return Err(WorkspaceTrustError::FileTooLarge(logical));
        }
        total = total
            .checked_add(metadata.len())
            .ok_or(WorkspaceTrustError::SurfaceTooLarge)?;
        if total > MAX_EXECUTABLE_TOTAL_BYTES {
            return Err(WorkspaceTrustError::SurfaceTooLarge);
        }
        let relative = logical
            .strip_prefix(&scope_boundary)
            .map_err(|_| WorkspaceTrustError::InvalidRoot(root.clone()))?;
        let bytes = fs::read(&resolved)?;
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

/// Walk the executable surface, following symlinks and recording where their
/// bytes really live.
///
/// ## Why links are followed rather than refused
///
/// This used to return `ExecutableSymlink` for any link. That made Wayland
/// Core unusable from any host that composes a workspace out of assets it owns
/// — Wayland Desktop links builtin and user skill directories into a per-chat
/// workspace, so `--trust-workspace` cleared the profile error and then the
/// fingerprint refused, with no third path.
///
/// Refusing links bought nothing that survives inspection. **This fingerprint
/// is only ever compared against an EXPLICIT trust grant.** With no grant the
/// workspace is untrusted whatever the walk returns, so following a link
/// changes nothing observable. With a grant, the user has already said this
/// workspace's executable surface is theirs — and a workspace that can run
/// skills at all can read any file the user can, so a link to
/// `~/.ssh/id_rsa` buys an attacker nothing they did not already have the
/// moment trust was granted.
///
/// What the ban DID buy, and what is preserved here, is that trust must not
/// survive a change to what actually executes. That is why `resolved` is
/// hashed by CONTENT: rewrite the target, or repoint the link at different
/// bytes, and the fingerprint moves and the grant is void. Hashing the link
/// itself would fingerprint a pointer and let the executed content drift for
/// free — the property the ban existed to protect, lost by the cheaper fix.
///
/// Fail-closed cases kept: a link that cannot be resolved (including a cycle,
/// which surfaces as an `ELOOP` error from `canonicalize`), and a target that
/// is not a regular file or directory — so a device or FIFO cannot be read
/// into the hash and cannot hang the walk.
fn collect_regular_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), WorkspaceTrustError> {
    collect_from(root, directory, directory, output, 0)
}

/// `logical_dir` is the path under the workspace; `real_dir` is where to read.
/// They diverge once a directory link has been followed.
fn collect_from(
    root: &Path,
    logical_dir: &Path,
    real_dir: &Path,
    output: &mut Vec<(PathBuf, PathBuf)>,
    depth: usize,
) -> Result<(), WorkspaceTrustError> {
    // A link cycle normally surfaces as ELOOP from `canonicalize`, but a chain
    // of distinct directories each linking one level deeper does not. Bound it.
    const MAX_LINK_DEPTH: usize = 32;
    if depth > MAX_LINK_DEPTH {
        return Err(WorkspaceTrustError::SurfaceTooLarge);
    }
    if !real_dir.exists() {
        return Ok(());
    }
    let metadata = match fs::metadata(real_dir) {
        Ok(metadata) => metadata,
        // Resolution failed: a dangling link, a cycle, a permission wall. The
        // surface cannot be established, so it cannot be certified.
        Err(_) => {
            return Err(WorkspaceTrustError::ExecutableSymlink(
                logical_dir.to_path_buf(),
            ));
        }
    };
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(real_dir)? {
        let entry = entry?;
        let real = entry.path();
        let logical = logical_dir.join(entry.file_name());

        // `metadata` follows the link; `symlink_metadata` would report the
        // link itself.
        let resolved = match fs::metadata(&real) {
            Ok(resolved) => resolved,
            Err(_) => return Err(WorkspaceTrustError::ExecutableSymlink(logical)),
        };

        if resolved.is_dir() {
            collect_from(root, &logical, &real, output, depth + 1)?;
        } else if resolved.is_file() {
            // The LOGICAL path is what must stay inside the workspace; the
            // resolved target is allowed to live elsewhere precisely because
            // its bytes, not its location, are what get hashed.
            if !logical.starts_with(root) {
                return Err(WorkspaceTrustError::InvalidRoot(logical));
            }
            output.push((logical, real));
            if output.len() > MAX_EXECUTABLE_FILES {
                return Err(WorkspaceTrustError::SurfaceTooLarge);
            }
        } else {
            // Not a regular file and not a directory: a device, socket or
            // FIFO. Reading it into the hash could block forever.
            return Err(WorkspaceTrustError::ExecutableSymlink(logical));
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

    /// A DANGLING link still fails closed. This is what remains of
    /// `executable_surface_symlinks_fail_closed`: the surface cannot be
    /// established, so it cannot be certified.
    #[cfg(unix)]
    #[test]
    fn unresolvable_symlink_fails_closed() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join(".wayland-core/skills")).unwrap();
        symlink(
            workspace.path().join("does-not-exist"),
            workspace.path().join(".wayland-core/skills/escape"),
        )
        .unwrap();
        assert!(matches!(
            fingerprint_workspace(workspace.path()),
            Err(WorkspaceTrustError::ExecutableSymlink(_))
        ));
    }

    /// A link to a FIFO fails closed. Without this, the hash walk would block
    /// forever on `fs::read`.
    #[cfg(unix)]
    #[test]
    fn symlink_to_non_regular_file_fails_closed() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().unwrap();
        let fifo = outside.path().join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(
            status.success(),
            "mkfifo must succeed or this test is vacuous"
        );

        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join(".wayland-core/skills")).unwrap();
        symlink(&fifo, workspace.path().join(".wayland-core/skills/pipe")).unwrap();
        assert!(matches!(
            fingerprint_workspace(workspace.path()),
            Err(WorkspaceTrustError::ExecutableSymlink(_))
        ));
    }

    /// POSITIVE CONTROL, and the case that unbreaks every host composing a
    /// workspace from its own assets. A linked-in skill DIRECTORY fingerprints
    /// rather than refusing.
    #[cfg(unix)]
    #[test]
    fn linked_skill_directory_fingerprints() {
        use std::os::unix::fs::symlink;

        let assets = tempfile::tempdir().unwrap();
        let skill = assets.path().join("office-cli");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), b"# office\n").unwrap();

        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join(".wayland-core/skills")).unwrap();
        symlink(
            &skill,
            workspace.path().join(".wayland-core/skills/office-cli"),
        )
        .unwrap();

        fingerprint_workspace(workspace.path())
            .expect("a resolvable linked skill directory must fingerprint");
    }

    /// THE property the old ban existed to protect, and the one a naive fix
    /// loses. Rewriting the LINK TARGET's bytes must move the fingerprint, or
    /// a granted trust would survive a change to what actually executes.
    ///
    /// Hashing the link itself would pass `linked_skill_directory_fingerprints`
    /// above and fail here, which is exactly why both exist.
    #[cfg(unix)]
    #[test]
    fn rewriting_a_link_target_moves_the_fingerprint() {
        use std::os::unix::fs::symlink;

        let assets = tempfile::tempdir().unwrap();
        let skill = assets.path().join("office-cli");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), b"# original\n").unwrap();

        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join(".wayland-core/skills")).unwrap();
        symlink(
            &skill,
            workspace.path().join(".wayland-core/skills/office-cli"),
        )
        .unwrap();

        let before = fingerprint_workspace(workspace.path()).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            b"# rewritten to do something else\n",
        )
        .unwrap();
        let after = fingerprint_workspace(workspace.path()).unwrap();

        assert_ne!(
            before.digest, after.digest,
            "rewriting the target of a linked skill MUST invalidate the fingerprint; \
             otherwise trust granted over one executable surface silently covers another"
        );
    }

    /// Repointing the link at different content must also move it.
    #[cfg(unix)]
    #[test]
    fn repointing_a_link_moves_the_fingerprint() {
        use std::os::unix::fs::symlink;

        let assets = tempfile::tempdir().unwrap();
        for (dir, body) in [("one", "# one\n"), ("two", "# two\n")] {
            let d = assets.path().join(dir);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("SKILL.md"), body).unwrap();
        }

        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join(".wayland-core/skills")).unwrap();
        let link = workspace.path().join(".wayland-core/skills/s");
        symlink(assets.path().join("one"), &link).unwrap();
        let before = fingerprint_workspace(workspace.path()).unwrap();

        fs::remove_file(&link).unwrap();
        symlink(assets.path().join("two"), &link).unwrap();
        let after = fingerprint_workspace(workspace.path()).unwrap();

        assert_ne!(
            before.digest, after.digest,
            "repointing a linked skill at different content MUST invalidate the fingerprint"
        );
    }
}
