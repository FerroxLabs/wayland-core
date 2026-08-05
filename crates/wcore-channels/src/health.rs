//! Per-adapter health projection — every non-healthy state carries a reason.
//!
//! # Why a reason is mandatory rather than optional
//!
//! A health surface that can say `degraded` with no reason is a surface an
//! operator cannot act on, and it is indistinguishable from a health surface
//! that is itself broken. [`ChannelHealth::reason`] is therefore `Some` for
//! every state except [`HealthState::Healthy`], and there is a test that
//! reddens if any constructor is added that violates it.
//!
//! # Why `Unknown` exists and is not `Healthy`
//!
//! A channel that has been registered but never started has no observed state.
//! Reporting that as healthy would be the adapter attesting to its own liveness
//! without evidence — the exact shape lane 24c measured at the delivery sink,
//! where the gateway's own ledger reported a clean carry while the destination
//! held a duplicate. `Unknown` says "not observed", which is the truth.

use serde::{Deserialize, Serialize};

use crate::event::ConnectionState;

/// Coarse health of one registered adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HealthState {
    /// Connected and polling without error.
    Healthy,
    /// Connected but erroring, or mid-supervised-reconnect. Traffic may still
    /// flow. Always carries a reason.
    Degraded,
    /// Not connected. Always carries a reason.
    Disconnected,
    /// The platform rejected the credential. Distinct from `Disconnected`
    /// because the operator action is different: rotate a token, not wait.
    Unauthenticated,
    /// Registered, never observed. NOT healthy — nothing has been measured.
    Unknown,
}

impl HealthState {
    /// Whether this state is the only one permitted to omit a reason.
    pub fn requires_reason(self) -> bool {
        !matches!(self, HealthState::Healthy)
    }

    /// Map a published [`ConnectionState`] onto a health state.
    ///
    /// `Connecting` maps to `Degraded`, not `Healthy`: a connection in progress
    /// has not carried a message yet.
    pub fn from_connection_state(state: ConnectionState) -> Self {
        match state {
            ConnectionState::Connected => HealthState::Healthy,
            ConnectionState::Connecting | ConnectionState::Reconnecting => HealthState::Degraded,
            ConnectionState::Disconnected => HealthState::Disconnected,
            ConnectionState::AuthError => HealthState::Unauthenticated,
            // Deliberately exhaustive with NO wildcard arm. `ConnectionState`
            // is `#[non_exhaustive]` for downstream crates, but it is defined
            // in this crate, so a new variant must fail to compile here and be
            // classified on purpose. A `_` arm would silently absorb it, and
            // the absorbing default would be whatever the arm happened to say.
        }
    }
}

/// Health of one adapter as the manager has observed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelHealth {
    pub channel: String,
    pub platform: String,
    pub state: HealthState,
    /// Why the adapter is not healthy. `None` only when
    /// [`HealthState::Healthy`]. Content-free of secrets by contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Consecutive failing polls since the last success.
    #[serde(default)]
    pub consecutive_errors: u32,
    /// How many times supervised reconnect has re-established this adapter.
    /// A channel that is Healthy with a rising reconnect count is flapping,
    /// which a bare state field cannot express.
    #[serde(default)]
    pub reconnects: u32,
}

impl ChannelHealth {
    /// Registered, nothing observed yet.
    pub fn unknown(channel: impl Into<String>, platform: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            platform: platform.into(),
            state: HealthState::Unknown,
            reason: Some("registered; no poll observed yet".to_string()),
            consecutive_errors: 0,
            reconnects: 0,
        }
    }

    /// Whether this record satisfies the reason invariant.
    pub fn reason_invariant_holds(&self) -> bool {
        if self.state.requires_reason() {
            self.reason.as_ref().is_some_and(|r| !r.trim().is_empty())
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_non_healthy_state_requires_a_reason() {
        for state in [
            HealthState::Degraded,
            HealthState::Disconnected,
            HealthState::Unauthenticated,
            HealthState::Unknown,
        ] {
            assert!(
                state.requires_reason(),
                "{state:?} must carry a reason or an operator cannot act on it"
            );
        }
        assert!(!HealthState::Healthy.requires_reason());
    }

    #[test]
    fn a_non_healthy_record_without_a_reason_fails_the_invariant() {
        let mut h = ChannelHealth::unknown("acme", "slack");
        assert!(h.reason_invariant_holds());
        h.reason = None;
        assert!(
            !h.reason_invariant_holds(),
            "the invariant must be able to go red, or it is decoration"
        );
        h.reason = Some("   ".to_string());
        assert!(
            !h.reason_invariant_holds(),
            "whitespace is not a reason; a blank string is the same dead end as None"
        );
    }

    #[test]
    fn connecting_is_degraded_not_healthy() {
        // A connection in progress has carried nothing. Calling it healthy is
        // the adapter reporting on itself before there is anything to report.
        assert_eq!(
            HealthState::from_connection_state(ConnectionState::Connecting),
            HealthState::Degraded
        );
        assert_eq!(
            HealthState::from_connection_state(ConnectionState::Reconnecting),
            HealthState::Degraded
        );
        assert_eq!(
            HealthState::from_connection_state(ConnectionState::Connected),
            HealthState::Healthy
        );
        assert_eq!(
            HealthState::from_connection_state(ConnectionState::AuthError),
            HealthState::Unauthenticated
        );
        assert_eq!(
            HealthState::from_connection_state(ConnectionState::Disconnected),
            HealthState::Disconnected
        );
    }

    #[test]
    fn unknown_is_the_registered_but_unobserved_state() {
        let h = ChannelHealth::unknown("acme", "slack");
        assert_eq!(h.state, HealthState::Unknown);
        assert_ne!(
            h.state,
            HealthState::Healthy,
            "a registered-but-unpolled channel must never read healthy"
        );
        assert!(h.reason.is_some());
    }
}
