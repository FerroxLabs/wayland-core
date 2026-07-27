//! Thread and group binding, and the profile/agent routing it feeds.
//!
//! # What this answers
//!
//! An inbound message arrives addressed to a platform conversation. Two
//! separate questions follow: which stable internal BINDING is that
//! conversation, and which profile and agent does that binding ROUTE to. They
//! are separate because a conversation's identity outlives whichever agent is
//! currently answering it.
//!
//! # The default is declared, never inherited (threat T-24-03-03)
//!
//! [`BindingTable::new`] REQUIRES a default target. There is no constructor
//! that leaves it unset, so there is no code path where an unbound conversation
//! falls through to "whatever was routed last". An inherited default is a
//! spoofing primitive: a sender who can open a new conversation gets routed to
//! the identity of whoever was served immediately before them.
//!
//! # Keys are escaped, and that is load-bearing
//!
//! Binding keys compose several attacker-influenced strings — a conversation id
//! and a thread id both come from the platform and, on several platforms, from
//! a name a sender chooses. Joining them with a raw separator lets one field
//! impersonate two: a conversation literally named `general/t42` would compose
//! the same key as thread `t42` of conversation `general`, and would inherit
//! its binding. [`escape_segment`] percent-escapes the separator so no segment
//! can spill into the next, and there is a test that reddens if it stops.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::event::IncomingMessage;

/// Separator between binding-key segments. Escaped out of every segment.
const SEP: char = '/';

/// A platform conversation, in the terms the binding table indexes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRef {
    /// Platform tag (`"slack"`, `"discord"`).
    pub platform: String,
    /// Receiving bot identity for multi-account platforms.
    pub account_id: Option<String>,
    /// Enclosing space — guild / workspace / team.
    pub space_id: Option<String>,
    /// Chat / room / DM id.
    pub conversation_id: String,
    /// Thread within the conversation, if any.
    pub thread_id: Option<String>,
}

impl ConversationRef {
    /// Build a reference from an inbound message received on `channel_name`.
    ///
    /// `platform` falls back to the channel name when the connector did not
    /// stamp one, matching how the dispatch kernel composes its dedupe key.
    pub fn from_message(channel_name: &str, msg: &IncomingMessage) -> Self {
        Self {
            platform: msg
                .platform
                .clone()
                .unwrap_or_else(|| channel_name.to_string()),
            account_id: msg.account_id.clone(),
            space_id: msg.space_id.clone(),
            conversation_id: msg.conversation_id.clone(),
            thread_id: msg.thread_id.clone(),
        }
    }
}

/// Where a binding routes: a profile, and optionally a named agent within it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteTarget {
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

impl RouteTarget {
    pub fn profile(profile: impl Into<String>) -> Self {
        Self {
            profile: profile.into(),
            agent: None,
        }
    }

    pub fn with_agent(profile: impl Into<String>, agent: impl Into<String>) -> Self {
        Self {
            profile: profile.into(),
            agent: Some(agent.into()),
        }
    }
}

/// Which level of the binding table answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BindingSource {
    /// An explicit binding on the exact thread.
    Thread,
    /// An explicit binding on the conversation.
    Conversation,
    /// An explicit binding on the enclosing space.
    Space,
    /// No explicit binding — the table's DECLARED default answered.
    Default,
}

/// The resolved binding for one conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// Stable internal key for this conversation. Escaped; see module docs.
    pub key: String,
    /// The profile and agent this binding routes to.
    pub target: RouteTarget,
    /// Which level answered — so an operator can see that a route came from
    /// the default rather than from a binding they think they created.
    pub source: BindingSource,
}

/// Percent-escape the separator (and the escape character) out of one key
/// segment, so no segment can spill into the next.
pub fn escape_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '%' => out.push_str("%25"),
            c if c == SEP => out.push_str("%2F"),
            c => out.push(c),
        }
    }
    out
}

/// Compose the stable binding key for `conv` at thread granularity when the
/// conversation carries a thread, conversation granularity otherwise.
pub fn binding_key(conv: &ConversationRef) -> String {
    let mut key = format!(
        "{}{SEP}{}{SEP}{}",
        escape_segment(&conv.platform),
        escape_segment(conv.account_id.as_deref().unwrap_or("-")),
        escape_segment(&conv.conversation_id),
    );
    if let Some(thread) = &conv.thread_id {
        key.push(SEP);
        key.push_str(&escape_segment(thread));
    }
    key
}

/// Conversation-level key (thread stripped) — the fallback lookup.
fn conversation_key(conv: &ConversationRef) -> String {
    format!(
        "{}{SEP}{}{SEP}{}",
        escape_segment(&conv.platform),
        escape_segment(conv.account_id.as_deref().unwrap_or("-")),
        escape_segment(&conv.conversation_id),
    )
}

/// Space-level key — the broadest explicit level.
fn space_key(conv: &ConversationRef) -> Option<String> {
    conv.space_id.as_ref().map(|space| {
        format!(
            "{}{SEP}{}{SEP}space:{}",
            escape_segment(&conv.platform),
            escape_segment(conv.account_id.as_deref().unwrap_or("-")),
            escape_segment(space),
        )
    })
}

/// Conversation-to-profile bindings, with a DECLARED default.
#[derive(Debug, Clone)]
pub struct BindingTable {
    explicit: HashMap<String, RouteTarget>,
    default_target: RouteTarget,
}

impl BindingTable {
    /// Build a table. The default target is REQUIRED — see the module docs on
    /// threat T-24-03-03. There is deliberately no `Default` impl and no
    /// constructor that omits it.
    pub fn new(default_target: RouteTarget) -> Self {
        Self {
            explicit: HashMap::new(),
            default_target,
        }
    }

