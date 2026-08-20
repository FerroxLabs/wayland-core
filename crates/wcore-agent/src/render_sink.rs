//! FerroxLabs/wayland#1098 — the wcore-agent half of the `render_artifact`
//! injection.
//!
//! `wcore-tools` owns the `RenderSink` boundary trait (it cannot depend on
//! `wcore-agent`); this adapter binds it to the session's real `OutputSink`.
//! Exactly the shape `host_send_transport::HostDelegatedTransport` uses for
//! `MessageTransport`.

use std::sync::Arc;

use wcore_tools::render::{RenderSink, RenderedArtifact};

use crate::output::OutputSink;

/// Routes a rendered artifact onto the json-stream protocol.
pub struct ProtocolRenderSink {
    output: Arc<dyn OutputSink>,
}

impl ProtocolRenderSink {
    pub fn new(output: Arc<dyn OutputSink>) -> Self {
        Self { output }
    }
}

impl RenderSink for ProtocolRenderSink {
    fn render(&self, artifact: RenderedArtifact) {
        self.output.emit_render_artifact(
            &artifact.call_id,
            &artifact.title,
            artifact.mime,
            &artifact.content,
        );
    }

    /// The honesty gate: live only when the bound sink actually has a render
    /// surface. A terminal, null or relay sink (every sub-agent gets one of
    /// those — `spawner.rs` gives children a `NullSink` or a `ChannelSink`,
    /// never the `ProtocolSink`) reports false, and `render_artifact` then
    /// fails loudly instead of discarding. It stays REGISTERED either way:
    /// `tool_inventory` is inside the recovery authority digest, so the tool
    /// set must not move with the output surface.
    fn is_live(&self) -> bool {
        self.output.render_artifact_supported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::null_sink::NullSink;

    #[test]
    fn a_null_sink_is_not_a_render_surface() {
        let sink = ProtocolRenderSink::new(Arc::new(NullSink));
        assert!(
            !sink.is_live(),
            "a sink with no host must not advertise a render surface"
        );
    }
}
