//! Prepared tool-effect interfaces and durable filesystem receipt parsing.
//!
//! # What a host filesystem can and cannot prove
//!
//! Supported host filesystems cannot provide pathname compare-and-swap
//! against a non-cooperating writer, so nothing here can make a write
//! *exclusive*. [`VirtualFs::compare_exchange_file`] therefore stays
//! unimplemented for [`crate::vfs::RealFs`] and always will.
//!
//! That is a statement about PREVENTION. It is not a statement about
//! CLASSIFICATION, and F13 only ever needed the second one. A receipt
//! prepared before the write records three things that a later read can be
//! compared against: the exact preimage identity, the exact intended
//! postimage identity, and the kernel object the preparation saw
//! (device/inode/nlink/mode/uid/gid on unix, the equivalent on Windows).
//! After a crash, one read of the target answers:
//!
//! * bytes equal the intended postimage -> the write DID land
//! * bytes equal the preimage AND the object is still the prepared one
//!   -> the write did NOT land
//! * anything else -> **cannot tell**
//!
//! The third answer is the load-bearing one. A partial write, a racing
//! third-party writer, a replaced inode, a changed mode: all of them land
//! there, stay [`FilesystemReconciliation::Conflict`], and reach the operator
//! as an unresolved unknown. A reconciler that guessed at that case would be
//! strictly worse than the honest question it replaced, so every uncertain
//! path in this module fails toward `Conflict`/`Unknown` rather than toward
//! an answer.
//!
//! Preparation is also best-effort on purpose. [`prepare_filesystem_effect`]
//! returns `None` — never an error — whenever the evidence cannot be captured
//! (an unobservable target such as a symlink or a hard link, a backend with
//! no identity primitive, a preimage larger than
//! [`MAX_PREPARED_PREIMAGE_BYTES`]). The tool then runs its ordinary path and
//! a crash there remains exactly the operator question it was before.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wcore_types::tool::ToolResult;

use crate::path_validation::validate_user_path;
use crate::vfs::{
    FileContentIdentity, FileObjectIdentity, FileObservation, FilePrecondition, VfsError, VirtualFs,
};

pub const FILESYSTEM_EFFECT_RECEIPT_VERSION: u32 = 1;
pub const FILESYSTEM_EFFECT_RECONCILER: &str = "wcore.filesystem.compare_exchange.v1";

/// Largest preimage or postimage a prepared filesystem effect will bind.
///
/// A present precondition is only reconcilable while its preimage is durably
/// checkpointed, and the journal's checkpoint store is quota'd per blob and
/// per session. Refusing to prepare above this bound keeps a session editing
/// unusually large files from spending that quota and then being unable to
/// write at all; those calls simply stay opaque, which is where they were
/// before this seam existed.
pub const MAX_PREPARED_PREIMAGE_BYTES: u64 = 4 * 1024 * 1024;

/// Content identity stored in a durable receipt. It contains no file bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesystemContentIdentity {
    pub sha256: String,
    pub len: u64,
}

impl From<FileContentIdentity> for FilesystemContentIdentity {
    fn from(identity: FileContentIdentity) -> Self {
        Self {
            sha256: identity.sha256_hex(),
            len: identity.len,
        }
    }
}

impl FilesystemContentIdentity {
    fn matches(&self, identity: FileContentIdentity) -> bool {
        self.len == identity.len && self.sha256 == identity.sha256_hex()
    }

    fn is_valid(&self) -> bool {
        self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

/// Exact target state observed during preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilesystemEffectPrecondition {
    Absent,
    Present { identity: FilesystemContentIdentity },
}

/// Versioned, content-free intent persisted before physical execution starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesystemEffectReceiptV1 {
    pub version: u32,
    pub reconciler: String,
    pub path: PathBuf,
    pub preparation_object: FileObjectIdentity,
    pub precondition: FilesystemEffectPrecondition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_identity: Option<FilesystemContentIdentity>,
    pub intended: FilesystemContentIdentity,
}

impl FilesystemEffectReceiptV1 {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn checkpoint_identity(&self) -> Option<&FilesystemContentIdentity> {
        self.checkpoint_identity.as_ref()
    }

    #[must_use]
    pub fn precondition_identity(&self) -> Option<&FilesystemContentIdentity> {
        match &self.precondition {
            FilesystemEffectPrecondition::Absent => None,
            FilesystemEffectPrecondition::Present { identity } => Some(identity),
        }
    }

