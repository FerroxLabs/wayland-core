//! FailoverReason taxonomy ported from openclaw MIT © Peter Steinberger 2025.
//!
//! 11-variant enum describes WHY a provider call failed in a way the failover
//! state machine can act on. Wraps the existing ProviderError as a `source`
//! so this addition is purely additive and does not break the ABI.
//!
//! `ContextOverflow` is distinct from `Format`: recovery for ContextOverflow is
//! "compact history or pick a larger-context model" rather than "swap provider"
//! (matches openclaw's separate `context_overflow` classification).

use crate::ProviderError;
use serde::{Deserialize, Serialize};

/// Why a provider call failed, taxonomized for failover decisions.
///
/// String representations match openclaw's TS string-union for cross-language
/// log/telemetry compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailoverReason {
    Auth,
    AuthPermanent,
    Format,
    RateLimit,
    Overloaded,
    Billing,
    Timeout,
    ModelNotFound,
    SessionExpired,
    /// Prompt/context exceeded the model's window. Recovery is to compact or
    /// route to a larger-context model — NOT to swap providers.
    ContextOverflow,
    Unknown,
}

impl FailoverReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::AuthPermanent => "auth_permanent",
            Self::Format => "format",
            Self::RateLimit => "rate_limit",
            Self::Overloaded => "overloaded",
            Self::Billing => "billing",
            Self::Timeout => "timeout",
            Self::ModelNotFound => "model_not_found",
            Self::SessionExpired => "session_expired",
            Self::ContextOverflow => "context_overflow",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for FailoverReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Rich error envelope wrapping a ProviderError with classification context.
///
/// Use this at adapter boundaries when the caller needs the structured reason
/// for failover policy decisions. The underlying `ProviderError` is preserved
/// via std::error::Error::source().
#[derive(Debug)]
pub struct FailoverError {
    pub reason: FailoverReason,
    pub provider: String,
    pub model: Option<String>,
    pub status: Option<u16>,
    pub code: Option<String>,
    pub source: ProviderError,
}

impl FailoverError {
    pub fn new(reason: FailoverReason, provider: impl Into<String>, source: ProviderError) -> Self {
        Self {
            reason,
            provider: provider.into(),
            model: None,
            status: None,
            code: None,
            source,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }
}

impl std::fmt::Display for FailoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} failover from {} ({}): {}",
            self.reason,
            self.provider,
            self.model.as_deref().unwrap_or("-"),
            self.source
        )
    }
}

impl std::error::Error for FailoverError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl From<ProviderError> for FailoverError {
    /// Legacy compatibility: wrap a bare ProviderError with reason=Unknown.
    /// Real classification happens in T1-A2 (classify::classify_failover).
    ///
    /// # Prefer
    ///
    /// Use [`wrap_provider_error`] when the provider name is known. This
    /// `From` impl exists for legacy compatibility and produces
    /// `provider: "unknown"`, which loses the per-provider attribution
    /// downstream consumers rely on.
    fn from(err: ProviderError) -> Self {
        Self {
            reason: FailoverReason::Unknown,
            provider: String::from("unknown"),
            model: None,
            status: None,
            code: None,
            source: err,
        }
    }
}

