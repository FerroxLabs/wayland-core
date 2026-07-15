//! W8a A.3 — VirtualFs trait + RealFs / InMemoryFs / SandboxedFs impls (X2).
//!
//! Tools that touch the filesystem go through `ToolContext.vfs` (an
//! `Arc<dyn VirtualFs>`) so the engine can swap RealFs for an in-memory
//! mock in tests, and clamp sub-agents to a `SandboxedFs { root }`
//! rooted at their workspace.
//!
//! Wave SD hardening (SECURITY MAJORs #13 + #14 + closed in tandem with
//! the legacy `execute()` validation in read.rs / write.rs / edit.rs):
//!
//! 1. `fallthrough_reads` is **gone**. Reads are sandbox-checked the
//!    same way writes are. The previous escape hatch let a sub-agent
//!    `Read("/etc/passwd")` whenever the host flipped the flag for
//!    performance. If a use case really needs broader reads, callers
//!    must build a `SandboxPolicy { read_allowlist, write_allowlist }`
//!    and pass paths through explicit allow-list checks.
//!
//! 2. `contain()` now resolves symlinks via `std::fs::canonicalize`
//!    BEFORE the containment compare. Lex-normalization (`..` collapse)
//!    is only used for paths that don't yet exist. A symlink planted
//!    inside the sandbox that points outside is detected and refused.
//!    TOCTOU: the canonicalize re-runs on every operation — never
//!    cached — so swapping the symlink between two ops doesn't escape.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::ffi::{CString, OsStr};
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VfsError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("path {path:?} is outside sandbox root {root:?}")]
    OutsideSandbox { path: PathBuf, root: PathBuf },
    #[error("path {path:?} not found")]
    NotFound { path: PathBuf },
    #[error("refused: {path:?} is a protected secret path")]
    SecretDenied { path: PathBuf },
}

/// Strong content identity used by conditional filesystem mutations.
///
/// This intentionally describes bytes only. It is not an inode, generation,
/// ACL, or platform file-identity receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileContentIdentity {
    pub sha256: [u8; 32],
    pub len: u64,
}

impl FileContentIdentity {
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            sha256: sha256(bytes),
            len: bytes.len() as u64,
        }
    }

    #[must_use]
    pub fn sha256_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.sha256 {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }
}

/// Byte-level observation made while holding an implementation's same-path
/// serialization boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileObservation {
    Absent,
    Present(FileContentIdentity),
}

/// Stable authority and path-object identity captured with a file observation.
///
/// `authority` names the VFS instance or host filesystem namespace. `path`
/// names the resolved path inside that authority. `parent` and `file` are
/// implementation-owned object tokens (Unix device/inode identities for
/// `RealFs`, generation identities for `InMemoryFs`). A missing `parent`
/// means the target's parent did not yet exist at preparation time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileObjectIdentity {
    pub authority: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

impl FileObjectIdentity {
    pub(crate) fn same_path_authority(&self, current: &Self) -> bool {
        self.authority == current.authority
            && self.path == current.path
            && self
                .parent
                .as_ref()
                .is_none_or(|expected| current.parent.as_ref() == Some(expected))
    }

    pub(crate) fn same_prepared_object(&self, current: &Self) -> bool {
        self.same_path_authority(current) && self.file == current.file
    }
}

/// Identity-aware, read-only snapshot used to prepare and reconcile durable
/// filesystem effects. Contents are runtime-only and are never serialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifiedFileObservation {
    pub observation: FileObservation,
    pub object: FileObjectIdentity,
    contents: Option<Vec<u8>>,
}

impl IdentifiedFileObservation {
    #[must_use]
    pub fn contents(&self) -> Option<&[u8]> {
        self.contents.as_deref()
    }
}

/// State that must still be present immediately before a mutation is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePrecondition {
    Absent,
    Present(FileContentIdentity),
}

impl FilePrecondition {
    fn matches(self, observation: FileObservation) -> bool {
        match (self, observation) {
            (Self::Absent, FileObservation::Absent) => true,
            (Self::Present(expected), FileObservation::Present(observed)) => expected == observed,
            _ => false,
        }
    }
}

