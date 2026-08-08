use std::borrow::Cow;
use std::sync::OnceLock;

use base64::Engine as _;
use regex::{Regex, RegexSet};

/// Compiled PII pattern set. Each entry is (label, regex_str).
/// Order matters: label is embedded in the replacement string.
static PATTERNS: &[(&str, &str)] = &[
    ("AWS_ACCESS_KEY", r"AKIA[0-9A-Z]{16}"),
    // AWS secret: 40 chars of base64url after "aws_secret_access_key" or standalone.
    // Pattern uses [\x27\x22] to avoid the rustc char-literal ambiguity with raw strings.
    (
        "AWS_SECRET_KEY",
        r"(?i)aws.{0,30}secret.{0,30}[=:\s][\x22\x27]?([A-Za-z0-9/+=]{40})[\x22\x27]?",
    ),
    ("OPENAI_API_KEY", r"sk-[A-Za-z0-9]{32,}"),
    ("ANTHROPIC_API_KEY", r"sk-ant-[A-Za-z0-9\-_]+"),
    // JWT: header.payload.signature (all base64url segments)
    (
        "JWT",
        r"eyJ(?:[A-Za-z0-9_-]\s*)+\.\s*eyJ(?:[A-Za-z0-9_-]\s*)+\.\s*(?:[A-Za-z0-9_-]\s*)+",
    ),
    // Bearer token (header value style, >=20 chars of token material)
    ("BEARER_TOKEN", r"Bearer\s+(?:[A-Za-z0-9._~+\-/=]\s*){20,}"),
    // ── Prior Wayland Python engine redaction port — additional credential prefixes ──
    // GitHub personal access tokens (classic) and fine-grained.
    ("GITHUB_PAT", r"ghp_[A-Za-z0-9]{20,}"),
    ("GITHUB_PAT_FG", r"github_pat_[A-Za-z0-9_]{20,}"),
    // GitHub OAuth / server-to-server family: gho_, ghu_, ghs_, ghr_.
    ("GITHUB_OAUTH", r"gh[ousr]_[A-Za-z0-9]{20,}"),
    // Slack bot/user/app/refresh tokens: xoxb-/xoxa-/xoxp-/xoxr-/xoxs-.
    ("SLACK_TOKEN", r"xox[baprs]-[A-Za-z0-9-]{10,}"),
    // Google API keys (Maps/YouTube/etc).
    ("GOOGLE_API_KEY", r"AIza[A-Za-z0-9_\-]{30,}"),
    // Google OAuth refresh code per Google's OAuth 2.0 docs.
    ("GOOGLE_OAUTH_REFRESH", r"\b4/0[A-Za-z0-9_\-]{20,}\b"),
    // Stripe live / test / restricted secret keys.
    (
        "STRIPE_SECRET_KEY",
        r"(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{20,}",
    ),
    // SendGrid API key (literal "SG." prefix, then two base64ish segments).
    ("SENDGRID_API_KEY", r"SG\.[A-Za-z0-9_\-]{20,}"),
    // HuggingFace user access token.
    ("HUGGINGFACE_TOKEN", r"hf_[A-Za-z0-9]{20,}"),
    // Replicate API token.
    ("REPLICATE_TOKEN", r"r8_[A-Za-z0-9]{20,}"),
    // npm access token.
    ("NPM_TOKEN", r"npm_[A-Za-z0-9]{30,}"),
    // PyPI API token.
    ("PYPI_TOKEN", r"pypi-[A-Za-z0-9_\-]{20,}"),
    // DigitalOcean personal / OAuth tokens.
    ("DIGITALOCEAN_TOKEN", r"do[op]_v1_[A-Za-z0-9]{20,}"),
    // Perplexity API key.
    ("PERPLEXITY_API_KEY", r"pplx-[A-Za-z0-9]{20,}"),
    // Groq Cloud API key.
    ("GROQ_API_KEY", r"gsk_[A-Za-z0-9]{20,}"),
    // Tavily search API key.
    ("TAVILY_API_KEY", r"tvly-[A-Za-z0-9]{20,}"),
    // Exa search API key.
    ("EXA_API_KEY", r"exa_[A-Za-z0-9]{20,}"),
    // Firecrawl API key.
    ("FIRECRAWL_API_KEY", r"fc-[A-Za-z0-9]{20,}"),
    // BrowserBase live API key.
    ("BROWSERBASE_KEY", r"bb_live_[A-Za-z0-9_\-]{20,}"),
    // Telegram bot tokens: <digits>:<>=30 url-safe chars>, with optional "bot" prefix.
    ("TELEGRAM_BOT_TOKEN", r"(?:bot)?\d{8,}:[A-Za-z0-9_\-]{30,}"),
    // PEM-encoded private key blocks (RSA, EC, OPENSSH, generic PRIVATE KEY).
    (
        "PRIVATE_KEY_BLOCK",
        r"-----B\s*E\s*G\s*I\s*N[A-Z\s]*PRIVATE\s+KEY-----[\s\S]*?-----E\s*N\s*D[A-Z\s]*PRIVATE\s+KEY-----",
    ),
    // Database connection-string passwords: protocol://user:PASS@host.
    // Full-match replacement (not group-1) is acceptable here — connection
    // strings already embed credentials and should be redacted entirely.
    (
        "DB_CONNECTION_STRING",
        r"(?i)(?:postgres(?:ql)?|mysql|mongodb(?:\+srv)?|redis|amqp)://[^:\s]+:[^@\s]+@\S+",
    ),
    // Shell/.env/YAML-style secret assignments. Match secret-bearing key
    // segments rather than every variable so benign configuration remains
    // readable while unknown credential values are still scrubbed.
    (
        "SECRET_ASSIGNMENT",
        // The key prefix is BOUNDED on purpose. Unbounded (`(?:[A-Z][A-Z0-9]*_)*`
        // / `(?:_[A-Z0-9]+)*`) it is an amplifier: whitespace-normalized text has
        // exactly one `^` anchor, at offset 0, so a single trailing `TOKEN=` let
        // the prefix walk forward across an entire 34 KB blob and the whole thing
        // was replaced by one marker. Caps the key at ~248 chars per side. This
        // IS a coverage narrowing at the extremes — an env key with more than 8
        // underscore-separated segments, or a single segment over 31 chars, stops
        // matching — and it is stated rather than assumed; see
        // `a_realistically_long_key_still_matches`.
        r"(?im)^\s*(?:export\s+)?(?:[A-Z][A-Z0-9]{0,30}_){0,8}(?:API_KEY|TOKEN|SECRET|PASSWORD|PASSWD|PASSPHRASE|PRIVATE_KEY|ACCESS_KEY|CREDENTIALS?|AUTH)(?:_[A-Z0-9]{1,30}){0,8}\s*[:=]\s*[^#\r\n]+",
    ),
    // E.164 phone numbers: +<country><6-14 digits>. Negative lookahead via
    // word boundary so adjacent alphanumerics don't reach in.
    ("PHONE_E164", r"\+[1-9]\d{6,14}\b"),
    // Discord snowflake user/role mentions.
    ("DISCORD_MENTION", r"<@!?\d{17,20}>"),
];

