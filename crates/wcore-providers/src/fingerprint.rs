//! Key fingerprinting — guess the provider from a pasted credential's *shape*.
//!
//! This is the first step of the `/config` "paste-to-detect" flow: the user
//! pastes an API key, token, or credential blob, and we narrow it to a ranked
//! set of provider candidates **before** making any network call. The live
//! probe (`list_models` against the guessed provider) is the source of truth;
//! a fingerprint is only a hypothesis. A unique prefix (`sk-ant-`) yields one
//! high-confidence candidate; a shared prefix (bare `sk-`) yields an ambiguous
//! set to validate in parallel; an unknown shape yields nothing (fall back to a
//! provider picker).
//!
//! Pure and side-effect-free by design, so the whole table is unit-testable
//! without a network. Prefixes are drawn from each provider's published key
//! format (see the config-redesign research synthesis); they are matched
//! **longest-first** so that `sk-ant-` wins over the bare `sk-` bucket.

/// What kind of credential the pasted string appears to be. Most providers use
/// a simple bearer key; AWS and GCP do not, and need a guided completion path
/// rather than a single validating GET.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// A bearer-style API key, validated by one authenticated `GET /models`.
    BearerKey,
    /// An AWS access-key id (`AKIA…`/`ASIA…`). NOT a complete credential on its
    /// own — needs the secret access key + region. Branch to the AWS wizard.
    AwsAccessKeyId,
    /// An AWS Bedrock bearer API key (`ABSK…` / `bedrock-api-key-…`). Unlike a
    /// classic access key this *is* a usable bearer, no SigV4 required.
    BedrockApiKey,
    /// A GCP service-account JSON blob. Long-term signing material, not a token;
    /// needs `project_id` + a minted OAuth token before Vertex can be reached.
    GcpServiceAccount,
    /// A GCP OAuth access token (`ya29.…`). A valid but ephemeral opaque bearer.
    GcpAccessToken,
    /// A JSON Web Token (`eyJ…`). Decode the header to route (Azure AD, etc.).
    Jwt,
    /// Shape not recognized — fall back to a provider picker.
    Unknown,
}

/// How sure the prefix match is. Drives the validation strategy: a single
/// [`Confidence::High`] candidate is validated alone; [`Confidence::Ambiguous`]
/// or [`Confidence::Low`] candidates are validated in parallel, first 2xx wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// No prefix; a shape-only heuristic (e.g. a bare 32-hex string → Azure).
    Low,
    /// A shared/ambiguous prefix (bare `sk-` → OpenAI *or* DeepSeek).
    Ambiguous,
    /// A unique, unambiguous prefix (`sk-ant-`, `xai-`, `gsk_`).
    High,
}

/// A single ranked provider candidate. `slug` matches the provider-catalog id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderGuess {
    pub slug: &'static str,
    pub confidence: Confidence,
}

impl ProviderGuess {
    const fn new(slug: &'static str, confidence: Confidence) -> Self {
        Self { slug, confidence }
    }
}

/// The result of fingerprinting a pasted credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// The cleaned credential: surrounding quotes, an `export NAME=` wrapper,
    /// and a trailing `# comment` are all stripped. This is what gets validated
    /// and stored — never the raw paste.
    pub normalized: String,
    /// What kind of credential the shape indicates.
    pub kind: CredentialKind,
    /// Ranked provider candidates, most-specific first. Empty ⇒ unknown shape.
    pub candidates: Vec<ProviderGuess>,
    /// A provider slug inferred from an `export NAME=…` variable name, if the
    /// paste was an env-var line (e.g. `ANTHROPIC_API_KEY` ⇒ `anthropic`). Used
    /// to break ties in the ambiguous bucket.
    pub env_var_hint: Option<&'static str>,
    /// True when the shape alone cannot be validated and a guided form is
    /// required: AWS (secret + region), Azure (resource endpoint), GCP (project).
    pub needs_completion: bool,
}

impl Fingerprint {
    /// `true` when exactly one high-confidence candidate exists — validate it
    /// alone. Otherwise the caller should probe the candidates in parallel.
    pub fn is_unambiguous(&self) -> bool {
        self.candidates.len() == 1 && self.candidates[0].confidence == Confidence::High
    }

    /// The best single guess, if any (candidates are kept in rank order).
    pub fn best(&self) -> Option<&ProviderGuess> {
        self.candidates.first()
    }
}

