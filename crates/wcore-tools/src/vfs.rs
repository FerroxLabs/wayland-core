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
use std::ffi::CString;
#[cfg(any(unix, windows))]
use std::ffi::OsStr;
#[cfg(any(unix, windows))]
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
    /// The open did not land on the object the containment check approved.
    ///
    /// Raised by the handle-pinned read path (`vfs_pinned`) when the name it
    /// was handed resolves to something other than the ordinary file the jail
    /// cleared — a symlink, a directory, a device, or a parent directory that
    /// was replaced between the two steps. Distinct from `OutsideSandbox`
    /// because nothing about the REQUEST was out of bounds; the filesystem
    /// changed underneath it.
    #[error("refused: {path:?} did not resolve to the approved object: {reason}")]
    PathRaced { path: PathBuf, reason: String },
    #[error(
        "refused: {path:?} is inside the workspace's repository-control surface \
         (.git / .wayland-core), which the file tools may read but never write. \
         Use the Bash tool and a git command if this write is genuinely intended."
    )]
    RepoControlDenied { path: PathBuf },
    /// FerroxLabs/wayland#1096 direction 2 — a write aimed at a place skills
    /// are LOADED from. Distinct from [`Self::RepoControlDenied`] because the
    /// generic repo-control refusal names no destination, and the destination
    /// is the entire point: the model has produced a file and needs to be told
    /// where it belongs, not merely that this is not it.
    #[error(
        "refused: {path:?} is inside a skill SOURCE directory. Skills are \
         LOADED from there and never written to: a file left there sits outside \
         the session workspace, so the session that produced it cannot read it \
         back, and a SKILL.md left there is instruction injection into the next \
         session. Write files a skill produces to ${{WCORE_SKILL_OUTPUT_DIR}} \
         instead (<cwd>/.wayland-out/skills/<session_id>/)."
    )]
    SkillSourceDenied { path: PathBuf },
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

    /// Read a file with the check and the use bound to ONE kernel object
    /// (FerroxLabs/wayland#1105).
    ///
    /// `read` takes a pathname and resolves it. A caller that has already
    /// decided a path is permitted — `SandboxedFs`, which canonicalizes and
    /// compares against its root and its standing grants — therefore hands
    /// back a NAME, and the backend resolves it a second time. Anything that
    /// can write in the directory can swap the leaf between those two
    /// resolutions, and the approved object and the read object are then
    /// different objects.
    ///
    /// An implementor of this method must resolve the leaf exactly once,
    /// relative to a RETAINED parent directory handle, without following a
    /// symlink at the leaf or at the parent, and read the bytes from that same
    /// descriptor. Where that is impossible the default below applies and the
    /// jail refuses rather than falling back to `read` — a fallback would be a
    /// silent downgrade to the window this method exists to close.
    async fn read_pinned(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        Err(VfsError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "handle-pinned reads are not implemented for {}",
                path.display()
            ),
        )))
    }

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
    async fn read_pinned(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || crate::vfs_pinned::pinned_read_bytes(&path))
            .await
            .map_err(|error| VfsError::Io(io::Error::other(error)))?
    }
    async fn observe_file(&self, path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || observe_real_file(&path))
            .await
            .map_err(|error| VfsError::Io(io::Error::other(error)))?
    }
    /// #1155. Ordinary host paths CAN be compare-exchanged after all.
    ///
    /// This returned `Unsupported`, on the reasoning that a host filesystem
    /// "cannot protect a pathname from non-cooperating concurrent writers".
    /// That was true of every primitive this crate had, and it is what left
    /// the production Edit/Write path losing a save that landed inside the
    /// guard: measured at 140 of 200 interleaved saves destroyed.
    ///
    /// `wcore_config::atomic_write_checked` publishes with an atomic exchange
    /// (`renameat2(RENAME_EXCHANGE)` / `renamex_np(RENAME_SWAP)`) and hands
    /// back the bytes it displaced. Those bytes ARE the destination at the
    /// instant of publication, so comparing them to the precondition is a
    /// genuine content compare-and-swap against a writer that never agreed to
    /// cooperate.
    ///
    /// Two boundaries, stated rather than implied:
    ///
    /// * Only the CONTENT precondition is enforced atomically.
    ///   `expected_object` names one kernel object, which an exchange does not
    ///   compare; it is checked against a prior observation here, exactly as
    ///   `SandboxedFs` already checks it, and is documented on
    ///   [`IntendedFileMutation::from_observation`] as fixture-backend only.
    /// * A `FilePrecondition::Absent` create has nothing to exchange with, so
    ///   it degrades to observe-then-rename. #1155 is the overwrite race; the
    ///   create-vs-create race is narrower and is not closed here.
    async fn compare_exchange_file(
        &self,
        path: &Path,
        mutation: &IntendedFileMutation,
    ) -> Result<FileMutationOutcome, VfsError> {
        // A leaf this backend cannot OBSERVE cannot be compare-exchanged
        // either. The commonest case is a symlinked leaf: `observe_file` opens
        // it `O_NOFOLLOW` and fails with ELOOP, deliberately, because a symlink
        // has no stable object identity to bind a receipt to (see
        // `prepared_file_effects::a_symlinked_target_stays_opaque_and_still_writes`,
        // which also fixes that a write through the link must still succeed).
        //
        // Answering `Unsupported` puts the caller back on read-then-write,
        // which is precisely what such a path did before #1155. Nothing is
        // swallowed — that path opens the same object and reports any real
        // failure itself — but the #1155 race is NOT closed for a symlinked
        // leaf, and saying so here is the point of the branch.
        let Ok(observed) = self.observe_file(path).await else {
            return Err(VfsError::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "compare-exchange needs an observable leaf, and {} could not be observed",
                    path.display()
                ),
            )));
        };
        let current = observed.observation;

        // The non-atomic classification, in the order `InMemoryFs` uses, so
        // the two backends answer the same question the same way. Only the
        // content precondition is re-enforced atomically below; these decide
        // WHICH outcome a caller is told about, and refuse early.
        if !mutation.postcondition_authority_matches(&observed) {
            return Ok(FileMutationOutcome::Conflict { current });
        }
        if mutation.already_applied_matches(&observed) {
            return Ok(FileMutationOutcome::AlreadyApplied {
                current: mutation.intended,
            });
        }
        if current == FileObservation::Present(mutation.intended)
            || !mutation.precondition_matches(&observed)
        {
            return Ok(FileMutationOutcome::Conflict { current });
        }

        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent).await?;
        }

        let precondition = mutation.precondition;
        let intended = mutation.intended;
        let path_owned = path.to_path_buf();
        let data = mutation.contents().to_vec();
        let published = tokio::task::spawn_blocking(move || {
            wcore_config::atomic_write_checked(&path_owned, &data, |displaced| {
                if precondition.matches(observation_of(displaced)) {
                    Ok(())
                } else {
                    Err("the precondition no longer holds".to_owned())
                }
            })
        })
        .await
        .map_err(|error| VfsError::Io(io::Error::other(error)))??;

        match published {
            Ok(()) => Ok(FileMutationOutcome::Applied {
                previous: current,
                current: intended,
            }),
            // The publish was retracted: the destination held bytes the
            // precondition does not name. Report what is there NOW, not the
            // stale observation from before the exchange.
            Err(_) => Ok(FileMutationOutcome::Conflict {
                current: self.observe_file(path).await?.observation,
            }),
        }
    }
}

