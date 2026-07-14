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
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use wcore_channels::ChannelToolPosture;
use wcore_config::config::Config;
use wcore_providers::LlmProvider;
use wcore_types::execution_policy::{ApprovalPolicy, PolicySource};

use crate::bootstrap::AgentBootstrap;
use crate::channel_inbound::TurnDispatcher;
use crate::channel_media::ChannelMediaEnricher;
use crate::channel_tools::ChannelToolScope;
use crate::engine::AgentEngine;
use crate::output::OutputSink;
use crate::session::SessionManager;

/// Engine-backed dispatcher: one [`AgentEngine`] per channel session,
/// pooled by the hashed session id, all sharing a single provider.
pub struct ChannelTurnDispatcher {
    config: Config,
    cwd: String,
    provider: Arc<dyn LlmProvider>,
    /// Per-channel tool posture, keyed by `channel_name`. A channel absent
    /// from this map falls back to the safe `Conversational` posture rooted
    /// at `cwd` — so an unconfigured channel can never accidentally get host
    /// filesystem/shell access.
    postures: HashMap<String, ChannelToolScope>,
    /// Pool keyed by the HASHED session id (not the raw kernel session key,
    /// which contains colons the `SessionManager` rejects). Each value is an
    /// `Arc<Mutex<AgentEngine>>` so concurrent turns for the SAME session
    /// serialise on the inner mutex while different sessions run freely.
    engines: Arc<Mutex<HashMap<String, Arc<Mutex<AgentEngine>>>>>,
    /// Optional inbound-media enricher. `Some` only when the host wired a
    /// vision and/or transcription backend; otherwise inbound attachments
    /// stay bare-URL summaries. Resolves image/audio attachments to derived
    /// text before the turn prompt is built.
    media: Option<Arc<ChannelMediaEnricher>>,
}

fn remote_channel_config(mut config: Config) -> Config {
    config.tools.auto_approve = false;
    config.retain_default_tool_allow_list();
    config
}

impl ChannelTurnDispatcher {
    /// Build a dispatcher over a resolved [`Config`], the working directory
    /// new sessions run in, the shared provider, and the per-channel tool
    /// postures. Tool auto-approval is always forced OFF for the per-session
    /// engines (see [`Self::engine_for`]); the posture additionally
    /// reduces/jails the toolset itself.
    pub fn new(
        config: Config,
        cwd: String,
        provider: Arc<dyn LlmProvider>,
        postures: HashMap<String, ChannelToolScope>,
        media: Option<Arc<ChannelMediaEnricher>>,
    ) -> Self {
        Self {
            config,
            cwd,
            provider,
            postures,
            engines: Arc::new(Mutex::new(HashMap::new())),
            media,
        }
    }

    /// Build the turn prompt, eagerly enriching inbound media first when an
    /// enricher is installed and the message carries attachments. The
    /// enrichment mutates a per-turn clone — the kernel's `IncomingMessage`
    /// is left untouched.
    async fn prompt_for(
        &self,
        channel_name: &str,
        msg: &wcore_channels::IncomingMessage,
    ) -> String {
        match &self.media {
            Some(media) if !msg.attachments.is_empty() => {
                let mut enriched = msg.clone();
                media.enrich(&mut enriched.attachments, channel_name).await;
                build_turn_prompt(&enriched)
            }
            _ => build_turn_prompt(msg),
        }
    }

