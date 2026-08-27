//! Wire contract for the quiesced snapshot lease (wayland#896).
//!
//! Desktop's recovery-point capture cannot be honest without a producer-owned
//! quiescence window. This module is the versioned, ADDITIVE half a host reads:
//! three commands, five events, and the closed refusal vocabulary that tells a
//! host which of its assumptions was wrong.
//!
//! ## What is NOT re-spelled here
//!
//! The mechanism — enumerating roots, taking the epoch, holding and reclaiming
//! the lease — lives in `wcore_config::quiesce`, which owns `profile_home()`
//! and `profiles_root()`. `wcore-protocol` deliberately does not depend on
//! `wcore-config` (the decoupling the crate map calls for), so these are
//! mirror types plus the boundary checks that need no filesystem at all.
//!
//! ## The boundary check runs BEFORE any probe
//!
//! [`validate_acquire`] is total and pure: a host frame is judged on its own
//! contents. That ordering is the point. A predicate that is fail-OPEN until
//! some probe has run looks identical to a working guard in every test that
//! runs the probe first — this codebase has already shipped one. Version and
//! shape are settled here, with no I/O, so `unsupported_version` cannot be a
//! verdict that depends on whether a directory happened to exist.
//!
//! ## Every refusal is a receipt
//!
//! There is no silent drop. A refused command answers with
//! [`crate::events::ProtocolEvent::QuiesceRefused`] carrying a closed
//! [`QuiesceRefusalReason`], for the same reason `goal_control_refused` exists:
//! a control-plane command that is accepted and does nothing is
//! indistinguishable from one that worked.

use serde::{Deserialize, Serialize};

/// Wire version for the quiesced-snapshot-lease contract.
///
/// Separate from `CONTRACT_MINOR`, like `goal_version` and `recovery_version`:
/// a host must be able to reason about this subcontract's shape without
/// decoding the whole descriptor first.
pub const QUIESCENCE_PROTOCOL_VERSION: u16 = 1;

/// Shortest lease a host may request, in milliseconds.
pub const MIN_LEASE_TTL_MS: u64 = 1_000;

/// Longest lease a host may request, in milliseconds. A lease is a write
/// freeze; an unbounded one is an outage with a receipt.
pub const MAX_LEASE_TTL_MS: u64 = 15 * 60 * 1_000;

/// Bound on every opaque identifier a host supplies on this surface.
pub const MAX_IDENTIFIER_LEN: usize = 128;

/// Bound on how many profiles one explicit selection may name.
pub const MAX_SELECTED_PROFILES: usize = 256;

/// Which profile a covered root is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuiesceProfileIdentity {
    /// The active default home (`WAYLAND_HOME`, else `~/.wayland`).
    Default,
    /// A named profile under the profiles root.
    Named { name: String },
}

/// One root the lease covers.
///
/// `path` is present because Desktop has to copy the root; the lease is
/// worthless if the host has to guess where the state is. `root_digest` lets a
/// host attribute a `mutated` verdict to a specific root instead of retrying
/// the whole capture blind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuiesceRoot {
    pub identity: QuiesceProfileIdentity,
    pub path: String,
    pub root_digest: String,
    pub file_count: u64,
    pub byte_count: u64,
}

/// What a granted lease covers.
///
/// `complete` is always `true` on a grant — incomplete coverage is refused, not
/// reported — and exists so a host can assert the invariant on the frame it
/// actually received rather than inferring it from the absence of a refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuiesceCoverage {
    pub roots: Vec<QuiesceRoot>,
    pub complete: bool,
}

/// Which named profiles a host wants covered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "select", rename_all = "snake_case")]
pub enum QuiesceProfileSelector {
    /// Every profile the producer enumerates at acquire time. The honest
    /// default: a host that hardcodes a list silently misses a profile created
    /// since it last looked.
    All,
    /// Exactly these. All must exist, or the request is refused for partial
    /// coverage.
    Named { names: Vec<String> },
}

/// What a lease must cover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuiesceScope {
    /// Cover the default home. Defaults to `true`: a recovery point that
    /// silently omits the default profile is the failure this contract exists
    /// to prevent.
    #[serde(default = "default_include_default")]
    pub include_default: bool,
    pub profiles: QuiesceProfileSelector,
}

fn default_include_default() -> bool {
    true
}

