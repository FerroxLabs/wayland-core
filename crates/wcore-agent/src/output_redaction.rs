//! Central redaction for tool output before it reaches hooks, hosts, models,
//! traces, provider requests, or persisted transcripts.

use std::sync::{Arc, Weak};

use parking_lot::{Mutex, RwLock};
use wcore_safety::PIIScrubber;

/// #584 — process-wide set of live active-approval-token sources.
///
/// The active-token scrub used to be reachable ONLY through
/// [`crate::output::protocol_sink::ActiveTokenRedactor`], which exactly three
/// `ProtocolSink` methods held a handle to. Every other producer of tool text —
/// most importantly the authoritative `ToolResult` card, emitted in
/// `orchestration` through a RAW `Arc<dyn ProtocolEmitter>` that structurally
/// cannot reach a sink — emitted unscrubbed. The streaming chunk was redacted
/// and the final card that supersedes it was not.
///
/// Wiring one more redactor handle down the dispatch path would have reproduced
/// the same failure mode one call site later, so the scrub is instead published
/// process-wide: every [`ActiveTokenRedactor`] that ever publishes tokens
/// registers its shared set here, and [`redact_active_tokens`] strips the union
/// of every live set. Sources are held WEAKLY, so a dropped bridge's set is
/// pruned automatically and two concurrent bridges (tests, sub-agents) union
/// rather than clobber each other.
static ACTIVE_TOKEN_SOURCES: Mutex<Vec<Weak<RwLock<Vec<String>>>>> = Mutex::new(Vec::new());

/// Publish `inner` as a live active-token source. Idempotent: a set already
/// registered is not registered twice. Dead entries are pruned on every call,
/// which is also what keeps the pointer-identity check below sound (a pruned
/// slot can never alias a freshly allocated set).
pub(crate) fn register_active_token_source(inner: &Arc<RwLock<Vec<String>>>) {
    let mut sources = ACTIVE_TOKEN_SOURCES.lock();
    sources.retain(|weak| weak.strong_count() > 0);
    let target = Arc::as_ptr(inner);
    if !sources.iter().any(|weak| weak.as_ptr() == target) {
        sources.push(Arc::downgrade(inner));
    }
}

/// Strip every in-flight approval `apr-<uuid>` from `text`, replacing each with
/// `[REDACTED]`. A no-op — and a single uncontended lock — when no approval is
/// in flight, which is the steady state.
///
/// Only exact tokens minted by this process seconds earlier are removed, so
/// this cannot eat a checksum, an errno, or a product name out of an error
/// message: the underlying error survives intact.
pub fn redact_active_tokens(text: &str) -> String {
    let sets: Vec<Arc<RwLock<Vec<String>>>> = {
        let mut sources = ACTIVE_TOKEN_SOURCES.lock();
        sources.retain(|weak| weak.strong_count() > 0);
        sources.iter().filter_map(Weak::upgrade).collect()
    };
    let mut out = text.to_string();
    for set in sets {
        for token in set.read().iter() {
            if !token.is_empty() {
                out = out.replace(token, "[REDACTED]");
            }
        }
    }
    out
}