/// Intended replacement bytes plus the exact preimage required to write them.
///
/// The intended digest is computed internally so callers cannot accidentally
/// bind a receipt to bytes different from the bytes passed to the VFS.
#[derive(Clone)]
pub struct IntendedFileMutation {
    pub precondition: FilePrecondition,
    pub intended: FileContentIdentity,
    expected_object: Option<FileObjectIdentity>,
    contents: Vec<u8>,
}

impl std::fmt::Debug for IntendedFileMutation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IntendedFileMutation")
            .field("precondition", &self.precondition)
            .field("intended", &self.intended)
            .field("expected_object", &self.expected_object)
            .field("contents", &"[redacted]")
            .finish()
    }
}

impl IntendedFileMutation {
    #[must_use]
    pub fn new(precondition: FilePrecondition, contents: impl Into<Vec<u8>>) -> Self {
        let contents = contents.into();
        let intended = FileContentIdentity::from_bytes(&contents);
        Self {
            precondition,
            intended,
            expected_object: None,
            contents,
        }
    }

    /// Bind this mutation to an exact VFS authority/path object.
    ///
    /// This is currently used only by deterministic/cooperative fixture
    /// backends. Ordinary host Write/Edit operations remain opaque.
    #[must_use]
    pub fn from_observation(
        observed: &IdentifiedFileObservation,
        contents: impl Into<Vec<u8>>,
    ) -> Self {
        let precondition = match observed.observation {
            FileObservation::Absent => FilePrecondition::Absent,
            FileObservation::Present(identity) => FilePrecondition::Present(identity),
        };
        let mut mutation = Self::new(precondition, contents);
        mutation.expected_object = Some(observed.object.clone());
        mutation
    }

    #[must_use]
    pub fn contents(&self) -> &[u8] {
        &self.contents
    }

    fn precondition_matches(&self, observed: &IdentifiedFileObservation) -> bool {
        self.precondition.matches(observed.observation)
            && self
                .expected_object
                .as_ref()
                .is_none_or(|expected| expected.same_prepared_object(&observed.object))
    }

    fn postcondition_authority_matches(&self, observed: &IdentifiedFileObservation) -> bool {
        self.expected_object
            .as_ref()
            .is_none_or(|expected| expected.same_path_authority(&observed.object))
    }

    fn already_applied_matches(&self, observed: &IdentifiedFileObservation) -> bool {
        if observed.observation != FileObservation::Present(self.intended)
            || !self.postcondition_authority_matches(observed)
        {
            return false;
        }
        // A byte-identical fixture mutation is a no-op only while the exact
        // prepared object still matches.
        match self.precondition {
            FilePrecondition::Present(preimage) if preimage == self.intended => {
                self.precondition_matches(observed)
            }
            _ => true,
        }
    }

    fn with_expected_object(&self, expected_object: FileObjectIdentity) -> Self {
        let mut rebound = self.clone();
        rebound.expected_object = Some(expected_object);
        rebound
    }
}

/// Result of one conditional mutation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMutationOutcome {
    Applied {
        previous: FileObservation,
        current: FileContentIdentity,
    },
    AlreadyApplied {
        current: FileContentIdentity,
    },
    Conflict {
        current: FileObservation,
    },
}

/// Provider-neutral filesystem the agent runs against.
///
/// All methods take `&Path` and return `VfsError`. Implementors are
/// expected to be `Send + Sync` so they can be shared via `Arc`.
#[async_trait]
pub trait VirtualFs: Send + Sync {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError>;
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError>;
    async fn exists(&self, path: &Path) -> Result<bool, VfsError>;
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError>;
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError>;
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError>;

    /// Observe bytes and the VFS/path object identity in one implementation-
    /// owned read-only operation. Durable receipts use this instead of `read`
    /// so matching bytes alone can never resolve an uncertain effect.
    async fn observe_file(&self, path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
        Err(VfsError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "identity-aware file observation is not implemented for {}",
                path.display()
            ),
        )))
    }

    /// Compare the current bytes with an intended mutation when the backend
    /// owns an authoritative revision/serialization boundary.
    ///
    /// In-memory and explicitly cooperative fixture backends may implement
    /// this. Ordinary host filesystems must return `Unsupported` because they
    /// cannot protect a pathname from non-cooperating concurrent writers.
    async fn compare_exchange_file(
        &self,
        path: &Path,
        mutation: &IntendedFileMutation,
    ) -> Result<FileMutationOutcome, VfsError> {
        let _ = mutation;
        Err(VfsError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "durable file compare-exchange is not implemented for {}",
                path.display()
            ),
        )))
    }

    /// The containment root for a sandboxed filesystem, or `None` for an
    /// unconstrained one (`RealFs`, `InMemoryFs`). Tools that shell out to a
    /// subprocess (e.g. Grep → `rg`/`grep`) can't route the scan through the
    /// vfs, so they use this to anchor the subprocess working directory to the
    /// jail root — making a relative search path resolve against the sandbox,
    /// not the process cwd (F36).
    fn root(&self) -> Option<&Path> {
        None
    }
}

