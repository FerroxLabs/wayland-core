//! Profile-supervisor seam for the ACP transports (persona-profiles PR-7).
//!
//! `wcore-acp` is a mid-layer transport crate and MUST NOT depend on the
//! process/spawn machinery, the profile store, or an ACP client pool. The
//! supervisor is reached through this transport-neutral trait, implemented in
//! `wcore-cli` exactly like [`crate::turn::TurnEngine`], [`crate::a2a::A2aHandler`]
//! and [`crate::roster::AgentRoster`]. Keeps the crate dependency-free while the
//! CLI layer owns process lifecycle.
//!
//! # What this is (and the invariant it enforces)
//!
//! An in-process persona overlay (PR-4') and a per-PROFILE agent are two
//! DIFFERENT mechanisms. A persona shares this process's single
//! credential/home identity; a `profile:<name>` agent is a SEPARATE PROCESS
//! (`wcore acp serve --profile <name>`) with its OWN `WAYLAND_HOME` ⇒ own
//! keys/.env/memory/SOUL. One profile per process is the ONLY safe topology —
//! N profiles in one address space is the credential-bleed the red-team
//! rejected (shared `WAYLAND_HOME`/`*_API_KEY`/egress globals). The router
//! therefore never resolves a profile to an in-process overlay; it spawns/routes
//! to that profile's child.
//!
//! # Hard requirements on any implementation
//!   * **One child = one profile = one identity.** Never multiplex profiles.
//!   * **Fail closed.** An invalid/absent profile ([`crate::error::AcpError`]
//!     from `profile_dir`, or a missing home dir) MUST error — NEVER fall
//!     through to this process's default home (that is the cross-write
//!     corruption `json_stream_profile_guard` exists to prevent).
//!   * **No secrets cross the wire.** The router routes JSON-RPC to a localhost
//!     child; a profile's home path, model, provider, or key is never surfaced
//!     through [`crate::protocol::AgentInfo`] (that stays id + label only, R4).
//!   * **Authz-gated + default-OFF.** The server only consults a router for a
//!     session whose create-time `agent` was AUTHORIZED by the roster, and only
//!     when the operator enabled the supervisor feature.

use std::pin::Pin;

use async_trait::async_trait;
use futures::stream::Stream;

use crate::error::AcpError;
use crate::protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest, SessionGetResponse};

/// A transport-neutral supervisor that fans ACP sessions whose agent is a
/// `profile:<name>` selector out to a per-profile child process.
///
/// Installed on an [`crate::server::AcpServer`] via
/// [`crate::server::AcpServer::with_profile_router`]. When no router is
/// installed, a `profile:` agent is simply unreachable: the roster does not
/// enumerate profiles (feature default-OFF), so `session/create` never
/// authorizes one, and the server behaves exactly as before the extension.
///
/// The server dispatches to the router by branching on the session's
/// create-time-stored `agent` (never the per-message body), so an authorized
/// profile binding cannot be smuggled in per message.
#[async_trait]
pub trait ProfileRouter: Send + Sync {
    /// Establish routing for a newly created session whose `agent` is a
    /// `profile:<name>` selector: spawn (or reuse) the child process for that
    /// profile and open a child session mapped to the parent `session_id`.
    ///
    /// `agent` is the FULL selector as authorized at create-time (e.g.
    /// `"profile:work"`); the implementation strips the `profile:` prefix.
    /// `req` is the originating create request (model/tools/system_prompt) —
    /// forwarded to the child's own `session/create`.
    ///
    /// FAIL CLOSED: on an invalid/absent profile, or a child that fails to
    /// spawn/handshake, return `Err` — the server then discards the parent
    /// session record, so a failed bind never leaves a dangling session that
    /// could later fall through to the default identity.
    async fn open(
        &self,
        session_id: &str,
        agent: &str,
        req: &SessionCreateRequest,
    ) -> Result<(), AcpError>;

    /// Forward a message to the child session mapped to `req.session_id` and
    /// stream the child's response back. The returned stream MUST end with
    /// exactly one terminal [`MessageEvent`] (`Done` or `Error`) — a transport
    /// or child-death failure mid-stream is surfaced as a terminal `Error`
    /// frame, not a dropped stream. An `Err` here is reserved for failures
    /// BEFORE a stream exists (e.g. unknown session).
    async fn send(
        &self,
        req: MessageSendRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError>;

    /// Fetch metadata for the child session mapped to `session_id`.
    async fn get(&self, session_id: &str) -> Result<SessionGetResponse, AcpError>;

    /// Tear down the child session mapped to `session_id`. The implementation
    /// reaps the child process when its last session is deleted.
    async fn delete(&self, session_id: &str) -> Result<(), AcpError>;
}
