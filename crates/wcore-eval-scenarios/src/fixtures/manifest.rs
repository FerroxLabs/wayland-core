//! Content-addressed identity for one complete deterministic fixture bundle.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const MANIFEST_SCHEMA: &str = "wcore-eval-composite-fixture";
const MANIFEST_VERSION: u32 = 1;
const BINDING_SCHEMA: &str = "wcore-eval-composite-fixture-binding";
const BINDING_VERSION: u32 = 1;
const MAX_COMPONENT_BYTES: u64 = 16 * 1024 * 1024;

/// The six content identities that define an F04 deterministic fixture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureComponents {
    openai_script_sha256: String,
    seeded_repository_sha256: String,
    hidden_outcome_sha256: String,
    mcp_script_sha256: String,
    egress_script_sha256: String,
    remote_execution_script_sha256: String,
}

impl FixtureComponents {
    /// Compute every component identity from the exact artifact bytes.
    pub fn from_artifacts(
        openai_script: &[u8],
        seeded_repository: &[u8],
        hidden_outcome: &[u8],
        mcp_script: &[u8],
        egress_script: &[u8],
        remote_execution_script: &[u8],
    ) -> Self {
        Self {
            openai_script_sha256: sha256(openai_script),
            seeded_repository_sha256: sha256(seeded_repository),
            hidden_outcome_sha256: sha256(hidden_outcome),
            mcp_script_sha256: sha256(mcp_script),
            egress_script_sha256: sha256(egress_script),
            remote_execution_script_sha256: sha256(remote_execution_script),
        }
    }

    fn entries(&self) -> [(&'static str, &str); 6] {
        [
            ("openai_script", &self.openai_script_sha256),
            ("seeded_repository", &self.seeded_repository_sha256),
            ("hidden_outcome", &self.hidden_outcome_sha256),
            ("mcp_script", &self.mcp_script_sha256),
            ("egress_script", &self.egress_script_sha256),
            (
                "remote_execution_script",
                &self.remote_execution_script_sha256,
            ),
        ]
    }
}

/// Versioned manifest whose digest changes when any component artifact changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositeFixtureManifest {
    schema: String,
    schema_version: u32,
    components: FixtureComponents,
    fixture_sha256: String,
}

impl CompositeFixtureManifest {
    pub fn from_artifacts(
        openai_script: &[u8],
        seeded_repository: &[u8],
        hidden_outcome: &[u8],
        mcp_script: &[u8],
        egress_script: &[u8],
        remote_execution_script: &[u8],
    ) -> Self {
        Self::new(FixtureComponents::from_artifacts(
            openai_script,
            seeded_repository,
            hidden_outcome,
            mcp_script,
            egress_script,
            remote_execution_script,
        ))
    }

    fn new(components: FixtureComponents) -> Self {
        let canonical = serde_json::to_vec(&CanonicalManifest {
            schema: MANIFEST_SCHEMA,
            schema_version: MANIFEST_VERSION,
            components: &components,
        })
        .expect("fixture manifest contains only infallible JSON values");
        Self {
            schema: MANIFEST_SCHEMA.to_string(),
            schema_version: MANIFEST_VERSION,
            components,
            fixture_sha256: sha256(&canonical),
        }
    }

    pub fn components(&self) -> &FixtureComponents {
        &self.components
    }

    pub fn fixture_sha256(&self) -> &str {
        &self.fixture_sha256
    }

    pub fn verify(&self) -> Result<(), FixtureManifestError> {
        if self.schema != MANIFEST_SCHEMA || self.schema_version != MANIFEST_VERSION {
            return Err(FixtureManifestError::UnsupportedSchema);
        }
        for (name, digest) in self.components.entries() {
            if !valid_sha256(digest) {
                return Err(FixtureManifestError::InvalidNamedSha256 {
                    component: name.to_string(),
                });
            }
        }
        let expected = Self::new(self.components.clone());
        if self.fixture_sha256 != expected.fixture_sha256 {
            return Err(FixtureManifestError::DigestMismatch);
        }
        Ok(())
    }
}

/// Relative paths to the six artifacts whose bytes define a fixture manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureArtifactPaths {
    openai_script: PathBuf,
    seeded_repository: PathBuf,
    hidden_outcome: PathBuf,
    mcp_script: PathBuf,
    egress_script: PathBuf,
    remote_execution_script: PathBuf,
}

impl FixtureArtifactPaths {
    pub fn new(
        openai_script: impl Into<PathBuf>,
        seeded_repository: impl Into<PathBuf>,
        hidden_outcome: impl Into<PathBuf>,
        mcp_script: impl Into<PathBuf>,
        egress_script: impl Into<PathBuf>,
        remote_execution_script: impl Into<PathBuf>,
    ) -> Self {
        Self {
            openai_script: openai_script.into(),
            seeded_repository: seeded_repository.into(),
            hidden_outcome: hidden_outcome.into(),
            mcp_script: mcp_script.into(),
            egress_script: egress_script.into(),
            remote_execution_script: remote_execution_script.into(),
        }
    }

    fn entries(&self) -> [(&'static str, &Path); 6] {
        [
            ("openai_script", &self.openai_script),
            ("seeded_repository", &self.seeded_repository),
            ("hidden_outcome", &self.hidden_outcome),
            ("mcp_script", &self.mcp_script),
            ("egress_script", &self.egress_script),
            ("remote_execution_script", &self.remote_execution_script),
        ]
    }
}

/// A controller-supplied manifest bound to exact, independently readable files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundCompositeFixtureManifest {
    schema: String,
    schema_version: u32,
    manifest: CompositeFixtureManifest,
    artifacts: FixtureArtifactPaths,
}

