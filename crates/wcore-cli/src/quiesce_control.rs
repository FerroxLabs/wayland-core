//! JSON-stream bridge for the quiesced snapshot lease (wayland#896).
//!
//! Three host commands in, five receipts out. This module is the ONLY place the
//! wire contract in `wcore_protocol::quiescence` meets the mechanism in
//! `wcore_config::quiesce`, and it exists as its own file so the mapping from
//! mechanism refusal to wire refusal can be read and tested in one piece.
//!
//! ## It never returns nothing
//!
//! [`handle_quiesce_control`] answers every quiescence command with at least one
//! event. A control-plane command that is accepted and silently does nothing is
//! indistinguishable at the host from one that worked — the failure mode
//! `goal_control_refused` was added to close, and the reason a host that cannot
//! tell them apart will store an empty recovery point believing it succeeded.
//!
//! ## The boundary check runs before the filesystem
//!
//! Every arm validates the frame through the pure `wcore_protocol::quiescence`
//! validators BEFORE touching the control plane. `unsupported_version` in
//! particular must not be a verdict that depends on whether a directory
//! happened to exist: a predicate that is fail-open until some probe has run
//! looks identical to a working guard in any test that runs the probe first.

use wcore_config::quiesce::{
    CoveredRoot, ExpiredLease, LeaseRecord, LeaseRequest, LeaseScope, ProfileSelector,
    QuiesceError, ReleaseVerdict, RootIdentity,
};
use wcore_protocol::commands::{
    ProtocolCommand, QuiesceAcquireCommand, QuiesceReleaseCommand, QuiesceStatusCommand,
};
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::quiescence::{
    QUIESCENCE_PROTOCOL_VERSION, QuiesceCoverage, QuiesceHeldLease, QuiesceProfileIdentity,
    QuiesceProfileSelector, QuiesceRefusalReason, QuiesceReleaseVerdict, QuiesceRoot, QuiesceScope,
    validate_acquire, validate_release, validate_status,
};

/// True when this command belongs to the quiescence surface.
#[must_use]
pub fn is_quiesce_command(command: &ProtocolCommand) -> bool {
    matches!(
        command,
        ProtocolCommand::QuiesceAcquire(_)
            | ProtocolCommand::QuiesceRelease(_)
            | ProtocolCommand::QuiesceStatus(_)
    )
}

/// Answer one quiescence command.
///
/// Returns an empty vec ONLY for a command that is not a quiescence command;
/// every quiescence command produces at least one receipt.
#[must_use]
pub fn handle_quiesce_control(command: &ProtocolCommand) -> Vec<ProtocolEvent> {
    match command {
        ProtocolCommand::QuiesceAcquire(command) => acquire(command),
        ProtocolCommand::QuiesceRelease(command) => release(command),
        ProtocolCommand::QuiesceStatus(command) => status(command),
        _ => Vec::new(),
    }
}

fn acquire(command: &QuiesceAcquireCommand) -> Vec<ProtocolEvent> {
    if let Some(reason) = validate_acquire(
        command.quiescence_version,
        &command.request_id,
        &command.lease_id,
        &command.session_id,
        &command.scope,
        command.ttl_ms,
    ) {
        return vec![refusal(
            &command.request_id,
            &command.lease_id,
            &command.session_id,
            reason,
            boundary_detail(reason),
        )];
    }

    let request = LeaseRequest {
        lease_id: command.lease_id.clone(),
        owner: command.session_id.clone(),
        scope: to_scope(&command.scope),
        ttl_ms: command.ttl_ms,
    };
    match wcore_config::quiesce::acquire(&request) {
        Ok(grant) => {
            let mut events = Vec::with_capacity(2);
            // Order matters: the expiry receipt describes the lease that had to
            // be reclaimed BEFORE this one could be granted, so it precedes the
            // grant it made possible.
            if let Some(expired) = grant.reclaimed {
                events.push(expiry(
                    &expired,
                    &command.session_id,
                    &command.request_id,
                ));
            }
            events.push(ProtocolEvent::QuiesceLeaseGranted {
                quiescence_version: QUIESCENCE_PROTOCOL_VERSION,
                request_id: command.request_id.clone(),
                lease_id: grant.record.lease_id.clone(),
                session_id: command.session_id.clone(),
                epoch: grant.record.epoch.clone(),
                coverage: to_coverage(&grant.record.roots),
                acquired_unix_ms: grant.record.acquired_unix_ms,
                expires_unix_ms: grant.record.expires_unix_ms,
                idempotent_replay: grant.idempotent_replay,
            });
            events
        }
        Err(error) => vec![refusal(
            &command.request_id,
            &command.lease_id,
            &command.session_id,
            to_reason(&error),
            error.to_string(),
        )],
    }
}

