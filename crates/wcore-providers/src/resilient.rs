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

use crate::cooldown::{CooldownClass, CooldownPermit, CooldownState, CooldownTracker};
use crate::failover_policy::{
    CandidateCapabilities, CandidateReceipt, CandidateRejection, FailoverCandidateMetadata,
    FailoverReceipt, FailoverRoutingPolicy, PricingEvidence, RequestRequirements,
    evaluate_candidate,
};
use crate::{FailoverReason, LlmProvider, ModelInfo, ProviderError, classify_failover};

/// Classify a retryable `ProviderError` and decide whether it should count
/// against the circuit breaker.
///
/// Request-semantic format and context-overflow failures are NOT the provider's
/// fault and do not poison candidate health. A missing model cools only that
/// provider/model candidate permanently; each fallback has an independent
/// tracker, so it cannot disable other models/providers.
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
/// A malformed request (HTTP 400) is provider-independent and aborts. Context
/// overflow is deliberately excluded: candidate admission knows each model's
/// context window and may safely select a larger compatible target.
///
/// Deliberately EXCLUDED (these are provider/model-specific — a different
/// fallback may succeed, so the chain must CONTINUE): 401/403 (bad credential
/// for this provider), 404 (`ModelNotFound` on this provider), and
/// `MissingApiKey`. A misconfigured first fallback must not abort the chain.
fn is_request_fatal(err: &ProviderError) -> bool {
    matches!(err, ProviderError::Api { status: 400, .. })
}

fn classify_error(err: &ProviderError) -> FailoverReason {
    let status = match err {
        ProviderError::Api { status, .. } => Some(*status),
        _ => None,
    };
    classify_failover(err, status, None, None)
}

fn retry_after(err: &ProviderError) -> Option<std::time::Duration> {
    match err {
        ProviderError::RateLimited { retry_after_ms } => {
            Some(std::time::Duration::from_millis(*retry_after_ms))
        }
        _ => None,
    }
}

fn duration_millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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

    fn report_failover(&self, _receipt: &FailoverReceipt) {}
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
    fallbacks: Vec<ResilientFallback>,
    health: Arc<CooldownTracker>,
    reporter: Arc<dyn CircuitReporter>,
    policy: FailoverRoutingPolicy,
}

struct ResilientFallback {
    label: String,
    pricing_provider: String,
    model: String,
    provider: Arc<dyn LlmProvider>,
    metadata: FailoverCandidateMetadata,
    health: Arc<CooldownTracker>,
}

impl ResilientProvider {
    pub fn new(
        primary_name: impl Into<String>,
        primary: Arc<dyn LlmProvider>,
        fallbacks: Vec<(String, Arc<dyn LlmProvider>)>,
        cfg: CircuitConfig,
        reporter: Arc<dyn CircuitReporter>,
    ) -> Self {
        let fallbacks = fallbacks
            .into_iter()
            .map(|(label, provider)| (label.clone(), label.clone(), label, provider))
            .collect();
        Self::new_with_fallback_identities(primary_name, primary, fallbacks, cfg, reporter)
    }

    pub fn new_with_fallback_identities(
        primary_name: impl Into<String>,
        primary: Arc<dyn LlmProvider>,
        fallbacks: Vec<(String, String, String, Arc<dyn LlmProvider>)>,
        cfg: CircuitConfig,
        reporter: Arc<dyn CircuitReporter>,
    ) -> Self {
        let candidates = fallbacks
            .into_iter()
            .map(|(label, pricing_provider, model, provider)| {
                // B02-D1 — the SAME kernel the typed constructor uses. Reading
                // `model_output_ceiling` alone here made every Flux tier alias
                // (deliberately absent from that table, CORE-4) an inadmissible
                // failover candidate. Two metadata builders, one resolver, so
                // the defect cannot be reintroduced in half the tree. The
                // legacy tuple carries no `Config`, hence `config_window = 0`.
                let context_window = wcore_config::context_window::ContextWindow::resolve(
                    0,
                    &pricing_provider,
                    &model,
                    0,
                )
                .window;
                let metadata = FailoverCandidateMetadata {
                    label,
                    provider: pricing_provider,
                    model,
                    organization: None,
                    region: None,
                    // Compatibility was not expressible in the legacy tuple.
                    // Preserve its behavior; production bootstrap uses the
                    // typed constructor below with fail-closed metadata.
                    capabilities: CandidateCapabilities {
                        tools: true,
                        vision: true,
                        structured_output: true,
                        context_window,
                    },
                    pricing: PricingEvidence::default(),
                };
                (metadata, provider)
            })
            .collect();
        Self::new_with_policy(
            primary_name,
            primary,
            candidates,
            cfg,
            reporter,
            FailoverRoutingPolicy::default(),
        )
    }

    pub fn new_with_policy(
        primary_name: impl Into<String>,
        primary: Arc<dyn LlmProvider>,
        fallbacks: Vec<(FailoverCandidateMetadata, Arc<dyn LlmProvider>)>,
        cfg: CircuitConfig,
        reporter: Arc<dyn CircuitReporter>,
        policy: FailoverRoutingPolicy,
    ) -> Self {
        let threshold = u32::try_from(cfg.fail_threshold).unwrap_or(u32::MAX);
        let transient_base = cfg.cooldown;
        Self {
            primary_name: primary_name.into(),
            primary,
            fallbacks: fallbacks
                .into_iter()
                .map(|(metadata, provider)| ResilientFallback {
                    label: metadata.label.clone(),
                    pricing_provider: metadata.provider.clone(),
                    model: metadata.model.clone(),
                    provider,
                    metadata,
                    health: Arc::new(CooldownTracker::with_failure_threshold_and_base(
                        threshold,
                        transient_base,
                    )),
                })
                .collect(),
            health: Arc::new(CooldownTracker::with_failure_threshold_and_base(
                threshold,
                transient_base,
            )),
            reporter,
            policy,
        }
    }

