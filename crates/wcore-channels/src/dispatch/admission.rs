//! Turn admission — classifies whether an inbound message is
//! turn-worthy, observe-only, or should be dropped.
//!
//! Classification is independent of access control: `classify` decides
//! *worthiness* (loop guard, mention-gating), while
//! [`crate::dispatch::access::decide_access`] decides *permission*. The
//! orchestrator combines the two.

use crate::event::{ChatType, IncomingMessage};

use super::access::InboundPolicy;

/// What the dispatch kernel should do with an inbound message after
/// classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnAdmission {
    /// A candidate turn — the access gate still decides whether it runs.
    Dispatch,
    /// Record to history but don't start an agent turn (e.g. an
    /// un-addressed group message under `require_mention`).
    ObserveOnly,
    /// Already handled by an earlier stage; take no further action.
    Handled,
    /// Drop the message. `record_history` distinguishes a silent loop-guard
    /// drop (`false`) from a drop that should still be journaled (`true`).
    Drop { record_history: bool },
}

/// Classify a message for turn-worthiness.
///
/// Order:
/// 1. Self/bot authored -> `Drop { record_history: false }` (loop guard).
/// 2. Group/channel + `require_mention` + not addressed -> `ObserveOnly`.
/// 3. Otherwise -> `Dispatch` (a candidate; access decided separately).
///
/// This does NOT enforce access control.
pub fn classify(msg: &IncomingMessage, policy: &InboundPolicy) -> TurnAdmission {
    // Loop guard: never react to our own / other bots' messages.
    if msg.is_self || msg.is_bot {
        return TurnAdmission::Drop {
            record_history: false,
        };
    }

    // Group/channel mention gating.
    let is_group = msg.chat_type != ChatType::Direct;
    if is_group && policy.require_mention && !msg.was_mentioned {
        return TurnAdmission::ObserveOnly;
    }

    TurnAdmission::Dispatch
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg() -> IncomingMessage {
        IncomingMessage::new("id1", "conv1", "Alice", "hi", 0)
    }

    #[test]
    fn self_message_dropped_without_history() {
        let mut m = msg();
        m.is_self = true;
        assert_eq!(
            classify(&m, &InboundPolicy::default()),
            TurnAdmission::Drop {
                record_history: false
            }
        );
    }

    #[test]
    fn bot_message_dropped_without_history() {
        let mut m = msg();
        m.is_bot = true;
        assert_eq!(
            classify(&m, &InboundPolicy::default()),
            TurnAdmission::Drop {
                record_history: false
            }
        );
    }

    #[test]
    fn unmentioned_group_is_observe_only() {
        let p = InboundPolicy {
            require_mention: true,
            ..Default::default()
        };
        let mut m = msg();
        m.chat_type = ChatType::Group;
        m.was_mentioned = false;
        assert_eq!(classify(&m, &p), TurnAdmission::ObserveOnly);
    }

    #[test]
    fn mentioned_group_dispatches() {
        let p = InboundPolicy {
            require_mention: true,
            ..Default::default()
        };
        let mut m = msg();
        m.chat_type = ChatType::Group;
        m.was_mentioned = true;
        assert_eq!(classify(&m, &p), TurnAdmission::Dispatch);
    }

    #[test]
    fn group_without_require_mention_dispatches() {
        let p = InboundPolicy {
            require_mention: false,
            ..Default::default()
        };
        let mut m = msg();
        m.chat_type = ChatType::Group;
        m.was_mentioned = false;
        assert_eq!(classify(&m, &p), TurnAdmission::Dispatch);
    }

    #[test]
    fn channel_unmentioned_is_observe_only() {
        let p = InboundPolicy {
            require_mention: true,
            ..Default::default()
        };
        let mut m = msg();
        m.chat_type = ChatType::Channel;
        m.was_mentioned = false;
        assert_eq!(classify(&m, &p), TurnAdmission::ObserveOnly);
    }

    #[test]
    fn dm_dispatches_regardless_of_mention() {
        let p = InboundPolicy {
            require_mention: true,
            ..Default::default()
        };
        let mut m = msg();
        m.chat_type = ChatType::Direct;
        m.was_mentioned = false;
        assert_eq!(classify(&m, &p), TurnAdmission::Dispatch);
    }

    #[test]
    fn self_flag_beats_mention_gating() {
        // A self message in a group is dropped, not observed.
        let p = InboundPolicy {
            require_mention: true,
            ..Default::default()
        };
        let mut m = msg();
        m.chat_type = ChatType::Group;
        m.is_self = true;
        assert_eq!(
            classify(&m, &p),
            TurnAdmission::Drop {
                record_history: false
            }
        );
    }
}