/// Minimum metadata surface tools need (size + is_dir). Avoids leaking
/// `std::fs::Metadata` into the trait so InMemoryFs can be honest about
/// its lack of filesystem-grade attributes.
#[derive(Debug, Clone)]
pub struct VfsMetadata {
    pub size: u64,
    pub is_dir: bool,
}

/// RealFs — passes through to `tokio::fs`.
pub struct RealFs;

#[async_trait]
impl VirtualFs for RealFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        Ok(tokio::fs::read(path).await?)
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        let path_owned = path.to_path_buf();
        let data = contents.to_vec();
        tokio::task::spawn_blocking(move || wcore_config::atomic_write(&path_owned, &data))
            .await
            .map_err(|e| VfsError::Io(std::io::Error::other(e)))??;
        Ok(())
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        Ok(tokio::fs::try_exists(path).await?)
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        let mut entries = tokio::fs::read_dir(dir).await?;
        let mut out = Vec::new();
        while let Some(e) = entries.next_entry().await? {
            out.push(e.path());
        }
        Ok(out)
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        Ok(tokio::fs::remove_file(path).await?)
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        let m = tokio::fs::metadata(path).await?;
        Ok(VfsMetadata {
            size: m.len(),
            is_dir: m.is_dir(),
        })
    }
    async fn observe_file(&self, path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || observe_real_file(&path))
            .await
            .map_err(|error| VfsError::Io(io::Error::other(error)))?
    }
    async fn compare_exchange_file(
        &self,
        path: &Path,
        mutation: &IntendedFileMutation,
    ) -> Result<FileMutationOutcome, VfsError> {
        let _ = mutation;
        Err(VfsError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "authoritative compare-exchange for ordinary host files is unavailable for {}",
                path.display()
            ),
        )))
    }
}

fn observe_real_file(path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
    #[cfg(unix)]
    {
        observe_real_file_unix(path)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(VfsError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            "identity-aware file observation is unavailable on this platform",
        )))
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedUnixFile {
    observation: FileObservation,
    object: FileObjectIdentity,
    contents: Option<Vec<u8>>,
}

#[cfg(unix)]
impl ObservedUnixFile {
    fn identified(&self) -> IdentifiedFileObservation {
        IdentifiedFileObservation {
            observation: self.observation,
            object: self.object.clone(),
            contents: self.contents.clone(),
        }
    }
}

#[cfg(unix)]
fn observe_real_file_unix(path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let normalized = lex_normalize(&absolute, Path::new(""));
    let leaf = normalized.file_name().ok_or_else(|| {
        VfsError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file observation requires a file name: {path:?}"),
        ))
    })?;
    let requested_parent = normalized.parent().ok_or_else(|| {
        VfsError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file observation requires a parent directory: {path:?}"),
        ))
    })?;

    match fs::canonicalize(requested_parent) {
        Ok(parent_path) => {
            let parent = fs::OpenOptions::new().read(true).open(&parent_path)?;
            let metadata = parent.metadata()?;
            ensure_directory(&metadata, &parent_path)?;
            let observed = observe_unix_file(&parent, &parent_path, leaf)?;
            Ok(observed.identified())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let (anchor_path, suffix) = canonical_existing_ancestor(requested_parent, path)?;
            let anchor = fs::OpenOptions::new().read(true).open(&anchor_path)?;
            let metadata = anchor.metadata()?;
            ensure_directory(&metadata, &anchor_path)?;
            let resolved_parent = anchor_path.join(suffix);
            Ok(IdentifiedFileObservation {
                observation: FileObservation::Absent,
                object: FileObjectIdentity {
                    authority: real_fs_authority()?,
                    path: resolved_parent.join(leaf),
                    parent: None,
                    file: None,
                },
                contents: None,
            })
        }
        Err(error) => Err(VfsError::Io(error)),
    }
}

