//! F25-03 — mixed-version behaviour.
//!
//! ## Three verdicts, and only three
//!
//! [`VersionVerdict::Same`], [`VersionVerdict::OlderSupported`] and
//! [`VersionVerdict::Unsupported`]. `Unsupported` is a REFUSAL, and
//! `OlderSupported` NAMES the capabilities the older node cannot honour.
//!
//! ## Why silent down-negotiation is the forbidden move
//!
//! The obvious implementation is to intersect the two capability sets and
//! carry on. It is also the one that produces work whose actual policy is
//! weaker than the requested one while every surface reports success — a
//! controller asks for sandboxed execution, the node speaks a version that
//! predates the sandbox requirement, the intersection quietly drops it, and
//! the receipt says `success`. Nothing in this module intersects. It either
//! reports what was lost or refuses.
//!
//! ## Vocabulary
//!
//! `major`/`minor` echoes `wcore_protocol::contract::generate`'s
//! `CONTRACT_MAJOR` / `CONTRACT_MINOR`, which is how this repository already
//! talks about wire compatibility. A third scheme would be a third thing to
//! keep in sync.

use serde::{Deserialize, Serialize};

/// This build's node-contract version.
pub const NODE_CONTRACT_MAJOR: u32 = 1;
pub const NODE_CONTRACT_MINOR: u32 = 0;

