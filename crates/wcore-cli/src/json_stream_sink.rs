//! wayland#1219 — the ONE construction of the `--json-stream` output sink.
//!
//! This lived inline in `main.rs`, where nothing could reach it. It is a named
//! function in the lib so the acceptance test drives the sink the PRODUCTION
//! path builds, not a lookalike assembled in the test. A capability the
//! product forgets to switch on is invisible to a test that switches it on
//! itself.

use std::sync::Arc;

use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_config::tools::AdvertisedCapabilitiesConfig;
use wcore_protocol::writer::ProtocolEmitter;

/// Build the JSON-stream host sink exactly as the runtime does.
///
/// `structured_traces` comes from `[observability] structured_traces`;
/// `advertised` carries the pre-bootstrap capability mirror (cost
/// attribution, online evolution).
pub fn build_json_stream_sink(
    writer: Arc<dyn ProtocolEmitter>,
    structured_traces: bool,
    advertised: Arc<AdvertisedCapabilitiesConfig>,
) -> ProtocolSink {
    ProtocolSink::with_emitter(writer)
        .with_structured_traces(structured_traces)
        .with_advertised_capabilities(advertised)
        // v0.9.4 W1.2 (F2): enable sub-agent event relay to the Desktop
        // host. Harmless when no sub-agents spawn (no-op emission path).
        .with_sub_agent_traces(true)
        // wayland#1219: open the hitl_suspend gate on the host path.
        //
        // Until now `with_hitl_suspend` had ZERO callers anywhere in the
        // workspace, so `emit_approval_required` / `emit_suspend` /
        // `emit_approval_resume` were dead code on `--json-stream` — while
        // bootstrap still installed a BLOCKING egress consent doorbell over
        // this sink. An `EgressVerdict::Ask` therefore emitted nothing, hung
        // for the 300s approval TTL, and failed with a message blaming the
        // user for declining a prompt the host was never sent.
        //
        // The Desktop host already renders `approval_required` (it is the
        // tool-approval modal) and already echoes `resume_token` back on
        // `approval_resume`; a host that does not recognise the frames drops
        // them per the W0 forward-additive decoder contract. Advertised in
        // `ready` as `capabilities.hitl_suspend`.
        .with_hitl_suspend(true)
        // The host reads the first stdout line as the handshake, so
        // `ready` must be the first frame on every platform. Bootstrap
        // emits diagnostics before `ready` exists (on Windows the
        // `windows_job_object` local-shell notice does so on EVERY
        // session); hold them until the handshake is out.
        .deferring_info_until_ready()
}