pub(crate) fn redact_tool_output(content: &str) -> String {
    // #584 ordering: the active-token scrub runs on the SAME pass as the PII
    // scrub and therefore ahead of truncation/compaction at every call site,
    // so a token straddling a truncation boundary is removed while it is still
    // one contiguous string. `redact` is an exact `str::replace`; a token cut
    // in half survives an emit-time-only pass.
    //
    // Within that pass the ACTIVE-TOKEN scrub must run FIRST. `PIIScrubber` is
    // itself a cutter: its patterns are greedy and none of them know where an
    // `apr-<uuid>` begins or ends, so a match that starts INSIDE a token
    // consumes the tail and strands the head, which the exact-match token
    // scrub can then no longer see. That is not hypothetical — a v4 UUID whose
    // fourth group ends in `fc` (1 token in 256) puts `fc-` inside the token,
    // FIRECRAWL_API_KEY (`fc-[A-Za-z0-9]{20,}`) matches from there through the
    // final group and on into any alphanumeric output that follows, and 25
    // characters of a live approval token reach the wire ahead of the
    // replacement.
    //
    // Token-first is the right order because it is safer in the direction that
    // is REACHABLE, not because it dominates. It reads unmodified input, so it
    // always sees a whole token, and the PII pass still sees everything the
    // token scrub left behind.
    //
    // It is NOT strictly safer, and the inverse is measured, not theoretical.
    // The token scrub substitutes `[REDACTED]`, and `[` and `]` fall outside
    // the character class of several PII patterns, so a substitution landing
    // INSIDE a PII match splits it and strands the tail. Verbatim, against
    // this compiled product, for `"Bearer " + 25*'A' + token + 30*'Z'`:
    //     PII-first   -> `[REDACTED:BEARER_TOKEN]`
    //     token-first -> `[REDACTED:BEARER_TOKEN][REDACTED]ZZZZ...` (30 chars survive)
    // Same shape for `sk-ant-...` under ANTHROPIC_API_KEY.
    //
    // That case needs a real secret to literally embed a live, process-minted
    // `apr-<uuid>`, which the model cannot know (GHSA-8r7g), so it is
    // unreachable in practice while the token-straddle case above is a
    // measured 1-in-256. Do not read this ordering as a general rule that
    // token-first cannot strand a secret: it can, and if the token
    // placeholder ever changes, re-measure both directions before assuming
    // this still holds.
    let __t = std::time::Instant::now();
    let __a = std::time::Instant::now();
    let __tok = redact_active_tokens(content);
    crate::perfcount::record(&crate::perfcount::REDACT_ACTIVE_TOKENS, __a, content.len());
    let __p = std::time::Instant::now();
    let __out = PIIScrubber.scrub(&__tok).into_owned();
    crate::perfcount::record(&crate::perfcount::PII_SCRUB, __p, __tok.len());
    crate::perfcount::record(&crate::perfcount::REDACT_TOOL_OUTPUT, __t, content.len());
    __out
}

const MAX_PENDING_BYTES: usize = 1024 * 1024;

/// Buffers only the candidate record needed to decide whether bytes are safe.
/// Every buffer has a hard cap; an oversized candidate is replaced once and
/// discarded through its delimiter so attacker output cannot exhaust memory.
pub(crate) struct StreamingRedactor {
    line: String,
    private_key: Option<String>,
    encoded_block: String,
    split_direct: String,
    discard_line: bool,
    discard_private_key: bool,
    discard_encoded: bool,
}

