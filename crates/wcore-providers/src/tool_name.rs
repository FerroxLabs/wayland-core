//! Reversible tool-name codec shared by every provider wire format.
//!
//! Both major provider families reject tool names outside a narrow charset and
//! return HTTP 400 on the *entire* request when a single tool violates it —
//! aborting every tool-calling turn once the profile carries an offending MCP
//! tool:
//! * OpenAI-compatible (ChatGPT subscription, Groq, DeepSeek, Together,
//!   Moonshot/Kimi, Flux Router, Azure, …): `tools[N].function.name` must match
//!   `^[a-zA-Z0-9_-]+$`, **max 64 chars**.
//! * Anthropic (native, Bedrock, Vertex): `tools[N].custom.name` must match
//!   `^[a-zA-Z0-9_-]{1,128}$`.
//!
//! WCore tool ids routinely violate both. MCP tools are named either bare or
//! `mcp__{server}__{tool}` on collision (wcore-mcp `tool_proxy`), where the
//! server id and tool name come straight from the server and may carry `:`,
//! `::`, `.`, spaces or unicode (`Browser::execute`, `tool:brave`,
//! `com.microsoft-markitdown` — FerroxLabs/wayland#297) **and/or** exceed 64
//! chars (`mcp__io-github-taylorwilsdon-google-workspace-mcp__batch_modify_gmail_message_labels`
//! is 83 chars, charset-clean yet rejected by OpenAI's 64-char limit).
//!
//! Sanitizing outbound alone is not enough: the model then calls back with the
//! sanitized spelling and the engine has no tool by that name. The fix must
//! round-trip — encode every name we serialize OUTBOUND (tool definitions AND
//! assistant-history `tool_calls`) and decode every name we parse INBOUND
//! (streamed `tool_call`/`function_call`/`tool_use`) back to the canonical id
//! before the provider emits [`LlmEvent::ToolUse`](wcore_types::llm::LlmEvent).
//! The engine and everything downstream keep seeing canonical ids unchanged.
//!
//! The wire name always matches `^[a-zA-Z0-9_-]{1,64}$` — 64 satisfies BOTH
//! OpenAI's 64 and Anthropic's 128 — so a single encoder serves every path.
//!
//! Two regimes, keyed off length:
//! * **Charset only (fits in 64 chars):** stateless and reversible. A name that
//!   already matches `^[a-zA-Z0-9_-]+$`, is ≤ 64 chars, and does not start with
//!   the sentinel is emitted unchanged (`get_weather`, `Read`, `Bash` are never
//!   touched). Any other in-budget name is emitted as `SENTINEL + hex`, where
//!   every byte not in `[A-Za-z0-9-]` (this includes `_`, so the escape marker
//!   is unambiguous, plus any multi-byte UTF-8) is written as `_HH`. Decode
//!   reverses this purely from the wire string — no state. The encoder wraps
//!   the rare real name that itself starts with the sentinel, so a leading
//!   sentinel on the wire always denotes an encoded name.
//! * **Over-length (escaped form would exceed 64 chars):** truncation is
//!   inherently lossy, so a pure stateless inverse is impossible. The name is
//!   clamped to `SENTINEL + readable_prefix + "-" + hash(original)` (always
//!   ≤ 64, always wire-legal, unique per original via the hash) and the
//!   `clamped → original` mapping is memoised in a process-global table so
//!   decode can route the model's call back to the real tool id. The table is
//!   a deterministic, idempotent memo (same original always yields the same
//!   clamped key mapping to the same value), so it stays correct across the
//!   shared `Arc<…Provider>` serving concurrent streams. `build_tools` runs on
//!   every request, so the table is always warm before the model can call back.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Marker prefix identifying an encoded name on the wire. Valid under the
/// shared name charset. Chosen to be an improbable real tool-name prefix; the
/// encoder self-guards any name that nonetheless starts with it, so collision
/// probability affects only how often a name is wrapped, never correctness.
const SENTINEL: &str = "wct_";

/// Maximum wire length for a tool name. 64 is OpenAI's hard limit and well
/// under Anthropic's 128, so one budget satisfies every provider path.
const MAX_WIRE_LEN: usize = 64;

/// Upper bound on distinct over-length names retained in the reverse-routing
/// memo table. Growth is bounded by tool cardinality (dozens–hundreds); the cap
/// is a defensive guard against a pathological server minting endless distinct
/// long names. Past the cap, encode still emits a valid clamped wire name;
/// only reverse routing of that particular name is dropped (best effort).
const MAX_CLAMP_ENTRIES: usize = 4096;