/// Longest-prefix-first table of unambiguous credential prefixes. Order matters:
/// `sk-ant-` must be tested before `sk-`, `sk-or-` before `sk-`, etc. Each entry
/// is `(prefix, slug)` and yields a [`Confidence::High`] bearer candidate.
const UNIQUE_PREFIXES: &[(&str, &str)] = &[
    ("sk-ant-", "anthropic"),
    ("sk-flux-", "flux-router"),
    ("sk-proj-", "openai"),
    ("sk-svcacct-", "openai"),
    ("sk-admin-", "openai"),
    ("sk-or-v1-", "openrouter"),
    ("sk-or-", "openrouter"),
    ("csk-", "cerebras"),
    ("gsk_", "groq"),
    ("xai-", "xai"),
    ("pplx-", "perplexity"),
    ("r8_", "replicate"),
    ("hf_", "huggingface"),
    ("AIza", "gemini"),
];

/// Map a recognised credential env-var name to a provider slug. Used both as a
/// tie-breaker hint and (for `export FOO=bar` pastes) to bias ranking.
fn slug_for_env_var(name: &str) -> Option<&'static str> {
    let slug = match name {
        "ANTHROPIC_API_KEY" => "anthropic",
        "FLUX_API_KEY" => "flux-router",
        "OPENAI_API_KEY" => "openai",
        "OPENROUTER_API_KEY" => "openrouter",
        "GROQ_API_KEY" => "groq",
        "XAI_API_KEY" => "xai",
        "DEEPSEEK_API_KEY" => "deepseek",
        "MISTRAL_API_KEY" => "mistral",
        "PERPLEXITY_API_KEY" => "perplexity",
        "CEREBRAS_API_KEY" => "cerebras",
        "TOGETHER_API_KEY" => "together",
        "FIREWORKS_API_KEY" => "fireworks-ai",
        "COHERE_API_KEY" => "cohere",
        "REPLICATE_API_TOKEN" => "replicate",
        "GEMINI_API_KEY" | "GOOGLE_API_KEY" => "gemini",
        "HF_TOKEN" | "HUGGINGFACE_API_KEY" | "HUGGING_FACE_HUB_TOKEN" => "huggingface",
        _ => return None,
    };
    Some(slug)
}

/// Fingerprint a pasted credential into ranked provider candidates.
///
/// Never makes a network call. The returned [`Fingerprint::candidates`] are a
/// hypothesis to be confirmed by a live probe; an empty list means "unknown
/// shape — show the provider picker".
pub fn fingerprint_key(raw: &str) -> Fingerprint {
    let (normalized, env_var_hint) = normalize(raw);

    // 1. Structured credential: a GCP service-account JSON blob.
    if let Some(kind) = json_credential_kind(&normalized) {
        let needs_completion = kind == CredentialKind::GcpServiceAccount;
        return Fingerprint {
            normalized,
            kind,
            candidates: vec![ProviderGuess::new("vertex", Confidence::High)],
            env_var_hint,
            needs_completion,
        };
    }

    // 2. Non-bearer / opaque token shapes that short-circuit to a guided path.
    if let Some(fp) = non_bearer_shape(&normalized, env_var_hint) {
        return fp;
    }

    // 3. Unique bearer prefixes (longest-first).
    for (prefix, slug) in UNIQUE_PREFIXES {
        if normalized.starts_with(prefix) {
            return Fingerprint {
                normalized,
                kind: CredentialKind::BearerKey,
                candidates: vec![ProviderGuess::new(slug, Confidence::High)],
                env_var_hint,
                needs_completion: false,
            };
        }
    }

    // 4. Ambiguous bare `sk-` bucket: OpenAI legacy vs DeepSeek (and other
    //    OpenAI-compatible relays). Rank by the env-var hint when present.
    if normalized.starts_with("sk-") {
        let mut candidates = vec![
            ProviderGuess::new("openai", Confidence::Ambiguous),
            ProviderGuess::new("deepseek", Confidence::Ambiguous),
        ];
        rank_by_hint(&mut candidates, env_var_hint);
        return Fingerprint {
            normalized,
            kind: CredentialKind::BearerKey,
            candidates,
            env_var_hint,
            needs_completion: false,
        };
    }

    // 5. No-prefix shapes: trust the env-var hint if we have one, else low-
    //    confidence heuristics (a bare 32-hex string is Azure-shaped).
    if let Some(hint) = env_var_hint {
        return Fingerprint {
            normalized,
            kind: CredentialKind::BearerKey,
            candidates: vec![ProviderGuess::new(hint, Confidence::Ambiguous)],
            env_var_hint,
            needs_completion: false,
        };
    }

    if is_hex(&normalized) && normalized.len() == 32 {
        // Azure OpenAI keys are 32-hex; they still need a resource endpoint.
        return Fingerprint {
            normalized,
            kind: CredentialKind::BearerKey,
            candidates: vec![ProviderGuess::new("azure-openai", Confidence::Low)],
            env_var_hint,
            needs_completion: true,
        };
    }

    // 6. Unknown shape — caller shows the provider picker.
    Fingerprint {
        normalized,
        kind: CredentialKind::Unknown,
        candidates: Vec::new(),
        env_var_hint,
        needs_completion: false,
    }
}