/// Pre-compiled individual regexes, one per pattern, in the same order as PATTERNS.
static COMPILED: OnceLock<Vec<Regex>> = OnceLock::new();

/// Fast pre-filter: any pattern matches at all?
static FAST_SET: OnceLock<RegexSet> = OnceLock::new();
static BASE64_CANDIDATES: OnceLock<Regex> = OnceLock::new();
static WRAPPED_BASE64_CANDIDATES: OnceLock<Regex> = OnceLock::new();

fn compiled() -> &'static Vec<Regex> {
    COMPILED.get_or_init(|| {
        PATTERNS
            .iter()
            .map(|(_, pat)| Regex::new(pat).expect("wcore-safety: invalid PII regex"))
            .collect()
    })
}

fn fast_set() -> &'static RegexSet {
    FAST_SET.get_or_init(|| {
        let pats: Vec<&str> = PATTERNS.iter().map(|(_, p)| *p).collect();
        RegexSet::new(pats).expect("wcore-safety: invalid PII regex set")
    })
}

fn base64_candidates() -> &'static Regex {
    BASE64_CANDIDATES.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9+/_-]{24,}={0,2}")
            .expect("wcore-safety: invalid base64 candidate regex")
    })
}

fn wrapped_base64_candidates() -> &'static Regex {
    WRAPPED_BASE64_CANDIDATES.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9+/_-](?:[A-Za-z0-9+/_=\r\n\t ]{22,})")
            .expect("wcore-safety: invalid wrapped base64 candidate regex")
    })
}

