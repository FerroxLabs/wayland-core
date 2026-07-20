//! WorktreeManager — git worktree create/cleanup for swarm workers.
//!
//! All git subprocess calls flow through
//! [`wcore_config::shell::shell_command_argv`] (argv mode — no shell
//! interpretation), per AGENTS.md cross-platform rules. Working directory
//! is set with `.current_dir(...)` on the returned `tokio::process::Command`.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use wcore_config::shell;
use wcore_sandbox::process_capture::{CaptureLimits, ProcessCaptureError, capture_bounded_process};
use wcore_sandbox::{DirectoryHandleLoan, RegularFileAuthority};

use crate::error::{Result, SwarmError};

#[path = "worktree_security.rs"]
mod security;
use security::{
    DirectoryAuthority, DirectoryAuthorityIdentity, ensure_absent_destination,
    ensure_real_directory, ensure_unchanged_real_directory, is_real_directory_entry,
    make_guard_dir_private, reject_option_like_ref, validate_worker_id, write_empty_private_config,
};

/// Manages the `<repo>/.swarm-worktrees/` directory and per-worker
/// worktrees within it. Each worker gets a fresh checkout at
/// `<repo>/.swarm-worktrees/<worker_id>` on a branch named by
/// [`super::SwarmBrief::worker_branch_prefix`] + `/` + `worker_id`.
pub struct WorktreeManager {
    repo_root: PathBuf,
    repo_authority: DirectoryAuthority,
    swarm_root: PathBuf,
    swarm_authority: DirectoryAuthority,
    swarm_parent: PathBuf,
    git_program: String,
    capture_limits: CaptureLimits,
    _git_guard_dir: tempfile::TempDir,
    empty_git_config: PathBuf,
    disabled_hooks: PathBuf,
    control_root: PathBuf,
    admission_lock: Mutex<()>,
    active_reservations: ActiveReservationRegistry,
    #[cfg(test)]
    ambient_git_env: Vec<(String, std::ffi::OsString)>,
    #[cfg(test)]
    git_prefix_args: Vec<String>,
}

/// Parent-issued storage proof for one delegated mutation checkout.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceCapacity {
    pub available_bytes: u64,
    pub safety_margin_bytes: u64,
    pub max_transaction_bytes: u64,
    pub max_aggregate_bytes: u64,
}

/// Identity-bound roots owned by one delegated mutation transaction.
#[derive(Clone)]
pub struct TransactionWorkspace {
    pub owner: String,
    pub root: PathBuf,
    pub checkout: PathBuf,
    pub scratch: PathBuf,
    pub base_commit: String,
    pub head_commit: String,
    pub tree: String,
    pub reserved_bytes: u64,
    authorities: Arc<TransactionWorkspaceAuthorities>,
    cleanup: Arc<TransactionCleanup>,
}

struct TransactionWorkspaceAuthorities {
    checkout: DirectoryAuthority,
    scratch: DirectoryAuthority,
    reservation: Arc<RegularFileAuthority>,
}

#[derive(Clone)]
struct ActiveReservation {
    root_identity: DirectoryAuthorityIdentity,
    authority: Arc<RegularFileAuthority>,
    bytes: u64,
}

type ActiveReservationRegistry = Arc<StdMutex<HashMap<String, ActiveReservation>>>;

const RESERVATION_FILE: &str = ".wayland-reservation";
const LEASE_FILE: &str = ".wayland-active-lease";
const CONTROL_DIR: &str = ".wayland-control";
const WORKSPACE_SAFETY_MARGIN_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TRANSACTION_WORKSPACE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_AGGREGATE_WORKSPACE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_RESERVATION_FILE_BYTES: u64 = 64;

