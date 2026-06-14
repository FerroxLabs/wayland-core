use std::borrow::Cow;
use std::sync::OnceLock;

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
        r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
    ),
    // Bearer token (header value style, >=20 chars of token material)
    ("BEARER_TOKEN", r"Bearer [A-Za-z0-9._\-]{20,}"),
    // ── Hermes redact.py port — additional credential prefixes ──
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
        r"-----BEGIN[A-Z ]*PRIVATE KEY-----[\s\S]*?-----END[A-Z ]*PRIVATE KEY-----",
    ),
    // Database connection-string passwords: protocol://user:PASS@host.
    // Full-match replacement (not group-1) is acceptable here — connection
    // strings already embed credentials and should be redacted entirely.
    (
        "DB_CONNECTION_STRING",
        r"(?i)(?:postgres(?:ql)?|mysql|mongodb(?:\+srv)?|redis|amqp)://[^:\s]+:[^@\s]+@\S+",
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

/// Scrubs known PII/credential patterns from a string, replacing each match
/// with `[REDACTED:<KIND>]`.
///
/// Returns `Cow::Borrowed(input)` with zero allocation when no pattern matches.
pub struct PIIScrubber;

impl PIIScrubber {
    /// Scrub `input`, returning the original slice if nothing matched.
    pub fn scrub<'a>(&self, input: &'a str) -> Cow<'a, str> {
        // Fast bail-out: if the set finds no match, return borrowed.
        if !fast_set().is_match(input) {
            return Cow::Borrowed(input);
        }

        let mut result = input.to_owned();
        for (idx, rx) in compiled().iter().enumerate() {
            let label = PATTERNS[idx].0;
            let replacement = format!("[REDACTED:{label}]");
            // Replace all non-overlapping matches. For capture-group patterns
            // (AWS_SECRET_KEY), replace the full match (group 0).
            let replaced = rx.replace_all(&result, replacement.as_str()).into_owned();
            result = replaced;
        }
        Cow::Owned(result)
    }
}

#[cfg(test)]
mod tests {
    //! Per-pattern positive + negative coverage for the patterns ported from
    //! the Hermes ``redact.py`` library (T3-4). Existing patterns
    //! (AWS_*, OPENAI_API_KEY, ANTHROPIC_API_KEY, JWT, BEARER_TOKEN) are
    //! covered by ``crates/wcore-safety/tests/safety_tests.rs``.
    use super::PIIScrubber;

    fn redacted(input: &str, label: &str) -> bool {
        let s = PIIScrubber;
        s.scrub(input).contains(&format!("[REDACTED:{label}]"))
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