fn scrub_direct<'a>(input: &'a str) -> Cow<'a, str> {
    if !fast_set().is_match(input) {
        return Cow::Borrowed(input);
    }

    let mut result = input.to_owned();
    for (idx, rx) in compiled().iter().enumerate() {
        let label = PATTERNS[idx].0;
        let replacement = format!("[REDACTED:{label}]");
        if label == "SECRET_ASSIGNMENT" {
            result = rx
                .replace_all(&result, |captures: &regex::Captures<'_>| {
                    let matched = captures
                        .get(0)
                        .expect("wcore-safety: replacement capture must exist")
                        .as_str();
                    if matched.contains("[REDACTED:") {
                        matched.to_owned()
                    } else {
                        replacement.clone()
                    }
                })
                .into_owned();
        } else {
            result = rx.replace_all(&result, replacement.as_str()).into_owned();
        }
    }
    Cow::Owned(result)
}

/// Byte spans `(start, end, label)` of every direct pattern match in `input`,
/// sorted ascending and merged.
///
/// Deliberately NOT the implementation of `scrub_direct`. Those two are not
/// equivalent: `scrub_direct` is a cascade of sequential `replace_all`s, and its
/// `[REDACTED:` guard exists so `SECRET_ASSIGNMENT` preserves a line an earlier
/// pattern already marked. In a single merged pass no marker exists yet, so the
/// guard would be vacuous and the leftmost label would win —
/// `API_KEY=ghp_aaaa...` would go from `API_KEY=[REDACTED:GITHUB_PAT]` to
/// `[REDACTED:SECRET_ASSIGNMENT]`, new over-redaction on the path every consumer
/// uses. This function is used ONLY by the whitespace-normalized re-scan.
fn direct_match_spans(input: &str) -> Vec<(usize, usize, &'static str)> {
    if !fast_set().is_match(input) {
        return Vec::new();
    }
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    for (idx, rx) in compiled().iter().enumerate() {
        let label = PATTERNS[idx].0;
        for m in rx.find_iter(input) {
            // Same guard `scrub_direct` applies, or a second pass re-matches its
            // own marker and shifts every subsequent offset.
            if label == "SECRET_ASSIGNMENT" && m.as_str().contains("[REDACTED:") {
                continue;
            }
            spans.push((m.start(), m.end(), label));
        }
    }
    spans.sort_by_key(|(start, end, _)| (*start, std::cmp::Reverse(*end)));
    let mut merged: Vec<(usize, usize, &'static str)> = Vec::new();
    for (start, end, label) in spans {
        match merged.last_mut() {
            // Overlapping OR adjacent: an unmerged pair would splice a
            // backwards slice and either panic or emit matched bytes.
            Some(prev) if start <= prev.1 => prev.1 = prev.1.max(end),
            _ => merged.push((start, end, label)),
        }
    }
    merged
}

/// Delete ASCII whitespace, recording for each byte of the result the
/// `(start, end)` byte range of the ORIGINAL char it came from.
fn normalize_with_map(src: &str) -> (String, Vec<(usize, usize)>) {
    let mut out = String::with_capacity(src.len());
    let mut map = Vec::with_capacity(src.len());
    for (i, ch) in src.char_indices() {
        if ch.is_ascii_whitespace() {
            continue;
        }
        let len = ch.len_utf8();
        out.push(ch);
        for _ in 0..len {
            map.push((i, i + len));
        }
    }
    (out, map)
}

/// Whitespace-normalize `src`, find the direct matches in that normalized
/// space, and splice their markers back into ORIGINAL coordinates.
///
/// The redaction unit is the MATCH, not the candidate. Everything outside a
/// match stays byte-identical — line numbers, tabs and newlines included.
/// Returns `None` when there is nothing to redact, which is the common path and
/// allocates only the normalized copy.
///
/// Bound honesty: the span is the bound of the match IN NORMALIZED SPACE. Every
/// open-ended pattern (GITHUB_PAT `{20,}`, OPENAI `{32,}`, ANTHROPIC `+`, SLACK,
/// GOOGLE) still extends to the next non-alphanumeric character with whitespace
/// deleted. Tightening that needs trailing anchors on the pattern set, which is
/// a bigger change than this.
fn splice_normalized_spans(src: &str) -> Option<String> {
    let (normalized, map) = normalize_with_map(src);
    let spans = direct_match_spans(&normalized);
    if spans.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(src.len());
    let mut last = 0usize;
    for (start, end, label) in spans {
        if end == 0 || end > map.len() {
            continue;
        }
        let origin_start = map[start].0;
        let origin_end = map[end - 1].1;
        debug_assert!(
            origin_start >= last,
            "direct_match_spans must return sorted, merged spans"
        );
        if origin_start < last {
            continue;
        }
        out.push_str(&src[last..origin_start]);
        out.push_str(&format!("[REDACTED:{label}]"));
        last = origin_end;
    }
    out.push_str(&src[last..]);
    Some(out)
}

fn decoded_contains_secret(candidate: &str) -> bool {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};

    [
        STANDARD.decode(candidate),
        STANDARD_NO_PAD.decode(candidate),
        URL_SAFE.decode(candidate),
        URL_SAFE_NO_PAD.decode(candidate),
    ]
    .into_iter()
    .flatten()
    .any(|bytes| {
        matches!(
            scrub_direct(&String::from_utf8_lossy(&bytes)),
            Cow::Owned(_)
        )
    })
}