struct ActiveLease {
    release: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl ActiveLease {
    fn acquire(file: DirectoryHandleLoan) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("wayland-swarm-lease".to_owned())
            .spawn(move || {
                let result: std::io::Result<()> = (|| {
                    let mut lock = fd_lock::RwLock::new(file);
                    let guard = lock.write()?;
                    ready_tx.send(Ok(())).map_err(|_| {
                        std::io::Error::new(ErrorKind::BrokenPipe, "lease owner disappeared")
                    })?;
                    let _ = release_rx.recv();
                    drop(guard);
                    Ok(())
                })();
                if let Err(error) = result {
                    let _ = ready_tx.send(Err(error.to_string()));
                }
            })
            .map_err(|error| SwarmError::WorktreeIo(format!("active lease: {error}")))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                release: Some(release_tx),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(SwarmError::WorktreeIo(format!("active lease: {error}")))
            }
            Err(error) => {
                let _ = thread.join();
                Err(SwarmError::WorktreeIo(format!("active lease: {error}")))
            }
        }
    }

    fn close(&mut self) {
        self.release.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ActiveLease {
    fn drop(&mut self) {
        self.close();
    }
}

struct TransactionCleanup {
    owner: String,
    root: PathBuf,
    root_authority: StdMutex<Option<DirectoryAuthority>>,
    swarm_root: PathBuf,
    swarm_authority: DirectoryAuthority,
    quarantine_root: PathBuf,
    quarantine_authority: DirectoryAuthority,
    reservation_authority: Arc<RegularFileAuthority>,
    reserved_bytes: u64,
    active_reservations: ActiveReservationRegistry,
    release_lock: StdMutex<()>,
    lease: StdMutex<Option<ActiveLease>>,
    released: AtomicBool,
}

impl TransactionCleanup {
    fn root_authority(&self) -> Result<DirectoryAuthority> {
        self.root_authority
            .lock()
            .map_err(|_| {
                SwarmError::WorktreeIo("transaction root authority is poisoned".to_owned())
            })?
            .as_ref()
            .cloned()
            .ok_or_else(|| {
                SwarmError::WorktreeIo("transaction root authority was already consumed".to_owned())
            })
    }

    fn release(&self) -> Result<()> {
        let _release = self.release_lock.lock().map_err(|_| {
            SwarmError::WorktreeIo("transaction cleanup authority is poisoned".to_owned())
        })?;
        if self.released.load(Ordering::Acquire) {
            return Ok(());
        }
        self.root_authority()?;
        validate_reservation_contents(&self.reservation_authority, self.reserved_bytes)?;
        let mut lease_was_poisoned = false;
        let mut slot = match self.lease.lock() {
            Ok(slot) => slot,
            Err(error) => {
                lease_was_poisoned = true;
                self.lease.clear_poison();
                error.into_inner()
            }
        };
        if let Some(mut lease) = slot.take() {
            lease.close();
        }
        drop(slot);
        if lease_was_poisoned {
            return Err(SwarmError::WorktreeIo(
                "transaction lease authority was poisoned; lease was closed but cleanup must be retried"
                    .to_owned(),
            ));
        }
        let cleanup_result = with_directory_lock(&self.swarm_root, &self.swarm_authority, || {
            let retained_reservation = self
                .active_reservations
                .lock()
                .map_err(|_| {
                    SwarmError::WorktreeIo("active reservation registry is poisoned".to_owned())
                })?
                .remove(&self.owner);
            let root_authority = match self
                .root_authority
                .lock()
                .map_err(|_| {
                    SwarmError::WorktreeIo("transaction root authority is poisoned".to_owned())
                })?
                .take()
            {
                Some(authority) => authority,
                None => {
                    if let Some(retained) = retained_reservation {
                        self.active_reservations
                            .lock()
                            .map_err(|_| {
                                SwarmError::WorktreeIo(
                                    "active reservation registry is poisoned".to_owned(),
                                )
                            })?
                            .insert(self.owner.clone(), retained);
                    }
                    return Err(SwarmError::WorktreeIo(
                        "transaction root authority was already consumed".to_owned(),
                    ));
                }
            };
            match remove_transaction_root(
                &self.swarm_root,
                &self.swarm_authority,
                &self.owner,
                &self.root,
                root_authority,
                &self.quarantine_root,
                &self.quarantine_authority,
            ) {
                Ok(()) => Ok(()),
                Err((error, root_authority)) => {
                    *self.root_authority.lock().map_err(|_| {
                        SwarmError::WorktreeIo("transaction root authority is poisoned".to_owned())
                    })? = Some(root_authority);
                    if let Some(retained) = retained_reservation {
                        self.active_reservations
                            .lock()
                            .map_err(|_| {
                                SwarmError::WorktreeIo(
                                    "active reservation registry is poisoned".to_owned(),
                                )
                            })?
                            .insert(self.owner.clone(), retained);
                    }
                    Err(error)
                }
            }
        });
        cleanup_result?;
        self.released.store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for TransactionCleanup {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

impl std::fmt::Debug for TransactionWorkspace {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransactionWorkspace")
            .field("owner", &self.owner)
            .field("root", &self.root)
            .field("checkout", &self.checkout)
            .field("scratch", &self.scratch)
            .field("base_commit", &self.base_commit)
            .field("head_commit", &self.head_commit)
            .field("tree", &self.tree)
            .field("reserved_bytes", &self.reserved_bytes)
            .finish()
    }
}

fn with_directory_lock<T>(
    path: &Path,
    authority: &DirectoryAuthority,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    authority.validate_path(path)?;
    let file = authority.try_clone_handle()?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = loop {
        match lock.write() {
            Ok(guard) => break guard,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    };
    authority.validate_path(path)?;
    action()
}

fn create_private_regular_file(path: &Path, contents: &[u8]) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(file)
}

fn remove_transaction_root(
    swarm_root: &Path,
    swarm_authority: &DirectoryAuthority,
    owner: &str,
    root: &Path,
    root_authority: DirectoryAuthority,
    quarantine_root: &Path,
    quarantine_authority: &DirectoryAuthority,
) -> std::result::Result<(), (SwarmError, DirectoryAuthority)> {
    remove_transaction_root_inner_with_hooks(
        swarm_root,
        swarm_authority,
        owner,
        root,
        root_authority,
        quarantine_root,
        quarantine_authority,
        || {},
        || {},
        || {},
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn remove_transaction_root_inner(
    swarm_root: &Path,
    swarm_authority: &DirectoryAuthority,
    owner: &str,
    root: &Path,
    root_authority: DirectoryAuthority,
    quarantine_root: &Path,
    quarantine_authority: &DirectoryAuthority,
    before_quarantine: impl FnOnce(),
) -> std::result::Result<(), (SwarmError, DirectoryAuthority)> {
    remove_transaction_root_inner_with_hooks(
        swarm_root,
        swarm_authority,
        owner,
        root,
        root_authority,
        quarantine_root,
        quarantine_authority,
        before_quarantine,
        || {},
        || {},
    )
}

#[allow(clippy::too_many_arguments)]
fn remove_transaction_root_inner_with_hooks(
    swarm_root: &Path,
    swarm_authority: &DirectoryAuthority,
    owner: &str,
    root: &Path,
    root_authority: DirectoryAuthority,
    _quarantine_root: &Path,
    _quarantine_authority: &DirectoryAuthority,
    before_quarantine: impl FnOnce(),
    before_transaction_delete: impl FnOnce(),
    before_placeholder_delete: impl FnOnce(),
) -> std::result::Result<(), (SwarmError, DirectoryAuthority)> {
    validate_worker_id(owner).map_err(|error| (error, root_authority.clone()))?;
    if root != swarm_root.join(owner) || !root.starts_with(swarm_root) {
        return Err((
            SwarmError::WorktreeIo("refused cleanup outside owned transaction root".to_owned()),
            root_authority,
        ));
    }
    swarm_authority
        .validate_path(swarm_root)
        .map_err(|error| (error, root_authority.clone()))?;
    before_quarantine();
    // Deletion resolves from the retained directory object. No quarantine
    // pathname is needed, so failures return the same authority and stage.
    before_transaction_delete();
    before_placeholder_delete();
    root_authority.remove_open_dir_all()
}

fn transaction_is_active(authority: &DirectoryAuthority, path: &Path) -> Result<bool> {
    authority.validate_path(path)?;
    let file = authority.try_clone_handle()?;
    let mut lock = fd_lock::RwLock::new(file);
    match lock.try_write() {
        Ok(_guard) => Ok(false),
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(true),
        Err(error) if error.kind() == ErrorKind::Interrupted => {
            transaction_is_active(authority, path)
        }
        Err(error) => Err(error.into()),
    }
}

fn read_workspace_reservation(path: &Path) -> Result<u64> {
    let authority = RegularFileAuthority::open(path)
        .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))?;
    let value = authority
        .read_bounded_to_string(MAX_RESERVATION_FILE_BYTES)
        .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))?;
    authority
        .validate_path(path)
        .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))?;
    let bytes = value.trim().parse::<u64>().map_err(|_| {
        SwarmError::DispatchAdmission(format!("invalid workspace reservation: {}", path.display()))
    })?;
    if bytes == 0 || bytes > MAX_TRANSACTION_WORKSPACE_BYTES {
        return Err(SwarmError::DispatchAdmission(format!(
            "workspace reservation is outside the admitted range: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

fn validate_reservation_authority(
    authority: &RegularFileAuthority,
    path: &Path,
    expected_bytes: u64,
) -> Result<()> {
    authority
        .validate_path(path)
        .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))?;
    validate_reservation_contents(authority, expected_bytes)
}

fn validate_reservation_contents(
    authority: &RegularFileAuthority,
    expected_bytes: u64,
) -> Result<()> {
    let value = authority
        .read_bounded_to_string(MAX_RESERVATION_FILE_BYTES)
        .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))?;
    let actual = value.trim().parse::<u64>().map_err(|_| {
        SwarmError::DispatchAdmission("invalid retained workspace reservation".to_owned())
    })?;
    if actual != expected_bytes {
        return Err(SwarmError::DispatchAdmission(format!(
            "retained workspace reservation changed: expected {expected_bytes}, observed {actual}"
        )));
    }
    Ok(())
}