    /// Validate the complete persisted receipt without touching the target.
    /// Reducers call this immediately before granting physical start authority;
    /// recovery calls it again before trusting any filesystem observation.
    pub fn validate(&self) -> Result<(), VfsError> {
        let validated_path = validate_user_path(&self.path).map_err(|error| {
            VfsError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid filesystem effect receipt path: {error}"),
            ))
        })?;
        let checkpoint_is_valid = match (&self.precondition, &self.checkpoint_identity) {
            (FilesystemEffectPrecondition::Absent, None) => true,
            (FilesystemEffectPrecondition::Present { identity }, Some(checkpoint_identity)) => {
                identity == checkpoint_identity && checkpoint_identity.is_valid()
            }
            _ => false,
        };
        let object_tokens_are_valid = !self.preparation_object.authority.is_empty()
            && !self.preparation_object.path.as_os_str().is_empty()
            && self
                .preparation_object
                .parent
                .as_ref()
                .is_none_or(|token| !token.is_empty())
            && self
                .preparation_object
                .file
                .as_ref()
                .is_none_or(|token| !token.is_empty());
        let object_matches_precondition = match &self.precondition {
            FilesystemEffectPrecondition::Absent => self.preparation_object.file.is_none(),
            FilesystemEffectPrecondition::Present { .. } => {
                self.preparation_object.parent.is_some() && self.preparation_object.file.is_some()
            }
        };
        if self.version != FILESYSTEM_EFFECT_RECEIPT_VERSION
            || self.reconciler != FILESYSTEM_EFFECT_RECONCILER
            || validated_path != self.path
            || !object_tokens_are_valid
            || !object_matches_precondition
            || !self.intended.is_valid()
            || !checkpoint_is_valid
            || matches!(
                &self.precondition,
                FilesystemEffectPrecondition::Present { identity } if !identity.is_valid()
            )
        {
            return Err(VfsError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported filesystem effect receipt",
            )));
        }
        Ok(())
    }

    /// Reconcile current bytes against the exact prepared pre/post identities.
    /// This method never writes.
    pub async fn reconcile(
        &self,
        vfs: &dyn VirtualFs,
    ) -> Result<FilesystemReconciliation, VfsError> {
        self.validate()?;

        let current = vfs.observe_file(&self.path).await?;
        if !self.preparation_object.same_path_authority(&current.object) {
            return Ok(FilesystemReconciliation::Conflict {
                current: current.observation,
            });
        }
        if observation_matches_identity(current.observation, &self.intended) {
            let byte_identical_noop = matches!(
                &self.precondition,
                FilesystemEffectPrecondition::Present { identity } if identity == &self.intended
            );
            if byte_identical_noop
                && !self
                    .preparation_object
                    .same_prepared_object(&current.object)
            {
                return Ok(FilesystemReconciliation::Conflict {
                    current: current.observation,
                });
            }
            return Ok(FilesystemReconciliation::AlreadyApplied {
                current: current.observation,
            });
        }
        if self
            .preparation_object
            .same_prepared_object(&current.object)
            && receipt_precondition_matches(&self.precondition, current.observation)
        {
            return Ok(FilesystemReconciliation::NotStarted {
                current: current.observation,
            });
        }
        Ok(FilesystemReconciliation::Conflict {
            current: current.observation,
        })
    }
}

/// Content-only observation suitable for persisted reconciliation evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilesystemObservationReceipt {
    Absent,
    Present { identity: FilesystemContentIdentity },
}

impl From<FileObservation> for FilesystemObservationReceipt {
    fn from(observation: FileObservation) -> Self {
        match observation {
            FileObservation::Absent => Self::Absent,
            FileObservation::Present(identity) => Self::Present {
                identity: identity.into(),
            },
        }
    }
}

/// Read-only recovery classification for a durable filesystem receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesystemReconciliation {
    AlreadyApplied { current: FileObservation },
    NotStarted { current: FileObservation },
    Conflict { current: FileObservation },
}

/// Runtime-only prepared effect for a backend with an authoritative receipt.
///
/// No ordinary host filesystem tool constructs this type. It is retained at
/// the dispatcher boundary so a future revisioned/cooperative backend can opt
/// in without weakening opaque-by-default recovery.
#[derive(Clone)]
pub struct PreparedToolEffect {
    receipt: FilesystemEffectReceiptV1,
    preimage: Option<Vec<u8>>,
    invocation: Value,
}

