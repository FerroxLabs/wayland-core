//! `bridge` — OPT-IN client for the Wayland Desktop WhatsApp bridge.
//!
//! The adapter in [`crate`] speaks the Meta Business Cloud API and nothing
//! else. Wayland Desktop additionally drives two unofficial WhatsApp Web
//! backends through a Node subprocess that speaks JSON-RPC 2.0 over stdio.
//! This module lets Core **talk to that subprocess**. It does not reimplement
//! the WhatsApp Web protocol, and it must not: Baileys is a reverse-engineered
//! client including a Signal-protocol E2E stack.
//!
//! # `meta-business` remains the default, and Node is never required
//!
//! [`WhatsappBackend`] defaults to [`WhatsappBackend::MetaBusiness`], which
//! routes to the Cloud API adapter and touches none of this module. A Core
//! install with no Node, no bridge and no `backend` key configured behaves
//! exactly as it did before this module existed. Selecting `baileys` or
//! `whatsapp-web` is an explicit operator act.
//!
//! # Distribution: the operator supplies the bridge
//!
//! Core does not vendor, download or install the bridge. [`WhatsappBridgeConfig::bridge_path`]
//! names an existing `bridge.js` on disk. Three reasons, in order of weight:
//!
//! 1. **A single-binary install must keep working.** Vendoring the bridge's
//!    dependency tree (measured 2026-07-30 at 122 MB as shipped by Desktop and
//!    139 MB from a fresh `npm install`) into a Rust release, or running a
//!    package install on first use, makes a Node toolchain a de facto
//!    build/runtime dependency of every Core install — including the ones that
//!    will never send a WhatsApp message.
//! 2. **Fetching executable code at runtime is a supply-chain surface** Core
//!    does not otherwise have, and would be the worst option on that axis.
//! 3. **Redistribution carries obligations that not-redistributing does not.**
//!    The bridge's own sources carry attribution the Desktop project wrote for
//!    upstream MIT work, and one backend wraps an Apache-2.0 library. Executing
//!    an operator's own copy is not redistribution. See `docs/whatsapp-bridge.md`.
//!
//! # Fail closed, and never advertise what is not reachable
//!
//! [`preflight`] answers "can this actually run" from the filesystem, and
//! [`Channel::probe`] reports its verdict through
//! [`ProbeOutcome`](wcore_channels::probe::ProbeOutcome), whose `is_ready()` is
//! true only for `Ok`. A missing Node runtime, a missing bridge script, a
//! bridge whose backend package is not installed, or a number that has never
//! been paired each yield `Incomplete` naming exactly what is absent — never a
//! green, and never a `Connected` state.
//!
//! The third of those was added because of a measurement, and it is the one
//! nobody would have anticipated: the real bridge answers its `health` RPC
//! perfectly happily with **no `node_modules` at all**, because `health` is
//! special-cased before any backend loads. Only the following `connect` fails.
//! A readiness check built on the handshake alone reported a green for a bridge
//! that could not send a single message.
//!
//! # The backend is read back from the bridge, not assumed
//!
//! `BridgeSession::open` performs a `health` handshake — the one RPC
//! `bridge.js` answers before loading any backend — and **refuses the session**
//! unless the backend the bridge reports is the backend we asked for.
//!
//! What that actually protects against, measured against the real bridge rather
//! than assumed: a `bridge.js` that **does not understand `--backend`** reports
//! `baileys`, because an absent or valueless flag is what its own argument
//! parser defaults to. So a `bridge_path` aimed at an older or unrelated bridge
//! would silently drive an unofficial client against a personal number. The
//! handshake catches exactly that. (An *unrecognised* `--backend` value is a
//! different case: the bridge echoes it back verbatim and fails later at load
//! with `-32000`. This crate never sends one — [`WhatsappBackend`] is a closed
//! enum and an unknown config value is rejected at parse time.)

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use wcore_channels::event::{ChannelEvent, ConnectionState, MessageReceipt};
use wcore_channels::outgoing::OutgoingMessage;
use wcore_channels::probe::ProbeReport;
use wcore_channels::{Channel, ChannelError};

