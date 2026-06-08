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
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use wcore_channels::{ChannelManager, OutgoingMessage};
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
    channels: Option<Arc<Mutex<ChannelManager>>>,
    slash: Option<SlashSink>,
    skill: Option<SkillSink>,
}

impl EngineJobHandler {
    pub fn new(
        channels: Option<Arc<Mutex<ChannelManager>>>,
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
                        "slash cron fired (no active dispatcher — fire logged)"
                    );
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
                let guard = mgr.lock().await;
                guard
                    .send_to(channel_name, msg)
                    .await
                    .map_err(|e| CronError::Dispatch(format!("channel send: {e}")))?;
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
                        "skill cron fired (no active dispatcher — fire logged)"
                    );
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::Mutex as AsyncMutex;
    use wcore_channels::ChannelManager;
    use wcore_channels::MockChannel;
    use wcore_cron::JobHandler;

    #[tokio::test]
    async fn log_only_handler_succeeds_on_slash_and_skill() {
        let h = EngineJobHandler::log_only();
        h.dispatch(&Target::Slash {
            command: "/x".into(),
        })
        .await
        .unwrap();
        h.dispatch(&Target::Skill {
            name: "noop".into(),
            args: serde_json::json!({}),
        })
        .await
        .unwrap();
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
        let mgr_arc = Arc::new(AsyncMutex::new(mgr));

        let h = EngineJobHandler::new(Some(mgr_arc.clone()), None, None);
        h.dispatch(&Target::Channel {
            channel_name: "alpha".into(),
            text: "ping".into(),
        })
        .await
        .unwrap();

        // Stop cleanly.
        mgr_arc.lock().await.stop_all().await.unwrap();
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
}
