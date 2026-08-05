//! Protocol version negotiation at connect.
//!
//! Phase 24 Success Criterion 4.
//!
//! # An explicit refusal, never a silent downgrade
//!
//! When a client is below the server's floor, the connection is REFUSED and
//! the refusal names the version required. The alternative — accepting the
//! connection and quietly serving a reduced surface — produces a client that
//! works for a while and then fails somewhere far from the cause, on the first
//! feature the silent downgrade removed. The operator debugging that failure
//! has no reason to suspect a version.
//!
//! # Negotiating DOWN to a newer client is not a downgrade
//!
//! A client NEWER than the server is a different situation and is accepted, at
//! the server's version. That is not silent: the negotiated version is
//! returned, the client is told it did not get what it asked for, and it is
//! the client's business whether to proceed. The asymmetry is real — a server
//! cannot invent a protocol it does not implement, but a newer client can
//! choose to speak an older one.
//!
//! # Versions here are `major.minor` and comparison is NUMERIC
//!
//! String comparison is the trap: `"0.10" < "0.9"` lexicographically, so a
//! lexicographic floor check silently refuses every client from 0.10 onward.
//! [`Version::parse`] converts to integers and there is a test that reddens if
//! the comparison ever regresses to string ordering.

use serde::{Deserialize, Serialize};

use crate::protocol::ACP_PROTOCOL_VERSION;

/// A `major.minor` protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

impl Version {
    /// Parse `major.minor`. Rejects anything else — a version this code does
    /// not understand must not be compared, because every comparison against
    /// an unparsed version is a guess.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let (major, minor) = s.split_once('.')?;
        if major.is_empty() || minor.is_empty() {
            return None;
        }
        Some(Version {
            major: major.parse().ok()?,
            minor: minor.parse().ok()?,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The version this server implements — the SAME constant the `initialize`
/// handshake advertises, read from there rather than restated, so the two can
/// never disagree.
pub fn server_version() -> Version {
    Version::parse(ACP_PROTOCOL_VERSION).unwrap_or(Version { major: 0, minor: 1 })
}

/// The oldest client version this server will serve.
///
/// Equal to [`server_version`] today. It is a separate function rather than an
/// alias because the two answer different questions and will diverge the first
/// time the protocol gains a minor version while old clients are still
/// supported.
pub fn minimum_client_version() -> Version {
    Version { major: 0, minor: 1 }
}

/// A successful negotiation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Negotiated {
    /// What the client asked for.
    pub requested: Version,
    /// What both sides will actually speak.
    pub agreed: Version,
    /// `true` when the client asked for something newer than the server can
    /// serve and was met at the server's version. Surfaced so a client can log
    /// or refuse rather than discovering it by a missing field later.
    pub client_is_newer: bool,
}

/// Why a connection was refused at the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiateError {
    /// Older than the floor. Names the required version.
    TooOld {
        requested: Version,
        required: Version,
    },
    /// Not a `major.minor` string at all.
    Unparseable { requested: String },
}

impl std::fmt::Display for NegotiateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NegotiateError::TooOld {
                requested,
                required,
            } => write!(
                f,
                "client protocol {requested} is below this server's minimum; \
                 upgrade the client to at least {required}"
            ),
            NegotiateError::Unparseable { requested } => write!(
                f,
                "client protocol {requested:?} is not a major.minor version"
            ),
        }
    }
}

impl std::error::Error for NegotiateError {}

impl From<NegotiateError> for crate::error::AcpError {
    fn from(e: NegotiateError) -> Self {
        crate::error::AcpError::Protocol(e.to_string())
    }
}

/// Negotiate against a client's advertised protocol version.
pub fn negotiate(client: &str) -> Result<Negotiated, NegotiateError> {
    let Some(requested) = Version::parse(client) else {
        return Err(NegotiateError::Unparseable {
            requested: client.to_string(),
        });
    };
    let floor = minimum_client_version();
    if requested < floor {
        return Err(NegotiateError::TooOld {
            requested,
            required: floor,
        });
    }
    let server = server_version();
    Ok(Negotiated {
        requested,
        agreed: requested.min(server),
        client_is_newer: requested > server,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_below_the_floor_is_refused_by_name_not_downgraded() {
        let err = negotiate("0.0").expect_err("below the floor must refuse");
        let NegotiateError::TooOld { required, .. } = &err else {
            panic!("expected TooOld, got {err:?}");
        };
        assert_eq!(*required, minimum_client_version());
        assert!(
            err.to_string()
                .contains(&minimum_client_version().to_string()),
            "the refusal must NAME the version required, or the operator has \
             nothing to act on: {err}"
        );
    }

    #[test]
    fn a_client_at_the_floor_is_accepted_at_the_server_version() {
        let n = negotiate("0.1").expect("the floor itself is supported");
        assert_eq!(n.agreed, server_version());
        assert!(!n.client_is_newer);
    }

    #[test]
    fn a_newer_client_is_met_at_the_server_version_and_told_so() {
        // Not a silent downgrade: the client learns it did not get what it
        // asked for and decides for itself.
        let n = negotiate("9.4").expect("a newer client is servable");
        assert_eq!(n.requested, Version { major: 9, minor: 4 });
        assert_eq!(n.agreed, server_version());
        assert!(
            n.client_is_newer,
            "the client must be TOLD it was met lower, or it discovers it via \
             a missing field somewhere far from here"
        );
    }

    #[test]
    fn version_comparison_is_numeric_and_not_lexicographic() {
        // The trap: "0.10" < "0.9" as strings, so a lexicographic floor check
        // silently refuses every client from 0.10 onward — a whole-fleet
        // outage that arrives on a version bump nobody associates with it.
        assert!(
            "0.10" < "0.9",
            "the string ordering really is the wrong way round"
        );
        assert!(
            Version::parse("0.10").unwrap() > Version::parse("0.9").unwrap(),
            "numeric ordering must disagree with the string ordering above"
        );
        assert!(negotiate("0.10").is_ok());
        assert!(Version::parse("1.0").unwrap() > Version::parse("0.99").unwrap());
    }

    #[test]
    fn an_unparseable_version_is_refused_rather_than_compared() {
        // Every comparison against an unparsed version is a guess.
        for bad in ["", "abc", "1", "1.", ".1", "1.2.3", "v1.2", "-1.0"] {
            assert!(
                matches!(negotiate(bad), Err(NegotiateError::Unparseable { .. })),
                "{bad:?} must be refused as unparseable"
            );
        }
    }

    #[test]
    fn the_server_version_is_the_one_the_handshake_advertises() {
        // Restating the constant here instead of reading it is how the
        // handshake and the negotiator drift apart.
        assert_eq!(server_version().to_string(), ACP_PROTOCOL_VERSION);
    }
}
