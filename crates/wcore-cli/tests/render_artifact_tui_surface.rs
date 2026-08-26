//! FerroxLabs/wayland#1138 — the TUI half of `render_artifact`.
//!
//! #1098 shipped the engine half (a real tool, a capped and redacted
//! chokepoint, a published wire type) and exactly one sink that could carry
//! it: the json-stream `ProtocolSink`. Under the TUI the artifact was
//! discarded twice over — `ChannelSink` inherited the two `OutputSink`
//! DEFAULTS, so it claimed no render surface and swallowed the emit, and the
//! bridge's `RenderArtifact` arm was an explicit no-op.
//!
//! These tests exercise the COMPOSED stack — the real tool over the real
//! `ProtocolRenderSink` over the real `ChannelSink`, and the resulting frame
//! through the real `apply_event` — because a trait default that silently
//! swallows is a trap a unit test on the override cannot catch.

use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use wcore_agent::output::OutputSink;
use wcore_agent::render_sink::ProtocolRenderSink;
use wcore_cli::tui::app::App;
use wcore_cli::tui::ChannelSink;
use wcore_cli::tui::apply_event;
use wcore_cli::tui::TurnElement;
use wcore_protocol::events::{ProtocolEvent, RenderMime};
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::render::RenderArtifactTool;
use wcore_tools::vfs::{RealFs, SandboxedFs};

fn channel() -> (
    Arc<ChannelSink>,
    tokio::sync::mpsc::UnboundedReceiver<ProtocolEvent>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (Arc::new(ChannelSink::new(tx)), rx)
}

fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<ProtocolEvent>) -> Vec<ProtocolEvent> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        out.push(event);
    }
    out
}

/// The sink half. `ChannelSink` IS a host connection — an in-process one with
/// a transcript the user is looking at — so it must say so.
#[test]
fn the_tui_channel_sink_advertises_a_render_surface() {
    let (sink, _rx) = channel();
    assert!(
        sink.render_artifact_supported(),
        "the TUI transcript is a render surface; the sink that feeds it must say so"
    );
}

/// The composed liveness gate the TOOL actually reads. Asserting the override
/// alone would pass even if `ProtocolRenderSink` never consulted it.
#[test]
fn the_render_sink_over_a_channel_sink_is_live() {
    let (sink, _rx) = channel();
    let render_sink = ProtocolRenderSink::new(sink);
    assert!(
        wcore_tools::render::RenderSink::is_live(&render_sink),
        "a render sink bound to the TUI must report live, or the tool refuses every call"
    );
}

/// End to end, through every production seam: tool -> ProtocolRenderSink ->
/// ChannelSink -> ProtocolEvent -> apply_event -> transcript.
#[tokio::test]
async fn a_rendered_artifact_reaches_the_tui_transcript() {
    let (sink, mut rx) = channel();
    let tool = RenderArtifactTool::new(Arc::new(ProtocolRenderSink::new(sink)));
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(
        "call-1138",
        CancellationToken::new(),
        Arc::new(SandboxedFs::new(RealFs, workspace.path())),
        None,
        Arc::new(wcore_tools::NullToolOutputSink),
    );

    let result = tool
        .execute_with_ctx(
            json!({"title": "Findings", "content": "# Findings\n\nthe needle\n"}),
            &ctx,
        )
        .await;
    assert!(
        !result.is_error,
        "the TUI has a display, so the tool must not refuse: {}",
        result.content
    );

    let events = drain(&mut rx);
    let frame = events
        .iter()
        .find(|event| matches!(event, ProtocolEvent::RenderArtifact { .. }))
        .cloned()
        .unwrap_or_else(|| {
            panic!("ChannelSink must forward the artifact; got {events:?}")
        });
    match &frame {
        ProtocolEvent::RenderArtifact {
            title,
            mime,
            content,
            truncated,
            ..
        } => {
            assert_eq!(title, "Findings");
            assert_eq!(*mime, RenderMime::Markdown);
            assert!(content.contains("the needle"));
            assert!(!truncated);
        }
        other => panic!("expected RenderArtifact, got {other:?}"),
    }

    let mut app = App::new();
    apply_event(&mut app, frame);
    let rendered: String = app
        .session
        .turns
        .iter()
        .flat_map(|turn| turn.elements.iter())
        .filter_map(|element| match element {
            TurnElement::Markdown(text) => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("the needle"),
        "the artifact must appear in the transcript, not be discarded: {rendered:?}"
    );
    assert!(
        rendered.contains("Findings"),
        "the title labels the surface: {rendered:?}"
    );
}

/// The cap is the sink's job, not the caller's — the same chokepoint rule
/// `ProtocolSink::emit_render_artifact` documents. A TUI that inherited the
/// emit but not the cap would push an unbounded blob into a scrollback.
#[test]
fn the_channel_sink_applies_the_content_cap() {
    let (sink, mut rx) = channel();
    let oversized = "x".repeat(wcore_protocol::events::RENDER_ARTIFACT_CONTENT_LIMIT_BYTES + 4096);
    sink.emit_render_artifact("call-cap", "Big", RenderMime::Plain, &oversized);
    let events = drain(&mut rx);
    match events.first() {
        Some(ProtocolEvent::RenderArtifact {
            content, truncated, ..
        }) => {
            assert!(*truncated, "an over-cap render must be marked truncated");
            assert!(
                content.len() < oversized.len(),
                "the cap must actually cut: {} vs {}",
                content.len(),
                oversized.len()
            );
        }
        other => panic!("expected a RenderArtifact frame, got {other:?}"),
    }
}

/// Plain and HTML must not be re-interpreted as markdown on the way to the
/// terminal: the literal bytes are what the model asked to show.
#[test]
fn plain_text_is_shown_literally() {
    let mut app = App::new();
    apply_event(
        &mut app,
        ProtocolEvent::RenderArtifact {
            msg_id: String::new(),
            call_id: "call-plain".into(),
            title: "Raw".into(),
            mime: RenderMime::Plain,
            content: "# not a heading\n* not a bullet\n".into(),
            truncated: false,
            critical: wcore_protocol::events::NonCritical,
        },
    );
    let rendered: String = app
        .session
        .turns
        .iter()
        .flat_map(|turn| turn.elements.iter())
        .filter_map(|element| match element {
            TurnElement::Markdown(text) => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("# not a heading"),
        "plain text must survive verbatim: {rendered:?}"
    );
}