impl StreamingRedactor {
    pub(crate) fn new() -> Self {
        Self {
            line: String::new(),
            private_key: None,
            encoded_block: String::new(),
            split_direct: String::new(),
            discard_line: false,
            discard_private_key: false,
            discard_encoded: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &str) -> Option<String> {
        let __t = std::time::Instant::now();
        let __r = self.push_inner(chunk);
        crate::perfcount::record(&crate::perfcount::STREAMING_REDACTOR, __t, chunk.len());
        __r
    }
    fn push_inner(&mut self, chunk: &str) -> Option<String> {
        self.line.push_str(chunk);
        let mut emitted = String::new();
        while let Some(newline) = self.line.find('\n') {
            if self.discard_line {
                self.line.drain(..=newline);
                self.discard_line = false;
                continue;
            }
            if newline + 1 > MAX_PENDING_BYTES {
                self.line.drain(..=newline);
                emitted.push_str("[REDACTED:OVERSIZED_STREAM_RECORD]\n");
                continue;
            }
            let complete = self.line[..=newline].to_owned();
            self.line.drain(..=newline);
            self.process_complete_line(complete, &mut emitted);
        }
        if self.line.len() > MAX_PENDING_BYTES {
            self.line.clear();
            self.discard_line = true;
            emitted.push_str("[REDACTED:OVERSIZED_STREAM_RECORD]");
        }
        (!emitted.is_empty()).then_some(emitted)
    }

    pub(crate) fn finish(&mut self) -> Option<String> {
        if self.private_key.take().is_some() || self.discard_private_key {
            self.line.clear();
            self.encoded_block.clear();
            return Some("[REDACTED:PRIVATE_KEY_BLOCK]".to_string());
        }
        let mut emitted = String::new();
        if !self.split_direct.is_empty() {
            emitted.push_str(&redact_tool_output(&std::mem::take(&mut self.split_direct)));
        }
        if !self.encoded_block.is_empty()
            && !self.discard_line
            && is_base64_continuation(&self.line)
        {
            self.encoded_block.push_str(&std::mem::take(&mut self.line));
        }
        if !self.encoded_block.is_empty() {
            emitted.push_str(&redact_encoded_candidate(&std::mem::take(
                &mut self.encoded_block,
            )));
        }
        if !self.discard_line && !self.line.is_empty() {
            emitted.push_str(&redact_tool_output(&std::mem::take(&mut self.line)));
        }
        (!emitted.is_empty()).then_some(emitted)
    }

    fn process_complete_line(&mut self, line: String, emitted: &mut String) {
        if self.discard_private_key {
            if is_private_key_end(&line) {
                self.discard_private_key = false;
            }
            return;
        }

        if let Some(private_key) = self.private_key.as_mut() {
            private_key.push_str(&line);
            if private_key.len() > MAX_PENDING_BYTES {
                self.private_key = None;
                self.discard_private_key = true;
                emitted.push_str("[REDACTED:PRIVATE_KEY_BLOCK]\n");
                return;
            }
            if is_private_key_end(&line) {
                let block = self.private_key.take().expect("private-key buffer present");
                emitted.push_str(&redact_tool_output(&block));
            }
            return;
        }

        if !self.split_direct.is_empty() {
            if is_private_key_begin(&line) {
                emitted.push_str(&redact_tool_output(&std::mem::take(&mut self.split_direct)));
                self.process_complete_line(line, emitted);
                return;
            }
            let continuation = normalize_ascii_whitespace(&line);
            let pending = std::mem::take(&mut self.split_direct);
            let mut combined = pending.clone();
            combined.push_str(&line);
            if split_direct_candidate(&combined)
                && !continuation.is_empty()
                && is_direct_secret_continuation(&continuation)
            {
                if combined.len() > MAX_PENDING_BYTES {
                    emitted.push_str("[REDACTED:OVERSIZED_STREAM_RECORD]\n");
                } else {
                    self.split_direct = combined;
                }
            } else {
                let redacted = redact_tool_output(&combined);
                if redacted != combined {
                    emitted.push_str(&redacted);
                } else {
                    emitted.push_str(&redact_tool_output(&pending));
                    self.process_complete_line(line, emitted);
                }
            }
            return;
        }

        if is_private_key_begin(&line) {
            if !self.encoded_block.is_empty() {
                emitted.push_str(&redact_encoded_candidate(&std::mem::take(
                    &mut self.encoded_block,
                )));
            }
            if is_private_key_end(&line) {
                emitted.push_str(&redact_tool_output(&line));
            } else {
                self.private_key = Some(line);
            }
            return;
        }

        let redacted_line = redact_tool_output(&line);
        if redacted_line != line {
            if !self.encoded_block.is_empty() {
                emitted.push_str(&redact_encoded_candidate(&std::mem::take(
                    &mut self.encoded_block,
                )));
            }
            emitted.push_str(&redacted_line);
            return;
        }

        if split_direct_candidate(&line) {
            self.split_direct = line;
            return;
        }

        if (!self.encoded_block.is_empty() && is_base64_continuation(&line))
            || is_base64_fragment(&line)
        {
            if self.discard_encoded {
                return;
            }
            self.encoded_block.push_str(&line);
            if self.encoded_block.len() > MAX_PENDING_BYTES {
                self.encoded_block.clear();
                self.discard_encoded = true;
                emitted.push_str("[REDACTED:OVERSIZED_ENCODED_CANDIDATE]\n");
            }
            return;
        }
        if self.discard_encoded {
            self.discard_encoded = false;
        } else if !self.encoded_block.is_empty() {
            emitted.push_str(&redact_encoded_candidate(&std::mem::take(
                &mut self.encoded_block,
            )));
        }

        emitted.push_str(&redact_tool_output(&line));
    }
}

const DIRECT_SECRET_PREFIXES: &[&str] = &[
    "Bearer",
    "eyJ",
    "-----BEGIN",
    "-----END",
    "AKIA",
    "sk-",
    "sk-ant-",
    "ghp_",
    "github_pat_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "xoxb-",
    "xoxa-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
    "AIza",
    "4/0",
    "sk_live_",
    "sk_test_",
    "rk_live_",
    "rk_test_",
    "SG.",
    "hf_",
    "r8_",
    "npm_",
    "pypi-",
    "dop_v1_",
    "doo_v1_",
    "pplx-",
    "gsk_",
    "tvly-",
    "exa_",
    "fc-",
    "bb_live_",
];

fn normalize_ascii_whitespace(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect()
}

fn redact_encoded_candidate(value: &str) -> String {
    let first_line_end = value.find('\n').unwrap_or(value.len());
    let payload_start = value[..first_line_end]
        .rfind(['=', ':'])
        .filter(|delimiter| is_encoded_label(&value[..*delimiter]))
        .map_or(0, |delimiter| delimiter + 1);
    let normalized = normalize_ascii_whitespace(&value[payload_start..]);
    if !normalized.is_empty() {
        let redacted = redact_tool_output(&normalized);
        if redacted != normalized {
            return redacted;
        }
    }
    redact_tool_output(value)
}

fn is_encoded_label(value: &str) -> bool {
    let label = value.to_ascii_lowercase();
    [
        "base64", "encoded", "payload", "data", "token", "secret", "key",
    ]
    .iter()
    .any(|word| label.contains(word))
}

fn split_direct_candidate(value: &str) -> bool {
    let normalized = normalize_ascii_whitespace(value);
    if DIRECT_SECRET_PREFIXES
        .iter()
        .any(|prefix| normalized.contains(prefix))
    {
        return true;
    }

    std::iter::once(0)
        .chain(value.char_indices().filter_map(|(index, ch)| {
            (ch.is_ascii_whitespace() || matches!(ch, '=' | ':' | ';' | '"' | '\''))
                .then_some(index + ch.len_utf8())
        }))
        .any(|start| {
            let suffix = normalize_ascii_whitespace(&value[start..]);
            DIRECT_SECRET_PREFIXES.iter().any(|prefix| {
                !suffix.is_empty() && suffix.len() < prefix.len() && prefix.starts_with(&suffix)
            })
        })
}

fn is_base64_fragment(line: &str) -> bool {
    let normalized = line.trim().replace(|ch: char| ch.is_ascii_whitespace(), "");
    if normalized.len() >= 8 && is_base64_chars(&normalized) {
        return true;
    }

    let trimmed = line.trim();
    let Some(delimiter) = trimmed.rfind(['=', ':']) else {
        return false;
    };
    if !is_encoded_label(&trimmed[..delimiter]) {
        return false;
    }
    let suffix = normalize_ascii_whitespace(&trimmed[delimiter + 1..]);
    suffix.is_empty() || is_base64_chars(&suffix)
}

fn is_base64_continuation(line: &str) -> bool {
    let normalized = line.trim().replace(|ch: char| ch.is_ascii_whitespace(), "");
    !normalized.is_empty() && is_base64_chars(&normalized)
}

fn is_direct_secret_continuation(value: &str) -> bool {
    if value.bytes().all(|byte| byte == b'.') {
        return true;
    }
    let segment = value.strip_prefix('.').unwrap_or(value);
    !segment.is_empty()
        && segment.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'+' | b'/' | b'_' | b'-' | b'=' | b'.' | b'~')
        })
}

