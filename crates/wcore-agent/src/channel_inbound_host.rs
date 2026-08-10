//! F24-C3-H2 — the inbound stack, assembled once and hosted by any runtime.
//!
//! # Why this module exists
//!
//! Receiving an inbound channel message is not one thing. It is three, and
//! before this module they were only ever assembled together inside
//! [`crate::bootstrap::AgentBootstrap`]:
//!
//! 1. an [`InboundSubscriber`](crate::channel_inbound::InboundSubscriber) on
//!    the [`ChannelManager`] broadcast, which applies each channel's
//!    `[inbound]` access policy and turns admitted messages into agent turns;
//! 2. a [`TurnDispatcher`](crate::channel_inbound::TurnDispatcher) with a
//!    provider behind it, because a turn needs a model;
//! 3. an [inbound webhook host](crate::inbound_webhook) for the platforms whose
//!    inbound arrives as a signed POST rather than by polling.
//!
//! `AgentBootstrap` builds all three, and exactly three call sites opt into it
//! — `run`, `run_tui_mode`, `run_json_stream_mode`. All three are interactive
//! or host-attached sessions.
//!
//! **`gateway run` was not one of them.** The gateway registered its adapters
//! and called `start_all`, so they polled, and then dropped every inbound event
//! on the floor — while its own `[inbound_webhook] enabled = true` said
//! otherwise. Measured live at `e88cf43f` against the running gateway: the
//! process was alive, the config said enabled, and a request to the configured
//! bind got `ECONNREFUSED`. That is the surface Phase 24 installs as a systemd
//! unit, a launchd plist and a scheduled task — the one an operator actually
//! runs — so the persistent runtime could not receive inbound at all.
//!
//! The wiring lives here, rather than being open-coded a second time in
//! `wcore-cli`, so that:
//!
//! - the assembly is testable in-crate rather than only through a binary;
//! - a runtime that hosts channels cannot half-wire it — [`spawn`] either
//!   returns a live [`InboundHost`] or an [`InboundHostError`] naming what is
//!   missing, and there is no third state in which it returns success having
//!   bound nothing;
//! - the next runtime that needs inbound has one call to make.
//!
//! # The refusal, and why it is not a warning
//!
//! `[inbound_webhook] enabled = true` is an explicit operator opt-in. If the
//! stack cannot be built, [`spawn`] returns [`InboundHostError`] and the caller
//! is expected to **refuse to start**, naming the cause. Starting healthy while
//! listening on nothing is the defect this module exists to close, and
//! downgrading it to a log line would reproduce it one level down: the operator
//! reads `enabled = true`, the process reports itself up, and nothing arrives.
//!
//! When the webhook host is NOT enabled the failure is degraded rather than
//! fatal — a gateway with no model can still run its schedule — but it is still
//! *reported*, by being returned to the caller rather than swallowed here.

use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::sync::watch;
use wcore_channels::ChannelManager;
use wcore_config::config::Config;

/// Dedupe window for inbound messages, in milliseconds. Matches the value
/// `AgentBootstrap` uses so the two hosts cannot disagree about what a repeat
/// is.
const DEDUPE_TTL_MS: u64 = 60_000;
/// Dedupe cache capacity. Matches `AgentBootstrap`.
const DEDUPE_MAX: usize = 1024;

/// A live inbound stack. Dropping this does not stop it; call [`InboundHost::shutdown`].
pub struct InboundHost {
    /// The subscriber loop over the manager's broadcast.
    pub subscriber: tokio::task::JoinHandle<()>,
    /// The webhook host task and its shutdown switch, when
    /// `[inbound_webhook] enabled = true` and the bind parsed.
    pub webhook: Option<(tokio::task::JoinHandle<()>, watch::Sender<bool>)>,
    /// The bind the webhook host was asked for, carried so a caller can report
    /// the address it is actually serving without re-reading config.
    pub webhook_bind: Option<String>,
    /// Number of channels whose `[inbound]` policy was loaded. Zero here with
    /// registered adapters means the policy directory and the registration
    /// directory disagree — the F24-C3-H1 shape.
    ///
    /// This is the count **at spawn**. After a
    /// [`reload_policies`](InboundHost::reload_policies) it is stale by
    /// construction — read [`ChannelPolicyRegistry::len`] through
    /// [`InboundHost::policies`] for the live number.
    pub policies_loaded: usize,
    /// The live access-policy + tool-posture registry the subscriber and the
    /// dispatcher both read.
    ///
    /// F24-C3-H5: exposed so a long-lived runtime (`gateway run`) can refresh
    /// inbound configuration when the operator adds a channel, WITHOUT tearing
    /// down and re-spawning the inbound stack. Before this, the maps were
    /// captured at spawn and unreachable afterwards, so `channel reload`
    /// registered an adapter that could never be admitted.
    pub policies: Arc<crate::channel_policy::ChannelPolicyRegistry>,
    /// The workspace root a channel naming no `tool_workspace_root` is jailed
    /// to. Retained so [`reload_policies`](InboundHost::reload_policies)
    /// resolves postures exactly as `spawn` did — re-deriving it at the call
    /// site is how the two lifecycles drift apart.
    workspace: String,
}