/// Platform tag shared with the Cloud API adapter — the backend differs, the
/// platform does not.
pub const PLATFORM: &str = "whatsapp";

/// Default seconds to wait for the `health` handshake before giving up on a
/// spawned bridge. Generous: Node cold start on a loaded host is not fast.
pub const DEFAULT_HANDSHAKE_TIMEOUT_SECS: u64 = 20;

/// Default seconds to wait for any other RPC reply.
pub const DEFAULT_RPC_TIMEOUT_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// Backend selection
// ---------------------------------------------------------------------------

/// Which WhatsApp backend a channel instance drives.
///
/// The default is [`Self::MetaBusiness`] — the official Cloud API — and that is
/// deliberate, not incidental. The two bridged variants drive unofficial
/// reverse-engineered clients against a personal WhatsApp account; see
/// [`Self::is_unofficial`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhatsappBackend {
    /// Official Meta WhatsApp Business Cloud API. Handled entirely by
    /// [`crate::WhatsappChannel`]; no subprocess, no Node.
    #[default]
    MetaBusiness,
    /// WhatsApp Web protocol via `@whiskeysockets/baileys`, through the bridge.
    Baileys,
    /// WhatsApp Web protocol via `whatsapp-web.js` + Chromium, through the bridge.
    WhatsappWeb,
}

impl WhatsappBackend {
    /// The exact string `bridge.js --backend` expects, and the exact string
    /// its `health` reply reports back.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::MetaBusiness => "meta-business",
            Self::Baileys => "baileys",
            Self::WhatsappWeb => "whatsapp-web",
        }
    }

    /// Whether this backend is driven through the Node bridge subprocess.
    ///
    /// `meta-business` is reachable through the bridge too, but Core already
    /// speaks the Cloud API natively over HTTPS, so routing it through a Node
    /// subprocess would add a dependency and buy nothing.
    pub const fn is_bridged(self) -> bool {
        matches!(self, Self::Baileys | Self::WhatsappWeb)
    }

    /// Whether this backend is an unofficial client of WhatsApp.
    ///
    /// Both bridged backends drive the WhatsApp Web protocol from a personal
    /// account. That is not a supported use of WhatsApp, and Meta bans accounts
    /// for it — see `docs/whatsapp-bridge.md`. Callers surface this; it is not
    /// this type's job to decide policy.
    pub const fn is_unofficial(self) -> bool {
        matches!(self, Self::Baileys | Self::WhatsappWeb)
    }

    /// The npm package this backend needs installed beside the bridge.
    ///
    /// `None` for `meta-business`, which the bridge serves with `axios` alone.
    /// Measured from `whatsapp-bridge/package.json`.
    pub const fn npm_package(self) -> Option<&'static str> {
        match self {
            Self::MetaBusiness => None,
            Self::Baileys => Some("@whiskeysockets/baileys"),
            Self::WhatsappWeb => Some("whatsapp-web.js"),
        }
    }

    /// Subdirectory of the bridge's session root this backend stores pairing
    /// material under, and the entry inside it that exists once paired.
    ///
    /// Measured from `backends/baileys.js` (`<session>/baileys/creds.json`, via
    /// `useMultiFileAuthState`) and `backends/whatsapp-web.js`
    /// (`<session>/whatsapp-web/session-wayland`, via `LocalAuth` with
    /// `clientId: 'wayland'`).
    const fn pairing_marker(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::MetaBusiness => None,
            Self::Baileys => Some(("baileys", "creds.json")),
            Self::WhatsappWeb => Some(("whatsapp-web", "session-wayland")),
        }
    }

    /// Every accepted wire name, for error messages and operator help.
    pub const ALL_WIRE_NAMES: [&'static str; 3] = ["meta-business", "baileys", "whatsapp-web"];
}

/// A `backend` value that names no known backend.
///
/// Returned rather than defaulted, and the reason is measured rather than
/// assumed. Driving the real `bridge.js` shows it treats an unrecognised
/// `--backend` value verbatim — `health` echoes `baileyz` back — and only
/// fails later, at backend load, with `-32000 Unknown backend: baileyz`. What
/// it DOES silently default to `baileys` is an **absent or valueless**
/// `--backend` flag. Either way a typo must be caught here, at parse time,
/// rather than after a Node process is holding a WhatsApp socket.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown whatsapp backend {got:?} — expected one of: {}", WhatsappBackend::ALL_WIRE_NAMES.join(", "))]
pub struct UnknownBackend {
    /// The rejected value, echoed so the operator can see their typo.
    pub got: String,
}

