//! v0.8.1 U12 — A2A (Agent-to-Agent) protocol on the ACP transport substrate.
//!
//! MVP scope (3 methods):
//! - a2a/handshake
//! - a2a/message/send
//! - a2a/capabilities
//!
//! Remaining 4 methods (message/stream, task/{create,status,cancel}) are
//! v0.8.2 follow-up.

pub mod default_handler;
pub mod handler;
pub mod types;

pub use default_handler::DefaultA2aHandler;
pub use handler::A2aHandler;
pub use types::{A2aAttachment, A2aCapabilities, A2aError, A2aHandshake, A2aMessage};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_handler_handshake_returns_self_identity() {
        let h = DefaultA2aHandler::new("test-agent");
        let incoming = A2aHandshake {
            agent_id: "peer".to_string(),
            agent_kind: "other".to_string(),
            version: "0.0.1".to_string(),
            capabilities: A2aCapabilities::default(),
        };
        let reply = h.on_handshake(incoming).await.unwrap();
        assert_eq!(reply.agent_kind, "wayland-core");
        assert_eq!(reply.agent_id, "test-agent");
    }

    #[tokio::test]
    async fn default_handler_echo_message() {
        let h = DefaultA2aHandler::new("test-agent");
        let msg = A2aMessage {
            from: "peer".to_string(),
            to: "test-agent".to_string(),
            text: "hello".to_string(),
            attachments: vec![],
            correlation_id: Some("corr-1".to_string()),
        };
        let reply = h.on_message(msg).await.unwrap();
        assert_eq!(reply.text, "ack: hello");
        assert_eq!(reply.correlation_id, Some("corr-1".to_string()));
        assert_eq!(reply.from, "test-agent");
        assert_eq!(reply.to, "peer");
    }

    #[tokio::test]
    async fn default_handler_capabilities_starts_empty() {
        let h = DefaultA2aHandler::new("x");
        let caps = h.capabilities().await.unwrap();
        assert!(caps.skills.is_empty());
        assert!(caps.tools.is_empty());
    }

    #[tokio::test]
    async fn set_capabilities_round_trip() {
        let h = DefaultA2aHandler::new("x");
        let mut caps = A2aCapabilities::default();
        caps.skills.push("plan".to_string());
        caps.tools.push("read".to_string());
        h.set_capabilities(caps.clone());
        let got = h.capabilities().await.unwrap();
        assert_eq!(got.skills, vec!["plan"]);
        assert_eq!(got.tools, vec!["read"]);
    }
}
