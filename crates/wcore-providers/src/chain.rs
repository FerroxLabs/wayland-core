//! R4: ProviderChain — transparent sequential fallback across LLM providers.
//!
//! When the active provider returns a retryable error (5xx, connection
//! timeout, 429/rate-limit), the chain tries the next provider in order.
//! Terminal errors (4xx non-429, auth failures, malformed requests, parse
//! errors) propagate immediately — the request cannot succeed by retrying
//! on a different provider.
//!
//! On full exhaustion the last error is returned wrapped in a
//! `ProviderError::Connection` message that includes the attempt count.
//!
//! This is intentionally stateless: no circuit-breaker, no cooldown. It
//! composes with `ResilientProvider` — each slot can be a `ResilientProvider`
//! if you want per-provider circuit-breaking on top.
//!
//! ## T1-A1b call-site migration (LOCKED ABI)
//!
//! `ProviderChain::stream` still returns `Result<_, ProviderError>` for
//! backward compatibility. The `FailoverError` envelope from
//! `crate::failover` is available via `wrap_provider_error(name, err)` for
//! internal consumption — full classification logic lands in T1-A2.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::{LlmProvider, ProviderError};

/// Returns `true` for errors where trying the next provider may succeed.
///
/// Retryable: 5xx server errors, connection/timeout errors, 429 rate-limit.
/// Terminal:  4xx (non-429), auth errors, malformed request, parse errors.
fn is_chain_retryable(e: &ProviderError) -> bool {
    match e {
        // Connection timeouts / network failures
        ProviderError::Connection(_) => true,
        // reqwest-level errors: timeouts, TLS failures, DNS failures
        ProviderError::Http(inner) => inner.is_timeout() || inner.is_connect(),
        // Egress chokepoint: transport failures follow the same rule as Http;
        // a policy Denied is terminal (another provider would be denied too).
        ProviderError::Egress(e) => match e {
            wcore_egress::EgressError::Transport(inner) => inner.is_timeout() || inner.is_connect(),
            wcore_egress::EgressError::Denied(_) => false,
            // Terminal: an over-cap body won't shrink on a retry/failover.
            wcore_egress::EgressError::BodyTooLarge { .. } => false,
        },
        // Rate-limit — another provider might not be rate-limited
        ProviderError::RateLimited { .. } => true,
        // 5xx server-side errors are transient; 4xx are terminal
        ProviderError::Api { status, .. } => *status >= 500,
        // SSE parse error — structural bug in this provider's response
        ProviderError::Parse(_) => false,
        // Request too large — won't shrink on a different provider
        ProviderError::PromptTooLong(_) => false,
        // Flux 409 context_overflow — recovery is compact-then-retry on the
        // SAME provider (the engine drives it), never failover. Terminal here.
        ProviderError::ContextOverflow { .. } => false,
        // Missing credential is a config error the user must fix; failing over
        // would only mask it. Terminal.
        ProviderError::MissingApiKey => false,
        // Flux capability / entitlement gates (402): terminal — surface the
        // typed message. Another provider can't grant a Flux-only capability
        // or resolve this account's spend ceiling.
        ProviderError::PremiumLocked { .. }
        | ProviderError::UpgradeRequired { .. }
        | ProviderError::SpendCeilingUnresolved { .. } => false,
    }
}

/// A named provider slot in the chain.
pub struct ProviderSlot {
    pub name: String,
    pub provider: Arc<dyn LlmProvider>,
}

/// Ordered list of providers tried in sequence on retryable failures.
///
/// Implements `LlmProvider` so it is a drop-in replacement wherever a
/// single `Arc<dyn LlmProvider>` is expected.
pub struct ProviderChain {
    providers: Vec<ProviderSlot>,
}

impl ProviderChain {
    /// Build a chain from `(name, provider)` pairs. The first entry is
    /// tried first. Panics if `providers` is empty.
    pub fn new(providers: Vec<(impl Into<String>, Arc<dyn LlmProvider>)>) -> Self {
        assert!(
            !providers.is_empty(),
            "ProviderChain requires at least one provider"
        );
        Self {
            providers: providers
                .into_iter()
                .map(|(name, provider)| ProviderSlot {
                    name: name.into(),
                    provider,
                })
                .collect(),
        }
    }