#[cfg(unix)]
fn canonical_existing_ancestor<'a>(
    requested: &'a Path,
    original: &Path,
) -> Result<(PathBuf, &'a Path), VfsError> {
    let mut existing = requested;
    loop {
        match fs::canonicalize(existing) {
            Ok(canonical) => {
                let suffix = requested
                    .strip_prefix(existing)
                    .map_err(|error| VfsError::Io(io::Error::other(error)))?;
                return Ok((canonical, suffix));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                existing = existing.parent().ok_or_else(|| {
                    VfsError::Io(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no existing ancestor for {original:?}"),
                    ))
                })?;
            }
            Err(error) => return Err(VfsError::Io(error)),
        }
    }
}

#[cfg(unix)]
fn c_name(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "filesystem name contains an embedded NUL",
        )
    })
}

#[cfg(unix)]
fn openat_file(parent: &fs::File, name: &OsStr, flags: i32, mode: u32) -> io::Result<fs::File> {
    let name = c_name(name)?;
    // SAFETY: `name` is a live NUL-terminated string, `parent` remains open,
    // and ownership of a successful descriptor is transferred exactly once.
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, mode) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `descriptor` was returned by `openat` above and is uniquely owned.
    Ok(unsafe { fs::File::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn ensure_directory(metadata: &fs::Metadata, path: &Path) -> io::Result<()> {
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("CAS parent is not a directory: {path:?}"),
        ))
    }
}

#[cfg(unix)]
fn unix_identity_token(metadata: &fs::Metadata) -> String {
    format!(
        "unix-v1:{}:{}:{}:{:o}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.nlink(),
        metadata.mode(),
        metadata.uid(),
        metadata.gid()
    )
}

#[cfg(unix)]
fn real_fs_authority() -> Result<String, VfsError> {
    let root = fs::metadata(Path::new("/"))?;
    Ok(format!("realfs:unix:{}:{}", root.dev(), root.ino()))
}

#[cfg(unix)]
fn observe_unix_file(
    parent: &fs::File,
    parent_path: &Path,
    leaf: &OsStr,
) -> Result<ObservedUnixFile, VfsError> {
    let parent_metadata = parent.metadata()?;
    ensure_directory(&parent_metadata, parent_path)?;
    let object_path = parent_path.join(leaf);
    let authority = real_fs_authority()?;
    let parent_identity = Some(unix_identity_token(&parent_metadata));
    let mut file = match openat_file(
        parent,
        leaf,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        0,
    ) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
            return Ok(ObservedUnixFile {
                observation: FileObservation::Absent,
                object: FileObjectIdentity {
                    authority,
                    path: object_path,
                    parent: parent_identity,
                    file: None,
                },
                contents: None,
            });
        }
        Err(error) => return Err(VfsError::Io(error)),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(VfsError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "CAS target must be a singly-linked regular file",
        )));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(ObservedUnixFile {
        observation: FileObservation::Present(FileContentIdentity::from_bytes(&bytes)),
        object: FileObjectIdentity {
            authority,
            path: object_path,
            parent: parent_identity,
            file: Some(unix_identity_token(&metadata)),
        },
        contents: Some(bytes),
    })
}

/// InMemoryFs — pure ephemeral byte store. Used in tests to isolate
/// tool tests from real disk.
pub struct InMemoryFs {
    authority: String,
    files: Arc<RwLock<std::collections::HashMap<PathBuf, InMemoryFile>>>,
}

#[derive(Clone)]
struct InMemoryFile {
    bytes: Vec<u8>,
    generation: String,
}

impl Default for InMemoryFs {
    fn default() -> Self {
        Self {
            authority: format!("in-memory:{}", uuid::Uuid::new_v4()),
            files: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }
}

impl InMemoryFs {
    pub fn new() -> Self {
        Self::default()
    }