impl std::fmt::Debug for PreparedToolEffect {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedToolEffect")
            .field("receipt", &self.receipt)
            .field("preimage", &"[redacted]")
            .field("invocation", &"[redacted]")
            .finish()
    }
}

impl PreparedToolEffect {
    /// The exact tool input this receipt was prepared against.
    ///
    /// `Tool::execute_prepared_effect` receives no input of its own — the
    /// dispatcher hands it only the prepared effect — so the invocation
    /// travels with the preparation. Runtime-only: it is never serialized
    /// into the durable receipt.
    #[must_use]
    pub fn invocation(&self) -> &Value {
        &self.invocation
    }

    pub fn durable_receipt(&self) -> Result<Value, serde_json::Error> {
        serde_json::to_value(&self.receipt)
    }

    #[must_use]
    pub fn filesystem_receipt(&self) -> &FilesystemEffectReceiptV1 {
        &self.receipt
    }

    /// Exact preimage for storage in the journal's private checkpoint blob
    /// store. These bytes are runtime-only and never included in the receipt.
    #[must_use]
    pub fn preimage_bytes(&self) -> Option<&[u8]> {
        self.preimage.as_deref()
    }
}

impl From<FilePrecondition> for FilesystemEffectPrecondition {
    fn from(precondition: FilePrecondition) -> Self {
        match precondition {
            FilePrecondition::Absent => Self::Absent,
            FilePrecondition::Present(identity) => Self::Present {
                identity: identity.into(),
            },
        }
    }
}

/// Whether the prepared physical boundary produced authoritative evidence.
///
/// Only `Unknown` is ambiguous. Every other variant asserts that this process
/// knows what happened to the world, and orchestration uses that to keep an
/// ordinary tool error (a refused path, a string that did not match) from
/// being recorded as a reconcilable unknown that blocks the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffectDisposition {
    /// The intended mutation reached the target during this dispatch.
    Applied,
    /// The target already held the intended bytes; the mutation was a no-op.
    AlreadyApplied,
    /// The mutation definitely did not reach the target. Either the tool
    /// never called the filesystem, or it called it, failed, and the target
    /// was afterwards observed still holding the prepared preimage on the
    /// prepared object.
    NotApplied,
    /// The target no longer matches the prepared identities and the outcome
    /// was decided against a different object than the one prepared.
    Conflict,
    /// Nothing in this process observed the effect resolve. This is the case
    /// that must reach an operator rather than be guessed at.
    Unknown,
}

/// How far the physical write got inside this process.
///
/// The filesystem alone cannot separate "the tool refused before writing"
/// from "the tool wrote and someone else immediately overwrote it", so this
/// in-process fact is recorded at the write call itself rather than inferred
/// afterwards from bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilesystemWriteAttempt {
    /// The tool returned before it asked the filesystem to do anything.
    #[default]
    NotAttempted,
    /// The write was issued and did not report success.
    Attempted,
    /// The write reported success.
    Completed,
}

/// Result returned to orchestration after executing a prepared effect.
pub struct ToolEffectExecution {
    pub result: ToolResult,
    pub disposition: ToolEffectDisposition,
    pub observed_receipt: Value,
}

impl ToolEffectExecution {
    pub(crate) fn unknown(result: ToolResult, observed_receipt: Value) -> Self {
        Self {
            result,
            disposition: ToolEffectDisposition::Unknown,
            observed_receipt,
        }
    }
}

