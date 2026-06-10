//! `ChannelTurnDispatcher` — the real engine-backed [`TurnDispatcher`].
//!
//! The inbound subscriber ([`crate::channel_inbound::InboundSubscriber`])
//! decides admit/observe/drop and routes a session key; this dispatcher is
//! the seam that turns an admitted inbound message into an actual agent
//! turn and returns the reply text. It mirrors the production
//! `EngineTurnEngine` pattern from `wcore-cli`'s ACP server: a per-session
//! engine pool sharing one provider, building a fresh engine per session
//! via [`AgentBootstrap`] and pooling the `Arc`.
//!
//! It differs from the ACP engine in three deliberate ways:
//!
//! 1. **Silent sink.** Channel turns use [`crate::output::null_sink::NullSink`]
//!    so nothing streams to the CLI/host UI — the only output that matters is
//!    the reply text, which the subscriber sends back through the channel.
//! 2. **No protocol/relay machinery.** There is no protocol writer and no
//!    relay; the reply is the `run()` return value.
//! 3. **Safer tool posture.** Channel senders are remote, so the per-session
//!    engine is built with tool auto-approval FORCED OFF (see
//!    [`ChannelTurnDispatcher::engine_for`]).
//!
//! ## No channel recursion
//!
//! Every per-session engine is built with `.without_channels(true)`, so it
//! does NOT re-register channels, call `start_all`, upgrade the
//! send-message transport, or spawn another inbound subscriber. Without
//! this, each conversation would spin up a fresh channel fleet (and another
//! Telegram poller), recursing without bound.
//
// TODO(phase): (1) the engine pool is unbounded — add LRU / idle eviction so
//   a flood of distinct conversations cannot grow memory without limit.
// TODO(phase): (2) each new session re-runs the full `AgentBootstrap`
//   (re-initialising MCP, plugins, skills per conversation) — heavyweight;
//   share the expensive sub-systems across sessions later.
// TODO(phase): (3) history is in-memory only (lost on process restart unless
//   disk-resume is wired). With `DefaultHasher` the hashed id is also NOT
//   stable across runs, so cross-restart disk resume would not match even if
//   wired — see `hashed_session_id`.
// TODO(phase): (4) per-session engines carry the boot-default
//   `NullMessageTransport` (no outbound send_message transport of their own);
//   replies go back via the subscriber's `send_to`, which is sufficient for
//   v1. Wiring the outer channel transport into the per-session engine is a
//   later enhancement.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use wcore_config::config::Config;
use wcore_providers::LlmProvider;

use crate::bootstrap::AgentBootstrap;
use crate::channel_inbound::TurnDispatcher;
use crate::engine::AgentEngine;
use crate::output::OutputSink;

/// Engine-backed dispatcher: one [`AgentEngine`] per channel session,
/// pooled by the hashed session id, all sharing a single provider.
pub struct ChannelTurnDispatcher {
    config: Config,
    cwd: String,
    provider: Arc<dyn LlmProvider>,
    /// Pool keyed by the HASHED session id (not the raw kernel session key,
    /// which contains colons the `SessionManager` rejects). Each value is an
    /// `Arc<Mutex<AgentEngine>>` so concurrent turns for the SAME session
    /// serialise on the inner mutex while different sessions run freely.
    engines: Arc<Mutex<HashMap<String, Arc<Mutex<AgentEngine>>>>>,
}