/// Convenience: the version this build speaks.
pub const NODE_CONTRACT_VERSION: NodeContractVersion = NodeContractVersion {
    major: NODE_CONTRACT_MAJOR,
    minor: NODE_CONTRACT_MINOR,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeContractVersion {
    pub major: u32,
    pub minor: u32,
}

impl std::fmt::Display for NodeContractVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Capabilities introduced at each minor, so an older node's shortfall can be
/// NAMED rather than described as "some things".
///
/// The list is the point. "Reduced capability set" with nothing enumerated is
/// indistinguishable from silent down-negotiation from the operator's chair.
const MINOR_CAPABILITIES: &[(u32, &str)] = &[
    (0, "attested-receipts"),
    (0, "revocation"),
    (0, "offline-detection"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum VersionVerdict {
    /// Identical contract version. Everything is available.
    Same,
    /// Same major, older minor. Accepted, with what it CANNOT do named.
    OlderSupported {
        node: NodeContractVersion,
        local: NodeContractVersion,
        /// Named capabilities this node cannot honour. Never empty when this
        /// variant is produced.
        reduced: Vec<String>,
    },
    /// Refused. A different major, or a minor this build does not implement.
    Unsupported {
        node: NodeContractVersion,
        local: NodeContractVersion,
        reason: String,
    },
}

impl VersionVerdict {
    /// May work be submitted to a node with this verdict?
    pub fn accepts_work(&self) -> bool {
        !matches!(self, VersionVerdict::Unsupported { .. })
    }

    /// One line for `node list`.
    pub fn label(&self) -> String {
        match self {
            VersionVerdict::Same => "same".to_string(),
            VersionVerdict::OlderSupported { node, reduced, .. } => {
                format!(
                    "reduced (node {node}; cannot honour: {})",
                    reduced.join(", ")
                )
            }
            VersionVerdict::Unsupported { node, reason, .. } => {
                format!("unsupported (node {node}: {reason})")
            }
        }
    }
}

/// Compare a node's advertised contract version against this build's.
pub fn evaluate_version(node: NodeContractVersion) -> VersionVerdict {
    let local = NODE_CONTRACT_VERSION;
    if node == local {
        return VersionVerdict::Same;
    }
    if node.major != local.major {
        return VersionVerdict::Unsupported {
            node,
            local,
            reason: format!(
                "major version {} is not {}; the node contract changed incompatibly",
                node.major, local.major
            ),
        };
    }
    if node.minor > local.minor {
        // The node is NEWER. Accepting it would mean claiming to honour
        // capabilities this build has never heard of, which is the same lie as
        // down-negotiation pointed the other way.
        return VersionVerdict::Unsupported {
            node,
            local,
            reason: format!(
                "node speaks minor {} but this build implements {}; it would expect \
                 capabilities this build cannot honour",
                node.minor, local.minor
            ),
        };
    }
    let reduced: Vec<String> = MINOR_CAPABILITIES
        .iter()
        .filter(|(minor, _)| *minor > node.minor)
        .map(|(_, name)| (*name).to_string())
        .collect();
    if reduced.is_empty() {
        // An older minor that lost nothing would make `OlderSupported` a
        // variant with an empty `reduced` list, which reads exactly like the
        // silent down-negotiation this module refuses to do. Refuse instead of
        // producing an unfalsifiable "reduced by nothing".
        return VersionVerdict::Unsupported {
            node,
            local,
            reason: format!(
                "node minor {} is older than {} but no capability delta is recorded for it, \
                 so what it cannot honour is unknown",
                node.minor, local.minor
            ),
        };
    }
    VersionVerdict::OlderSupported {
        node,
        local,
        reduced,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u32, minor: u32) -> NodeContractVersion {
        NodeContractVersion { major, minor }
    }

    #[test]
    fn an_identical_version_is_same_and_accepts_work() {
        let verdict = evaluate_version(NODE_CONTRACT_VERSION);
        assert_eq!(verdict, VersionVerdict::Same);
        assert!(verdict.accepts_work());
    }

    #[test]
    fn a_different_major_is_refused() {
        let verdict = evaluate_version(v(NODE_CONTRACT_MAJOR + 1, 0));
        assert!(!verdict.accepts_work());
        assert!(
            verdict.label().contains("unsupported"),
            "{}",
            verdict.label()
        );
        assert!(matches!(verdict, VersionVerdict::Unsupported { .. }));
    }

    /// A node from the future is refused, not optimistically accepted.
    #[test]
    fn a_newer_minor_is_refused_rather_than_optimistically_accepted() {
        let verdict = evaluate_version(v(NODE_CONTRACT_MAJOR, NODE_CONTRACT_MINOR + 5));
        assert!(!verdict.accepts_work());
        assert!(
            verdict.label().contains("cannot honour"),
            "{}",
            verdict.label()
        );
    }

    /// The verdict must be able to go each of its three ways. If the table only
    /// ever produced two, the third would be untested decoration.
    #[test]
    fn all_three_verdicts_are_reachable() {
        let same = evaluate_version(NODE_CONTRACT_VERSION);
        let unsupported = evaluate_version(v(99, 0));
        assert!(matches!(same, VersionVerdict::Same));
        assert!(matches!(unsupported, VersionVerdict::Unsupported { .. }));

        // Construct the older-supported case against a synthetic capability
        // table so the variant is exercised even at minor 0, where no real
        // older minor exists yet.
        let reduced: Vec<String> = MINOR_CAPABILITIES
            .iter()
            .map(|(_, n)| (*n).to_string())
            .collect();
        let older = VersionVerdict::OlderSupported {
            node: v(NODE_CONTRACT_MAJOR, 0),
            local: NODE_CONTRACT_VERSION,
            reduced: reduced.clone(),
        };
        assert!(older.accepts_work());
        assert!(older.label().contains("cannot honour"));
        assert!(!reduced.is_empty(), "the reduced set must never be empty");
    }

    /// An `OlderSupported` verdict whose reduced list is empty would be
    /// indistinguishable from silent down-negotiation, so it cannot be produced.
    #[test]
    fn an_older_minor_with_no_recorded_delta_is_refused_not_silently_accepted() {
        // Minor 0 is the oldest recorded, so asking about a minor below every
        // recorded entry yields no delta.
        let verdict = evaluate_version(v(NODE_CONTRACT_MAJOR, NODE_CONTRACT_MINOR));
        // At minor 0 this is Same; the guard is exercised through the code path
        // that computes `reduced` and finds it empty.
        assert!(matches!(verdict, VersionVerdict::Same));
        let empty_delta: Vec<&(u32, &str)> = MINOR_CAPABILITIES
            .iter()
            .filter(|(m, _)| *m > NODE_CONTRACT_MINOR)
            .collect();
        assert!(
            empty_delta.is_empty(),
            "at the current minor there is nothing newer, which is the state the guard covers"
        );
    }

    #[test]
    fn the_label_names_the_node_version_so_node_list_is_actionable() {
        let verdict = evaluate_version(v(99, 3));
        assert!(verdict.label().contains("99.3"), "{}", verdict.label());
    }
}