    /// Resolve the tool scope for `channel_name`, defaulting to the safe
    /// `Conversational` posture rooted at `cwd` for an unconfigured channel.
    fn scope_for(&self, channel_name: &str) -> ChannelToolScope {
        self.postures
            .get(channel_name)
            .cloned()
            .unwrap_or_else(|| ChannelToolScope {
                posture: ChannelToolPosture::Conversational,
                workspace_root: std::path::PathBuf::from(&self.cwd),
            })
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
    async fn engine_for(
        &self,
        hashed_id: &str,
        scope: &ChannelToolScope,
    ) -> anyhow::Result<Arc<Mutex<AgentEngine>>> {
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
        // the engine through its default `ToolConfirmer` path. Remote channel
        // turns are always typed Smart/Prompt below. Clear the
        // legacy boolean here as defense in depth; the typed builder also
        // normalizes both compatibility fields before any skill checker,
        // spawner, or engine is built. Read-only tools on the default
        // allow-list still run, while mutating tools fail closed because a
        // channel has no interactive terminal approver.
        // A local convenience allow-list is not remote authority. Channel
        // sessions have no interactive approver, so inheriting Bash/Write/MCP
        // entries would silently bypass the explicit Prompt posture.
        let config = remote_channel_config(self.config.clone());

        // Load-or-create the session for this id. `init_session` CREATES a
        // session and hard-errors ("Session ID '…' already exists") if the id
        // is already on disk — which happens whenever a prior process
        // persisted this conversation (the in-memory pool only dedupes within
        // one process). So probe the session store first: if the session
        // exists, RESUME it (preserving history across restarts); otherwise
        // create it fresh.
        let session_mgr = SessionManager::new(
            PathBuf::from(&self.config.session.directory),
            self.config.session.max_sessions,
        );
        let existing = session_mgr.load(hashed_id).ok();
        let is_new = existing.is_none();

        let execution_policy = config
            .execution_policy
            .with_requested_approvals(ApprovalPolicy::Prompt, PolicySource::Protocol);
        let mut bootstrap = AgentBootstrap::new(config, self.cwd.clone(), output)
            .with_execution_policy(execution_policy)
            .provider(self.provider.clone())
            // MANDATORY: stop the per-session engine from re-registering
            // channels / spawning pollers / spawning another subscriber.
            .without_channels(true)
            // SECURITY — reduce/jail the toolset for this REMOTE sender so
            // a channel turn cannot reach host filesystem/shell tools.
            .channel_tool_posture(scope.clone());
        if let Some(session) = existing {
            bootstrap = bootstrap.resume(session);
        }
        let result = bootstrap.build().await?;
        let mut engine = result.engine;

        if is_new {
            engine.init_session(&self.config.provider_label, &self.cwd, Some(hashed_id))?;
        }
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

/// Build the prompt text for a channel turn from an inbound message.
///
/// The agent's input is the raw message text plus — when the message
/// carried media — a concise, clearly-delimited summary of each attachment
/// so the model knows files arrived and can decide how to respond (the raw
/// download is a separate, per-connector concern). The attachment lines are
/// untrusted, agent-facing context, NOT system instructions; they describe
/// the kind/type/url the connector populated. A text-only message returns
/// its text unchanged (byte-identical to the pre-media behaviour).
fn build_turn_prompt(msg: &wcore_channels::IncomingMessage) -> String {
    if msg.attachments.is_empty() {
        return msg.text.clone();
    }
    let mut out = msg.text.clone();
    out.push_str("\n\n[attachments received with this message:");
    for (i, att) in msg.attachments.iter().enumerate() {
        let kind = format!("{:?}", att.kind);
        let ty = att.content_type.as_deref().unwrap_or("unknown type");
        // Prefer derived text when present — a transcript for audio, a
        // description for images (populated by the connector or by the
        // inbound-media enricher); else describe the bare media reference.
        if let Some(t) = att.transcribed.as_deref() {
            let label = match att.kind {
                wcore_channels::MediaKind::Image => "description",
                _ => "transcript",
            };
            out.push_str(&format!("\n  {}. {kind} ({ty}) — {label}: {t}", i + 1));
        } else {
            out.push_str(&format!("\n  {}. {kind} ({ty}) — {}", i + 1, att.url));
        }
    }
    out.push(']');
    out
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
        let scope = self.scope_for(channel_name);
        tracing::debug!(
            channel = %channel_name,
            posture = ?scope.posture,
            "channel turn dispatch"
        );
        let engine = self.engine_for(&hashed, &scope).await?;
        // The inbound message id doubles as the turn's msg_id (stable per
        // inbound event); the dedupe cache upstream already guarantees one
        // dispatch per id.
        let msg_id = msg.id.clone();
        let prompt = self.prompt_for(channel_name, msg).await;
        let result = {
            let mut guard = engine.lock().await;
            guard.run(&prompt, &msg_id).await?
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
    fn turn_prompt_is_text_only_without_attachments() {
        let msg = wcore_channels::IncomingMessage::new("m1", "c1", "alice", "hello", 0);
        assert_eq!(build_turn_prompt(&msg), "hello");
    }

    #[test]
    fn turn_prompt_summarizes_attachments() {
        let mut msg = wcore_channels::IncomingMessage::new("m1", "c1", "alice", "look", 0);
        msg.attachments = vec![
            wcore_channels::Attachment {
                url: "https://x/a.png".into(),
                content_type: Some("image/png".into()),
                kind: wcore_channels::MediaKind::Image,
                ..Default::default()
            },
            wcore_channels::Attachment {
                kind: wcore_channels::MediaKind::Audio,
                transcribed: Some("hi there".into()),
                ..Default::default()
            },
        ];
        let p = build_turn_prompt(&msg);
        assert!(p.starts_with("look\n\n[attachments received"));
        assert!(p.contains("Image (image/png) — https://x/a.png"));
        assert!(p.contains("Audio (unknown type) — transcript: hi there"));
        assert!(p.trim_end().ends_with(']'));
    }

    #[test]
    fn turn_prompt_labels_enriched_image_as_description() {
        // An image whose `transcribed` was populated by the media enricher
        // surfaces under a "description" label (not "transcript").
        let mut msg = wcore_channels::IncomingMessage::new("m1", "c1", "alice", "look", 0);
        msg.attachments = vec![wcore_channels::Attachment {
            url: "https://x/a.png".into(),
            content_type: Some("image/png".into()),
            kind: wcore_channels::MediaKind::Image,
            transcribed: Some("a red bicycle".into()),
            ..Default::default()
        }];
        let p = build_turn_prompt(&msg);
        assert!(
            p.contains("Image (image/png) — description: a red bicycle"),
            "got: {p}"
        );
    }

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
        assert!(
            id.len() >= 6 && id.len() <= 40,
            "len {} out of bounds",
            id.len()
        );
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
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

    #[test]
    fn remote_sessions_drop_local_tool_grants() {
        let mut config = Config::default();
        config.tools.auto_approve = true;
        config.tools.allow_list = vec!["Read".into(), "Bash".into(), "Grep".into(), "Write".into()];

        let remote = remote_channel_config(config);

        assert!(!remote.tools.auto_approve);
        assert_eq!(remote.tools.allow_list, vec!["Read", "Grep"]);
    }
}