/// Detect opaque-token / non-bearer shapes that can't be validated from the
/// pasted string alone.
fn non_bearer_shape(s: &str, env_var_hint: Option<&'static str>) -> Option<Fingerprint> {
    // AWS Bedrock bearer API key — a usable bearer, no SigV4 needed.
    if s.starts_with("ABSK") || s.starts_with("bedrock-api-key-") {
        return Some(Fingerprint {
            normalized: s.to_string(),
            kind: CredentialKind::BedrockApiKey,
            candidates: vec![ProviderGuess::new("bedrock", Confidence::High)],
            env_var_hint,
            needs_completion: false,
        });
    }
    // AWS classic access-key id — incomplete credential (needs secret + region).
    if let Some(rest) = strip_aws_access_key_prefix(s) {
        // AKIA/ASIA/... + 16 base32-ish chars = 20 total.
        if rest.len() == 16 && rest.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Some(Fingerprint {
                normalized: s.to_string(),
                kind: CredentialKind::AwsAccessKeyId,
                candidates: vec![ProviderGuess::new("bedrock", Confidence::High)],
                env_var_hint,
                needs_completion: true,
            });
        }
    }
    // GCP OAuth access token — ephemeral opaque bearer (NOT a JWT, don't decode).
    if s.starts_with("ya29.") {
        return Some(Fingerprint {
            normalized: s.to_string(),
            kind: CredentialKind::GcpAccessToken,
            candidates: vec![ProviderGuess::new("vertex", Confidence::High)],
            env_var_hint,
            needs_completion: true,
        });
    }
    // JWT — decode the header to route (Azure AD / GCP id-token). Routing is the
    // caller's job; here we just classify the kind and leave candidates empty.
    if s.starts_with("eyJ") && s.matches('.').count() == 2 {
        return Some(Fingerprint {
            normalized: s.to_string(),
            kind: CredentialKind::Jwt,
            candidates: Vec::new(),
            env_var_hint,
            needs_completion: true,
        });
    }
    None
}

/// AWS access-key id prefixes (`AKIA` long-term, `ASIA` temporary session, plus
/// the rarer `ABIA`/`ACCA`). Returns the remainder after the 4-char prefix.
fn strip_aws_access_key_prefix(s: &str) -> Option<&str> {
    for p in ["AKIA", "ASIA", "ABIA", "ACCA"] {
        if let Some(rest) = s.strip_prefix(p) {
            return Some(rest);
        }
    }
    None
}

/// Classify a JSON credential blob. Currently recognises GCP service-account
/// JSON; returns `None` for non-JSON input.
fn json_credential_kind(s: &str) -> Option<CredentialKind> {
    let t = s.trim_start();
    if !t.starts_with('{') {
        return None;
    }
    // Cheap structural check — avoid a full parse on a hot paste path.
    if t.contains("\"type\"") && t.contains("\"service_account\"") {
        return Some(CredentialKind::GcpServiceAccount);
    }
    None
}