    /// Number of providers in the chain.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// True when the chain holds zero providers.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

#[async_trait]
impl LlmProvider for ProviderChain {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let mut last_err = None;
        let mut attempts = 0usize;

        // W1 v0.6.3: consume the smart-router hint. The hint is a free-form
        // label produced by `wcore-providers::routing` and stamped onto the
        // request by the agent engine; surfacing it in the dispatch span
        // makes the router's decision visible without changing fallback
        // order (unknown labels are ignored).
        if let Some(hint) = request.routing_hint.as_ref() {
            tracing::debug!(
                target: "wcore_providers::chain",
                routing_hint = %hint.0,
                chain_len = self.providers.len(),
                "ProviderChain dispatch with routing hint"
            );
        }

        for slot in &self.providers {
            attempts += 1;
            match slot.provider.stream(request).await {
                Ok(rx) => return Ok(rx),
                Err(e) if is_chain_retryable(&e) => {
                    last_err = Some(e);
                    // continue to next provider
                }
                Err(terminal) => return Err(terminal),
            }
        }

        // All providers exhausted with retryable errors.
        Err(ProviderError::Connection(format!(
            "all {} provider(s) in chain failed: {}",
            attempts,
            last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unknown".into()),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── test doubles ──────────────────────────────────────────────────────────

    struct FixedProvider {
        result: Box<dyn Fn() -> Result<mpsc::Receiver<LlmEvent>, ProviderError> + Send + Sync>,
        call_count: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl LlmProvider for FixedProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
    }

    fn ok_provider(counter: Arc<AtomicUsize>) -> Arc<dyn LlmProvider> {
        Arc::new(FixedProvider {
            result: Box::new(|| {
                let (_tx, rx) = mpsc::channel(1);
                Ok(rx)
            }),
            call_count: counter,
        })
    }
    fn err_provider(err: fn() -> ProviderError, counter: Arc<AtomicUsize>) -> Arc<dyn LlmProvider> {
        Arc::new(FixedProvider {
            result: Box::new(move || Err(err())),
            call_count: counter,
        })
    }

    fn dummy_request() -> LlmRequest {
        LlmRequest {
            model: "test".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1,
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

    // ── tests ─────────────────────────────────────────────────────────────────

    /// chain of 2: first returns 500 → second succeeds → Ok
    #[tokio::test]
    async fn first_5xx_falls_through_to_second() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let chain = ProviderChain::new(vec![
            (
                "p1",
                err_provider(
                    || ProviderError::Api {
                        status: 500,
                        message: "internal".into(),
                    },
                    c1.clone(),
                ),
            ),
            ("p2", ok_provider(c2.clone())),
        ]);
        chain.stream(&dummy_request()).await.unwrap();
        assert_eq!(c1.load(Ordering::SeqCst), 1, "p1 must be called once");
        assert_eq!(c2.load(Ordering::SeqCst), 1, "p2 must be called once");
    }

    /// chain of 2: first returns 400 → second NOT tried → first error returned
    #[tokio::test]
    async fn first_4xx_is_terminal_second_not_tried() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let chain = ProviderChain::new(vec![
            (
                "p1",
                err_provider(
                    || ProviderError::Api {
                        status: 400,
                        message: "bad request".into(),
                    },
                    c1.clone(),
                ),
            ),
            ("p2", ok_provider(c2.clone())),
        ]);
        let err = chain.stream(&dummy_request()).await.unwrap_err();
        assert!(
            matches!(err, ProviderError::Api { status: 400, .. }),
            "must propagate the 400 directly"
        );
        assert_eq!(c1.load(Ordering::SeqCst), 1, "p1 called once");
        assert_eq!(c2.load(Ordering::SeqCst), 0, "p2 must not be tried");
    }

    /// chain of 2: both fail with retryable errors → aggregated Connection error
    #[tokio::test]
    async fn both_fail_returns_connection_error_with_attempt_count() {
        let chain = ProviderChain::new(vec![
            (
                "p1",
                err_provider(
                    || ProviderError::Connection("p1 down".into()),
                    Arc::new(AtomicUsize::new(0)),
                ),
            ),
            (
                "p2",
                err_provider(
                    || ProviderError::Connection("p2 down".into()),
                    Arc::new(AtomicUsize::new(0)),
                ),
            ),
        ]);
        let err = chain.stream(&dummy_request()).await.unwrap_err();
        match err {
            ProviderError::Connection(msg) => {
                assert!(
                    msg.contains("2 provider(s)"),
                    "message must include attempt count; got: {msg}"
                );
            }
            other => panic!("expected Connection, got {other:?}"),
        }
    }

    /// chain of 3: middle fails → boundary navigation correct (p1 ok → done, p3 not tried)
    #[tokio::test]
    async fn chain_of_3_first_ok_middle_never_reached() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let c3 = Arc::new(AtomicUsize::new(0));
        let chain = ProviderChain::new(vec![
            ("p1", ok_provider(c1.clone())),
            (
                "p2",
                err_provider(
                    || ProviderError::Api {
                        status: 503,
                        message: "overloaded".into(),
                    },
                    c2.clone(),
                ),
            ),
            ("p3", ok_provider(c3.clone())),
        ]);
        chain.stream(&dummy_request()).await.unwrap();
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(
            c2.load(Ordering::SeqCst),
            0,
            "p2 never reached because p1 succeeded"
        );
        assert_eq!(
            c3.load(Ordering::SeqCst),
            0,
            "p3 never reached because p1 succeeded"
        );
    }

    /// chain of 3: p1 fails (5xx), p2 fails (5xx), p3 succeeds — full traversal
    #[tokio::test]
    async fn chain_of_3_traverses_to_third_on_5xx() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let c3 = Arc::new(AtomicUsize::new(0));
        let chain = ProviderChain::new(vec![
            (
                "p1",
                err_provider(
                    || ProviderError::Api {
                        status: 502,
                        message: "gateway".into(),
                    },
                    c1.clone(),
                ),
            ),
            (
                "p2",
                err_provider(
                    || ProviderError::Api {
                        status: 503,
                        message: "overloaded".into(),
                    },
                    c2.clone(),
                ),
            ),
            ("p3", ok_provider(c3.clone())),
        ]);
        chain.stream(&dummy_request()).await.unwrap();
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
        assert_eq!(c3.load(Ordering::SeqCst), 1);
    }

    /// chain of 2: first 429 → second tried (rate-limit IS retryable)
    #[tokio::test]
    async fn first_429_rate_limit_falls_through_to_second() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let chain = ProviderChain::new(vec![
            (
                "p1",
                err_provider(
                    || ProviderError::RateLimited {
                        retry_after_ms: 60_000,
                    },
                    c1.clone(),
                ),
            ),
            ("p2", ok_provider(c2.clone())),
        ]);
        chain.stream(&dummy_request()).await.unwrap();
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1, "p2 must be tried after 429");
    }

