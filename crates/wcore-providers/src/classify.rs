//! 3-tier failover classifier — ported from openclaw MIT (c) Peter Steinberger 2025.
//!
//! Classifies a provider error into a FailoverReason using three signal sources
//! in precedence order:
//!   1. HTTP status code (most authoritative — 401/403/429/etc)
//!   2. Body / error message text (pattern matching on known phrases)
//!   3. SDK / vendor-specific error code (provider-specific identifiers)
//!
//! If no signal fires, returns FailoverReason::Unknown.

use crate::{FailoverReason, ProviderError};

/// Classify a provider error into a FailoverReason.
///
/// Signal precedence: status > body > sdk_code > default Unknown.
pub fn classify_failover(
    err: &ProviderError,
    http_status: Option<u16>,
    body_text: Option<&str>,
    sdk_code: Option<&str>,
) -> FailoverReason {
    // Tier 1: HTTP status
    if let Some(s) = http_status
        && let Some(r) = classify_by_status(s)
    {
        return r;
    }
    // Tier 2: body text patterns
    if let Some(b) = body_text
        && let Some(r) = classify_by_body(b)
    {
        return r;
    }
    // Tier 3: SDK code
    if let Some(c) = sdk_code
        && let Some(r) = classify_by_sdk_code(c)
    {
        return r;
    }
    // Fallback: inspect the ProviderError shape
    classify_by_provider_error(err)
}

fn classify_by_status(status: u16) -> Option<FailoverReason> {
    // openclaw semantic split: 503/529 are explicit "overloaded" signals; other
    // 5xx codes (500/502/504) are treated as transient timeouts so failover
    // does a fast retry / try the next provider rather than a long backoff.
    match status {
        401 => Some(FailoverReason::Auth),
        403 => Some(FailoverReason::AuthPermanent),
        404 => Some(FailoverReason::ModelNotFound),
        408 => Some(FailoverReason::Timeout),
        429 => Some(FailoverReason::RateLimit),
        402 => Some(FailoverReason::Billing),
        413 => Some(FailoverReason::ContextOverflow),
        // Anthropic + OpenAI use 529 for "overloaded"; 503 is the standard "service unavailable".
        529 | 503 => Some(FailoverReason::Overloaded),
        // Other 5xx — server error; treat as Timeout per openclaw recovery semantics.
        s if (500..=599).contains(&s) => Some(FailoverReason::Timeout),
        // 400 with no body classification — format error
        400 => Some(FailoverReason::Format),
        _ => None,
    }
}

