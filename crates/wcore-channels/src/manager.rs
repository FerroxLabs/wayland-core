//! `ChannelManager` — drives a registry of `Channel` impls.
//!
//! v0.7.0 2.A.2: each channel runs on its own tokio task that
//! polls `poll_events()` and forwards results to a single broadcast
//! channel the engine + UI subscribe to. Outbound sends go through
//! `send_to(name, msg)` which routes to the channel's send_message.
//!
//! Concurrency model: each channel is held in an `Arc<Mutex<Box<dyn
//! Channel>>>` so the poll task and the send path serialize against
//! the same instance (most platform SDKs aren't `Sync`-on-write).
//! Polling cadence is configurable; default 250ms.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;

use crate::Channel;
use crate::error::ChannelError;
use crate::event::{ChannelEvent, MessageReceipt};
use crate::outgoing::OutgoingMessage;

const DEFAULT_POLL_INTERVAL_MS: u64 = 250;
const EVENT_CHANNEL_CAP: usize = 256;

/// Driver for a set of `Channel` instances. Build with `new`, register
/// channels with `register`, then call `start_all` to spawn the poll
/// tasks. `subscribe()` returns a tokio broadcast receiver carrying
/// `ChannelEvent`s tagged with the originating channel name.
pub struct ChannelManager {
    channels: HashMap<String, Arc<Mutex<Box<dyn Channel>>>>,
    poll_tasks: HashMap<String, JoinHandle<()>>,
    poll_interval: Duration,
    events_tx: broadcast::Sender<TaggedEvent>,
}

/// One `ChannelEvent` annotated with the channel that produced it.
#[derive(Debug, Clone)]
pub struct TaggedEvent {
    pub channel_name: String,
    pub event: ChannelEvent,
}

impl ChannelManager {
    pub fn new() -> Self {
        let (events_tx, _) = broadcast::channel(EVENT_CHANNEL_CAP);
        Self {
            channels: HashMap::new(),
            poll_tasks: HashMap::new(),
            poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
            events_tx,
        }
    }

    /// Override the polling interval. Default 250ms.
    pub fn with_poll_interval(mut self, dur: Duration) -> Self {
        self.poll_interval = dur;
        self
    }

    /// Register a channel. Replaces any existing channel under the
    /// same name (stops the old poll task first).
    pub async fn register(&mut self, ch: Box<dyn Channel>) {
        let name = ch.name().to_string();
        if let Some(handle) = self.poll_tasks.remove(&name) {
            handle.abort();
        }
        self.channels.insert(name, Arc::new(Mutex::new(ch)));
    }

    /// Subscribe to the unified event stream. Late subscribers miss
    /// events emitted before they subscribed (broadcast semantics).
    pub fn subscribe(&self) -> broadcast::Receiver<TaggedEvent> {
        self.events_tx.subscribe()
    }

    /// Start every registered channel and spawn its poll task.
    /// Idempotent — channels already started skip re-start.
    pub async fn start_all(&mut self) -> Result<(), ChannelError> {
        for (name, slot) in self.channels.iter() {
            if self.poll_tasks.contains_key(name) {
                continue;
            }
            {
                let mut guard = slot.lock().await;
                guard.start().await?;
            }
            let task_slot = Arc::clone(slot);
            let task_name = name.clone();
            let task_tx = self.events_tx.clone();
            let interval = self.poll_interval;
            let handle = tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await;
                    let evs = {
                        let mut guard = task_slot.lock().await;
                        match guard.poll_events().await {
                            Ok(v) => v,
                            Err(ChannelError::NotStarted) => break,
                            Err(e) => {
                                tracing::warn!(
                                    target: "wcore_channels::manager",
                                    channel = %task_name,
                                    error = %e,
                                    "poll_events errored; backing off one tick"
                                );
                                continue;
                            }
                        }
                    };
                    for event in evs {
                        let _ = task_tx.send(TaggedEvent {
                            channel_name: task_name.clone(),
                            event,
                        });
                    }
                }
            });
            self.poll_tasks.insert(name.clone(), handle);
        }
        Ok(())
    }

    /// Stop every registered channel + abort its poll task.
    pub async fn stop_all(&mut self) -> Result<(), ChannelError> {
        let names: Vec<String> = self.channels.keys().cloned().collect();
        for name in names {
            if let Some(handle) = self.poll_tasks.remove(&name) {
                handle.abort();
            }
            if let Some(slot) = self.channels.get(&name) {
                let mut guard = slot.lock().await;
                let _ = guard.stop().await;
            }
        }
        Ok(())
    }

    /// Send a message through a named channel.
    pub async fn send_to(
        &self,
        name: &str,
        msg: OutgoingMessage,
    ) -> Result<MessageReceipt, ChannelError> {
        let slot = self
            .channels
            .get(name)
            .ok_or_else(|| ChannelError::Config(format!("unknown channel: {name}")))?;
        let mut guard = slot.lock().await;
        guard.send_message(msg).await
    }

    /// List names of registered channels, sorted alphabetically.
    pub fn list_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.channels.keys().cloned().collect();
        names.sort();
        names
    }
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockChannel;
    use std::time::Duration;

    #[tokio::test]
    async fn register_and_list() {
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(MockChannel::new("alpha"))).await;
        mgr.register(Box::new(MockChannel::new("beta"))).await;
        assert_eq!(
            mgr.list_names(),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    #[tokio::test]
    async fn start_all_emits_connection_state_changes() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(20));
        let mut rx = mgr.subscribe();
        mgr.register(Box::new(MockChannel::new("alpha"))).await;
        mgr.start_all().await.unwrap();

        // Each MockChannel emits a Connected event on start().
        let tagged = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("event arrived")
            .expect("ok");
        assert_eq!(tagged.channel_name, "alpha");
        assert!(matches!(
            tagged.event,
            ChannelEvent::ConnectionStateChanged { .. }
        ));
        mgr.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn send_to_unknown_channel_errors() {
        let mgr = ChannelManager::new();
        let err = mgr
            .send_to("missing", OutgoingMessage::text("c1", "x"))
            .await
            .expect_err("expected unknown-channel error");
        assert!(matches!(err, ChannelError::Config(_)));
    }

    #[tokio::test]
    async fn send_to_registered_channel_routes() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(20));
        mgr.register(Box::new(MockChannel::new("alpha"))).await;
        mgr.start_all().await.unwrap();
        // Drain initial state-change event.
        let rx = mgr.subscribe();

        let receipt = mgr
            .send_to("alpha", OutgoingMessage::text("c1", "hello"))
            .await
            .unwrap();
        assert!(!receipt.id.is_empty());
        let _ = rx; // suppress unused
        mgr.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn injected_inbound_reaches_subscriber() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(15));
        let mut rx = mgr.subscribe();
        let mut ch = MockChannel::new("alpha");
        ch.inject_text("c1", "alice", "hi");
        mgr.register(Box::new(ch)).await;
        mgr.start_all().await.unwrap();

        // Loop until we see the MessageReceived (skip state-change).
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut got_msg = false;
        while std::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(tagged)) => {
                    if matches!(tagged.event, ChannelEvent::MessageReceived { .. }) {
                        got_msg = true;
                        break;
                    }
                }
                _ => continue,
            }
        }
        assert!(
            got_msg,
            "expected to receive an injected MessageReceived event"
        );
        mgr.stop_all().await.unwrap();
    }
}