    /// PromptTooLong is terminal — p2 not tried
    #[tokio::test]
    async fn prompt_too_long_is_terminal() {
        let c2 = Arc::new(AtomicUsize::new(0));
        let chain = ProviderChain::new(vec![
            (
                "p1",
                err_provider(
                    || ProviderError::PromptTooLong("exceeds limit".into()),
                    Arc::new(AtomicUsize::new(0)),
                ),
            ),
            ("p2", ok_provider(c2.clone())),
        ]);
        let err = chain.stream(&dummy_request()).await.unwrap_err();
        assert!(matches!(err, ProviderError::PromptTooLong(_)));
        assert_eq!(c2.load(Ordering::SeqCst), 0, "p2 must not be tried");
    }

    /// Parse error is terminal — p2 not tried
    #[tokio::test]
    async fn parse_error_is_terminal() {
        let c2 = Arc::new(AtomicUsize::new(0));
        let chain = ProviderChain::new(vec![
            (
                "p1",
                err_provider(
                    || ProviderError::Parse("bad json".into()),
                    Arc::new(AtomicUsize::new(0)),
                ),
            ),
            ("p2", ok_provider(c2.clone())),
        ]);
        let err = chain.stream(&dummy_request()).await.unwrap_err();
        assert!(matches!(err, ProviderError::Parse(_)));
        assert_eq!(c2.load(Ordering::SeqCst), 0, "p2 must not be tried");
    }
}
