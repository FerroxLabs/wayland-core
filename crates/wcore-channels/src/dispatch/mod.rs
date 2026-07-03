//! Inbound dispatch kernel — pure, fail-closed admission for channel
//! traffic.
//!
//! Given an [`IncomingMessage`], an [`InboundPolicy`], and a
//! [`DedupeCache`], [`evaluate`] decides whether the message becomes an
//! agent turn, is observed only, or is dropped — and if it dispatches,
//! which session key it routes to.
//!
//! The kernel is deliberately free of async, I/O, and any engine/agent
//! dependency: it is the security gate's logic core, unit-testable in
//! isolation. The orchestrating channel runtime supplies `now_ms` and the
//! cache.
//!
//! Pipeline (each step may short-circuit):
//! 1. **classify** — loop guard (self/bot) and mention gating. A `Drop`
//!    or `ObserveOnly` here returns immediately (no dedup needed).
//! 2. **dedup** — suppress platform re-deliveries of the same message id.
//! 3. **access** — the fail-closed allowlist gate.
//! 4. **route** — derive the session key for the admitted turn.

pub mod access;
pub mod admission;
pub mod dedupe;
pub mod rate_limit;
pub mod session_key;

pub use access::{
    AccessDecision, AckMode, ChannelToolPosture, DmPolicy, GroupPolicy, InboundPolicy,
    decide_access,
};
pub use admission::{TurnAdmission, classify};
pub use dedupe::{DedupeCache, DedupeKey};
pub use rate_limit::{
    AutoReplyRateLimiter, DEFAULT_AUTO_REPLY_WINDOW, DEFAULT_CONVERSATION_CAP,
    DEFAULT_MAX_AUTO_REPLIES,
};
pub use session_key::build_session_key;

use crate::event::IncomingMessage;

/// Result of evaluating one inbound message through the dispatch kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchOutcome {
    /// Final admission decision.
    pub admission: TurnAdmission,
    /// The routed session key — `Some` only when `admission` is
    /// [`TurnAdmission::Dispatch`].
    pub session_key: Option<String>,
    /// Short, content-free deny reason — `Some` only when the access gate
    /// rejected the message.
    pub deny_reason: Option<String>,
}