impl InboundHost {
    /// Re-read the channel configs from disk and swap BOTH the access
    /// policies and the tool postures into the live registry. Returns the
    /// number of channels now covered.
    ///
    /// # Why this is one call and not two
    ///
    /// F24-C3-H5 has two facets, and repairing only the first is worse than
    /// leaving the bug: the reloaded channel starts receiving (so the obvious
    /// test goes green) while running under the dispatcher's fallback posture
    /// instead of the one its config asked for. Both maps therefore live in
    /// one [`ChannelPolicyRegistry`](crate::channel_policy::ChannelPolicyRegistry)
    /// and move in a single swap; there is no API here that can refresh one.
    ///
    /// Reads through [`crate::bootstrap::load_channel_policy_configs`], the
    /// same named seam `spawn` uses, so the reloaded policies come from the
    /// same directory the adapters were re-registered from (F24-C3-H1).
    ///
    /// # It refuses rather than revoking
    ///
    /// On a load error the current snapshot is **kept** and the error is
    /// returned. The two loaders over `<home>/channels` disagree about a
    /// malformed file — the registry skips it and registers the rest, the
    /// policy loader hard-errors — so swapping in whatever came back would let
    /// one typo'd file deny every already-working channel. That is the same
    /// defect this method exists to fix, arriving from the other direction.
    ///
    /// # It also refuses an admits-everyone config (P2)
    ///
    /// The same refusal `spawn` applies at cold start runs here, because both
    /// go through `ChannelPolicySnapshot::from_configs`. A gate that only ran
    /// at startup would be walked around by exactly the route F24-C3-H5
    /// documents: edit a channel to `dm = "open"`, run `channel reload`, and
    /// the open policy would be live in a process that had already passed its
    /// startup check. The derivation refuses before a snapshot exists, so
    /// nothing is swapped and the bounded policies already in effect stay in
    /// effect.
    pub fn reload_policies(&self) -> Result<usize, wcore_channels::ChannelError> {
        let configs = crate::bootstrap::load_channel_policy_configs()?;
        self.policies
            .replace_from_configs(configs, std::path::Path::new(&self.workspace))
            .map_err(|refusal| wcore_channels::ChannelError::Config(refusal.to_string()))
    }

    /// Stop the webhook host and abort the subscriber.
    pub fn shutdown(self) {
        if let Some((handle, tx)) = self.webhook {
            let _ = tx.send(true);
            handle.abort();
        }
        self.subscriber.abort();
    }
}