    fn parent_identity(&self, path: &Path) -> Option<String> {
        path.parent()
            .map(|parent| format!("{}:parent:{}", self.authority, parent.display()))
    }
}

#[async_trait]
impl VirtualFs for InMemoryFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        self.files
            .read()
            .get(path)
            .map(|file| file.bytes.clone())
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_path_buf(),
            })
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError> {
        self.files
            .write()
            .insert(path.to_path_buf(), InMemoryFile::new(contents));
        Ok(())
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        Ok(self.files.read().contains_key(path))
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        Ok(self
            .files
            .read()
            .keys()
            .filter(|p| p.parent() == Some(dir))
            .cloned()
            .collect())
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        self.files
            .write()
            .remove(path)
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_path_buf(),
            })?;
        Ok(())
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        let files = self.files.read();
        let bytes = files.get(path).ok_or_else(|| VfsError::NotFound {
            path: path.to_path_buf(),
        })?;
        Ok(VfsMetadata {
            size: bytes.bytes.len() as u64,
            is_dir: false,
        })
    }
    async fn observe_file(&self, path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
        let files = self.files.read();
        let file = files.get(path);
        let contents = file.map(|file| file.bytes.clone());
        let observation = contents
            .as_deref()
            .map_or(FileObservation::Absent, |bytes| {
                FileObservation::Present(FileContentIdentity::from_bytes(bytes))
            });
        Ok(IdentifiedFileObservation {
            observation,
            object: FileObjectIdentity {
                authority: self.authority.clone(),
                path: path.to_path_buf(),
                parent: self.parent_identity(path),
                file: file.map(|file| file.generation.clone()),
            },
            contents,
        })
    }
    async fn compare_exchange_file(
        &self,
        path: &Path,
        mutation: &IntendedFileMutation,
    ) -> Result<FileMutationOutcome, VfsError> {
        let mut files = self.files.write();
        let file = files.get(path);
        let contents = file.map(|file| file.bytes.clone());
        let current = contents
            .as_deref()
            .map_or(FileObservation::Absent, |bytes| {
                FileObservation::Present(FileContentIdentity::from_bytes(bytes))
            });
        let identified = IdentifiedFileObservation {
            observation: current,
            object: FileObjectIdentity {
                authority: self.authority.clone(),
                path: path.to_path_buf(),
                parent: self.parent_identity(path),
                file: file.map(|file| file.generation.clone()),
            },
            contents,
        };

        if !mutation.postcondition_authority_matches(&identified) {
            return Ok(FileMutationOutcome::Conflict { current });
        }
        if mutation.already_applied_matches(&identified) {
            return Ok(FileMutationOutcome::AlreadyApplied {
                current: mutation.intended,
            });
        }
        if current == FileObservation::Present(mutation.intended) {
            return Ok(FileMutationOutcome::Conflict { current });
        }
        if !mutation.precondition_matches(&identified) {
            return Ok(FileMutationOutcome::Conflict { current });
        }

        files.insert(path.to_path_buf(), InMemoryFile::new(mutation.contents()));
        Ok(FileMutationOutcome::Applied {
            previous: current,
            current: mutation.intended,
        })
    }
}

impl InMemoryFile {
    fn new(contents: &[u8]) -> Self {
        Self {
            bytes: contents.to_vec(),
            generation: format!("in-memory-file:{}", uuid::Uuid::new_v4()),
        }
    }
}

/// SandboxedFs — wraps a `VirtualFs` (typically `RealFs`) and rejects
/// any operation whose canonical path escapes `root`. Reads and writes
/// both apply the same containment check; there is intentionally no
/// "fallthrough_reads" footgun (Wave SD SECURITY MAJOR #13).
pub struct SandboxedFs<F: VirtualFs> {
    inner: F,
    root: PathBuf,
}

impl<F: VirtualFs> SandboxedFs<F> {
    /// `root` is canonicalized on construction so the contain check
    /// compares apples to apples (e.g. macOS `/var` → `/private/var`).
    /// Falls back to `root` if canonicalization fails (dir doesn't
    /// exist yet); per-op containment still re-checks the live
    /// filesystem.
    pub fn new(inner: F, root: impl Into<PathBuf>) -> Self {
        let raw = root.into();
        let root = fs::canonicalize(&raw).unwrap_or(raw);
        Self { inner, root }
    }

