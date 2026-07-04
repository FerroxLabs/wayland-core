//! Boot-time "unavailable capabilities and why" advisory (#660).
//!
//! Optional capability tools (vision, image generation, transcription, TTS,
//! video, Discord, …) hide themselves from the call schema when their backend
//! is unconfigured — `Tool::is_available() == false` at registration, so the
//! model never sees the tool. The drop is silent: asked to generate an image
//! with no key set, the model fabricates a cause ("I don't have image
//! generation") instead of the honest, actionable reason ("set OPENAI_API_KEY").
//!
//! This module surfaces the truth into the system prompt. Availability is read
//! straight from the populated [`ToolRegistry`] (the single source of truth —
//! the backend resolver already decided), and each absent capability contributes
//! a static, human-readable hint naming the env var(s) that would enable it.
//! When every capability is available the advisory is `None`, so a fully
//! configured session's prompt is byte-identical to before.

use wcore_tools::registry::ToolRegistry;

/// One optional capability: the tool `name()` that exposes it and the honest
/// hint naming the configuration that would enable it.
struct Capability {
    /// Human-facing capability label used in the advisory line.
    label: &'static str,
    /// The tool's `name()` — present in the registry iff the capability is
    /// available this session.
    tool: &'static str,
    /// What the user must configure to enable it. Env-var names verified
    /// against each backend resolver.
    hint: &'static str,
}

/// The env-gated capabilities whose absence is otherwise invisible to the model.
/// Env-var names mirror the resolvers in `crate::tool_backends` (`image_gen`,
/// `tts`, `video_analyze`, `discord`, and `build_vision_backend` /
/// `build_transcription_backend` in `tool_backends/mod.rs`).
const OPTIONAL_CAPABILITIES: &[Capability] = &[
    Capability {
        label: "Image generation",
        tool: "image_generate",
        hint: "set OPENAI_API_KEY, FAL_API_KEY, or GEMINI_API_KEY",
    },
    Capability {
        label: "Image understanding (vision)",
        tool: "vision_analyze",
        hint: "set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY",
    },
    Capability {
        label: "Audio transcription",
        tool: "transcribe_audio",
        hint: "set GROQ_API_KEY or OPENAI_API_KEY",
    },
    Capability {
        label: "Text-to-speech",
        tool: "text_to_speech",
        hint: "set OPENAI_API_KEY or ELEVENLABS_API_KEY",
    },
    Capability {
        label: "Video analysis",
        tool: "video_analyze",
        hint: "set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY (and install ffmpeg)",
    },
    Capability {
        label: "Discord",
        tool: "discord_server",
        hint: "set DISCORD_BOT_TOKEN",
    },
];

/// Render the "unavailable capabilities" advisory for appending to the system
/// prompt, given the fully-populated tool registry.
///
/// Returns `None` when every optional capability is available, keeping the
/// prompt unchanged for fully-configured sessions.
pub fn render_capability_advisory(registry: &ToolRegistry) -> Option<String> {
    render_from_names(&registry.tool_names())
}

/// Testable core: build the advisory from a set of registered tool names.
fn render_from_names(registered: &[String]) -> Option<String> {
    let missing: Vec<&Capability> = OPTIONAL_CAPABILITIES
        .iter()
        .filter(|c| !registered.iter().any(|n| n == c.tool))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push_str("\n\n# Unavailable capabilities\n");
    out.push_str(
        "The capabilities below are NOT available in this session because their backend \
         is not configured. If the user asks for one, do NOT claim the ability does not \
         exist or invent another reason — tell them exactly what to configure:\n",
    );
    for c in missing {
        out.push_str(&format!("- {} — unavailable: {}\n", c.label, c.hint));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_capability_tools() -> Vec<String> {
        OPTIONAL_CAPABILITIES
            .iter()
            .map(|c| c.tool.to_string())
            .collect()
    }

    #[test]
    fn none_when_every_capability_available() {
        // A fully-configured session (all capability tools registered) must
        // produce no advisory, keeping the prompt byte-identical to before.
        assert!(render_from_names(&all_capability_tools()).is_none());
    }

    #[test]
    fn lists_only_the_missing_capabilities_with_hints() {
        // Only vision is configured; every other capability must be named as
        // unavailable, each with its honest env-var hint.
        let registered = vec!["vision_analyze".to_string(), "read".to_string()];
        let advisory = render_from_names(&registered).expect("advisory when capabilities missing");
        assert!(advisory.contains("# Unavailable capabilities"));
        // Vision is present → must NOT be listed as unavailable.
        assert!(
            !advisory.contains("Image understanding"),
            "configured capability must not appear: {advisory}"
        );
        // Missing ones are named with their fix.
        assert!(advisory.contains("Image generation"));
        assert!(advisory.contains("set OPENAI_API_KEY, FAL_API_KEY, or GEMINI_API_KEY"));
        assert!(advisory.contains("Text-to-speech"));
        assert!(advisory.contains("set DISCORD_BOT_TOKEN"));
    }

    #[test]
    fn honest_instruction_forbids_fabricating_a_cause() {
        // The advisory must instruct the model to name the fix, not fabricate.
        let advisory = render_from_names(&[]).expect("advisory when nothing registered");
        assert!(advisory.contains("do NOT claim the ability does not exist"));
    }
}