    /// How many `CooldownTracker`s this provider actually constructed: one for
    /// the primary plus one per admitted fallback candidate.
    ///
    /// Exists so the F05 capability report (`F05-TRUTH-3`) can state a measured
    /// fact about the object that was built instead of asserting a literal at
    /// the report site. A reader that wants "is the cooldown tracker
    /// constructed" asks the thing that would own it.
    pub fn cooldown_tracker_count(&self) -> usize {
        1 + self.fallbacks.len()
    }

    /// F32: forward every event from the primary's stream onto a fresh channel,
    /// recording the breaker verdict only when the stream terminates:
    /// `Done` → success (closes a HalfOpen trial), a terminal `Error` (or the
    /// channel closing with no `Done`) → failure. This prevents a provider that
    /// always accepts headers then dies mid-body from looking permanently
    /// healthy. Events are passed through unmodified.
    fn spawn_health_forwarder(
        mut rx: mpsc::Receiver<LlmEvent>,
        health: Arc<CooldownTracker>,
        reporter: Arc<dyn CircuitReporter>,
        primary_name: String,
        fallback: Option<String>,
        was_probe: bool,
    ) -> mpsc::Receiver<LlmEvent> {
        let (tx, out_rx) = mpsc::channel(32);
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
                health.record_success();
                if was_probe {
                    reporter.report(
                        &primary_name,
                        fallback.as_deref(),
                        CircuitState::Closed,
                        None,
                    );
                }
            } else {
                health.record_failure(FailoverReason::Unknown, None);
                reporter.report(
                    &primary_name,
                    fallback.as_deref(),
                    CircuitState::Open,
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
        let requirements = RequestRequirements::from_request(request);
        let mut previous_provider = self.primary_name.as_str();
        let mut previous_attempted = false;
        let mut last_error = None;
        let mut receipt;

        let primary_state = self.health.state();
        if let Some(permit) = self.health.try_acquire() {
            match crate::attempt_lifecycle::scope_provider_attempt_identity(
                self.primary_name.clone(),
                request.model.clone(),
                self.primary.stream(request),
            )
            .await
            {
                Ok(rx) => {
                    return Ok(Self::spawn_health_forwarder(
                        rx,
                        Arc::clone(&self.health),
                        Arc::clone(&self.reporter),
                        self.primary_name.clone(),
                        None,
                        permit == CooldownPermit::HalfOpen,
                    ));
                }
                Err(e) if is_request_fatal(&e) => return Err(e),
                Err(e) => {
                    let reason = classify_error(&e);
                    if should_trip_breaker(&e) {
                        self.health.record_failure(reason, retry_after(&e));
                        if matches!(self.health.state(), CooldownState::Cooling { .. }) {
                            self.reporter.report(
                                &self.primary_name,
                                None,
                                CircuitState::Open,
                                Some(&e.to_string()),
                            );
                        }
                    } else {
                        self.health.record_failure(reason, None);
                    }
                    previous_attempted = e.is_retryable()
                        || crate::retry::configured_fallback_previous_attempted(&e);
                    receipt = FailoverReceipt::new(
                        reason,
                        self.primary_name.clone(),
                        request.model.clone(),
                    );
                    if self.fallbacks.is_empty() {
                        return Err(e);
                    }
                    last_error = Some(e);
                }
            }
        } else {
            self.reporter.report(
                &self.primary_name,
                self.fallbacks
                    .first()
                    .map(|fallback| fallback.label.as_str()),
                CircuitState::Open,
                Some("circuit open; skipping primary"),
            );
            if self.fallbacks.is_empty() {
                return Err(ProviderError::NotAttempted {
                    reason: "primary circuit is open and no fallback is configured".into(),
                });
            }
            let reason = match primary_state {
                CooldownState::Cooling { reason, .. } | CooldownState::HalfOpen { reason } => {
                    reason
                }
                CooldownState::Ready => FailoverReason::Unknown,
            };
            receipt =
                FailoverReceipt::new(reason, self.primary_name.clone(), request.model.clone());
        }

        for fallback in &self.fallbacks {
            let policy_disposition =
                evaluate_candidate(&fallback.metadata, requirements, &self.policy);
            if let Err(rejection) = policy_disposition {
                receipt.candidates.push(CandidateReceipt {
                    provider: fallback.pricing_provider.clone(),
                    model: fallback.model.clone(),
                    region: fallback.metadata.region.clone(),
                    disposition: Err(rejection),
                    failure_reason: None,
                    cooldown_reason: None,
                    retry_after_ms: None,
                    pricing: fallback.metadata.pricing.clone(),
                });
                continue;
            }

            let fallback_state = fallback.health.state();
            let Some(permit) = fallback.health.try_acquire() else {
                let cooldown_reason = match fallback_state {
                    CooldownState::Cooling { reason, .. } | CooldownState::HalfOpen { reason } => {
                        Some(reason)
                    }
                    CooldownState::Ready => None,
                };
                receipt.candidates.push(CandidateReceipt {
                    provider: fallback.pricing_provider.clone(),
                    model: fallback.model.clone(),
                    region: fallback.metadata.region.clone(),
                    disposition: Err(CandidateRejection::CooldownActive),
                    failure_reason: None,
                    cooldown_reason,
                    retry_after_ms: fallback.health.retry_after().map(duration_millis),
                    pricing: fallback.metadata.pricing.clone(),
                });
                continue;
            };

            let admission = match crate::retry::admit_configured_fallback(
                previous_provider,
                &fallback.label,
                &fallback.pricing_provider,
                &fallback.model,
                previous_attempted,
            ) {
                Ok(admission) => admission,
                Err(error) => {
                    receipt.candidates.push(CandidateReceipt {
                        provider: fallback.pricing_provider.clone(),
                        model: fallback.model.clone(),
                        region: fallback.metadata.region.clone(),
                        disposition: Err(CandidateRejection::BudgetDenied),
                        failure_reason: None,
                        cooldown_reason: None,
                        retry_after_ms: None,
                        pricing: fallback.metadata.pricing.clone(),
                    });
                    self.reporter.report_failover(&receipt);
                    return Err(error);
                }
            };
            let mut pricing = fallback.metadata.pricing.clone();
            if admission.estimated_microcents.is_some() {
                pricing.estimated_microcents = admission.estimated_microcents;
            }

            let mut fallback_request = request.clone();
            fallback_request.model.clone_from(&fallback.model);
            match crate::attempt_lifecycle::scope_provider_attempt_identity(
                fallback.pricing_provider.clone(),
                fallback.model.clone(),
                fallback.provider.stream(&fallback_request),
            )
            .await
            {
                Ok(rx) => {
                    receipt.candidates.push(CandidateReceipt {
                        provider: fallback.pricing_provider.clone(),
                        model: fallback.model.clone(),
                        region: fallback.metadata.region.clone(),
                        disposition: Ok(()),
                        failure_reason: None,
                        cooldown_reason: None,
                        retry_after_ms: None,
                        pricing,
                    });
                    receipt.selected_provider = Some(fallback.pricing_provider.clone());
                    receipt.selected_model = Some(fallback.model.clone());
                    self.reporter.report_failover(&receipt);
                    self.reporter.report(
                        &self.primary_name,
                        Some(&fallback.label),
                        CircuitState::Open,
                        None,
                    );
                    return Ok(Self::spawn_health_forwarder(
                        rx,
                        Arc::clone(&fallback.health),
                        Arc::clone(&self.reporter),
                        self.primary_name.clone(),
                        Some(fallback.label.clone()),
                        permit == CooldownPermit::HalfOpen,
                    ));
                }
                Err(e) => {
                    let reason = classify_error(&e);
                    fallback.health.record_failure(reason, retry_after(&e));
                    receipt.candidates.push(CandidateReceipt {
                        provider: fallback.pricing_provider.clone(),
                        model: fallback.model.clone(),
                        region: fallback.metadata.region.clone(),
                        disposition: Ok(()),
                        failure_reason: Some(reason),
                        cooldown_reason: None,
                        retry_after_ms: fallback.health.retry_after().map(duration_millis),
                        pricing,
                    });
                    if is_request_fatal(&e) {
                        self.reporter.report_failover(&receipt);
                        return Err(e);
                    }
                    previous_provider = &fallback.label;
                    previous_attempted = e.is_retryable()
                        || crate::retry::configured_fallback_previous_attempted(&e);
                    last_error = Some(e);
                }
            }
        }
        self.reporter.report_failover(&receipt);
        Err(match last_error {
            // B02-R3(a) — the COMMON case. On the first failing turn the
            // primary is attempted, `last_error` is `Some`, and the chain's
            // facts were thrown away: the operator got a bare "Connection
            // error: ..." with no hint that two configured fallbacks had been
            // refused. The typed error still decides retry and billing, so the
            // VARIANT is preserved exactly and only its sentence grows.
            Some(error) => annotate_with_chain(error, &receipt),
            // The primary was SKIPPED (circuit open), so there is no typed
            // error to preserve and the chain's own message is the whole story.
            None => ProviderError::NotAttempted {
                reason: describe_exhausted_chain(&receipt),
            },
        })
    }
}

/// B02-D3 — turn the receipt the chain already built into the sentence the
/// operator reads.
///
/// `emit_provider_failover_receipt` is overridden only by the JSON-stream
/// protocol sink, so on the CLI and TUI the receipt was reported and then
/// discarded and the user got a fixed string naming no candidate, no reason
/// and no remedy. Everything here is already on the receipt that ships to the
/// host, so this discloses nothing new — and it deliberately carries ONLY
/// provider, model and the rejection slug. Never a base_url, a credential, or
/// a policy value.
///
/// Capped at [`MAX_DESCRIBED_CANDIDATES`] entries plus a count of the rest:
/// this string reaches logs and the session journal, so a long chain must not
/// become a multi-kilobyte line.
fn describe_exhausted_chain(receipt: &FailoverReceipt) -> String {
    format!(
        "no fallback candidate was eligible after {}/{} failed ({}): {}{}",
        receipt.failed_provider,
        receipt.failed_model,
        receipt.reason,
        describe_chain_candidates(receipt),
        CHAIN_REMEDY
    )
}

/// B02-R3(a) — keep the typed error, add the chain's facts to its sentence.
///
/// `describe_exhausted_chain` was reachable only through
/// `last_error.unwrap_or_else(...)`, i.e. only when the primary was SKIPPED
/// because its circuit was already open. On the turn that actually breaks —
/// the primary is attempted, fails, and every fallback is then refused by
/// policy — `last_error` is `Some` and the operator saw the primary's bare
/// message with no sign that a chain had been consulted at all.
///
/// The variant is preserved deliberately. `is_retryable` and
/// `was_not_attempted` drive the retry loop and the billing disposition;
/// rewrapping a `Connection` into a `NotAttempted` to carry a string would
/// change both. Only the free-text field grows, so classification cannot move.
///
/// Variants with no free-text field to grow (`Http` and `Egress`, which wrap a
/// foreign error; `RateLimited` and `MissingApiKey`, which are pure data) are
/// returned untouched — the receipt still reaches the host sink.
fn annotate_with_chain(error: ProviderError, receipt: &FailoverReceipt) -> ProviderError {
    let suffix = format!(
        " \u{2014} fallback chain exhausted: {}{}",
        describe_chain_candidates(receipt),
        CHAIN_REMEDY
    );
    match error {
        ProviderError::Api { status, message } => ProviderError::Api {
            status,
            message: message + &suffix,
        },
        ProviderError::Parse(message) => ProviderError::Parse(message + &suffix),
        ProviderError::Connection(message) => ProviderError::Connection(message + &suffix),
        ProviderError::PromptTooLong(message) => ProviderError::PromptTooLong(message + &suffix),
        ProviderError::NotAttempted { reason } => ProviderError::NotAttempted {
            reason: reason + &suffix,
        },
        ProviderError::PremiumLocked {
            capability,
            message,
        } => ProviderError::PremiumLocked {
            capability,
            message: message + &suffix,
        },
        ProviderError::UpgradeRequired { message } => ProviderError::UpgradeRequired {
            message: message + &suffix,
        },
        ProviderError::SpendCeilingUnresolved {
            reason,
            upgrade_url,
        } => ProviderError::SpendCeilingUnresolved {
            reason: reason + &suffix,
            upgrade_url,
        },
        ProviderError::ContextOverflow {
            required_tokens,
            model_window,
            routed_model,
            message,
        } => ProviderError::ContextOverflow {
            required_tokens,
            model_window,
            routed_model,
            message: message + &suffix,
        },
        error @ (ProviderError::Http(_)
        | ProviderError::Egress(_)
        | ProviderError::RateLimited { .. }
        | ProviderError::MissingApiKey) => error,
    }
}

/// The remedy sentence shared by both terminal messages, so the operator is
/// told the same thing whichever branch produced the error.
const CHAIN_REMEDY: &str = ". Point [provider_chain] fallback_models at a model \
     this build can size, or wait out the cooldown.";

/// Render the candidates the chain considered: what each one was, and whether
/// it was refused (and why) or attempted (and how it failed). Capped at
/// [`MAX_DESCRIBED_CANDIDATES`] plus a count of the rest.
fn describe_chain_candidates(receipt: &FailoverReceipt) -> String {
    let mut out = String::new();
    let shown = receipt.candidates.len().min(MAX_DESCRIBED_CANDIDATES);
    for (i, candidate) in receipt.candidates.iter().take(shown).enumerate() {
        if i > 0 {
            out.push_str("; ");
        }
        out.push_str(&describe_candidate(candidate));
    }
    if receipt.candidates.is_empty() {
        out.push_str("no candidates were configured");
    }
    let elided = receipt.candidates.len().saturating_sub(shown);
    if elided > 0 {
        out.push_str(&format!(" (+{elided} more)"));
    }
    out
}

/// One candidate's line: what it was, and whether it was refused before any
/// call (and why) or physically attempted (and how that ended).
///
/// Shared by the single-line error suffix and the multi-line terminal receipt
/// so an operator reading either one sees the same word for the same decision.
fn describe_candidate(candidate: &CandidateReceipt) -> String {
    match candidate.disposition {
        Err(rejection) => format!(
            "{}/{} rejected \u{2014} {}",
            candidate.provider, candidate.model, rejection
        ),
        Ok(()) => format!(
            "{}/{} attempted \u{2014} {}",
            candidate.provider,
            candidate.model,
            match candidate.failure_reason {
                Some(reason) => reason.as_str(),
                // Reachable only from the receipt of a SUCCESSFUL failover,
                // where the last candidate is the one that served the turn.
                // The error paths never build such a candidate, so the
                // single-line suffix above is unchanged by this arm.
                None => "succeeded",
            }
        ),
    }
}

/// R3(b) — render one failover receipt as the block a terminal operator reads.
///
/// `OutputSink::emit_provider_failover_receipt` is overridden only by the
/// JSON-stream sink, so a Desktop host saw which candidates were refused and
/// why while a CLI or TUI user saw nothing at all. This is the CLI/TUI
/// rendering of the SAME receipt that already ships to the host, so it
/// discloses nothing new — and like [`describe_chain_candidates`] it carries
/// ONLY provider, model, the rejection slug and the retry hint. Never a
/// base_url, a credential, or a policy value.
///
/// Capped at [`MAX_DESCRIBED_CANDIDATES`] entries plus a count of the rest:
/// this text reaches the session journal, so a long chain must not become a
/// multi-kilobyte block.
pub fn describe_failover_receipt(receipt: &FailoverReceipt) -> String {
    let mut out = format!(
        "provider failover: {}/{} failed ({})",
        receipt.failed_provider, receipt.failed_model, receipt.reason
    );
    let shown = receipt.candidates.len().min(MAX_DESCRIBED_CANDIDATES);
    for candidate in receipt.candidates.iter().take(shown) {
        out.push_str("\n  - ");
        out.push_str(&describe_candidate(candidate));
        if let Some(retry_after_ms) = candidate.retry_after_ms {
            out.push_str(&format!(" (retry after {retry_after_ms}ms)"));
        }
    }
    if receipt.candidates.is_empty() {
        out.push_str("\n  - no candidates were configured");
    }
    let elided = receipt.candidates.len().saturating_sub(shown);
    if elided > 0 {
        out.push_str(&format!("\n  - (+{elided} more)"));
    }
    match (&receipt.selected_provider, &receipt.selected_model) {
        // The turn was served, by something other than what the operator
        // asked for. Naming it is the whole point: cost, context window and
        // tool support all just changed under them.
        (Some(provider), Some(model)) => {
            out.push_str(&format!("\n  -> routed to {provider}/{model}"));
        }
        // Nothing served the turn. Same remedy sentence the error carries, so
        // the two never disagree.
        _ => out.push_str(&format!(
            "\n  -> no fallback candidate was eligible{CHAIN_REMEDY}"
        )),
    }
    out
}

/// How many candidates the terminal error names before eliding the tail.
const MAX_DESCRIBED_CANDIDATES: usize = 5;

#[cfg(test)]
mod receipt_rendering_tests {
    use super::*;