fn is_base64_chars(value: &str) -> bool {
    value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'_' | b'-' | b'=')
    })
}

fn is_private_key_begin(line: &str) -> bool {
    let compact = normalize_ascii_whitespace(line);
    compact.contains("-----BEGIN") && compact.contains("PRIVATEKEY-----")
}

fn is_private_key_end(line: &str) -> bool {
    let compact = normalize_ascii_whitespace(line);
    compact.contains("-----END") && compact.contains("PRIVATEKEY-----")
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    fn test_openai_key() -> String {
        ["sk", "-", "abcdefghijklmnopqrstuvwxyz0123456789AB"].concat()
    }

    #[test]
    fn direct_and_encoded_secrets_are_redacted() {
        let secret = test_openai_key();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&secret);

        assert!(!redact_tool_output(&secret).contains(&secret));
        assert_eq!(redact_tool_output(&encoded), "[REDACTED:ENCODED_SECRET]");
    }

    #[test]
    fn secret_split_across_stream_chunks_never_emits_raw_material() {
        let secret = test_openai_key();
        let split = 19;
        let mut redactor = StreamingRedactor::new();
        assert!(
            redactor
                .push(&format!("prefix {}", &secret[..split]))
                .is_none()
        );
        assert!(
            redactor
                .push(&format!("{} suffix", &secret[split..]))
                .is_none()
        );

        let emitted = redactor.finish().expect("buffered stream");
        assert!(!emitted.contains(&secret));
        assert!(emitted.contains("[REDACTED:OPENAI_API_KEY]"));
    }

    #[test]
    fn encoded_secret_split_across_stream_chunks_is_redacted() {
        let secret = test_openai_key();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&secret);
        let split = encoded.len() / 2;
        let mut redactor = StreamingRedactor::new();
        assert!(redactor.push(&encoded[..split]).is_none());
        assert!(redactor.push(&encoded[split..]).is_none());

        assert_eq!(
            redactor.finish().as_deref(),
            Some("[REDACTED:ENCODED_SECRET]")
        );
    }

    #[test]
    fn encoded_secret_split_by_arbitrary_whitespace_is_redacted() {
        let secret = test_openai_key();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&secret);
        let wrapped = encoded
            .as_bytes()
            .chunks(9)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect::<Vec<_>>()
            .join(" \n\t");
        let mut redactor = StreamingRedactor::new();
        for chunk in wrapped.as_bytes().chunks(7) {
            let _ = redactor.push(std::str::from_utf8(chunk).unwrap());
        }
        let emitted = redactor.finish().expect("buffered encoded candidate");

        assert!(!emitted.contains(&encoded));
        assert_eq!(emitted, "[REDACTED:ENCODED_SECRET]");
    }

    #[test]
    fn direct_secret_split_at_arbitrary_newlines_is_never_emitted() {
        for chunks in [
            vec!["g\n", "h\n", "p_abcde\n", "fghijklmnopqrstuvwxyz012345\n"],
            vec![
                "prefix g\n",
                "h\n",
                "p_abcde\n",
                "fghijklmnopqrstuvwxyz012345\n",
            ],
        ] {
            let mut redactor = StreamingRedactor::new();
            let mut emitted = String::new();
            for chunk in chunks {
                if let Some(output) = redactor.push(chunk) {
                    emitted.push_str(&output);
                }
            }
            if let Some(output) = redactor.finish() {
                emitted.push_str(&output);
            }

            assert!(!emitted.contains("g\nh\np_"));
            assert!(emitted.contains("[REDACTED:"), "got: {emitted}");
        }
    }

    #[test]
    fn bearer_and_jwt_split_at_newlines_are_never_emitted() {
        for chunks in [
            vec!["Bearer \n", "abcdefghij\n", "klmnopqrstuvwxyz012345\n"],
            vec!["Bearer \n", "abcdefghijklmnop+\n", "qrstuvwxyz012345\n"],
            vec![
                "eyJhead\n",
                "er\n",
                ".\n",
                "eyJpay\n",
                "load\n",
                ".\n",
                "signature\n",
            ],
        ] {
            let raw = chunks.concat();
            let mut redactor = StreamingRedactor::new();
            let mut emitted = String::new();
            for chunk in chunks {
                if let Some(output) = redactor.push(chunk) {
                    emitted.push_str(&output);
                }
            }
            if let Some(output) = redactor.finish() {
                emitted.push_str(&output);
            }

            assert_ne!(emitted, raw, "raw credential escaped: {emitted}");
            assert!(emitted.contains("[REDACTED:"), "got: {emitted}");
        }
    }

    #[test]
    fn labeled_base64_split_across_lines_is_never_emitted() {
        let secret = test_openai_key();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&secret);
        let chunks = [
            format!("encoded_payload: {}\n", &encoded[..7]),
            format!("{}\n", &encoded[7..19]),
            format!("{}\n", &encoded[19..]),
        ];
        let mut redactor = StreamingRedactor::new();
        let mut emitted = String::new();
        for chunk in chunks {
            if let Some(output) = redactor.push(&chunk) {
                emitted.push_str(&output);
            }
            assert!(!emitted.contains(&encoded));
        }
        if let Some(output) = redactor.finish() {
            emitted.push_str(&output);
        }

        assert!(!emitted.contains(&encoded));
        assert!(
            emitted.contains("[REDACTED:ENCODED_SECRET]"),
            "got: {emitted}"
        );
    }

    #[test]
    fn private_key_markers_split_across_lines_are_never_emitted() {
        let chunks = [
            "-----BE\n",
            "GIN\n",
            " PRIVATE KEY-----\n",
            "raw-key-material\n",
            "-----END\n",
            " PRIVATE KEY-----\n",
        ];
        let raw = chunks.concat();
        let mut redactor = StreamingRedactor::new();
        let mut emitted = String::new();
        for chunk in chunks {
            if let Some(output) = redactor.push(chunk) {
                emitted.push_str(&output);
            }
            assert!(!emitted.contains("raw-key-material"));
        }
        if let Some(output) = redactor.finish() {
            emitted.push_str(&output);
        }

        assert_ne!(emitted, raw);
        assert_eq!(emitted, "[REDACTED:PRIVATE_KEY_BLOCK]\n");
    }

    #[test]
    fn oversized_unterminated_record_fails_closed_with_bounded_memory() {
        let mut redactor = StreamingRedactor::new();
        let emitted = redactor
            .push(&"A".repeat(MAX_PENDING_BYTES + 1))
            .expect("oversized marker");

        assert_eq!(emitted, "[REDACTED:OVERSIZED_STREAM_RECORD]");
        assert!(redactor.line.is_empty());
        assert!(redactor.finish().is_none());
    }

    #[test]
    fn complete_ordinary_lines_stream_without_fixed_size_delay() {
        let mut redactor = StreamingRedactor::new();
        assert_eq!(redactor.push("first\nsecond"), Some("first\n".to_string()));
        assert_eq!(redactor.push("\nthird"), Some("second\n".to_string()));
        assert_eq!(redactor.finish(), Some("third".to_string()));
    }

    #[test]
    fn arbitrarily_long_split_secret_line_is_held_until_safe() {
        let secret = format!("sk-{}", "A".repeat(32 * 1024));
        let split = secret.len() / 2;
        let mut redactor = StreamingRedactor::new();
        assert!(redactor.push(&secret[..split]).is_none());
        let emitted = redactor
            .push(&format!("{}\n", &secret[split..]))
            .expect("completed line");
        assert!(!emitted.contains(&secret));
        assert!(emitted.contains("[REDACTED:OPENAI_API_KEY]"));
    }

    #[test]
    fn pem_block_is_not_emitted_before_its_end_marker() {
        let block = ["-----", "BEGIN PRIVATE KEY-----\nraw-key-material\n"].concat();
        let mut redactor = StreamingRedactor::new();
        assert!(redactor.push(&block).is_none());
        assert_eq!(
            redactor.push("-----END PRIVATE KEY-----\n").as_deref(),
            Some("[REDACTED:PRIVATE_KEY_BLOCK]\n")
        );
    }

    #[test]
    fn abandoned_prefix_candidate_does_not_bypass_pem_buffering() {
        let block = [
            "-----",
            "BEGIN PRIVATE KEY-----\nraw-key-material\n-----END PRIVATE KEY-----\n",
        ]
        .concat();
        let mut redactor = StreamingRedactor::new();
        assert!(redactor.push("g\n").is_none());
        assert_eq!(
            redactor.push(&block),
            Some("g\n[REDACTED:PRIVATE_KEY_BLOCK]\n".to_string())
        );
    }

    /// #584 regression. `PIIScrubber` must not run ahead of the active-token
    /// scrub inside [`redact_tool_output`]. Its patterns are greedy and
    /// token-unaware, so a PII match that starts INSIDE an `apr-<uuid>` eats
    /// the token's tail and leaves the head on the wire, past the exact-match
    /// token scrub that would otherwise have removed the whole thing.
    ///
    /// This token's fourth group ends in `fc` — the 1-in-256 UUID that puts a
    /// FIRECRAWL_API_KEY prefix (`fc-[A-Za-z0-9]{20,}`) inside the token — and
    /// the alphanumeric filler behind it is what the greedy match runs into.
    /// Under the old ordering the output opened with 25 plaintext characters
    /// of the token.
    #[test]
    fn pii_scrub_cannot_strand_the_head_of_an_active_token() {
        let token = String::from("apr-00000000-0000-4000-80fc-000000000000");
        // Keep the handle alive: sources are held by `Weak`.
        let redactor = crate::output::protocol_sink::ActiveTokenRedactor::new();
        redactor.set(vec![token.clone()]);
        let head = &token[..16];
        let payload = format!("{}{token}{}", "A".repeat(80), "B".repeat(400));

        // CONTROL: the old ordering really does strand the head on this input,
        // so the assertion below pins the ordering rather than passing
        // vacuously if the PII pattern set ever stops matching here.
        let pii_first = redact_active_tokens(&PIIScrubber.scrub(&payload));
        assert!(
            pii_first.contains(head),
            "control failed: PII-first left no stranded token head, so this \
             test no longer exercises the ordering it exists to pin: {pii_first}"
        );

        let out = redact_tool_output(&payload);
        assert!(
            !out.contains(head),
            "the PII scrub stranded the head {head} of an active approval \
             token on the wire: {out}"
        );
        assert!(
            !out.contains(&token),
            "the whole active approval token survived redaction: {out}"
        );
    }
}
