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
use crate::{LlmProvider, ModelInfo, ProviderError, classify_failover};

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

/// F20: True only for REQUEST-SEMANTIC errors — the ones that would fail
/// identically on EVERY provider in the chain, so trying a fallback is
/// pointless and the chain must abort immediately.
///
/// This is the abort set: `PromptTooLong` and the request-shape `Api` errors
/// (413 payload/context too large, 400 malformed request). These are properties
/// of the request itself, not of any one provider.
///
/// Deliberately EXCLUDED (these are provider/model-specific — a different
/// fallback may succeed, so the chain must CONTINUE): 401/403 (bad credential
/// for this provider), 404 (`ModelNotFound` on this provider), and
/// `MissingApiKey`. A misconfigured first fallback must not abort the chain.
fn is_request_fatal(err: &ProviderError) -> bool {
    match err {
        ProviderError::PromptTooLong(_) => true,
        ProviderError::Api { status, .. } => *status == 400 || *status == 413,
        _ => false,
    }
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
    // F32: `Arc` so the stream-terminal forwarder task can record the breaker
    // verdict (success on `Done`, failure on a terminal mid-stream `Error`)
    // after `stream()` has already returned the channel.
    breaker: Arc<CircuitBreaker>,
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
            breaker: Arc::new(CircuitBreaker::new(cfg)),
            reporter,
        }
    }

    /// F32: forward every event from the primary's stream onto a fresh channel,
    /// recording the breaker verdict only when the stream terminates:
    /// `Done` → success (closes a HalfOpen trial), a terminal `Error` (or the
    /// channel closing with no `Done`) → failure. This prevents a provider that
    /// always accepts headers then dies mid-body from looking permanently
    /// healthy. Events are passed through unmodified.
    fn spawn_breaker_forwarder(
        &self,
        mut rx: mpsc::Receiver<LlmEvent>,
    ) -> mpsc::Receiver<LlmEvent> {
        let (tx, out_rx) = mpsc::channel(32);
        let breaker = Arc::clone(&self.breaker);
        let reporter = Arc::clone(&self.reporter);
        let primary_name = self.primary_name.clone();
        tokio::spawn(async move {
            // `saw_done` distinguishes a clean completion from a stream that
            // closed without a terminal Done (treated as a mid-stream failure).
            let mut saw_done = false;
            let mut saw_error = false;
            while let Some(event) = rx.recv().await {
                match &event {
                    LlmEvent::Done { .. } => saw_done = true,
                    LlmEvent::Error(_) => saw_error = true,
                    _ => {}
                }
                if tx.send(event).await.is_err() {
                    // Consumer dropped the receiver — stop forwarding. We do not
                    // record a verdict here: an abandoned read is not a provider
                    // health signal.
                    return;
                }
            }
            if saw_done && !saw_error {
                if let Some(new) = breaker.on_success() {
                    reporter.report(&primary_name, None, new, None);
                }
            } else if let Some(new) = breaker.on_failure() {
                // Mid-stream death (terminal Error or channel closed without Done).
                reporter.report(
                    &primary_name,
                    None,
                    new,
                    Some("stream terminated without success"),
                );
            }
        });
        out_rx
    }
}

#[async_trait]
impl LlmProvider for ResilientProvider {
    /// Delegate to the wrapped primary so callers that introspect the
    /// provider (e.g. the `/model` picker's default `list_models` fallback)
    /// see the real alias key, not the blanket `""`. The breaker only guards
    /// `stream`; metadata is always answered by the primary.
    fn alias_key(&self) -> &str {
        self.primary.alias_key()
    }