impl BoundCompositeFixtureManifest {
    /// Build a binding by reading the live fixture inputs below `root`.
    pub fn from_artifacts(
        root: &Path,
        artifacts: FixtureArtifactPaths,
    ) -> Result<Self, FixtureManifestError> {
        let bytes = read_artifacts(root, &artifacts)?;
        Ok(Self {
            schema: BINDING_SCHEMA.to_string(),
            schema_version: BINDING_VERSION,
            manifest: CompositeFixtureManifest::from_artifacts(
                &bytes[0], &bytes[1], &bytes[2], &bytes[3], &bytes[4], &bytes[5],
            ),
            artifacts,
        })
    }

    pub fn manifest(&self) -> &CompositeFixtureManifest {
        &self.manifest
    }

    /// Re-read the live artifacts and reject any label, content, or path drift.
    pub fn verify(&self, root: &Path) -> Result<(), FixtureManifestError> {
        if self.schema != BINDING_SCHEMA || self.schema_version != BINDING_VERSION {
            return Err(FixtureManifestError::UnsupportedBindingSchema);
        }
        self.manifest.verify()?;
        let actual = Self::from_artifacts(root, self.artifacts.clone())?;
        for ((component, expected), (_, observed)) in self
            .manifest
            .components
            .entries()
            .into_iter()
            .zip(actual.manifest.components.entries())
        {
            if expected != observed {
                return Err(FixtureManifestError::ArtifactDigestMismatch {
                    component: component.to_string(),
                });
            }
        }
        if self.manifest.fixture_sha256 != actual.manifest.fixture_sha256 {
            return Err(FixtureManifestError::DigestMismatch);
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct CanonicalManifest<'a> {
    schema: &'static str,
    schema_version: u32,
    components: &'a FixtureComponents,
}

fn read_artifacts(
    root: &Path,
    artifacts: &FixtureArtifactPaths,
) -> Result<[Vec<u8>; 6], FixtureManifestError> {
    artifacts
        .entries()
        .map(|(component, relative)| read_artifact(root, component, relative))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| FixtureManifestError::InvalidArtifactSet)
}

fn read_artifact(
    root: &Path,
    component: &'static str,
    relative: &Path,
) -> Result<Vec<u8>, FixtureManifestError> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(FixtureManifestError::InvalidArtifactPath { component });
    }
    let path = root.join(relative);
    let mut file = match open_artifact(root, relative) {
        Ok(file) => file,
        #[cfg(unix)]
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(FixtureManifestError::UnsafeArtifact { component, path });
        }
        Err(error) => {
            return Err(FixtureManifestError::ArtifactIo {
                component,
                path,
                detail: error.to_string(),
            });
        }
    };
    let metadata = file
        .metadata()
        .map_err(|error| FixtureManifestError::ArtifactIo {
            component,
            path: path.clone(),
            detail: error.to_string(),
        })?;
    if !metadata.is_file() {
        return Err(FixtureManifestError::UnsafeArtifact { component, path });
    }
    if metadata.len() > MAX_COMPONENT_BYTES {
        return Err(FixtureManifestError::ArtifactTooLarge { component });
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_COMPONENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| FixtureManifestError::ArtifactIo {
            component,
            path: path.clone(),
            detail: error.to_string(),
        })?;
    if bytes.len() as u64 > MAX_COMPONENT_BYTES {
        return Err(FixtureManifestError::ArtifactTooLarge { component });
    }
    Ok(bytes)
}

#[cfg(unix)]
fn open_artifact(root: &Path, relative: &Path) -> std::io::Result<File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let root = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: `root` is a valid C string. O_NOFOLLOW rejects a replaced root,
    // and the returned descriptor is immediately owned by `File`.
    let root_fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if root_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `root_fd` is a newly owned descriptor on the success path.
    let mut directory = unsafe { File::from_raw_fd(root_fd) };
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        };
        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let last = index + 1 == components.len();
        let flags = if last {
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW
        } else {
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW
        };
        // SAFETY: the parent descriptor remains open for this call, `name` is
        // a single validated component, and the returned descriptor is owned.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a newly owned descriptor on the success path.
        let opened = unsafe { File::from_raw_fd(fd) };
        if last {
            return Ok(opened);
        }
        directory = opened;
    }
    Err(std::io::Error::from(std::io::ErrorKind::InvalidInput))
}

#[cfg(not(unix))]
fn open_artifact(root: &Path, relative: &Path) -> std::io::Result<File> {
    let root = root.canonicalize()?;
    let path = root.join(relative).canonicalize()?;
    if !path.starts_with(&root) {
        return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    }
    File::open(path)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FixtureManifestError {
    #[error("{component} identity must be 64 lowercase hexadecimal characters")]
    InvalidNamedSha256 { component: String },
    #[error("unsupported composite fixture manifest schema")]
    UnsupportedSchema,
    #[error("unsupported composite fixture binding schema")]
    UnsupportedBindingSchema,
    #[error("composite fixture manifest digest does not match its components")]
    DigestMismatch,
    #[error("{component} artifact bytes do not match the supplied manifest")]
    ArtifactDigestMismatch { component: String },
    #[error("{component} artifact path must be a non-empty relative path without traversal")]
    InvalidArtifactPath { component: &'static str },
    #[error("{component} artifact is not a regular non-symlink file: {path}")]
    UnsafeArtifact {
        component: &'static str,
        path: PathBuf,
    },
    #[error("{component} artifact exceeds the 16 MiB fixture limit")]
    ArtifactTooLarge { component: &'static str },
    #[error("could not read {component} artifact {path}: {detail}")]
    ArtifactIo {
        component: &'static str,
        path: PathBuf,
        detail: String,
    },
    #[error("fixture binding must contain exactly six artifacts")]
    InvalidArtifactSet,
}