    /// Returns Ok when `path` resolves inside `self.root`, Err
    /// otherwise.
    ///
    /// Strategy:
    ///   1. Lexically normalize the candidate path (strip `.`, collapse
    ///      `..`) — this rejects classic traversal strings before any
    ///      I/O.
    ///   2. Canonicalize the longest existing prefix via `fs::canonicalize`,
    ///      which **resolves symlinks**. The result MUST start with
    ///      `self.root` after the same canonicalization step that ran
    ///      in `new()`. This closes the SECURITY MAJOR #13 symlink
    ///      bypass: a symlink `<root>/escape -> /etc` lex-normalizes
    ///      to `<root>/escape` (in-bounds) but canonicalize() returns
    ///      `/etc` (out of bounds) and we refuse.
    ///   3. For paths whose existing prefix is exactly `self.root`
    ///      (i.e. the leaf doesn't exist yet — e.g. a write target),
    ///      step 2's canonical prefix already starts with `self.root`,
    ///      so the suffix is allowed because no symlink can escape
    ///      through a not-yet-created node.
    async fn contain(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let normalized = lex_normalize(path, &self.root);

        // Walk up the path to the longest existing prefix, canonicalize
        // it (which follows symlinks), and check the canonical form
        // sits inside `self.root`. If the prefix canonicalizes to
        // somewhere outside the root, refuse — even if the trailing
        // not-yet-existing suffix is benign.
        let (canon_prefix, suffix) = match canonicalize_existing_prefix(&normalized).await {
            Some((prefix, suffix)) => (prefix, suffix),
            None => {
                return Err(VfsError::OutsideSandbox {
                    path: normalized,
                    root: self.root.clone(),
                });
            }
        };

        if !canon_prefix.starts_with(&self.root) {
            return Err(VfsError::OutsideSandbox {
                path: normalized,
                root: self.root.clone(),
            });
        }

        // Re-assemble: canonical prefix + (still-relative) suffix.
        // When the entire path already exists `suffix` is empty and the
        // canonical prefix IS the read target; `PathBuf::join("")` would
        // leave a stray trailing separator on some platforms (turns a
        // file lookup into a dir lookup → ENOTDIR), so short-circuit.
        if suffix.as_os_str().is_empty() {
            Ok(canon_prefix)
        } else {
            Ok(canon_prefix.join(suffix))
        }
    }

    fn bind_identity(
        &self,
        mut object: FileObjectIdentity,
    ) -> Result<FileObjectIdentity, VfsError> {
        if !object.path.starts_with(&self.root) {
            return Err(VfsError::OutsideSandbox {
                path: object.path,
                root: self.root.clone(),
            });
        }
        object.authority = format!(
            "sandbox:{}|{}",
            sandbox_root_identity(&self.root)?,
            object.authority
        );
        Ok(object)
    }
}

fn sandbox_root_identity(root: &Path) -> Result<String, VfsError> {
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    #[cfg(unix)]
    {
        let metadata = fs::metadata(&canonical)?;
        if !metadata.is_dir() {
            return Err(VfsError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("sandbox root is not a directory: {canonical:?}"),
            )));
        }
        Ok(format!(
            "unix:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            canonical.display()
        ))
    }
    #[cfg(not(unix))]
    {
        Ok(format!("path:{}", canonical.display()))
    }
}

/// Find the longest existing ancestor of `path` and return its
/// canonical form plus the (possibly empty) trailing not-yet-existing
/// suffix. Returns `None` only when even `path.ancestors()` can't yield
/// a real prefix (e.g. relative path with no anchor) — the caller
/// should refuse such inputs.
async fn canonicalize_existing_prefix(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut p: &Path = path;
    loop {
        // `tokio::fs::canonicalize` offloads the blocking `std::fs::canonicalize`
        // syscall to the blocking pool. On a stalled network mount — e.g. a
        // Windows `\\wsl$\` 9P share (FerroxLabs/wayland#287) — that syscall can
        // hang indefinitely; keeping it OFF the runtime thread means the
        // per-tool dispatch timeout still fires (an error result) instead of the
        // worker wedging mid-poll and the tool hanging silently forever. A
        // blocking syscall on the reactor cannot be preempted by
        // `tokio::time::timeout`.
        if let Ok(canon) = tokio::fs::canonicalize(p).await {
            // Suffix is the part of `path` that lives beyond `p`. When
            // `p == path` (the whole path exists and canonicalized
            // cleanly), the suffix is empty and the read target IS the
            // canonical form — don't join `""` since some PathBuf
            // implementations append `/` and turn a file lookup into a
            // dir lookup ("Not a directory" / ENOTDIR).
            let suffix = path.strip_prefix(p).unwrap_or(Path::new(""));
            return Some((canon, suffix.to_path_buf()));
        }
        p = p.parent()?;
    }
}