    fn refused(provider: &str, model: &str, why: CandidateRejection) -> CandidateReceipt {
        CandidateReceipt {
            provider: provider.into(),
            model: model.into(),
            region: None,
            disposition: Err(why),
            failure_reason: None,
            cooldown_reason: None,
            retry_after_ms: None,
            pricing: PricingEvidence::default(),
        }
    }

    /// Platform-neutral coverage of the block a CLI/TUI operator reads. The
    /// terminal wiring itself is proven on unix by
    /// `wcore-agent/tests/terminal_failover_receipt_test.rs`, which reads fd 2.
    #[test]
    fn an_exhausted_chain_renders_every_refusal_and_the_remedy() {
        let mut receipt =
            FailoverReceipt::new(FailoverReason::RateLimit, "anthropic", "claude-sonnet-4-6");
        receipt.candidates.push(refused(
            "openai",
            "gpt-5",
            CandidateRejection::ContextWindowUnknown,
        ));
        receipt.candidates.push(refused(
            "google",
            "gemini-3-pro",
            CandidateRejection::CooldownActive,
        ));

        assert_eq!(
            describe_failover_receipt(&receipt),
            "provider failover: anthropic/claude-sonnet-4-6 failed (rate_limit)\n  \
             - openai/gpt-5 rejected \u{2014} context_window_unknown\n  \
             - google/gemini-3-pro rejected \u{2014} cooldown_active\n  \
             -> no fallback candidate was eligible. Point [provider_chain] \
             fallback_models at a model this build can size, or wait out the cooldown."
        );
    }