impl std::str::FromStr for WhatsappBackend {
    type Err = UnknownBackend;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "meta-business" => Ok(Self::MetaBusiness),
            "baileys" => Ok(Self::Baileys),
            "whatsapp-web" => Ok(Self::WhatsappWeb),
            other => Err(UnknownBackend {
                got: other.to_string(),
            }),
        }
    }
}

impl std::fmt::Display for WhatsappBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire_name())
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// TOML schema for one bridged WhatsApp channel instance.
///
/// Carries no secrets: Baileys and whatsapp-web.js authenticate by QR pairing
/// and persist their own session material under [`Self::session_dir`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhatsappBridgeConfig {
    /// Which bridged backend to drive. Must be `baileys` or `whatsapp-web`;
    /// `meta-business` is served by [`crate::WhatsappChannel`] instead.
    pub backend: WhatsappBackend,

    /// Absolute path to the operator's `bridge.js`. Core does not ship one.
    pub bridge_path: PathBuf,

    /// Explicit Node interpreter. When absent, `node` is resolved from `PATH`.
    #[serde(default)]
    pub node_path: Option<PathBuf>,

    /// Directory the bridge persists pairing/session state under. When absent,
    /// the bridge picks its own default beneath the user's home directory.
    #[serde(default)]
    pub session_dir: Option<PathBuf>,

    /// Human-readable label, logs only.
    #[serde(default)]
    pub workspace_name: String,

    /// Fallback chat id when an `OutgoingMessage` arrives with none.
    #[serde(default)]
    pub default_recipient: String,

    /// Seconds to wait for the `health` handshake.
    #[serde(default = "default_handshake_timeout_secs")]
    pub handshake_timeout_secs: u64,

    /// Seconds to wait for any other RPC reply.
    #[serde(default = "default_rpc_timeout_secs")]
    pub rpc_timeout_secs: u64,
}

fn default_handshake_timeout_secs() -> u64 {
    DEFAULT_HANDSHAKE_TIMEOUT_SECS
}

fn default_rpc_timeout_secs() -> u64 {
    DEFAULT_RPC_TIMEOUT_SECS
}

mod preflight;
mod rpc;

pub use preflight::{BridgeLaunch, BridgeUnavailable, preflight};
pub use rpc::BridgeError;

use preflight::pairing_dir;
use rpc::{BridgeSession, Inbox};

// ---------------------------------------------------------------------------
// The adapter
// ---------------------------------------------------------------------------

/// A WhatsApp channel driven through the Node bridge subprocess.
///
/// Construction never spawns anything and never fails — a Core install with a
/// bridged channel configured but no Node present still boots. The failure
/// surfaces at [`Channel::start`] and [`Channel::probe`], named.
pub struct WhatsappBridgeChannel {
    name: String,
    config: WhatsappBridgeConfig,
    state: ConnectionState,
    session: Option<Arc<BridgeSession>>,
    inbox: Inbox,
}

impl WhatsappBridgeChannel {
    /// Construct. Spawns nothing; see [`Channel::probe`] for readiness.
    pub fn new(name: impl Into<String>, config: WhatsappBridgeConfig) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            session: None,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// The backend this instance drives.
    pub fn backend(&self) -> WhatsappBackend {
        self.config.backend
    }

    /// Cached connection state.
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    fn session(&self) -> Result<&Arc<BridgeSession>, ChannelError> {
        self.session.as_ref().ok_or(ChannelError::NotStarted)
    }
}