/// Process-global memo mapping a length-clamped wire name back to its canonical
/// tool id. See the module docs for why this is correct under concurrency.
fn clamp_registry() -> &'static Mutex<HashMap<String, String>> {
    static REG: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// True when `name` is safe to put on the wire verbatim: non-empty, every byte
/// in `[A-Za-z0-9_-]`, and not masquerading as an encoded name.
fn is_plain(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(SENTINEL)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Encode a canonical WCore tool name into a wire-legal function name that
/// always matches `^[a-zA-Z0-9_-]{1,64}$`. A no-op for names that are already
/// legal, in-budget, and unambiguous (see module docs).
pub(crate) fn encode_tool_name(name: &str) -> String {
    // Fast path: already legal, unambiguous, and within the length budget.
    if is_plain(name) && name.len() <= MAX_WIRE_LEN {
        return name.to_string();
    }
    // Reversible charset escape. For a charset-clean but over-length name this
    // only prepends the sentinel (no bytes need escaping), so `escaped` may
    // still exceed the budget — handled by the clamp below.
    let escaped = escape_charset(name);
    if escaped.len() <= MAX_WIRE_LEN {
        return escaped;
    }
    clamp_and_register(name)
}

/// Reversible charset escape: `SENTINEL + hex`, where every byte not in
/// `[A-Za-z0-9-]` (incl. `_`, so `_` unambiguously introduces an escape, and
/// any multi-byte UTF-8) is written as `_HH`. Always sentinel-tagged, so its
/// output is a valid input to the hex branch of [`decode_tool_name`].
fn escape_charset(name: &str) -> String {
    let mut out = String::with_capacity(SENTINEL.len() + name.len() * 3);
    out.push_str(SENTINEL);
    for &b in name.as_bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' {
            out.push(b as char);
        } else {
            out.push('_');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// Clamp an over-length name to a deterministic, wire-legal, unique key and
/// memoise `clamped → name` for reverse routing. Truncation is lossy, so this
/// key is reversible only via the memo table, not via [`decode_tool_name`]'s
/// stateless hex path.
fn clamp_and_register(name: &str) -> String {
    let hash = fnv1a64_hex(name);
    // SENTINEL + prefix + '-' + hash ≤ MAX_WIRE_LEN.
    let budget = MAX_WIRE_LEN - SENTINEL.len() - 1 - hash.len();
    let prefix: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(budget)
        .collect();
    let clamped = format!("{SENTINEL}{prefix}-{hash}");

    let mut reg = lock_registry();
    if reg.contains_key(&clamped) || reg.len() < MAX_CLAMP_ENTRIES {
        reg.insert(clamped.clone(), name.to_string());
    } else {
        warn_clamp_cap_reached();
    }
    clamped
}

/// Warn (at most once per process) that the clamp memo is full, so a new
/// distinct over-length name can no longer be reverse-routed. Silent past the
/// cap would otherwise mask a tool that the model can call but never resolve.
fn warn_clamp_cap_reached() {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            target: "wcore_providers::tool_name",
            cap = MAX_CLAMP_ENTRIES,
            "tool-name clamp memo at capacity; reverse routing for further \
             distinct over-length names is dropped (best effort)"
        );
    });
}