fn release(command: &QuiesceReleaseCommand) -> Vec<ProtocolEvent> {
    if let Some(reason) = validate_release(
        command.quiescence_version,
        &command.request_id,
        &command.lease_id,
        &command.session_id,
        &command.epoch,
    ) {
        return vec![refusal(
            &command.request_id,
            &command.lease_id,
            &command.session_id,
            reason,
            boundary_detail(reason),
        )];
    }

    match wcore_config::quiesce::release(&command.lease_id, &command.epoch) {
        Ok(receipt) => vec![ProtocolEvent::QuiesceLeaseReleased {
            quiescence_version: QUIESCENCE_PROTOCOL_VERSION,
            request_id: command.request_id.clone(),
            lease_id: receipt.lease_id,
            session_id: command.session_id.clone(),
            epoch_at_acquire: receipt.epoch_at_acquire,
            epoch_at_release: receipt.epoch_at_release,
            verdict: match receipt.verdict {
                ReleaseVerdict::Clean => QuiesceReleaseVerdict::Clean,
                ReleaseVerdict::Mutated => QuiesceReleaseVerdict::Mutated,
            },
            released_unix_ms: receipt.released_unix_ms,
        }],
        Err(error) => vec![refusal(
            &command.request_id,
            &command.lease_id,
            &command.session_id,
            to_reason(&error),
            error.to_string(),
        )],
    }
}

fn status(command: &QuiesceStatusCommand) -> Vec<ProtocolEvent> {
    if let Some(reason) = validate_status(
        command.quiescence_version,
        &command.request_id,
        &command.session_id,
    ) {
        return vec![refusal(
            &command.request_id,
            "",
            &command.session_id,
            reason,
            boundary_detail(reason),
        )];
    }

    match wcore_config::quiesce::status() {
        Ok(report) => {
            let mut events = Vec::with_capacity(2);
            if let Some(expired) = report.reclaimed {
                events.push(expiry(&expired, &command.session_id, &command.request_id));
            }
            events.push(ProtocolEvent::QuiesceStatusReport {
                quiescence_version: QUIESCENCE_PROTOCOL_VERSION,
                request_id: command.request_id.clone(),
                session_id: command.session_id.clone(),
                held: report.held.as_ref().map(to_held),
                available: report.available.iter().map(to_identity).collect(),
            });
            events
        }
        Err(error) => vec![refusal(
            &command.request_id,
            "",
            &command.session_id,
            to_reason(&error),
            error.to_string(),
        )],
    }
}

// --- mapping ---------------------------------------------------------------

/// Mechanism refusal to wire refusal. Total over [`QuiesceError`], so a new
/// mechanism failure cannot quietly land on a neighbouring reason.
fn to_reason(error: &QuiesceError) -> QuiesceRefusalReason {
    match error {
        QuiesceError::PartialCoverage { .. } => QuiesceRefusalReason::PartialCoverage,
        QuiesceError::ConcurrentCapture { .. } => QuiesceRefusalReason::ConcurrentCapture,
        QuiesceError::StaleLease { .. } => QuiesceRefusalReason::StaleLease,
        QuiesceError::UnknownLease { .. } => QuiesceRefusalReason::UnknownLease,
        QuiesceError::ControlPlaneConflict { .. } => QuiesceRefusalReason::ControlPlaneConflict,
        QuiesceError::InvalidRequest(_) => QuiesceRefusalReason::InvalidRequest,
        QuiesceError::ControlPlaneUnavailable(_) => QuiesceRefusalReason::ControlPlaneUnavailable,
    }
}

/// Operator-facing text for a refusal decided at the protocol boundary, where
/// there is no mechanism error to quote.
fn boundary_detail(reason: QuiesceRefusalReason) -> String {
    match reason {
        QuiesceRefusalReason::UnsupportedVersion => format!(
            "quiescence_version must be {QUIESCENCE_PROTOCOL_VERSION}"
        ),
        QuiesceRefusalReason::PartialCoverage => {
            "the request covers no profile state".to_string()
        }
        _ => "the frame could not be honoured as written".to_string(),
    }
}

fn to_scope(scope: &QuiesceScope) -> LeaseScope {
    LeaseScope {
        include_default: scope.include_default,
        profiles: match &scope.profiles {
            QuiesceProfileSelector::All => ProfileSelector::All,
            QuiesceProfileSelector::Named { names } => ProfileSelector::Named(names.clone()),
        },
    }
}

fn to_identity(identity: &RootIdentity) -> QuiesceProfileIdentity {
    match identity {
        RootIdentity::Default => QuiesceProfileIdentity::Default,
        RootIdentity::Named { name } => QuiesceProfileIdentity::Named { name: name.clone() },
    }
}

fn to_coverage(roots: &[CoveredRoot]) -> QuiesceCoverage {
    QuiesceCoverage {
        roots: roots
            .iter()
            .map(|root| QuiesceRoot {
                identity: to_identity(&root.identity),
                path: root.path.display().to_string(),
                root_digest: root.digest.clone(),
                file_count: root.file_count,
                byte_count: root.byte_count,
            })
            .collect(),
        // Hard-wired true, and correct: a grant only exists when coverage was
        // complete, because `wcore_config::quiesce::acquire` refuses anything
        // less. It is on the wire so a host asserts the invariant on the frame
        // it received instead of inferring it from the absence of a refusal.
        complete: true,
    }
}