/// Move the candidate whose slug matches the env-var hint to the front.
fn rank_by_hint(candidates: &mut [ProviderGuess], hint: Option<&'static str>) {
    if let Some(hint) = hint
        && let Some(pos) = candidates.iter().position(|c| c.slug == hint)
    {
        candidates.swap(0, pos);
    }
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Clean a pasted credential. Handles the messy reality of what people paste:
/// surrounding whitespace, wrapping quotes, a full `export NAME="value"` shell
/// line (the variable name becomes a provider hint), and a trailing `# comment`.
fn normalize(raw: &str) -> (String, Option<&'static str>) {
    let mut s = raw.trim();

    // `export NAME=value` or `NAME=value` — extract the value, keep the hint.
    let mut hint = None;
    if let Some((name, value)) = split_assignment(s) {
        hint = slug_for_env_var(name);
        s = value;
    }

    let s = strip_quotes_and_comment(s);
    (s.to_string(), hint)
}

/// Parse a `[export ]NAME=VALUE` line. Returns `(name, value)` only when the
/// left-hand side is a syntactically valid shell variable name.
fn split_assignment(s: &str) -> Option<(&str, &str)> {
    let body = s.strip_prefix("export ").map(str::trim_start).unwrap_or(s);
    let (name, value) = body.split_once('=')?;
    let name = name.trim_end();
    if is_valid_env_name(name) {
        Some((name, value.trim()))
    } else {
        None
    }
}

fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Strip matched surrounding quotes, then a trailing whitespace-delimited
/// `# comment`. If the value is quoted, the comment outside the quotes is
/// dropped and quote contents are taken verbatim.
fn strip_quotes_and_comment(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\''] {
        if let Some(rest) = s.strip_prefix(q)
            && let Some(end) = rest.find(q)
        {
            return &rest[..end];
        }
    }
    // Unquoted: drop a trailing ` # comment` (space-delimited so we never eat a
    // '#' that is part of the value).
    match s.split_once(" #") {
        Some((before, _)) => before.trim_end(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slugs(fp: &Fingerprint) -> Vec<&str> {
        fp.candidates.iter().map(|c| c.slug).collect()
    }

    #[test]
    fn unique_prefixes_resolve_to_one_high_candidate() {
        let cases = [
            ("sk-ant-api03-AAbb1234567890", "anthropic"),
            ("sk-proj-abcDEF123", "openai"),
            ("sk-svcacct-xyz", "openai"),
            ("sk-or-v1-deadbeef", "openrouter"),
            ("csk-1234567890", "cerebras"),
            ("gsk_abcd1234", "groq"),
            ("xai-abcd1234", "xai"),
            ("pplx-abcd1234", "perplexity"),
            ("r8_abcd1234", "replicate"),
            ("hf_abcd1234", "huggingface"),
            ("AIzaSyA1234567890abcdefghijklmnopqrst", "gemini"),
        ];
        for (key, want) in cases {
            let fp = fingerprint_key(key);
            assert!(
                fp.is_unambiguous(),
                "{key} should be unambiguous, got {:?}",
                fp.candidates
            );
            assert_eq!(fp.best().unwrap().slug, want, "wrong provider for {key}");
            assert_eq!(fp.kind, CredentialKind::BearerKey);
            assert!(!fp.needs_completion);
        }
    }

    #[test]
    fn anthropic_wins_over_bare_sk_bucket() {
        // The longest-prefix rule must put sk-ant- ahead of the sk- fallback.
        let fp = fingerprint_key("sk-ant-api03-zzz");
        assert_eq!(slugs(&fp), vec!["anthropic"]);
    }

    #[test]
    fn bare_sk_is_ambiguous_openai_and_deepseek() {
        let fp = fingerprint_key("sk-0123456789abcdef0123456789abcdef");
        assert_eq!(fp.kind, CredentialKind::BearerKey);
        assert_eq!(slugs(&fp), vec!["openai", "deepseek"]);
        assert!(!fp.is_unambiguous());
        assert!(
            fp.candidates
                .iter()
                .all(|c| c.confidence == Confidence::Ambiguous)
        );
    }

    #[test]
    fn env_var_line_is_unwrapped_and_hints_ranking() {
        let fp = fingerprint_key("export DEEPSEEK_API_KEY=\"sk-abcdef123456\"");
        assert_eq!(fp.normalized, "sk-abcdef123456");
        assert_eq!(fp.env_var_hint, Some("deepseek"));
        // The hint pulls deepseek ahead of openai in the ambiguous bucket.
        assert_eq!(slugs(&fp), vec!["deepseek", "openai"]);
    }

    #[test]
    fn plain_assignment_without_export_is_unwrapped() {
        let fp = fingerprint_key("ANTHROPIC_API_KEY=sk-ant-api03-xyz");
        assert_eq!(fp.normalized, "sk-ant-api03-xyz");
        assert_eq!(fp.env_var_hint, Some("anthropic"));
        assert_eq!(slugs(&fp), vec!["anthropic"]);
    }

    #[test]
    fn surrounding_quotes_and_whitespace_are_stripped() {
        assert_eq!(
            fingerprint_key("  'sk-ant-api03-xyz'  ").normalized,
            "sk-ant-api03-xyz"
        );
        assert_eq!(fingerprint_key("\"xai-abc\"").normalized, "xai-abc");
    }

    #[test]
    fn trailing_comment_is_dropped() {
        let fp = fingerprint_key("gsk_livekey123  # my groq key");
        assert_eq!(fp.normalized, "gsk_livekey123");
        assert_eq!(slugs(&fp), vec!["groq"]);
    }

    #[test]
    fn hash_inside_value_is_preserved() {
        // No space before '#', so it is part of the value, not a comment.
        let fp = fingerprint_key("sk-ant-api03-ab#cd");
        assert_eq!(fp.normalized, "sk-ant-api03-ab#cd");
    }

    #[test]
    fn aws_access_key_needs_completion() {
        let fp = fingerprint_key("AKIAIOSFODNN7EXAMPLE");
        assert_eq!(fp.kind, CredentialKind::AwsAccessKeyId);
        assert_eq!(slugs(&fp), vec!["bedrock"]);
        assert!(
            fp.needs_completion,
            "a lone access-key id can't be validated alone"
        );
    }

    #[test]
    fn aws_session_key_prefix_recognised() {
        let fp = fingerprint_key("ASIAIOSFODNN7EXAMPLE");
        assert_eq!(fp.kind, CredentialKind::AwsAccessKeyId);
    }

    #[test]
    fn bedrock_bearer_key_is_complete() {
        let fp = fingerprint_key("ABSKabcdef0123456789");
        assert_eq!(fp.kind, CredentialKind::BedrockApiKey);
        assert_eq!(slugs(&fp), vec!["bedrock"]);
        assert!(!fp.needs_completion);
    }

    #[test]
    fn gcp_service_account_json_detected() {
        let blob = r#"{ "type": "service_account", "project_id": "demo", "private_key": "x" }"#;
        let fp = fingerprint_key(blob);
        assert_eq!(fp.kind, CredentialKind::GcpServiceAccount);
        assert_eq!(slugs(&fp), vec!["vertex"]);
        assert!(fp.needs_completion);
    }

    #[test]
    fn gcp_access_token_is_opaque_not_jwt() {
        let fp = fingerprint_key("ya29.a0ARrdaM-opaque-token");
        assert_eq!(fp.kind, CredentialKind::GcpAccessToken);
        assert_eq!(slugs(&fp), vec!["vertex"]);
    }

    #[test]
    fn jwt_is_classified_without_candidates() {
        let fp = fingerprint_key("eyJhbGciOiJI.eyJzdWIiOiIx.sigpart");
        assert_eq!(fp.kind, CredentialKind::Jwt);
        assert!(
            fp.candidates.is_empty(),
            "JWT routing is decided by the caller"
        );
        assert!(fp.needs_completion);
    }

    #[test]
    fn bare_32_hex_is_low_confidence_azure_needing_endpoint() {
        let fp = fingerprint_key("0123456789abcdef0123456789abcdef");
        assert_eq!(slugs(&fp), vec!["azure-openai"]);
        assert_eq!(fp.best().unwrap().confidence, Confidence::Low);
        assert!(fp.needs_completion);
    }

    #[test]
    fn no_prefix_with_env_hint_uses_the_hint() {
        let fp = fingerprint_key("MISTRAL_API_KEY=abcdef0123456789ABCDEF0123");
        assert_eq!(fp.env_var_hint, Some("mistral"));
        assert_eq!(slugs(&fp), vec!["mistral"]);
    }

    #[test]
    fn unknown_shape_yields_no_candidates() {
        let fp = fingerprint_key("just-some-random-text");
        assert_eq!(fp.kind, CredentialKind::Unknown);
        assert!(fp.candidates.is_empty());
        assert!(fp.best().is_none());
    }

    #[test]
    fn empty_input_is_unknown_not_a_panic() {
        let fp = fingerprint_key("   ");
        assert_eq!(fp.normalized, "");
        assert_eq!(fp.kind, CredentialKind::Unknown);
        assert!(fp.candidates.is_empty());
    }

    #[test]
    fn google_api_key_env_alias_maps_to_gemini() {
        let fp = fingerprint_key("GOOGLE_API_KEY=AIzaSyXXXX");
        // Both the env hint and the AIza prefix agree.
        assert_eq!(fp.env_var_hint, Some("gemini"));
        assert_eq!(slugs(&fp), vec!["gemini"]);
    }
}
