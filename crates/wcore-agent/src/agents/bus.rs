//! W7 F2 / v0.8.0 Task J: AgentBus — `tokio::broadcast` channel for
//! cross-agent messages.
//!
//! W7 wired the channel; only `StatusUpdate` was emitted. v0.8.0 Task J
//! (Phase 0 audit CRIT-1) wires the production sub-agent spawner so
//! lifecycle events (`Spawned` / `FirstMessage` / `Completed` /
//! `Errored`) flow on the same bus. Parent agents can subscribe before
//! spawning a child to observe its lifecycle.

use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub enum AgentMessage {
    /// Generic free-form status update, emitted ad-hoc by agents.
    StatusUpdate { agent: String, message: String },
    /// Streamed partial result fragment from a sub-agent (W7 placeholder;
    /// not yet emitted by the spawner — reserved for F3 adaptive
    /// orchestration).
    ResultFragment {
        agent: String,
        payload: serde_json::Value,
    },
    /// Sub-agent asking the parent for guidance (W7 placeholder).
    RequestHelp { agent: String, question: String },
    /// Parent-initiated abort signal (W7 placeholder).
    Abort { reason: String },
    /// v0.8.0 Task J — sub-agent has been constructed and is about to
    /// begin its first turn.
    ///
    /// - `agent`: friendly name from `SubAgentConfig.name` (matches
    ///   `SubAgentResult.name`).
    /// - `parent_call_id`: optional parent's `SpawnTool` call_id; `None`
    ///   for direct `spawn_one` callers that bypass `SpawnTool`.
    /// - `timestamp_ms`: ms since UNIX_EPOCH for ordering on the
    ///   subscriber side. We don't depend on `chrono`/`time` here — a
    ///   plain `u128` keeps the bus dep-free.
    Spawned {
        agent: String,
        parent_call_id: Option<String>,
        timestamp_ms: u128,
    },
    /// v0.8.0 Task J — sub-agent has just received its first input
    /// prompt. `content_preview` is a UTF-8-safe truncation of the
    /// prompt (≤ 200 chars) so subscribers can correlate Spawned with
    /// FirstMessage without paying the full prompt cost on the bus.
    FirstMessage {
        agent: String,
        content_preview: String,
    },
    /// v0.8.0 Task J — sub-agent finished cleanly. `turns` and
    /// `output_tokens` mirror `SubAgentResult.turns` and
    /// `SubAgentResult.usage.output_tokens` so subscribers can build a
    /// per-agent budget view without a side-channel.
    Completed {
        agent: String,
        turns: usize,
        output_tokens: u64,
    },
    /// v0.8.0 Task J — sub-agent finished with an error. `error` is the
    /// `Display`-rendered cause; the spawner converts the underlying
    /// engine error to a short string before publishing.
    Errored { agent: String, error: String },
}

#[derive(Clone)]
pub struct AgentBus {
    tx: broadcast::Sender<AgentMessage>,
}

impl AgentBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }
    pub fn sender(&self) -> broadcast::Sender<AgentMessage> {
        self.tx.clone()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<AgentMessage> {
        self.tx.subscribe()
    }

    /// v0.8.0 Task J — convenience: publish without forcing every
    /// caller through `sender().send(...)`. Returns the number of
    /// subscribers reached (broadcast `send` semantics). When there are
    /// no subscribers the call still succeeds with `Ok(0)` because the
    /// broadcast channel only errors when *all* receivers are dropped
    /// AND the channel is closed — for an active `AgentBus` that's
    /// impossible (the bus itself holds the sender).
    pub fn publish(&self, msg: AgentMessage) -> usize {
        // `send` errors only when there are no active receivers. That
        // is the steady-state for production — most spawn calls happen
        // without an observer subscribed. We silently swallow that
        // single error variant so callers don't have to think about it.
        self.tx.send(msg).unwrap_or(0)
    }
}

impl AgentBus {
    /// v0.8.1 U2 — block until a `Completed` or `Errored` event for
    /// `child_id` arrives, or the timeout fires. Returns the matching
    /// event. Subscribes immediately so events published after this
    /// call returns the future (but before the timeout) are observed;
    /// events published BEFORE the call are missed (broadcast semantics).
    ///
    /// `child_id` is matched against the `agent` field on the lifecycle
    /// variants — the same field the spawner populates from
    /// `SubAgentConfig.name`.
    pub async fn await_completion(
        &self,
        child_id: &str,
        timeout: std::time::Duration,
    ) -> Result<AgentMessage, AgentBusError> {
        let mut rx = self.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(msg)) => {
                    if event_matches_completion(&msg, child_id) {
                        return Ok(msg);
                    }
                    // Otherwise keep draining until the deadline.
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    return Err(AgentBusError::SubscriberLagged);
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                    // Don't fail on lag — keep draining so a late-but-
                    // present terminal event still resolves the caller.
                    continue;
                }
                Err(_) => return Err(AgentBusError::Timeout),
            }
        }
    }
}