fn to_held(record: &LeaseRecord) -> QuiesceHeldLease {
    QuiesceHeldLease {
        lease_id: record.lease_id.clone(),
        owner: record.owner.clone(),
        epoch: record.epoch.clone(),
        acquired_unix_ms: record.acquired_unix_ms,
        expires_unix_ms: record.expires_unix_ms,
        coverage: to_coverage(&record.roots),
    }
}

fn expiry(expired: &ExpiredLease, session_id: &str, request_id: &str) -> ProtocolEvent {
    ProtocolEvent::QuiesceLeaseExpired {
        quiescence_version: QUIESCENCE_PROTOCOL_VERSION,
        lease_id: expired.lease_id.clone(),
        owner: expired.owner.clone(),
        session_id: session_id.to_string(),
        request_id: request_id.to_string(),
        epoch_at_acquire: expired.epoch.clone(),
        expires_unix_ms: expired.expires_unix_ms,
        observed_unix_ms: expired.observed_unix_ms,
    }
}

fn refusal(
    request_id: &str,
    lease_id: &str,
    session_id: &str,
    reason: QuiesceRefusalReason,
    detail: String,
) -> ProtocolEvent {
    ProtocolEvent::QuiesceRefused {
        quiescence_version: QUIESCENCE_PROTOCOL_VERSION,
        request_id: request_id.to_string(),
        lease_id: lease_id.to_string(),
        session_id: session_id.to_string(),
        reason,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acquire_command(version: u16, ttl_ms: u64) -> ProtocolCommand {
        ProtocolCommand::QuiesceAcquire(QuiesceAcquireCommand {
            quiescence_version: version,
            request_id: "req-1".into(),
            lease_id: "lease-1".into(),
            session_id: "sess-1".into(),
            scope: QuiesceScope {
                include_default: true,
                profiles: QuiesceProfileSelector::All,
            },
            ttl_ms,
        })
    }

    /// The boundary refusal must not require the filesystem to be in any
    /// particular state — that is the whole point of validating before the
    /// probe. This test runs with whatever control plane the host happens to
    /// have and must still refuse.
    #[test]
    fn an_unsupported_version_is_refused_without_touching_the_control_plane() {
        let events = handle_quiesce_control(&acquire_command(QUIESCENCE_PROTOCOL_VERSION + 1, 60_000));
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProtocolEvent::QuiesceRefused { reason, .. } => {
                assert_eq!(*reason, QuiesceRefusalReason::UnsupportedVersion);
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_out_of_range_ttl_is_refused_at_the_boundary() {
        let events = handle_quiesce_control(&acquire_command(QUIESCENCE_PROTOCOL_VERSION, 0));
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProtocolEvent::QuiesceRefused { reason, .. } => {
                assert_eq!(*reason, QuiesceRefusalReason::InvalidRequest);
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_non_quiescence_command_produces_nothing() {
        assert!(handle_quiesce_control(&ProtocolCommand::Ping).is_empty());
        assert!(!is_quiesce_command(&ProtocolCommand::Ping));
        assert!(is_quiesce_command(&acquire_command(
            QUIESCENCE_PROTOCOL_VERSION,
            60_000
        )));
    }

    /// Every mechanism refusal must reach the wire as its OWN reason. A mapping
    /// that collapsed two of them would build a host retry loop against a
    /// condition that will never clear.
    #[test]
    fn every_mechanism_refusal_maps_to_a_distinct_wire_reason() {
        let cases = [
            (
                QuiesceError::PartialCoverage {
                    missing: vec!["profile:archive".into()],
                },
                QuiesceRefusalReason::PartialCoverage,
            ),
            (
                QuiesceError::ConcurrentCapture {
                    holder_lease_id: "other".into(),
                    expires_unix_ms: 1,
                },
                QuiesceRefusalReason::ConcurrentCapture,
            ),
            (
                QuiesceError::StaleLease {
                    lease_id: "lease-1".into(),
                    detail: "moved".into(),
                },
                QuiesceRefusalReason::StaleLease,
            ),
            (
                QuiesceError::UnknownLease {
                    lease_id: "lease-1".into(),
                },
                QuiesceRefusalReason::UnknownLease,
            ),
            (
                QuiesceError::ControlPlaneConflict {
                    control: "/c".into(),
                    root: "/r".into(),
                },
                QuiesceRefusalReason::ControlPlaneConflict,
            ),
            (
                QuiesceError::InvalidRequest("bad"),
                QuiesceRefusalReason::InvalidRequest,
            ),
            (
                QuiesceError::ControlPlaneUnavailable("io".into()),
                QuiesceRefusalReason::ControlPlaneUnavailable,
            ),
        ];
        let mut seen = Vec::new();
        for (error, expected) in &cases {
            assert_eq!(to_reason(error), *expected);
            seen.push(*expected);
        }
        let mut unique = seen.clone();
        unique.sort_by_key(|reason| format!("{reason:?}"));
        unique.dedup_by_key(|reason| format!("{reason:?}"));
        assert_eq!(
            unique.len(),
            seen.len(),
            "two mechanism refusals collapsed onto one wire reason"
        );
    }
}