fn lex_normalize(path: &Path, base: &Path) -> PathBuf {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut out = PathBuf::new();
    for c in candidate.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                out.push(c.as_os_str());
            }
        }
    }
    out
}

#[async_trait]
impl<F: VirtualFs + 'static> VirtualFs for SandboxedFs<F> {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        let p = self.contain(path).await?;
        self.inner.read(&p).await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError> {
        let p = self.contain(path).await?;
        self.inner.write(&p, contents).await
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        let p = self.contain(path).await?;
        self.inner.exists(&p).await
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        let p = self.contain(dir).await?;
        self.inner.list(&p).await
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        let p = self.contain(path).await?;
        self.inner.remove_file(&p).await
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        let p = self.contain(path).await?;
        self.inner.metadata(&p).await
    }
    async fn observe_file(&self, path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
        let p = self.contain(path).await?;
        let mut observed = self.inner.observe_file(&p).await?;
        observed.object = self.bind_identity(observed.object)?;
        Ok(observed)
    }
    async fn compare_exchange_file(
        &self,
        path: &Path,
        mutation: &IntendedFileMutation,
    ) -> Result<FileMutationOutcome, VfsError> {
        let p = self.contain(path).await?;
        let inner_observed = self.inner.observe_file(&p).await?;
        let wrapped_observed = IdentifiedFileObservation {
            observation: inner_observed.observation,
            object: self.bind_identity(inner_observed.object.clone())?,
            contents: inner_observed.contents.clone(),
        };
        let already_applied = mutation.already_applied_matches(&wrapped_observed);
        if !mutation.postcondition_authority_matches(&wrapped_observed)
            || (wrapped_observed.observation == FileObservation::Present(mutation.intended)
                && !already_applied)
            || (!already_applied && !mutation.precondition_matches(&wrapped_observed))
        {
            return Ok(FileMutationOutcome::Conflict {
                current: wrapped_observed.observation,
            });
        }
        let rebound = mutation.with_expected_object(inner_observed.object);
        self.inner.compare_exchange_file(&p, &rebound).await
    }
    fn root(&self) -> Option<&Path> {
        Some(&self.root)
    }
}

/// Wraps a `VirtualFs` and refuses any op whose path is a PROJECT-committed
/// secret per the active `WorkspacePolicy` (a secret-named file under the
/// workspace root). Two deployments:
///   * Workspace posture: layered INSIDE `SandboxedFs`
///     (`SandboxedFs::new(SecretDenyFs::new(RealFs, p), root)`) so it inspects
///     the canonicalized path and catches symlinks-to-secrets inside the root.
///     The jail already confines every path to the root, so the scope check is
///     always satisfied there — behaviour is unchanged.
///   * #667 Full-posture channel/remote: installed WITHOUT a `SandboxedFs`
///     jail (Full stays unconfined for non-secret paths); the workspace-scoped
///     [`is_project_secret`](crate::workspace_policy::WorkspacePolicy::is_project_secret)
///     predicate is what limits the new denial to the project's own secrets,
///     leaving host secrets outside the workspace readable.
pub struct SecretDenyFs<F: VirtualFs> {
    inner: F,
    policy: std::sync::Arc<crate::workspace_policy::WorkspacePolicy>,
}

