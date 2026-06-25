//! Reversible tool-name codec for the OpenAI wire format.
//!
//! OpenAI (and every OpenAI-compatible endpoint: ChatGPT subscription, Groq,
//! DeepSeek, Together, Sakana, …) constrains tool/function names to
//! `^[a-zA-Z0-9_-]+$` and returns `400 invalid_value` on
//! `tools[N].function.name` otherwise. WCore tool ids routinely contain `:`,
//! `::`, and `.` — e.g. `Browser::execute`, `tool:brave`,
//! `ai.perplexity-perplexity-mcp`, `com.microsoft-markitdown` — so the raw name
//! is rejected (FerroxLabs/wayland#297).
//!
//! Sanitizing outbound alone is not enough: the model then calls back with the
//! sanitized spelling and the engine has no tool by that name. The fix must
//! round-trip — encode every name we serialize OUTBOUND (tool definitions AND
//! assistant-history `tool_calls`) and decode every name we parse INBOUND
//! (streamed `tool_call`/`function_call`) back to the canonical id before the
//! provider emits [`LlmEvent::ToolUse`](wcore_types::llm::LlmEvent). The engine
//! and everything downstream keep seeing canonical ids unchanged.
//!
//! The codec is **stateless and reversible** — no per-request registry, so it
//! is trivially correct across the shared `Arc<OpenAIProvider>` serving
//! concurrent streams and across both the chat and Responses SSE paths.
//!
//! Encoding is conditional on a [`SENTINEL`] prefix so the common case is a
//! no-op:
//! * A name that already matches `^[a-zA-Z0-9_-]+$` **and** does not start with
//!   the sentinel is emitted unchanged. Normal snake_case OpenAI tools
//!   (`get_weather`, `Read`, `Bash`) are never touched.
//! * Any other name is emitted as `SENTINEL + hex`, where every byte that is
//!   not `[A-Za-z0-9-]` (this includes `_`, so the escape marker is
//!   unambiguous, and any multi-byte UTF-8) is written as `_HH` (uppercase
//!   hex). The encoder also wraps the rare real name that itself starts with
//!   the sentinel, so a leading sentinel on the wire ALWAYS denotes an encoded
//!   name — decode never misfires on a genuine tool id.

/// Marker prefix identifying an encoded name on the wire. Valid under the
/// OpenAI name charset. Chosen to be an improbable real tool-name prefix; the
/// encoder self-guards any name that nonetheless starts with it, so collision
/// probability affects only how often a name is wrapped, never correctness.
const SENTINEL: &str = "wct_";

/// True when `name` is safe to put on the wire verbatim: non-empty, every byte
/// in `[A-Za-z0-9_-]`, and not masquerading as an encoded name.
fn is_plain(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(SENTINEL)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Encode a canonical WCore tool name into an OpenAI-wire-legal function name.
/// A no-op for names that are already legal and unambiguous (see module docs).
pub(crate) fn encode_tool_name(name: &str) -> String {
    if is_plain(name) {
        return name.to_string();
    }
    let mut out = String::with_capacity(SENTINEL.len() + name.len() * 3);
    out.push_str(SENTINEL);
    for &b in name.as_bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' {
            out.push(b as char);
        } else {
            // Escape '_' and every other non-charset byte (incl. ':', '.', and
            // multi-byte UTF-8) so '_' unambiguously introduces an escape.
            out.push('_');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// Decode a wire function name back to the canonical WCore tool id. A no-op for
/// any name without the sentinel prefix (i.e. names emitted verbatim, or a
/// model-hallucinated name we never sent — which then surfaces as a normal
/// unknown-tool error downstream).
pub(crate) fn decode_tool_name(name: &str) -> String {
    let Some(body) = name.strip_prefix(SENTINEL) else {
        return name.to_string();
    };
    let bytes = body.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'_' {
            // Expect exactly two hex digits. If malformed (truncated stream or
            // a hallucinated name), keep the byte literally — best effort.
            if i + 2 < bytes.len()
                && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
            {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names already legal under `^[a-zA-Z0-9_-]+$` must pass through untouched
    /// — zero blast radius for normal OpenAI snake_case / CamelCase tools.
    #[test]
    fn plain_names_are_unchanged() {
        for n in ["Read", "Bash", "get_weather", "web-search", "Tool123", "a"] {
            assert_eq!(encode_tool_name(n), n, "encode changed plain name {n}");
            assert_eq!(decode_tool_name(n), n, "decode changed plain name {n}");
        }
    }

    /// The exact offenders from the bug report round-trip to canonical ids and
    /// the encoded form is OpenAI-wire-legal.
    #[test]
    fn reporter_bad_names_round_trip() {
        let bad = [
            "Browser::execute",
            "tool:brave",
            "tool:tavily",
            "ai.perplexity-perplexity-mcp",
            "com.microsoft-markitdown",
            "org.wikipedia-wikipedia-mcp",
        ];
        for n in bad {
            let enc = encode_tool_name(n);
            assert!(
                enc != n && enc.starts_with(SENTINEL),
                "{n} should be wrapped, got {enc}"
            );
            assert!(is_wire_legal(&enc), "encoded {enc} is not wire-legal");
            assert_eq!(decode_tool_name(&enc), n, "round-trip failed for {n}");
        }
    }

    /// A real name that itself starts with the sentinel is wrapped (not emitted
    /// verbatim) so a leading sentinel on the wire always denotes an encoded
    /// name — decode is never ambiguous.
    #[test]
    fn names_starting_with_sentinel_are_guarded() {
        for n in ["wct_foo", "wct_", "wct_a_b"] {
            let enc = encode_tool_name(n);
            assert!(is_wire_legal(&enc));
            assert_eq!(decode_tool_name(&enc), n, "guard round-trip failed for {n}");
        }
    }

    /// Underscores in the original survive the escape (encoded only inside a
    /// wrapped name; a plain underscore name is untouched).
    #[test]
    fn underscores_round_trip_inside_wrapped_names() {
        // Plain underscore name: untouched.
        assert_eq!(encode_tool_name("a_b"), "a_b");
        // Wrapped because of the dot; the underscore must still survive.
        let n = "a.b_c";
        assert_eq!(decode_tool_name(&encode_tool_name(n)), n);
    }

    /// Arbitrary unicode round-trips byte-exact.
    #[test]
    fn unicode_round_trips() {
        let n = "tool:café→x";
        let enc = encode_tool_name(n);
        assert!(is_wire_legal(&enc));
        assert_eq!(decode_tool_name(&enc), n);
    }

    /// A non-sentinel name the model "invents" is left alone (becomes a normal
    /// unknown-tool error upstream, not corrupted by decode).
    #[test]
    fn decode_passes_through_unknown_plain_names() {
        assert_eq!(decode_tool_name("Hallucinated_Tool"), "Hallucinated_Tool");
    }

    fn is_wire_legal(s: &str) -> bool {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }
}
