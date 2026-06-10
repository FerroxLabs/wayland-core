//! Pure session-key derivation.
//!
//! The session key determines which agent session an inbound message
//! routes to. Key composition is policy-driven so operators can choose
//! between shared and isolated sessions per user / per thread.
//!
//! Defaults: groups get **isolated per-user** sessions
//! (`group_sessions_per_user = true`), and threads are **shared** with
//! their parent chat (`thread_sessions_per_user = false`) — a thread only
//! splits into its own session when `thread_sessions_per_user` is set.

use crate::event::{ChatType, IncomingMessage};

use super::access::InboundPolicy;

/// Derive the session key for `msg` within `channel_name` under `policy`.
///
/// DM (`Direct`):
/// `agent:main:<channel>:dm:<conversation_id>`, plus `:<thread_id>` when
/// the message carries a thread id (DMs always split per thread).
///
/// Group/Channel:
/// `agent:main:<channel>:<conversation_id>`, plus `:<sender_id>` when
/// `group_sessions_per_user`, plus `:<thread_id>` when the message has a
/// thread id AND `thread_sessions_per_user`.
pub fn build_session_key(
    channel_name: &str,
    msg: &IncomingMessage,
    policy: &InboundPolicy,
) -> String {
    match msg.chat_type {
        ChatType::Direct => {
            let mut key = format!("agent:main:{channel_name}:dm:{}", msg.conversation_id);
            if let Some(thread) = &msg.thread_id {
                key.push(':');
                key.push_str(thread);
            }
            key
        }
        ChatType::Group | ChatType::Channel => {
            let mut key = format!("agent:main:{channel_name}:{}", msg.conversation_id);
            if policy.group_sessions_per_user {
                key.push(':');
                key.push_str(&msg.sender_id);
            }
            if policy.thread_sessions_per_user
                && let Some(thread) = &msg.thread_id
            {
                key.push(':');
                key.push_str(thread);
            }
            key
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> IncomingMessage {
        let mut m = IncomingMessage::new("id1", "conv1", "Alice", "hi", 0);
        m.sender_id = "u1".into();
        m
    }

    #[test]
    fn dm_key_without_thread() {
        let mut m = base();
        m.chat_type = ChatType::Direct;
        let key = build_session_key("slack", &m, &InboundPolicy::default());
        assert_eq!(key, "agent:main:slack:dm:conv1");
    }

    #[test]
    fn dm_key_with_thread() {
        let mut m = base();
        m.chat_type = ChatType::Direct;
        m.thread_id = Some("t9".into());
        let key = build_session_key("slack", &m, &InboundPolicy::default());
        assert_eq!(key, "agent:main:slack:dm:conv1:t9");
    }

    #[test]
    fn group_isolated_per_user() {
        let p = InboundPolicy {
            group_sessions_per_user: true,
            ..Default::default()
        };
        let mut m = base();
        m.chat_type = ChatType::Group;
        let key = build_session_key("slack", &m, &p);
        assert_eq!(key, "agent:main:slack:conv1:u1");
    }

    #[test]
    fn group_shared_when_per_user_off() {
        let p = InboundPolicy {
            group_sessions_per_user: false,
            ..Default::default()
        };
        let mut m = base();
        m.chat_type = ChatType::Group;
        let key = build_session_key("slack", &m, &p);
        assert_eq!(key, "agent:main:slack:conv1");
    }

    #[test]
    fn group_thread_split_when_enabled() {
        let p = InboundPolicy {
            group_sessions_per_user: true,
            thread_sessions_per_user: true,
            ..Default::default()
        };
        let mut m = base();
        m.chat_type = ChatType::Group;
        m.thread_id = Some("t9".into());
        let key = build_session_key("slack", &m, &p);
        assert_eq!(key, "agent:main:slack:conv1:u1:t9");
    }

    #[test]
    fn group_thread_shared_when_disabled() {
        let p = InboundPolicy {
            group_sessions_per_user: true,
            thread_sessions_per_user: false,
            ..Default::default()
        };
        let mut m = base();
        m.chat_type = ChatType::Group;
        m.thread_id = Some("t9".into());
        let key = build_session_key("slack", &m, &p);
        // Thread id ignored -> shared with the parent chat session.
        assert_eq!(key, "agent:main:slack:conv1:u1");
    }

    #[test]
    fn group_thread_split_ignored_without_thread_id() {
        // thread_sessions_per_user on, but no thread id present.
        let p = InboundPolicy {
            group_sessions_per_user: false,
            thread_sessions_per_user: true,
            ..Default::default()
        };
        let mut m = base();
        m.chat_type = ChatType::Group;
        m.thread_id = None;
        let key = build_session_key("slack", &m, &p);
        assert_eq!(key, "agent:main:slack:conv1");
    }

    #[test]
    fn channel_uses_group_composition() {
        let p = InboundPolicy {
            group_sessions_per_user: true,
            ..Default::default()
        };
        let mut m = base();
        m.chat_type = ChatType::Channel;
        let key = build_session_key("disc", &m, &p);
        assert_eq!(key, "agent:main:disc:conv1:u1");
    }
}