    /// Delegate model listing to the primary. Without this the trait default
    /// runs against `alias_key()` — which, before the delegation above,
    /// returned `""` and yielded an empty `/model` list for every provider.
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        self.primary.list_models().await
    }

    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        if self.breaker.before_call().is_some() {
            match self.primary.stream(request).await {
                Ok(rx) => {
                    // F32: header acceptance is NOT yet a success. stream() returns
                    // Ok(rx) once headers arrive, but the request can still die
                    // mid-body (surfaced as a terminal LlmEvent::Error on the
                    // channel, never as Err here). Recording success now would keep
                    // a provider that always dies mid-stream looking "healthy".
                    // Instead, defer the breaker verdict to the stream's terminal
                    // event by forwarding through a wrapper channel: Done → success,
                    // terminal Error → failure.
                    return Ok(self.spawn_breaker_forwarder(rx));
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
                // F20: a NON-retryable primary error must distinguish
                // request-semantic faults (abort — they would fail on every
                // provider) from provider/model-specific ones (401/403/404/
                // MissingApiKey — a misconfigured primary). The latter must
                // fall through to the fallback chain rather than abort before
                // it is ever tried; otherwise fallbacks never run for the most
                // common misconfiguration, defeating their entire purpose
                // (same policy the fallback loop below applies).
                Err(e) if is_request_fatal(&e) => return Err(e),
                Err(e) => {
                    if self.fallbacks.is_empty() {
                        return Err(e);
                    }
                    // fall through to fallbacks
                }
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
                // Retryable failures move on to the next fallback.
                Err(e) if e.is_retryable() => continue,
                // F20: only REQUEST-SEMANTIC errors (would fail on every
                // provider too) abort the chain. A provider/model-specific
                // non-retryable error (401/403/404/MissingApiKey — e.g. a
                // misconfigured first fallback) must NOT abort: continue to the
                // next fallback. The last entry's error surfaces below.
                Err(e) if is_request_fatal(&e) => return Err(e),
                Err(_) => continue,
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
    /// Emit a terminal `Done` so the breaker forwarder (F32) classifies the
    /// stream as a real success — a stream that closes WITHOUT a `Done` is now
    /// (correctly) treated as a mid-stream failure.
    fn ok_done_channel() -> mpsc::Receiver<LlmEvent> {
        use wcore_types::message::{FinishReason, StopReason, TokenUsage};
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::Stop,
                    usage: TokenUsage::default(),
                })
                .await;
        });
        rx
    }

    struct AlwaysOk;
    #[async_trait]
    impl LlmProvider for AlwaysOk {
        // Report a real alias key + catalog so the delegation can be asserted.
        fn alias_key(&self) -> &str {
            "openai-chatgpt"
        }
        async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            Ok(ok_done_channel())
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
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
            omit_max_tokens: false,
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

    /// F32: a provider that accepts headers (returns `Ok(rx)`) but then dies
    /// mid-stream (terminal `LlmEvent::Error`) must NOT be recorded as healthy.
    /// Enough such mid-stream deaths must trip the breaker — proving the verdict
    /// is deferred to the stream's terminal event, not header acceptance.
    #[tokio::test]
    async fn mid_stream_error_counts_as_failure_not_success() {
        struct HeadersThenDie;
        #[async_trait]
        impl LlmProvider for HeadersThenDie {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                let (tx, rx) = mpsc::channel(1);
                tokio::spawn(async move {
                    // Some output, then a terminal mid-stream error — never a Done.
                    let _ = tx.send(LlmEvent::TextDelta("partial".into())).await;
                    let _ = tx.send(LlmEvent::Error("connection reset".into())).await;
                });
                Ok(rx)
            }
        }
        let rep = Arc::new(CapReporter {
            events: Mutex::new(vec![]),
        });
        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(HeadersThenDie),
            vec![],
            CircuitConfig {
                fail_threshold: 2,
                window: Duration::from_secs(30),
                cooldown: Duration::from_secs(60),
            },
            rep.clone(),
        );
        // Each call: headers accepted (Ok), then mid-stream death. Drain each
        // returned stream so the forwarder observes the terminal Error and
        // records the verdict. After 2 such deaths the breaker must open.
        for _ in 0..3 {
            if let Ok(mut rx) = resilient.stream(&dummy_request()).await {
                while rx.recv().await.is_some() {}
            }
            // Let the forwarder's spawned task run to completion.
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            rep.events
                .lock()
                .iter()
                .any(|(_, _, s)| *s == CircuitState::Open),
            "mid-stream deaths must trip the breaker — header acceptance must \
             not be recorded as a success; got {:?}",
            rep.events.lock()
        );
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

    /// Rank 20: once the primary's circuit is Open, a configured fallback must
    /// actually serve the request — the primary is skipped and the fallback's
    /// `Ok` is returned. This proves the failover chain is reachable (a
    /// non-empty `fallbacks` Vec is the contract `bootstrap` must now satisfy);
    /// before the fix `bootstrap` always passed `Vec::new()`, so this path was
    /// dead.
    #[tokio::test]
    async fn open_circuit_fails_over_to_fallback() {
        // Primary always fails AND counts its calls, so we can assert it is
        // skipped once the breaker opens.
        struct CountingFail {
            calls: AtomicUsize,
        }
        #[async_trait]
        impl LlmProvider for CountingFail {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::Connection("primary down".into()))
            }
        }
        let primary = Arc::new(CountingFail {
            calls: AtomicUsize::new(0),
        });
        let fallback = Arc::new(AlwaysOk);
        let resilient = ResilientProvider::new(
            "primary",
            primary.clone(),
            vec![("fb".into(), fallback)],
            CircuitConfig {
                fail_threshold: 2,
                window: Duration::from_secs(30),
                cooldown: Duration::from_secs(60),
            },
            Arc::new(NoOpCircuitReporter),
        );
        // First two calls trip the breaker (each still falls over to the
        // fallback and returns Ok). After the 2nd failure the circuit is Open.
        for _ in 0..2 {
            assert!(
                resilient.stream(&dummy_request()).await.is_ok(),
                "fallback must serve the request while the primary is failing"
            );
        }
        let calls_after_open = primary.calls.load(Ordering::SeqCst);
        // A subsequent call with the circuit Open must skip the primary
        // entirely and still succeed via the fallback.
        assert!(
            resilient.stream(&dummy_request()).await.is_ok(),
            "fallback must serve the request once the primary circuit is open"
        );
        assert_eq!(
            primary.calls.load(Ordering::SeqCst),
            calls_after_open,
            "primary must NOT be called once its circuit is open — the open \
             path must route straight to the fallback"
        );
    }

    /// F20: a misconfigured FIRST fallback whose error is non-retryable but
    /// provider/model-specific (404 ModelNotFound / MissingApiKey) must NOT
    /// abort the chain — the SECOND fallback is still tried and serves the
    /// request. Before the fix, the first non-retryable error returned early.
    #[tokio::test]
    async fn provider_specific_error_in_fallback_continues_chain() {
        struct NotFound;
        #[async_trait]
        impl LlmProvider for NotFound {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                Err(ProviderError::Api {
                    status: 404,
                    message: "model not found".into(),
                })
            }
        }
        struct MissingKey;
        #[async_trait]
        impl LlmProvider for MissingKey {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                Err(ProviderError::MissingApiKey)
            }
        }
        // Primary down (retryable) → falls into the fallback chain. First two
        // fallbacks fail with provider-specific non-retryable errors; the third
        // succeeds and must serve the request.
        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(AlwaysFail),
            vec![
                (
                    "bad-model".into(),
                    Arc::new(NotFound) as Arc<dyn LlmProvider>,
                ),
                ("no-key".into(), Arc::new(MissingKey)),
                ("good".into(), Arc::new(AlwaysOk)),
            ],
            CircuitConfig::default(),
            Arc::new(NoOpCircuitReporter),
        );
        assert!(
            resilient.stream(&dummy_request()).await.is_ok(),
            "a 404/MissingApiKey first fallback must not abort the chain — \
             the later working fallback must still be reached"
        );
    }

    /// F20 (primary boundary): a NON-retryable provider/model-specific error
    /// from the PRIMARY (here MissingApiKey — a misconfigured primary) must fall
    /// through to the fallback chain, not abort before any fallback runs. Before
    /// the fix the primary's `Err(other) => return Err(other)` arm aborted here,
    /// so fallbacks never ran for the most common misconfiguration.
    #[tokio::test]
    async fn provider_specific_error_in_primary_falls_through_to_fallback() {
        struct PrimaryMissingKey;
        #[async_trait]
        impl LlmProvider for PrimaryMissingKey {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                Err(ProviderError::MissingApiKey)
            }
        }
        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(PrimaryMissingKey),
            vec![("good".into(), Arc::new(AlwaysOk) as Arc<dyn LlmProvider>)],
            CircuitConfig::default(),
            Arc::new(NoOpCircuitReporter),
        );
        assert!(
            resilient.stream(&dummy_request()).await.is_ok(),
            "a non-retryable primary (MissingApiKey) must fall through to the \
             working fallback, not abort before the chain is tried"
        );
    }

    /// F20: a REQUEST-SEMANTIC error (413/400/PromptTooLong) from a fallback
    /// WOULD fail on every provider, so the chain aborts immediately rather
    /// than wasting calls on the remaining fallbacks.
    #[tokio::test]
    async fn request_fatal_error_in_fallback_aborts_chain() {
        struct TooLarge {
            calls: AtomicUsize,
        }
        #[async_trait]
        impl LlmProvider for TooLarge {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::Api {
                    status: 413,
                    message: "context length exceeded".into(),
                })
            }
        }
        let never = Arc::new(TooLarge {
            calls: AtomicUsize::new(0),
        });
        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(AlwaysFail),
            vec![
                (
                    "too-large".into(),
                    Arc::new(TooLarge {
                        calls: AtomicUsize::new(0),
                    }) as Arc<dyn LlmProvider>,
                ),
                ("never".into(), never.clone()),
            ],
            CircuitConfig::default(),
            Arc::new(NoOpCircuitReporter),
        );
        let err = resilient.stream(&dummy_request()).await.unwrap_err();
        assert!(
            matches!(err, ProviderError::Api { status: 413, .. }),
            "the request-fatal 413 must surface and abort the chain"
        );
        assert_eq!(
            never.calls.load(Ordering::SeqCst),
            0,
            "the chain must abort on the request-fatal error — later fallbacks must NOT be called"
        );
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

    /// Regression: the wrap is metadata-transparent. `alias_key` and
    /// `list_models` must reflect the wrapped primary — not the blanket trait
    /// defaults (`""` → empty catalog), which made `/model` return nothing for
    /// every provider since every provider is wrapped in `ResilientProvider`.
    #[tokio::test]
    async fn delegates_alias_key_and_list_models_to_primary() {
        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(AlwaysOk),
            vec![],
            CircuitConfig::default(),
            Arc::new(NoOpCircuitReporter),
        );
        assert_eq!(
            resilient.alias_key(),
            "openai-chatgpt",
            "alias_key must come from the primary, not the trait default \"\""
        );
        let models = resilient
            .list_models()
            .await
            .expect("list_models must not error");
        assert!(
            !models.is_empty(),
            "list_models must yield the primary's alias catalog, not an empty list"
        );
    }
}