#[async_trait]
impl Channel for WhatsappBridgeChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        PLATFORM
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.state == ConnectionState::Connected {
            return Ok(());
        }

        // Fail closed BEFORE any state change: a channel that cannot launch
        // must not pass through `Connecting`, because a health projection that
        // samples mid-start would read that as progress.
        let launch = preflight(&self.config).map_err(BridgeError::from)?;

        self.state = ConnectionState::Connecting;

        let session = BridgeSession::open(
            &launch,
            Arc::clone(&self.inbox),
            Duration::from_secs(self.config.handshake_timeout_secs),
            Duration::from_secs(self.config.rpc_timeout_secs),
        )
        .await
        .inspect_err(|_| {
            self.state = ConnectionState::Disconnected;
        })?;

        let session = Arc::new(session);

        // `connect` returns as soon as the socket is created; pairing (QR) and
        // the transition to `connected` arrive as notifications afterwards.
        // So the adapter is Connecting here, NOT Connected — the bridge's own
        // `connection.status` promotes it. Declaring Connected now would be an
        // adapter attesting to a link it has not seen come up.
        if let Err(e) = session.call("connect", serde_json::json!({})).await {
            session.close().await;
            self.state = ConnectionState::Disconnected;
            return Err(e.into());
        }

        self.session = Some(session);
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connecting,
            });
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        if let Some(session) = self.session.take() {
            let _ = session.call("disconnect", serde_json::json!({})).await;
            session.close().await;
        }
        if self.state != ConnectionState::Disconnected {
            self.state = ConnectionState::Disconnected;
            self.inbox
                .lock()
                .await
                .push_back(ChannelEvent::ConnectionStateChanged {
                    state: ConnectionState::Disconnected,
                });
        }
        Ok(())
    }

    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        let mut inbox = self.inbox.lock().await;
        if inbox.is_empty() && self.session.is_none() {
            return Err(ChannelError::NotStarted);
        }
        // The bridge owns the authoritative link state; mirror the last one we
        // saw so `state()` does not lie to a UI that polls it.
        let events: Vec<ChannelEvent> = inbox.drain(..).collect();
        for ev in &events {
            if let ChannelEvent::ConnectionStateChanged { state } = ev {
                self.state = *state;
            }
        }
        Ok(events)
    }

    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        let session = self.session()?;

        // Both bridged backends' `sendMedia` take a LOCAL filePath and reject
        // anything else (verified in backends/baileys.js and
        // backends/whatsapp-web.js) — the `mediaUrl` form is meta-business
        // only. `OutgoingMessage::attachments` carries URLs, so there is no
        // honest mapping. Refuse loudly rather than send the text and drop the
        // attachment silently.
        if !msg.attachments.is_empty() {
            return Err(ChannelError::Unsupported {
                op: "send_message with attachments".to_string(),
                platform: format!("whatsapp/{}", self.config.backend),
            });
        }

        let chat_id = if msg.conversation_id.is_empty() {
            if self.config.default_recipient.is_empty() {
                return Err(ChannelError::Rejected(
                    "no conversation_id and no default_recipient configured".to_string(),
                ));
            }
            self.config.default_recipient.clone()
        } else {
            msg.conversation_id.clone()
        };

        let result = session
            .call(
                "sendText",
                serde_json::json!({ "chatId": chat_id, "text": msg.text }),
            )
            .await?;

        let id = result
            .get("messageId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(MessageReceipt {
            id,
            conversation_id: chat_id,
            ts_secs: chrono::Utc::now().timestamp(),
        })
    }

    async fn react(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let session = self.session()?;
        session
            .call(
                "react",
                serde_json::json!({
                    "chatId": conversation_id,
                    "messageId": message_id,
                    "emoji": emoji,
                }),
            )
            .await?;
        Ok(())
    }

    async fn send_typing(&self, conversation_id: &str) -> Result<(), ChannelError> {
        let session = self.session()?;
        session
            .call(
                "setPresence",
                serde_json::json!({ "chatId": conversation_id, "presence": "composing" }),
            )
            .await?;
        Ok(())
    }

    /// What this adapter drives through the bridge, and what it does not.
    ///
    /// `edit`/`delete`: the bridge's method allowlist has no entry for either,
    /// so there is no RPC to call — this is a bridge-surface absence, recorded
    /// as `NotImplemented` (a wider bridge could add them) rather than
    /// `PlatformHasNoApi` (the Web protocol does have both).
    fn native_actions(&self) -> wcore_channels::NativeActions {
        use wcore_channels::ActionSupport::{Implemented, NotImplemented};
        wcore_channels::NativeActions::none()
            .edit(NotImplemented)
            .delete(NotImplemented)
            .react(Implemented)
            .typing(Implemented)
            .note(
                "driven through the Wayland Desktop WhatsApp bridge. edit/delete: the bridge's \
                 ALLOWED_RPC_METHODS has no edit or delete method, so no RPC exists to call. \
                 Attachments are refused: both bridged backends' sendMedia require a local \
                 filePath and OutgoingMessage carries URLs.",
            )
    }

    /// Readiness, answered from the filesystem and then from the bridge itself.
    ///
    /// This is the surface that keeps the capability from being advertised when
    /// it is not reachable, and every one of its three gates was put there by a
    /// measurement rather than by anticipation:
    ///
    /// 1. **[`preflight`]** — Node, the script, and the backend's npm package.
    ///    Without the third, a bridge with no `node_modules` answers `health`
    ///    happily and then fails every `connect`.
    /// 2. **The `health` handshake** — the bridge confirms, in its own words,
    ///    which backend it loaded.
    /// 3. **Pairing material on disk** — a reachable bridge that has never been
    ///    paired cannot send anything. `ProbeOutcome::Ok` sets
    ///    `authenticated: true`, so claiming it without evidence of a pairing
    ///    would be exactly the unearned green this type exists to prevent.
    async fn probe(&self) -> Result<ProbeReport, ChannelError> {
        let launch = match preflight(&self.config) {
            Ok(l) => l,
            // `Incomplete` is the right verdict: nothing has been asked of the
            // platform yet, the operator's own machine is missing a piece.
            Err(u) => return Ok(ProbeReport::incomplete(&self.name, PLATFORM, u.findings)),
        };

        // A throwaway inbox: a probe must not inject events into the live one.
        let scratch: Inbox = Arc::new(Mutex::new(VecDeque::new()));
        match BridgeSession::open(
            &launch,
            scratch,
            Duration::from_secs(self.config.handshake_timeout_secs),
            Duration::from_secs(self.config.rpc_timeout_secs),
        )
        .await
        {
            Ok(session) => {
                session.close().await;

                // Gate 3. Both bridged backends authenticate by QR pairing, and
                // a probe must send nothing — so pairing is read from the
                // material the backend persists, at the path the bridge itself
                // computes.
                match pairing_dir(&self.config) {
                    Some(marker) if marker.exists() => Ok(ProbeReport::ok(
                        &self.name,
                        PLATFORM,
                        format!("bridge/{}", self.config.backend),
                    )),
                    _ => Ok(ProbeReport::incomplete(
                        &self.name,
                        PLATFORM,
                        vec!["whatsapp_pairing".to_string()],
                    )),
                }
            }
            Err(BridgeError::BackendMismatch { want, got }) => Ok(ProbeReport::unauthenticated(
                &self.name,
                PLATFORM,
                format!("backend_mismatch: requested {want}, bridge loaded {got}"),
            )),
            Err(e) => Ok(ProbeReport::unreachable(
                &self.name,
                PLATFORM,
                e.to_string(),
            )),
        }
    }

    fn config_schema(&self) -> &str {
        include_str!("../../schemas/whatsapp-bridge.json")
    }

    /// WhatsApp caps a single text body at 4096 characters on every backend.
    fn max_message_len(&self) -> Option<usize> {
        Some(4096)
    }

    // `supports_outbound_idempotency` is deliberately NOT overridden. The
    // bridge transmits no idempotency key and neither WhatsApp Web backend
    // accepts one, so the trait default of `false` is the true answer. Slack
    // and Discord both declared `true` here on mockito evidence and both
    // produced duplicates against the real API; a mock cannot witness what a
    // destination does with a key.
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::path::PathBuf;

    /// A bridged-backend config with short timeouts, for tests.
    pub(crate) fn cfg(backend: WhatsappBackend, bridge_path: PathBuf) -> WhatsappBridgeConfig {
        WhatsappBridgeConfig {
            backend,
            bridge_path,
            node_path: None,
            session_dir: None,
            workspace_name: "test".to_string(),
            default_recipient: String::new(),
            handshake_timeout_secs: 5,
            rpc_timeout_secs: 5,
        }
    }

    /// Lay out a directory that looks the way an installed bridge looks:
    /// `<root>/bridge.js` plus `<root>/node_modules/<pkg>/`. Returns the
    /// tempdir (kept alive by the caller) and the script path.
    pub(crate) fn installed_bridge(backend: WhatsappBackend) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("bridge.js");
        std::fs::write(&script, "// stand-in\n").unwrap();
        if let Some(pkg) = backend.npm_package() {
            std::fs::create_dir_all(dir.path().join("node_modules").join(pkg)).unwrap();
        }
        (dir, script)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::cfg;
    use super::*;
    use crate::bridge::rpc::testing::test_session;
    use std::path::PathBuf;
    use std::str::FromStr;

    // -- selector: both directions -----------------------------------------

    #[test]
    fn backend_default_is_meta_business_not_baileys() {
        // The load-bearing default. bridge.js defaults to `baileys`; Core must
        // not inherit that, or an install that says nothing about backends
        // would drive an unofficial client.
        assert_eq!(WhatsappBackend::default(), WhatsappBackend::MetaBusiness);
        assert!(!WhatsappBackend::default().is_bridged());
        assert!(!WhatsappBackend::default().is_unofficial());
    }

    #[test]
    fn backend_selector_accepts_exactly_the_three_known_names() {
        // CAN PASS.
        assert_eq!(
            WhatsappBackend::from_str("meta-business").unwrap(),
            WhatsappBackend::MetaBusiness
        );
        assert_eq!(
            WhatsappBackend::from_str("baileys").unwrap(),
            WhatsappBackend::Baileys
        );
        assert_eq!(
            WhatsappBackend::from_str("whatsapp-web").unwrap(),
            WhatsappBackend::WhatsappWeb
        );
    }

    #[test]
    fn backend_selector_rejects_an_unknown_name_rather_than_defaulting() {
        // CAN FAIL. A typo must not silently become baileys.
        for bad in ["baileyz", "", "META-BUSINESS", "whatsapp_web", "cloud"] {
            let err = WhatsappBackend::from_str(bad)
                .unwrap_err_or_panic(&format!("{bad:?} must be rejected"));
            assert_eq!(err.got, bad);
            assert!(
                err.to_string().contains("baileys"),
                "the rejection must name the valid options: {err}"
            );
        }
    }

    #[test]
    fn wire_names_round_trip_through_serde_and_from_str() {
        // The two parsers must not drift: `--backend` uses `wire_name`, the
        // TOML uses serde, and the health read-back compares against
        // `wire_name`. A mismatch would make the handshake permanently fail.
        for b in [
            WhatsappBackend::MetaBusiness,
            WhatsappBackend::Baileys,
            WhatsappBackend::WhatsappWeb,
        ] {
            let json = serde_json::to_string(&b).unwrap();
            let unquoted = json.trim_matches('"');
            assert_eq!(unquoted, b.wire_name(), "serde and wire_name disagree");
            assert_eq!(WhatsappBackend::from_str(unquoted).unwrap(), b);
        }
        assert_eq!(
            WhatsappBackend::ALL_WIRE_NAMES.len(),
            3,
            "ALL_WIRE_NAMES must list every variant"
        );
    }

    #[test]
    fn toml_rejects_an_unknown_backend_value() {
        // The registry parses TOML, so the rejection must survive that path too.
        let body = r#"
backend = "baileyz"
bridge_path = "/nonexistent/bridge.js"
"#;
        let err = toml::from_str::<WhatsappBridgeConfig>(body).unwrap_err();
        assert!(
            err.to_string().contains("baileyz") || err.to_string().contains("unknown variant"),
            "expected an unknown-variant rejection, got: {err}"
        );
    }

    #[test]
    fn toml_accepts_a_known_backend_value() {
        // Control for the test above: the same shape with a valid name parses,
        // proving the rejection came from the VALUE and not the schema.
        let body = r#"
backend = "baileys"
bridge_path = "/nonexistent/bridge.js"
"#;
        let cfg: WhatsappBridgeConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.backend, WhatsappBackend::Baileys);
        assert_eq!(cfg.handshake_timeout_secs, DEFAULT_HANDSHAKE_TIMEOUT_SECS);
    }

    // -- adapter: fails closed without ever reporting health ----------------

    #[tokio::test]
    async fn construction_never_spawns_and_start_fails_closed_when_node_is_absent() {
        let mut c = cfg(
            WhatsappBackend::Baileys,
            PathBuf::from("/definitely/not/here/bridge.js"),
        );
        c.node_path = Some(PathBuf::from("/definitely/not/here/node"));

        // Construction must succeed — a Core install with this configured but
        // nothing installed still has to boot.
        let mut ch = WhatsappBridgeChannel::new("wa-personal", c);
        assert_eq!(ch.state(), ConnectionState::Disconnected);

        let err = ch.start().await.unwrap_err();
        match &err {
            ChannelError::Config(m) => {
                assert!(m.contains("node"), "must name node: {m}");
                assert!(m.contains("bridge_path"), "must name bridge_path: {m}");
            }
            other => panic!("expected Config with an actionable message, got {other:?}"),
        }
        assert_eq!(
            ch.state(),
            ConnectionState::Disconnected,
            "a channel that cannot launch must NOT sit in Connecting"
        );
    }

    #[tokio::test]
    async fn probe_reports_incomplete_and_not_ready_when_the_bridge_is_absent() {
        // The anti-false-advertising property: an unreachable bridge must never
        // read as ready anywhere.
        let mut c = cfg(
            WhatsappBackend::Baileys,
            PathBuf::from("/definitely/not/here/bridge.js"),
        );
        c.node_path = Some(PathBuf::from("/definitely/not/here/node"));

        let ch = WhatsappBridgeChannel::new("wa-personal", c);
        let report = ch.probe().await.unwrap();

        assert_eq!(
            report.outcome,
            wcore_channels::probe::ProbeOutcome::Incomplete
        );
        assert!(
            !report.outcome.is_ready(),
            "an absent bridge must not be advertised as ready"
        );
        assert!(!report.config_complete);
        assert!(!report.authenticated);
        assert!(report.findings.contains(&"node_runtime".to_string()));
        assert!(report.findings.contains(&"bridge_path".to_string()));
    }

    #[tokio::test]
    async fn a_bridged_channel_does_not_claim_outbound_idempotency() {
        // Slack and Discord both claimed this on mock evidence and both
        // duplicated against the real API. The bridge transmits no key.
        let ch =
            WhatsappBridgeChannel::new("wa", cfg(WhatsappBackend::Baileys, PathBuf::from("/nope")));
        assert!(!ch.supports_outbound_idempotency());
    }

    #[tokio::test]
    async fn attachments_are_refused_rather_than_silently_dropped() {
        // There is no honest mapping from a URL attachment to either bridged
        // backend's local-filePath sendMedia, so the send must fail visibly.
        // Driven over a fabricated wire so no Node is needed.
        let (session, _srv) = test_session(WhatsappBackend::Baileys).await;
        let mut ch =
            WhatsappBridgeChannel::new("wa", cfg(WhatsappBackend::Baileys, PathBuf::from("/nope")));
        ch.session = Some(session);

        let msg = OutgoingMessage {
            conversation_id: "123@s.whatsapp.net".to_string(),
            text: "see attached".to_string(),
            thread_id: None,
            reply_to: None,
            attachments: vec!["https://cdn.example/pic.jpg".to_string()],
        };
        let err = ch.send_message(msg).await.unwrap_err();
        assert!(
            matches!(err, ChannelError::Unsupported { .. }),
            "got {err:?}"
        );
    }

    /// Small helper so the rejection test reads as one assertion per case.
    trait UnwrapErrOrPanic<T, E> {
        fn unwrap_err_or_panic(self, msg: &str) -> E;
    }

    impl<T: std::fmt::Debug, E> UnwrapErrOrPanic<T, E> for Result<T, E> {
        fn unwrap_err_or_panic(self, msg: &str) -> E {
            match self {
                Ok(v) => panic!("{msg} — but it was accepted as {v:?}"),
                Err(e) => e,
            }
        }
    }
}