/// The observation a run of bytes represents, `None` meaning nothing was there.
fn observation_of(bytes: Option<&[u8]>) -> FileObservation {
    bytes.map_or(FileObservation::Absent, |bytes| {
        FileObservation::Present(FileContentIdentity::from_bytes(bytes))
    })
}

/// Does this error mean the backend has no compare-exchange, as opposed to a
/// compare-exchange that failed? Callers fall back to read-then-write on the
/// former and surface the latter.
///
/// One predicate rather than a `kind()` test repeated at each call site: the
/// default [`VirtualFs::compare_exchange_file`] is the only thing that defines
/// what "unsupported" looks like, and a caller that spelled it differently
/// would silently turn a real failure into a racy fallback.
#[must_use]
pub fn is_compare_exchange_unsupported(error: &VfsError) -> bool {
    matches!(error, VfsError::Io(io) if io.kind() == io::ErrorKind::Unsupported)
}

fn observe_real_file(path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
    #[cfg(unix)]
    {
        observe_real_file_unix(path)
    }
    #[cfg(windows)]
    {
        observe_real_file_windows(path)
    }
    #[cfg(not(any(unix, windows)))]
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

#[cfg(any(unix, windows))]
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
pub(crate) fn openat_file(
    parent: &fs::File,
    name: &OsStr,
    flags: i32,
    mode: u32,
) -> io::Result<fs::File> {
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

#[cfg(any(unix, windows))]
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

/// Windows half of [`observe_real_file`].
///
/// Structurally identical to [`observe_real_file_unix`], including its refusal
/// set: the parent directory is canonicalized and retained as a handle, the
/// leaf is opened RELATIVE to that retained handle, reparse points are opened
/// rather than followed, and a directory, a reparse point, or a multiply-linked
/// file is refused outright.
///
/// The relative open is `NtCreateFile` with `RootDirectory` set to the retained
/// parent handle — the only `openat` equivalent Windows offers, and the same
/// primitive `wcore_sandbox::DirectoryAuthority` already uses for its
/// handle-rooted child operations. Re-opening `canonical_parent.join(leaf)` by
/// pathname would have been shorter, but it would leave the parent-directory
/// substitution window that the recorded parent identity exists to close: the
/// bytes would come from one directory object and the parent token from
/// another. A receipt whose two halves can disagree is exactly the "matching
/// bytes alone resolved an uncertain effect" failure this module forbids.
#[cfg(windows)]
fn observe_real_file_windows(path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
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
            let parent = open_windows_directory(&parent_path)?;
            let metadata = parent.metadata()?;
            ensure_directory(&metadata, &parent_path)?;
            observe_windows_file(&parent, &parent_path, leaf)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let (anchor_path, suffix) = canonical_existing_ancestor(requested_parent, path)?;
            let anchor = open_windows_directory(&anchor_path)?;
            let metadata = anchor.metadata()?;
            ensure_directory(&metadata, &anchor_path)?;
            let resolved_parent = anchor_path.join(suffix);
            Ok(IdentifiedFileObservation {
                observation: FileObservation::Absent,
                object: FileObjectIdentity {
                    authority: windows_authority(&windows_object_identity(&anchor)?),
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

/// Open an already-canonical directory as a retained handle.
///
/// `FILE_FLAG_BACKUP_SEMANTICS` is what makes a directory openable at all on
/// Windows. `FILE_FLAG_OPEN_REPARSE_POINT` is defence in depth: `canonicalize`
/// has already resolved every reparse point, so a reparse point observed here
/// means one was planted between the two calls — opening the link itself turns
/// that into a refusal (the `is_dir` check below fails) instead of a silent
/// traversal.
#[cfg(windows)]
pub(crate) fn open_windows_directory(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// Everything the identity token and the type refusals are derived from, read
/// once from a single retained handle.
#[cfg(windows)]
pub(crate) struct WindowsObjectIdentity {
    volume_serial: u32,
    file_index: u64,
    /// 128-bit `FILE_ID_INFO` identity. `None` where the volume cannot serve it
    /// (`GetFileInformationByHandleEx(FileIdInfo)` is not universal); the token
    /// records that explicitly so a receipt written with one is never silently
    /// compared against a receipt written without.
    file_id: Option<[u8; 16]>,
    pub(crate) attributes: u32,
    links: u32,
}

#[cfg(windows)]
pub(crate) fn windows_object_identity(
    handle: &fs::File,
) -> Result<WindowsObjectIdentity, VfsError> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx,
    };

    // SAFETY: `BY_HANDLE_FILE_INFORMATION` is plain-old-data with no invalid
    // bit patterns, and it is fully written by the call below before it is read.
    let mut information = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    // SAFETY: `handle` keeps the OS handle valid for the call and `information`
    // is a writable, correctly sized output buffer.
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut information) } == 0 {
        return Err(VfsError::Io(io::Error::last_os_error()));
    }

    // SAFETY: `FILE_ID_INFO` is plain-old-data with no invalid bit patterns.
    let mut id_information = unsafe { std::mem::zeroed::<FILE_ID_INFO>() };
    // SAFETY: `handle` stays valid for the call; the pointer/length pair
    // describes exactly the `FILE_ID_INFO` the kernel writes.
    let has_file_id = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileIdInfo,
            std::ptr::addr_of_mut!(id_information).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } != 0;

    Ok(WindowsObjectIdentity {
        volume_serial: information.dwVolumeSerialNumber,
        file_index: (u64::from(information.nFileIndexHigh) << 32)
            | u64::from(information.nFileIndexLow),
        file_id: has_file_id.then_some(id_information.FileId.Identifier),
        attributes: information.dwFileAttributes,
        links: information.nNumberOfLinks,
    })
}

/// The Windows analogue of `real_fs_authority`.
///
/// Unix anchors the authority to the root filesystem's `dev`/`ino`. Windows has
/// no single root, so the volume the observation was taken on plays that role:
/// a receipt prepared on one volume can never be reconciled against a same-named
/// path on another.
#[cfg(windows)]
fn windows_authority(identity: &WindowsObjectIdentity) -> String {
    format!("realfs:windows:{}", identity.volume_serial)
}

/// Windows analogue of `unix_identity_token`.
///
/// `FILE_ATTRIBUTE_ARCHIVE` and `FILE_ATTRIBUTE_NOT_CONTENT_INDEXED` are masked
/// out deliberately. Unix folds `mode`/`uid`/`gid` into its token because those
/// are stable, security-relevant properties of the object. The two Windows bits
/// masked here are neither: `ARCHIVE` is set by any ordinary write and cleared
/// by any backup agent, and `NOT_CONTENT_INDEXED` is toggled by the search
/// indexer — both flip on a file no user or agent touched, which would turn
/// routine background activity into spurious `Conflict` reconciliations.
#[cfg(windows)]
fn windows_identity_token(identity: &WindowsObjectIdentity) -> String {
    const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0000_0020;
    const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: u32 = 0x0000_2000;

    let file_id = match identity.file_id {
        Some(bytes) => format!("{:032x}", u128::from_be_bytes(bytes)),
        None => "none".to_owned(),
    };
    format!(
        "windows-v1:{}:{}:{}:{}:{:x}",
        identity.volume_serial,
        identity.file_index,
        file_id,
        identity.links,
        identity.attributes & !(FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED),
    )
}

/// Direct proof for [`windows_identity_token`], which no filesystem-level test
/// can force.
///
/// Two of the five inputs to the token are attribute bits that flip on a file
/// nobody touched, and the token is not allowed to notice them; every other
/// input IS an identity and the token must notice all of them. A test that
/// only writes real files cannot force `ARCHIVE` off, cannot choose a volume
/// serial, and cannot make a volume decline `FILE_ID_INFO` — so a token that
/// masked the wrong bits, or that collapsed the "this volume has no file id"
/// case onto a real all-zero id, would pass everything else in this crate.
#[cfg(all(windows, test))]
mod windows_identity_token_tests {
    use super::{WindowsObjectIdentity, windows_authority, windows_identity_token};

    const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
    const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0000_0020;
    const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: u32 = 0x0000_2000;

    fn identity() -> WindowsObjectIdentity {
        WindowsObjectIdentity {
            volume_serial: 0xDEAD_BEEF,
            file_index: 0x0000_1234_5678_9ABC,
            file_id: Some([7_u8; 16]),
            attributes: FILE_ATTRIBUTE_ARCHIVE,
            links: 1,
        }
    }

    #[test]
    fn token_ignores_only_the_two_incidental_attribute_bits() {
        let base = identity();
        let mut quiesced = identity();
        quiesced.attributes = 0;
        let mut indexed = identity();
        indexed.attributes = FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED;

        assert_eq!(
            windows_identity_token(&base),
            windows_identity_token(&quiesced),
            "a backup agent clearing FILE_ATTRIBUTE_ARCHIVE must not read as a \
             different object"
        );
        assert_eq!(
            windows_identity_token(&base),
            windows_identity_token(&indexed),
            "the search indexer toggling FILE_ATTRIBUTE_NOT_CONTENT_INDEXED \
             must not read as a different object"
        );
    }

    #[test]
    fn token_notices_every_attribute_bit_that_is_not_masked() {
        let base = identity();
        let mut readonly = identity();
        readonly.attributes |= FILE_ATTRIBUTE_READONLY;

        assert_ne!(
            windows_identity_token(&base),
            windows_identity_token(&readonly),
            "READONLY is a real, security-relevant property of the object and \
             must not be masked away with the incidental bits"
        );
    }

    #[test]
    fn token_notices_each_identity_input_independently() {
        let base = identity();
        let base_token = windows_identity_token(&base);

        let mut other_volume = identity();
        other_volume.volume_serial = 0x1234_5678;
        let mut other_index = identity();
        other_index.file_index = 0x0000_1234_5678_9ABD;
        let mut other_file_id = identity();
        other_file_id.file_id = Some([8_u8; 16]);
        let mut other_links = identity();
        other_links.links = 2;

        for (name, changed) in [
            ("volume serial", other_volume),
            ("file index", other_index),
            ("file id", other_file_id),
            ("link count", other_links),
        ] {
            assert_ne!(
                base_token,
                windows_identity_token(&changed),
                "the token must distinguish objects differing only in {name}"
            );
        }
    }

    #[test]
    fn absent_file_id_never_collides_with_a_real_one() {
        let mut unavailable = identity();
        unavailable.file_id = None;
        let mut zeroed = identity();
        zeroed.file_id = Some([0_u8; 16]);

        assert_ne!(
            windows_identity_token(&unavailable),
            windows_identity_token(&zeroed),
            "\"this volume cannot serve FILE_ID_INFO\" must be recorded \
             explicitly; folding it onto an all-zero id would let a receipt \
             written without a file id be compared against one written with"
        );
    }

    /// The counted name handed to `NtCreateFile` with a `RootDirectory` must
    /// be exactly one ordinary component. Anything else would reintroduce the
    /// ambient pathname walk the retained handle exists to avoid, so it has to
    /// be refused BEFORE the kernel sees it.
    #[test]
    fn relative_open_refuses_anything_that_is_not_one_plain_component() {
        use super::{open_windows_child_no_follow, open_windows_directory};
        use std::ffi::OsStr;

        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("leaf.txt"), b"reachable").unwrap();
        std::fs::write(root.join("plain.txt"), b"plain").unwrap();
        let directory = open_windows_directory(&root).unwrap();

        for escaping in [
            r"sub\leaf.txt",
            "sub/leaf.txt",
            "..",
            r"..\plain.txt",
            ".",
            r"C:\Windows\System32\drivers\etc\hosts",
            "",
        ] {
            let error = open_windows_child_no_follow(&directory, OsStr::new(escaping))
                .err()
                .unwrap_or_else(|| panic!("{escaping:?} must be refused, not opened"));
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::InvalidInput,
                "{escaping:?} must be refused as malformed input, not passed to the kernel"
            );
        }

        // Positive control: the same call opens an ordinary single component,
        // so the refusals above are the guard talking and not a broken open.
        open_windows_child_no_follow(&directory, OsStr::new("plain.txt"))
            .expect("a plain single-component name must still open");
    }

    #[test]
    fn authority_is_volume_scoped() {
        let base = identity();
        let mut other_volume = identity();
        other_volume.volume_serial = 0x1234_5678;

        assert_ne!(
            windows_authority(&base),
            windows_authority(&other_volume),
            "Windows has no single filesystem root, so the volume plays the \
             role unix gives the root device: a receipt prepared on one volume \
             must never reconcile against a same-named path on another"
        );
        assert_eq!(windows_authority(&base), windows_authority(&identity()));
    }
}

