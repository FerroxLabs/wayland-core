//! W7 F8: ResilientProvider — wraps any LlmProvider with a circuit
//! breaker (Closed → Open → HalfOpen) and a fallback chain. The
//! inner provider's `with_retry` (HTTP-level) is unchanged; this is
//! the outer ring that decides "is this provider broken enough to
//! switch to the fallback."
//!
//! Retry classification: `ProviderError::is_retryable()` is the single
//! source of truth (`RateLimited`, `Connection`, and transient HTTP 5xx /
//! 408 / 429 `Api` errors — E-H4). Whether a retryable failure counts
//! toward the circuit breaker is a further decision: `should_trip_breaker`
//! excludes semantic failures (bad input) so they cannot open the circuit.
//! The `ProviderCompat.retry_policy` knob from the spec is reserved for a
//! future wave; this module does NOT consume it.
//!
//! ## CircuitBreaker consolidation (AF3 Risk 1)
//!
//! The private CircuitBreaker impl that lived here has been replaced with
//! the shared `wcore_config::circuit_breaker::CircuitBreaker`. Type aliases
//! for `CircuitConfig` and `CircuitState` keep the existing public API stable.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_config::circuit_breaker::{
    BreakerState, CircuitBreaker as SharedCircuitBreaker, CircuitBreakerConfig,
};
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::cooldown::CooldownClass;
use crate::{LlmProvider, ProviderError, classify_failover};

/// Classify a retryable `ProviderError` and decide whether it should count
/// against the circuit breaker.
///
/// Semantic failures (`ContextOverflow`, `Format`, `ModelNotFound`) are NOT
/// the provider's fault and a retry on the *same* provider will fail
/// identically — counting them toward the breaker would open the circuit on
/// a wedged *input*, not a wedged provider. Only transient and permanent
/// provider-side reasons trip the breaker.
fn should_trip_breaker(err: &ProviderError) -> bool {
    let status = match err {
        ProviderError::Api { status, .. } => Some(*status),
        _ => None,
    };
    let reason = classify_failover(err, status, None, None);
    !matches!(reason.cooldown_class(), CooldownClass::Semantic)
}

/// Alias for the shared `CircuitBreakerConfig`; keeps callers in `wcore-agent` stable.
pub type CircuitConfig = CircuitBreakerConfig;

/// Alias for the shared `BreakerState`; keeps callers in `wcore-agent` stable.
pub type CircuitState = BreakerState;

pub trait CircuitReporter: Send + Sync {
    fn report(
        &self,
        primary: &str,
        fallback: Option<&str>,
        state: CircuitState,
        error: Option<&str>,
    );
}

#[derive(Default)]
pub struct NoOpCircuitReporter;
impl CircuitReporter for NoOpCircuitReporter {
    fn report(&self, _: &str, _: Option<&str>, _: CircuitState, _: Option<&str>) {}
}

/// Thin wrapper around the shared `CircuitBreaker` that exposes the
/// legacy `before_call` / `on_success` / `on_failure` API used by
/// existing tests and `ResilientProvider`.
pub struct CircuitBreaker {
    inner: SharedCircuitBreaker,
}

impl CircuitBreaker {
    pub fn new(cfg: CircuitConfig) -> Self {
        Self {
            inner: SharedCircuitBreaker::new(cfg),
        }
    }

    /// Returns `Some(current_state)` when the caller should proceed with
    /// the call, `None` when the breaker is Open and cooldown has not elapsed.
    ///
    /// Side-effect: transitions Open → HalfOpen once `cooldown` elapses
    /// (delegated to `SharedCircuitBreaker::is_open`).
    pub fn before_call(&self) -> Option<CircuitState> {
        if self.inner.is_open() {
            None
        } else {
            Some(self.inner.state())
        }
    }

    /// Returns `Some(new_state)` iff a state transition occurred
    /// (HalfOpen → Closed). Returns `None` for the Closed no-op case.
    pub fn on_success(&self) -> Option<CircuitState> {
        let prev = self.inner.state();
        self.inner.record_success();
        if prev == BreakerState::HalfOpen {
            Some(BreakerState::Closed)
        } else {
            None
        }
    }

    /// Returns `Some(new_state)` iff the breaker transitioned to Open.
    pub fn on_failure(&self) -> Option<CircuitState> {
        self.inner.record_failure()
    }
}

pub struct ResilientProvider {
    primary: Arc<dyn LlmProvider>,
    primary_name: String,
    fallbacks: Vec<(String, Arc<dyn LlmProvider>)>,
    breaker: CircuitBreaker,
    reporter: Arc<dyn CircuitReporter>,
}
impl ResilientProvider {
    pub fn new(
        primary_name: impl Into<String>,
        primary: Arc<dyn LlmProvider>,
        fallbacks: Vec<(String, Arc<dyn LlmProvider>)>,
        cfg: CircuitConfig,
        reporter: Arc<dyn CircuitReporter>,
    ) -> Self {
        Self {
            primary_name: primary_name.into(),
            primary,
            fallbacks,
            breaker: CircuitBreaker::new(cfg),
            reporter,
        }
    }
}

