//! Per-tool result formatters.
//!
//! Today's tool output is rendered as a raw JSON dump on the tool card.
//! This module replaces that with a `ToolResultFormatter` trait + a
//! per-tool implementation, plus a `formatter_for(name)` dispatcher that
//! is **Total** — unknown tool names fall through to the `generic`
//! pretty-printer so the dispatcher never panics or returns `None`.
//!
//! Wave 2 C2/C3:
//! - `summary_line` — a one-line compact summary shown on the tool card.
//! - `detail_lines` — a multi-line breakdown shown when the card is
//!   expanded (Ctrl+E, wired in W3).
//! - `extract_urls` — feeds the Sources block (W3 D4).
//!
//! Formatters are zero-sized unit structs held behind `&'static dyn`
//! references; the dispatcher returns a static singleton so callers
//! never own one.
//!
//! Each per-tool file documents its expected payload shape — those
//! shapes are read from the actual `*ToolDef` outputs in W1, and the
//! formatters degrade gracefully (missing fields collapse to `?` or are
//! omitted) so a payload-schema drift in a future wcore version cannot
//! crash the TUI.

use std::time::Duration;

use ratatui::text::Line;
use serde_json::Value;

use super::theme::Theme;

pub mod bash;
pub mod discord;
pub mod file_ops;
pub mod generic;
pub mod github;
pub mod homeassistant;
pub mod image_gen;
pub mod transcribe;
pub mod tts;
pub mod vision;
pub mod web;
pub mod web_fetch;

/// UAT-T3: the regression suite that drives the REAL tools instead of
/// hand-built payloads. See its module docs for why the previous suite could
/// not fail.
#[cfg(test)]
mod real_payload_tests;

/// Render one tool's JSON result payload into UI lines.
///
/// All three methods read from `serde_json::Value` and are tolerant of
/// missing/typed-wrong fields — the formatters never panic on a
/// malformed payload; they degrade to placeholders.
pub trait ToolResultFormatter: Send + Sync + 'static {
    /// Single-line summary shown on the compact tool card.
    ///
    /// Example: `Found 8 results in 2.3s` or `Posted to #general · 42 chars`.
    fn summary_line(&self, payload: &Value, duration: Duration) -> String;

    /// Multi-line detail shown when the card is expanded (W3 Ctrl+E).
    ///
    /// The default `generic` implementation pretty-prints the JSON
    /// truncated to 30 lines; per-tool formatters override with a
    /// richer layout (e.g. numbered web results, command stdout).
    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>>;

    /// URLs extracted from the payload for the Sources block (W3 D4).
    /// Default empty — only `web`/`web_fetch`/`github`/`image_gen`
    /// override.
    fn extract_urls(&self, payload: &Value) -> Vec<String> {
        let _ = payload;
        Vec::new()
    }

    /// Friendly one-line summary of the tool's REQUEST args (not its
    /// result payload). v0.9.1.1 B4-hunt: the approval card + activity
    /// rail render this instead of the raw JSON dump that leaked into
    /// the inline approval prompt. Returns `None` to fall back to the
    /// generic compact-JSON path. Per-tool formatters override with a
    /// human-readable preview keyed on the args shape (e.g. `tts_speak`
    /// args of `{"text": "..."}` render as a quoted excerpt).
    fn format_args(&self, args: &Value) -> Option<String> {
        let _ = args;
        None
    }
}