/// Open one direct child of a RETAINED directory handle without following a
/// reparse point and without re-walking any pathname.
///
/// `NtCreateFile` resolves `ObjectName` inside `RootDirectory` only, which is
/// what makes this the `openat` equivalent. `FILE_NON_DIRECTORY_FILE` makes the
/// kernel refuse a directory at open time and `FILE_OPEN_REPARSE_POINT` opens a
/// symlink/junction as itself so the caller's attribute check can refuse it.
#[cfg(windows)]
pub(crate) fn open_windows_child_no_follow(
    parent: &fs::File,
    leaf: &OsStr,
) -> io::Result<fs::File> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        NtCreateFile,
    };
    use windows_sys::Win32::Foundation::{HANDLE, RtlNtStatusToDosError, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, SYNCHRONIZE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    const OBJ_CASE_INSENSITIVE: u32 = 0x40;

    // A counted name handed to `NtCreateFile` with a `RootDirectory` must be
    // exactly one ordinary component. A separator, a drive prefix or a `..`
    // would reintroduce the ambient walk this function exists to avoid, so
    // refuse rather than pass it to the kernel.
    let mut components = Path::new(leaf).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file observation requires a single-component file name: {leaf:?}"),
        ));
    }

    let mut wide: Vec<u16> = leaf.encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "filesystem name contains an embedded NUL",
        ));
    }
    let byte_len = wide
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file name is too long"))?;
    let unicode_name = UNICODE_STRING {
        Length: byte_len,
        MaximumLength: byte_len,
        Buffer: wide.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &unicode_name,
        // Matching the volume's own rule is load-bearing in BOTH directions: on
        // an ordinary NTFS directory, omitting the flag would fail to find a
        // differently-cased name the Win32 layer resolves fine; on a
        // per-directory case-sensitive one (WSL), setting it would let a
        // differently-cased sibling answer for the requested name.
        Attributes: if windows_directory_is_case_sensitive(parent) {
            0
        } else {
            OBJ_CASE_INSENSITIVE
        },
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    // SAFETY: an out-parameter fully written by `NtCreateFile` before it is read.
    let mut status_block = unsafe { std::mem::zeroed::<IO_STATUS_BLOCK>() };
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: the retained parent handle, the counted name buffer (`wide` is
    // still owned and alive) and every out-parameter stay valid for the call.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            FILE_GENERIC_READ | SYNCHRONIZE,
            &attributes,
            &mut status_block,
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        // SAFETY: translating an NTSTATUS has no pointer preconditions. The DOS
        // mapping is what gives the caller a real `ErrorKind::NotFound` for an
        // absent child rather than an opaque negative status.
        let code = unsafe { RtlNtStatusToDosError(status) };
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    if handle.is_null() {
        return Err(io::Error::other(
            "NtCreateFile succeeded without returning a handle",
        ));
    }
    // SAFETY: a successful `NtCreateFile` transfers ownership of exactly one
    // handle, which `File` now solely owns and closes.
    Ok(unsafe { fs::File::from_raw_handle(handle) })
}

/// Whether per-directory case sensitivity is enabled on this directory.
///
/// A volume that cannot answer (`FileCaseSensitiveInfo` predates neither every
/// Windows build nor every filesystem) is reported as case-INSENSITIVE, which
/// is both the Windows default and exactly what a `Win32` path open would have
/// done — so an unanswerable probe never resolves a name the ordinary API would
/// have refused.
#[cfg(windows)]
fn windows_directory_is_case_sensitive(directory: &fs::File) -> bool {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FileCaseSensitiveInfo, GetFileInformationByHandleEx,
    };

    const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 0x1;

    #[repr(C)]
    struct FileCaseSensitiveInformation {
        flags: u32,
    }

    let mut information = FileCaseSensitiveInformation { flags: 0 };
    // SAFETY: `directory` keeps the handle valid for the call; the pointer and
    // length describe exactly the structure the kernel writes.
    let answered = unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle(),
            FileCaseSensitiveInfo,
            std::ptr::addr_of_mut!(information).cast(),
            std::mem::size_of::<FileCaseSensitiveInformation>() as u32,
        )
    } != 0;
    answered && information.flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0
}