fn classify_by_body(body: &str) -> Option<FailoverReason> {
    let b = body.to_ascii_lowercase();
    // Order matters — more specific phrases first
    if b.contains("session expired") || b.contains("session has expired") {
        return Some(FailoverReason::SessionExpired);
    }
    // Billing must key on GENUINE credit/quota/payment signals, never on the
    // bare word "billing" — and never on "purchase credits". Anthropic appends a
    // "Please go to Plans & Billing to upgrade or purchase credits." remediation
    // tail to a wide range of error bodies — including non-billing ones — so any
    // match on a substring of that tail (`contains("billing")`, but ALSO
    // `contains("purchase credits")`) mis-classifies an unrelated failure as
    // out-of-credit and surfaces a false "balance is too low" to a user whose
    // balance is fine, pinning the key into a permanent cooldown (issue #329).
    // Match only the unambiguous signals: insufficient quota, an explicit
    // credit-balance exhaustion, payment-required, or the "billing plan"
    // entitlement phrasing. A genuine Anthropic credit error always carries
    // "credit balance is too low", so dropping "purchase credits" loses no
    // real-credit coverage.
    if b.contains("insufficient_quota")
        || b.contains("insufficient quota")
        || b.contains("credit balance is too low")
        || b.contains("billing plan")
        || b.contains("payment required")
    {
        return Some(FailoverReason::Billing);
    }
    if b.contains("invalid api key") || b.contains("invalid_api_key") || b.contains("unauthorized")
    {
        return Some(FailoverReason::Auth);
    }
    if b.contains("permission denied") || b.contains("forbidden") {
        return Some(FailoverReason::AuthPermanent);
    }
    if b.contains("rate limit") || b.contains("rate_limit") || b.contains("too many requests") {
        return Some(FailoverReason::RateLimit);
    }
    // ordering matters — billing/quota messages often contain "not allowed"
    // phrasing (e.g. "you are not allowed to use this model on your plan"),
    // which would otherwise misclassify as AuthPermanent. Billing + RateLimit
    // arms above must run first to absorb those bodies.
    if b.contains("not allowed") {
        return Some(FailoverReason::AuthPermanent);
    }
    if b.contains("overloaded") || b.contains("server is busy") || b.contains("capacity") {
        return Some(FailoverReason::Overloaded);
    }
    if b.contains("model not found") || b.contains("model_not_found") || b.contains("unknown model")
    {
        return Some(FailoverReason::ModelNotFound);
    }
    // Context overflow MUST be checked before generic Format patterns —
    // openclaw treats this as a distinct recovery class (compact / route to
    // larger-context model rather than swap provider).
    if b.contains("context length exceeded")
        || b.contains("context_length_exceeded")
        || b.contains("prompt is too long")
        || b.contains("prompt too long")
        || b.contains("context window")
        || b.contains("request_too_large")
        || b.contains("max_tokens_to_sample")
        || b.contains("token limit exceeded")
    {
        return Some(FailoverReason::ContextOverflow);
    }
    if b.contains("timed out") || b.contains("timeout") || b.contains("deadline exceeded") {
        return Some(FailoverReason::Timeout);
    }
    if b.contains("invalid request")
        || b.contains("malformed")
        || b.contains("bad request")
        || b.contains("invalid json")
        || b.contains("invalid_argument")
    {
        return Some(FailoverReason::Format);
    }
    None
}

fn classify_by_sdk_code(code: &str) -> Option<FailoverReason> {
    let c = code.to_ascii_lowercase();
    match c.as_str() {
        // OpenAI shapes
        "insufficient_quota" => Some(FailoverReason::Billing),
        "invalid_api_key" | "invalid_authorization" => Some(FailoverReason::Auth),
        "rate_limit_exceeded" => Some(FailoverReason::RateLimit),
        // openclaw treats context_length_exceeded as its own recovery class
        // (compact / pick larger-context model), NOT a Format error.
        "context_length_exceeded" => Some(FailoverReason::ContextOverflow),
        "model_not_found" => Some(FailoverReason::ModelNotFound),
        // Anthropic shapes
        "authentication_error" => Some(FailoverReason::Auth),
        "permission_error" => Some(FailoverReason::AuthPermanent),
        "rate_limit_error" => Some(FailoverReason::RateLimit),
        "overloaded_error" => Some(FailoverReason::Overloaded),
        "invalid_request_error" => Some(FailoverReason::Format),
        // AWS Bedrock shapes
        "throttlingexception" => Some(FailoverReason::RateLimit),
        "modelnotreadyexception" => Some(FailoverReason::Overloaded),
        "validationexception" => Some(FailoverReason::Format),
        // Google Vertex / Gemini shapes
        "resource_exhausted" => Some(FailoverReason::RateLimit),
        "unauthenticated" => Some(FailoverReason::Auth),
        "permission_denied" => Some(FailoverReason::AuthPermanent),
        "deadline_exceeded" => Some(FailoverReason::Timeout),
        "failed_precondition" => Some(FailoverReason::Format),
        "unavailable" => Some(FailoverReason::Overloaded),
        // OS-level errno family (openclaw classifies authoritatively as timeout)
        "etimedout" | "econnreset" | "econnaborted" | "ehostunreach" | "eai_again" => {
            Some(FailoverReason::Timeout)
        }
        // Generic
        "auth_permanent" => Some(FailoverReason::AuthPermanent),
        "session_expired" => Some(FailoverReason::SessionExpired),
        _ => None,
    }
}

