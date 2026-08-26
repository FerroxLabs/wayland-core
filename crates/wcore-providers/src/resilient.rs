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
use parking_lot::Mutex;
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

/// True when the circuit was opened because the provider was momentarily
/// unavailable — it timed out, said it was overloaded, or told the caller to
/// come back later.
///
/// Deliberately narrow. A cooldown entered because the request was REJECTED
/// (bad credential, billing, unknown model) must keep refusing without
/// sending: re-issuing it cannot succeed and every attempt costs a call. Only
/// the "come back in a moment" reasons are worth an unprotected probe.
///
/// ## Why `RateLimit` belongs here — and how a 429 counts
///
/// A 429 is the provider WORKING. It received the request, understood it,
/// and answered with the one status whose entire meaning is "this will
/// succeed later, here is how long to wait" — usually with a `Retry-After`
/// saying exactly when. That is the definition of momentary.
///
/// It still COUNTS toward the cooldown, and that is deliberate rather than
/// an omission. Two questions are being answered separately:
///
/// * Should other traffic be routed away from this provider? Yes — a
///   throttled key is a real reason to prefer a fallback, and the cooldown
///   is how the chain learns that. `record_failure` is unchanged, so with a
///   fallback configured a rate-limited primary fails over exactly as before.
/// * Should the cooldown refuse the LAST route? No, and this is the half
///   that was wrong. With no fallback, refusing protects nothing: there is
///   nowhere else to send, and the retry loop above already paces its
///   re-sends on the provider's own `Retry-After`. Refusing merely ends the
///   run three sends into a ten-send budget — measured, both 429 shapes
///   stopped at 3 sends on `primary circuit is open and no fallback is
///   configured` — and reports a circuit to a user whose actual problem is
///   a quota.
///
/// The single-probe lease still applies, so this is not a hole in the
/// concurrency guard; see [`CooldownTracker::try_acquire_unprotected_probe`].
fn cooldown_is_momentary(state: CooldownState) -> bool {
    let reason = match state {
        CooldownState::Cooling { reason, .. } | CooldownState::HalfOpen { reason } => reason,
        CooldownState::Ready => return false,
    };
    matches!(
        reason,
        FailoverReason::Timeout | FailoverReason::Overloaded | FailoverReason::RateLimit
    )
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

    /// A chain routed around its primary.
    ///
    /// `primary_failure_code` is the machine-readable class of the fault that
    /// took the primary out (`retry::provider_failure_code`) — the same value
    /// `ProviderError::NotAttempted` carries for #1127. `FailoverReceipt::reason`
    /// cannot stand in for it: `FailoverReason` classifies a refused connection
    /// and a read timeout identically as `timeout`, so a reporter that rendered
    /// the reason would send the reader hunting the wrong fault. `None` when the
    /// primary was never classified (nothing upstream failed).
    fn report_failover(&self, _receipt: &FailoverReceipt, _primary_failure_code: Option<&str>) {}
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
    /// #1127 — the failure class of the primary's most recent failed dispatch.
    ///
    /// The circuit that later SKIPS the primary remembers only a
    /// `FailoverReason`, which is far coarser than the failure code the engine
    /// prices retries with: a refused connection and a read timeout are both
    /// `Timeout` there, and only one of them is capped at
    /// `REFUSED_ENDPOINT_MAX_RETRIES`. Kept here so a refusal that opened the
    /// circuit on an earlier call is still nameable on the call that finds it
    /// open. Cleared on a successful primary dispatch — the endpoint answered,
    /// so the old class is no longer evidence of anything.
    primary_failure_code: Mutex<Option<String>>,
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
                let context_window =
                    wcore_config::limits::model_output_ceiling(&pricing_provider, &model)
                        .map(|(_, window)| u64::from(window));
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
            primary_failure_code: Mutex::new(None),
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

    /// Report a failover receipt together with the primary's fault CLASS.
    ///
    /// #1133. The class is already in hand — `stream` stores it in
    /// `primary_failure_code` the moment the primary fails — and it is what a
    /// reader needs to act: `connection_refused` and `timeout` both arrive as
    /// `FailoverReason::Timeout` on the receipt, so the receipt alone cannot
    /// tell a closed port from a slow one. Reading the lock here (rather than
    /// at each of the four call sites) keeps the guard off the call stack of
    /// the reporter, which is host code and must never run under our mutex.
    fn report_failover(&self, receipt: &FailoverReceipt) {
        let primary_failure_code = self.primary_failure_code.lock().clone();
        self.reporter
            .report_failover(receipt, primary_failure_code.as_deref());
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

/// Why one candidate was refused BEFORE any call was made to it, in the terms
/// a person can act on.
///
/// [`CandidateRejection`] is a machine token; this is its other half. Kept
/// beside the sentence it feeds so a new rejection variant cannot be added
/// without a note for it.
fn rejection_note(rejection: CandidateRejection) -> &'static str {
    match rejection {
        CandidateRejection::ProviderNotAllowed => "the routing policy does not allow this provider",
        CandidateRejection::ProviderDenied => "the routing policy denies this provider",
        CandidateRejection::RegionNotAllowed => "the routing policy does not allow this region",
        CandidateRejection::OrganizationMismatch => {
            "it belongs to a different organization than the routing policy requires"
        }
        CandidateRejection::ToolsUnsupported => "it does not support the tools this request needs",
        CandidateRejection::VisionUnsupported => {
            "it does not support the images this request carries"
        }
        CandidateRejection::StructuredOutputUnsupported => {
            "it does not support the structured output this request needs"
        }
        CandidateRejection::ContextWindowUnknown => "its context window is unknown",
        CandidateRejection::ContextWindowTooSmall => {
            "its context window is too small for this request"
        }
        CandidateRejection::PricingStale => {
            "its pricing evidence is stale and the routing policy requires fresh pricing"
        }
        CandidateRejection::PricingUnavailable => {
            "it is unpriced and the routing policy requires priced candidates"
        }
        CandidateRejection::CooldownActive => "it is in cooldown after an earlier failure",
        CandidateRejection::BudgetDenied => "the spend budget refused it",
    }
}

/// The sentence the chain ends on when it walked the whole roster with NOTHING
/// dispatched: the primary skipped for an open circuit, and every configured
/// candidate refused before any call.
///
/// It replaces a blanket "no configured fallback candidate passed routing
/// policy", which was true of exactly one of the thirteen dispositions that
/// reach here. MEASURED (#1127) on 0.13.6: a run whose fallback endpoint had
/// already been DISPATCHED three times — counted server-side on the mock —
/// died on that sentence, and the disposition on the receipt was
/// `CooldownActive`. Routing policy was never consulted, so the user it sends
/// to `[provider_policy]` finds an empty section, and the refused `base_url`
/// that actually opened every circuit is named nowhere.
///
/// Built from the receipt rather than from this function's control flow, so it
/// can only say what the chain recorded.
///
/// `primary_failure_code` is preferred over the receipt's `FailoverReason`
/// wherever the chain proved one: the reason cannot tell a refused connection
/// from a read timeout — both classify as `Timeout` — and only one of the two
/// points the user at a `base_url`.
fn nothing_attempted_reason(
    receipt: &FailoverReceipt,
    primary_failure_code: Option<&str>,
) -> String {
    let candidates: Vec<String> = receipt
        .candidates
        .iter()
        .map(|candidate| {
            let note = match candidate.disposition {
                Ok(()) => "it was dispatched and failed",
                Err(rejection) => rejection_note(rejection),
            };
            // An unclassified cooldown adds nothing but a word that reads
            // like an answer; a mid-stream failure records exactly that.
            match candidate
                .cooldown_reason
                .filter(|reason| *reason != FailoverReason::Unknown)
            {
                Some(reason) => format!(
                    "{}/{}: {note} ({reason})",
                    candidate.provider, candidate.model
                ),
                None => format!("{}/{}: {note}", candidate.provider, candidate.model),
            }
        })
        .collect();
    let tail = if candidates.is_empty() {
        "and no fallback candidate was configured".to_string()
    } else {
        format!(
            "and no configured fallback candidate could be dispatched ({})",
            candidates.join("; ")
        )
    };
    let primary_fault = primary_failure_code
        .map(str::to_string)
        .unwrap_or_else(|| receipt.reason.to_string());
    format!(
        "the primary '{}' was skipped because its circuit is open ({primary_fault}), {tail}",
        receipt.failed_provider
    )
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
        let mut admission = self.health.try_acquire();
        // An open circuit does two separable things: it stops traffic
        // HAMMERING a failing endpoint, and it FAILS FAST so the caller can go
        // somewhere else. With an empty fallback chain the second one has no
        // meaning — there is nowhere else — and failing fast only converts a
        // transient outage into a guaranteed failure that the caller's own
        // bounded budget never gets to test. The first one keeps all of its
        // meaning and is preserved here: the probe is taken under the SAME
        // single-flight lease `try_acquire` uses, so at most one request is in
        // flight against the open circuit and every concurrent caller is still
        // refused. Rate remains bounded by the caller's own backoff.
        //
        // Narrow on purpose. This degrades refusal to probing only when ALL of
        // (a) there is no fallback to route to, (b) the cooldown was entered
        // for a momentary reason, and (c) no other probe holds the lease. A
        // rejected request (auth, billing, unknown model, rate limit) keeps
        // hard refusal, because re-issuing it cannot succeed.
        let mut unprotected_probe = false;
        if admission.is_none() && self.fallbacks.is_empty() && cooldown_is_momentary(primary_state)
        {
            admission = self.health.try_acquire_unprotected_probe();
            unprotected_probe = admission.is_some();
        }
        if unprotected_probe {
            self.reporter.report(
                &self.primary_name,
                None,
                CircuitState::Open,
                Some(
                    "circuit open with no fallback configured; sending a single guarded probe \
                     to the primary",
                ),
            );
        }
        if let Some(permit) = admission {
            match crate::attempt_lifecycle::scope_provider_attempt_identity(
                self.primary_name.clone(),
                request.model.clone(),
                self.primary.stream(request),
            )
            .await
            {
                Ok(rx) => {
                    *self.primary_failure_code.lock() = None;
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
                    *self.primary_failure_code.lock() =
                        Some(crate::retry::provider_failure_code(&e));
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
            let reason = match primary_state {
                CooldownState::Cooling { reason, .. } | CooldownState::HalfOpen { reason } => {
                    reason
                }
                CooldownState::Ready => FailoverReason::Unknown,
            };
            if self.fallbacks.is_empty() {
                // Name WHY the circuit is open. "primary circuit is open and
                // no fallback is configured" describes this function's
                // control flow, not the user's problem: it is the sentence a
                // rate-limited run used to die on, and a reader has no way to
                // get from it to "you are over quota". The cause is already
                // in hand — it is what opened the circuit — so carry it.
                return Err(ProviderError::NotAttempted {
                    reason: format!(
                        "primary circuit is open ({reason}) and no fallback is configured"
                    ),
                    failure_code: self.primary_failure_code.lock().clone(),
                });
            }
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
                    self.report_failover(&receipt);
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
                    self.report_failover(&receipt);
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
                        self.report_failover(&receipt);
                        return Err(e);
                    }
                    previous_provider = &fallback.label;
                    previous_attempted = e.is_retryable()
                        || crate::retry::configured_fallback_previous_attempted(&e);
                    last_error = Some(e);
                }
            }
        }
        self.report_failover(&receipt);
        let primary_failure_code = self.primary_failure_code.lock().clone();
        Err(last_error.unwrap_or_else(|| ProviderError::NotAttempted {
            reason: nothing_attempted_reason(&receipt, primary_failure_code.as_deref()),
            failure_code: primary_failure_code,
        }))
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

        fn report_failover(&self, receipt: &FailoverReceipt, _: Option<&str>) {
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
            flux_loop_intent: None,
            flux_turn_nonce: None,
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

    /// Wiring grade for the unknown-context-window admission rule.
    ///
    /// This exercises the production `evaluate_candidate` call site in
    /// `stream()`, not the policy function in isolation: a candidate whose
    /// window is not statically known must be physically DISPATCHED, while a
    /// candidate with a known-too-small window must still be skipped.
    ///
    /// `wcore_config::limits::model_output_ceiling` returns `None` for every
    /// model outside its static table, which is what both
    /// `new_with_fallback_identities` and the production
    /// `wcore_agent::bootstrap::build_fallback_providers` feed into
    /// `CandidateCapabilities::context_window`. The Flux tier aliases are the
    /// headline case. Before this rule the chain below dispatched NOTHING and
    /// the turn died with no answer and no evidence.
    #[tokio::test]
    async fn unknown_context_window_candidate_is_dispatched() {
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

        let too_small = Arc::new(CountOk(AtomicUsize::new(0)));
        let unknown = Arc::new(CountOk(AtomicUsize::new(0)));
        let reporter = Arc::new(ReceiptReporter::default());
        let resilient = ResilientProvider::new_with_policy(
            "primary",
            Arc::new(AlwaysFail),
            vec![
                (
                    candidate("too-small", true, Some(10_000)),
                    too_small.clone(),
                ),
                (candidate("flux-auto", true, None), unknown.clone()),
            ],
            CircuitConfig::default(),
            reporter.clone(),
            FailoverRoutingPolicy::default(),
        );
        let mut request = dummy_request();
        request.client_context_tokens = Some(50_000);

        resilient.stream(&request).await.unwrap();

        assert_eq!(
            too_small.0.load(Ordering::SeqCst),
            0,
            "a known-too-small window must still be refused"
        );
        assert_eq!(
            unknown.0.load(Ordering::SeqCst),
            1,
            "an unknown window must be attempted, not refused"
        );

        let receipts = reporter.receipts.lock().clone();
        assert_eq!(receipts.len(), 1);
        assert_eq!(
            receipts[0].candidates[0].disposition,
            Err(CandidateRejection::ContextWindowTooSmall)
        );
        assert_eq!(receipts[0].candidates[1].disposition, Ok(()));
        assert_eq!(receipts[0].selected_model.as_deref(), Some("flux-auto"));
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

    /// REVISED, deliberately. This test used to assert the opposite — "a
    /// rate limit must keep hard fail-fast even with no fallback", on the
    /// grounds that re-issuing burns quota that is already exhausted. Two
    /// things about that were wrong, and both are measured:
    ///
    /// * It does not burn quota in the sense that matters. A 429 is refused
    ///   before the model runs — this crate's own
    ///   `ProviderError::produced_no_billable_output` returns true for it —
    ///   so a re-send costs one request against a limiter, not a completion.
    ///   And the engine's retry loop honours the provider's own
    ///   `Retry-After` before each one, so the re-send happens when the
    ///   provider said to come back.
    /// * It cost the run its budget and told the user the wrong cause.
    ///   Measured against a 429 endpoint on the shipped tree: both a
    ///   `Retry-After: 7` shape and a bare 429 stopped at 3 sends against a
    ///   10-retry budget, ending on `primary circuit is open and no fallback
    ///   is configured`. The breaker threshold bound before the retry budget
    ///   ever could, and a user over quota was told about a circuit.
    ///
    /// This is the same shape as
    /// `open_circuit_without_fallback_still_sends_after_a_momentary_outage`
    /// (job corpus B-2): with nowhere else to route, refusing protects
    /// nothing. It changes only the no-fallback case — `record_failure` is
    /// untouched, so a rate-limited primary WITH a fallback still fails over,
    /// which `open_circuit_fails_over_to_fallback` covers.
    #[tokio::test]
    async fn open_circuit_without_fallback_still_probes_a_rate_limited_provider() {
        struct AlwaysRateLimited;
        #[async_trait]
        impl LlmProvider for AlwaysRateLimited {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                Err(ProviderError::RateLimited {
                    retry_after_ms: 60_000,
                })
            }
        }

        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(AlwaysRateLimited),
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
            Err(ProviderError::RateLimited { .. })
        ));
        // The circuit is open now (fail_threshold = 1). The second call must
        // still REACH the provider: the primary's own 429, not a refusal to
        // try. Anything else and the retry budget above can never be spent on
        // the one failure class that is defined by "try again later".
        assert!(
            matches!(
                resilient.stream(&dummy_request()).await,
                Err(ProviderError::RateLimited { .. })
            ),
            "an open circuit with nowhere else to route must still probe a \
             rate-limited provider"
        );
    }

    /// The half of `cooldown_is_momentary` that must NOT move. Adding
    /// `RateLimit` is only safe if the rejected classes still refuse without
    /// sending — re-issuing a bad credential cannot succeed and every attempt
    /// costs a call.
    #[test]
    fn only_come_back_later_reasons_earn_an_unprotected_probe() {
        for momentary in [
            FailoverReason::Timeout,
            FailoverReason::Overloaded,
            FailoverReason::RateLimit,
        ] {
            assert!(
                cooldown_is_momentary(CooldownState::Cooling {
                    retry_at: None,
                    reason: momentary
                }),
                "{momentary} is a come-back-later reason and must earn a probe"
            );
            assert!(
                cooldown_is_momentary(CooldownState::HalfOpen { reason: momentary }),
                "{momentary} must earn a probe from HalfOpen too"
            );
        }
        // Known-positive controls: the REJECTED classes still refuse. If this
        // half ever goes green-by-default the test above proves nothing.
        for rejected in [
            FailoverReason::Auth,
            FailoverReason::AuthPermanent,
            FailoverReason::Billing,
            FailoverReason::ModelNotFound,
            FailoverReason::Format,
            FailoverReason::ContextOverflow,
            FailoverReason::SessionExpired,
            FailoverReason::Unknown,
        ] {
            assert!(
                !cooldown_is_momentary(CooldownState::Cooling {
                    retry_at: None,
                    reason: rejected
                }),
                "{rejected} is a rejection — re-issuing it cannot succeed, so \
                 the open circuit must keep refusing"
            );
        }
        // A circuit that is not open is not momentarily anything.
        assert!(!cooldown_is_momentary(CooldownState::Ready));
    }

    /// The refusal must name WHY the circuit is open. "primary circuit is
    /// open and no fallback is configured" describes this module's control
    /// flow; it is the sentence a rate-limited run used to die on, and a
    /// reader cannot get from it to "you are over quota".
    ///
    /// Reached with a REJECTED reason (403 -> `AuthPermanent`), because that
    /// is the class that still refuses after the change above.
    #[tokio::test]
    async fn the_circuit_refusal_names_the_reason_it_opened() {
        struct AlwaysForbidden;
        #[async_trait]
        impl LlmProvider for AlwaysForbidden {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                Err(ProviderError::Api {
                    status: 403,
                    message: "forbidden".into(),
                })
            }
        }
        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(AlwaysForbidden),
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
            Err(ProviderError::Api { status: 403, .. })
        ));
        let refusal = match resilient.stream(&dummy_request()).await {
            Err(ProviderError::NotAttempted { reason, .. }) => reason,
            other => panic!("expected a NotAttempted refusal, got {other:?}"),
        };
        assert!(
            refusal.contains(FailoverReason::AuthPermanent.as_str()),
            "the refusal must name the reason the circuit opened; got: {refusal}"
        );
        // Known-positive: the rest of the sentence is still there, so the
        // assertion above is testing an ADDITION and not a rewrite that lost
        // the original information.
        assert!(
            refusal.contains("no fallback is configured"),
            "got: {refusal}"
        );
    }

    /// Finding 3, round 2. The probe an open circuit allows when there is no
    /// fallback is SINGLE-FLIGHT: a second caller arriving while it is in the
    /// air is refused. This is the anti-hammering half of E-H2, and it is the
    /// half that has to survive dropping the fail-fast half.
    #[tokio::test]
    async fn an_open_circuit_probe_is_single_flight_across_concurrent_callers() {
        struct GatedFail {
            calls: Arc<std::sync::atomic::AtomicUsize>,
            entered: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Notify>,
        }
        #[async_trait]
        impl LlmProvider for GatedFail {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n > 0 {
                    // Park inside the provider so the probe is provably still
                    // in flight while the second caller asks for admission.
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                Err(ProviderError::Connection("primary is down".into()))
            }
        }

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let resilient = Arc::new(ResilientProvider::new(
            "primary",
            Arc::new(GatedFail {
                calls: Arc::clone(&calls),
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
            vec![],
            CircuitConfig {
                fail_threshold: 1,
                window: Duration::from_secs(30),
                cooldown: Duration::from_secs(60),
            },
            Arc::new(NoOpCircuitReporter),
        ));

        // One failure opens the circuit for a momentary reason.
        assert!(matches!(
            resilient.stream(&dummy_request()).await,
            Err(ProviderError::Connection(_))
        ));

        let probing = {
            let resilient = Arc::clone(&resilient);
            tokio::spawn(async move { resilient.stream(&dummy_request()).await })
        };
        entered.notified().await;

        // Bounded: without the lease the second caller ALSO reaches the parked
        // provider and never returns, and an unbounded await would turn that
        // into a harness timeout instead of a legible failure.
        let concurrent =
            tokio::time::timeout(Duration::from_secs(5), resilient.stream(&dummy_request()))
                .await
                .expect(
                    "a second caller must be refused immediately, not queued behind the \
             probe inside the provider",
                );
        assert!(
            matches!(concurrent, Err(ProviderError::NotAttempted { .. })),
            "a second caller must be refused while the single probe is in \
             flight, got {concurrent:?}"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "the concurrent caller must not have reached the provider"
        );

        release.notify_one();
        assert!(matches!(
            probing.await.expect("probe task joins"),
            Err(ProviderError::Connection(_))
        ));
    }

    /// A rejected request still refuses without sending once the circuit is
    /// open: retrying a 403 cannot succeed and every attempt costs a call.
    /// This is the guarantee the open circuit exists to provide when there is
    /// no fallback, and it is unchanged.
    #[tokio::test]
    async fn open_circuit_without_fallback_refuses_a_rejected_request() {
        struct AlwaysForbidden;
        #[async_trait]
        impl LlmProvider for AlwaysForbidden {
            async fn stream(
                &self,
                _: &LlmRequest,
            ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
                Err(ProviderError::Api {
                    status: 403,
                    message: "forbidden".into(),
                })
            }
        }

        let resilient = ResilientProvider::new(
            "primary",
            Arc::new(AlwaysForbidden),
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
            Err(ProviderError::Api { status: 403, .. })
        ));
        assert!(matches!(
            resilient.stream(&dummy_request()).await,
            Err(ProviderError::NotAttempted { .. })
        ));
    }

    /// Job corpus row B-2: three connection resets tripped the breaker, and
    /// every later attempt came back "not attempted" while the provider had
    /// already healed — a momentary outage became a lost job. With no fallback
    /// there is nowhere to route, so refusing protects nothing: send, and let
    /// the caller's own bounded budget decide when to stop.
    #[tokio::test]
    async fn open_circuit_without_fallback_still_sends_after_a_momentary_outage() {
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
        // The circuit is open now. The second call must still reach the
        // provider — the primary's own error, not a refusal to try.
        assert!(matches!(
            resilient.stream(&dummy_request()).await,
            Err(ProviderError::Connection(_))
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
    /// #1127 — a provider whose connect phase is REFUSED, in the wire shape
    /// `reqwest` produces (`with_transport_cause` keeps only the innermost
    /// link, which is what `classify_connect_chain` reads).
    struct RefusedProvider;
    #[async_trait]
    impl LlmProvider for RefusedProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            Err(ProviderError::Connection(
                "error sending request: Connection refused (os error 111) (endpoint 127.0.0.1:9)"
                    .into(),
            ))
        }
    }

    /// One failure opens a circuit, and it stays open long enough that the
    /// SECOND call in each test below finds nothing it may dispatch.
    fn collapse_config() -> CircuitConfig {
        CircuitConfig {
            fail_threshold: 1,
            window: Duration::from_secs(60),
            cooldown: Duration::from_secs(60),
        }
    }

    fn chain_of(
        primary: Arc<dyn LlmProvider>,
        fallback: Arc<dyn LlmProvider>,
    ) -> ResilientProvider {
        ResilientProvider::new(
            "primary",
            primary,
            vec![("fallback".to_string(), fallback)],
            collapse_config(),
            Arc::new(NoOpCircuitReporter),
        )
    }

    /// Drive the chain until it collapses with nothing left to dispatch, and
    /// hand back the refusal it ends on.
    async fn collapse(resilient: &ResilientProvider) -> ProviderError {
        resilient
            .stream(&dummy_request())
            .await
            .expect_err("the first call must fail and open both circuits");
        resilient
            .stream(&dummy_request())
            .await
            .expect_err("the second call must be refused with nothing dispatched")
    }

    /// #1127 — the sentence a collapsed chain ends on must describe what
    /// actually happened to each candidate.
    ///
    /// Measured on 0.13.6: a run whose fallback endpoint had already been
    /// DISPATCHED three times (counted server-side) died on "no configured
    /// fallback candidate passed routing policy". The candidate was refused
    /// for an ACTIVE COOLDOWN and the primary for an OPEN CIRCUIT; routing
    /// policy was never consulted, and a user sent to read `[provider_policy]`
    /// finds it empty.
    #[tokio::test]
    async fn a_collapsed_chain_names_the_real_disposition_not_routing_policy() {
        let resilient = chain_of(Arc::new(RefusedProvider), Arc::new(RefusedProvider));
        let reason = match collapse(&resilient).await {
            ProviderError::NotAttempted { reason, .. } => reason,
            other => panic!("expected a NotAttempted refusal, got {other:?}"),
        };
        assert!(
            reason.contains("cooldown"),
            "the refusal must name the cooldown that actually refused the \
             candidate; got: {reason}"
        );
        assert!(
            !reason.contains("routing policy"),
            "no routing policy was consulted on this path; got: {reason}"
        );
        assert!(
            reason.contains("primary"),
            "the refusal must say the primary was skipped too; got: {reason}"
        );
    }

    /// Known-positive control for the test above: the SAME collapse path, with
    /// a candidate that really was refused by the routing policy, must still
    /// say routing policy. Without this, a fix that hardcoded "cooldown" would
    /// pass.
    #[tokio::test]
    async fn a_policy_refused_candidate_still_names_the_routing_policy() {
        let metadata = FailoverCandidateMetadata {
            label: "fallback".into(),
            provider: "openai".into(),
            model: "gpt-test".into(),
            organization: None,
            region: None,
            capabilities: CandidateCapabilities {
                tools: true,
                vision: true,
                structured_output: true,
                context_window: Some(1_000_000),
            },
            // Unpriced, against a policy that requires priced candidates.
            pricing: PricingEvidence::default(),
        };
        let resilient = ResilientProvider::new_with_policy(
            "primary",
            Arc::new(RefusedProvider),
            vec![(metadata, Arc::new(RefusedProvider) as Arc<dyn LlmProvider>)],
            collapse_config(),
            Arc::new(NoOpCircuitReporter),
            FailoverRoutingPolicy {
                require_priced: true,
                ..FailoverRoutingPolicy::default()
            },
        );
        let reason = match collapse(&resilient).await {
            ProviderError::NotAttempted { reason, .. } => reason,
            other => panic!("expected a NotAttempted refusal, got {other:?}"),
        };
        assert!(
            reason.contains("routing policy"),
            "a candidate the policy really did refuse must say so; got: {reason}"
        );
    }
    /// #1127 — a provider that answers every request with an HTTP 503.
    struct UnavailableProvider;
    #[async_trait]
    impl LlmProvider for UnavailableProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            Err(ProviderError::Api {
                status: 503,
                message: "service unavailable".into(),
            })
        }
    }

    /// #1127 — the failure CLASS must survive a collapsed chain.
    ///
    /// `provider_failure_code` is what the engine prices retries with
    /// (`class_retry_budget`) and what the host is told
    /// (`emit_provider_failure`). A chain that reports its own control flow
    /// (`provider_not_attempted`) in place of the fault erases the refused
    /// endpoint's short budget and names nothing the user can act on.
    #[tokio::test]
    async fn a_collapsed_chain_carries_the_class_that_ended_it() {
        let resilient = chain_of(Arc::new(RefusedProvider), Arc::new(RefusedProvider));
        let error = collapse(&resilient).await;
        assert_eq!(
            crate::retry::provider_failure_code(&error),
            crate::retry::FAILURE_CONNECTION_REFUSED,
            "the refused primary is the fault that opened every circuit"
        );
        let ProviderError::NotAttempted { reason, .. } = &error else {
            panic!("expected a NotAttempted refusal, got {error:?}");
        };
        assert!(
            reason.contains(crate::retry::FAILURE_CONNECTION_REFUSED),
            "the sentence must name the same fault the class does; got: {reason}"
        );
    }

    /// Known-positive control for the test above: a chain that collapsed for a
    /// DIFFERENT reason must carry that reason, not a hardcoded refusal.
    #[tokio::test]
    async fn a_collapsed_chain_does_not_invent_a_refusal() {
        let resilient = chain_of(Arc::new(UnavailableProvider), Arc::new(UnavailableProvider));
        let error = collapse(&resilient).await;
        assert_eq!(
            crate::retry::provider_failure_code(&error),
            "http_503",
            "the class carried must be the one that actually ended the chain"
        );
    }
}