/// Scrubs known PII/credential patterns from a string, replacing each match
/// with `[REDACTED:<KIND>]`.
///
/// Returns `Cow::Borrowed(input)` with zero allocation when no pattern matches.
pub struct PIIScrubber;

impl PIIScrubber {
    /// Scrub `input`, returning the original slice if nothing matched.
    pub fn scrub<'a>(&self, input: &'a str) -> Cow<'a, str> {
        let direct = scrub_direct(input);
        // Fast, deterministic MIME-wrapped case: streaming redaction groups a
        // pure base64 candidate before calling us. Strip arbitrary ASCII
        // whitespace and decode the whole record before the more permissive
        // embedded-candidate scan below.
        let wrapped_record = direct.bytes().any(|byte| byte.is_ascii_whitespace())
            && direct.bytes().all(|byte| {
                byte.is_ascii_whitespace()
                    || byte.is_ascii_alphanumeric()
                    || matches!(byte, b'+' | b'/' | b'_' | b'-' | b'=')
            });
        if wrapped_record {
            // Span-accurate: replace each MATCH in original coordinates instead
            // of returning the whole whitespace-stripped record. Returning the
            // normalized record destroyed every newline, tab and indent in the
            // output even when the actual secret was 40 bytes.
            if let Some(spliced) = splice_normalized_spans(direct.as_ref()) {
                return Cow::Owned(spliced);
            }
            let normalized_record: String = direct
                .chars()
                .filter(|ch| !ch.is_ascii_whitespace())
                .collect();
            // Whole-record decode branch, deliberately unchanged: a blob that
            // base64-decodes to a secret genuinely IS the unit.
            if normalized_record.len() >= 24 && decoded_contains_secret(&normalized_record) {
                return Cow::Owned("[REDACTED:ENCODED_SECRET]".to_string());
            }
        }
        let mut last = 0;
        let mut encoded = None::<String>;
        for candidate in base64_candidates().find_iter(direct.as_ref()) {
            if !decoded_contains_secret(candidate.as_str()) {
                continue;
            }
            let out = encoded.get_or_insert_with(|| String::with_capacity(direct.len()));
            out.push_str(&direct[last..candidate.start()]);
            out.push_str("[REDACTED:ENCODED_SECRET]");
            last = candidate.end();
        }
        let continuous = if let Some(mut encoded) = encoded {
            encoded.push_str(&direct[last..]);
            Cow::Owned(encoded)
        } else {
            direct
        };

        let mut last = 0;
        let mut wrapped = None::<String>;
        for candidate in wrapped_base64_candidates().find_iter(continuous.as_ref()) {
            // The candidate regex matches any run of alphanumerics + ASCII
            // whitespace + `+/_=`, unbounded — ordinary punctuation-free prose
            // IS such a run, so a 2000-line numbered Read result is ONE
            // candidate. Replacing the candidate therefore sized the redaction
            // by the surrounding benign text: 49,995 bytes became 34. Replace
            // only the matched spans, in original coordinates.
            if let Some(spliced) = splice_normalized_spans(candidate.as_str()) {
                let out = wrapped.get_or_insert_with(|| String::with_capacity(continuous.len()));
                out.push_str(&continuous[last..candidate.start()]);
                out.push_str(&spliced);
                last = candidate.end();
                continue;
            }
            let normalized: String = candidate
                .as_str()
                .chars()
                .filter(|ch| !ch.is_ascii_whitespace())
                .collect();
            if normalized.len() < 24 || !decoded_contains_secret(&normalized) {
                continue;
            }
            let out = wrapped.get_or_insert_with(|| String::with_capacity(continuous.len()));
            out.push_str(&continuous[last..candidate.start()]);
            out.push_str("[REDACTED:ENCODED_SECRET]");
            last = candidate.end();
        }
        if let Some(mut wrapped) = wrapped {
            wrapped.push_str(&continuous[last..]);
            Cow::Owned(wrapped)
        } else {
            continuous
        }
    }
}

