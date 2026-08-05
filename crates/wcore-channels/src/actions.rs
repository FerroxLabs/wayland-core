//! Native-action capability declarations — Phase 24 Success Criterion 3.
//!
//! # The state this replaces, measured
//!
//! [`Channel::edit_message`](crate::Channel::edit_message) and
//! [`Channel::delete_message`](crate::Channel::delete_message) have existed on
//! the trait since Phase 24 opened. On 2026-07-30 **no adapter of ten
//! overrode either** — measured with `/usr/bin/grep` against a live
//! known-positive (`async fn send_message`, 10 overrides across 10 adapter
//! crates; `async fn edit_message`, 0; `async fn delete_message`, 0).
//!
//! The defaults were already honest: both return
//! [`ChannelError::Unsupported`](crate::ChannelError::Unsupported), a NAMED
//! outcome, never a silent `Ok`. So the gap was never truthfulness. It was that
//! **the truth carried no information.** A caller receiving `Unsupported` from
//! WhatsApp and `Unsupported` from Slack learned the same thing from two
//! situations that are not the same situation at all:
//!
//! - WhatsApp Cloud API **has no message-edit endpoint**. No amount of work
//!   closes that. `Unsupported` is the final answer and always will be.
//! - Slack **has `chat.update`**. `Unsupported` there meant "nobody wrote it",
//!   and it read identically.
//!
//! Worse, the only way to find out was to **make the call** — which for a
//! delete is a request you may not want to issue speculatively.
//!
//! This is exactly the shape
//! [`supports_outbound_idempotency`](crate::Channel::supports_outbound_idempotency)
//! already exists to fix on the outbound-delivery side: a capability the
//! delivery spine reads **before** dispatching, so it can choose a different
//! action rather than discover the answer from a failure. [`NativeActions`] is
//! the same construct for the native-action half.
//!
//! # The three states, and why two of them are not one state
//!
//! [`ActionSupport`] deliberately splits "no" into
//! [`PlatformHasNoApi`](ActionSupport::PlatformHasNoApi) and
//! [`NotImplemented`](ActionSupport::NotImplemented). The first is a permanent
//! property of somebody else's product; the second is a backlog item that
//! belongs to us. Collapsing them is how a gap becomes invisible: a matrix of
//! ten `Unsupported`s looks like a completed survey of an impossible feature,
//! and is in fact an unstarted one.
//!
//! # The declaration is not allowed to be its own witness
//!
//! A capability method an adapter simply asserts is worth nothing — the
//! recurring rule in this crate's test suite is that nothing may be the sole
//! witness to its own correctness. [`ActionSupport`] is therefore checkable
//! against behaviour in **both directions**, and
//! `wcore-channels-registry/tests/native_action_matrix.rs` runs that check over
//! every adapter the registry can build:
//!
//! - an op declared [`Implemented`](ActionSupport::Implemented) **must not**
//!   answer `Unsupported` when called — if the override is missing, the trait
//!   default fires and the case reddens;
//! - an op declared [`PlatformHasNoApi`](ActionSupport::PlatformHasNoApi) or
//!   [`NotImplemented`](ActionSupport::NotImplemented) **must** answer
//!   `Unsupported` — if someone implements the op and forgets the declaration,
//!   the case reddens too.
//!
//! Both directions are reachable, which is the property a permanently-red or
//! permanently-green gate lacks.

use serde::{Deserialize, Serialize};

/// Whether one native action is available on an adapter — and when it is not,
/// **whose problem that is.**
///
/// See the [module docs](self) for why "no" is two states rather than one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionSupport {
    /// This adapter drives the platform's real API for the operation.
    ///
    /// A claim the wire has to back. The conformance matrix calls the operation
    /// and fails the adapter if the trait default answers instead.
    Implemented,

    /// The platform exposes no API for this operation. **Permanent.**
    ///
    /// Not a backlog item — there is nothing to build. Carries a short reason
    /// through [`NativeActions::note`] so an operator reading the matrix is not
    /// left to guess whether we surveyed the platform or gave up on it.
    PlatformHasNoApi,

    /// The platform HAS the API; this adapter has not implemented it yet.
    ///
    /// The honest default. An adapter that says nothing lands here, so a new
    /// adapter cannot silently inherit a green.
    NotImplemented,
}

impl ActionSupport {
    /// Whether a call to the corresponding trait method is expected to reach a
    /// real implementation rather than falling through to the `Unsupported`
    /// default.
    pub fn is_implemented(self) -> bool {
        matches!(self, ActionSupport::Implemented)
    }

    /// Stable lowercase token for CLI/JSON rendering.
    pub fn as_str(self) -> &'static str {
        match self {
            ActionSupport::Implemented => "implemented",
            ActionSupport::PlatformHasNoApi => "platform-has-no-api",
            ActionSupport::NotImplemented => "not-implemented",
        }
    }
}

