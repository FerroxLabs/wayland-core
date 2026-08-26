p = "crates/wcore-cli/src/tui/engine_bridge.rs"
s = open(p).read()
anchor = """        self.send(ProtocolEvent::PluginRegistrationFailed {
            plugin_name: plugin_name.to_string(),
            surface: surface.to_string(),
            error_kind: error_kind.to_string(),
            message: message.to_string(),
        });
    }
}
"""
new = """        self.send(ProtocolEvent::PluginRegistrationFailed {
            plugin_name: plugin_name.to_string(),
            surface: surface.to_string(),
            error_kind: error_kind.to_string(),
            message: message.to_string(),
        });
    }

    /// FerroxLabs/wayland#1138. The TUI transcript IS a render surface - an
    /// in-process one, but one a user is looking at - so this sink claims it.
    ///
    /// Both `OutputSink` methods below are DEFAULTED, and inheriting the
    /// defaults is exactly what #1138 reports: the default
    /// `render_artifact_supported` returns false, so `ProtocolRenderSink::is_live`
    /// said no and every `render_artifact` call under the TUI was refused; had
    /// it said yes, the default `emit_render_artifact` would then have
    /// discarded the content silently. A trait default that swallows is a trap,
    /// and the warning comment on the trait did not stop this implementor
    /// walking into it - so the guard is `tests/render_artifact_tui_surface.rs`,
    /// which drives the real tool over this sink and through the real bridge.
    fn render_artifact_supported(&self) -> bool {
        true
    }

    /// The TUI's half of the truncation chokepoint. The cap is applied HERE,
    /// not by the caller, for the same reason `ProtocolSink` applies it in its
    /// own override: one place, and no emitter able to route around it.
    ///
    /// Redaction runs BEFORE truncation and the order is load-bearing - the
    /// scrub matches WHOLE tokens, so cutting first can leave the prefix of a
    /// straddling `apr-<uuid>` in the frame, where no whole-token scrub can
    /// ever match it again. Same argument, same order, as `ProtocolSink`.
    ///
    /// `msg_id` is left empty: unlike `ProtocolSink` this sink holds no
    /// current-turn handle, and the bridge renders the artifact as its own
    /// transcript entry rather than correlating it into a turn.
    fn emit_render_artifact(
        &self,
        call_id: &str,
        title: &str,
        mime: wcore_protocol::events::RenderMime,
        content: &str,
    ) {
        let redacted = wcore_agent::output_redaction::redact_active_tokens(content);
        let (content, truncated) = wcore_protocol::events::truncate_render_content(&redacted);
        self.send(ProtocolEvent::RenderArtifact {
            msg_id: String::new(),
            call_id: call_id.to_string(),
            title: wcore_protocol::events::truncate_render_title(
                &wcore_agent::output_redaction::redact_active_tokens(title),
            ),
            mime,
            content,
            truncated,
            critical: wcore_protocol::events::NonCritical,
        });
    }
}
"""
assert s.count(anchor) == 1, s.count(anchor)
open(p, "w").write(s.replace(anchor, new))
print("engine_bridge ok")