fn is_legacy_linked_worktree(root: &Path) -> bool {
    let git = root.join(".git");
    RegularFileAuthority::open(&git).is_ok() && !root.join("scratch").exists()
}

fn logical_tree_bytes(
    root: wcore_sandbox::DirectoryAuthority,
    limit: u64,
    cancel: Option<&CancellationToken>,
) -> Result<u64> {
    let mut total = 0_u64;
    let mut pending = vec![root];
    let mut entries = 0_u64;
    while let Some(directory) = pending.pop() {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(SwarmError::DispatchAdmission(
                "workspace accounting cancelled".to_owned(),
            ));
        }
        entries = entries
            .checked_add(1)
            .ok_or_else(|| SwarmError::WorktreeIo("workspace entry count overflowed".to_owned()))?;
        if entries > 1_000_000 {
            return Err(SwarmError::DispatchAdmission(
                "workspace entry count exceeded the runtime budget".to_owned(),
            ));
        }
        for name in directory
            .child_names()
            .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))?
        {
            match directory.open_child_directory(&name) {
                Ok(child) => pending.push(child),
                Err(wcore_sandbox::SandboxError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotADirectory =>
                {
                    let file = directory.open_child_file(&name).map_err(|error| {
                        SwarmError::DispatchAdmission(format!(
                            "workspace accounting refused {name:?}: {error}"
                        ))
                    })?;
                    total = total
                        .checked_add(file.len().map_err(|error| {
                            SwarmError::DispatchAdmission(format!(
                                "workspace accounting refused {name:?}: {error}"
                            ))
                        })?)
                        .ok_or_else(|| {
                            SwarmError::WorktreeIo("workspace size overflowed".to_owned())
                        })?;
                    if total > limit {
                        return Ok(total);
                    }
                }
                Err(error) => {
                    return Err(SwarmError::DispatchAdmission(format!(
                        "workspace accounting refused {name:?}: {error}"
                    )));
                }
            }
        }
    }
    Ok(total)
}

