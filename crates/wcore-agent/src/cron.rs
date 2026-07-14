//! v0.8.1 U7 — production wire-up for `wcore-cron`.
//!
//! This module is the seam where the cron crate's `JobHandler` trait
//! meets the engine's three target surfaces:
//!
//! - [`Target::Slash`] — forwarded to the optional [`SlashSink`] handle
//!   (a closure stored in the handler). Synchronous slash dispatch
//!   needs an active engine + session, so unattended cron firings
//!   currently log+stage the command for the next interactive session.
//! - [`Target::Channel`] — forwarded to [`wcore_channels::ChannelManager::send_to`]
//!   if a manager was supplied to [`EngineJobHandler::new`].
//! - [`Target::Skill`] — forwarded to the optional [`SkillSink`]
//!   handle (a closure that knows how to invoke the engine's
//!   skill-tool dispatch path on a one-shot session).
//!
//! Bootstrap (`bootstrap.rs`) constructs an `EngineJobHandler` and
//! spawns a [`wcore_cron::CronRunner`] with it after the engine is
//! built. The runner handle is stashed on the bootstrap result so
//! `Drop` cancels the background task on session end.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use wcore_channels::{ChannelError, ChannelManager, OutgoingMessage};
use wcore_cron::{CronError, JobHandler, Target};

/// Sink for slash-command dispatch.
///
/// The cron runner is shared across sessions; it cannot synchronously
/// invoke `Dispatcher::try_dispatch` against an active session. Instead
/// the sink receives the raw command string, and bootstrap can plug in
/// any of:
///
/// - a `tracing::info!` logger (default — slash cron fires are recorded
///   and surfaced to the user on next session start),
/// - a session-attached dispatcher (when a long-running session is in
///   flight),
/// - a no-op (for headless deployments).
pub type SlashSink =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// Sink for skill-invocation dispatch.
///
/// Same shape as [`SlashSink`]: `(skill_name, args_json) -> async result`.
pub type SkillSink = Arc<
    dyn Fn(String, serde_json::Value) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

/// The engine-side `JobHandler`. Holds optional surfaces for each
/// target type — a missing surface logs the fire and returns Ok, so
/// the runner keeps ticking and the job's `last_fired` advances.
pub struct EngineJobHandler {
    channels: Option<Arc<RwLock<ChannelManager>>>,
    slash: Option<SlashSink>,
    skill: Option<SkillSink>,
}

impl EngineJobHandler {
    pub fn new(
        channels: Option<Arc<RwLock<ChannelManager>>>,
        slash: Option<SlashSink>,
        skill: Option<SkillSink>,
    ) -> Self {
        Self {
            channels,
            slash,
            skill,
        }
    }

    /// A handler with every surface absent — fires are logged only.
    /// Useful for the headless bootstrap path where no channels are
    /// configured and no live session is attached.
    pub fn log_only() -> Self {
        Self::new(None, None, None)
    }
}

#[async_trait]
impl JobHandler for EngineJobHandler {
    async fn dispatch(&self, target: &Target) -> Result<(), CronError> {
        match target {
            Target::Slash { command } => {
                if let Some(sink) = &self.slash {
                    sink(command.clone())
                        .await
                        .map_err(|e| CronError::Dispatch(format!("slash: {e}")))?;
                    info!(
                        target: "wcore_agent::cron",
                        command = %command,
                        "slash cron fired"
                    );
                } else {
                    info!(
                        target: "wcore_agent::cron",
                        command = %command,
                        "slash cron staged (no active dispatcher — fire logged)"
                    );
                    // rank 3: no live slash dispatcher in this process (the
                    // cross-session dispatcher is out of scope here). Return
                    // NoDispatcher so the runner records the fire as Staged and
                    // advances last_fired (anti-hot-loop) WITHOUT falsely
                    // marking it a success.
                    return Err(CronError::NoDispatcher);
                }
                Ok(())
            }
            Target::Channel { channel_name, text } => {
                let Some(mgr) = &self.channels else {
                    warn!(
                        target: "wcore_agent::cron",
                        channel = %channel_name,
                        "channel cron fire dropped — no ChannelManager wired"
                    );
                    // F-063: return Err so the runner does NOT persist last_fired
                    // for a no-op fire. A missing channel sink means nothing was
                    // sent; advancing the clock would make the job look healthy.
                    return Err(CronError::Dispatch("no channel sink available".to_string()));
                };
                // Convention: when bootstrap-side cron fires, the
                // `channel_name` doubles as the conversation_id of the
                // channel's default room. Per-platform overrides live
                // on the cron job's text or as a future `conversation_id`
                // field; v0.8.1 uses one-room semantics.
                let msg = OutgoingMessage::text(channel_name.clone(), text.clone());
                let guard = mgr.read().await;
                guard
                    .send_to(channel_name, msg)
                    .await
                    .map_err(|e| match e {
                        // A `Config` error (e.g. "unknown channel: X") is permanent:
                        // the channel is not registered in this process and won't be
                        // without a reconfigure. Map it to NoDispatcher so the runner
                        // advances `last_fired` (anti-hot-loop) and stages the fire,
                        // instead of re-firing every tick forever (the source of the
                        // "unknown channel: desktop" 30s retry storm). Genuine
                        // transient send failures stay `Dispatch` → retried.
                        ChannelError::Config(_) => CronError::NoDispatcher,
                        other => CronError::Dispatch(format!("channel send: {other}")),
                    })?;
                debug!(
                    target: "wcore_agent::cron",
                    channel = %channel_name,
                    "channel cron fired"
                );
                Ok(())
            }
            Target::Skill { name, args } => {
                if let Some(sink) = &self.skill {
                    sink(name.clone(), args.clone())
                        .await
                        .map_err(|e| CronError::Dispatch(format!("skill: {e}")))?;
                    info!(
                        target: "wcore_agent::cron",
                        skill = %name,
                        "skill cron fired"
                    );
                } else {
                    info!(
                        target: "wcore_agent::cron",
                        skill = %name,
                        "skill cron staged (no active dispatcher — fire logged)"
                    );
                    // rank 3: no skill sink wired in this process. Return
                    // NoDispatcher so the runner stages the fire (advances
                    // last_fired, anti-hot-loop) rather than recording a false
                    // success.
                    return Err(CronError::NoDispatcher);
                }
                Ok(())
            }
        }
    }
}