/// T1-A1b call-site migration helper: wrap a `ProviderError` into a
/// `FailoverError` envelope tagged with the provider name (and reason=Unknown
/// until T1-A2 classification lands).
///
/// Use this at chain.rs / resilient.rs / adapter boundaries when you want to
/// preserve the provider name in the envelope. The public ABI of
/// `LlmProvider::stream` is unchanged — this helper is additive.
pub fn wrap_provider_error(provider: impl Into<String>, err: ProviderError) -> FailoverError {
    FailoverError {
        reason: FailoverReason::Unknown,
        provider: provider.into(),
        model: None,
        status: None,
        code: None,
        source: err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_reasons_round_trip_serde() {
        let reasons = [
            FailoverReason::Auth,
            FailoverReason::AuthPermanent,
            FailoverReason::Format,
            FailoverReason::RateLimit,
            FailoverReason::Overloaded,
            FailoverReason::Billing,
            FailoverReason::Timeout,
            FailoverReason::ModelNotFound,
            FailoverReason::SessionExpired,
            FailoverReason::ContextOverflow,
            FailoverReason::Unknown,
        ];
        for r in reasons {
            let j = serde_json::to_string(&r).unwrap();
            let back: FailoverReason = serde_json::from_str(&j).unwrap();
            assert_eq!(r, back, "serde round-trip failed for {:?}", r);
        }
    }

    #[test]
    fn test_display_stable_strings() {
        assert_eq!(FailoverReason::Auth.to_string(), "auth");
        assert_eq!(FailoverReason::AuthPermanent.to_string(), "auth_permanent");
        assert_eq!(FailoverReason::ModelNotFound.to_string(), "model_not_found");
        assert_eq!(
            FailoverReason::SessionExpired.to_string(),
            "session_expired"
        );
        assert_eq!(FailoverReason::RateLimit.to_string(), "rate_limit");
    }

    #[test]
    fn test_serde_lowercase_snake_case() {
        let j = serde_json::to_string(&FailoverReason::AuthPermanent).unwrap();
        assert_eq!(j, "\"auth_permanent\"");
    }

    #[test]
    fn test_envelope_construction() {
        let perr = ProviderError::Api {
            status: 401,
            message: "bad token".into(),
        };
        let env = FailoverError::new(FailoverReason::Auth, "anthropic", perr)
            .with_model("claude-opus-4-7")
            .with_status(401);
        assert_eq!(env.reason, FailoverReason::Auth);
        assert_eq!(env.provider, "anthropic");
        assert_eq!(env.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(env.status, Some(401));
    }

    #[test]
    fn test_envelope_from_provider_error_legacy_compat() {
        let perr = ProviderError::Parse("malformed json".into());
        let env: FailoverError = perr.into();
        assert_eq!(env.reason, FailoverReason::Unknown);
        assert_eq!(env.provider, "unknown");
    }

    #[test]
    fn test_envelope_source_chain() {
        let perr = ProviderError::RateLimited {
            retry_after_ms: 1000,
        };
        let env = FailoverError::new(FailoverReason::RateLimit, "openai", perr);
        let src = std::error::Error::source(&env);
        assert!(src.is_some(), "source should return wrapped ProviderError");
        let msg = format!("{}", src.unwrap());
        assert!(
            msg.contains("Rate limited"),
            "source message preserved: {msg}"
        );
    }

    #[test]
    fn test_envelope_display_includes_reason_and_provider() {
        let perr = ProviderError::Connection("dns failed".into());
        let env = FailoverError::new(FailoverReason::Timeout, "gemini", perr);
        let out = format!("{env}");
        assert!(out.contains("timeout"));
        assert!(out.contains("gemini"));
        assert!(out.contains("Connection error"));
    }

    // ── T1-A1b call-site migration helper tests ──────────────────────────────

    /// wrap_provider_error round-trips a ProviderError into a FailoverError
    /// with reason=Unknown (until T1-A2 real classification lands) and
    /// preserves the source error.
    #[test]
    fn wrap_provider_error_round_trip() {
        let perr = ProviderError::Api {
            status: 503,
            message: "overloaded".into(),
        };
        let env = wrap_provider_error("anthropic", perr);
        assert_eq!(env.reason, FailoverReason::Unknown);
        assert_eq!(env.provider, "anthropic");
        // Source preserved via std::error::Error::source
        let src = std::error::Error::source(&env).expect("source must be present");
        let msg = format!("{src}");
        assert!(msg.contains("503"), "source preserved verbatim: {msg}");
    }

    /// A RateLimited ProviderError wraps into reason=Unknown for now;
    /// T1-A2 (classify_failover) will refine this to RateLimit.
    #[test]
    fn wrap_rate_limited_error() {
        let perr = ProviderError::RateLimited {
            retry_after_ms: 5000,
        };
        let env = wrap_provider_error("openai", perr);
        assert_eq!(
            env.reason,
            FailoverReason::Unknown,
            "T1-A1b stays Unknown; T1-A2 will reclassify"
        );
        assert_eq!(env.provider, "openai");
        let src = std::error::Error::source(&env).expect("source preserved");
        assert!(format!("{src}").contains("Rate limited"));
    }

    /// The existing `From<ProviderError> for FailoverError` impl is the
    /// chain-side consumption path. Verify it works with the same input the
    /// chain would receive on a failed `stream()` call.
    #[test]
    fn chain_consumes_failover_error_via_from() {
        let perr = ProviderError::Connection("p1 down".into());
        // Simulate what chain.rs would do internally on a retryable error:
        let env: FailoverError = perr.into();
        assert_eq!(env.reason, FailoverReason::Unknown);
        // provider is "unknown" via legacy-compat From — call sites that
        // know the provider name should use `wrap_provider_error` instead.
        assert_eq!(env.provider, "unknown");
        let src = std::error::Error::source(&env).expect("source preserved");
        assert!(format!("{src}").contains("p1 down"));
    }

    /// The CircuitBreaker uses bool retryability today (FailoverReason::Unknown
    /// is treated as a generic failure). Verify the envelope's reason field
    /// is readable so T1-A3 can dispatch on it later — and that today's
    /// behavior (any reason except success trips the circuit on threshold) is
    /// preserved by simply reading `env.reason` without crashing.
    #[test]
    fn circuit_breaker_reads_failover_reason() {
        let perr = ProviderError::Api {
            status: 500,
            message: "internal".into(),
        };
        let env = wrap_provider_error("vertex", perr);
        // The reason field is a Copy type — circuit code can read it without
        // moving the envelope.
        let reason = env.reason;
        assert_eq!(reason, FailoverReason::Unknown);
        // And the envelope still owns its source after the read.
        assert!(std::error::Error::source(&env).is_some());
    }

    /// Negative test: an empty provider name doesn't panic; wrap accepts
    /// anything Into<String>.
    #[test]
    fn wrap_provider_error_accepts_empty_provider_name() {
        let perr = ProviderError::Parse("malformed sse".into());
        let env = wrap_provider_error("", perr);
        assert_eq!(env.provider, "");
        assert_eq!(env.reason, FailoverReason::Unknown);
        // Display still renders without crashing.
        let _ = format!("{env}");
    }
}