/// Closed vocabulary of quiescence refusals.
///
/// Kept apart deliberately. `ConcurrentCapture` clears on its own and a host
/// should back off; `PartialCoverage` never clears without the host changing
/// the request; `StaleLease` means the host's view moved and it must resync.
/// Collapsing any two builds a retry loop against a condition that will never
/// clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuiesceRefusalReason {
    /// `quiescence_version` is not the version this Core speaks.
    UnsupportedVersion,
    /// A requested root is absent or unreadable, or the request covers none.
    PartialCoverage,
    /// A different live lease already holds capture.
    ConcurrentCapture,
    /// The host acted on a lease view that has since moved — a reused id under
    /// a different scope, an epoch echo that was never granted, or a lease that
    /// lapsed before release.
    StaleLease,
    /// No lease with that id is held.
    UnknownLease,
    /// The lease control plane resolves inside a covered root, so recording the
    /// lease would mutate the state it freezes.
    ControlPlaneConflict,
    /// The frame could not be honoured as written.
    InvalidRequest,
    /// The control plane itself could not be read or written.
    ControlPlaneUnavailable,
}

/// Whether the covered state moved while the lease was held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuiesceReleaseVerdict {
    /// Every covered root hashes exactly as it did at acquire. The capture
    /// taken under this lease is a valid recovery point.
    Clean,
    /// Something moved. The capture is NOT a valid recovery point, and a host
    /// that stores it anyway has stored a torn snapshot.
    Mutated,
}

/// A live lease, as `quiesce_status` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuiesceHeldLease {
    pub lease_id: String,
    pub owner: String,
    pub epoch: String,
    pub acquired_unix_ms: u64,
    pub expires_unix_ms: u64,
    pub coverage: QuiesceCoverage,
}

/// Pure, total validation of an acquire frame.
///
/// Runs before any filesystem probe — see the module note on fail-open
/// predicates. Returns the FIRST reason the frame is unacceptable, with version
/// checked first so a host speaking a future dialect is told that rather than
/// being told its scope is malformed.
#[must_use]
pub fn validate_acquire(
    quiescence_version: u16,
    request_id: &str,
    lease_id: &str,
    session_id: &str,
    scope: &QuiesceScope,
    ttl_ms: u64,
) -> Option<QuiesceRefusalReason> {
    if quiescence_version != QUIESCENCE_PROTOCOL_VERSION {
        return Some(QuiesceRefusalReason::UnsupportedVersion);
    }
    if !identifier_ok(request_id) || !identifier_ok(lease_id) || !identifier_ok(session_id) {
        return Some(QuiesceRefusalReason::InvalidRequest);
    }
    if !(MIN_LEASE_TTL_MS..=MAX_LEASE_TTL_MS).contains(&ttl_ms) {
        return Some(QuiesceRefusalReason::InvalidRequest);
    }
    match &scope.profiles {
        QuiesceProfileSelector::All => {}
        QuiesceProfileSelector::Named { names } => {
            if names.is_empty() || names.len() > MAX_SELECTED_PROFILES {
                return Some(QuiesceRefusalReason::InvalidRequest);
            }
            if names.iter().any(|name| !profile_name_ok(name)) {
                return Some(QuiesceRefusalReason::InvalidRequest);
            }
        }
    }
    // A request that covers nothing is a coverage failure, not a shape failure:
    // the host asked for a complete capture of the empty set.
    if !scope.include_default
        && matches!(&scope.profiles, QuiesceProfileSelector::Named { names } if names.is_empty())
    {
        return Some(QuiesceRefusalReason::PartialCoverage);
    }
    None
}

/// Pure, total validation of a release frame.
#[must_use]
pub fn validate_release(
    quiescence_version: u16,
    request_id: &str,
    lease_id: &str,
    session_id: &str,
    epoch: &str,
) -> Option<QuiesceRefusalReason> {
    if quiescence_version != QUIESCENCE_PROTOCOL_VERSION {
        return Some(QuiesceRefusalReason::UnsupportedVersion);
    }
    if !identifier_ok(request_id) || !identifier_ok(lease_id) || !identifier_ok(session_id) {
        return Some(QuiesceRefusalReason::InvalidRequest);
    }
    if !epoch_ok(epoch) {
        return Some(QuiesceRefusalReason::InvalidRequest);
    }
    None
}

/// Pure, total validation of a status frame.
#[must_use]
pub fn validate_status(
    quiescence_version: u16,
    request_id: &str,
    session_id: &str,
) -> Option<QuiesceRefusalReason> {
    if quiescence_version != QUIESCENCE_PROTOCOL_VERSION {
        return Some(QuiesceRefusalReason::UnsupportedVersion);
    }
    if !identifier_ok(request_id) || !identifier_ok(session_id) {
        return Some(QuiesceRefusalReason::InvalidRequest);
    }
    None
}

fn identifier_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// Profile names on this surface follow the producer's own grammar: ASCII
/// letters, digits, `.`, `_`, `-`, at most 64 bytes. Rejecting here keeps a
/// path separator from ever reaching a root resolver.
fn profile_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !name.starts_with('.')
}

