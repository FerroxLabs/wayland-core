//! `MockChannel` — in-memory channel for tests + as a reference
//! implementation showing how the lifecycle hooks compose.

use async_trait::async_trait;
use std::collections::VecDeque;

use crate::Channel;
use crate::error::ChannelError;
use crate::event::{ChannelEvent, ConnectionState, IncomingMessage, MessageReceipt};
use crate::outgoing::OutgoingMessage;

/// Mock channel. `start()` flips state to `Connected`, `stop()` to
/// `Disconnected`. `inject_inbound` queues an event for the next
/// `poll_events`; `sent_messages` records every outbound so tests
/// can assert against it.
pub struct MockChannel {
    name: String,
    platform: String,
    started: bool,
    inbound: VecDeque<ChannelEvent>,
    pub sent: Vec<OutgoingMessage>,
    next_id: u64,
}

impl MockChannel {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            platform: "mock".to_string(),
            started: false,
            inbound: VecDeque::new(),
            sent: Vec::new(),
            next_id: 0,
        }
    }

    /// Queue an inbound event for the next `poll_events`.
    pub fn inject(&mut self, ev: ChannelEvent) {
        self.inbound.push_back(ev);
    }

    /// Convenience: queue an inbound `MessageReceived` for the next poll.
    pub fn inject_text(
        &mut self,
        conversation_id: impl Into<String>,
        author: impl Into<String>,
        text: impl Into<String>,
    ) {
        self.inject(ChannelEvent::MessageReceived {
            msg: IncomingMessage::new(
                format!("mock-in-{}", self.next_id),
                conversation_id,
                author,
                text,
                0,
            ),
        });
        self.next_id += 1;
    }
}

#[async_trait]
impl Channel for MockChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        &self.platform
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if !self.started {
            self.started = true;
            self.inbound
                .push_back(ChannelEvent::ConnectionStateChanged {
                    state: ConnectionState::Connected,
                });
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        if self.started {
            self.started = false;
            self.inbound
                .push_back(ChannelEvent::ConnectionStateChanged {
                    state: ConnectionState::Disconnected,
                });
        }
        Ok(())
    }

    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        if !self.started && self.inbound.is_empty() {
            return Err(ChannelError::NotStarted);
        }
        Ok(self.inbound.drain(..).collect())
    }

    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        if !self.started {
            return Err(ChannelError::NotStarted);
        }
        let id = format!("mock-out-{}", self.next_id);
        self.next_id += 1;
        let receipt = MessageReceipt {
            id,
            conversation_id: msg.conversation_id.clone(),
            ts_secs: 0,
        };
        self.sent.push(msg);
        Ok(receipt)
    }

    fn config_schema(&self) -> &str {
        r#"{"name": "string", "platform": "mock"}"#
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lifecycle_start_then_stop_emits_state_changes() {
        let mut ch = MockChannel::new("acme");
        ch.start().await.unwrap();
        let evs = ch.poll_events().await.unwrap();
        assert!(matches!(
            evs[0],
            ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connected
            }
        ));
        ch.stop().await.unwrap();
        let evs = ch.poll_events().await.unwrap();
        assert!(matches!(
            evs[0],
            ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Disconnected
            }
        ));
    }

    #[tokio::test]
    async fn send_after_stop_errors() {
        let mut ch = MockChannel::new("acme");
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();
        ch.stop().await.unwrap();
        // Drain the state-change event.
        let _ = ch.poll_events().await.unwrap();
        let err = ch
            .send_message(OutgoingMessage::text("c1", "hello"))
            .await
            .expect_err("expected NotStarted");
        assert!(matches!(err, ChannelError::NotStarted));
    }

    #[tokio::test]
    async fn inject_text_round_trips() {
        let mut ch = MockChannel::new("acme");
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();
        ch.inject_text("c1", "alice", "hi there");
        let evs = ch.poll_events().await.unwrap();
        match &evs[0] {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.text, "hi there");
                assert_eq!(msg.author, "alice");
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_records_outbound() {
        let mut ch = MockChannel::new("acme");
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();
        ch.send_message(OutgoingMessage::text("c1", "hi"))
            .await
            .unwrap();
        assert_eq!(ch.sent.len(), 1);
        assert_eq!(ch.sent[0].text, "hi");
    }
}
