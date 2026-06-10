//! Fail-closed inbound access policy — THE security gate.
//!
//! `decide_access` is the single chokepoint that decides whether an
//! inbound message is permitted to reach the agent. Its posture is
//! deliberately fail-closed: an unconfigured channel denies everything
//! until the operator adds explicit allowlist entries (see
//! [`InboundPolicy`]'s `Default`).
//!
//! This module is pure config + logic — no I/O, no async. The
//! orchestrator in [`crate::dispatch`] combines it with classification,
//! dedup, and session-key derivation.

use serde::{Deserialize, Serialize};

use crate::event::{ChatType, IncomingMessage};

/// Policy governing who may DM the bot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// Anyone may DM the bot.
    Open,
    /// Only `sender_id`s in `dm_allowlist` may DM the bot.
    Allowlist,
    /// Pairing handshake required (deferred to a later phase — currently
    /// fail-closed: every pairing DM is denied).
    Pairing,
    /// DMs are rejected entirely.
    Disabled,
}

/// Policy governing whether group/channel messages are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Any group/channel message is accepted (still subject to
    /// mention-gating, which is enforced in admission, not here).
    Open,
    /// Only allowlisted group chats AND allowlisted senders are accepted.
    Allowlist,
    /// Group/channel messages are rejected entirely.
    Disabled,
}

/// Inbound access + session-shaping policy for one channel.
///
/// **Fail-closed by default.** The [`Default`] impl denies all inbound
/// until the operator opts in: `dm: Allowlist` with an EMPTY allowlist
/// (so no one is permitted), `group: Disabled`, and `require_mention:
/// true`. An unconfigured channel therefore rejects every message. To
/// open DMs to everyone, set `dm_allowlist = ["*"]`; to allow a specific
/// person, add their stable `sender_id`.
///
/// Allowlist semantics: a list permits an id iff it contains the literal
/// `"*"` (wildcard) OR the exact id. An EMPTY list permits NOTHING.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InboundPolicy {
    /// Who may DM the bot.
    #[serde(default = "default_dm_policy")]
    pub dm: DmPolicy,
    /// Whether group/channel messages are accepted.
    #[serde(default = "default_group_policy")]
    pub group: GroupPolicy,
    /// In groups, only act when the bot is addressed (mention/reply/
    /// quote/thread). Enforced in admission classification.
    #[serde(default = "default_require_mention")]
    pub require_mention: bool,
    /// Permitted `sender_id`s for DMs. `"*"` = any. Empty = none.
    #[serde(default)]
    pub dm_allowlist: Vec<String>,
    /// Permitted group `conversation_id`s. `"*"` = any. Empty = none.
    #[serde(default)]
    pub group_allowlist: Vec<String>,
    /// Permitted `sender_id`s within groups. `"*"` = any. Empty = none.
    #[serde(default)]
    pub sender_allowlist: Vec<String>,
    /// Give each user their own isolated session within a group. See
    /// [`crate::dispatch::build_session_key`].
    #[serde(default = "default_true")]
    pub group_sessions_per_user: bool,
    /// Split sessions per thread within a group. See
    /// [`crate::dispatch::build_session_key`].
    #[serde(default)]
    pub thread_sessions_per_user: bool,
}

fn default_dm_policy() -> DmPolicy {
    DmPolicy::Allowlist
}
fn default_group_policy() -> GroupPolicy {
    GroupPolicy::Disabled
}
fn default_require_mention() -> bool {
    true
}
fn default_true() -> bool {
    true
}

impl Default for InboundPolicy {
    /// Fail-closed posture — denies all inbound until configured.
    fn default() -> Self {
        Self {
            dm: DmPolicy::Allowlist,
            group: GroupPolicy::Disabled,
            require_mention: true,
            dm_allowlist: Vec::new(),
            group_allowlist: Vec::new(),
            sender_allowlist: Vec::new(),
            group_sessions_per_user: true,
            thread_sessions_per_user: false,
        }
    }
}

