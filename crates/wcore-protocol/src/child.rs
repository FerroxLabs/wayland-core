//! Protocol-facing durable child model.
//!
//! These are direct re-exports of the canonical `wcore-types` model. The
//! protocol must not grow a second child state machine with subtly different
//! status, authority, or delivery semantics.

pub use wcore_types::spawner::{
    ChildDeliveryReconciliation, ChildDeliveryState, ChildDeliveryTarget, ChildDesiredState,
    ChildId, ChildOrigin, ChildParent, ChildPolicySnapshot, ChildRecoveryState,
    ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildError, DurableChildRecord, DurableChildResult,
    DurableChildStatus, DurableChildTransition,
};

/// Serialized durable-child protocol version.
pub const DURABLE_CHILD_PROTOCOL_VERSION: u16 = DURABLE_CHILD_SCHEMA_VERSION;