impl TransactionWorkspace {
    /// Return the retained transaction-root capability. Orchestration
    /// evidence beneath this root must be accessed through this capability;
    /// [`Self::root`] is display metadata only.
    pub fn root_authority(&self) -> Result<wcore_sandbox::DirectoryAuthority> {
        Ok(self.cleanup.root_authority()?.to_sandbox())
    }

    /// Return a clone of the retained checkout capability. Consumers must use
    /// this object for filesystem authority; [`Self::checkout`] is display
    /// metadata only and must never be reopened to mint a successor authority.
    pub fn checkout_authority(&self) -> wcore_sandbox::DirectoryAuthority {
        self.authorities.checkout.to_sandbox()
    }

    /// Return a clone of the retained scratch capability. Consumers must use
    /// this object for filesystem authority; [`Self::scratch`] is display
    /// metadata only.
    pub fn scratch_authority(&self) -> wcore_sandbox::DirectoryAuthority {
        self.authorities.scratch.to_sandbox()
    }

    pub(crate) fn validate_execution_authority(&self) -> Result<()> {
        self.cleanup.root_authority()?.validate_path(&self.root)?;
        self.authorities.checkout.validate_path(&self.checkout)?;
        self.authorities.scratch.validate_path(&self.scratch)?;
        validate_reservation_authority(
            &self.authorities.reservation,
            &self.root.join(RESERVATION_FILE),
            self.reserved_bytes,
        )?;
        if self.checkout.starts_with(&self.scratch)
            || self.scratch.starts_with(&self.checkout)
            || self.checkout.parent() != Some(self.root.as_path())
            || self.scratch.parent() != Some(self.root.as_path())
        {
            return Err(SwarmError::DispatchAdmission(
                "transaction checkout and scratch authority relationship changed".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn logical_used_bytes(&self) -> Result<u64> {
        self.logical_used_bytes_with_cancel(None)
    }

    pub(crate) fn logical_used_bytes_with_cancel(
        &self,
        cancel: Option<&CancellationToken>,
    ) -> Result<u64> {
        self.validate_execution_authority()?;
        let checkout = logical_tree_bytes(self.checkout_authority(), self.reserved_bytes, cancel)?;
        if checkout > self.reserved_bytes {
            self.validate_execution_authority()?;
            return Ok(checkout);
        }
        let remaining = self.reserved_bytes - checkout;
        let scratch = logical_tree_bytes(self.scratch_authority(), remaining, cancel)?;
        let total = checkout
            .checked_add(scratch)
            .ok_or_else(|| SwarmError::WorktreeIo("workspace size overflowed".to_owned()))?;
        self.validate_execution_authority()?;
        Ok(total)
    }
}

const CLEANUP_GRACE: Duration = Duration::from_secs(5);
const GIT_CAPTURE_LIMITS: CaptureLimits = CaptureLimits {
    stdout_bytes: 1024 * 1024,
    stderr_bytes: 256 * 1024,
    timeout: Duration::from_secs(120),
};
const UNSAFE_CHECKOUT_CONFIG: &str =
    r"^(filter\..*\.(clean|smudge|process)|include\.path|includeif\..*\.path)$";
#[path = "worktree_cleanup.rs"]
mod cleanup;
#[path = "worktree_manager.rs"]
mod manager;

fn capture_error(context: &str, error: ProcessCaptureError) -> SwarmError {
    SwarmError::WorktreeIo(format!("{context}: {error}"))
}

fn worktree_add_error(path: &Path, reason: String) -> SwarmError {
    let residual = match std::fs::symlink_metadata(path) {
        Ok(_) => format!("; residual worktree path preserved: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => format!(
            "; could not inspect possible residual worktree path {}: {error}",
            path.display()
        ),
    };
    SwarmError::WorktreeIo(format!("{reason}{residual}"))
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