/// Outcome of the access gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessDecision {
    /// Message is permitted to proceed.
    Allow,
    /// Message is rejected. `reason` is a short, non-PII-leaking tag for
    /// logging — it never embeds sender ids or message content.
    Deny { reason: String },
}

/// True iff `list` permits `id`: contains the `"*"` wildcard, or contains
/// `id` exactly. An empty list permits nothing (fail-closed).
fn permits(list: &[String], id: &str) -> bool {
    list.iter().any(|e| e == "*" || e == id)
}

/// The fail-closed access gate. Decides whether `msg` is permitted under
/// `policy`, without considering mention-gating (that lives in
/// admission). Reasons are short, content-free tags.
pub fn decide_access(msg: &IncomingMessage, policy: &InboundPolicy) -> AccessDecision {
    match msg.chat_type {
        ChatType::Direct => match policy.dm {
            DmPolicy::Disabled => AccessDecision::Deny {
                reason: "dms disabled".into(),
            },
            DmPolicy::Open => AccessDecision::Allow,
            DmPolicy::Allowlist => {
                if permits(&policy.dm_allowlist, &msg.sender_id) {
                    AccessDecision::Allow
                } else {
                    AccessDecision::Deny {
                        reason: "sender not in dm allowlist".into(),
                    }
                }
            }
            DmPolicy::Pairing => AccessDecision::Deny {
                reason: "pairing not yet implemented".into(),
            },
        },
        ChatType::Group | ChatType::Channel => match policy.group {
            GroupPolicy::Disabled => AccessDecision::Deny {
                reason: "groups disabled".into(),
            },
            GroupPolicy::Open => AccessDecision::Allow,
            GroupPolicy::Allowlist => {
                if !permits(&policy.group_allowlist, &msg.conversation_id) {
                    AccessDecision::Deny {
                        reason: "group not allowlisted".into(),
                    }
                } else if !permits(&policy.sender_allowlist, &msg.sender_id) {
                    AccessDecision::Deny {
                        reason: "sender not in group allowlist".into(),
                    }
                } else {
                    AccessDecision::Allow
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm_from(sender: &str) -> IncomingMessage {
        let mut m = IncomingMessage::new("id1", "conv1", "Alice", "hi", 0);
        m.sender_id = sender.into();
        m.chat_type = ChatType::Direct;
        m
    }

    fn group_from(conv: &str, sender: &str) -> IncomingMessage {
        let mut m = IncomingMessage::new("id1", conv, "Alice", "hi", 0);
        m.sender_id = sender.into();
        m.chat_type = ChatType::Group;
        m
    }

    #[test]
    fn default_policy_is_fail_closed() {
        let p = InboundPolicy::default();
        assert_eq!(p.dm, DmPolicy::Allowlist);
        assert_eq!(p.group, GroupPolicy::Disabled);
        assert!(p.require_mention);
        assert!(p.dm_allowlist.is_empty());
        assert!(p.group_allowlist.is_empty());
        assert!(p.sender_allowlist.is_empty());
        assert!(p.group_sessions_per_user);
        assert!(!p.thread_sessions_per_user);
        // A DM under the default policy is denied (empty allowlist).
        assert!(matches!(
            decide_access(&dm_from("u1"), &p),
            AccessDecision::Deny { .. }
        ));
        // A group message under the default policy is denied (disabled).
        assert!(matches!(
            decide_access(&group_from("g1", "u1"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    // ---- DM ----

    #[test]
    fn dm_empty_allowlist_denies() {
        let p = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec![],
            ..Default::default()
        };
        assert!(matches!(
            decide_access(&dm_from("u1"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    #[test]
    fn dm_wildcard_allows_anyone() {
        let p = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec!["*".into()],
            ..Default::default()
        };
        assert_eq!(decide_access(&dm_from("anyone"), &p), AccessDecision::Allow);
    }

    #[test]
    fn dm_exact_id_allows_only_that_id() {
        let p = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec!["u1".into()],
            ..Default::default()
        };
        assert_eq!(decide_access(&dm_from("u1"), &p), AccessDecision::Allow);
        assert!(matches!(
            decide_access(&dm_from("u2"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    #[test]
    fn dm_open_allows_all() {
        let p = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        assert_eq!(decide_access(&dm_from("u1"), &p), AccessDecision::Allow);
    }

    #[test]
    fn dm_disabled_denies_even_with_wildcard() {
        let p = InboundPolicy {
            dm: DmPolicy::Disabled,
            dm_allowlist: vec!["*".into()],
            ..Default::default()
        };
        assert!(matches!(
            decide_access(&dm_from("u1"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    #[test]
    fn dm_pairing_denies_with_specific_reason() {
        let p = InboundPolicy {
            dm: DmPolicy::Pairing,
            dm_allowlist: vec!["*".into()],
            ..Default::default()
        };
        match decide_access(&dm_from("u1"), &p) {
            AccessDecision::Deny { reason } => assert!(reason.contains("pairing")),
            AccessDecision::Allow => panic!("pairing must deny until implemented"),
        }
    }

    // ---- Group ----

    #[test]
    fn group_disabled_denies_even_with_wildcards() {
        let p = InboundPolicy {
            group: GroupPolicy::Disabled,
            group_allowlist: vec!["*".into()],
            sender_allowlist: vec!["*".into()],
            ..Default::default()
        };
        assert!(matches!(
            decide_access(&group_from("g1", "u1"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    #[test]
    fn group_open_allows() {
        let p = InboundPolicy {
            group: GroupPolicy::Open,
            ..Default::default()
        };
        assert_eq!(
            decide_access(&group_from("g1", "u1"), &p),
            AccessDecision::Allow
        );
    }

    #[test]
    fn group_allowlist_requires_both_group_and_sender() {
        let p = InboundPolicy {
            group: GroupPolicy::Allowlist,
            group_allowlist: vec!["g1".into()],
            sender_allowlist: vec!["u1".into()],
            ..Default::default()
        };
        // Both match -> allow.
        assert_eq!(
            decide_access(&group_from("g1", "u1"), &p),
            AccessDecision::Allow
        );
        // Group not allowlisted -> deny.
        match decide_access(&group_from("g2", "u1"), &p) {
            AccessDecision::Deny { reason } => assert!(reason.contains("group")),
            AccessDecision::Allow => panic!("non-allowlisted group must deny"),
        }
        // Sender not allowlisted -> deny.
        match decide_access(&group_from("g1", "u2"), &p) {
            AccessDecision::Deny { reason } => assert!(reason.contains("sender")),
            AccessDecision::Allow => panic!("non-allowlisted sender must deny"),
        }
    }

    #[test]
    fn group_allowlist_empty_lists_deny() {
        let p = InboundPolicy {
            group: GroupPolicy::Allowlist,
            group_allowlist: vec![],
            sender_allowlist: vec![],
            ..Default::default()
        };
        assert!(matches!(
            decide_access(&group_from("g1", "u1"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    #[test]
    fn channel_chat_type_uses_group_policy() {
        let p = InboundPolicy {
            group: GroupPolicy::Open,
            ..Default::default()
        };
        let mut m = group_from("c1", "u1");
        m.chat_type = ChatType::Channel;
        assert_eq!(decide_access(&m, &p), AccessDecision::Allow);
    }

    #[test]
    fn permits_helper_semantics() {
        assert!(!permits(&[], "x"), "empty list permits nothing");
        assert!(permits(&["*".into()], "x"), "wildcard permits any");
        assert!(permits(&["x".into()], "x"), "exact match permits");
        assert!(!permits(&["y".into()], "x"), "non-match denies");
    }
}
