//! R4: ProviderChain — transparent sequential fallback across LLM providers.
//!
//! When the active provider returns a retryable error (5xx, connection
//! timeout, 429/rate-limit), the chain tries the next provider in order.
//! Terminal errors (4xx non-429, auth failures, malformed requests, parse
//! errors) propagate immediately — the request cannot succeed by retrying
//! on a different provider.
//!
//! On full exhaustion the last error is returned with its VARIANT intact —
//! see `exhausted`. Wrapping it in a `ProviderError::Connection` (as this did
//! before #1077) erased the failure class, so a 5xx or a 429 that exhausted a
//! chain was indistinguishable from a connect failure downstream.
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
            wcore_egress::EgressError::BeforeDispatch(_) => false,
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
        ProviderError::MissingApiKey | ProviderError::NotAttempted { .. } => false,
        // Flux capability / entitlement gates (402): terminal — surface the
        // typed message. Another provider can't grant a Flux-only capability
        // or resolve this account's spend ceiling.
        ProviderError::PremiumLocked { .. }
        | ProviderError::UpgradeRequired { .. }
        | ProviderError::SpendCeilingUnresolved { .. } => false,
    }
}

/// Render the chain's exhaustion without erasing the failure CLASS (#1077).
///
/// This used to return `ProviderError::Connection(format!("all N provider(s)
/// …"))` for every exhaustion. That is a lie for four of the five errors that
/// can reach here (only `Connection` itself survived it), and downstream
/// classification is variant-driven: `retry::provider_failure_code` reported
/// `connection` for a chain exhausted on an HTTP 500, and `wcore_agent`
/// deliberately treats a connect failure and a 500 differently — a 500 can
/// follow partial generation that was already billed, so it is denied the
/// unserved-outage retry budget a connect failure is granted.
///
/// So: keep the variant. Fold the attempt count into the message only where a
/// free-form message already exists (`Connection`, `Api`); the variants that
/// carry no message (`RateLimited`) or an opaque source (`Http`, `Egress`) are
/// returned verbatim rather than being flattened to reach a format string.
///
/// REACHABILITY — measured, not assumed. `ProviderChain::new` has **zero**
/// production call sites on this tree: all 17 occurrences repo-wide are
/// `#[cfg(test)]` or under `tests/` (13 of them in this module's own test
/// block). The shipped failover path is `ResilientProvider`, built by
/// `create_provider`/`bootstrap`, and it already returns `last_error`
/// verbatim (`resilient.rs:617`) — it never laundered anything. So this is a
/// latent defect in an exported type, closed before a host or a future
/// wiring reaches it; it is NOT a bug any 0.13.4 user could have hit. Do not
/// re-grade #1077 as a user-visible fix.
fn exhausted(attempts: usize, last_err: Option<ProviderError>) -> ProviderError {
    let prefix = format!("all {attempts} provider(s) in chain failed: ");
    match last_err {
        // Unreachable in practice — `new` asserts the chain is non-empty, so
        // the loop always records an error before falling through. Kept so the
        // exhaustion path has no `unwrap`.
        None => ProviderError::Connection(format!("{prefix}unknown")),
        Some(ProviderError::Connection(message)) => {
            ProviderError::Connection(format!("{prefix}{message}"))
        }
        Some(ProviderError::Api { status, message }) => ProviderError::Api {
            status,
            message: format!("{prefix}{message}"),
        },
        Some(other) => other,
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

        let mut previous_provider = None::<(&str, bool)>;
        for slot in &self.providers {
            if let Some((previous_provider, previous_attempted)) = previous_provider {
                crate::retry::admit_configured_fallback(
                    previous_provider,
                    &slot.name,
                    &slot.name,
                    &request.model,
                    previous_attempted,
                )?;
            }
            attempts += 1;
            match crate::attempt_lifecycle::scope_provider_attempt_identity(
                slot.name.clone(),
                request.model.clone(),
                slot.provider.stream(request),
            )
            .await
            {
                Ok(rx) => return Ok(rx),
                Err(e) if is_chain_retryable(&e) => {
                    let previous_attempted =
                        crate::retry::configured_fallback_previous_attempted(&e);
                    last_err = Some(e);
                    previous_provider = Some((&slot.name, previous_attempted));
                    // continue to next provider
                }
                Err(terminal) => return Err(terminal),
            }
        }

        // All providers exhausted with retryable errors.
        Err(exhausted(attempts, last_err))
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
            flux_loop_intent: None,
            flux_turn_nonce: None,
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
            routed_model_hint: None,
            replay_reasoning_content: false,
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

    #[tokio::test]
    async fn scoped_zero_preserves_configured_provider_fallback() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let admissions = Arc::new(AtomicUsize::new(0));
        let chain = ProviderChain::new(vec![
            (
                "p1",
                err_provider(
                    || ProviderError::Api {
                        status: 503,
                        message: "overloaded".into(),
                    },
                    c1.clone(),
                ),
            ),
            ("p2", ok_provider(c2.clone())),
        ]);

        let admission_count = Arc::clone(&admissions);
        let admitter: crate::retry::ConfiguredFallbackAdmitter =
            Arc::new(move |previous, next, _, _, previous_attempted| {
                assert_eq!((previous, next), ("p1", "p2"));
                assert!(previous_attempted);
                admission_count.fetch_add(1, Ordering::SeqCst);
                Ok(Default::default())
            });
        let result = crate::retry::scope_configured_fallback_admitter(
            admitter,
            crate::retry::scope_max_retries(0, chain.stream(&dummy_request())),
        )
        .await;

        result.expect("configured fallback must remain available when nested retries are disabled");
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(admissions.load(Ordering::SeqCst), 1);
        assert_eq!(
            c2.load(Ordering::SeqCst),
            1,
            "zero-retry scope limits each provider send, not configured fallback order"
        );
    }

    #[tokio::test]
    async fn configured_fallback_denial_prevents_next_provider_dispatch() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let chain = ProviderChain::new(vec![
            (
                "p1",
                err_provider(
                    || ProviderError::Connection("primary down".into()),
                    c1.clone(),
                ),
            ),
            ("p2", ok_provider(c2.clone())),
        ]);
        let admitter: crate::retry::ConfiguredFallbackAdmitter = Arc::new(|_, _, _, _, _| {
            Err(ProviderError::Api {
                status: 400,
                message: "fallback budget denied".into(),
            })
        });

        let error = crate::retry::scope_configured_fallback_admitter(
            admitter,
            chain.stream(&dummy_request()),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ProviderError::Api { status: 400, .. }));
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 0);
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

    // ── #1077 red arm ────────────────────────────────────────────────────────

    /// A real, DNS-free `reqwest` connect failure: bind a loopback port, learn
    /// its number, drop the listener, then connect to it. Nothing is listening,
    /// so the kernel answers RST — `ECONNREFUSED`, the shape #1077 is about —
    /// and no name is ever resolved, so the test stays DNS-hermetic.
    async fn real_refused_egress_error() -> wcore_egress::EgressError {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);
        crate::http_client::build()
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .expect_err("a port with no listener must refuse")
    }

    /// #1077 quotes `is_chain_retryable`'s `ProviderError::Http` arm (line 40)
    /// and asks for `is_connect()` to be split there. That arm cannot fire for
    /// a connect failure: `provider_error_from_reqwest` (reached from the live
    /// egress path via `provider_error_from_egress`) converts every
    /// connect/timeout reqwest error into `ProviderError::Connection` first, so
    /// the only arm a refused connect ever reaches is line 38.
    ///
    /// Editing line 40 or line 44 changes nothing at runtime. This pins that.
    #[tokio::test]
    async fn a_refused_connect_never_reaches_the_http_or_egress_arm() {
        let err = crate::retry::provider_error_from_egress(real_refused_egress_error().await);
        assert!(
            matches!(err, ProviderError::Connection(_)),
            "the ticket's premise is that a refused connect arrives as Http/Egress; got {err:?}"
        );
        assert!(
            is_chain_retryable(&err),
            "line 38 is the arm that decides a refused connect"
        );

        // CONTROL: the Egress arm the ticket points at is reachable by SOME
        // error, so the assertion above is about ROUTING, not about a match
        // that never matches anything.
        assert!(
            !is_chain_retryable(&ProviderError::Egress(wcore_egress::EgressError::Denied(
                "policy".into()
            ))),
            "control: the Egress arm must still be exercised by the match"
        );
    }

    /// RED ARM — #1077, the defect that IS in this file.
    ///
    /// On exhaustion the chain re-renders the last error as
    /// `ProviderError::Connection(format!("all N provider(s) …"))`. That erases
    /// the failure CLASS. `wcore_agent`'s `is_unserved_request_failure` admits
    /// `"connection"` to the 900 s `UNSERVED_OUTAGE_BUDGET` and deliberately
    /// denies it to every 5xx other than 503/529 ("a 500 can follow partial
    /// generation that was billed"). So exhausting a chain on an HTTP 500
    /// promotes that 500 into a retry window the engine refuses to give it.
    #[tokio::test]
    async fn chain_exhaustion_preserves_the_failure_class() {
        // NEGATIVE CONTROL first: `provider_failure_code` is not stuck
        // returning "connection" for everything. A real refused connect,
        // converted by the production path, must classify as
        // `connection_refused` — if this fails, the assertion below proves
        // nothing.
        let refused = crate::retry::provider_error_from_egress(real_refused_egress_error().await);
        assert_eq!(
            crate::retry::provider_failure_code(&refused),
            crate::retry::FAILURE_CONNECTION_REFUSED,
            "control: the classifier must be able to return something other than `connection`"
        );

        let chain = ProviderChain::new(vec![(
            "p1",
            err_provider(
                || ProviderError::Api {
                    status: 500,
                    message: "upstream boom".into(),
                },
                Arc::new(AtomicUsize::new(0)),
            ),
        )]);
        let err = chain.stream(&dummy_request()).await.unwrap_err();
        assert_eq!(
            crate::retry::provider_failure_code(&err),
            "http_500",
            "chain exhaustion must not launder a 5xx into a connect failure"
        );
    }

    /// The class is preserved WITHOUT dropping the diagnostic that the old
    /// rendering existed for: a message-bearing variant still carries the
    /// attempt count, it just keeps its own variant while doing so.
    #[tokio::test]
    async fn chain_exhaustion_keeps_the_attempt_count_on_a_message_bearing_error() {
        let chain = ProviderChain::new(vec![
            (
                "p1",
                err_provider(
                    || ProviderError::Api {
                        status: 502,
                        message: "gateway".into(),
                    },
                    Arc::new(AtomicUsize::new(0)),
                ),
            ),
            (
                "p2",
                err_provider(
                    || ProviderError::Api {
                        status: 503,
                        message: "overloaded".into(),
                    },
                    Arc::new(AtomicUsize::new(0)),
                ),
            ),
        ]);
        match chain.stream(&dummy_request()).await.unwrap_err() {
            ProviderError::Api { status, message } => {
                assert_eq!(status, 503, "the LAST error is the one returned");
                assert!(
                    message.contains("2 provider(s)"),
                    "attempt count must survive; got: {message}"
                );
                assert!(
                    message.contains("overloaded"),
                    "the underlying message must survive; got: {message}"
                );
            }
            other => panic!("expected the 503 to survive as Api, got {other:?}"),
        }
    }

    /// Same laundering, second class: a rate limit exhausted through the chain
    /// loses `http_429`, so the engine's `Retry-After` path can never see it.
    #[tokio::test]
    async fn chain_exhaustion_preserves_the_rate_limit_class() {
        // CONTROL: unwrapped, the same error classifies correctly.
        assert_eq!(
            crate::retry::provider_failure_code(&ProviderError::RateLimited { retry_after_ms: 0 }),
            "http_429",
            "control: an unwrapped rate limit classifies as http_429"
        );

        let chain = ProviderChain::new(vec![(
            "p1",
            err_provider(
                || ProviderError::RateLimited { retry_after_ms: 0 },
                Arc::new(AtomicUsize::new(0)),
            ),
        )]);
        let err = chain.stream(&dummy_request()).await.unwrap_err();
        assert_eq!(
            crate::retry::provider_failure_code(&err),
            "http_429",
            "chain exhaustion must not launder a rate limit into a connect failure"
        );
    }
}