/// Why the inbound stack could not be assembled.
///
/// Deliberately a typed error rather than a `String`: the caller's decision
/// (refuse to start vs. degrade) depends on which of these it is, and matching
/// on a message is how that decision rots.
#[derive(Debug, thiserror::Error)]
pub enum InboundHostError {
    /// No usable LLM provider. The inbound path ends in an agent turn, so a
    /// subscriber without a provider would admit messages and then fail every
    /// one of them — an accepts-and-never-does-the-thing shape.
    #[error(
        "inbound dispatch needs an LLM provider and none could be built: {0}. \
         Configure a provider (`wayland-core config` / `[default] provider`), \
         or set `[inbound_webhook] enabled = false` to run without inbound."
    )]
    ProviderUnavailable(String),

    /// `[inbound_webhook] enabled = true` but the bind address does not parse,
    /// so no listener could exist at the address the operator named.
    #[error(
        "[inbound_webhook] enabled = true but bind address {bind:?} is not a \
         valid socket address, so nothing can listen on it"
    )]
    InvalidBind {
        /// The unparseable value, echoed back verbatim.
        bind: String,
    },

    /// P2. One or more channels admit an unbounded set of senders and have not
    /// acknowledged it. Starting would put an agent with this host's tools
    /// behind a door anyone can open, so the stack is not assembled at all.
    ///
    /// Unlike [`Self::ProviderUnavailable`] this is never a degrade-and-carry-on
    /// condition: the caller must refuse to start. A gateway that logged it and
    /// continued would be back to discouraging the dangerous configuration
    /// instead of making it unreachable.
    #[error("{0}")]
    OpenAdmission(#[from] wcore_channels::OpenAdmissionRefusal),

    /// P2 root cause 2. A file under `<profile home>/channels` could not be
    /// read or parsed, so the set of inbound policies is UNKNOWN.
    ///
    /// Fatal for the same reason [`Self::OpenAdmission`] is, and this is the
    /// arm that stops it being fatal-in-name-only. Treating the failure as an
    /// empty list is what let one junk `.toml` switch the open-admission gate
    /// off for every sibling channel — `refuse_open_admission([])` is `Ok` —
    /// and, on a deployment that was working, silently converted the next
    /// restart into universal denial. A security gate must not be satisfiable
    /// by making its input disappear.
    ///
    /// The explanation is on the error itself (see
    /// [`crate::bootstrap::UNREADABLE_CHANNEL_DIR`]) rather than here, so the
    /// gateway, the desktop host and a headless run all print the same words.
    #[error("{0}")]
    PolicyLoad(#[from] wcore_channels::ChannelError),
}

/// Assemble and spawn the inbound stack over `manager`.
///
/// `workspace` is the working directory agent turns run in. The subscriber is
/// spawned **before** this returns, and the caller is expected to call
/// [`ChannelManager::start_all`] **after** it — tokio's broadcast drops events
/// published before a receiver exists, so starting the poll loops first loses
/// every message that arrives in the gap.
///
/// Returns `Err` rather than a half-built host: there is no success value in
/// which the subscriber is absent.
pub async fn spawn(
    manager: Arc<RwLock<ChannelManager>>,
    config: &Config,
    workspace: String,
) -> Result<InboundHost, InboundHostError> {
    // The bind is validated BEFORE the provider is built, so an operator with
    // a typo'd address is told about the typo rather than about a provider.
    let webhook_cfg = config.inbound_webhook.clone();
    if webhook_cfg.enabled && webhook_cfg.bind.parse::<std::net::SocketAddr>().is_err() {
        return Err(InboundHostError::InvalidBind {
            bind: webhook_cfg.bind,
        });
    }

    // F24-C3-H1 lives here: the `[inbound]` access policy and tool posture MUST
    // be read from the same directory the adapters were registered from.
    // `load_channel_policy_configs` is the named seam that guarantees it.
    //
    // F24-C3-H5: both maps are derived by ONE function and held in ONE shared
    // registry, which the subscriber, the dispatcher and the caller's reload
    // path all read. The derivation used to be open-coded here (and again in
    // `bootstrap.rs`), which is what made it possible to refresh the access
    // policy and silently forget the tool posture.
    //
    // P2: `from_configs` REFUSES an admits-everyone channel. It is checked here
    // — before the provider is built and before anything is spawned — so the
    // failure costs nothing and cannot leave a half-armed stack behind.
    //
    // P2 root cause 2: the load is FALLIBLE here. It used to be
    // `unwrap_or_default()`, so one unparseable sibling `.toml` emptied the
    // list and the gate below had nothing to refuse.
    let channel_configs = crate::bootstrap::load_channel_policy_configs()?;
    let policies_loaded = channel_configs.len();
    let policies = Arc::new(crate::channel_policy::ChannelPolicyRegistry::from_configs(
        channel_configs,
        std::path::Path::new(&workspace),
    )?);

    let provider = crate::bootstrap::create_provider_with_oauth(config)
        .map_err(|e| InboundHostError::ProviderUnavailable(e.to_string()))?;

    // Same enricher `AgentBootstrap` installs, including the #660 behaviour of
    // installing it with NO backend configured so an unseen image produces an
    // honest degraded notice rather than a blind answer.
    let media_enricher = {
        let vision = crate::tool_backends::build_vision_backend(config);
        let transcription = crate::tool_backends::build_transcription_backend(config);
        let source = Arc::new(crate::channel_media::ManagerMediaSource::new(Arc::clone(
            &manager,
        )));
        Some(Arc::new(crate::channel_media::ChannelMediaEnricher::new(
            vision,
            transcription,
            source,
        )))
    };

    let dispatcher: Arc<dyn crate::channel_inbound::TurnDispatcher> =
        Arc::new(crate::channel_dispatch::ChannelTurnDispatcher::new(
            config.clone(),
            workspace.clone(),
            provider,
            Arc::clone(&policies),
            media_enricher,
        ));

    let subscriber = crate::channel_inbound::InboundSubscriber::new(
        Arc::clone(&manager),
        dispatcher,
        Arc::clone(&policies),
        DEDUPE_TTL_MS,
        DEDUPE_MAX,
    );
    let subscriber = subscriber.spawn().await;

    let webhook = crate::inbound_webhook::spawn(Arc::clone(&manager), &webhook_cfg);
    let webhook_bind = webhook.as_ref().map(|_| webhook_cfg.bind.clone());

    Ok(InboundHost {
        subscriber,
        webhook,
        webhook_bind,
        policies_loaded,
        policies,
        workspace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> Arc<RwLock<ChannelManager>> {
        Arc::new(RwLock::new(ChannelManager::new()))
    }

    /// A config whose provider is resolvable, so the only variable under test
    /// is the webhook half.
    fn provider_ok_config() -> Config {
        Config {
            api_key: "f24c3h2-not-a-real-key".to_string(),
            ..Config::default()
        }
    }

    /// The refusal is the point of the module: an unparseable bind must NOT
    /// yield a host that reports success and listens on nothing.
    ///
    /// This is the same failure shape as the finding — `enabled = true` over a
    /// socket nobody serves — reached by a different route, so it is checkable
    /// without a running binary.
    #[tokio::test]
    async fn enabled_webhook_with_an_unparseable_bind_refuses_rather_than_returning_a_host() {
        let mut config = provider_ok_config();
        config.inbound_webhook.enabled = true;
        config.inbound_webhook.bind = "not-a-socket-address".to_string();

        let result = spawn(manager(), &config, ".".to_string()).await;

        match result {
            Err(InboundHostError::InvalidBind { bind }) => {
                assert_eq!(
                    bind, "not-a-socket-address",
                    "the refusal must echo the value the operator wrote, not a normalised one"
                );
            }
            Err(other) => panic!("expected InvalidBind, got {other}"),
            Ok(_) => panic!(
                "an enabled webhook with an unservable bind returned a host — this is exactly \
                 F24-C3-H2: reporting healthy while nothing can listen"
            ),
        }
    }

    /// The positive control for the test above. Without it, `InvalidBind` would
    /// also be satisfied by a `spawn` that refused unconditionally, and the
    /// refusal test would pass on a module that never works.
    #[tokio::test]
    async fn a_servable_bind_yields_a_host_that_is_actually_listening() {
        let mut config = provider_ok_config();
        config.inbound_webhook.enabled = true;
        // Port 0 asks the OS for a free port, so this test cannot flake on a
        // busy fixed port and cannot collide with a parallel test.
        config.inbound_webhook.bind = "127.0.0.1:0".to_string();

        let host = spawn(manager(), &config, ".".to_string())
            .await
            .expect("a valid loopback bind must produce a host");

        assert!(
            host.webhook.is_some(),
            "webhook enabled with a valid bind must spawn a host task"
        );
        assert_eq!(host.webhook_bind.as_deref(), Some("127.0.0.1:0"));
        assert!(
            !host.subscriber.is_finished(),
            "the subscriber must still be running immediately after spawn"
        );
        host.shutdown();
    }

    /// With the webhook disabled the subscriber is STILL spawned — the polling
    /// adapters (email IMAP, telegram getUpdates, discord gateway) deliver over
    /// the same broadcast and need no HTTP listener. A host that only wired
    /// inbound when the webhook was enabled would leave every polling adapter
    /// in the gateway exactly as broken as before.
    #[tokio::test]
    async fn a_disabled_webhook_still_spawns_the_subscriber_for_polling_adapters() {
        let mut config = provider_ok_config();
        config.inbound_webhook.enabled = false;

        let host = spawn(manager(), &config, ".".to_string())
            .await
            .expect("a disabled webhook is not an error");

        assert!(
            host.webhook.is_none(),
            "a disabled webhook must not bind a listener"
        );
        assert!(
            !host.subscriber.is_finished(),
            "the subscriber must run even with the webhook disabled, or every polling \
             adapter stays undelivered"
        );
        host.shutdown();
    }
}