    /// A SUCCESSFUL failover is the case with no error to piggyback on: the
    /// operator is silently on another provider unless this block says so.
    #[test]
    fn a_successful_failover_names_the_route_and_omits_the_reconfigure_remedy() {
        let mut receipt =
            FailoverReceipt::new(FailoverReason::Overloaded, "anthropic", "claude-opus-4-1");
        receipt.candidates.push(CandidateReceipt {
            provider: "openai".into(),
            model: "gpt-5".into(),
            region: Some("us-east".into()),
            disposition: Ok(()),
            failure_reason: None,
            cooldown_reason: None,
            retry_after_ms: None,
            pricing: PricingEvidence::default(),
        });
        receipt.selected_provider = Some("openai".into());
        receipt.selected_model = Some("gpt-5".into());

        assert_eq!(
            describe_failover_receipt(&receipt),
            "provider failover: anthropic/claude-opus-4-1 failed (overloaded)\n  \
             - openai/gpt-5 attempted \u{2014} succeeded\n  \
             -> routed to openai/gpt-5"
        );
    }

    /// The block reaches the session journal, so a long chain must be bounded.
    /// A cooling candidate must also carry its retry hint — "wait out the
    /// cooldown" is not actionable without knowing how long.
    #[test]
    fn a_long_chain_is_elided_and_a_cooling_candidate_carries_its_retry_hint() {
        let mut receipt = FailoverReceipt::new(FailoverReason::Timeout, "anthropic", "claude");
        let mut cooling = refused("p0", "m0", CandidateRejection::CooldownActive);
        cooling.retry_after_ms = Some(1_500);
        receipt.candidates.push(cooling);
        for i in 1..8 {
            receipt.candidates.push(refused(
                &format!("p{i}"),
                &format!("m{i}"),
                CandidateRejection::ProviderDenied,
            ));
        }

        let rendered = describe_failover_receipt(&receipt);
        assert!(
            rendered.contains("p0/m0 rejected \u{2014} cooldown_active (retry after 1500ms)"),
            "the retry hint is missing: {rendered}"
        );
        assert!(
            rendered.contains("(+3 more)"),
            "8 candidates were not elided to {MAX_DESCRIBED_CANDIDATES}: {rendered}"
        );
        assert!(
            !rendered.contains("p6/"),
            "an elided candidate was still printed: {rendered}"
        );
    }