impl<F: VirtualFs> SecretDenyFs<F> {
    pub fn new(inner: F, policy: std::sync::Arc<crate::workspace_policy::WorkspacePolicy>) -> Self {
        Self { inner, policy }
    }
    fn guard(&self, path: &Path) -> Result<(), VfsError> {
        if self.policy.is_project_secret(path) {
            return Err(VfsError::SecretDenied {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl<F: VirtualFs + 'static> VirtualFs for SecretDenyFs<F> {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        self.guard(path)?;
        self.inner.read(path).await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError> {
        self.guard(path)?;
        self.inner.write(path, contents).await
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        self.guard(path)?;
        self.inner.exists(path).await
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        self.guard(dir)?;
        self.inner.list(dir).await
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        self.guard(path)?;
        self.inner.remove_file(path).await
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        self.guard(path)?;
        self.inner.metadata(path).await
    }
    async fn observe_file(&self, path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
        self.guard(path)?;
        self.inner.observe_file(path).await
    }
    async fn compare_exchange_file(
        &self,
        path: &Path,
        mutation: &IntendedFileMutation,
    ) -> Result<FileMutationOutcome, VfsError> {
        self.guard(path)?;
        self.inner.compare_exchange_file(path, mutation).await
    }
}

fn sha256(input: &[u8]) -> [u8; 32] {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const ROUND: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((input.len() + 72) & !63);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut state = INITIAL;
    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, bytes) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sum1)
                .wrapping_add(choose)
                .wrapping_add(ROUND[index])
                .wrapping_add(words[index]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut output = [0_u8; 32];
    for (bytes, word) in output.chunks_exact_mut(4).zip(state) {
        bytes.copy_from_slice(&word.to_be_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn secret_deny_fs_blocks_and_passes() {
        use crate::workspace_policy::WorkspacePolicy;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), b"TOKEN=abc").unwrap();
        let policy = Arc::new(WorkspacePolicy::contained(root));
        let fs = SecretDenyFs::new(RealFs, Arc::clone(&policy));

        assert!(matches!(
            fs.read(&root.join(".env")).await,
            Err(VfsError::SecretDenied { .. })
        ));
        assert!(matches!(
            fs.write(&root.join(".env"), b"x").await,
            Err(VfsError::SecretDenied { .. })
        ));
        fs.write(&root.join("main.rs"), b"fn main(){}")
            .await
            .unwrap();
        assert_eq!(
            fs.read(&root.join("main.rs")).await.unwrap(),
            b"fn main(){}"
        );
    }

    // Unix-only: exercises `std::os::unix::fs::symlink`. Gating the whole test
    // with `#[cfg(unix)]` (rather than an inner `#[cfg(not(unix))] return;`)
    // avoids an `unreachable_code` error on Windows under `-D warnings`.
    #[cfg(unix)]
    #[tokio::test]
    async fn secret_deny_catches_symlink_to_secret_when_inner() {
        // Load-bearing: SecretDenyFs must be layered INSIDE SandboxedFs so it
        // sees the canonical (symlink-resolved) path. A benign-named symlink
        // pointing at .env must be denied.
        use crate::workspace_policy::WorkspacePolicy;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join(".env"), b"TOKEN=abc").unwrap();
        std::os::unix::fs::symlink(root.join(".env"), root.join("notes.txt")).unwrap();

        let policy = Arc::new(WorkspacePolicy::contained(&root));
        let jail = SandboxedFs::new(SecretDenyFs::new(RealFs, Arc::clone(&policy)), root.clone());
        assert!(matches!(
            jail.read(&root.join("notes.txt")).await,
            Err(VfsError::SecretDenied { .. })
        ));
    }

    /// #667 Full-posture read path: `SecretDenyFs` installed WITHOUT a
    /// `SandboxedFs` jail (Full stays unconfined) denies the project's own
    /// `.env` but leaves a secret OUTSIDE the workspace root readable — the
    /// workspace-scoped `is_project_secret` predicate does the limiting.
    #[tokio::test]
    async fn full_posture_denies_project_secret_but_allows_host_secret() {
        use crate::workspace_policy::WorkspacePolicy;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join(".env"), b"PROJECT=secret").unwrap();
        std::fs::write(root.join("main.rs"), b"fn main() {}").unwrap();

        // A host secret OUTSIDE the workspace root.
        let host = tempfile::tempdir().unwrap();
        let host_root = std::fs::canonicalize(host.path()).unwrap();
        std::fs::write(host_root.join(".env"), b"HOST=secret").unwrap();

        // Full posture = trusted_local + channel/remote opt-in, no jail wrapper.
        let policy = Arc::new(WorkspacePolicy::trusted_local(&root).with_project_secret_deny());
        let fs = SecretDenyFs::new(RealFs, Arc::clone(&policy));

        assert!(
            matches!(
                fs.read(&root.join(".env")).await,
                Err(VfsError::SecretDenied { .. })
            ),
            "project .env must be denied on the read path"
        );
        assert_eq!(
            fs.read(&root.join("main.rs")).await.unwrap(),
            b"fn main() {}",
            "ordinary project file must still be readable"
        );
        assert_eq!(
            fs.read(&host_root.join(".env")).await.unwrap(),
            b"HOST=secret",
            "a host secret OUTSIDE the workspace root stays readable (Full = trusted-remote operator)"
        );
    }
}