fn classify_by_provider_error(err: &ProviderError) -> FailoverReason {
    match err {
        ProviderError::RateLimited { .. } => FailoverReason::RateLimit,
        // PromptTooLong is the Rust-level context-overflow signal: recovery is
        // compact/route-to-larger-context-model, NOT swap provider. Per the
        // FailoverReason taxonomy doc at the top of this module.
        ProviderError::PromptTooLong(_) => FailoverReason::ContextOverflow,
        // Flux 409 context_overflow — same recovery as PromptTooLong: compact
        // and retry on the same provider, never swap.
        ProviderError::ContextOverflow { .. } => FailoverReason::ContextOverflow,
        ProviderError::Connection(_) => FailoverReason::Timeout,
        ProviderError::Parse(_) => FailoverReason::Format,
        ProviderError::Http(_) => FailoverReason::Timeout,
        // Egress: a transport failure classifies like Http (Timeout); a policy
        // Denied is not a productive failover target.
        ProviderError::Egress(e) => match e {
            wcore_egress::EgressError::Transport(_) => FailoverReason::Timeout,
            wcore_egress::EgressError::Denied(_) => FailoverReason::Unknown,
            // An over-cap response body is not a productive failover target.
            wcore_egress::EgressError::BodyTooLarge { .. } => FailoverReason::Unknown,
        },
        ProviderError::Api { status, .. } => {
            classify_by_status(*status).unwrap_or(FailoverReason::Unknown)
        }
        // Missing credential is a terminal config error, not a productive
        // failover target — another provider has its own (also-required) key.
        ProviderError::MissingApiKey => FailoverReason::Unknown,
        // Flux capability / entitlement gates (402): terminal — not a
        // productive failover target; the typed message is surfaced to the user.
        ProviderError::PremiumLocked { .. }
        | ProviderError::UpgradeRequired { .. }
        | ProviderError::SpendCeilingUnresolved { .. } => FailoverReason::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_err() -> ProviderError {
        ProviderError::Api {
            status: 0,
            message: "dummy".into(),
        }
    }

    // ---- Tier 1: HTTP status ----

    #[test]
    fn status_400_format() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(400), None, None),
            FailoverReason::Format
        );
    }

    #[test]
    fn status_401_auth() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(401), None, None),
            FailoverReason::Auth
        );
    }

    #[test]
    fn status_402_billing() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(402), None, None),
            FailoverReason::Billing
        );
    }

    #[test]
    fn status_403_auth_permanent() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(403), None, None),
            FailoverReason::AuthPermanent
        );
    }

    #[test]
    fn status_404_model_not_found() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(404), None, None),
            FailoverReason::ModelNotFound
        );
    }

    #[test]
    fn status_408_timeout() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(408), None, None),
            FailoverReason::Timeout
        );
    }

    #[test]
    fn status_429_rate_limit() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(429), None, None),
            FailoverReason::RateLimit
        );
    }

    #[test]
    fn status_500_timeout() {
        // openclaw recovery semantics: 500/502/504 -> Timeout (fast retry / try next provider).
        // Only 503 and 529 are explicit Overloaded signals.
        assert_eq!(
            classify_failover(&dummy_err(), Some(500), None, None),
            FailoverReason::Timeout
        );
    }

    #[test]
    fn status_502_timeout() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(502), None, None),
            FailoverReason::Timeout
        );
    }

    #[test]
    fn status_504_timeout() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(504), None, None),
            FailoverReason::Timeout
        );
    }

    #[test]
    fn status_503_overloaded() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(503), None, None),
            FailoverReason::Overloaded
        );
    }

    #[test]
    fn status_529_overloaded() {
        assert_eq!(
            classify_failover(&dummy_err(), Some(529), None, None),
            FailoverReason::Overloaded
        );
    }

    #[test]
    fn status_413_context_overflow() {
        // HTTP 413 Payload Too Large -- the wire signal for context overflow.
        assert_eq!(
            classify_failover(&dummy_err(), Some(413), None, None),
            FailoverReason::ContextOverflow
        );
    }

    #[test]
    fn status_unrecognized_falls_through() {
        // 418 is not classified — should fall through to provider-error tier
        // dummy_err is Api{status: 0}, classify_by_status(0) -> None, so Unknown
        assert_eq!(
            classify_failover(&dummy_err(), Some(418), None, None),
            FailoverReason::Unknown
        );
    }

    // ---- Tier 2: Body text ----

    #[test]
    fn body_session_expired() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("Your session expired"), None),
            FailoverReason::SessionExpired
        );
    }

    #[test]
    fn body_session_has_expired_variant() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("the session has expired"), None),
            FailoverReason::SessionExpired
        );
    }

    #[test]
    fn body_billing_insufficient_quota() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("insufficient_quota for org"), None),
            FailoverReason::Billing
        );
    }

    #[test]
    fn body_billing_payment_required() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("payment required"), None),
            FailoverReason::Billing
        );
    }

    /// Issue #329: a genuine Anthropic low-credit 400 body must still classify
    /// as Billing. The signal is the credit-balance phrasing, NOT the trailing
    /// "Plans & Billing" link.
    #[test]
    fn body_billing_anthropic_credit_balance_too_low() {
        let anthropic_body = "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\
            \"message\":\"Your credit balance is too low to access the Anthropic API. \
            Please go to Plans & Billing to upgrade or purchase credits.\"}}";
        assert_eq!(
            classify_failover(&dummy_err(), None, Some(anthropic_body), None),
            FailoverReason::Billing
        );
    }

    /// Issue #329 regression: an Anthropic error that is NOT about credits but
    /// carries the standard remediation tail ("Please go to Plans & Billing to
    /// upgrade or purchase credits.") must NOT be classified as Billing (which
    /// would surface a false "balance is too low" and pin the key into a
    /// permanent cooldown). The tail contains the substring "purchase credits",
    /// so keying Billing on "purchase credits" would re-introduce the exact #329
    /// bug; this test pins that "purchase credits" is NOT a Billing signal. The
    /// body deliberately omits "credit balance is too low" — the only genuine
    /// credit signal — so a correct classifier falls through to its real reason.
    #[test]
    fn body_non_billing_error_with_billing_link_is_not_billing() {
        let body = "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\
            \"message\":\"This model requires a specific feature. \
            Please go to Plans & Billing to upgrade or purchase credits.\"}}";
        assert_ne!(
            classify_failover(&dummy_err(), None, Some(body), None),
            FailoverReason::Billing,
            "a non-credit error that merely carries the standard 'purchase \
             credits' remediation tail must not be classified as out-of-credit"
        );
    }

    #[test]
    fn body_auth_invalid_api_key() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("Invalid API key provided"), None),
            FailoverReason::Auth
        );
    }

    #[test]
    fn body_auth_unauthorized() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("request was unauthorized"), None),
            FailoverReason::Auth
        );
    }

    #[test]
    fn body_auth_permanent_forbidden() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("Forbidden"), None),
            FailoverReason::AuthPermanent
        );
    }

    #[test]
    fn body_auth_permanent_permission_denied() {
        assert_eq!(
            classify_failover(
                &dummy_err(),
                None,
                Some("permission denied on resource"),
                None
            ),
            FailoverReason::AuthPermanent
        );
    }

    /// R3-C1: regression test for the R1 ordering fix in
    /// `classify_by_body`. Billing/quota messages frequently include
    /// "not allowed" phrasing (e.g. "you are not allowed to use this
    /// model on your billing plan"). The billing arm runs BEFORE the
    /// "not allowed" arm so these composite messages resolve to
    /// `Billing` rather than `AuthPermanent`. A future reorder that
    /// moves the "not allowed" check above the billing check would
    /// silently regress this; this test pins the ordering.
    #[test]
    fn body_billing_with_not_allowed_phrasing_resolves_to_billing() {
        assert_eq!(
            classify_failover(
                &dummy_err(),
                None,
                Some("you are not allowed to use this model on your billing plan"),
                None
            ),
            FailoverReason::Billing
        );
    }

    #[test]
    fn body_rate_limit() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("rate limit reached"), None),
            FailoverReason::RateLimit
        );
    }

    #[test]
    fn body_too_many_requests() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("Too Many Requests"), None),
            FailoverReason::RateLimit
        );
    }

    #[test]
    fn body_overloaded() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("server is overloaded"), None),
            FailoverReason::Overloaded
        );
    }

    #[test]
    fn body_model_not_found() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("model not found: gpt-x"), None),
            FailoverReason::ModelNotFound
        );
    }

    #[test]
    fn body_unknown_model() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("Unknown model claude-z"), None),
            FailoverReason::ModelNotFound
        );
    }

    #[test]
    fn body_timeout() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("request timed out"), None),
            FailoverReason::Timeout
        );
    }

    #[test]
    fn body_deadline_exceeded() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("deadline exceeded"), None),
            FailoverReason::Timeout
        );
    }

    #[test]
    fn body_format_malformed() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("malformed request"), None),
            FailoverReason::Format
        );
    }

    #[test]
    fn body_format_invalid_json() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("invalid json payload"), None),
            FailoverReason::Format
        );
    }

    #[test]
    fn body_case_insensitive_match() {
        assert_eq!(
            classify_failover(&dummy_err(), None, Some("RATE LIMIT EXCEEDED"), None),
            FailoverReason::RateLimit
        );
    }

    // ---- Tier 3: SDK code ----

    #[test]
    fn sdk_code_openai_insufficient_quota() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("insufficient_quota")),
            FailoverReason::Billing
        );
    }

    #[test]
    fn sdk_code_openai_invalid_api_key() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("invalid_api_key")),
            FailoverReason::Auth
        );
    }

    #[test]
    fn sdk_code_openai_rate_limit_exceeded() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("rate_limit_exceeded")),
            FailoverReason::RateLimit
        );
    }

    #[test]
    fn sdk_code_openai_context_length_exceeded() {
        // openclaw recovery class: context_overflow recovers via compaction or
        // larger-context-model routing, not by swapping providers.
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("context_length_exceeded")),
            FailoverReason::ContextOverflow
        );
    }

    #[test]
    fn body_context_overflow_phrases() {
        for phrase in [
            "this prompt is too long for the model",
            "context length exceeded for claude-opus",
            "request exceeds context window",
            "request_too_large",
            "token limit exceeded",
        ] {
            assert_eq!(
                classify_failover(&dummy_err(), None, Some(phrase), None),
                FailoverReason::ContextOverflow,
                "expected ContextOverflow for body: {phrase}"
            );
        }
    }

    #[test]
    fn sdk_code_bedrock_throttling() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("ThrottlingException")),
            FailoverReason::RateLimit
        );
    }

    #[test]
    fn sdk_code_bedrock_model_not_ready() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("ModelNotReadyException")),
            FailoverReason::Overloaded
        );
    }

    #[test]
    fn sdk_code_vertex_resource_exhausted() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("RESOURCE_EXHAUSTED")),
            FailoverReason::RateLimit
        );
    }

    #[test]
    fn sdk_code_vertex_deadline_exceeded() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("DEADLINE_EXCEEDED")),
            FailoverReason::Timeout
        );
    }

    #[test]
    fn sdk_code_vertex_unavailable() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("UNAVAILABLE")),
            FailoverReason::Overloaded
        );
    }

    #[test]
    fn sdk_code_os_errno_family_classified_as_timeout() {
        for code in [
            "ETIMEDOUT",
            "ECONNRESET",
            "ECONNABORTED",
            "EHOSTUNREACH",
            "EAI_AGAIN",
        ] {
            assert_eq!(
                classify_failover(&dummy_err(), None, None, Some(code)),
                FailoverReason::Timeout,
                "expected Timeout for OS code: {code}"
            );
        }
    }

    #[test]
    fn sdk_code_anthropic_authentication_error() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("authentication_error")),
            FailoverReason::Auth
        );
    }

    #[test]
    fn sdk_code_anthropic_overloaded_error() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("overloaded_error")),
            FailoverReason::Overloaded
        );
    }

    #[test]
    fn sdk_code_anthropic_permission_error() {
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("permission_error")),
            FailoverReason::AuthPermanent
        );
    }

    #[test]
    fn sdk_code_unknown_falls_through_to_provider_error() {
        // dummy_err is Api{status: 0}; classify_by_status(0) is None → Unknown
        assert_eq!(
            classify_failover(&dummy_err(), None, None, Some("totally_made_up_code")),
            FailoverReason::Unknown
        );
    }

    // ---- Precedence ----

    #[test]
    fn status_wins_over_body() {
        // status=401 (Auth) vs body says rate limit — status must win
        assert_eq!(
            classify_failover(
                &dummy_err(),
                Some(401),
                Some("rate limit exceeded"),
                Some("rate_limit_exceeded"),
            ),
            FailoverReason::Auth
        );
    }

    #[test]
    fn body_wins_over_sdk_code_when_no_status() {
        // body says billing, sdk_code says auth — body wins because status is None
        assert_eq!(
            classify_failover(
                &dummy_err(),
                None,
                Some("insufficient quota"),
                Some("invalid_api_key"),
            ),
            FailoverReason::Billing
        );
    }

    // ---- Provider-error fallback ----

    #[test]
    fn fallback_rate_limited_provider_error() {
        let err = ProviderError::RateLimited {
            retry_after_ms: 1000,
        };
        assert_eq!(
            classify_failover(&err, None, None, None),
            FailoverReason::RateLimit
        );
    }

    #[test]
    fn fallback_connection_provider_error() {
        let err = ProviderError::Connection("dns failed".into());
        assert_eq!(
            classify_failover(&err, None, None, None),
            FailoverReason::Timeout
        );
    }

    #[test]
    fn fallback_parse_provider_error() {
        let err = ProviderError::Parse("bad sse".into());
        assert_eq!(
            classify_failover(&err, None, None, None),
            FailoverReason::Format
        );
    }

    #[test]
    fn fallback_prompt_too_long_provider_error() {
        // PromptTooLong must map to ContextOverflow so downstream policy
        // compacts / re-routes to a larger model instead of swapping provider.
        let err = ProviderError::PromptTooLong("too big".into());
        assert_eq!(
            classify_failover(&err, None, None, None),
            FailoverReason::ContextOverflow
        );
    }

    #[test]
    fn fallback_api_provider_error_uses_embedded_status() {
        // ProviderError::Api with status=429, no other signals → fallback uses
        // classify_by_status on the embedded status
        let err = ProviderError::Api {
            status: 429,
            message: "throttled".into(),
        };
        assert_eq!(
            classify_failover(&err, None, None, None),
            FailoverReason::RateLimit
        );
    }

    // ---- Default Unknown ----

    #[test]
    fn default_unknown_when_no_signal_fires() {
        // No status, body doesn't match any pattern, no code, dummy_err with status 0
        assert_eq!(
            classify_failover(
                &dummy_err(),
                None,
                Some("something completely random"),
                None
            ),
            FailoverReason::Unknown
        );
    }
}
