//! v0.8.1 U12 — DefaultA2aHandler is a minimal in-process A2A handler.
//!
//! Production wiring substitutes one backed by the engine; this default
//! provides a working substrate so the protocol path is exercised end-to-end
//! before engine-coupling lands.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::handler::A2aHandler;
use super::types::{A2aCapabilities, A2aError, A2aHandshake, A2aMessage};

pub struct DefaultA2aHandler {
    agent_id: String,
    capabilities: Arc<Mutex<A2aCapabilities>>,
}

impl DefaultA2aHandler {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            capabilities: Arc::new(Mutex::new(A2aCapabilities::default())),
        }
    }

    pub fn set_capabilities(&self, caps: A2aCapabilities) {
        if let Ok(mut guard) = self.capabilities.lock() {
            *guard = caps;
        }
    }

    fn caps_snapshot(&self) -> A2aCapabilities {
        self.capabilities
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl A2aHandler for DefaultA2aHandler {
    /// Respond to an A2A handshake.
    ///
    /// F-070 (defense-in-depth): callers that do not identify themselves
    /// (empty `agent_id` in the incoming handshake, i.e. unauthenticated
    /// probes) receive only `agent_kind`. The primary gate is the F-017
    /// auth middleware which blocks unauthenticated requests before they
    /// reach this handler. This secondary check ensures that even if the
    /// auth layer is misconfigured, version and agent_id are not disclosed
    /// to anonymous callers. Authenticated agent peers supply a non-empty
    /// `agent_id` in their handshake.
    async fn on_handshake(&self, h: A2aHandshake) -> Result<A2aHandshake, A2aError> {
        // A caller that supplies an empty agent_id is treated as anonymous.
        // Return only agent_kind — not version or agent_id — to limit
        // fingerprinting surface for unauthenticated probes. (F-070)
        if h.agent_id.is_empty() {
            return Ok(A2aHandshake {
                agent_id: String::new(),
                agent_kind: "wayland-core".to_string(),
                version: String::new(),
                capabilities: A2aCapabilities::default(),
            });
        }

        Ok(A2aHandshake {
            agent_id: self.agent_id.clone(),
            agent_kind: "wayland-core".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: self.caps_snapshot(),
        })
    }

    async fn on_message(&self, m: A2aMessage) -> Result<A2aMessage, A2aError> {
        // Default echo. Real handler substitutes an engine-backed
        // implementation that runs engine.run(m.text).
        Ok(A2aMessage {
            from: self.agent_id.clone(),
            to: m.from,
            text: format!("ack: {}", m.text),
            attachments: vec![],
            correlation_id: m.correlation_id,
        })
    }

    async fn capabilities(&self) -> Result<A2aCapabilities, A2aError> {
        Ok(self.caps_snapshot())
    }
}