#[cfg(windows)]
fn observe_windows_file(
    parent: &fs::File,
    parent_path: &Path,
    leaf: &OsStr,
) -> Result<IdentifiedFileObservation, VfsError> {
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

    let parent_identity = windows_object_identity(parent)?;
    let object_path = parent_path.join(leaf);
    let authority = windows_authority(&parent_identity);
    let parent_token = Some(windows_identity_token(&parent_identity));

    let mut file = match open_windows_child_no_follow(parent, leaf) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(IdentifiedFileObservation {
                observation: FileObservation::Absent,
                object: FileObjectIdentity {
                    authority,
                    path: object_path,
                    parent: parent_token,
                    file: None,
                },
                contents: None,
            });
        }
        Err(error) => return Err(VfsError::Io(error)),
    };
    let identity = windows_object_identity(&file)?;
    if identity.attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || identity.links != 1
    {
        return Err(VfsError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "CAS target must be a singly-linked regular file",
        )));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(IdentifiedFileObservation {
        observation: FileObservation::Present(FileContentIdentity::from_bytes(&bytes)),
        object: FileObjectIdentity {
            authority,
            path: object_path,
            parent: parent_token,
            file: Some(windows_identity_token(&identity)),
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
    /// An in-memory map has no kernel objects and no pathname walk, so the key
    /// lookup IS the pinned read. Load-bearing rather than decorative:
    /// `SandboxedFs<InMemoryFs>` is a real configuration in the test suite, and
    /// leaving it on the trait default would make every jailed read there
    /// refuse.
    async fn read_pinned(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        self.read(path).await
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
    /// Standing path grants ("always allow this folder"), shared live with the
    /// session's `WorkspacePolicy` rather than copied — a grant must take
    /// effect on the very next call, not on the next session.
    ///
    /// EVERY grant confers read, on the pure-read operations. Only a grant
    /// carrying `write` widens the mutating four (`write`, `remove_file`,
    /// `observe_file`, `compare_exchange_file`), and it is minted under
    /// strictly more refusals than a read grant — see
    /// `WorkspacePolicy::check_write_grantable` (#1104). The asymmetry is
    /// enforced HERE by asking two different questions
    /// ([`contain_read`](Self::contain_read) vs
    /// [`contain_write`](Self::contain_write)), not by trusting the caller to
    /// pass the right list.
    path_grants: Arc<RwLock<Vec<crate::workspace_policy::SessionPathGrant>>>,
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
        Self {
            inner,
            root,
            path_grants: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Share the session's standing path grants with this jail.
    ///
    /// Takes the `Arc` from
    /// `WorkspacePolicy::session_path_grant_handle` — the same allocation, so
    /// a grant approved mid-session is visible here immediately. Without this
    /// the user approves "always allow this folder" and `Read` keeps refusing,
    /// because the OS sandbox and the in-process file tools would be reading
    /// two different answers to the same question.
    #[must_use]
    pub fn with_path_grants(
        mut self,
        grants: Arc<RwLock<Vec<crate::workspace_policy::SessionPathGrant>>>,
    ) -> Self {
        self.path_grants = grants;
        self
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

        // Step 2 above resolves symlinks, and step 3 argues that the
        // not-yet-existing suffix is safe because "no symlink can escape
        // through a not-yet-created node". A DANGLING symlink is the node that
        // argument does not cover: it EXISTS, `canonicalize` refuses it only
        // because its target does not, so it lands in `suffix` and is never
        // re-examined. `landing_prefix` follows exactly that one node and
        // reports where the operation would really act.
        let Some(landing) = landing_prefix(&canon_prefix, &suffix).await else {
            return Err(VfsError::OutsideSandbox {
                path: normalized,
                root: self.root.clone(),
            });
        };

        if !landing.starts_with(&self.root) {
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

    /// Containment for the pure-READ operations: inside the sandbox root, OR
    /// inside any folder the user explicitly granted.
    async fn contain_read(&self, path: &Path) -> Result<PathBuf, VfsError> {
        self.contain_granted(path, false).await
    }

    /// Containment for the MUTATING operations: inside the sandbox root, or
    /// inside a folder granted with WRITE.
    ///
    /// #1104. Before this, all four mutating operations used the bare
    /// [`contain`](Self::contain), because write outside the workspace was not
    /// grantable at all. They now ask the same question `contain_read` asks
    /// with the access raised, so a plain read grant still refuses every one of
    /// them — the DoD's "a read-only grant on the same folder still refuses the
    /// write" is this one boolean, and it is checked in exactly one place.
    async fn contain_write(&self, path: &Path) -> Result<PathBuf, VfsError> {
        self.contain_granted(path, true).await
    }

    /// Falls through to [`contain`](Self::contain) first so the ordinary
    /// in-workspace path is byte-for-byte unchanged, and so a session with no
    /// grants behaves exactly as it did before this existed. The grant check
    /// runs on the CANONICALIZED path for the same reason `contain` does:
    /// otherwise `<granted>/link -> /etc/shadow` would pass.
    async fn contain_granted(&self, path: &Path, write: bool) -> Result<PathBuf, VfsError> {
        let refusal = match self.contain(path).await {
            Ok(contained) => return Ok(contained),
            Err(refusal) => refusal,
        };
        // Only an out-of-root refusal is a candidate for a grant. A secret
        // denial or an I/O error is not something "always allow this folder"
        // is entitled to override.
        let VfsError::OutsideSandbox {
            path: attempted, ..
        } = &refusal
        else {
            return Err(refusal);
        };
        // Expiry is evaluated HERE, at use time, not when the grant was made:
        // a long-running turn must lose access the moment the deadline passes,
        // not at whatever later point something happens to rebuild a sandbox.
        let now = std::time::SystemTime::now();
        let grants = self.live_grant_roots(now, write);
        if grants.is_empty() {
            return Err(refusal);
        }
        let Some((canon_prefix, suffix)) = canonicalize_existing_prefix(attempted).await else {
            return Err(refusal);
        };
        // Same dangling-boundary resolution the jail check applies. A grant
        // that could be stepped out of by a spelling the jail refuses would be
        // a hole opened BY the grant, which is the worst outcome available
        // here.
        let Some(landing) = landing_prefix(&canon_prefix, &suffix).await else {
            return Err(refusal);
        };
        if !grants.iter().any(|root| landing.starts_with(root)) {
            return Err(refusal);
        }
        // A grant widens WHERE we may look, never WHAT. The secret rules are
        // not a property of the workspace, they are a property of the file, so
        // an `id_rsa` or a `.env` sitting inside an otherwise perfectly
        // reasonable granted folder stays refused. Checked lexically on the
        // canonical path, so a benign-named symlink to a secret is caught and
        // a secret created after the grant is caught too — there is no walk to
        // go stale.
        let target = if suffix.as_os_str().is_empty() {
            canon_prefix.clone()
        } else {
            canon_prefix.join(&suffix)
        };
        if crate::workspace_policy::is_secret_path_static(&target) {
            return Err(VfsError::SecretDenied { path: target });
        }
        if suffix.as_os_str().is_empty() {
            Ok(canon_prefix)
        } else {
            Ok(canon_prefix.join(suffix))
        }
    }

    /// The roots of every live grant conferring at least `write`.
    ///
    /// Expiry is evaluated HERE, at use time, not when the grant was made: a
    /// long-running turn must lose access the moment the deadline passes, not
    /// at whatever later point something happens to rebuild a sandbox.
    fn live_grant_roots(&self, now: std::time::SystemTime, write: bool) -> Vec<PathBuf> {
        self.path_grants
            .read()
            .iter()
            .filter(|grant| {
                grant.expires_at.is_none_or(|deadline| now < deadline) && (!write || grant.write)
            })
            .map(|grant| grant.root.clone())
            .collect()
    }

    /// Bind an observed object to the authority that vouches for it.
    ///
    /// The authority is what `compare_exchange_file` compares before applying a
    /// mutation, so it must name the boundary the object actually sits behind.
    /// #1104: an object inside a granted WRITE root is bound to THAT root's
    /// identity, not the jail's. Binding it to the jail root would claim the
    /// workspace vouches for a file that is not in it, and two different
    /// granted roots would then be indistinguishable — a mutation prepared
    /// against one could be applied to the other.
    ///
    /// A path reachable through no live write grant is refused, exactly as
    /// before: this is the mutating path's containment check, and it must not
    /// be widened by a plain READ grant.
    fn bind_identity(
        &self,
        mut object: FileObjectIdentity,
    ) -> Result<FileObjectIdentity, VfsError> {
        let authority_root = if object.path.starts_with(&self.root) {
            self.root.clone()
        } else {
            let now = std::time::SystemTime::now();
            self.live_grant_roots(now, true)
                .into_iter()
                .find(|granted| object.path.starts_with(granted))
                .ok_or_else(|| VfsError::OutsideSandbox {
                    path: object.path.clone(),
                    root: self.root.clone(),
                })?
        };
        object.authority = format!(
            "sandbox:{}|{}",
            sandbox_root_identity(&authority_root)?,
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

/// How many dangling-symlink hops [`landing_prefix`] will follow.
///
/// A chain longer than this is not a path anybody meant, and refusing is the
/// only safe answer: "could not resolve" must never be treated as "resolves
/// inside". `SYMLOOP_MAX` is 8 on Linux and 32 on macOS; this is the tighter
/// of the two on purpose, because this is a boundary check and not a resolver.
const MAX_DANGLING_HOPS: usize = 8;

/// The canonical directory an operation on `canon_prefix + suffix` will really
/// act in.
///
/// Returns `canon_prefix` unchanged in the ordinary case — the first
/// not-yet-existing component genuinely does not exist, and the containment
/// reasoning in [`SandboxedFs::contain`] holds as written. It differs only when
/// that component is a DANGLING symlink, in which case the operation is
/// redirected wherever the link points and the caller must judge THAT.
///
/// Only the FIRST suffix component can be such a node: everything deeper has a
/// non-existent parent and therefore cannot itself exist.
///
/// `None` means the chain could not be resolved inside the hop budget. The
/// caller refuses on `None`.
///
/// MEASURED before this existed: `RealFs::write` goes through
/// `wcore_config::atomic_write`, which writes a tempfile and RENAMES over the
/// destination, so it replaces the dangling link's own dentry instead of
/// following it — no bytes escaped. That containment is a property of one
/// backend's write strategy, not of the boundary, and `observe_file` and any
/// future `VirtualFs` implementor do not share it. The check belongs at the
/// boundary.
async fn landing_prefix(canon_prefix: &Path, suffix: &Path) -> Option<PathBuf> {
    let Some(first) = suffix.components().next() else {
        return Some(canon_prefix.to_path_buf());
    };
    let mut node = canon_prefix.join(first.as_os_str());
    for _ in 0..MAX_DANGLING_HOPS {
        match tokio::fs::symlink_metadata(&node).await {
            // Not a link (or gone): whatever prefix contains it is the landing
            // point, and it is judged by the longest ancestor that does exist.
            Ok(metadata) if !metadata.is_symlink() => {
                return canonicalize_existing_prefix(&node)
                    .await
                    .map(|(prefix, _)| prefix);
            }
            Ok(_) => {}
            Err(_) => {
                return canonicalize_existing_prefix(&node)
                    .await
                    .map(|(prefix, _)| prefix);
            }
        }
        let target = tokio::fs::read_link(&node).await.ok()?;
        node = if target.is_absolute() {
            target
        } else {
            // A relative link is relative to the directory HOLDING it, not to
            // the process cwd — resolving it against anything else is how a
            // link that stays inside gets misread as one that leaves.
            lex_normalize(&target, node.parent().unwrap_or(Path::new("/")))
        };
    }
    None
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
    /// Reads go through [`VirtualFs::read_pinned`], never `read` (#1105).
    ///
    /// `contain_read` decides about the OBJECT it canonicalized; handing the
    /// resulting NAME to a path-based `read` lets the backend resolve it a
    /// second time, and anything able to write in the directory can swap the
    /// leaf in between. Measured on this tree before the change: 30,241 of
    /// 85,051 jailed reads returned bytes from outside the granted root while
    /// a second thread flipped the leaf between a regular file and a symlink.
    ///
    /// There is deliberately NO fallback to `self.inner.read` when a backend
    /// answers `Unsupported`. A fallback would be a silent downgrade back to
    /// that window, and it would be invisible because the read would still
    /// succeed. An out-of-tree `VirtualFs` implementor therefore has to
    /// implement `read_pinned` to be readable through a jail.
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        let p = self.contain_read(path).await?;
        self.inner.read_pinned(&p).await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError> {
        let p = self.contain_write(path).await?;
        self.inner.write(&p, contents).await
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        let p = self.contain_read(path).await?;
        self.inner.exists(&p).await
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        let p = self.contain_read(dir).await?;
        self.inner.list(&p).await
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        let p = self.contain_write(path).await?;
        self.inner.remove_file(&p).await
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        let p = self.contain_read(path).await?;
        self.inner.metadata(&p).await
    }
    async fn observe_file(&self, path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
        let p = self.contain_write(path).await?;
        let mut observed = self.inner.observe_file(&p).await?;
        observed.object = self.bind_identity(observed.object)?;
        Ok(observed)
    }
    async fn compare_exchange_file(
        &self,
        path: &Path,
        mutation: &IntendedFileMutation,
    ) -> Result<FileMutationOutcome, VfsError> {
        let p = self.contain_write(path).await?;
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
        // core#244 / core#322: a VCS CONTENT store is refused alongside the
        // secret-NAME predicate. `is_project_secret` matches names, and an
        // object file is named after its hash, so `.git/objects/ab/cdef...`
        // sailed through the in-process layer while `Bash` was already denied
        // the same bytes by `WorkspacePolicy::secret_deny_paths_dynamic`. This
        // call site is the whole of the in-process wiring: remove it and the
        // predicate still answers correctly and still denies nothing.
        if self.policy.is_project_secret(path) || self.policy.is_vcs_content_store(path) {
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
    /// Forwards the pin so the workspace posture — `SandboxedFs<SecretDenyFs
    /// <RealFs>>` — keeps reaching `RealFs::read_pinned`. Without this the
    /// layer in the middle would swallow the capability and every jailed read
    /// in the default posture would refuse.
    async fn read_pinned(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        self.guard(path)?;
        self.inner.read_pinned(path).await
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

/// Wraps a `VirtualFs` and refuses any MUTATION whose path is inside the
/// workspace's repository-control surface, per
/// [`WorkspacePolicy::is_repo_control_path`](crate::workspace_policy::WorkspacePolicy::is_repo_control_path).
///
/// Deliberately asymmetric with [`SecretDenyFs`], which guards every method: a
/// secret must not be READ, whereas `.git` and `.wayland-core` must not be
/// WRITTEN. Reading them is ordinary work — the skill loader reads
/// `.wayland-core/skills/**` on every boot, and `Read`ing `.git/HEAD` is a
/// perfectly normal thing for the model to do — so `read` / `exists` / `list` /
/// `metadata` / `observe_file` pass straight through and only `write`,
/// `remove_file` and `compare_exchange_file` are gated.
///
/// Installed for EVERY session, trusted and contained alike. That is the point:
/// the strict profile already denies `.git/config` and `.git/hooks/` through
/// `SecretDenyFs`'s secret-suffix list, but the trusted local profile installs
/// no VFS wrapper at all, so it is precisely the everyday local session that
/// could `Write` its own `.git/hooks/pre-commit`.
///
/// Layered INSIDE `SandboxedFs` where a jail exists, for the same reason
/// `SecretDenyFs` is: the jail hands down the canonicalized path. The guard
/// canonicalizes independently as well (`is_repo_control_path` does), so the
/// unjailed trusted deployment is equally symlink-safe.
pub struct RepoControlDenyFs<F: VirtualFs> {
    inner: F,
    policy: std::sync::Arc<crate::workspace_policy::WorkspacePolicy>,
}

impl<F: VirtualFs> RepoControlDenyFs<F> {
    pub fn new(inner: F, policy: std::sync::Arc<crate::workspace_policy::WorkspacePolicy>) -> Self {
        Self { inner, policy }
    }
    fn guard(&self, path: &Path) -> Result<(), VfsError> {
        // Skill-source FIRST: `<root>/.wayland-core/skills` satisfies both
        // predicates, and of the two refusals only this one tells the author
        // where the file should have gone (#1096 direction 2). Checking
        // repo-control first would leave the project-level load path with a
        // message that names no destination.
        if self.policy.is_skill_source_path(path) {
            return Err(VfsError::SkillSourceDenied {
                path: path.to_path_buf(),
            });
        }
        if self.policy.is_repo_control_path(path) {
            return Err(VfsError::RepoControlDenied {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl<F: VirtualFs + 'static> VirtualFs for RepoControlDenyFs<F> {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        self.inner.read(path).await
    }
    /// Forwards the pin, for exactly the reason `SecretDenyFs::read_pinned`
    /// gives: this type is a MIDDLE layer of
    /// `SandboxedFs<RepoControlDenyFs<SecretDenyFs<RealFs>>>`, and a middle
    /// layer that does not forward swallows the capability. The trait default
    /// refuses rather than falling back to `read` — deliberately, so the
    /// TOCTOU window stays closed — so without this every jailed read in the
    /// workspace posture fails, not just reads of the repo-control surface.
    ///
    /// No `guard` here on purpose: `.git` / `.wayland-core` are write-denied,
    /// never read-denied. Guarding would deny reads this type exists to allow.
    async fn read_pinned(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        self.inner.read_pinned(path).await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError> {
        self.guard(path)?;
        self.inner.write(path, contents).await
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        self.inner.exists(path).await
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        self.inner.list(dir).await
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        self.guard(path)?;
        self.inner.remove_file(path).await
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        self.inner.metadata(path).await
    }
    async fn observe_file(&self, path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
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
    fn root(&self) -> Option<&Path> {
        self.inner.root()
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