/// An epoch is the producer's opaque token. A host may only echo one back, so
/// the boundary check is a shape check, never an interpretation.
fn epoch_ok(epoch: &str) -> bool {
    !epoch.is_empty()
        && epoch.len() <= 128
        && epoch
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_all() -> QuiesceScope {
        QuiesceScope {
            include_default: true,
            profiles: QuiesceProfileSelector::All,
        }
    }

    #[test]
    fn acquire_validation_rejects_a_future_dialect_before_anything_else() {
        // Version is checked FIRST: a frame that is both a future version and
        // malformed must be told about the version, or the host chases the
        // wrong repair.
        let bad_scope = QuiesceScope {
            include_default: true,
            profiles: QuiesceProfileSelector::Named { names: Vec::new() },
        };
        assert_eq!(
            validate_acquire(
                QUIESCENCE_PROTOCOL_VERSION + 1,
                "req-1",
                "lease-1",
                "sess-1",
                &bad_scope,
                60_000
            ),
            Some(QuiesceRefusalReason::UnsupportedVersion)
        );
    }

    #[test]
    fn acquire_validation_is_pure_and_accepts_a_well_formed_frame() {
        assert_eq!(
            validate_acquire(
                QUIESCENCE_PROTOCOL_VERSION,
                "req-1",
                "lease-1",
                "sess-1",
                &scope_all(),
                60_000
            ),
            None
        );
    }

    #[test]
    fn acquire_validation_bounds_the_ttl_on_both_sides() {
        for ttl in [0, MIN_LEASE_TTL_MS - 1, MAX_LEASE_TTL_MS + 1, u64::MAX] {
            assert_eq!(
                validate_acquire(
                    QUIESCENCE_PROTOCOL_VERSION,
                    "req-1",
                    "lease-1",
                    "sess-1",
                    &scope_all(),
                    ttl
                ),
                Some(QuiesceRefusalReason::InvalidRequest),
                "ttl {ttl} must be refused"
            );
        }
        for ttl in [MIN_LEASE_TTL_MS, MAX_LEASE_TTL_MS] {
            assert_eq!(
                validate_acquire(
                    QUIESCENCE_PROTOCOL_VERSION,
                    "req-1",
                    "lease-1",
                    "sess-1",
                    &scope_all(),
                    ttl
                ),
                None,
                "ttl {ttl} is inside the window"
            );
        }
    }

    #[test]
    fn acquire_validation_rejects_a_path_separator_in_a_profile_name() {
        for hostile in ["../escape", "a/b", "a\\b", "", ".hidden", "n\u{0}l"] {
            let scope = QuiesceScope {
                include_default: false,
                profiles: QuiesceProfileSelector::Named {
                    names: vec![hostile.to_string()],
                },
            };
            assert_eq!(
                validate_acquire(
                    QUIESCENCE_PROTOCOL_VERSION,
                    "req-1",
                    "lease-1",
                    "sess-1",
                    &scope,
                    60_000
                ),
                Some(QuiesceRefusalReason::InvalidRequest),
                "profile name {hostile:?} must be refused"
            );
        }
    }

    #[test]
    fn acquire_validation_refuses_a_request_that_covers_nothing() {
        let scope = QuiesceScope {
            include_default: false,
            profiles: QuiesceProfileSelector::Named { names: Vec::new() },
        };
        // Empty explicit selection is a shape error; the zero-coverage branch
        // below it is what catches a host that turns off every source.
        assert_eq!(
            validate_acquire(
                QUIESCENCE_PROTOCOL_VERSION,
                "req-1",
                "lease-1",
                "sess-1",
                &scope,
                60_000
            ),
            Some(QuiesceRefusalReason::InvalidRequest)
        );
    }

    #[test]
    fn release_validation_requires_an_epoch_echo() {
        assert_eq!(
            validate_release(
                QUIESCENCE_PROTOCOL_VERSION,
                "req-1",
                "lease-1",
                "sess-1",
                ""
            ),
            Some(QuiesceRefusalReason::InvalidRequest)
        );
        assert_eq!(
            validate_release(
                QUIESCENCE_PROTOCOL_VERSION,
                "req-1",
                "lease-1",
                "sess-1",
                "sha256:abc"
            ),
            None
        );
    }

    #[test]
    fn scope_defaults_to_including_the_default_home() {
        let scope: QuiesceScope =
            serde_json::from_str(r#"{"profiles":{"select":"all"}}"#).expect("scope must parse");
        assert!(
            scope.include_default,
            "omitting include_default must not silently drop the default home"
        );
    }

    #[test]
    fn scope_rejects_an_unknown_field() {
        let parsed = serde_json::from_str::<QuiesceScope>(
            r#"{"profiles":{"select":"all"},"include_secrets":true}"#,
        );
        assert!(
            parsed.is_err(),
            "an unknown scope field must not be silently ignored"
        );
    }
}