/// Resolve a formatter by tool name.
///
/// Returns the generic fallback for unknown tools so the dispatcher is
/// **Total** — callers never need to handle a `None` case and the TUI
/// can always render *something* for any tool the engine fires.
///
/// **Case + alias normalization (v0.9.1.1 B3 fix):** the engine emits
/// tool names in mixed shapes — `"Bash"`, `"Read"`, `"Write"`, `"Edit"`
/// from the file/shell family; `"WebFetch"` from the web fetcher;
/// `"vision_analyze"` / `"image_generate"` / `"text_to_speech"` /
/// `"transcribe_audio"` / `"github_api"` / `"discord_server"` from the
/// rest. The dispatcher lowercases the input once and matches against
/// the canonical-lowercase form, plus accepts common short aliases so
/// existing call sites that pass the formatter's documented key (e.g.
/// `"web_fetch"`) keep working.
pub fn formatter_for(tool_name: &str) -> &'static dyn ToolResultFormatter {
    // Normalize: lowercase + collapse runs of whitespace to underscore.
    // Tool names from the engine are stable identifiers (no spaces in
    // practice) but the lowercase pass alone closes the case mismatch
    // that left every `Bash`/`Read`/`Write`/`Edit`/`WebFetch` falling
    // through to the generic fallback in v0.9.1.
    let key: String = tool_name
        .trim()
        .chars()
        .map(|c| {
            if c.is_whitespace() {
                '_'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();

    match key.as_str() {
        // Web family.
        "web" | "web_search" => &web::WebFormatter,
        "webfetch" | "web_fetch" | "fetch" => &web_fetch::WebFetchFormatter,
        // Multimodal.
        "vision" | "vision_analyze" | "analyze_image" => &vision::VisionFormatter,
        "transcribe" | "transcribe_audio" | "stt" => &transcribe::TranscribeFormatter,
        "image_gen" | "image_generate" | "generate_image" => &image_gen::ImageGenFormatter,
        "tts" | "text_to_speech" | "speak" => &tts::TtsFormatter,
        // Shell.
        "bash" | "shell" | "sh" => &bash::BashFormatter,
        // File ops — engine fires `Read`/`Write`/`Edit` separately; the
        // C3 spec uses `file_ops` as the umbrella key.
        "file_ops" | "read" | "write" | "edit" => &file_ops::FileOpsFormatter,
        // Integrations.
        "github" | "github_api" => &github::GithubFormatter,
        "discord" | "discord_server" => &discord::DiscordFormatter,
        "homeassistant" | "home_assistant" | "ha" => &homeassistant::HomeAssistantFormatter,
        _ => &generic::GenericFormatter,
    }
}

// ── shared helpers (in-module crate-private) ───────────────────────────
//
// Small utilities the per-tool formatters reuse. Kept terse: every
// helper has exactly one caller-shape it serves.

/// Render a Duration as `N.Ns` with one decimal place.
pub(crate) fn fmt_duration(d: Duration) -> String {
    format!("{:.1}s", d.as_secs_f64())
}

/// Read `payload[key]` as a string, returning `default` if absent or
/// not a string. Used everywhere — kept as a small helper so the
/// per-tool files stay readable.
pub(crate) fn str_or<'a>(payload: &'a Value, key: &str, default: &'a str) -> &'a str {
    payload.get(key).and_then(Value::as_str).unwrap_or(default)
}

/// Read `payload[key]` as a u64, returning `default` if absent or not
/// numeric.
pub(crate) fn u64_or(payload: &Value, key: &str, default: u64) -> u64 {
    payload.get(key).and_then(Value::as_u64).unwrap_or(default)
}

/// Read `payload[key]` as an i64, returning `default` if absent or not
/// numeric.
pub(crate) fn i64_or(payload: &Value, key: &str, default: i64) -> i64 {
    payload.get(key).and_then(Value::as_i64).unwrap_or(default)
}

// ── UAT-T3: making "unknown" expressible ───────────────────────────────────
//
// The helpers above take a `default`, and every caller passed a *plausible
// looking* one — `"?"` for a command, `0` for an exit code, `0` for a byte
// count. When the payload did not carry the key (which, measured across all
// 12 formatters, was the normal case and not the exception) the card rendered
// those defaults as though they were facts. A shell command that failed with
// exit 1 was reported as `exit 0`.
//
// A default is only safe when it is *true* for a missing field. `0` is not a
// true exit code and `"?"` is not a true command. The `opt_*` readers below
// return `None` instead, so a formatter has to decide what to do about not
// knowing — and `join_facts` makes omission the easy path.
//
// Rule for every formatter in this module: **never render a value you did not
// read.** If the field is absent, leave it out.

/// Read `payload[key]` as a string, or `None` when absent / not a string.
pub(crate) fn opt_str<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Read `payload[key]` as a u64, or `None` when absent / not numeric.
pub(crate) fn opt_u64(payload: &Value, key: &str) -> Option<u64> {
    payload.get(key).and_then(Value::as_u64)
}

/// The payload's raw text, when the tool did not emit JSON at all.
///
/// `parse_payload` (widget) / `parse_card_payload` (inline transcript) fall
/// back to `Value::String(raw)` when `serde_json::from_str` fails. Several
/// tools — `Bash`, `Read`, `Write`, `Edit` — return plain prose, so that
/// fallback is their normal path, not an error path. Formatters read the text
/// through here instead of calling `.get(…)` and silently getting `None` for
/// everything.
/// Returns the text **verbatim**, not trimmed.
///
/// It did trim, briefly, and that was a real bug caught by the real-payload
/// suite: `BashTool`'s result ends with the `\nSTDERR:\n` marker, so trimming
/// the trailing newline destroyed the marker, the stdout/stderr split failed,
/// and the byte count came back as 23 instead of 15. The emptiness check
/// still uses `trim`, because a whitespace-only payload carries no
/// information — but what is handed to the caller is exactly what the tool
/// produced.
pub(crate) fn raw_text(payload: &Value) -> Option<&str> {
    payload.as_str().filter(|s| !s.trim().is_empty())
}

/// Join the facts a formatter actually established into one summary line.
///
/// Empty entries are dropped, so a formatter can push `None`-derived pieces
/// without special-casing each one, and an absent field costs a missing
/// clause rather than a fabricated value. Returns an empty string when
/// nothing at all is known — callers surface that as "no summary", and the
/// inline renderer's `is_filler` check already drops an empty body line.
pub(crate) fn join_facts(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A one-line, width-clamped preview of arbitrary text.
///
/// Used when a tool's payload is prose the formatter cannot decompose: show
/// the tool's *real* first line rather than inventing structured fields. The
/// clamp is by `char`, not by byte, so multi-byte output cannot be split
/// mid-codepoint.
pub(crate) fn first_line_preview(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let trimmed = line.trim();
    let count = trimmed.chars().count();
    if count <= max {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formatter_for_unknown_returns_generic() {
        // The dispatcher must never panic — unknown tools fall through
        // to the generic formatter. Verify by feeding a payload whose
        // shape the generic formatter handles (no domain fields) and
        // confirming the summary matches the generic's idiom.
        let f = formatter_for("nonexistent_tool_name_zzz");
        let payload = json!({ "status": "completed" });
        let summary = f.summary_line(&payload, Duration::from_millis(1200));
        // The generic formatter returns the first non-empty top-level
        // key + its value if string, else "completed in 1.2s". `status`
        // is a string so the summary should embed "completed".
        assert!(
            summary.contains("completed") || summary.contains("status"),
            "generic fallback summary was: {summary}"
        );
    }

    #[test]
    fn known_tool_names_dispatch_to_specific_formatter() {
        // The dispatcher table must wire every claimed tool. We don't
        // know the type identity at runtime (trait objects), but we can
        // confirm the summary diverges from the generic fallback on a
        // payload designed to trip the per-tool format string.
        let f = formatter_for("web");
        // Two rows, so the assertion below is not coupled to whether the
        // summary pluralises. (It now does: a one-row payload reads
        // "Found 1 result", which broke this case when it asserted the
        // literal substring "results".)
        let payload = json!({
            "results": [
                { "title": "A", "url": "https://example.com", "domain": "example.com", "snippet": "s" },
                { "title": "B", "url": "https://example.org", "domain": "example.org", "snippet": "t" }
            ]
        });
        let summary = f.summary_line(&payload, Duration::from_secs_f64(2.3));
        // Web formatter's idiom: "Found N results in X.Xs". Generic
        // would say nothing of the sort.
        assert!(summary.contains("Found"), "web summary was: {summary}");
        assert!(summary.contains("2 results"), "web summary was: {summary}");
    }

    /// v0.9.1.1 B3: regression — every actual backend tool name the
    /// engine emits must dispatch to a non-fallback formatter. Before
    /// B3 the dispatcher keys were lowercase/snake_case stubs while the
    /// engine fires mixed-case names like `Bash`, `Read`, `WebFetch`
    /// and snake_cased names like `vision_analyze`, `text_to_speech`,
    /// `image_generate`, `github_api`, `discord_server`. Only `web` and
    /// `homeassistant` accidentally matched. This test pins the full
    /// set so a future tool rename / new tool addition forces an
    /// explicit dispatcher update.
    #[test]
    fn formatter_dispatcher_matches_all_backend_tool_names_v0911() {
        // (backend tool name, payload that distinguishes specific from generic)
        let bash_payload = json!({ "stdout": "ok", "exit_code": 0 });
        let read_payload = json!({ "path": "/x", "content": "hello" });
        let web_payload = json!({
            "results": [
                { "title": "A", "url": "https://example.com", "domain": "example.com", "snippet": "s" }
            ]
        });
        let web_fetch_payload = json!({ "url": "https://example.com", "status": 200 });
        let vision_payload = json!({ "description": "a cat" });
        let stt_payload = json!({ "text": "hello world" });
        let img_payload = json!({ "image_url": "https://x/y.png" });
        let tts_payload = json!({ "audio_url": "https://x/y.mp3" });
        let gh_payload = json!({ "html_url": "https://github.com/a/b/pull/1" });
        let discord_payload = json!({ "channel": "general" });
        let ha_payload = json!({ "entity": "light.kitchen", "state": "on" });

        let cases: &[(&str, &Value)] = &[
            // Shell family.
            ("Bash", &bash_payload),
            ("bash", &bash_payload),
            // File ops.
            ("Read", &read_payload),
            ("Write", &read_payload),
            ("Edit", &read_payload),
            // Web.
            ("web", &web_payload),
            ("WebFetch", &web_fetch_payload),
            ("web_fetch", &web_fetch_payload),
            // Multimodal.
            ("vision_analyze", &vision_payload),
            ("transcribe_audio", &stt_payload),
            ("image_generate", &img_payload),
            ("text_to_speech", &tts_payload),
            // Integrations.
            ("github_api", &gh_payload),
            ("discord_server", &discord_payload),
            ("homeassistant", &ha_payload),
        ];

        // The generic fallback's idiom is sufficiently distinct: it
        // never prints any of these tool-specific tokens. We verify
        // dispatch by comparing the resolved summary against the
        // generic formatter's summary on the same payload — they
        // must differ for every case above.
        let generic = formatter_for("nonexistent_tool_name_zzz");
        for (name, payload) in cases {
            let f = formatter_for(name);
            let specific = f.summary_line(payload, Duration::ZERO);
            let fallback = generic.summary_line(payload, Duration::ZERO);
            assert_ne!(
                specific, fallback,
                "tool `{name}` fell through to the generic fallback (summary={specific:?}); \
                 B3 dispatcher table missed this name"
            );
        }
    }

    /// v0.9.1.1 B3: case-insensitive dispatch. `Bash` / `bash` / `BASH`
    /// all dispatch to the same formatter so a future engine rename of
    /// the bash tool to either case keeps working.
    #[test]
    fn formatter_dispatcher_case_insensitive_v0911() {
        let payload = json!({ "stdout": "hello", "exit_code": 0 });
        let s_lower = formatter_for("bash").summary_line(&payload, Duration::ZERO);
        let s_upper = formatter_for("BASH").summary_line(&payload, Duration::ZERO);
        let s_pascal = formatter_for("Bash").summary_line(&payload, Duration::ZERO);
        assert_eq!(s_lower, s_upper, "BASH should match bash");
        assert_eq!(s_lower, s_pascal, "Bash should match bash");

        // WebFetch family — the one the v0.9.1 dispatcher missed.
        let p = json!({ "url": "https://example.com", "status": 200 });
        let a = formatter_for("WebFetch").summary_line(&p, Duration::ZERO);
        let b = formatter_for("webfetch").summary_line(&p, Duration::ZERO);
        let c = formatter_for("web_fetch").summary_line(&p, Duration::ZERO);
        assert_eq!(a, b);
        assert_eq!(a, c);
    }
}