impl ChannelTurnDispatcher {
    /// Build a dispatcher over a resolved [`Config`], the working directory
    /// new sessions run in, and the shared provider. Tool auto-approval is
    /// always forced OFF for the per-session engines (see [`Self::engine_for`]).
    pub fn new(config: Config, cwd: String, provider: Arc<dyn LlmProvider>) -> Self {
        Self {
            config,
            cwd,
            provider,
            engines: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Map a kernel session key (e.g. `agent:main:slack:dm:c1`) to a session
    /// id the [`crate::session::SessionManager`] accepts.
    ///
    /// The manager validates ids against `[a-f0-9-]{6,40}` and rejects the
    /// colons the kernel key carries, so we hash the key to a stable
    /// lowercase-hex string. SHA-256 (already a crate dependency) gives a
    /// deterministic 32-byte digest; we hex-encode the first 20 bytes (40
    /// chars) to stay within the manager's length bound. Same input → same id
    /// (stable within and across runs), so a future disk-resume path keyed on
    /// this id would match.
    fn hashed_session_id(session_key: &str) -> String {
        use std::fmt::Write;
        let digest = Sha256::digest(session_key.as_bytes());
        // First 20 bytes → 40 lowercase-hex chars: the upper bound of the
        // manager's 6..=40 pattern, carrying the full first 160 bits of the
        // SHA-256 digest. `GenericArray<u8, _>` has no `LowerHex`, so format
        // each byte (mirrors `file_history::path_bucket`).
        let mut out = String::with_capacity(40);
        for b in &digest[..20] {
            let _ = write!(&mut out, "{b:02x}");
        }
        out
    }

    /// Fetch (or build + cache) the engine for `hashed_id`. One engine per
    /// session preserves conversation history across turns.
    async fn engine_for(&self, hashed_id: &str) -> anyhow::Result<Arc<Mutex<AgentEngine>>> {
        {
            let pool = self.engines.lock().await;
            if let Some(existing) = pool.get(hashed_id) {
                return Ok(existing.clone());
            }
        }

        // Silent sink: channel turns must not stream to the CLI/host UI. The
        // reply text is the `run()` return value, sent back by the subscriber.
        let output: Arc<dyn OutputSink> = Arc::new(crate::output::null_sink::NullSink);

        // SECURITY — tool posture. Channel senders are REMOTE, so we never
        // auto-approve mutating tools (Bash/Write/Spawn) for them. We DO NOT
        // install a `ToolApprovalManager`: the engine's protocol-approval path
        // `.expect()`s a protocol writer (engine.rs `approval_channel`
        // builder), which channel turns deliberately lack — installing a
        // manager without a writer would panic every turn. Instead we drive
        // the engine through its default `ToolConfirmer` path with
        // `tools.auto_approve` FORCED OFF in the per-session config clone
        // below. With auto-approve off, read-only tools (Read/Grep/Glob, which
        // are on the default allow_list) still run, while mutating tools fall
        // through to confirmation. There is no interactive approver on a
        // channel, so a mutating tool gate-then-denies (or, if stdin is a TTY,
        // would block waiting for input it never gets) — both outcomes are the
        // intended safe behaviour for v1: a channel user cannot silently run a
        // shell or write files. Operators who set `--auto-approve` for their
        // local CLI do NOT thereby grant it to channel senders.
        let mut config = self.config.clone();
        config.tools.auto_approve = false;

        let bootstrap = AgentBootstrap::new(config, self.cwd.clone(), output)
            .provider(self.provider.clone())
            // MANDATORY: stop the per-session engine from re-registering
            // channels / spawning pollers / spawning another subscriber.
            .without_channels(true);
        let result = bootstrap.build().await?;
        let mut engine = result.engine;

        engine.init_session(&self.config.provider_label, &self.cwd, Some(hashed_id))?;
        engine.rebind_memory_session().await;
        engine.run_session_start_hooks().await;
        // No `set_approval_manager` / `set_protocol_writer`: see the posture
        // note above. The engine keeps `approval_manager = None` and uses the
        // non-auto-approve `ToolConfirmer`.

        let session = Arc::new(Mutex::new(engine));

        let mut pool = self.engines.lock().await;
        // Another turn may have built the engine concurrently; keep the first
        // to preserve a single conversation history.
        let entry = pool
            .entry(hashed_id.to_string())
            .or_insert_with(|| session.clone());
        Ok(entry.clone())
    }
}

#[async_trait]
impl TurnDispatcher for ChannelTurnDispatcher {
    async fn dispatch(
        &self,
        session_key: &str,
        channel_name: &str,
        msg: &wcore_channels::IncomingMessage,
    ) -> anyhow::Result<Option<String>> {
        let hashed = Self::hashed_session_id(session_key);
        tracing::debug!(
            channel = %channel_name,
            "channel turn dispatch"
        );
        let engine = self.engine_for(&hashed).await?;
        // The inbound message id doubles as the turn's msg_id (stable per
        // inbound event); the dedupe cache upstream already guarantees one
        // dispatch per id.
        let msg_id = msg.id.clone();
        let result = {
            let mut guard = engine.lock().await;
            guard.run(&msg.text, &msg_id).await?
        };
        if result.text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(result.text))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_session_id_is_stable() {
        let key = "agent:main:slack:dm:c1";
        assert_eq!(
            ChannelTurnDispatcher::hashed_session_id(key),
            ChannelTurnDispatcher::hashed_session_id(key),
            "same input must hash to the same id"
        );
    }

    #[test]
    fn hashed_session_id_matches_session_manager_pattern() {
        // The SessionManager accepts only `[a-f0-9-]{6,40}`.
        let id = ChannelTurnDispatcher::hashed_session_id("agent:main:telegram:group:42");
        assert!(id.len() >= 6 && id.len() <= 40, "len {} out of bounds", id.len());
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "id must be lowercase hex: {id}"
        );
    }

    #[test]
    fn hashed_session_id_differs_for_distinct_keys() {
        let a = ChannelTurnDispatcher::hashed_session_id("agent:main:slack:dm:c1");
        let b = ChannelTurnDispatcher::hashed_session_id("agent:main:slack:dm:c2");
        assert_ne!(a, b, "distinct session keys must hash to distinct ids");
    }

    #[test]
    fn hashed_session_id_is_forty_hex_chars() {
        let id = ChannelTurnDispatcher::hashed_session_id("anything");
        assert_eq!(id.len(), 40, "first-40-hex-chars of the SHA-256 digest");
    }
}
