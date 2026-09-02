//! The typed failure taxonomy a host reads off an error frame.
//!
//! FerroxLabs/wayland#1266 c3 moved this here from
//! `wcore-protocol::events`, unchanged, because a sub-agent`s own category has
//! to survive the relay into the parent and the carrier for that is
//! [`crate::spawner::SubAgentResult`] -- which lives in this crate, below
//! `wcore-protocol` in the dependency graph. `wcore-protocol` re-exports the
//! type, so `wcore_protocol::events::FailureCategory` still resolves at every
//! existing call site and the wire representation is byte-identical.
//!
//! It is listed in `contract::SOURCE_INPUTS` under its new path, so the
//! contract corpus still hashes the definition of a wire-visible type.

use serde::{Deserialize, Serialize};

/// FerroxLabs/wayland#1237, decomposed from wayland#388 c7 — the TYPED half of
/// a host-facing error frame.
///
/// #388 asks the product to expose which of five things went wrong: context or
/// token limit, provider rate limit, router failure, tool or runtime failure,
/// local Wayland error. Two of those five — rate limit and router failure —
/// arrive as the same non-2xx from the same host and cannot be told apart from
/// outside the router; that half is wayland#1184 and stays with flux.
///
/// This enum therefore has NO variant for either of them, and that absence is
/// the design rather than an omission: "core guessed which side of the router
/// it was" is not a state the type can represent. An upstream failure core
/// cannot classify is [`FailureCategory::Unknown`], and a host reading
/// `unknown` learns it must ask the router — not that core silently picked.
///
/// The other three ARE decidable inside core: each has its own exit out of the
/// run loop, and [`crate::events::ErrorInfo::category`] is where the exit says
/// so. Before this, every one of them reached the host as English prose in
/// `message`, and a Desktop app, a JSON-stream consumer or a CI wrapper had to
/// pattern-match it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    /// #388's "context/token limit". The turn could not proceed because the
    /// context window or an output-token ceiling was reached and could not be
    /// reduced — `AgentError::ContextTooLong`.
    ContextLimit,
    /// #388's "tool/runtime failure". A tool, a sub-agent, or the engine task
    /// itself failed or died: the engine-panic guard, an unrecovered tool
    /// breaker, a child that ended without a result.
    ToolRuntime,
    /// #388's "local Wayland error". The local process refused, aborted, or
    /// could not proceed on its own account: a session-persistence authority
    /// fault, a refused or malformed host command, a startup failure, an
    /// operator abort. Nothing upstream is implicated.
    LocalWayland,
    /// Core cannot decide, and says so instead of choosing.
    ///
    /// This is the honest answer for anything that arrives as an opaque
    /// upstream response — every provider non-2xx included, because the
    /// rate-limit-versus-router split (#1184) is not decidable from inside
    /// this repo: both are the same status from the same host.
    ///
    /// It is `Default` only so a frame written before this field existed still
    /// DECODES (see `ErrorInfo`'s `serde(default)`). It is never a default
    /// anyone can fall into while WRITING one: `ErrorInfo` has no `Default`,
    /// so every construction site in the workspace names a category or fails
    /// to compile.
    #[default]
    Unknown,
}