/// rank 3: build a real headless [`EngineJobHandler`] for a NO-LLM, NO-TUI
/// process (the `cron daemon`). Without this, the daemon installed a log-only
/// handler and every Skill/Channel fire silently no-op'd.
///
/// Constructs, for the given working directory:
/// - a **skill sink** — engine-less skill execution via a transient
///   [`crate::skill_tool::SkillTool`] (no LLM, no session). The catalog is
///   built the same way `bootstrap.rs` builds it (`load_catalog` +
///   cross-project widening). The M-18 post-substitution `!shell:` body+args
///   scan is copied verbatim from bootstrap so an unattended daemon fire is
///   held to the same execution-boundary denylist.
/// - a **channel sink** — a [`wcore_channels::ChannelManager`] auto-registered
///   from `~/.wayland/channels/*.toml` and `start_all`'d, so Channel cron jobs
///   dispatch.
///
/// Slash stays `None` (the cross-session slash dispatcher is out of scope for
/// a headless process — those fires now correctly yield NoDispatcher → Staged).
///
/// Config loads use `unwrap_or_default()` so the unattended daemon never
/// panics. The caller should treat any construction problem as non-fatal and
/// fall back to [`EngineJobHandler::log_only`].
pub async fn build_headless_cron_handler(cwd: &str) -> EngineJobHandler {
    use std::sync::Arc;

    // Resolved config — default on any load failure so the daemon never
    // panics. Default carries sane skill deny/allow + auto_approve and a
    // working credentials-store opener.
    let config = wcore_config::config::Config::default();

    let cwd_path = std::path::Path::new(cwd);

    // --- Skill sink (engine-less) ---------------------------------------
    // Build the catalog exactly as bootstrap does: load from disk, then widen
    // to sibling projects when cwd has a parent.
    let mut catalog = {
        let refs = wcore_skills::loader::load_catalog(cwd_path, &[], false, None).await;
        wcore_skills::refs::SkillCatalog::from_refs(refs)
    };
    if let Some(siblings_root) = cwd_path.parent() {
        catalog = catalog.with_cross_project_root(siblings_root);
    }
    let catalog = Arc::new(catalog);

    let skill_sink: SkillSink = {
        let catalog_for_cron = Arc::clone(&catalog);
        let deny_rules = config.tools.skills.deny.clone();
        let allow_rules = config.tools.skills.allow.clone();
        let auto_approve = config.tools.auto_approve;
        let workspace_trust = config.workspace_trust.clone();
        let workspace = cwd_path.to_path_buf();
        let cwd_for_cron = cwd.to_string();
        Arc::new(move |skill_name: String, args: serde_json::Value| {
            let catalog = Arc::clone(&catalog_for_cron);
            let checker = wcore_skills::permissions::SkillPermissionChecker::new(
                deny_rules.clone(),
                allow_rules.clone(),
                auto_approve,
            )
            .with_project_execution_trust_snapshot(&workspace, &workspace_trust);
            let cwd = cwd_for_cron.clone();
            Box::pin(async move {
                // Aud-12 / M-18 (+ B8 follow-up): the cron runner's
                // pre-dispatch `scan_target` only inspects the Skill
                // target's name + raw args. The text that actually executes
                // unattended is the skill BODY (`!shell:` directives run via
                // sh -c) AFTER argument substitution. A benign-looking `args`
                // value can splice a denylisted payload into a `!shell:`
                // body line that only becomes dangerous post-substitution.
                //
                // Scan the EXACT post-substitution string the shell will
                // receive: `render_shell_input` is the same function
                // `prepare_inline_content` (inside `SkillTool::execute`)
                // runs to compose the shell input, so the scanned bytes are
                // byte-identical to the executed bytes. The sink builds a
                // `SkillTool::new` (session_id = None) and passes `args`
                // through unchanged as the tool's `args` param, whose
                // `as_str()` is what the executor reads — so we mirror both
                // here. `resolve` hits the catalog LRU that
                // `SkillTool::execute` reuses, so this is not a second disk
                // read in the common case.
                if let Ok(skill) = catalog.resolve(&skill_name).await {
                    let args_str = args.as_str();
                    let composed =
                        wcore_skills::executor::render_shell_input(&skill, args_str, None);
                    // Cheap raw-args scan retained: catches payloads that
                    // never reach a `!shell:` line (e.g. injected into a
                    // non-shell body region) but are still attacker-supplied.
                    let raw_args = serde_json::to_string(&args).unwrap_or_default();
                    for chunk in [composed.as_str(), raw_args.as_str()] {
                        if let Some(reason) = wcore_cron::runner::scan_target_text(chunk) {
                            warn!(
                                target: "wcore_agent::cron",
                                skill = %skill_name,
                                reason = %reason,
                                "cron skill blocked: substituted body/args failed \
                                 execution-boundary scan"
                            );
                            return Err(format!(
                                "cron skill '{skill_name}' blocked before dispatch: {reason}"
                            ));
                        }
                    }
                }
                let tool = crate::skill_tool::SkillTool::new(catalog, cwd, checker);
                let input = serde_json::json!({ "skill": skill_name, "args": args });
                let result = wcore_tools::Tool::execute(&tool, input).await;
                if result.is_error {
                    Err(result.content)
                } else {
                    Ok(())
                }
            })
        })
    };

    // --- Channel sink ----------------------------------------------------
    // Auto-register channels from ~/.wayland/channels and start their poll
    // loops so Channel cron jobs dispatch. Every failure is non-fatal — the
    // handler still has a working skill sink.
    let mut channel_manager_inner = wcore_channels::ChannelManager::new();
    match config.open_credentials_store() {
        Ok(store) => {
            let creds: Arc<dyn wcore_config::credentials::CredentialsStore> = Arc::from(store);
            match wcore_channels_registry::auto_register_from_user_config(
                &mut channel_manager_inner,
                creds,
            )
            .await
            {
                Ok(count) => info!(
                    target: "wcore_agent::cron",
                    count,
                    "headless cron handler: channels auto-registered from ~/.wayland/channels"
                ),
                Err(e) => warn!(
                    target: "wcore_agent::cron",
                    error = %e,
                    "headless cron handler: channel auto-register failed; continuing"
                ),
            }
        }
        Err(e) => warn!(
            target: "wcore_agent::cron",
            error = %e,
            "headless cron handler: credentials store open failed; channels unavailable"
        ),
    }
    let channels = Arc::new(tokio::sync::RwLock::new(channel_manager_inner));
    if let Err(e) = channels.write().await.start_all().await {
        warn!(
            target: "wcore_agent::cron",
            error = %e,
            "headless cron handler: channel start_all failed; inbound polling may be partial"
        );
    }

    EngineJobHandler::new(Some(channels), None, Some(skill_sink))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::Mutex as AsyncMutex;
    use tokio::sync::RwLock as AsyncRwLock;
    use wcore_channels::ChannelManager;
    use wcore_channels::MockChannel;
    use wcore_cron::JobHandler;

    /// rank 3: a log-only handler has no live slash/skill dispatcher, so both
    /// arms must return `CronError::NoDispatcher` (which the runner turns into
    /// a Staged outcome + last_fired advance, NOT a false success). Previously
    /// these arms returned Ok and the runner recorded success on a no-op.
    #[tokio::test]
    async fn log_only_slash_and_skill_return_no_dispatcher() {
        let h = EngineJobHandler::log_only();
        let slash = h
            .dispatch(&Target::Slash {
                command: "/x".into(),
            })
            .await;
        assert!(
            matches!(slash, Err(CronError::NoDispatcher)),
            "slash with no dispatcher must return NoDispatcher, got {slash:?}"
        );
        let skill = h
            .dispatch(&Target::Skill {
                name: "noop".into(),
                args: serde_json::json!({}),
            })
            .await;
        assert!(
            matches!(skill, Err(CronError::NoDispatcher)),
            "skill with no sink must return NoDispatcher, got {skill:?}"
        );
    }

    /// F-063: channel with no sink returns Err so the runner does NOT
    /// persist last_fired for a no-op fire.
    #[tokio::test]
    async fn log_only_channel_returns_err() {
        let h = EngineJobHandler::log_only();
        let result = h
            .dispatch(&Target::Channel {
                channel_name: "no-such".into(),
                text: "hi".into(),
            })
            .await;
        assert!(
            result.is_err(),
            "channel with no sink must return Err to prevent last_fired from advancing"
        );
        match result.unwrap_err() {
            CronError::Dispatch(msg) => assert!(msg.contains("no channel sink")),
            other => panic!("expected Dispatch error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn channel_sink_dispatches_through_manager() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(50));
        mgr.register(Box::new(MockChannel::new("alpha"))).await;
        mgr.start_all().await.unwrap();
        let mgr_arc = Arc::new(AsyncRwLock::new(mgr));

        let h = EngineJobHandler::new(Some(mgr_arc.clone()), None, None);
        h.dispatch(&Target::Channel {
            channel_name: "alpha".into(),
            text: "ping".into(),
        })
        .await
        .unwrap();

        // Stop cleanly.
        mgr_arc.write().await.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn channel_unknown_returns_no_dispatcher_not_dispatch() {
        // A channel cron targeting an UNREGISTERED channel must map to
        // NoDispatcher (permanent → runner stages the fire + advances
        // last_fired) rather than Dispatch (transient → re-fired every tick
        // forever — the source of the "unknown channel: desktop" 30s storm).
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(50));
        mgr.register(Box::new(MockChannel::new("alpha"))).await;
        mgr.start_all().await.unwrap();
        let mgr_arc = Arc::new(AsyncRwLock::new(mgr));

        let h = EngineJobHandler::new(Some(mgr_arc.clone()), None, None);
        let result = h
            .dispatch(&Target::Channel {
                channel_name: "desktop".into(),
                text: "ping".into(),
            })
            .await;
        match result {
            Err(CronError::NoDispatcher) => {}
            other => panic!("expected NoDispatcher for unknown channel, got {other:?}"),
        }

        mgr_arc.write().await.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn slash_sink_invoked() {
        let counter = Arc::new(AsyncMutex::new(0_usize));
        let counter2 = counter.clone();
        let sink: SlashSink = Arc::new(move |_cmd| {
            let c = counter2.clone();
            Box::pin(async move {
                *c.lock().await += 1;
                Ok(())
            })
        });
        let h = EngineJobHandler::new(None, Some(sink), None);
        h.dispatch(&Target::Slash {
            command: "/morning".into(),
        })
        .await
        .unwrap();
        assert_eq!(*counter.lock().await, 1);
    }

    #[tokio::test]
    async fn skill_sink_invoked() {
        let counter = Arc::new(AsyncMutex::new(0_usize));
        let counter2 = counter.clone();
        let sink: SkillSink = Arc::new(move |_name, _args| {
            let c = counter2.clone();
            Box::pin(async move {
                *c.lock().await += 1;
                Ok(())
            })
        });
        let h = EngineJobHandler::new(None, None, Some(sink));
        h.dispatch(&Target::Skill {
            name: "summarize".into(),
            args: serde_json::json!({"k": "v"}),
        })
        .await
        .unwrap();
        assert_eq!(*counter.lock().await, 1);
    }

    /// rank 3: the headless handler must build without panicking and wire a
    /// real skill sink (so daemon-fired Skill jobs dispatch instead of
    /// silently no-op'ing). Slash stays None by design (no cross-session
    /// dispatcher in a headless process). The test module shares the file, so
    /// it can read the otherwise-private fields directly.
    #[tokio::test]
    async fn headless_handler_builds_with_skill_sink() {
        let dir = tempfile::tempdir().unwrap();
        let h = build_headless_cron_handler(&dir.path().to_string_lossy()).await;
        assert!(
            h.skill.is_some(),
            "headless handler must wire a skill sink so Skill cron jobs dispatch"
        );
        assert!(
            h.channels.is_some(),
            "headless handler must wire a channel manager so Channel cron jobs dispatch"
        );
        assert!(
            h.slash.is_none(),
            "slash stays None in a headless process (no cross-session dispatcher)"
        );
    }
}
