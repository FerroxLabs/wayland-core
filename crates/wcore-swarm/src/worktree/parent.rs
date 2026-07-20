//! Parent-owned landing primitive: import an accepted standalone candidate's
//! object graph through a parent-owned quarantine, revalidate it from the
//! quarantined bytes, and advance the parent target ref by an exact
//! compare-and-swap under one parent-owned lock — then coherently project and
//! verify the owned integration checkout.
//!
//! This module is the ONLY code that mutates the parent checkout. It is
//! deliberately:
//!
//! * **parent-driven** — every Git invocation runs in the *parent* integration
//!   checkout under the scrubbed argv-mode environment
//!   ([`WorktreeManager::git_command`]); the standalone child's object store is
//!   consulted only as a read-only alternate during import, so child commands,
//!   remotes, hooks, filters, replace refs, configuration, environment,
//!   executables, and caller-supplied refspecs can never drive the import;
//! * **quarantined** — candidate objects are byte-copied (never hardlinked,
//!   reflinked, or symlinked) into a parent-owned temporary object directory
//!   through the retained [`wcore_sandbox::DirectoryAuthority`] with
//!   `O_NOFOLLOW` at every component; hash/type/reachability and the exact
//!   accepted head/tree/base/source are revalidated *there* before any parent
//!   ref or working tree moves;
//! * **fail-closed** — parent drift, a dirty checkout, lock contention, a
//!   detached/ambiguous HEAD, a target branch checked out by an unowned
//!   worktree, missing/corrupt/substituted/foreign objects, a non-descendant
//!   candidate, and Git conflicts all stop before the target ref is advanced;
//! * **recoverable** — the logical commit point is a single exact
//!   `git update-ref <ref> <new> <old>`; interruption before promotion deletes
//!   only the owned temporary object directory, while interruption after
//!   promotion retains a recoverable transaction quarantine ref whose cleanup
//!   removes the ref but never directly deletes possibly-shared objects.
//!
//! It never reads or writes `SessionJournal` and cannot mint durable lifecycle
//! authority; it returns typed identity-bound outcomes sufficient for the upper
//! layer (wcore-agent) to journal and recover every boundary.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;
use wcore_sandbox::process_capture::{CaptureLimits, CapturedOutput, capture_bounded_process};
use wcore_sandbox::{DirectoryAuthority as SandboxDirectoryAuthority, SandboxError};

use super::CandidateSeal;
use super::security::{DirectoryAuthority, ref_slug, reject_option_like_ref, validate_target_ref};
use super::{TransactionWorkspace, WorktreeManager};
use crate::error::{Result, SwarmError};

/// Maximum bytes of any single copied Git object file. A larger object aborts
/// the quarantine import rather than allocate without bound.
const MAX_OBJECT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Maximum object-store entries walked while importing the candidate closure.
const MAX_IMPORT_ENTRIES: u64 = 4_000_000;

/// The exact identity of the parent target captured before landing. Every field
/// is re-proved under the parent lock immediately before the compare-and-swap;
/// any drift stops the landing before the target ref moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParentPreimage {
    /// Canonical absolute common Git directory of the integration checkout.
    pub common_git_dir: PathBuf,
    /// Canonical absolute integration checkout root.
    pub checkout_root: PathBuf,
    /// The fully-qualified target ref the landing advances (`refs/heads/...`).
    pub target_ref: String,
    /// The symbolic ref `HEAD` names, if `HEAD` is symbolic (`refs/heads/...`).
    /// `None` for a detached HEAD (which this primitive refuses to project).
    pub symbolic_head: Option<String>,
    /// The exact commit the target ref resolved to before the swap (the CAS
    /// `<old>` value).
    pub expected_commit: String,
    /// The tree the expected commit points at.
    pub expected_tree: String,
    /// The tree currently recorded by the checkout index.
    pub index_tree: String,
    /// A bounded digest over the clean working-tree status manifest.
    pub worktree_digest: String,
    /// Identity of the parent-owned cross-process landing lock.
    pub lock_identity: String,
}

/// The exact successor identity a completed landing produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParentSuccessor {
    /// The commit the target ref was advanced to (the CAS `<new>` value).
    pub landed_commit: String,
    /// The tree the landed commit points at.
    pub landed_tree: String,
    /// The transaction-scoped quarantine ref retaining the landed objects.
    pub quarantine_ref: String,
}

/// A recoverable handle to a completed landing, returned to the upper layer so
/// it can request an exact reverse compare-and-swap. Holding it does not itself
/// mutate anything; [`WorktreeManager::rollback_landing`] performs the reverse
/// CAS under the same lock and only while the successor is still live.
#[derive(Clone, Debug)]
pub struct RollbackHandle {
    pub preimage: ParentPreimage,
    pub successor: ParentSuccessor,
}

/// The typed result of a landing attempt. Each variant carries the exact
/// preimage/successor identity the upper layer journals and recovers from.
#[derive(Clone, Debug)]
pub enum LandingOutcome {
    /// The candidate was imported and revalidated in quarantine and the exact
    /// parent preimage was bound, but the target ref was not advanced (the
    /// caller requested a dry preparation).
    Prepared { preimage: ParentPreimage },
    /// The target ref was advanced by the exact compare-and-swap but coherent
    /// projection of symbolic HEAD / index / worktree is not yet verified.
    RefAdvanced {
        preimage: ParentPreimage,
        successor: ParentSuccessor,
    },
    /// The ref advanced and index/worktree were projected, but final surface
    /// verification is still pending in the caller's recovery matrix.
    Projected {
        preimage: ParentPreimage,
        successor: ParentSuccessor,
    },
    /// The landing completed coherently: ref, symbolic HEAD, index, and the
    /// owned worktree all equal the successor.
    Completed {
        preimage: ParentPreimage,
        successor: ParentSuccessor,
        rollback: Box<RollbackHandle>,
    },
    /// The candidate could not land because the parent state conflicts (drift,
    /// dirty checkout, non-descendant candidate, or a Git textual conflict). No
    /// parent ref or worktree byte changed.
    Conflict { preimage: Option<ParentPreimage> },
    /// The landing observed an inconsistency it must not silently resolve;
    /// explicit recovery is required. No foreign bytes were overwritten.
    RecoveryRequired {
        preimage: Option<ParentPreimage>,
        detail: String,
    },
}

/// A fail-closed refusal from the parent landing primitive.
#[derive(Debug, Error)]
pub enum ParentLandingError {
    #[error("integration checkout is not a Wayland-owned leasable target: {0}")]
    UnownedCheckout(String),
    #[error("parent integration checkout is dirty: {0}")]
    DirtyParent(String),
    #[error("parent state drifted before the compare-and-swap: {0}")]
    ParentDrift(String),
    #[error("candidate object graph failed quarantine revalidation: {0}")]
    QuarantineRevalidation(String),
    #[error("candidate is not a descendant of the expected parent commit: {0}")]
    NonDescendant(String),
    #[error("parent landing lock is contended or unavailable: {0}")]
    LockContention(String),
    #[error("git plumbing failed during landing: {0}")]
    Git(String),
    #[error("landing filesystem operation failed: {0}")]
    Io(String),
    #[error("landing projection could not be verified: {0}")]
    Projection(String),
    #[error("rollback refused because the landed successor is no longer live: {0}")]
    RollbackForeignDrift(String),
}

impl From<ParentLandingError> for SwarmError {
    fn from(error: ParentLandingError) -> Self {
        SwarmError::WorktreeIo(format!("parent landing: {error}"))
    }
}

type LandingResult<T> = std::result::Result<T, ParentLandingError>;

fn map_sandbox(error: SandboxError) -> ParentLandingError {
    ParentLandingError::Io(error.to_string())
}