/// True when `msg` is a terminal lifecycle event (`Completed` or
/// `Errored`) addressed to `child_id`. Other variants and other agents
/// are skipped.
fn event_matches_completion(msg: &AgentMessage, child_id: &str) -> bool {
    match msg {
        AgentMessage::Completed { agent, .. } | AgentMessage::Errored { agent, .. } => {
            agent == child_id
        }
        _ => false,
    }
}

/// v0.8.1 U2 — error returned by [`AgentBus::await_completion`].
#[derive(Debug, thiserror::Error)]
pub enum AgentBusError {
    /// The broadcast channel closed before the terminal event arrived.
    /// In production this can only happen if the `AgentBus` itself was
    /// dropped — i.e. the engine torn down mid-wait.
    #[error("subscriber lagged or bus closed before completion event")]
    SubscriberLagged,
    /// The configured timeout elapsed before a matching terminal event
    /// was observed.
    #[error("timed out waiting for child completion")]
    Timeout,
}

/// Helper: current UNIX-epoch milliseconds. Returns 0 on the (very
/// unlikely) clock-before-epoch error so the bus never panics on a
/// system clock anomaly.
pub fn now_ms() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Helper: UTF-8-safe truncation for `FirstMessage.content_preview`.
/// Caps the preview at `max_chars` characters (not bytes) and appends
/// an ellipsis when the input was longer.
pub fn preview(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        input.to_string()
    } else {
        let head: String = input.chars().take(max_chars).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bus_status_update_round_trip() {
        let bus = AgentBus::new(16);
        let mut rx = bus.subscribe();
        bus.sender()
            .send(AgentMessage::StatusUpdate {
                agent: "a".into(),
                message: "ok".into(),
            })
            .unwrap();
        match rx.recv().await.unwrap() {
            AgentMessage::StatusUpdate { agent, message } => {
                assert_eq!(agent, "a");
                assert_eq!(message, "ok");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[tokio::test]
    async fn publish_with_no_subscribers_is_ok() {
        // Regression: production hot path. Spawner publishes Spawned
        // even when nobody has subscribed. Must not panic, must not
        // surface an error.
        let bus = AgentBus::new(16);
        let reached = bus.publish(AgentMessage::Spawned {
            agent: "lonely".into(),
            parent_call_id: None,
            timestamp_ms: now_ms(),
        });
        assert_eq!(reached, 0);
    }

    #[tokio::test]
    async fn lifecycle_variants_round_trip() {
        let bus = AgentBus::new(16);
        let mut rx = bus.subscribe();

        bus.publish(AgentMessage::Spawned {
            agent: "child".into(),
            parent_call_id: Some("spawn:child".into()),
            timestamp_ms: 123,
        });
        bus.publish(AgentMessage::FirstMessage {
            agent: "child".into(),
            content_preview: "do the thing".into(),
        });
        bus.publish(AgentMessage::Completed {
            agent: "child".into(),
            turns: 2usize,
            output_tokens: 42u64,
        });
        bus.publish(AgentMessage::Errored {
            agent: "child".into(),
            error: "boom".into(),
        });

        let mut got = Vec::new();
        for _ in 0..4 {
            got.push(rx.recv().await.unwrap());
        }
        assert!(matches!(got[0], AgentMessage::Spawned { .. }));
        assert!(matches!(got[1], AgentMessage::FirstMessage { .. }));
        assert!(matches!(got[2], AgentMessage::Completed { .. }));
        assert!(matches!(got[3], AgentMessage::Errored { .. }));
    }

    #[test]
    fn preview_truncates_long_input() {
        let long = "a".repeat(300);
        let p = preview(&long, 200);
        // 200 chars + 1 ellipsis char = 201 chars
        assert_eq!(p.chars().count(), 201);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn preview_passes_through_short_input() {
        let p = preview("short", 200);
        assert_eq!(p, "short");
    }

    #[test]
    fn preview_handles_multibyte_correctly() {
        // 5-char emoji string, multi-byte. Should truncate by chars not
        // bytes — used to be a class of panic in earlier preview impls.
        let s = "🦀🦀🦀🦀🦀";
        let p = preview(s, 3);
        assert_eq!(p.chars().count(), 4); // 3 + ellipsis
        assert!(p.ends_with('…'));
    }
}