#[cfg(test)]
mod tests {
    //! Per-pattern positive + negative coverage for the patterns ported from
    //! the prior Wayland Python engine's redaction library (T3-4). Existing patterns
    //! (AWS_*, OPENAI_API_KEY, ANTHROPIC_API_KEY, JWT, BEARER_TOKEN) are
    //! covered by ``crates/wcore-safety/tests/safety_tests.rs``.
    use super::{PIIScrubber, decoded_contains_secret, wrapped_base64_candidates};
    use base64::Engine as _;

    fn redacted(input: &str, label: &str) -> bool {
        let s = PIIScrubber;
        s.scrub(input).contains(&format!("[REDACTED:{label}]"))
    }

    fn test_openai_key() -> String {
        ["sk", "-", "abcdefghijklmnopqrstuvwxyz0123456789AB"].concat()
    }

    #[test]
    fn base64_encoded_secret_is_redacted_without_decoding_normal_payloads() {
        let secret = test_openai_key();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&secret);
        let scrubber = PIIScrubber;

        assert_eq!(
            scrubber.scrub(&format!("payload={encoded}")),
            "payload=[REDACTED:ENCODED_SECRET]"
        );
        assert_eq!(
            scrubber.scrub("VGhpcyBpcyBhIG5vcm1hbCBiYXNlNjQgcGF5bG9hZA=="),
            "VGhpcyBpcyBhIG5vcm1hbCBiYXNlNjQgcGF5bG9hZA=="
        );
    }

    #[test]
    fn padded_and_binary_base64_secrets_are_redacted() {
        let secret = test_openai_key();
        let padded = format!("{}{}", "A".repeat(9_000), secret);
        let padded_encoded = base64::engine::general_purpose::STANDARD.encode(padded);
        assert_eq!(
            PIIScrubber.scrub(&padded_encoded),
            "[REDACTED:ENCODED_SECRET]"
        );

        let mut binary = vec![0xff, 0xfe];
        binary.extend_from_slice(secret.as_bytes());
        binary.push(0xff);
        let binary_encoded = base64::engine::general_purpose::STANDARD.encode(binary);
        assert_eq!(
            PIIScrubber.scrub(&binary_encoded),
            "[REDACTED:ENCODED_SECRET]"
        );
    }

    #[test]
    fn whitespace_wrapped_base64_secret_is_redacted() {
        let secret = test_openai_key();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&secret);
        let wrapped = encoded
            .as_bytes()
            .chunks(9)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect::<Vec<_>>()
            .join(" \n\t");

        let candidate = wrapped_base64_candidates()
            .find(&wrapped)
            .expect("wrapped candidate");
        let normalized: String = candidate
            .as_str()
            .chars()
            .filter(|ch| !ch.is_ascii_whitespace())
            .collect();
        assert!(decoded_contains_secret(&normalized));

        assert_eq!(PIIScrubber.scrub(&wrapped), "[REDACTED:ENCODED_SECRET]");
    }

    #[test]
    fn whitespace_split_direct_secret_is_redacted() {
        let split = "prefix token=gh\np_abcdefghij\nklmnopqrstuvwxyz012345 suffix";
        let scrubbed = PIIScrubber.scrub(split);

        assert!(!scrubbed.contains("gh\np_"));
        assert!(
            scrubbed.contains("[REDACTED:GITHUB_PAT]"),
            "got: {scrubbed}"
        );
    }

    #[test]
    fn env_and_case_variant_secret_assignments_are_redacted() {
        let scrubber = PIIScrubber;
        let input = "ordinary=value\nMy_Password = hunter2\nservice_auth: opaque-value";
        let scrubbed = scrubber.scrub(input);

        assert!(scrubbed.contains("ordinary=value"));
        assert!(!scrubbed.contains("hunter2"));
        assert!(!scrubbed.contains("opaque-value"));
        assert_eq!(scrubbed.matches("[REDACTED:SECRET_ASSIGNMENT]").count(), 2);
    }

    // ── GitHub family ───────────────────────────────────────────────────
    #[test]
    fn github_pat_positive() {
        assert!(redacted(
            "token=ghp_aBCDefGHIjKLmNOPqrSTuvWXyz0123456789",
            "GITHUB_PAT",
        ));
    }
    #[test]
    fn github_pat_negative() {
        // "ghp_" prefix but too short — should not match.
        let s = PIIScrubber;
        let out = s.scrub("see ghp_short for ref");
        assert!(!out.contains("[REDACTED:GITHUB_PAT]"), "got: {out}");
    }

    #[test]
    fn github_pat_finegrained_positive() {
        assert!(redacted(
            "github_pat_11ABCDEFG0123456789_aBCdEfGhIjKlMnOpQrStUv",
            "GITHUB_PAT_FG",
        ));
    }
    #[test]
    fn github_pat_finegrained_negative() {
        let s = PIIScrubber;
        let out = s.scrub("github_pat_tiny");
        assert!(!out.contains("[REDACTED:GITHUB_PAT_FG]"), "got: {out}");
    }

    #[test]
    fn github_oauth_positive() {
        assert!(redacted(
            "tok=gho_aBCDefGHIjKLmNOPqrSTuvWXyz0123",
            "GITHUB_OAUTH",
        ));
        assert!(redacted(
            "tok=ghs_aBCDefGHIjKLmNOPqrSTuvWXyz0123",
            "GITHUB_OAUTH",
        ));
    }
    #[test]
    fn github_oauth_negative() {
        // ghx_ is not a recognised GitHub prefix.
        let s = PIIScrubber;
        let out = s.scrub("ghx_aBCDefGHIjKLmNOPqrSTuvWXyz0123");
        assert!(!out.contains("[REDACTED:GITHUB_OAUTH]"), "got: {out}");
    }

    // ── Slack ──────────────────────────────────────────────────────────
    #[test]
    fn slack_token_positive() {
        assert!(redacted(
            "slack=xoxb-1234567890-0987654321-abcDEF",
            "SLACK_TOKEN",
        ));
    }
    #[test]
    fn slack_token_negative() {
        // xoxz- is not a real Slack prefix; should not match.
        let s = PIIScrubber;
        let out = s.scrub("xoxz-1234567890-0987654321-abcDEF");
        assert!(!out.contains("[REDACTED:SLACK_TOKEN]"), "got: {out}");
    }

    // ── Google ─────────────────────────────────────────────────────────
    #[test]
    fn google_api_key_positive() {
        assert!(redacted(
            "key=AIzaSyA-aBC123_-DEFghiJKLmnoPQRstuVWXyz0",
            "GOOGLE_API_KEY",
        ));
    }
    #[test]
    fn google_api_key_negative() {
        // "AIza" but too short tail.
        let s = PIIScrubber;
        let out = s.scrub("AIzaShort");
        assert!(!out.contains("[REDACTED:GOOGLE_API_KEY]"), "got: {out}");
    }

    #[test]
    fn google_oauth_refresh_positive() {
        assert!(redacted(
            "code=4/0AeaYSHBabcDEF-_ghiJKLmnoPQRst",
            "GOOGLE_OAUTH_REFRESH",
        ));
    }
    #[test]
    fn google_oauth_refresh_negative() {
        // Starts 4/1 — not the 4/0 OAuth refresh prefix.
        let s = PIIScrubber;
        let out = s.scrub("4/1AeaYSHBabcDEF_ghiJKLmnoPQRst");
        assert!(
            !out.contains("[REDACTED:GOOGLE_OAUTH_REFRESH]"),
            "got: {out}"
        );
    }

    // ── Stripe ─────────────────────────────────────────────────────────
    #[test]
    fn stripe_secret_key_positive() {
        assert!(redacted(
            "stripe=sk_live_aBCDEFghijKLMNOPqrstUVWX1234",
            "STRIPE_SECRET_KEY",
        ));
        assert!(redacted(
            "stripe=rk_test_aBCDEFghijKLMNOPqrstUVWX1234",
            "STRIPE_SECRET_KEY",
        ));
    }
    #[test]
    fn stripe_secret_key_negative() {
        // sk_dev_ is not a real Stripe environment.
        let s = PIIScrubber;
        let out = s.scrub("sk_dev_aBCDEFghijKLMNOPqrstUVWX1234");
        assert!(!out.contains("[REDACTED:STRIPE_SECRET_KEY]"), "got: {out}");
    }

    // ── SendGrid ───────────────────────────────────────────────────────
    #[test]
    fn sendgrid_api_key_positive() {
        assert!(redacted(
            "sg=SG.aBCdefGHIjklMNOpqrSTuv.WxyZ0123456789-_abc",
            "SENDGRID_API_KEY",
        ));
    }
    #[test]
    fn sendgrid_api_key_negative() {
        // Wrong prefix, no leading "SG.".
        let s = PIIScrubber;
        let out = s.scrub("XG.aBCdefGHIjklMNOpqrSTuv.WxyZ0123456789");
        assert!(!out.contains("[REDACTED:SENDGRID_API_KEY]"), "got: {out}");
    }

    // ── HuggingFace ────────────────────────────────────────────────────
    #[test]
    fn huggingface_token_positive() {
        assert!(redacted(
            "hf=hf_aBCDEFghijKLMNOPqrstUVWXyz01",
            "HUGGINGFACE_TOKEN",
        ));
    }
    #[test]
    fn huggingface_token_negative() {
        let s = PIIScrubber;
        let out = s.scrub("hf_short");
        assert!(!out.contains("[REDACTED:HUGGINGFACE_TOKEN]"), "got: {out}");
    }

    // ── Replicate / npm / PyPI ─────────────────────────────────────────
    #[test]
    fn replicate_token_positive() {
        assert!(redacted(
            "r=r8_aBCDEFghijKLMNOPqrstUVWXyz01",
            "REPLICATE_TOKEN",
        ));
    }
    #[test]
    fn replicate_token_negative() {
        let s = PIIScrubber;
        let out = s.scrub("r9_aBCDEFghijKLMNOPqrstUVWXyz01");
        assert!(!out.contains("[REDACTED:REPLICATE_TOKEN]"), "got: {out}");
    }

    #[test]
    fn npm_token_positive() {
        assert!(redacted(
            "npm=npm_aBCDEFghijKLMNOPqrstUVWXyz0123456789",
            "NPM_TOKEN",
        ));
    }
    #[test]
    fn npm_token_negative() {
        let s = PIIScrubber;
        let out = s.scrub("npm_too_short");
        assert!(!out.contains("[REDACTED:NPM_TOKEN]"), "got: {out}");
    }

    #[test]
    fn pypi_token_positive() {
        assert!(redacted(
            "pp=pypi-AgEIcHlwaS5vcmcCJDcyMTI3NjUz_-abcDEF",
            "PYPI_TOKEN",
        ));
    }
    #[test]
    fn pypi_token_negative() {
        let s = PIIScrubber;
        let out = s.scrub("pypi-short");
        assert!(!out.contains("[REDACTED:PYPI_TOKEN]"), "got: {out}");
    }

    // ── DigitalOcean / Perplexity / Groq / Tavily / Exa / BrowserBase ──
    #[test]
    fn digitalocean_token_positive() {
        assert!(redacted(
            "do=dop_v1_aBCDEFghijKLMNOPqrstUVWXyz01",
            "DIGITALOCEAN_TOKEN",
        ));
        assert!(redacted(
            "do=doo_v1_aBCDEFghijKLMNOPqrstUVWXyz01",
            "DIGITALOCEAN_TOKEN",
        ));
    }
    #[test]
    fn digitalocean_token_negative() {
        // dox_v1_ is not a recognised DO prefix.
        let s = PIIScrubber;
        let out = s.scrub("dox_v1_aBCDEFghijKLMNOPqrstUVWXyz01");
        assert!(!out.contains("[REDACTED:DIGITALOCEAN_TOKEN]"), "got: {out}");
    }

    #[test]
    fn perplexity_key_positive() {
        assert!(redacted(
            "p=pplx-aBCDEFghijKLMNOPqrstUVWXyz01",
            "PERPLEXITY_API_KEY",
        ));
    }
    #[test]
    fn perplexity_key_negative() {
        let s = PIIScrubber;
        let out = s.scrub("pplx-short");
        assert!(!out.contains("[REDACTED:PERPLEXITY_API_KEY]"), "got: {out}");
    }

    #[test]
    fn groq_key_positive() {
        assert!(redacted(
            "g=gsk_aBCDEFghijKLMNOPqrstUVWXyz01",
            "GROQ_API_KEY",
        ));
    }
    #[test]
    fn groq_key_negative() {
        let s = PIIScrubber;
        let out = s.scrub("gsk_short");
        assert!(!out.contains("[REDACTED:GROQ_API_KEY]"), "got: {out}");
    }

    #[test]
    fn tavily_key_positive() {
        assert!(redacted(
            "t=tvly-aBCDEFghijKLMNOPqrstUVWXyz01",
            "TAVILY_API_KEY",
        ));
    }
    #[test]
    fn tavily_key_negative() {
        let s = PIIScrubber;
        let out = s.scrub("tvly-short");
        assert!(!out.contains("[REDACTED:TAVILY_API_KEY]"), "got: {out}");
    }

    #[test]
    fn exa_key_positive() {
        assert!(redacted(
            "e=exa_aBCDEFghijKLMNOPqrstUVWXyz01",
            "EXA_API_KEY",
        ));
    }
    #[test]
    fn exa_key_negative() {
        let s = PIIScrubber;
        let out = s.scrub("exa_short");
        assert!(!out.contains("[REDACTED:EXA_API_KEY]"), "got: {out}");
    }

    #[test]
    fn browserbase_key_positive() {
        assert!(redacted(
            "bb=bb_live_aBCDEFghijKLMNOPqrstUVWXyz01",
            "BROWSERBASE_KEY",
        ));
    }
    #[test]
    fn browserbase_key_negative() {
        let s = PIIScrubber;
        let out = s.scrub("bb_test_aBCDEFghijKLMNOPqrstUVWXyz01");
        assert!(!out.contains("[REDACTED:BROWSERBASE_KEY]"), "got: {out}");
    }

    // ── Telegram / PEM / DB connstr / phone / discord ──────────────────
    #[test]
    fn telegram_bot_token_positive() {
        assert!(redacted(
            "tg=bot1234567890:AAH-aBCDefGHIjKLmNOPqrSTuvWXyz12",
            "TELEGRAM_BOT_TOKEN",
        ));
        assert!(redacted(
            "tg=1234567890:AAH-aBCDefGHIjKLmNOPqrSTuvWXyz12",
            "TELEGRAM_BOT_TOKEN",
        ));
    }
    #[test]
    fn telegram_bot_token_negative() {
        // Too-short digit prefix (< 8) — not a valid Telegram bot ID.
        let s = PIIScrubber;
        let out = s.scrub("12345:AAH-aBCDefGHIjKLmNOPqrSTuvWXyz12");
        assert!(!out.contains("[REDACTED:TELEGRAM_BOT_TOKEN]"), "got: {out}");
    }

    #[test]
    fn private_key_block_positive() {
        let pem =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEAx...\n-----END RSA PRIVATE KEY-----";
        assert!(redacted(pem, "PRIVATE_KEY_BLOCK"));
    }
    #[test]
    fn private_key_block_negative() {
        // Public key block — must not be redacted by the private-key pattern.
        let pem = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----";
        let s = PIIScrubber;
        let out = s.scrub(pem);
        assert!(!out.contains("[REDACTED:PRIVATE_KEY_BLOCK]"), "got: {out}");
    }

    #[test]
    fn db_connection_string_positive() {
        assert!(redacted(
            "DATABASE_URL=postgres://user:s3cret@db.example.com:5432/app",
            "DB_CONNECTION_STRING",
        ));
        assert!(redacted(
            "mongodb+srv://admin:hunter2@cluster0.mongodb.net/test",
            "DB_CONNECTION_STRING",
        ));
    }
    #[test]
    fn db_connection_string_negative() {
        // No password segment (missing :pass@) — must not match.
        let s = PIIScrubber;
        let out = s.scrub("see postgres://db.example.com:5432/app for ref");
        assert!(
            !out.contains("[REDACTED:DB_CONNECTION_STRING]"),
            "got: {out}"
        );
    }

    #[test]
    fn phone_e164_positive() {
        assert!(redacted("call +14155552671 now", "PHONE_E164"));
    }
    #[test]
    fn phone_e164_negative() {
        // Leading 0 in country code is invalid E.164 — must not match.
        let s = PIIScrubber;
        let out = s.scrub("ref +04155552671");
        assert!(!out.contains("[REDACTED:PHONE_E164]"), "got: {out}");
    }

    #[test]
    fn discord_mention_positive() {
        assert!(redacted("hi <@123456789012345678>", "DISCORD_MENTION"));
        assert!(redacted("hi <@!123456789012345678>", "DISCORD_MENTION"));
    }
    #[test]
    fn discord_mention_negative() {
        // 16-digit ID — below the 17-digit snowflake minimum.
        let s = PIIScrubber;
        let out = s.scrub("hi <@1234567890123456>");
        assert!(!out.contains("[REDACTED:DISCORD_MENTION]"), "got: {out}");
    }

    // ── Sanity: clean input still borrows after expanding pattern set ──
    #[test]
    fn clean_input_still_borrows_after_expansion() {
        let s = PIIScrubber;
        let input = "Plain log line, no secrets here, just user@example.com.";
        let out = s.scrub(input);
        assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
    }
}