/// Validate an exact 40/64 hex object id.
fn valid_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Map a ref-name guard refusal from `worktree_security` into the landing error.
fn reject_target_ref(target_ref: &str) -> LandingResult<()> {
    validate_target_ref(target_ref).map_err(|error| ParentLandingError::Git(error.to_string()))
}

/// The bound, validated integration checkout the landing mutates.
pub(super) struct IntegrationCheckout {
    /// Canonical absolute checkout root.
    root: PathBuf,
    /// Retained authority over the checkout root (identity + no-follow opens).
    authority: DirectoryAuthority,
    /// Canonical absolute common Git directory.
    common_git_dir: PathBuf,
    /// Canonical absolute real object directory (`<common>/objects`).
    objects_dir: PathBuf,
}

/// The exact commit identity a landing synthesized from a candidate working
/// tree, computed solely from the quarantined bytes.
struct BuiltCommit {
    head: String,
    tree: String,
}

/// A parent-owned quarantine holding the objects (new blobs, tree, and commit)
/// the landing built from the candidate working tree, plus the transient index
/// used to build them. The object store lives at `<dir>/objects` and the index
/// at `<dir>/land-index`, so migrating the objects never copies the index into
/// the parent store. Dropping it before promotion deletes only the owned
/// temporary directory (interruption-before-promotion cleanup).
pub(super) struct QuarantinedCandidate {
    dir: PathBuf,
    promoted: bool,
}

impl QuarantinedCandidate {
    fn objects_dir(&self) -> PathBuf {
        self.dir.join("objects")
    }

    fn index_file(&self) -> PathBuf {
        self.dir.join("land-index")
    }

    /// Explicitly discard the quarantine before promotion.
    fn discard(self) {
        // Drop runs the cleanup below.
        drop(self);
    }
}

