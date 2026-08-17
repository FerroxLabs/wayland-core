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
        // F27-C3. `FLUX_API_KEY` leads because the resolver's FIRST and
        // highest-priority arm is `dalle_backend_from_config` — an active
        // OpenAI-wire provider (FluxRouter or OpenAI) resolved from config,
        // which is not a `read_env_key` call at all.
        //
        // MEASURED 2026-07-29 on `hetzner-dsm`, not read off the source: a
        // session with ONLY `FLUX_API_KEY` set booted with
        // `image_gen: using gpt-image-1 ... (active OpenAI-wire provider)`,
        // i.e. the tool registered through an arm this hint did not name.
        // A user with a Flux key who hit the old advisory was told to set one
        // of four keys, none of which was the one they already had — the same
        // defect family as the `[browser]` / `[browser.policy]` remediation
        // that sent every user in a circle. Every OTHER media hint here
        // already names the OpenAI-wire arm; this was the outlier.
        hint: "set FLUX_API_KEY, OPENAI_API_KEY, FAL_API_KEY, GEMINI_API_KEY, or HF_API_KEY \
               (or configure an OpenAI-wire provider)",
    },
    Capability {
        label: "Image understanding (vision)",
        tool: "vision_analyze",
        hint: "set ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY, or FLUX_API_KEY \
               (or configure an OpenAI-wire provider)",
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
        hint: "set ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY, or FLUX_API_KEY \
               (or configure an OpenAI-wire provider), and install ffmpeg",
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
    // No clause here may forbid the model from reporting a cause. The W2/W3
    // sandbox gate measured that shape suppressing a TRUE cause elsewhere in
    // the product, so the instruction is written positively: name the reason
    // below, which is the accurate one for an absent capability tool.
    out.push_str(
        "The capabilities below are NOT available in this session because their backend \
         is not configured. If the user asks for one, tell them exactly what to configure \
         — that is the real reason, and it is the actionable one:\n",
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
        // F27-C3: the image-generation hint must name the config-provider arm
        // AND the credential that drives it. Measured live: a session with
        // only FLUX_API_KEY set registers the tool through that arm, so a hint
        // that omits it is wrong in the one configuration this product ships.
        assert!(
            advisory.contains(
                "set FLUX_API_KEY, OPENAI_API_KEY, FAL_API_KEY, GEMINI_API_KEY, or HF_API_KEY"
            ),
            "image-gen hint must lead with the config-arm credential: {advisory}"
        );
        assert!(
            advisory.contains("or configure an OpenAI-wire provider"),
            "image-gen hint must name the config arm in words, since no env-var \
             name expresses it: {advisory}"
        );
        assert!(advisory.contains("Text-to-speech"));
        assert!(advisory.contains("set DISCORD_BOT_TOKEN"));
    }

    #[test]
    fn instruction_names_the_fix_without_forbidding_a_cause() {
        // The advisory must instruct the model to name the fix. It must NOT do
        // it with a clause that forbids reporting a cause: the W2/W3 sandbox
        // gate measured that shape (in `bash/policy.rs`) suppressing the true
        // cause of a failure while a false one was asserted. Same wording, same
        // trap, so the same rule applies here.
        let advisory = render_from_names(&[]).expect("advisory when nothing registered");
        assert!(
            advisory.contains("tell them exactly what to configure"),
            "the advisory must still point at the fix: {advisory}"
        );
        for clause in ["do NOT claim", "do not invent", "invent another reason"] {
            assert!(
                !advisory.contains(clause),
                "no advisory may forbid the model from reporting a cause \
                 (found {clause:?}): {advisory}"
            );
        }
    }

    /// Pull the argument of each `read_env_key("NAME")` call out of resolver
    /// source, in source order (the order the resolver probes providers).
    fn read_env_keys_in(src: &str) -> Vec<String> {
        let mut keys = Vec::new();
        let marker = "read_env_key(\"";
        let mut rest = src;
        while let Some(i) = rest.find(marker) {
            rest = &rest[i + marker.len()..];
            if let Some(end) = rest.find('"') {
                keys.push(rest[..end].to_string());
                rest = &rest[end + 1..];
            } else {
                break;
            }
        }
        keys
    }

    /// The credential that populates `config.api_key` for the resolver's
    /// config-provider arm in the setup this product ships as its flagship.
    const CONFIG_ARM_KEY: &str = "FLUX_API_KEY";

    /// Does this resolver consult a config-resolved provider (rather than only
    /// environment variables)? Such an arm can enable a capability with NO
    /// matching `read_env_key`, which is exactly what the old guard could not
    /// see.
    fn resolver_has_config_arm(src: &str) -> bool {
        src.contains("dalle_backend_from_config") || src.contains("openai_wire_media_base")
    }

    /// Does a hint tell the user that configuring a provider — not just
    /// exporting a key — is a route to the capability?
    fn hint_names_config_arm(hint: &str) -> bool {
        hint.contains("OpenAI-wire provider")
    }

    /// Extract the `*_API_KEY` / `*_TOKEN` env-var names named in a hint string.
    fn env_vars_in(hint: &str) -> Vec<String> {
        hint.split(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .filter(|t| t.ends_with("_API_KEY") || t.ends_with("_TOKEN"))
            .map(|t| t.to_string())
            .collect()
    }

    /// Self-test for the Guard 1b repair (F27-C3), with the three assertions
    /// LANE-BRIEF §6b-ii requires — the third being the one that proves the
    /// repair does anything at all.
    ///
    /// Without assertion 3, this whole self-test would pass unchanged on the
    /// BROKEN guard, because the broken guard's predicate is not exercised
    /// here. Assertion 3 is what demonstrates the old matcher could not have
    /// caught the defect that was actually shipped.
    #[test]
    fn config_arm_guard_catches_what_the_env_var_matcher_could_not() {
        // The exact hint that shipped before this repair.
        const OLD_BROKEN_HINT: &str =
            "set OPENAI_API_KEY, FAL_API_KEY, GEMINI_API_KEY, or HF_API_KEY";
        let shipped_hint = OPTIONAL_CAPABILITIES
            .iter()
            .find(|c| c.tool == "image_generate")
            .map(|c| c.hint)
            .expect("image_generate capability present");

        // 1. Known-positive: the repaired guard accepts the hint we now ship.
        assert!(
            hint_names_config_arm(shipped_hint),
            "repaired guard rejects the shipped hint: {shipped_hint}"
        );

        // 2. Known-negative: the repaired guard REJECTS the hint that shipped
        //    the defect. A guard that accepts everything is not a guard.
        assert!(
            !hint_names_config_arm(OLD_BROKEN_HINT),
            "repaired guard accepts the very hint that omitted the config arm"
        );

        // 3. The old matcher would have MISSED it. `env_vars_in` on the broken
        //    hint yields exactly the resolver's `read_env_key` list, so the
        //    pre-repair equality assertion passed on it — which is precisely
        //    why the defect shipped. This is the assertion that proves the
        //    repair is not decorative.
        assert_eq!(
            env_vars_in(OLD_BROKEN_HINT),
            vec![
                "OPENAI_API_KEY",
                "FAL_API_KEY",
                "GEMINI_API_KEY",
                "HF_API_KEY"
            ],
            "the broken hint must still satisfy the OLD env-var matcher — if it no \
             longer does, this self-test has stopped demonstrating the blind spot"
        );

        // And the detector fires on the real resolver source, not just on
        // strings: the config arm genuinely exists in the file.
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tool_backends/image_gen.rs"),
        )
        .expect("read image_gen resolver source");
        assert!(resolver_has_config_arm(&src));
        assert!(
            !resolver_has_config_arm("fn build() { read_env_key(\"OPENAI_API_KEY\"); }"),
            "the config-arm detector must not fire on an env-only resolver"
        );
    }

    /// Anti-drift: the env-var hints in `OPTIONAL_CAPABILITIES` must stay in
    /// sync with the resolvers in `crate::tool_backends`. This test is the guard
    /// that stops the hint list from silently drifting from the resolvers again
    /// (the image-gen hint has already omitted `HF_API_KEY` once).
    #[test]
    fn advisory_hints_stay_in_sync_with_resolvers() {
        use std::fs;
        use std::path::Path;

        let backends = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tool_backends");

        // Guard 1: the image_generate hint must name exactly the provider keys
        // that `build_image_gen_backend` probes, in the same order.
        let image_gen_src = fs::read_to_string(backends.join("image_gen.rs"))
            .expect("read image_gen resolver source");
        let resolver_keys = read_env_keys_in(&image_gen_src);
        assert_eq!(
            resolver_keys,
            vec![
                "OPENAI_API_KEY",
                "FAL_API_KEY",
                "GEMINI_API_KEY",
                "HF_API_KEY"
            ],
            "image_gen resolver probe order changed — update the image_generate hint to match"
        );
        let image_hint = OPTIONAL_CAPABILITIES
            .iter()
            .find(|c| c.tool == "image_generate")
            .map(|c| c.hint)
            .expect("image_generate capability present");
        // The hint leads with the config-arm credential, then names every
        // `read_env_key` probe in resolver order.
        let mut expected = vec![CONFIG_ARM_KEY.to_string()];
        expected.extend(resolver_keys.iter().cloned());
        assert_eq!(
            env_vars_in(image_hint),
            expected,
            "image_generate hint env vars must match the config arm followed by the \
             resolver's probe order exactly: {image_hint}"
        );

        // Guard 1b — REPAIR (F27-C3). The assertion above compares the hint
        // against `read_env_key` calls only, so it is STRUCTURALLY BLIND to
        // the resolver's first arm, `dalle_backend_from_config`, which reads
        // no env var. It therefore certified a hint that omitted the arm that
        // actually enables the tool in a FluxRouter session — measured live on
        // 2026-07-29. Noting that and moving on would leave the defect in
        // place (LANE-BRIEF §6b-ii), so the instrument is repaired here:
        // whenever the resolver consults a config-provider arm, the hint must
        // say so in words as well as naming a key.
        assert!(
            resolver_has_config_arm(&image_gen_src),
            "the image_gen resolver no longer consults a config provider arm — if that \
             is intended, drop this guard and the OpenAI-wire clause from the hint"
        );
        assert!(
            hint_names_config_arm(image_hint),
            "the image_gen resolver's FIRST arm resolves an OpenAI-wire provider from \
             config, which no env-var name can express. The hint must say so: {image_hint}"
        );

        // Guard 2: every env var named in ANY hint must actually be read by some
        // resolver in tool_backends — no hint may promise a key that configures
        // nothing (catches typos and renamed keys across all capabilities).
        let mut all_src = String::new();
        for entry in fs::read_dir(&backends).expect("read tool_backends dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                all_src.push_str(&fs::read_to_string(&path).expect("read backend source"));
            }
        }
        for cap in OPTIONAL_CAPABILITIES {
            let vars = env_vars_in(cap.hint);
            assert!(
                !vars.is_empty(),
                "{} hint names no env var — expected at least one: {}",
                cap.label,
                cap.hint
            );
            for key in vars {
                assert!(
                    all_src.contains(&format!("\"{key}\"")),
                    "{} hint names {key}, but no tool_backends resolver reads it",
                    cap.label
                );
            }
        }
    }
}