/// One adapter's declared native-action surface.
///
/// Every field defaults to [`ActionSupport::NotImplemented`] via
/// [`NativeActions::none`], so an adapter that declares nothing is reported as
/// having done nothing — never as having nothing to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeActions {
    /// [`Channel::edit_message`](crate::Channel::edit_message).
    pub edit: ActionSupport,
    /// [`Channel::delete_message`](crate::Channel::delete_message).
    pub delete: ActionSupport,
    /// [`Channel::react`](crate::Channel::react).
    pub react: ActionSupport,
    /// [`Channel::send_typing`](crate::Channel::send_typing).
    ///
    /// Note this one's trait default is a silent `Ok(())` rather than an
    /// `Unsupported` error, because a missing typing indicator is invisible and
    /// harmless where a missing edit is not. That makes the declaration the
    /// ONLY way to tell a real typing indicator from a no-op, which is why
    /// typing is in this struct at all.
    pub typing: ActionSupport,
    /// Free-text note explaining any non-[`Implemented`](ActionSupport::Implemented)
    /// entry above — the platform endpoint that does not exist, the CLI flag
    /// that is not exposed, the ticket. Empty when everything is implemented.
    ///
    /// This exists because `platform-has-no-api` is an **absence claim**, and an
    /// absence claim without its evidence is not a measurement.
    pub note: String,
}

impl NativeActions {
    /// Nothing implemented — the trait default.
    pub fn none() -> Self {
        Self {
            edit: ActionSupport::NotImplemented,
            delete: ActionSupport::NotImplemented,
            react: ActionSupport::NotImplemented,
            typing: ActionSupport::NotImplemented,
            note: String::new(),
        }
    }

    /// Builder: set `edit`.
    pub fn edit(mut self, s: ActionSupport) -> Self {
        self.edit = s;
        self
    }
    /// Builder: set `delete`.
    pub fn delete(mut self, s: ActionSupport) -> Self {
        self.delete = s;
        self
    }
    /// Builder: set `react`.
    pub fn react(mut self, s: ActionSupport) -> Self {
        self.react = s;
        self
    }
    /// Builder: set `typing`.
    pub fn typing(mut self, s: ActionSupport) -> Self {
        self.typing = s;
        self
    }
    /// Builder: set the explanatory note.
    pub fn note(mut self, n: impl Into<String>) -> Self {
        self.note = n.into();
        self
    }

    /// The four operations as `(name, support)` pairs, in a stable order, so
    /// renderers and the conformance matrix iterate the same list.
    pub fn entries(&self) -> [(&'static str, ActionSupport); 4] {
        [
            ("edit", self.edit),
            ("delete", self.delete),
            ("react", self.react),
            ("typing", self.typing),
        ]
    }
}

impl Default for NativeActions {
    fn default() -> Self {
        Self::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default must be `NotImplemented`, not `PlatformHasNoApi`.
    ///
    /// Getting this backwards would let every adapter that declares nothing
    /// report "there is nothing to build here" — which is the exact
    /// false-completion the whole type exists to prevent, and it would be
    /// invisible because the matrix would render fully surveyed.
    #[test]
    fn default_is_not_implemented_never_platform_has_no_api() {
        let d = NativeActions::default();
        for (op, s) in d.entries() {
            assert_eq!(
                s,
                ActionSupport::NotImplemented,
                "default for {op} must be NotImplemented"
            );
            assert_ne!(
                s,
                ActionSupport::PlatformHasNoApi,
                "default for {op} must never claim the platform has no API"
            );
        }
        assert!(d.note.is_empty());
    }

    #[test]
    fn is_implemented_discriminates_the_two_negative_states_from_the_positive() {
        assert!(ActionSupport::Implemented.is_implemented());
        assert!(!ActionSupport::PlatformHasNoApi.is_implemented());
        assert!(!ActionSupport::NotImplemented.is_implemented());
        // …and the two negatives are NOT equal to each other. This assertion is
        // the point of the type; if it ever holds, the distinction is gone.
        assert_ne!(
            ActionSupport::PlatformHasNoApi,
            ActionSupport::NotImplemented
        );
    }

    #[test]
    fn serde_round_trips_with_stable_snake_case_tokens() {
        let a = NativeActions::none()
            .edit(ActionSupport::Implemented)
            .delete(ActionSupport::PlatformHasNoApi)
            .note("smtp: a sent mail cannot be recalled");
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("\"implemented\""), "json was {json}");
        assert!(json.contains("\"platform_has_no_api\""), "json was {json}");
        let back: NativeActions = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn entries_order_is_stable() {
        let names: Vec<&str> = NativeActions::none()
            .entries()
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(names, vec!["edit", "delete", "react", "typing"]);
    }
}