impl Drop for QuarantinedCandidate {
    fn drop(&mut self) {
        if !self.promoted {
            // Before promotion the quarantine is disposable; remove only the
            // owned temp object directory. Never touches the parent store.
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl WorktreeManager {
    /// Land an accepted standalone candidate into the parent integration
    /// checkout by exact compare-and-swap.
    ///
    /// The caller supplies the candidate's retained [`TransactionWorkspace`] and
    /// its live [`CandidateSeal`] (which this primitive revalidates), the
    /// absolute integration checkout to mutate, the fully-qualified target ref,
    /// and the exact commit the target ref is expected to currently name (the
    /// CAS `<old>`). Every fail-closed boundary described on
    /// [`LandingOutcome`] is enforced; no `SessionJournal` is read or written.
    ///
    /// When `commit_swap` is `false` the primitive stops after quarantine
    /// revalidation and preimage binding, returning [`LandingOutcome::Prepared`]
    /// without moving the target ref.
    pub async fn land_candidate(
        &self,
        candidate: &TransactionWorkspace,
        seal: &CandidateSeal,
        integration_checkout: &Path,
        target_ref: &str,
        expected_commit: &str,
        commit_swap: bool,
    ) -> Result<LandingOutcome> {
        Box::pin(self.land_candidate_inner(
            candidate,
            seal,
            integration_checkout,
            target_ref,
            expected_commit,
            commit_swap,
        ))
        .await
        .map_err(Into::into)
    }

    async fn land_candidate_inner(
        &self,
        candidate: &TransactionWorkspace,
        seal: &CandidateSeal,
        integration_checkout: &Path,
        target_ref: &str,
        expected_commit: &str,
        commit_swap: bool,
    ) -> LandingResult<LandingOutcome> {
        reject_target_ref(target_ref)?;
        if !valid_object_id(expected_commit) {
            return Err(ParentLandingError::ParentDrift(
                "expected parent commit is not an exact object id".to_owned(),
            ));
        }
        reject_option_like_ref("candidate head", &candidate.head_commit)
            .map_err(|error| ParentLandingError::QuarantineRevalidation(error.to_string()))?;
        if !valid_object_id(&candidate.head_commit) || !valid_object_id(&candidate.tree) {
            return Err(ParentLandingError::QuarantineRevalidation(
                "candidate head/tree are not exact object ids".to_owned(),
            ));
        }

        // Re-prove the candidate is a live, clean, isolated, sealed checkout
        // (drift/dirty/identity/alias/config/hook guard) before any bytes move.
        seal.revalidate()
            .map_err(|error| ParentLandingError::QuarantineRevalidation(error.to_string()))?;
        candidate
            .validate_execution_authority()
            .map_err(|error| ParentLandingError::QuarantineRevalidation(error.to_string()))?;

        // Bind and validate the leased integration checkout.
        let target = self.bind_integration_checkout(integration_checkout).await?;

        // Acquire the exclusive cross-process parent landing lock for this ref
        // and hold it for the whole critical section (CAS + projection).
        let (lock_file, lock_identity) = self.open_landing_lock_file(&target, target_ref)?;
        let mut lock_rw = fd_lock::RwLock::new(lock_file);
        let _lock_guard = lock_rw
            .try_write()
            .map_err(|error| ParentLandingError::LockContention(error.to_string()))?;

        // Bind the durable parent preimage under the lock and require the target
        // ref to currently name exactly the expected commit.
        let preimage = self
            .bind_parent_preimage(&target, target_ref, expected_commit, &lock_identity)
            .await?;

        // The candidate must have been forked from the parent's current tip:
        // its bound base is the exact commit the target ref currently names.
        // A candidate built on a stale base is a non-fast-forward conflict.
        if candidate.base_commit != expected_commit {
            return Ok(LandingOutcome::Conflict {
                preimage: Some(preimage),
            });
        }

        // Build the candidate's tree + commit from its working tree, on top of
        // the base, entirely in a parent-owned quarantine object directory, then
        // revalidate the reachable graph, head/tree, and B->H ancestry solely
        // from the quarantined bytes.
        let (quarantine, built) = self
            .import_candidate_quarantine(
                &target,
                candidate,
                seal,
                &preimage.expected_tree,
                expected_commit,
            )
            .await?;
        if let Err(error) = self
            .revalidate_quarantine(&target, &quarantine, &built, expected_commit)
            .await
        {
            quarantine.discard();
            return Err(error);
        }

        if !commit_swap {
            quarantine.discard();
            return Ok(LandingOutcome::Prepared { preimage });
        }

        // Re-verify the parent has not drifted since the preimage was bound.
        self.assert_parent_unchanged(&target, &preimage).await?;

        // Promotion: migrate quarantined objects into the parent real store and
        // bind a transaction-scoped quarantine ref BEFORE the CAS so the landed
        // objects stay reachable across an interrupted swap.
        let quarantine_ref = format!("refs/wayland/landing/{}", ref_slug(target_ref));
        self.promote_quarantine(&target, quarantine)?;
        self.bind_quarantine_ref(&target, &quarantine_ref, &built.head)
            .await?;

        // The logical commit point: exact full-ref compare-and-swap old->new.
        match self
            .compare_and_swap_ref(&target, target_ref, expected_commit, &built.head)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                // CAS old-mismatch: a concurrent lander or foreign edit moved
                // the target. The quarantine ref keeps our objects reachable but
                // the target is unchanged; report conflict without overwriting.
                return Ok(LandingOutcome::Conflict {
                    preimage: Some(preimage),
                });
            }
            Err(error) => return Err(error),
        }

        let successor = ParentSuccessor {
            landed_commit: built.head.clone(),
            landed_tree: built.tree.clone(),
            quarantine_ref,
        };

        // Coherently project symbolic HEAD, index, and the owned worktree. The
        // ref has already advanced, so a projection failure is a recoverable
        // ref-advanced state (the caller finishes projection or rolls back), not
        // a lost mutation.
        if let Err(error) = self
            .project_worktree(
                &target,
                &preimage,
                &preimage.expected_tree,
                &successor.landed_tree,
            )
            .await
        {
            tracing::warn!(error = %error, "parent landing ref advanced but projection is incomplete");
            return Ok(LandingOutcome::RefAdvanced {
                preimage,
                successor,
            });
        }
        // Verify every surface equals the successor; a mismatch leaves a
        // projected-but-unverified state for the caller's recovery matrix.
        if let Err(error) = self.verify_projection(&target, &preimage, &successor).await {
            tracing::warn!(error = %error, "parent landing projected but verification is incomplete");
            return Ok(LandingOutcome::Projected {
                preimage,
                successor,
            });
        }

        let rollback = RollbackHandle {
            preimage: preimage.clone(),
            successor: successor.clone(),
        };
        Ok(LandingOutcome::Completed {
            preimage,
            successor,
            rollback: Box::new(rollback),
        })
    }

    /// Reverse a completed landing by an exact reverse compare-and-swap under
    /// the same lock. Proceeds only while ref, symbolic HEAD, index, and
    /// worktree still equal the landed successor; any foreign movement stops the
    /// rollback in [`ParentLandingError::RollbackForeignDrift`] without erasing
    /// later work.
    pub async fn rollback_landing(
        &self,
        integration_checkout: &Path,
        handle: &RollbackHandle,
    ) -> Result<()> {
        Box::pin(self.rollback_landing_inner(integration_checkout, handle))
            .await
            .map_err(Into::into)
    }

    async fn rollback_landing_inner(
        &self,
        integration_checkout: &Path,
        handle: &RollbackHandle,
    ) -> LandingResult<()> {
        reject_target_ref(&handle.preimage.target_ref)?;
        let target = self.bind_integration_checkout(integration_checkout).await?;
        let (lock_file, _lock_identity) =
            self.open_landing_lock_file(&target, &handle.preimage.target_ref)?;
        let mut lock_rw = fd_lock::RwLock::new(lock_file);
        let _lock_guard = lock_rw
            .try_write()
            .map_err(|error| ParentLandingError::LockContention(error.to_string()))?;

        // The target ref must still name the landed successor; a later foreign
        // movement stops rollback rather than clobber it.
        let current = self
            .resolve_ref(&target, &handle.preimage.target_ref)
            .await?;
        if current.as_deref() != Some(handle.successor.landed_commit.as_str()) {
            return Err(ParentLandingError::RollbackForeignDrift(format!(
                "target ref {} no longer names the landed successor",
                handle.preimage.target_ref
            )));
        }
        // Index and worktree must also still equal the successor tree.
        let index_tree = self.index_tree(&target).await?;
        if index_tree != handle.successor.landed_tree {
            return Err(ParentLandingError::RollbackForeignDrift(
                "parent index diverged from the landed successor".to_owned(),
            ));
        }
        self.assert_clean_checkout(&target).await?;

        // Exact reverse compare-and-swap new->old.
        if !self
            .compare_and_swap_ref(
                &target,
                &handle.preimage.target_ref,
                &handle.successor.landed_commit,
                &handle.preimage.expected_commit,
            )
            .await?
        {
            return Err(ParentLandingError::RollbackForeignDrift(
                "reverse compare-and-swap observed a concurrent target movement".to_owned(),
            ));
        }
        // Re-project index and worktree from the successor tree back to the
        // preimage tree.
        self.project_worktree(
            &target,
            &handle.preimage,
            &handle.successor.landed_tree,
            &handle.preimage.expected_tree,
        )
        .await?;
        Ok(())
    }

    // ---- integration checkout binding -----------------------------------

    /// Bind a Wayland-owned integration checkout: prove it is a real directory
    /// held by this manager's authority, resolve its git/common/objects dirs
    /// under the scrubbed environment, and refuse an arbitrary or unsafe target.
    pub(super) async fn bind_integration_checkout(
        &self,
        integration_checkout: &Path,
    ) -> LandingResult<IntegrationCheckout> {
        if !integration_checkout.is_absolute() {
            return Err(ParentLandingError::UnownedCheckout(
                "integration checkout path must be absolute".to_owned(),
            ));
        }
        let root = std::fs::canonicalize(integration_checkout)
            .map_err(|error| ParentLandingError::UnownedCheckout(error.to_string()))?;
        let authority = DirectoryAuthority::open(&root)
            .map_err(|error| ParentLandingError::UnownedCheckout(error.to_string()))?;
        authority
            .validate_path(&root)
            .map_err(|error| ParentLandingError::UnownedCheckout(error.to_string()))?;

        // The checkout must be the main working tree of its own repository.
        let inside = self
            .landing_git_stdout(&root, &["rev-parse", "--is-inside-work-tree"])
            .await?;
        if inside.trim() != "true" {
            return Err(ParentLandingError::UnownedCheckout(
                "integration path is not inside a Git working tree".to_owned(),
            ));
        }
        let git_dir = self
            .landing_git_stdout(&root, &["rev-parse", "--absolute-git-dir"])
            .await?;
        let common_git_dir = self
            .landing_git_stdout(
                &root,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?;
        let git_dir = std::fs::canonicalize(PathBuf::from(git_dir.trim()))
            .map_err(|error| ParentLandingError::UnownedCheckout(error.to_string()))?;
        let common_git_dir = std::fs::canonicalize(PathBuf::from(common_git_dir.trim()))
            .map_err(|error| ParentLandingError::UnownedCheckout(error.to_string()))?;
        // A linked worktree has git_dir != common_git_dir; refuse it — the main
        // repository's HEAD/index/worktree are the durable landing target.
        if git_dir != common_git_dir {
            return Err(ParentLandingError::UnownedCheckout(
                "integration checkout is a linked worktree; landing requires the main checkout"
                    .to_owned(),
            ));
        }
        if common_git_dir != root.join(".git") {
            return Err(ParentLandingError::UnownedCheckout(
                "integration checkout does not own an in-tree .git directory".to_owned(),
            ));
        }
        let objects_dir = std::fs::canonicalize(common_git_dir.join("objects"))
            .map_err(|error| ParentLandingError::UnownedCheckout(error.to_string()))?;
        let alternates = objects_dir.join("info").join("alternates");
        if std::fs::symlink_metadata(&alternates).is_ok() {
            return Err(ParentLandingError::UnownedCheckout(
                "integration checkout uses an alternate object store".to_owned(),
            ));
        }
        Ok(IntegrationCheckout {
            root,
            authority,
            common_git_dir,
            objects_dir,
        })
    }

    // ---- lock -----------------------------------------------------------

    /// Open (creating if needed) the parent-owned per-ref landing lock file and
    /// return it plus its identity. The caller wraps it in an `fd_lock::RwLock`
    /// and holds the write guard for the critical section, so contention is
    /// observed at `try_write` time — never silently waited on.
    fn open_landing_lock_file(
        &self,
        target: &IntegrationCheckout,
        target_ref: &str,
    ) -> LandingResult<(std::fs::File, String)> {
        let lock_dir = target.common_git_dir.join("wayland-landing");
        std::fs::create_dir_all(&lock_dir)
            .map_err(|error| ParentLandingError::LockContention(error.to_string()))?;
        let lock_path = lock_dir.join(format!("{}.lock", ref_slug(target_ref)));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| ParentLandingError::LockContention(error.to_string()))?;
        Ok((file, lock_path.to_string_lossy().into_owned()))
    }

    /// Build a scrubbed, argv-mode git command scoped to an owned checkout, with
    /// an optional quarantine object directory and read-only alternates. The
    /// object-directory env vars are re-added after `git_command` scrubs them,
    /// so import runs entirely under parent authority.
    fn landing_git_command(
        &self,
        checkout: &Path,
        object_dir: Option<&Path>,
        alternates: &[PathBuf],
        args: &[&str],
    ) -> tokio::process::Command {
        let checkout_s = checkout.to_string_lossy().into_owned();
        let mut scoped: Vec<&str> = vec!["-C", checkout_s.as_str()];
        scoped.extend_from_slice(args);
        let mut cmd = self.git_command(&scoped);
        if let Some(dir) = object_dir {
            cmd.env("GIT_OBJECT_DIRECTORY", dir);
        }
        if !alternates.is_empty()
            && let Ok(joined) = std::env::join_paths(alternates)
        {
            cmd.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", joined);
        }
        cmd
    }

    fn landing_capture_limits(&self) -> CaptureLimits {
        self.capture_limits
    }

    // ---- preimage -------------------------------------------------------

    async fn bind_parent_preimage(
        &self,
        target: &IntegrationCheckout,
        target_ref: &str,
        expected_commit: &str,
        lock_identity: &str,
    ) -> LandingResult<ParentPreimage> {
        // Refuse a dirty checkout up front (staged/unstaged/untracked).
        self.assert_clean_checkout(target).await?;

        // The target ref must currently name exactly the expected commit.
        let current = self.resolve_ref(target, target_ref).await?.ok_or_else(|| {
            ParentLandingError::ParentDrift(format!("target ref {target_ref} does not exist"))
        })?;
        if current != expected_commit {
            return Err(ParentLandingError::ParentDrift(format!(
                "target ref {target_ref} names {current}, expected {expected_commit}"
            )));
        }

        // Refuse a detached or ambiguous HEAD by requiring a symbolic HEAD.
        let symbolic_head = self.symbolic_head(target).await?;
        // Refuse the case where a foreign linked worktree currently checks out
        // the target branch (its own HEAD would silently diverge from ours).
        self.assert_target_not_foreign_worktree(target, target_ref, symbolic_head.as_deref())
            .await?;

        let expected_tree = self
            .landing_git_stdout(
                &target.root,
                &[
                    "rev-parse",
                    "--verify",
                    &format!("{expected_commit}^{{tree}}"),
                ],
            )
            .await?;
        let expected_tree = expected_tree.trim().to_owned();
        let index_tree = self.index_tree(target).await?;
        let worktree_digest = self.clean_worktree_digest(target).await?;

        Ok(ParentPreimage {
            common_git_dir: target.common_git_dir.clone(),
            checkout_root: target.root.clone(),
            target_ref: target_ref.to_owned(),
            symbolic_head,
            expected_commit: expected_commit.to_owned(),
            expected_tree,
            index_tree,
            worktree_digest,
            lock_identity: lock_identity.to_owned(),
        })
    }

    async fn assert_parent_unchanged(
        &self,
        target: &IntegrationCheckout,
        preimage: &ParentPreimage,
    ) -> LandingResult<()> {
        target
            .authority
            .validate_path(&target.root)
            .map_err(|error| ParentLandingError::ParentDrift(error.to_string()))?;
        let current = self
            .resolve_ref(target, &preimage.target_ref)
            .await?
            .ok_or_else(|| {
                ParentLandingError::ParentDrift(format!(
                    "target ref {} vanished before the swap",
                    preimage.target_ref
                ))
            })?;
        if current != preimage.expected_commit {
            return Err(ParentLandingError::ParentDrift(format!(
                "target ref {} drifted to {current} before the swap",
                preimage.target_ref
            )));
        }
        if self.index_tree(target).await? != preimage.index_tree {
            return Err(ParentLandingError::ParentDrift(
                "parent index changed before the swap".to_owned(),
            ));
        }
        if self.clean_worktree_digest(target).await? != preimage.worktree_digest {
            return Err(ParentLandingError::ParentDrift(
                "parent worktree changed before the swap".to_owned(),
            ));
        }
        if self.symbolic_head(target).await? != preimage.symbolic_head {
            return Err(ParentLandingError::ParentDrift(
                "parent symbolic HEAD changed before the swap".to_owned(),
            ));
        }
        Ok(())
    }

    // ---- quarantine import ---------------------------------------------

    /// Build the candidate's tree + commit from its live working tree on top of
    /// the base, entirely in a parent-owned quarantine object directory.
    ///
    /// The build runs parent-owned Git (`git_command`, scrubbed argv) with a
    /// transient index and object directory in quarantine, the candidate as the
    /// work tree, and the parent object store as a read-only alternate — so the
    /// candidate's own `.git` config, hooks, refs, and remotes never load, and
    /// the child never runs. The base tree seeds the index, `add --all --force`
    /// captures the sealed working tree (adds / modifications / deletions), and
    /// `commit-tree` records the successor commit with a deterministic parent
    /// landing identity so the built id is reproducible.
    async fn import_candidate_quarantine(
        &self,
        target: &IntegrationCheckout,
        candidate: &TransactionWorkspace,
        seal: &CandidateSeal,
        base_tree: &str,
        base_commit: &str,
    ) -> LandingResult<(QuarantinedCandidate, BuiltCommit)> {
        let quarantine_root = target.common_git_dir.join("wayland-landing-quarantine");
        std::fs::create_dir_all(&quarantine_root)
            .map_err(|error| ParentLandingError::Io(format!("create quarantine root: {error}")))?;
        let unique = format!("{}-{}", candidate.owner, std::process::id());
        let dir = quarantine_root.join(unique);
        if std::fs::symlink_metadata(&dir).is_ok() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        std::fs::create_dir(&dir)
            .map_err(|error| ParentLandingError::Io(format!("create quarantine dir: {error}")))?;
        let quarantine = QuarantinedCandidate {
            dir,
            promoted: false,
        };
        // The object store and index live in disjoint quarantine paths so
        // migrating the objects never copies the index into the parent store.
        std::fs::create_dir(quarantine.objects_dir()).map_err(|error| {
            ParentLandingError::Io(format!("create quarantine objects: {error}"))
        })?;

        let work_tree = candidate.checkout_authority().display_path().to_path_buf();
        let parent_git_dir = target.common_git_dir.clone();
        let object_dir = quarantine.objects_dir();
        let index_file = quarantine.index_file();
        let alternates = [target.objects_dir.clone()];

        // Seed the transient index with the base tree so `add --all` records
        // deletions relative to the parent tip, not an empty tree.
        self.candidate_build_git(
            &parent_git_dir,
            &work_tree,
            &object_dir,
            &index_file,
            &alternates,
            &["read-tree", base_tree],
        )
        .await?;
        // TOCTOU close: re-scan the candidate `.git` (config/hooks/alternates/
        // source manifest) through the seal immediately before the staging step
        // that reads the working tree, so the configuration git could load is the
        // one just validated. A `filter.*` clean/smudge driver, a relocated
        // hooks path, or any non-benign config written into `<candidate>/.git`
        // after the mint scan fails closed HERE, before `git add` runs.
        seal.revalidate()
            .map_err(|error| ParentLandingError::QuarantineRevalidation(error.to_string()))?;
        // Capture the sealed working tree (ignore .gitignore so the full source
        // manifest is bound, matching the seal). Objects land in quarantine.
        self.candidate_build_git(
            &parent_git_dir,
            &work_tree,
            &object_dir,
            &index_file,
            &alternates,
            &["add", "--all", "--force", "--", "."],
        )
        .await?;
        let tree = self
            .candidate_build_git(
                &parent_git_dir,
                &work_tree,
                &object_dir,
                &index_file,
                &alternates,
                &["write-tree"],
            )
            .await?;
        let tree = tree.trim().to_owned();
        if !valid_object_id(&tree) {
            return Err(ParentLandingError::QuarantineRevalidation(
                "candidate working tree did not resolve to a tree object".to_owned(),
            ));
        }
        let message = format!("wayland: land delegated mutation {}", candidate.owner);
        let head = self
            .candidate_commit_tree(&object_dir, &alternates, &tree, base_commit, &message)
            .await?;
        Ok((quarantine, BuiltCommit { head, tree }))
    }

    async fn revalidate_quarantine(
        &self,
        target: &IntegrationCheckout,
        quarantine: &QuarantinedCandidate,
        built: &BuiltCommit,
        expected_commit: &str,
    ) -> LandingResult<()> {
        let head = &built.head;
        let quarantine_dir = quarantine.objects_dir();
        // From the quarantined bytes plus the parent real store (base objects),
        // revalidate the built identity and prove connectivity. The candidate's
        // own object store is NOT an alternate here — the successor must be
        // self-contained in quarantine + the parent base store.
        let alternates = vec![target.objects_dir.clone()];

        if !valid_object_id(head) {
            return Err(ParentLandingError::QuarantineRevalidation(
                "built head is not an exact object id".to_owned(),
            ));
        }
        // Type + existence of the head, computed solely from quarantine bytes.
        let object_type = self
            .quarantine_git_stdout(
                target,
                &quarantine_dir,
                &alternates,
                &["cat-file", "-t", head],
            )
            .await?;
        if object_type.trim() != "commit" {
            return Err(ParentLandingError::QuarantineRevalidation(format!(
                "built head {head} is a {} in quarantine, not a commit",
                object_type.trim()
            )));
        }
        // Recompute the tree solely from quarantined bytes and match the built
        // tree.
        let tree = self
            .quarantine_git_stdout(
                target,
                &quarantine_dir,
                &alternates,
                &["rev-parse", "--verify", &format!("{head}^{{tree}}")],
            )
            .await?;
        if tree.trim() != built.tree {
            return Err(ParentLandingError::QuarantineRevalidation(format!(
                "built tree recomputed as {} in quarantine, expected {}",
                tree.trim(),
                built.tree
            )));
        }
        // Full reachable-graph connectivity from the head down to the base,
        // solely from quarantine + base store. A missing/corrupt/substituted or
        // foreign object fails here before any parent ref moves.
        self.quarantine_git_stdout(
            target,
            &quarantine_dir,
            &alternates,
            &[
                "rev-list",
                "--objects",
                "--no-object-names",
                head,
                "--not",
                expected_commit,
            ],
        )
        .await?;
        // fsck the head's reachable graph in quarantine (hash/type integrity).
        self.quarantine_git_stdout(
            target,
            &quarantine_dir,
            &alternates,
            &["fsck", "--no-dangling", "--connectivity-only", head],
        )
        .await?;
        // B must be an ancestor of H (descendant candidate only).
        let ancestor = self
            .quarantine_git_status(
                target,
                &quarantine_dir,
                &alternates,
                &["merge-base", "--is-ancestor", expected_commit, head],
            )
            .await?;
        match ancestor {
            0 => Ok(()),
            1 => Err(ParentLandingError::NonDescendant(format!(
                "built commit {head} does not descend from parent {expected_commit}"
            ))),
            code => Err(ParentLandingError::QuarantineRevalidation(format!(
                "ancestry probe failed with status {code}"
            ))),
        }
    }

    // ---- promotion + refs ----------------------------------------------

    fn promote_quarantine(
        &self,
        target: &IntegrationCheckout,
        mut quarantine: QuarantinedCandidate,
    ) -> LandingResult<()> {
        // Migrate the built objects from quarantine into the parent's own object
        // store by byte copy (never hardlink/reflink). The objects are
        // content-addressed, so re-writing an object that already exists is
        // idempotent.
        let source =
            SandboxDirectoryAuthority::open(&quarantine.objects_dir()).map_err(map_sandbox)?;
        let destination =
            SandboxDirectoryAuthority::open(&target.objects_dir).map_err(map_sandbox)?;
        copy_object_store(&source, &destination)?;
        quarantine.promoted = true;
        // The temp dir is now redundant; remove it. Its objects live in the
        // parent store and are kept reachable by the quarantine ref bound next.
        let _ = std::fs::remove_dir_all(&quarantine.dir);
        Ok(())
    }

    /// Run one scrubbed, parent-owned Git build step against the candidate work
    /// tree with a transient quarantine index + object dir.
    ///
    /// `GIT_DIR` is pinned to the trusted parent git directory rather than left
    /// to discovery: git therefore loads the *parent's* configuration, never the
    /// candidate-controlled `<candidate>/.git/config`. This neutralizes the
    /// `filter.*` clean/smudge command-execution vector at the invocation — a
    /// candidate `.gitattributes` marking a path `filter=x` finds no `filter.x`
    /// driver in the trusted config, so `git add` runs no external command. The
    /// caller additionally re-scans the candidate config immediately before the
    /// staging step, closing the TOCTOU window to zero.
    async fn candidate_build_git(
        &self,
        parent_git_dir: &Path,
        work_tree: &Path,
        object_dir: &Path,
        index_file: &Path,
        alternates: &[PathBuf],
        args: &[&str],
    ) -> LandingResult<String> {
        let mut cmd = self.git_command(args);
        cmd.current_dir(work_tree)
            .env("GIT_DIR", parent_git_dir)
            .env("GIT_WORK_TREE", work_tree)
            .env("GIT_INDEX_FILE", index_file)
            .env("GIT_OBJECT_DIRECTORY", object_dir);
        if let Ok(joined) = std::env::join_paths(alternates) {
            cmd.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", joined);
        }
        let out = capture_bounded_process(cmd, self.landing_capture_limits(), None)
            .await
            .map_err(|error| ParentLandingError::QuarantineRevalidation(error.to_string()))?;
        if !out.status.success() {
            return Err(ParentLandingError::QuarantineRevalidation(format!(
                "candidate build step {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// Record the successor commit over `base_commit` with the built `tree`,
    /// using a deterministic parent landing identity so the id is reproducible.
    async fn candidate_commit_tree(
        &self,
        object_dir: &Path,
        alternates: &[PathBuf],
        tree: &str,
        base_commit: &str,
        message: &str,
    ) -> LandingResult<String> {
        let mut cmd = self.git_command(&["commit-tree", tree, "-p", base_commit, "-m", message]);
        cmd.env("GIT_OBJECT_DIRECTORY", object_dir)
            .env("GIT_AUTHOR_NAME", "Wayland Landing")
            .env("GIT_AUTHOR_EMAIL", "landing@wayland.invalid")
            .env("GIT_AUTHOR_DATE", "@0 +0000")
            .env("GIT_COMMITTER_NAME", "Wayland Landing")
            .env("GIT_COMMITTER_EMAIL", "landing@wayland.invalid")
            .env("GIT_COMMITTER_DATE", "@0 +0000");
        if let Ok(joined) = std::env::join_paths(alternates) {
            cmd.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", joined);
        }
        let out = capture_bounded_process(cmd, self.landing_capture_limits(), None)
            .await
            .map_err(|error| ParentLandingError::QuarantineRevalidation(error.to_string()))?;
        if !out.status.success() {
            return Err(ParentLandingError::QuarantineRevalidation(format!(
                "commit-tree failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let head = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !valid_object_id(&head) {
            return Err(ParentLandingError::QuarantineRevalidation(
                "commit-tree returned an invalid object id".to_owned(),
            ));
        }
        Ok(head)
    }

    async fn bind_quarantine_ref(
        &self,
        target: &IntegrationCheckout,
        quarantine_ref: &str,
        head: &str,
    ) -> LandingResult<()> {
        // Create the transaction-scoped ref unconditionally to keep the objects
        // reachable. update-ref with a "" old value creates/overwrites; we want
        // it to point at the head regardless of a prior interrupted attempt.
        self.landing_git_status(&target.root, &["update-ref", quarantine_ref, head])
            .await
            .and_then(|code| {
                if code == 0 {
                    Ok(())
                } else {
                    Err(ParentLandingError::Git(format!(
                        "failed to bind quarantine ref {quarantine_ref}"
                    )))
                }
            })
    }

    async fn compare_and_swap_ref(
        &self,
        target: &IntegrationCheckout,
        target_ref: &str,
        old: &str,
        new: &str,
    ) -> LandingResult<bool> {
        // The exact compare-and-swap: git update-ref <ref> <new> <old> fails
        // (non-zero) if the ref does not currently name <old>. That refusal is
        // the CAS old-mismatch signal — it never overwrites a drifted ref.
        let code = self
            .landing_git_status(&target.root, &["update-ref", target_ref, new, old])
            .await?;
        Ok(code == 0)
    }

    // ---- projection + verification -------------------------------------

    /// Transition the index + owned worktree from `from_tree` to `to_tree`
    /// without touching any ref. Uses the standard two-tree
    /// `read-tree -m -u <from> <to>` merge git itself uses for a clean checkout
    /// switch; a textual conflict (dirty worktree) fails closed before any file
    /// is written.
    async fn project_worktree(
        &self,
        target: &IntegrationCheckout,
        preimage: &ParentPreimage,
        from_tree: &str,
        to_tree: &str,
    ) -> LandingResult<()> {
        // Only project the index + worktree when HEAD symbolically names the
        // target ref; otherwise the ref moved but this checkout's HEAD points
        // elsewhere and we must not silently rewrite an unrelated worktree.
        if preimage.symbolic_head.as_deref() != Some(preimage.target_ref.as_str()) {
            return Err(ParentLandingError::Projection(
                "integration HEAD does not symbolically name the target ref".to_owned(),
            ));
        }
        let code = self
            .landing_git_status(&target.root, &["read-tree", "-m", "-u", from_tree, to_tree])
            .await?;
        if code != 0 {
            return Err(ParentLandingError::Projection(
                "projecting the successor tree into the worktree conflicted".to_owned(),
            ));
        }
        Ok(())
    }

    async fn verify_projection(
        &self,
        target: &IntegrationCheckout,
        preimage: &ParentPreimage,
        successor: &ParentSuccessor,
    ) -> LandingResult<()> {
        if self
            .resolve_ref(target, &preimage.target_ref)
            .await?
            .as_deref()
            != Some(successor.landed_commit.as_str())
        {
            return Err(ParentLandingError::Projection(
                "target ref does not name the landed commit after projection".to_owned(),
            ));
        }
        if self.symbolic_head(target).await? != preimage.symbolic_head {
            return Err(ParentLandingError::Projection(
                "symbolic HEAD changed during projection".to_owned(),
            ));
        }
        if self.index_tree(target).await? != successor.landed_tree {
            return Err(ParentLandingError::Projection(
                "index tree does not equal the landed tree after projection".to_owned(),
            ));
        }
        // The worktree must be clean against the new HEAD.
        self.assert_clean_checkout(target).await?;
        Ok(())
    }

    // ---- git plumbing helpers ------------------------------------------

    async fn resolve_ref(
        &self,
        target: &IntegrationCheckout,
        reference: &str,
    ) -> LandingResult<Option<String>> {
        let code_and_out = self
            .landing_git_capture(
                &target.root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{reference}^{{commit}}"),
                ],
            )
            .await?;
        if code_and_out.status.success() {
            let value = String::from_utf8_lossy(&code_and_out.stdout)
                .trim()
                .to_owned();
            if valid_object_id(&value) {
                Ok(Some(value))
            } else {
                Err(ParentLandingError::Git(format!(
                    "ref {reference} resolved to an invalid id"
                )))
            }
        } else {
            Ok(None)
        }
    }

    async fn symbolic_head(&self, target: &IntegrationCheckout) -> LandingResult<Option<String>> {
        let out = self
            .landing_git_capture(&target.root, &["symbolic-ref", "--quiet", "HEAD"])
            .await?;
        if out.status.success() {
            Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_owned()))
        } else {
            // Detached HEAD: symbolic-ref exits non-zero. Represented as None,
            // which project_landing refuses to touch.
            Ok(None)
        }
    }

    async fn index_tree(&self, target: &IntegrationCheckout) -> LandingResult<String> {
        let tree = self
            .landing_git_stdout(&target.root, &["write-tree"])
            .await?;
        Ok(tree.trim().to_owned())
    }

    async fn assert_clean_checkout(&self, target: &IntegrationCheckout) -> LandingResult<()> {
        let out = self
            .landing_git_capture(&target.root, &["status", "--porcelain=v1", "-z"])
            .await?;
        if !out.status.success() {
            return Err(ParentLandingError::Git(format!(
                "git status failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        if !out.stdout.is_empty() {
            return Err(ParentLandingError::DirtyParent(
                String::from_utf8_lossy(&out.stdout)
                    .replace('\0', " ")
                    .trim()
                    .to_owned(),
            ));
        }
        Ok(())
    }

    async fn clean_worktree_digest(&self, target: &IntegrationCheckout) -> LandingResult<String> {
        let out = self
            .landing_git_capture(&target.root, &["status", "--porcelain=v1", "-z"])
            .await?;
        if !out.status.success() {
            return Err(ParentLandingError::Git(format!(
                "git status failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let records: Vec<&[u8]> = out
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .collect();
        Ok(worktree_status_digest(&records))
    }

    async fn assert_target_not_foreign_worktree(
        &self,
        target: &IntegrationCheckout,
        target_ref: &str,
        symbolic_head: Option<&str>,
    ) -> LandingResult<()> {
        // If any linked worktree other than this checkout has the target branch
        // checked out, refuse: advancing the ref would silently diverge its
        // HEAD. `git worktree list --porcelain` lists every worktree's branch.
        let out = self
            .landing_git_stdout(&target.root, &["worktree", "list", "--porcelain"])
            .await?;
        let mut current_path: Option<PathBuf> = None;
        for line in out.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(path.trim()));
            } else if let Some(branch) = line.strip_prefix("branch ") {
                let branch = branch.trim();
                if branch == target_ref {
                    let is_self = current_path
                        .as_ref()
                        .and_then(|path| std::fs::canonicalize(path).ok())
                        .is_some_and(|canonical| canonical == target.root);
                    if !is_self {
                        return Err(ParentLandingError::UnownedCheckout(format!(
                            "target branch {target_ref} is checked out by another worktree"
                        )));
                    }
                    // The self worktree holding the branch is expected only when
                    // HEAD symbolically names it.
                    if symbolic_head != Some(target_ref) {
                        return Err(ParentLandingError::ParentDrift(
                            "integration worktree holds the target branch but HEAD is detached"
                                .to_owned(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Run scrubbed git in an owned checkout and return trimmed stdout, failing
    /// closed on a non-zero exit.
    async fn landing_git_stdout(&self, checkout: &Path, args: &[&str]) -> LandingResult<String> {
        let out = self.landing_git_capture(checkout, args).await?;
        if !out.status.success() {
            return Err(ParentLandingError::Git(format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// Run scrubbed git in an owned checkout and return only its exit code.
    async fn landing_git_status(&self, checkout: &Path, args: &[&str]) -> LandingResult<i32> {
        let out = self.landing_git_capture(checkout, args).await?;
        Ok(out.status.code().unwrap_or(-1))
    }

    async fn landing_git_capture(
        &self,
        checkout: &Path,
        args: &[&str],
    ) -> LandingResult<CapturedOutput> {
        let cmd = self.landing_git_command(checkout, None, &[], args);
        capture_bounded_process(cmd, self.landing_capture_limits(), None)
            .await
            .map_err(|error| ParentLandingError::Git(error.to_string()))
    }

    async fn quarantine_git_stdout(
        &self,
        target: &IntegrationCheckout,
        object_dir: &Path,
        alternates: &[PathBuf],
        args: &[&str],
    ) -> LandingResult<String> {
        let cmd = self.landing_git_command(&target.root, Some(object_dir), alternates, args);
        let out = capture_bounded_process(cmd, self.landing_capture_limits(), None)
            .await
            .map_err(|error| ParentLandingError::QuarantineRevalidation(error.to_string()))?;
        if !out.status.success() {
            return Err(ParentLandingError::QuarantineRevalidation(format!(
                "git {:?} in quarantine failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    async fn quarantine_git_status(
        &self,
        target: &IntegrationCheckout,
        object_dir: &Path,
        alternates: &[PathBuf],
        args: &[&str],
    ) -> LandingResult<i32> {
        let cmd = self.landing_git_command(&target.root, Some(object_dir), alternates, args);
        let out = capture_bounded_process(cmd, self.landing_capture_limits(), None)
            .await
            .map_err(|error| ParentLandingError::QuarantineRevalidation(error.to_string()))?;
        Ok(out.status.code().unwrap_or(-1))
    }
}

/// Recursively byte-copy a Git object store (`objects/`) from `source` to
/// `destination` through retained no-follow authorities. Skips `info/alternates`
/// (an object-store redirect) and refuses symlinked entries via `O_NOFOLLOW`.
fn copy_object_store(
    source: &SandboxDirectoryAuthority,
    destination: &SandboxDirectoryAuthority,
) -> LandingResult<()> {
    let mut entries: u64 = 0;
    // (source dir, destination dir, is objects root)
    let mut pending = vec![(source.clone(), destination.clone(), true)];
    while let Some((src_dir, dst_dir, is_root)) = pending.pop() {
        for name in src_dir.child_names().map_err(map_sandbox)? {
            entries = entries.checked_add(1).ok_or_else(|| {
                ParentLandingError::Io("object import entry count overflowed".to_owned())
            })?;
            if entries > MAX_IMPORT_ENTRIES {
                return Err(ParentLandingError::Io(
                    "object import exceeded the entry budget".to_owned(),
                ));
            }
            match src_dir.open_child_directory(&name) {
                Ok(child) => {
                    // Skip the object-store `info` directory's alternates by not
                    // copying `info` at all — a fresh store needs none of it.
                    if is_root && name == "info" {
                        continue;
                    }
                    let dst_child = dst_dir
                        .open_or_create_child_directory(&name)
                        .map_err(map_sandbox)?;
                    pending.push((child, dst_child, false));
                }
                Err(SandboxError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotADirectory =>
                {
                    let file = src_dir.open_child_file(&name).map_err(map_sandbox)?;
                    let length = file.len().map_err(map_sandbox)?;
                    if length > MAX_OBJECT_BYTES {
                        return Err(ParentLandingError::Io(format!(
                            "candidate object {name} exceeds the size budget"
                        )));
                    }
                    let bytes = file.read_bounded(MAX_OBJECT_BYTES).map_err(map_sandbox)?;
                    // Content-addressed: an already-present object is identical,
                    // so an AlreadyExists on create is a benign idempotent write.
                    match dst_dir.create_child_file(&name, &bytes) {
                        Ok(_) => {}
                        Err(SandboxError::Io(error))
                            if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(map_sandbox(error)),
                    }
                }
                Err(error) => return Err(map_sandbox(error)),
            }
        }
    }
    Ok(())
}

/// Bounded, deterministic SHA-256 digest (lowercase hex) over a sorted list of
/// `git status --porcelain=v1 -z` records. Binds the clean worktree status into
/// the parent preimage so any staged/unstaged/untracked change is drift.
fn worktree_status_digest(records: &[&[u8]]) -> String {
    let mut sorted: Vec<&[u8]> = records.to_vec();
    sorted.sort_unstable();
    let mut hasher = Sha256::new();
    hasher.update((sorted.len() as u64).to_le_bytes());
    for record in sorted {
        hasher.update((record.len() as u64).to_le_bytes());
        hasher.update(record);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest.iter() {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_object_id_accepts_sha1_and_sha256_only() {
        assert!(valid_object_id(&"a".repeat(40)));
        assert!(valid_object_id(&"0".repeat(64)));
        assert!(!valid_object_id(&"a".repeat(39)));
        assert!(!valid_object_id(&"a".repeat(41)));
        assert!(!valid_object_id("g".repeat(40).as_str()));
        assert!(!valid_object_id(""));
    }

    #[test]
    fn validate_target_ref_rejects_flags_traversal_and_metacharacters() {
        assert!(validate_target_ref("refs/heads/main").is_ok());
        assert!(validate_target_ref("refs/heads/feature/x-1.2").is_ok());
        for rejected in [
            "main",
            "refs/tags/v1",
            "refs/heads/",
            "refs/heads/-x",
            "refs/heads/a..b",
            "refs/heads/a//b",
            "refs/heads/a b",
            "refs/heads/a~1",
            "refs/heads/a^",
            "refs/heads/a:b",
            "refs/heads/a\\b",
            "refs/heads/x/",
        ] {
            assert!(
                validate_target_ref(rejected).is_err(),
                "accepted unsafe ref {rejected:?}"
            );
        }
    }

    #[test]
    fn ref_slug_is_filesystem_safe() {
        assert_eq!(ref_slug("refs/heads/feature/x"), "refs_heads_feature_x");
        assert_eq!(ref_slug("refs/heads/v1.2-rc"), "refs_heads_v1.2-rc");
        assert!(
            ref_slug("refs/heads/../escape")
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || byte == b'_'
                    || byte == b'-'
                    || byte == b'.')
        );
    }

    #[test]
    fn worktree_status_digest_is_order_independent_and_change_sensitive() {
        let a: &[u8] = b" M src/a.rs";
        let b: &[u8] = b"?? new.rs";
        let forward = worktree_status_digest(&[a, b]);
        let reversed = worktree_status_digest(&[b, a]);
        assert_eq!(forward, reversed, "status digest must be order independent");
        let empty = worktree_status_digest(&[]);
        assert_ne!(forward, empty, "a change must perturb the digest");
        assert_eq!(empty.len(), 64);
    }

    #[cfg(target_os = "linux")]
    mod live {
        use super::*;
        use crate::worktree::WorktreeManager;
        use std::path::Path;

        async fn git(repo: &Path, args: &[&str]) {
            let mut command = wcore_config::shell::shell_command_argv("git", args);
            command.current_dir(repo);
            let output = command.output().await.expect("run git fixture command");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        async fn init_repo(repo: &Path) {
            git(repo, &["init", "-b", "main"]).await;
            git(repo, &["config", "user.email", "wayland@example.invalid"]).await;
            git(repo, &["config", "user.name", "Wayland Test"]).await;
            std::fs::write(repo.join("README.md"), "base\n").unwrap();
            git(repo, &["add", "README.md"]).await;
            git(repo, &["commit", "-m", "base"]).await;
        }

        /// End-to-end: a real accepted candidate (base commit + mutated working
        /// tree) lands into a real integration checkout by exact CAS, projects
        /// the successor, and rolls back exactly.
        #[tokio::test]
        async fn lands_candidate_working_tree_and_rolls_back() {
            let source = tempfile::tempdir().unwrap();
            init_repo(source.path()).await;

            // Integration checkout: an independent clone at the base on `main`.
            let integration = tempfile::tempdir().unwrap();
            let integration_path = integration.path().join("checkout");
            git(
                source.path(),
                &[
                    "clone",
                    "--",
                    &source.path().to_string_lossy(),
                    &integration_path.to_string_lossy(),
                ],
            )
            .await;

            let state = tempfile::tempdir().unwrap();
            let checkouts = state.path().join("checkouts");
            std::fs::create_dir_all(&checkouts).unwrap();
            let manager =
                WorktreeManager::new_with_workspace_root(source.path(), &checkouts).unwrap();
            let pinned_head = manager.pinned_head().await.unwrap();
            let capacity = manager.workspace_capacity(1).await.unwrap();
            let workspace = manager
                .create_isolated_checkout(
                    "land-child",
                    "wayland-child/land-child",
                    &pinned_head,
                    capacity,
                )
                .await
                .unwrap();

            // Mutate the candidate working tree (the accepted mutation).
            let checkout_root = workspace.checkout_authority().display_path().to_path_buf();
            std::fs::write(checkout_root.join("added.txt"), "landed change\n").unwrap();
            let seal = workspace.seal_candidate().unwrap();

            let integration_canonical = std::fs::canonicalize(&integration_path).unwrap();
            let outcome = manager
                .land_candidate(
                    &workspace,
                    &seal,
                    &integration_canonical,
                    "refs/heads/main",
                    &pinned_head,
                    true,
                )
                .await
                .unwrap();
            let rollback = match outcome {
                LandingOutcome::Completed { rollback, .. } => *rollback,
                other => panic!("expected Completed, got {other:?}"),
            };

            // The mutation landed: file present, ref advanced past the base.
            assert!(integration_canonical.join("added.txt").is_file());
            let head = String::from_utf8(
                std::process::Command::new("git")
                    .current_dir(&integration_canonical)
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap();
            assert_ne!(head.trim(), pinned_head, "ref did not advance");

            // Rollback restores the base exactly.
            manager
                .rollback_landing(&integration_canonical, &rollback)
                .await
                .unwrap();
            assert!(!integration_canonical.join("added.txt").exists());
            let restored = String::from_utf8(
                std::process::Command::new("git")
                    .current_dir(&integration_canonical)
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap();
            assert_eq!(
                restored.trim(),
                pinned_head,
                "rollback did not restore base"
            );

            manager.release_transaction(&workspace).unwrap();
        }

        /// A candidate built on a stale base (parent tip already advanced) is a
        /// non-fast-forward conflict; no parent byte changes.
        #[tokio::test]
        async fn stale_base_is_a_conflict() {
            let source = tempfile::tempdir().unwrap();
            init_repo(source.path()).await;
            let integration = tempfile::tempdir().unwrap();
            let integration_path = integration.path().join("checkout");
            git(
                source.path(),
                &[
                    "clone",
                    "--",
                    &source.path().to_string_lossy(),
                    &integration_path.to_string_lossy(),
                ],
            )
            .await;
            // Advance the integration tip so the candidate base is stale. A
            // fresh clone inherits no identity config; set one for the fixture
            // commit (the landing itself never needs it — it uses commit-tree
            // with an explicit parent landing identity).
            git(
                &integration_path,
                &["config", "user.email", "wayland@example.invalid"],
            )
            .await;
            git(&integration_path, &["config", "user.name", "Wayland Test"]).await;
            std::fs::write(integration_path.join("drift.txt"), "drift\n").unwrap();
            git(&integration_path, &["add", "drift.txt"]).await;
            git(&integration_path, &["commit", "-m", "drift"]).await;
            let advanced = String::from_utf8(
                std::process::Command::new("git")
                    .current_dir(&integration_path)
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_owned();

            let state = tempfile::tempdir().unwrap();
            let checkouts = state.path().join("checkouts");
            std::fs::create_dir_all(&checkouts).unwrap();
            let manager =
                WorktreeManager::new_with_workspace_root(source.path(), &checkouts).unwrap();
            let pinned_head = manager.pinned_head().await.unwrap();
            let capacity = manager.workspace_capacity(1).await.unwrap();
            let workspace = manager
                .create_isolated_checkout(
                    "stale-child",
                    "wayland-child/stale-child",
                    &pinned_head,
                    capacity,
                )
                .await
                .unwrap();
            let checkout_root = workspace.checkout_authority().display_path().to_path_buf();
            std::fs::write(checkout_root.join("added.txt"), "x\n").unwrap();
            let seal = workspace.seal_candidate().unwrap();

            let integration_canonical = std::fs::canonicalize(&integration_path).unwrap();
            // Expected commit is the ADVANCED tip; the candidate base is the old
            // pinned head, so base != expected -> conflict.
            let outcome = manager
                .land_candidate(
                    &workspace,
                    &seal,
                    &integration_canonical,
                    "refs/heads/main",
                    &advanced,
                    true,
                )
                .await
                .unwrap();
            assert!(
                matches!(outcome, LandingOutcome::Conflict { .. }),
                "expected Conflict for a stale base, got {outcome:?}"
            );
            // The integration tip is unchanged.
            assert!(!integration_canonical.join("added.txt").exists());
            manager.release_transaction(&workspace).unwrap();
        }

        /// A `filter.*` clean/smudge driver written into the candidate `.git`
        /// AFTER sealing (with a benign sealed `.gitattributes` marking a path
        /// `filter=evil`) never executes and the landing fails closed: the
        /// pre-`add` re-scan rejects the non-benign config and the pinned parent
        /// `GIT_DIR` means `git add` would find no `filter.evil` driver anyway.
        /// The canary the filter would create is never produced and the parent
        /// ref does not move.
        #[tokio::test]
        async fn post_seal_filter_config_fails_closed_and_never_runs() {
            let source = tempfile::tempdir().unwrap();
            init_repo(source.path()).await;
            let integration = tempfile::tempdir().unwrap();
            let integration_path = integration.path().join("checkout");
            git(
                source.path(),
                &[
                    "clone",
                    "--",
                    &source.path().to_string_lossy(),
                    &integration_path.to_string_lossy(),
                ],
            )
            .await;
            let integration_canonical = std::fs::canonicalize(&integration_path).unwrap();
            let base = String::from_utf8(
                std::process::Command::new("git")
                    .current_dir(&integration_canonical)
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_owned();

            let state = tempfile::tempdir().unwrap();
            let checkouts = state.path().join("checkouts");
            std::fs::create_dir_all(&checkouts).unwrap();
            let manager =
                WorktreeManager::new_with_workspace_root(source.path(), &checkouts).unwrap();
            let pinned_head = manager.pinned_head().await.unwrap();
            let capacity = manager.workspace_capacity(1).await.unwrap();
            let workspace = manager
                .create_isolated_checkout(
                    "filter-child",
                    "wayland-child/filter-child",
                    &pinned_head,
                    capacity,
                )
                .await
                .unwrap();
            let checkout_root = workspace.checkout_authority().display_path().to_path_buf();

            // Sealed working tree: a benign mutation plus a `.gitattributes` that
            // marks every path with `filter=evil` (benign until a driver exists).
            std::fs::write(checkout_root.join("added.txt"), "change\n").unwrap();
            std::fs::write(checkout_root.join(".gitattributes"), "* filter=evil\n").unwrap();
            let seal = workspace.seal_candidate().unwrap();

            // Post-seal: plant the malicious clean driver into the candidate
            // `.git/config` (excluded from the sealed manifest). If it ever ran,
            // it would create this canary.
            let canary = source.path().join("FILTER_CANARY");
            let config_path = checkout_root.join(".git").join("config");
            let mut config = std::fs::read_to_string(&config_path).unwrap();
            config.push_str(&format!(
                "[filter \"evil\"]\n\tclean = touch {}\n",
                canary.display()
            ));
            std::fs::write(&config_path, config).unwrap();

            let outcome = manager
                .land_candidate(
                    &workspace,
                    &seal,
                    &integration_canonical,
                    "refs/heads/main",
                    &pinned_head,
                    true,
                )
                .await;
            assert!(
                outcome.is_err(),
                "landing must fail closed on a post-seal filter config, got {outcome:?}"
            );
            assert!(
                !canary.exists(),
                "the candidate-controlled clean filter must never execute"
            );
            let head = String::from_utf8(
                std::process::Command::new("git")
                    .current_dir(&integration_canonical)
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_owned();
            assert_eq!(head, base, "the parent ref must not move");
            manager.release_transaction(&workspace).unwrap();
        }
    }
}
