//! v0.8.1 U12 — A2A handler trait.

use async_trait::async_trait;

use super::types::{A2aCapabilities, A2aError, A2aHandshake, A2aMessage};

#[async_trait]
pub trait A2aHandler: Send + Sync {
    async fn on_handshake(&self, h: A2aHandshake) -> Result<A2aHandshake, A2aError>;
    async fn on_message(&self, m: A2aMessage) -> Result<A2aMessage, A2aError>;
    async fn capabilities(&self) -> Result<A2aCapabilities, A2aError>;
}
