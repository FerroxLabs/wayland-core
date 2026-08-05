//! Prepared tool-effect interfaces and durable filesystem receipt parsing.
//!
//! Ordinary host Write/Edit operations are opaque because supported host
//! filesystems cannot provide pathname compare-and-swap against
//! non-cooperating writers. The filesystem receipt types remain for safe
//! recovery of previously persisted receipts and for future storage backends
//! that can prove an authoritative revisioned transaction.

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
}

impl std::fmt::Debug for PreparedToolEffect {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedToolEffect")
            .field("receipt", &self.receipt)
            .field("preimage", &"[redacted]")
            .finish()
    }
}

impl PreparedToolEffect {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffectDisposition {
    Applied,
    AlreadyApplied,
    Conflict,
    Unknown,
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