/// Capture, before a single byte is written, the evidence recovery needs to
/// answer "did this write land?" positively rather than by guessing.
///
/// `postimage` is handed the observed preimage (`None` when the target is
/// absent) and returns the exact bytes the caller is about to write. Both the
/// closure and this function return `None` to mean "stay opaque"; neither
/// ever reports an error, because failing to prepare must leave the caller on
/// the behaviour it had before preparation existed.
pub async fn prepare_filesystem_effect<F>(
    vfs: &dyn VirtualFs,
    path: &Path,
    invocation: &Value,
    postimage: F,
) -> Option<PreparedToolEffect>
where
    F: FnOnce(Option<&[u8]>) -> Option<Vec<u8>>,
{
    // The receipt's own `validate` re-runs this on every load, so a path that
    // would not survive the round trip must not be bound in the first place.
    let validated = validate_user_path(path).ok()?;
    let observed = vfs.observe_file(&validated).await.ok()?;
    let preimage = observed.contents().map(<[u8]>::to_vec);
    if preimage
        .as_ref()
        .is_some_and(|bytes| bytes.len() as u64 > MAX_PREPARED_PREIMAGE_BYTES)
    {
        return None;
    }
    let intended_bytes = postimage(preimage.as_deref())?;
    if intended_bytes.len() as u64 > MAX_PREPARED_PREIMAGE_BYTES {
        return None;
    }

    let precondition = FilesystemEffectPrecondition::from(match observed.observation {
        FileObservation::Absent => FilePrecondition::Absent,
        FileObservation::Present(identity) => FilePrecondition::Present(identity),
    });
    let checkpoint_identity = match &precondition {
        FilesystemEffectPrecondition::Absent => None,
        FilesystemEffectPrecondition::Present { identity } => Some(identity.clone()),
    };
    // Recovery declines any receipt whose checkpoint it cannot load, so a
    // present precondition without the bytes behind it is not preparable.
    if checkpoint_identity.is_some() && preimage.is_none() {
        return None;
    }

    let receipt = FilesystemEffectReceiptV1 {
        version: FILESYSTEM_EFFECT_RECEIPT_VERSION,
        reconciler: FILESYSTEM_EFFECT_RECONCILER.to_string(),
        path: validated,
        preparation_object: observed.object,
        precondition,
        checkpoint_identity,
        intended: FileContentIdentity::from_bytes(&intended_bytes).into(),
    };
    // Refuse here rather than at the durable start boundary, where the same
    // failure would refuse the tool call outright.
    receipt.validate().ok()?;

    Some(PreparedToolEffect {
        receipt,
        preimage,
        invocation: invocation.clone(),
    })
}

/// Decide what this dispatch proved about the prepared effect.
///
/// The in-process [`FilesystemWriteAttempt`] settles the two common cases on
/// its own. Only a write that was issued and did not report success needs the
/// filesystem consulted, and only a target matching neither prepared identity
/// yields [`ToolEffectDisposition::Unknown`].
pub async fn classify_filesystem_execution(
    prepared: &PreparedToolEffect,
    vfs: &dyn VirtualFs,
    attempt: FilesystemWriteAttempt,
    result: ToolResult,
) -> ToolEffectExecution {
    let receipt = prepared.filesystem_receipt();
    let no_op = matches!(
        &receipt.precondition,
        FilesystemEffectPrecondition::Present { identity } if identity == &receipt.intended
    );
    let (disposition, outcome, observed) = match attempt {
        FilesystemWriteAttempt::NotAttempted => {
            (ToolEffectDisposition::NotApplied, "not_attempted", None)
        }
        FilesystemWriteAttempt::Completed => (
            if no_op {
                ToolEffectDisposition::AlreadyApplied
            } else {
                ToolEffectDisposition::Applied
            },
            "write_completed",
            None,
        ),
        FilesystemWriteAttempt::Attempted => match receipt.reconcile(vfs).await {
            Ok(FilesystemReconciliation::AlreadyApplied { current }) => (
                ToolEffectDisposition::Applied,
                "failed_write_left_the_intended_bytes",
                Some(current),
            ),
            Ok(FilesystemReconciliation::NotStarted { current }) => (
                ToolEffectDisposition::NotApplied,
                "failed_write_left_the_preimage",
                Some(current),
            ),
            Ok(FilesystemReconciliation::Conflict { current }) => (
                ToolEffectDisposition::Unknown,
                "failed_write_left_neither_identity",
                Some(current),
            ),
            Err(_) => (
                ToolEffectDisposition::Unknown,
                "failed_write_could_not_be_observed",
                None,
            ),
        },
    };
    let observed_receipt = serde_json::json!({
        "reconciler": FILESYSTEM_EFFECT_RECONCILER,
        "outcome": outcome,
        "observed": observed.map(FilesystemObservationReceipt::from),
    });
    ToolEffectExecution {
        result,
        disposition,
        observed_receipt,
    }
}

fn observation_matches_identity(
    observation: FileObservation,
    expected: &FilesystemContentIdentity,
) -> bool {
    matches!(observation, FileObservation::Present(current) if expected.matches(current))
}

fn receipt_precondition_matches(
    expected: &FilesystemEffectPrecondition,
    observation: FileObservation,
) -> bool {
    match (expected, observation) {
        (FilesystemEffectPrecondition::Absent, FileObservation::Absent) => true,
        (
            FilesystemEffectPrecondition::Present { identity: expected },
            FileObservation::Present(current),
        ) => expected.matches(current),
        _ => false,
    }
}