/// FNV-1a 64-bit hash rendered as 16 lowercase hex chars. Deterministic and
/// dependency-free; only in-process stability is required (the table is
/// repopulated by `build_tools` on every request).
fn fnv1a64_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Lock the memo table, recovering from a poisoned mutex (our critical sections
/// never panic, so the inner map is always consistent).
fn lock_registry() -> std::sync::MutexGuard<'static, HashMap<String, String>> {
    clamp_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Decode a wire function name back to the canonical WCore tool id. A length-
/// clamped name is reversed via the memo table; a charset-escaped name is
/// reversed from its hex body. A name with no sentinel prefix (a verbatim tool
/// id, or a model-hallucinated name we never sent) is returned unchanged and
/// surfaces as a normal unknown-tool error downstream.
pub(crate) fn decode_tool_name(name: &str) -> String {
    // Only sentinel-prefixed names are ever encoded/clamped; a plain name can
    // never be a memo key, so skip the global lock entirely for the common case.
    let Some(body) = name.strip_prefix(SENTINEL) else {
        return name.to_string();
    };
    // Length-clamped names are only reversible via the memo table.
    if let Some(original) = lock_registry().get(name) {
        return original.clone();
    }
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
            "path/to/tool",
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

    /// #139 collision safety: two DISTINCT raw names must never share a wire
    /// spelling, or one tool would shadow the other at dispatch. Adversarial
    /// set: near-identical dirty names, a plain name that IS another name's
    /// escape body (`a_2Fb` vs `a/b` → `wct_a_2Fb`), and a raw name carrying
    /// the sentinel itself. Every pair must encode distinctly and every name
    /// must round-trip to itself.
    #[test]
    fn distinct_raw_names_never_collide_on_the_wire() {
        let names = [
            "Browser::execute",
            "Browser:_execute",
            "Browser__execute",
            "Browser._execute",
            "a/b",
            "a.b",
            "a:b",
            "a_2Fb",
            "wct_a_2Fb",
        ];
        let encoded: Vec<String> = names.iter().map(|n| encode_tool_name(n)).collect();
        for (i, enc) in encoded.iter().enumerate() {
            assert!(is_wire_legal(enc), "{} encoded to illegal {enc}", names[i]);
            assert_eq!(
                decode_tool_name(enc),
                names[i],
                "round-trip failed for {}",
                names[i]
            );
            for j in (i + 1)..encoded.len() {
                assert_ne!(
                    *enc, encoded[j],
                    "{} and {} collided on the wire",
                    names[i], names[j]
                );
            }
        }
    }

    /// A charset-clean but over-length name (real MCP shape) is clamped to a
    /// wire-legal ≤ 64-char key and round-trips via the memo table. Without the
    /// clamp this 83-char name would pass `is_plain` verbatim and 400 OpenAI.
    #[test]
    fn long_clean_names_are_clamped_and_round_trip() {
        let n =
            "mcp__io-github-taylorwilsdon-google-workspace-mcp__batch_modify_gmail_message_labels";
        assert!(n.len() > MAX_WIRE_LEN, "fixture must exceed the budget");
        let enc = encode_tool_name(n);
        assert!(
            enc.starts_with(SENTINEL),
            "clamped name must be sentinel-tagged: {enc}"
        );
        assert!(is_wire_legal(&enc), "clamped {enc} is not wire-legal");
        assert_eq!(decode_tool_name(&enc), n, "clamp round-trip failed for {n}");
    }

    /// A name that is BOTH charset-dirty AND over-length is clamped and still
    /// round-trips.
    #[test]
    fn long_dirty_names_are_clamped_and_round_trip() {
        let n = format!(
            "mcp__vendor.example:server__{}",
            "very_long_tool_name_".repeat(5)
        );
        assert!(n.len() > MAX_WIRE_LEN);
        let enc = encode_tool_name(&n);
        assert!(is_wire_legal(&enc), "clamped {enc} is not wire-legal");
        assert_eq!(decode_tool_name(&enc), n, "clamp round-trip failed for {n}");
    }

    /// Two distinct over-length names sharing a long common prefix must clamp to
    /// distinct keys (the hash disambiguates) so neither shadows the other.
    #[test]
    fn distinct_long_names_do_not_collide() {
        let a = format!("mcp__server__{}_alpha", "x".repeat(60));
        let b = format!("mcp__server__{}_beta", "x".repeat(60));
        let ea = encode_tool_name(&a);
        let eb = encode_tool_name(&b);
        assert_ne!(ea, eb, "distinct long names must not collide");
        assert_eq!(decode_tool_name(&ea), a);
        assert_eq!(decode_tool_name(&eb), b);
    }

    /// A name exactly at the 64-char budget that is charset-clean is emitted
    /// verbatim; one char longer is clamped.
    #[test]
    fn length_boundary_at_budget() {
        let at = "a".repeat(MAX_WIRE_LEN);
        assert_eq!(encode_tool_name(&at), at, "≤64 clean name must be verbatim");
        let over = "a".repeat(MAX_WIRE_LEN + 1);
        let enc = encode_tool_name(&over);
        assert!(enc.len() <= MAX_WIRE_LEN && is_wire_legal(&enc));
        assert_eq!(decode_tool_name(&enc), over);
    }

    fn is_wire_legal(s: &str) -> bool {
        !s.is_empty()
            && s.len() <= MAX_WIRE_LEN
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }
}