#[async_trait]
impl LlmProvider for ResilientProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        if self.breaker.before_call().is_some() {
            match self.primary.stream(request).await {
                Ok(rx) => {
                    if let Some(new) = self.breaker.on_success() {
                        self.reporter.report(&self.primary_name, None, new, None);
                    }
                    return Ok(rx);
                }
                Err(e) if e.is_retryable() => {
                    // Only count provider-side (transient/permanent) failures
                    // against the breaker — a semantic error (bad input,
                    // context overflow) would reopen on the next identical
                    // request and is not the provider's health signal.
                    if should_trip_breaker(&e)
                        && let Some(new) = self.breaker.on_failure()
                    {
                        self.reporter
                            .report(&self.primary_name, None, new, Some(&e.to_string()));
                    }
                    if self.fallbacks.is_empty() {
                        // No fallback to try — surface the primary's error
                        // rather than the generic "all providers failed".
                        return Err(e);
                    }
                    // fall through to fallbacks
                }
                Err(other) => return Err(other),
            }
        } else {
            // Circuit open + cooldown not elapsed → skip primary, log the skip.
            self.reporter.report(
                &self.primary_name,
                self.fallbacks.first().map(|(n, _)| n.as_str()),
                CircuitState::Open,
                Some("circuit open; skipping primary"),
            );
        }
        // Try each fallback in order.
        for (name, fb) in &self.fallbacks {
            match fb.stream(request).await {
                Ok(rx) => {
                    self.reporter
                        .report(&self.primary_name, Some(name), CircuitState::Open, None);
                    return Ok(rx);
                }
                Err(e) if e.is_retryable() => continue,
                Err(other) => return Err(other),
            }
        }
        Err(ProviderError::Connection(
            "all providers in chain failed".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use parking_lot::Mutex;

    struct FlakyProvider {
        fails_remaining: AtomicUsize,
    }
    #[async_trait]
    impl LlmProvider for FlakyProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            if self.fails_remaining.fetch_sub(1, Ordering::SeqCst) > 0 {
                Err(ProviderError::Connection("flaky".into()))
            } else {
                let (_tx, rx) = mpsc::channel(1);
                Ok(rx)
            }
        }
    }
    struct AlwaysOk;
    #[async_trait]
    impl LlmProvider for AlwaysOk {
        async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }
    }
    struct AlwaysFail;
    #[async_trait]
    impl LlmProvider for AlwaysFail {
        async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            Err(ProviderError::Connection("always-fail".into()))
        }
    }
    struct CapReporter {
        events: Mutex<Vec<(String, Option<String>, CircuitState)>>,
    }
    impl CircuitReporter for CapReporter {
        fn report(&self, p: &str, f: Option<&str>, s: CircuitState, _e: Option<&str>) {
            self.events.lock().push((p.into(), f.map(String::from), s));
        }
    }

    fn dummy_request() -> LlmRequest {
        LlmRequest {
            model: "test".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
            thinking: None,
            reasoning_effort: None,
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
        }
    }

    #[tokio::test]
    async fn circuit_opens_after_threshold_failures_and_falls_back() {
        let primary = Arc::new(FlakyProvider {
            fails_remaining: AtomicUsize::new(10),
        });
        let fallback = Arc::new(AlwaysOk);
        let rep = Arc::new(CapReporter {
            events: Mutex::new(vec![]),
        });
        let resilient = ResilientProvider::new(
            "primary",
            primary,
            vec![("fb".into(), fallback)],
            CircuitConfig {
                fail_threshold: 3,
                window: Duration::from_secs(30),
                cooldown: Duration::from_secs(60),
            },
            rep.clone(),
        );
        // 4 failed primary calls → after the 3rd, circuit opens; 4th hits open path.
        for _ in 0..4 {
            let _ = resilient.stream(&dummy_request()).await;
        }
        let events = rep.events.lock();
        assert!(
            events.iter().any(|(_, _, s)| *s == CircuitState::Open),
            "must report Open state after threshold; got {events:?}"
        );
    }

    #[tokio::test]
    async fn closed_path_no_transitions_when_primary_succeeds() {
        let primary = Arc::new(AlwaysOk);
        let rep = Arc::new(CapReporter {
            events: Mutex::new(vec![]),
        });
        let resilient = ResilientProvider::new(
            "primary",
            primary,
            vec![],
            CircuitConfig::default(),
            rep.clone(),
        );
        let _ = resilient.stream(&dummy_request()).await.unwrap();
        // No transitions emitted (start Closed → still Closed).
        assert!(rep.events.lock().is_empty());
    }

    #[tokio::test]
    async fn all_providers_failing_returns_connection_error() {
        let primary = Arc::new(AlwaysFail);
        let fb = Arc::new(AlwaysFail);
        let rep = Arc::new(NoOpCircuitReporter);
        let resilient = ResilientProvider::new(
            "primary",
            primary,
            vec![("fb".into(), fb)],
            CircuitConfig::default(),
            rep,
        );
        let err = resilient.stream(&dummy_request()).await.unwrap_err();
        assert!(matches!(err, ProviderError::Connection(_)));
    }

    #[test]
    fn circuit_state_as_str_matches_protocol_literals() {
        assert_eq!(CircuitState::Closed.as_str(), "closed");
        assert_eq!(CircuitState::Open.as_str(), "open");
        assert_eq!(CircuitState::HalfOpen.as_str(), "half_open");
    }

    #[test]
    fn circuit_breaker_opens_after_threshold_failures() {
        let breaker = CircuitBreaker::new(CircuitConfig {
            fail_threshold: 3,
            window: Duration::from_secs(30),
            cooldown: Duration::from_secs(60),
        });
        // 1st + 2nd failures don't trip.
        assert!(breaker.on_failure().is_none());
        assert!(breaker.on_failure().is_none());
        // 3rd failure transitions to Open.
        assert_eq!(breaker.on_failure(), Some(CircuitState::Open));
    }

    // ----- E-H2: breaker classification + empty-fallback behaviour -----

    /// A transient provider-side error (503) MUST count against the breaker.
    #[test]
    fn should_trip_breaker_true_for_transient() {
        assert!(should_trip_breaker(&ProviderError::Connection(
            "reset".into()
        )));
        assert!(should_trip_breaker(&ProviderError::Api {
            status: 503,
            message: "overloaded".into(),
        }));
        assert!(should_trip_breaker(&ProviderError::RateLimited {
            retry_after_ms: 5000,
        }));
    }

    /// A semantic error (413 context overflow, 400 format) must NOT count —
    /// retrying the same provider with the same input fails identically, so
    /// it is not a provider-health signal.
    #[test]
    fn should_trip_breaker_false_for_semantic() {
        assert!(!should_trip_breaker(&ProviderError::Api {
            status: 413,
            message: "context length exceeded".into(),
        }));
        assert!(!should_trip_breaker(&ProviderError::Api {
            status: 400,
            message: "invalid request".into(),
        }));
        assert!(!should_trip_breaker(&ProviderError::PromptTooLong(
            "too long".into()
        )));
    }

    /// E-H2: with no fallbacks, a retryable primary failure must surface the
    /// *primary's* error verbatim — not the generic "all providers failed"
    /// (which would hide which provider/why for the common single-provider
    /// default config).
    #[tokio::test]
    async fn empty_fallbacks_surfaces_primary_error() {
        let primary = Arc::new(AlwaysFail);
        let resilient = ResilientProvider::new(
            "primary",
            primary,
            vec![], // default config: no fallback chain
            CircuitConfig::default(),
            Arc::new(NoOpCircuitReporter),
        );
        let err = resilient.stream(&dummy_request()).await.unwrap_err();
        // AlwaysFail returns Connection("always-fail") — must be propagated
        // as-is, not replaced by "all providers in chain failed".
        match err {
            ProviderError::Connection(msg) => {
                assert_eq!(msg, "always-fail", "primary error must pass through");
            }
            other => panic!("expected the primary's Connection error, got {other:?}"),
        }
    }

    /// E-H2: a semantic error from the primary must NOT open the breaker even
    /// after many repeats — the circuit stays Closed.
    #[tokio::test]
    async fn semantic_errors_do_not_open_breaker() {
        struct SemanticFail;
        #[async_trait]
        impl LlmProvider for SemanticFail {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                Err(ProviderError::Api {
                    status: 413,
                    message: "context length exceeded".into(),
                })
            }
        }
        let rep = Arc::new(CapReporter {
            events: Mutex::new(vec![]),
        });
        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(SemanticFail),
            vec![],
            CircuitConfig {
                fail_threshold: 2,
                window: Duration::from_secs(30),
                cooldown: Duration::from_secs(60),
            },
            rep.clone(),
        );
        // Many semantic failures — the breaker must never open.
        for _ in 0..6 {
            let _ = resilient.stream(&dummy_request()).await;
        }
        assert!(
            !rep.events
                .lock()
                .iter()
                .any(|(_, _, s)| *s == CircuitState::Open),
            "semantic errors must not trip the breaker"
        );
    }
}