/// Evaluate one inbound message: classify, dedup, gate, and route.
///
/// See the module docs for the pipeline. `now_ms` is caller-supplied
/// monotonic millis (for deterministic dedup); `dedupe` is the shared
/// per-channel cache and is mutated on every non-short-circuited call.
pub fn evaluate(
    channel_name: &str,
    msg: &IncomingMessage,
    policy: &InboundPolicy,
    dedupe: &mut DedupeCache,
    now_ms: u64,
) -> DispatchOutcome {
    // 1. Classification — loop guard + mention gating.
    match classify(msg, policy) {
        TurnAdmission::Dispatch => {}
        other => {
            // Drop (self/bot) or ObserveOnly — return as-is, no dedup.
            return DispatchOutcome {
                admission: other,
                session_key: None,
                deny_reason: None,
            };
        }
    }

    // 2. Dedup — suppress platform re-deliveries of the same message.
    let key = DedupeKey {
        platform: msg
            .platform
            .clone()
            .unwrap_or_else(|| channel_name.to_string()),
        account_id: msg.account_id.clone().unwrap_or_default(),
        message_id: msg.id.clone(),
    };
    if !dedupe.check(key, now_ms) {
        // Live duplicate — silently drop.
        return DispatchOutcome {
            admission: TurnAdmission::Drop {
                record_history: false,
            },
            session_key: None,
            deny_reason: None,
        };
    }

    // 3. Access gate — fail-closed allowlist enforcement.
    if let AccessDecision::Deny { reason } = decide_access(msg, policy) {
        return DispatchOutcome {
            admission: TurnAdmission::Drop {
                record_history: true,
            },
            session_key: None,
            deny_reason: Some(reason),
        };
    }

    // 4. Route — derive the session key for the admitted turn.
    let session_key = build_session_key(channel_name, msg, policy);
    DispatchOutcome {
        admission: TurnAdmission::Dispatch,
        session_key: Some(session_key),
        deny_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ChatType;

    fn cache() -> DedupeCache {
        DedupeCache::new(60_000, 1024)
    }

    fn dm(sender: &str, id: &str) -> IncomingMessage {
        let mut m = IncomingMessage::new(id, "conv1", "Alice", "hi", 0);
        m.sender_id = sender.into();
        m.chat_type = ChatType::Direct;
        m
    }

    #[test]
    fn disallowed_dm_drops_with_deny_reason() {
        // Default policy: dm Allowlist with EMPTY allowlist -> fail-closed.
        let policy = InboundPolicy::default();
        let mut c = cache();
        let out = evaluate("slack", &dm("u1", "m1"), &policy, &mut c, 0);
        assert_eq!(
            out.admission,
            TurnAdmission::Drop {
                record_history: true
            }
        );
        assert!(out.session_key.is_none());
        assert!(
            out.deny_reason.is_some(),
            "fail-closed deny carries a reason"
        );
    }

    #[test]
    fn allowed_dm_dispatches_with_session_key() {
        let policy = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let mut c = cache();
        let out = evaluate("slack", &dm("u1", "m1"), &policy, &mut c, 0);
        assert_eq!(out.admission, TurnAdmission::Dispatch);
        assert_eq!(
            out.session_key.as_deref(),
            Some("agent:main:slack:dm:conv1")
        );
        assert!(out.deny_reason.is_none());
    }

    #[test]
    fn duplicate_message_drops_after_first_dispatch() {
        let policy = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let mut c = cache();
        // First sight dispatches.
        let first = evaluate("slack", &dm("u1", "m1"), &policy, &mut c, 0);
        assert_eq!(first.admission, TurnAdmission::Dispatch);
        // Re-delivery of the same id within ttl drops silently.
        let dup = evaluate("slack", &dm("u1", "m1"), &policy, &mut c, 10);
        assert_eq!(
            dup.admission,
            TurnAdmission::Drop {
                record_history: false
            }
        );
        assert!(dup.session_key.is_none());
        assert!(dup.deny_reason.is_none());
    }

    #[test]
    fn unmentioned_group_is_observe_only_and_skips_dedup() {
        let policy = InboundPolicy {
            group: GroupPolicy::Open,
            require_mention: true,
            ..Default::default()
        };
        let mut c = cache();
        let mut m = IncomingMessage::new("m1", "g1", "Bob", "hi", 0);
        m.sender_id = "u1".into();
        m.chat_type = ChatType::Group;
        m.was_mentioned = false;
        let out = evaluate("slack", &m, &policy, &mut c, 0);
        assert_eq!(out.admission, TurnAdmission::ObserveOnly);
        assert!(out.session_key.is_none());
        // ObserveOnly short-circuits before dedup, so the cache stays empty.
        assert!(c.is_empty(), "observe-only must not consume a dedup slot");
    }

    #[test]
    fn self_message_drops_without_history() {
        let policy = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let mut c = cache();
        let mut m = dm("u1", "m1");
        m.is_self = true;
        let out = evaluate("slack", &m, &policy, &mut c, 0);
        assert_eq!(
            out.admission,
            TurnAdmission::Drop {
                record_history: false
            }
        );
        assert!(out.deny_reason.is_none());
        // Loop-guard drop short-circuits before dedup too.
        assert!(c.is_empty());
    }

    #[test]
    fn dedup_key_uses_platform_field_over_channel_name() {
        let policy = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let mut c = cache();
        let mut m = dm("u1", "m1");
        m.platform = Some("telegram".into());
        m.account_id = Some("bot7".into());
        let out = evaluate("chan-a", &m, &policy, &mut c, 0);
        assert_eq!(out.admission, TurnAdmission::Dispatch);
        // The recorded key should be (telegram, bot7, m1), not the channel.
        assert!(c.peek(&DedupeKey::new("telegram", "bot7", "m1"), 1));
        assert!(!c.peek(&DedupeKey::new("chan-a", "", "m1"), 1));
    }

    #[test]
    fn allowed_group_dispatches_with_per_user_key() {
        let policy = InboundPolicy {
            group: GroupPolicy::Allowlist,
            group_allowlist: vec!["g1".into()],
            sender_allowlist: vec!["u1".into()],
            require_mention: false,
            group_sessions_per_user: true,
            ..Default::default()
        };
        let mut c = cache();
        let mut m = IncomingMessage::new("m1", "g1", "Bob", "hi", 0);
        m.sender_id = "u1".into();
        m.chat_type = ChatType::Group;
        let out = evaluate("slack", &m, &policy, &mut c, 0);
        assert_eq!(out.admission, TurnAdmission::Dispatch);
        assert_eq!(out.session_key.as_deref(), Some("agent:main:slack:g1:u1"));
    }
}