    /// An empty chain must still say something an operator can act on.
    #[test]
    fn an_empty_chain_says_so_rather_than_rendering_a_bare_header() {
        let receipt = FailoverReceipt::new(FailoverReason::Auth, "anthropic", "claude");
        let rendered = describe_failover_receipt(&receipt);
        assert!(
            rendered.contains("no candidates were configured"),
            "an empty chain rendered nothing actionable: {rendered}"
        );
        assert!(
            rendered.contains("provider_chain"),
            "an empty chain did not name the setting to fix: {rendered}"
        );
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

    #[derive(Default)]
    struct ReceiptReporter {
        receipts: Mutex<Vec<FailoverReceipt>>,
    }
    impl CircuitReporter for ReceiptReporter {
        fn report(&self, _: &str, _: Option<&str>, _: CircuitState, _: Option<&str>) {}

        fn report_failover(&self, receipt: &FailoverReceipt) {
            self.receipts.lock().push(receipt.clone());
        }
    }

    fn candidate(
        label: &str,
        tools: bool,
        context_window: Option<u64>,
    ) -> FailoverCandidateMetadata {
        FailoverCandidateMetadata {
            label: label.into(),
            provider: label.into(),
            model: label.into(),
            organization: None,
            region: None,
            capabilities: CandidateCapabilities {
                tools,
                vision: true,
                structured_output: true,
                context_window,
            },
            pricing: PricingEvidence {
                source: "test".into(),
                age_seconds: Some(0),
                stale: false,
                priced: true,
                estimated_microcents: Some(1),
            },
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

    #[tokio::test]
    async fn incompatible_candidate_is_never_called_and_receipt_selects_next() {
        struct CountOk(AtomicUsize);
        #[async_trait]
        impl LlmProvider for CountOk {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(ok_done_channel())
            }
        }

        let incompatible = Arc::new(CountOk(AtomicUsize::new(0)));
        let compatible = Arc::new(CountOk(AtomicUsize::new(0)));
        let reporter = Arc::new(ReceiptReporter::default());
        let resilient = ResilientProvider::new_with_policy(
            "primary",
            Arc::new(AlwaysFail),
            vec![
                (
                    candidate("no-tools", false, Some(100_000)),
                    incompatible.clone(),
                ),
                (candidate("tools", true, Some(100_000)), compatible.clone()),
            ],
            CircuitConfig::default(),
            reporter.clone(),
            FailoverRoutingPolicy::default(),
        );
        let mut request = dummy_request();
        request.tools.push(wcore_types::tool::ToolDef {
            name: "read".into(),
            ..Default::default()
        });

        let admitter: crate::retry::ConfiguredFallbackAdmitter = Arc::new(|_, _, _, _, _| {
            Ok(crate::retry::ConfiguredFallbackAdmission {
                estimated_microcents: Some(77),
            })
        });
        crate::retry::scope_configured_fallback_admitter(admitter, resilient.stream(&request))
            .await
            .unwrap();

        assert_eq!(incompatible.0.load(Ordering::SeqCst), 0);
        assert_eq!(compatible.0.load(Ordering::SeqCst), 1);
        let receipts = reporter.receipts.lock();
        assert_eq!(receipts.len(), 1);
        assert_eq!(
            receipts[0].candidates[0].disposition,
            Err(CandidateRejection::ToolsUnsupported)
        );
        assert_eq!(receipts[0].selected_provider.as_deref(), Some("tools"));
        assert_eq!(receipts[0].selected_model.as_deref(), Some("tools"));
        assert_eq!(
            receipts[0].candidates[1].pricing.estimated_microcents,
            Some(77)
        );
    }

    #[tokio::test]
    async fn context_overflow_routes_only_to_a_proven_larger_window() {
        struct Overflow;
        #[async_trait]
        impl LlmProvider for Overflow {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                Err(ProviderError::PromptTooLong("too large".into()))
            }
        }
        struct CountOk(AtomicUsize);
        #[async_trait]
        impl LlmProvider for CountOk {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(ok_done_channel())
            }
        }

        let small = Arc::new(CountOk(AtomicUsize::new(0)));
        let large = Arc::new(CountOk(AtomicUsize::new(0)));
        let resilient = ResilientProvider::new_with_policy(
            "primary",
            Arc::new(Overflow),
            vec![
                (candidate("small", true, Some(10_000)), small.clone()),
                (candidate("large", true, Some(100_000)), large.clone()),
            ],
            CircuitConfig::default(),
            Arc::new(NoOpCircuitReporter),
            FailoverRoutingPolicy::default(),
        );
        let mut request = dummy_request();
        request.client_context_tokens = Some(50_000);

        resilient.stream(&request).await.unwrap();

        assert_eq!(small.0.load(Ordering::SeqCst), 0);
        assert_eq!(large.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn permanent_candidate_cooldown_is_named_and_never_dispatched() {
        struct CountOk(AtomicUsize);
        #[async_trait]
        impl LlmProvider for CountOk {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(ok_done_channel())
            }
        }

        let fallback = Arc::new(CountOk(AtomicUsize::new(0)));
        let reporter = Arc::new(ReceiptReporter::default());
        let resilient = ResilientProvider::new_with_policy(
            "primary",
            Arc::new(AlwaysFail),
            vec![(candidate("fallback", true, Some(100_000)), fallback.clone())],
            CircuitConfig::default(),
            reporter.clone(),
            FailoverRoutingPolicy::default(),
        );
        resilient.fallbacks[0]
            .health
            .record_failure(FailoverReason::AuthPermanent, None);

        resilient.stream(&dummy_request()).await.unwrap_err();

        assert_eq!(fallback.0.load(Ordering::SeqCst), 0);
        let receipts = reporter.receipts.lock();
        assert_eq!(
            receipts[0].candidates[0].disposition,
            Err(CandidateRejection::CooldownActive)
        );
        assert_eq!(
            receipts[0].candidates[0].cooldown_reason,
            Some(FailoverReason::AuthPermanent)
        );
        assert_eq!(receipts[0].candidates[0].retry_after_ms, None);
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

    #[tokio::test]
    async fn scoped_zero_preserves_configured_resilient_fallback() {
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

        struct CountingOk {
            calls: AtomicUsize,
        }
        #[async_trait]
        impl LlmProvider for CountingOk {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(ok_done_channel())
            }
        }

        let primary = Arc::new(CountingFail {
            calls: AtomicUsize::new(0),
        });
        let fallback = Arc::new(CountingOk {
            calls: AtomicUsize::new(0),
        });
        let admissions = Arc::new(AtomicUsize::new(0));
        let resilient = ResilientProvider::new(
            "primary",
            primary.clone(),
            vec![("fallback".into(), fallback.clone())],
            CircuitConfig::default(),
            Arc::new(NoOpCircuitReporter),
        );

        let admission_count = Arc::clone(&admissions);
        let admitter: crate::retry::ConfiguredFallbackAdmitter =
            Arc::new(move |previous, next, _, _, previous_attempted| {
                assert_eq!((previous, next), ("primary", "fallback"));
                assert!(previous_attempted);
                admission_count.fetch_add(1, Ordering::SeqCst);
                Ok(Default::default())
            });
        let result = crate::retry::scope_configured_fallback_admitter(
            admitter,
            crate::retry::scope_max_retries(0, resilient.stream(&dummy_request())),
        )
        .await;

        result.expect("configured fallback must remain available when nested retries are disabled");
        assert_eq!(primary.calls.load(Ordering::SeqCst), 1);
        assert_eq!(admissions.load(Ordering::SeqCst), 1);
        assert_eq!(
            fallback.calls.load(Ordering::SeqCst),
            1,
            "zero-retry scope limits each provider send, not configured fallback order"
        );
    }

    #[tokio::test]
    async fn configured_fallback_admission_uses_authoritative_pricing_identity() {
        struct ModelCapture {
            seen_models: Arc<Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl LlmProvider for ModelCapture {
            async fn stream(
                &self,
                request: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                self.seen_models.lock().push(request.model.clone());
                Ok(ok_done_channel())
            }
        }

        let seen_models = Arc::new(Mutex::new(Vec::new()));
        let resilient = ResilientProvider::new_with_fallback_identities(
            "primary",
            Arc::new(AlwaysFail),
            vec![(
                "haiku".into(),
                "anthropic".into(),
                "claude-haiku-4-5".into(),
                Arc::new(ModelCapture {
                    seen_models: Arc::clone(&seen_models),
                }) as Arc<dyn LlmProvider>,
            )],
            CircuitConfig::default(),
            Arc::new(NoOpCircuitReporter),
        );
        let admissions = Arc::new(AtomicUsize::new(0));
        let admission_count = Arc::clone(&admissions);
        let admitter: crate::retry::ConfiguredFallbackAdmitter = Arc::new(
            move |previous, label, pricing_provider, model, previous_attempted| {
                assert_eq!(previous, "primary");
                assert_eq!(label, "haiku");
                assert_eq!(pricing_provider, "anthropic");
                assert_eq!(model, "claude-haiku-4-5");
                assert!(previous_attempted);
                admission_count.fetch_add(1, Ordering::SeqCst);
                Ok(Default::default())
            },
        );

        let mut request = dummy_request();
        request.model = "claude-opus-4-6".into();
        let result =
            crate::retry::scope_configured_fallback_admitter(admitter, resilient.stream(&request))
                .await;

        result.expect("fallback must receive its canonical pricing identity");
        assert_eq!(admissions.load(Ordering::SeqCst), 1);
        assert_eq!(seen_models.lock().as_slice(), ["claude-haiku-4-5"]);
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

    #[tokio::test]
    async fn open_circuit_without_fallback_reports_that_no_request_was_attempted() {
        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(AlwaysFail),
            vec![],
            CircuitConfig {
                fail_threshold: 1,
                window: Duration::from_secs(30),
                cooldown: Duration::from_secs(60),
            },
            Arc::new(NoOpCircuitReporter),
        );

        assert!(matches!(
            resilient.stream(&dummy_request()).await,
            Err(ProviderError::Connection(_))
        ));
        assert!(matches!(
            resilient.stream(&dummy_request()).await,
            Err(ProviderError::NotAttempted { .. })
        ));
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
        let admissions = Arc::new(AtomicUsize::new(0));
        let admission_count = Arc::clone(&admissions);
        let admitter: crate::retry::ConfiguredFallbackAdmitter =
            Arc::new(move |previous, next, _, _, previous_attempted| {
                assert_eq!((previous, next), ("primary", "fb"));
                assert!(
                    !previous_attempted,
                    "an open circuit skipped the primary without a paid send"
                );
                admission_count.fetch_add(1, Ordering::SeqCst);
                Ok(Default::default())
            });
        // A subsequent call with the circuit Open must skip the primary
        // entirely and still succeed via the fallback.
        assert!(
            crate::retry::scope_configured_fallback_admitter(
                admitter,
                resilient.stream(&dummy_request()),
            )
            .await
            .is_ok(),
            "fallback must serve the request once the primary circuit is open"
        );
        assert_eq!(admissions.load(Ordering::SeqCst), 1);
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

    #[tokio::test]
    async fn final_fallback_preserves_typed_no_send_error() {
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

        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(AlwaysFail),
            vec![(
                "no-key".into(),
                Arc::new(MissingKey) as Arc<dyn LlmProvider>,
            )],
            CircuitConfig::default(),
            Arc::new(NoOpCircuitReporter),
        );

        assert!(matches!(
            resilient.stream(&dummy_request()).await,
            Err(ProviderError::MissingApiKey)
        ));
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
        let admissions = Arc::new(AtomicUsize::new(0));
        let admission_count = Arc::clone(&admissions);
        let admitter: crate::retry::ConfiguredFallbackAdmitter =
            Arc::new(move |previous, next, _, _, previous_attempted| {
                assert_eq!((previous, next), ("primary", "good"));
                assert!(
                    !previous_attempted,
                    "MissingApiKey proves the primary never sent a paid request"
                );
                admission_count.fetch_add(1, Ordering::SeqCst);
                Ok(Default::default())
            });
        assert!(
            crate::retry::scope_configured_fallback_admitter(
                admitter,
                resilient.stream(&dummy_request()),
            )
            .await
            .is_ok(),
            "a non-retryable primary (MissingApiKey) must fall through to the \
             working fallback, not abort before the chain is tried"
        );
        assert_eq!(admissions.load(Ordering::SeqCst), 1);
    }

    /// F20/F15: a malformed HTTP 400 request would fail on every provider, so
    /// the chain aborts immediately. Context overflow is no longer in this
    /// class: F15 can admit a later model with a proven larger window.
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
                    status: 400,
                    message: "malformed request".into(),
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
            matches!(err, ProviderError::Api { status: 400, .. }),
            "the request-fatal 400 must surface and abort the chain"
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
        assert_eq!(
            resilient.health.state(),
            CooldownState::Ready,
            "semantic errors must not make the primary unavailable"
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

    // ── B02-D3: the terminal error must carry the receipt's facts ───────────
    //
    // When every candidate is refused, the receipt naming WHICH candidate was
    // refused and WHY is handed to `report_failover` — and then thrown away.
    // The receipt only survives if the sink renders it, and
    // `OutputSink::emit_provider_failover_receipt` is overridden in exactly one
    // place: the JSON-stream protocol sink. A CLI or TUI operator therefore got
    // "Provider request was not attempted: no configured fallback candidate
    // passed routing policy" — no candidate, no reason, no remedy.
    //
    // That exact string is produced on the circuit-OPEN path: the primary is
    // skipped, so `last_error` is never set and the chain's own message is all
    // the user gets. These tests drive that path.

    /// Trip the primary's breaker, then take the circuit-open path with two
    /// candidates refused for DIFFERENT reasons — so the assertions cannot be
    /// satisfied by hardcoding one rejection into the message.
    async fn exhausted_chain_error() -> ProviderError {
        let unknown_window = candidate("flux-standard", true, None);
        let mut denied = candidate("blocked-model", true, Some(400_000));
        denied.provider = "denied-provider".into();

        let resilient = ResilientProvider::new_with_policy(
            "primary",
            Arc::new(AlwaysFail),
            vec![
                (unknown_window, Arc::new(AlwaysOk) as Arc<dyn LlmProvider>),
                (denied, Arc::new(AlwaysOk) as Arc<dyn LlmProvider>),
            ],
            CircuitConfig {
                fail_threshold: 1,
                window: Duration::from_secs(30),
                cooldown: Duration::from_secs(60),
            },
            Arc::new(ReceiptReporter::default()),
            FailoverRoutingPolicy {
                denied_providers: std::collections::BTreeSet::from(["denied-provider".to_string()]),
                ..Default::default()
            },
        );
        let mut request = dummy_request();
        request.client_context_tokens = Some(8_000);

        // Turn 1 attempts and trips the breaker; the error the user sees is the
        // primary's own. Turn 2 skips the primary entirely — that is the state
        // the repro was in, and the only one that reaches the chain's message.
        let _ = resilient.stream(&request).await;
        resilient
            .stream(&request)
            .await
            .expect_err("circuit is open and every candidate is refused")
    }

    #[tokio::test]
    async fn an_exhausted_chain_names_each_candidate_and_why_it_was_refused() {
        let message = exhausted_chain_error().await.to_string();

        for needle in [
            // the candidate that was refused …
            "flux-standard",
            // … and the reason, in the same vocabulary the receipt uses.
            "context_window_unknown",
            // the SECOND candidate, refused for a DIFFERENT reason — a message
            // that names only the first candidate, or only one rejection kind,
            // fails here.
            "denied-provider",
            "blocked-model",
            "provider_denied",
            // and what the primary was doing when the chain was entered.
            "primary",
        ] {
            assert!(
                message.contains(needle),
                "the terminal error must name {needle:?}; got: {message}"
            );
        }
    }

    /// B02-R3(a). Build the same refused chain, but return the FIRST turn's
    /// error — the primary was attempted and failed, so `last_error` is
    /// `Some(e)` and `describe_exhausted_chain` is never consulted. This is the
    /// COMMON case: the circuit-open path only exists after a turn has already
    /// tripped the breaker.
    async fn first_turn_exhausted_chain_error() -> ProviderError {
        let unknown_window = candidate("flux-standard", true, None);
        let mut denied = candidate("blocked-model", true, Some(400_000));
        denied.provider = "denied-provider".into();

        let resilient = ResilientProvider::new_with_policy(
            "primary",
            Arc::new(AlwaysFail),
            vec![
                (unknown_window, Arc::new(AlwaysOk) as Arc<dyn LlmProvider>),
                (denied, Arc::new(AlwaysOk) as Arc<dyn LlmProvider>),
            ],
            CircuitConfig {
                fail_threshold: 1,
                window: Duration::from_secs(30),
                cooldown: Duration::from_secs(60),
            },
            Arc::new(ReceiptReporter::default()),
            FailoverRoutingPolicy {
                denied_providers: std::collections::BTreeSet::from(["denied-provider".to_string()]),
                ..Default::default()
            },
        );
        let mut request = dummy_request();
        request.client_context_tokens = Some(8_000);
        resilient
            .stream(&request)
            .await
            .expect_err("the primary fails and every candidate is refused")
    }

    #[tokio::test]
    async fn the_first_failing_turn_also_names_the_refused_candidates() {
        let error = first_turn_exhausted_chain_error().await;
        let message = error.to_string();

        // The primary's own failure must still be in the sentence …
        assert!(
            message.contains("always-fail"),
            "the primary's own error must survive: {message}"
        );
        // … and so must the chain's, which the operator otherwise never sees on
        // the turn that actually broke.
        for needle in [
            "flux-standard",
            "context_window_unknown",
            "denied-provider",
            "blocked-model",
            "provider_denied",
        ] {
            assert!(
                message.contains(needle),
                "the first failing turn must name {needle:?}; got: {message}"
            );
        }

        // CLASSIFICATION IS LOAD-BEARING and must not move. `Connection` is
        // retryable and was genuinely attempted; converting it into
        // `NotAttempted` to carry the text would silently change both the retry
        // decision and the billing disposition.
        assert!(
            matches!(error, ProviderError::Connection(_)),
            "the typed variant must survive annotation: {error:?}"
        );
        assert!(error.is_retryable(), "a Connection failure stays retryable");
        assert!(
            !error.was_not_attempted(),
            "the primary WAS attempted; the request may have incurred usage"
        );
    }

    /// LEAK GUARD — NOT a fix-proof. B02-R3(c): this test PASSES with the D3
    /// change reverted, because the fixed string it replaced also contained no
    /// endpoint and no policy value. It is worth keeping (it fails the day
    /// someone interpolates a base_url or a denied-provider list into a
    /// user-facing message) but it must never be counted as evidence that the
    /// chain description exists. The two tests that carry that burden are
    /// `an_exhausted_chain_names_each_candidate_and_why_it_was_refused` and
    /// `the_first_failing_turn_also_names_the_refused_candidates`.
    #[tokio::test]
    async fn the_exhausted_chain_error_discloses_no_more_than_the_receipt_does() {
        // The receipt already ships provider + model + rejection to the host,
        // so restating them costs nothing. Endpoints, keys and policy values
        // are NOT on the receipt and must never be interpolated here.
        let message = exhausted_chain_error().await.to_string();
        assert!(
            !message.contains("http"),
            "an endpoint leaked into a user-facing error: {message}"
        );
        assert!(
            !message.contains("denied_providers"),
            "policy configuration leaked into a user-facing error: {message}"
        );
    }

    #[tokio::test]
    async fn a_long_refused_chain_cannot_grow_an_unbounded_error_string() {
        // The error reaches logs and the session journal, so a 40-entry chain
        // must not become a multi-kilobyte line.
        let fallbacks: Vec<_> = (0..40)
            .map(|i| {
                (
                    candidate(&format!("model-{i}"), true, None),
                    Arc::new(AlwaysOk) as Arc<dyn LlmProvider>,
                )
            })
            .collect();
        let resilient = ResilientProvider::new_with_policy(
            "primary",
            Arc::new(AlwaysFail),
            fallbacks,
            CircuitConfig {
                fail_threshold: 1,
                window: Duration::from_secs(30),
                cooldown: Duration::from_secs(60),
            },
            Arc::new(ReceiptReporter::default()),
            FailoverRoutingPolicy::default(),
        );
        let mut request = dummy_request();
        request.client_context_tokens = Some(8_000);
        let _ = resilient.stream(&request).await;
        let message = resilient
            .stream(&request)
            .await
            .expect_err("all 40 candidates have an unknown window")
            .to_string();

        assert!(
            message.contains("model-0"),
            "the first refused candidate must still be named: {message}"
        );
        assert!(
            message.contains("35 more"),
            "the elided tail must be counted, not dropped: {message}"
        );
        assert!(
            message.len() < 600,
            "error length {} is unbounded by the chain length: {message}",
            message.len()
        );
    }
}