    /// Bind the exact conversation (or thread, when `conv` carries one).
    pub fn bind(&mut self, conv: &ConversationRef, target: RouteTarget) {
        self.explicit.insert(binding_key(conv), target);
    }

    /// Bind every conversation in an enclosing space.
    pub fn bind_space(
        &mut self,
        platform: &str,
        account_id: Option<&str>,
        space_id: &str,
        target: RouteTarget,
    ) {
        let key = format!(
            "{}{SEP}{}{SEP}space:{}",
            escape_segment(platform),
            escape_segment(account_id.unwrap_or("-")),
            escape_segment(space_id),
        );
        self.explicit.insert(key, target);
    }

    /// The declared default this table falls back to.
    pub fn default_target(&self) -> &RouteTarget {
        &self.default_target
    }

    /// Resolve `conv`, most specific level first: thread, then conversation,
    /// then space, then the declared default.
    pub fn resolve(&self, conv: &ConversationRef) -> Binding {
        let key = binding_key(conv);

        if conv.thread_id.is_some()
            && let Some(target) = self.explicit.get(&key)
        {
            return Binding {
                key,
                target: target.clone(),
                source: BindingSource::Thread,
            };
        }

        let conv_key = conversation_key(conv);
        if let Some(target) = self.explicit.get(&conv_key) {
            return Binding {
                key,
                target: target.clone(),
                source: BindingSource::Conversation,
            };
        }

        if let Some(space_key) = space_key(conv)
            && let Some(target) = self.explicit.get(&space_key)
        {
            return Binding {
                key,
                target: target.clone(),
                source: BindingSource::Space,
            };
        }

        Binding {
            key,
            target: self.default_target.clone(),
            source: BindingSource::Default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(conversation_id: &str, thread: Option<&str>) -> ConversationRef {
        ConversationRef {
            platform: "slack".into(),
            account_id: Some("A1".into()),
            space_id: Some("T9".into()),
            conversation_id: conversation_id.into(),
            thread_id: thread.map(str::to_string),
        }
    }

    #[test]
    fn an_unbound_conversation_takes_the_declared_default_not_the_last_route() {
        let table = BindingTable::new(RouteTarget::profile("fallback"));
        let b = table.resolve(&conv("C-never-seen", None));
        assert_eq!(b.source, BindingSource::Default);
        assert_eq!(b.target.profile, "fallback");
    }

    #[test]
    fn resolution_prefers_thread_then_conversation_then_space() {
        let mut table = BindingTable::new(RouteTarget::profile("fallback"));
        table.bind_space("slack", Some("A1"), "T9", RouteTarget::profile("space"));
        assert_eq!(
            table.resolve(&conv("C1", None)).source,
            BindingSource::Space
        );
        assert_eq!(table.resolve(&conv("C1", None)).target.profile, "space");

        table.bind(&conv("C1", None), RouteTarget::profile("conversation"));
        let b = table.resolve(&conv("C1", Some("t1")));
        assert_eq!(
            b.source,
            BindingSource::Conversation,
            "a thread with no thread-level binding falls back to its conversation"
        );
        assert_eq!(b.target.profile, "conversation");

        table.bind(
            &conv("C1", Some("t1")),
            RouteTarget::with_agent("thread", "ag"),
        );
        let b = table.resolve(&conv("C1", Some("t1")));
        assert_eq!(b.source, BindingSource::Thread);
        assert_eq!(b.target.agent.as_deref(), Some("ag"));
        // The sibling thread is unaffected — a thread binding must not leak
        // sideways.
        assert_eq!(
            table.resolve(&conv("C1", Some("t2"))).source,
            BindingSource::Conversation
        );
    }

    #[test]
    fn a_conversation_id_cannot_impersonate_a_thread_of_another_conversation() {
        // The attack: bind thread `t42` of conversation `general` to a
        // privileged profile, then open a conversation literally NAMED
        // `general/t42`. With a raw separator both compose the same key and the
        // second inherits the first's binding. Escaping is what stops it.
        let mut table = BindingTable::new(RouteTarget::profile("fallback"));
        table.bind(
            &conv("general", Some("t42")),
            RouteTarget::profile("privileged"),
        );

        let impersonator = conv("general/t42", None);
        let b = table.resolve(&impersonator);
        assert_eq!(
            b.source,
            BindingSource::Default,
            "a conversation named across the separator must NOT inherit the \
             thread binding; got {b:?}"
        );
        assert_eq!(b.target.profile, "fallback");

        // And the keys really are distinct, which is the mechanism.
        assert_ne!(
            binding_key(&conv("general", Some("t42"))),
            binding_key(&impersonator)
        );
    }

    #[test]
    fn escaping_is_reversible_enough_to_stay_injective() {
        // A segment that already contains the escape character must not be able
        // to forge an escaped separator.
        assert_ne!(escape_segment("a%2Fb"), escape_segment("a/b"));
        assert_eq!(escape_segment("a/b"), "a%2Fb");
        assert_eq!(escape_segment("a%2Fb"), "a%252Fb");
    }

    #[test]
    fn binding_key_from_message_uses_the_channel_name_when_no_platform_is_stamped() {
        let mut msg = IncomingMessage::new("m1", "C1", "alice", "hi", 0);
        msg.thread_id = Some("t1".into());
        let r = ConversationRef::from_message("acme-slack", &msg);
        assert_eq!(r.platform, "acme-slack");
        msg.platform = Some("slack".into());
        let r = ConversationRef::from_message("acme-slack", &msg);
        assert_eq!(r.platform, "slack");
    }
}
