//! The gateway lifecycle state machine and the status projection an
//! operator reads.
//!
//! Phase 24 plan 24-01, Task 1.
//!
//! Every operator verb in `crates/wcore-cli/src/gateway.rs` drives exactly
//! one transition here and reads back exactly this projection. The machine
//! refuses every illegal transition BY NAME rather than no-op'ing, because
//! a silent no-op is indistinguishable from a performed action and the CLI
//! derives its exit status from the refusal variant.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Every state the gateway can be observed in.
///
/// `Uninstalled` and `Installed` describe the SERVICE registration;
/// `Stopped` through `Failed` describe the PROCESS. They are one enum
/// because an operator asks one question ("what is the gateway doing?")
/// and must not have to correlate two answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayState {
    /// No service registration exists for this home.
    Uninstalled,
    /// A service registration exists but no process is running.
    Installed,
    /// Registered and previously started; not currently running.
    Stopped,
    /// The process has been launched and has not yet reported ready.
    Starting,
    /// Admitting work.
    Running,
    /// Admission is closed; in-flight work is finishing or being abandoned.
    Draining,
    /// Drain finished. No work is in flight and the ledger is durable.
    Drained,
    /// The runtime recorded a failure. Distinct from `Stopped` so that a
    /// crash is never reported as a clean stop.
    Failed,
}

impl std::fmt::Display for GatewayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Uninstalled => "Uninstalled",
            Self::Installed => "Installed",
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Draining => "Draining",
            Self::Drained => "Drained",
            Self::Failed => "Failed",
        };
        f.write_str(s)
    }
}

/// The transitions the operator verbs drive. There is exactly one per verb
/// that changes state, plus the two the runtime itself reports (`Started`,
/// `DrainComplete`) and the one failure edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// `gateway install` — register the native service.
    Install,
    /// `gateway uninstall` — remove the native service registration.
    Uninstall,
    /// `gateway start` — launch the process.
    Start,
    /// Reported by the runtime once it is admitting work.
    Started,
    /// `gateway stop` — take the process down.
    Stop,
    /// `gateway drain` — close admission and finish in-flight work.
    Drain,
    /// Reported by the runtime when drain reached its terminal point.
    DrainComplete,
    /// Reported by the runtime when it broke.
    Fail,
}

impl std::fmt::Display for Transition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Install => "Install",
            Self::Uninstall => "Uninstall",
            Self::Start => "Start",
            Self::Started => "Started",
            Self::Stop => "Stop",
            Self::Drain => "Drain",
            Self::DrainComplete => "DrainComplete",
            Self::Fail => "Fail",
        };
        f.write_str(s)
    }
}

/// A refused transition. Three refusals have their own names because the
/// CLI returns a different exit status for each and an operator script
/// distinguishes them; everything else is the catch-all, which still names
/// both operands.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LifecycleError {
    #[error("gateway is already running")]
    AlreadyRunning,

    #[error("gateway is not running")]
    NotRunning,

    #[error("cannot Drain from {from}: drain requires a running gateway")]
    DrainRequiresRunning { from: GatewayState },

    #[error("illegal transition {transition} from {from}")]
    IllegalTransition {
        from: GatewayState,
        transition: Transition,
    },
}

impl GatewayState {
    /// Apply a transition, or refuse it by name.
    pub fn apply(self, transition: Transition) -> Result<GatewayState, LifecycleError> {
        use GatewayState as S;
        use Transition as T;

        // Named refusals first — these carry more information than the
        // catch-all and the CLI maps each to its own exit status.
        match (self, transition) {
            (S::Running | S::Starting | S::Draining, T::Start) => {
                return Err(LifecycleError::AlreadyRunning);
            }
            (S::Stopped | S::Installed | S::Uninstalled, T::Stop) => {
                return Err(LifecycleError::NotRunning);
            }
            (from, T::Drain) if from != S::Running => {
                return Err(LifecycleError::DrainRequiresRunning { from });
            }
            _ => {}
        }

        let next = match (self, transition) {
            // Failure is reachable from every live state. A runtime that
            // cannot record that it broke reports Running forever.
            (S::Starting | S::Running | S::Draining, T::Fail) => S::Failed,

            (S::Uninstalled, T::Install) => S::Installed,
            (S::Installed | S::Stopped | S::Failed, T::Start) => S::Starting,
            (S::Starting, T::Started) => S::Running,
            (S::Running, T::Drain) => S::Draining,
            (S::Draining, T::DrainComplete) => S::Drained,
            (S::Running | S::Draining | S::Drained | S::Starting | S::Failed, T::Stop) => {
                S::Stopped
            }
            (S::Installed | S::Stopped | S::Drained | S::Failed, T::Uninstall) => S::Uninstalled,

            (from, transition) => {
                return Err(LifecycleError::IllegalTransition { from, transition });
            }
        };
        Ok(next)
    }
}

/// The machine-readable status an operator reads from `gateway status`
/// and a host reads from the protocol.
///
/// `binary_path` and `binary_version` are what make an upgrade and a
/// rollback OBSERVABLE: without them, "restart through the service" and
/// "restart through the service on a new build" produce identical output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusProjection {
    pub state: GatewayState,
    /// The live process identity, or `None` when nothing is running.
    pub pid: Option<u32>,
    /// Seconds since the running process started, or `None`.
    pub uptime_secs: Option<u64>,
    /// The profile this gateway hosts. One gateway, one home, one profile
    /// — the supervisor's existing topology, not a second one.
    pub profile: String,
    /// Turns currently in flight. Drain publishes this falling to zero.
    pub turns_in_flight: usize,
    /// Deliveries accepted but not yet settled in the ledger.
    pub deliveries_pending: usize,
    /// The binary the running process was launched from.
    pub binary_path: Option<PathBuf>,
    /// That binary's reported version.
    pub binary_version: Option<String>,
}

impl StatusProjection {
    /// The projection for a gateway that is not running. It carries NO
    /// process identity and NO uptime: reporting a pid for a process that
    /// is gone is exactly how a status verb lies to an operator.
    pub fn stopped(profile: impl Into<String>) -> Self {
        Self {
            state: GatewayState::Stopped,
            pid: None,
            uptime_secs: None,
            profile: profile.into(),
            turns_in_flight: 0,
            deliveries_pending: 0,
            binary_path: None,
            binary_version: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninstall_is_refused_while_running() {
        assert!(matches!(
            GatewayState::Running.apply(Transition::Uninstall),
            Err(LifecycleError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn a_failed_gateway_can_be_restarted() {
        assert_eq!(
            GatewayState::Failed.apply(Transition::Start).unwrap(),
            GatewayState::Starting
        );
    }

    #[test]
    fn drain_refusal_names_the_state_for_every_non_running_state() {
        for from in [
            GatewayState::Uninstalled,
            GatewayState::Installed,
            GatewayState::Stopped,
            GatewayState::Starting,
            GatewayState::Draining,
            GatewayState::Drained,
            GatewayState::Failed,
        ] {
            match from.apply(Transition::Drain) {
                Err(LifecycleError::DrainRequiresRunning { from: named }) => {
                    assert_eq!(named, from)
                }
                other => panic!("{from} + Drain should name the state, got {other:?}"),
            }
        }
    }
}
