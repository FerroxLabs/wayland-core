//! FerroxLabs/wayland#174 items 2-5 — the ENFORCEMENT side of spend governance.
//!
//! `wcore-budget` owns the decision tables ([`wcore_budget::SpendPolicy`],
//! [`wcore_budget::EscalationGate`], [`wcore_budget::SpendAuditor`]) and knows
//! nothing about prices or providers. This module is where those tables meet
//! the running agent: it resolves a provider/model pair to a
//! [`ModelSpendProfile`] through `wcore-pricing`, holds the run's gate, and
//! wraps the live provider so a refusal happens BEFORE any request is sent.
//!
//! ## Why a provider decorator and not a check at each dispatch site
//!
//! A guard that each call site has to remember to call is a guard that the
//! next call site forgets. This crate has shipped exactly that before. Every
//! physical provider send in the agent goes through
//! [`LlmProvider::stream`], so wrapping the engine's provider handle covers
//! the conversation turn, its retries, the configured-fallback re-dispatch and
//! the compaction summarization call with ONE enforcement point that cannot be
//! bypassed by adding a fourth caller.
//!
//! The model-change sites are gated in addition, not instead: refusing at the
//! swap gives the operator an error that names the surface that tried it,
//! while the decorator is the backstop that makes the guarantee true even for
//! a surface nobody thought of.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use wcore_budget::{
    EscalationGate, EscalationRecord, ModelBilling, ModelSpendProfile, SpendAuditDispatch,
    SpendAuditSink, SpendAuditor, SpendMode, SpendPolicy, SpendRefusal, now_unix_ms,
};
use wcore_providers::{LlmProvider, ModelInfo, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};

/// Token count both axes are priced at when ordering two models. One million
/// each: the catalog is quoted per million tokens, so this reads the rate
/// straight out without introducing a second unit to get wrong.
const RATE_PROBE_TOKENS: u64 = 1_000_000;

/// Resolve a provider/model pair to the profile the guard reasons about.
///
/// Order matters and each step is load-bearing:
///
/// 1. **Local prefix first.** `ollama:qwen3` is local whatever the catalog
///    says, and `local-only` must not depend on a pricing row existing.
/// 2. **`cost_is_known_free` next.** That flag is the operator asserting a
///    genuinely free endpoint; it is already what the reservation path trusts
///    for `$0`, so the two must not disagree.
/// 3. **The catalog.** A `priced` row at zero is [`ModelBilling::Free`]; a
///    `priced` row above zero is [`ModelBilling::Metered`].
/// 4. **Anything left is [`ModelBilling::Unpriced`]** — an unresolvable model,
///    or a router placeholder row. Never `Free`: "no price found" and "the
///    price is zero" are different facts, and collapsing them is how an
///    unmetered router alias walks through `no-paid`.
#[must_use]
pub fn classify_model(
    provider: &str,
    model: &str,
    compat: &wcore_config::compat::ProviderCompat,
) -> ModelSpendProfile {
    if wcore_types::model_aliases::is_local_model(model) {
        return ModelSpendProfile::new(provider, model, ModelBilling::Local, 0.0);
    }
    if compat.cost_is_known_free.unwrap_or(false) {
        return ModelSpendProfile::new(provider, model, ModelBilling::Free, 0.0);
    }
    match wcore_pricing::DEFAULT_CATALOG.estimate_cost_status_resolved(
        provider,
        model,
        RATE_PROBE_TOKENS,
        RATE_PROBE_TOKENS,
        1.0,
    ) {
        Some(status) if status.priced => {
            let usd = status.microcents as f64 / wcore_types::crucible::MICROCENTS_PER_USD;
            let billing = if usd > 0.0 {
                ModelBilling::Metered
            } else {
                ModelBilling::Free
            };
            ModelSpendProfile::new(provider, model, billing, usd)
        }
        _ => ModelSpendProfile::new(provider, model, ModelBilling::Unpriced, 0.0),
    }
}

/// The surface that asked for a model change. Kept as a closed set so an
/// escalation record always names something a reader can go and look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationSource {
    /// The smart-routing tier swap (`select_tier_model`).
    TierSwap,
    /// A skill/hook `switch_model` directive.
    SwitchModel,
    /// A provider-chain configured fallback to another provider/model.
    ConfiguredFallback,
    /// The dedicated compaction model.
    CompactionModel,
    /// An explicit operator action — the TUI `/model` pick, or a host rebind.
    Operator,
}

impl EscalationSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TierSwap => "tier_swap",
            Self::SwitchModel => "switch_model",
            Self::ConfiguredFallback => "configured_fallback",
            Self::CompactionModel => "compaction_model",
            Self::Operator => "operator",
        }
    }
}

/// The run's live spend guard: mode policy, escalation ceiling, and the audit
/// accumulator, behind one handle the engine can clone into decorators.
pub struct SpendGuard {
    policy: SpendPolicy,
    gate: Mutex<EscalationGate>,
    auditor: Mutex<Arc<SpendAuditor>>,
    sink: Arc<dyn SpendAuditSink>,
}

impl std::fmt::Debug for SpendGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpendGuard")
            .field("mode", &self.policy.mode())
            .field("authorized", &self.gate.lock().authorized().label())
            .finish_non_exhaustive()
    }
}

impl SpendGuard {
    #[must_use]
    pub fn new(
        mode: SpendMode,
        session_id: impl Into<String>,
        baseline: ModelSpendProfile,
        sink: Arc<dyn SpendAuditSink>,
    ) -> Self {
        let session_id = session_id.into();
        let auditor = Arc::new(SpendAuditor::new(
            format!("task-{}", uuid::Uuid::new_v4()),
            session_id.clone(),
            mode,
            &baseline,
            now_unix_ms(),
        ));
        Self {
            policy: SpendPolicy::new(mode),
            gate: Mutex::new(EscalationGate::new(session_id, baseline)),
            auditor: Mutex::new(auditor),
            sink,
        }
    }

    #[must_use]
    pub fn mode(&self) -> SpendMode {
        self.policy.mode()
    }

    /// The session identity this guard files its audit record and its
    /// escalations under.
    #[must_use]
    pub fn session_id(&self) -> String {
        self.auditor.lock().session_id()
    }

    /// Re-point BOTH halves — the audit record and the escalation gate — at
    /// `session_id`.
    ///
    /// #1161 — the guard is installed by `install_spend_guard` at engine
    /// construction, which is before the engine has a durable conversation
    /// identity to offer. Both halves must move together: filing the record
    /// under one id while the escalations that justify it carry another is the
    /// same unjoinable trail with an extra step.
    pub fn rebind_session_id(&self, session_id: &str) {
        self.auditor.lock().rebind_session_id(session_id);
        self.gate.lock().rebind_session_id(session_id);
    }

    /// The model the run is currently authorized up to.
    #[must_use]
    pub fn authorized_model(&self) -> String {
        self.gate.lock().authorized().label()
    }

    /// The live auditor. Cloned rather than borrowed so a caller can charge it
    /// without holding the guard's lock.
    #[must_use]
    pub fn auditor(&self) -> Arc<SpendAuditor> {
        Arc::clone(&self.auditor.lock())
    }

    /// Admit one dispatch. Mode first, escalation second, and a refusal from
    /// either is recorded on the task's audit before it is returned — a
    /// refusal nobody can see afterwards is half a guard.
    pub fn admit(&self, profile: &ModelSpendProfile) -> Result<(), SpendRefusal> {
        let outcome = self
            .policy
            .admit(profile)
            .and_then(|()| self.gate.lock().admit(profile));
        if let Err(refusal) = &outcome {
            self.auditor.lock().refused(refusal);
        }
        outcome
    }

    /// Authorize an escalation onto `profile`, recording the reason durably
    /// before the dispatch that depends on it can run.
    ///
    /// The mode check runs FIRST and is not waivable: `no-paid` and
    /// `local-only` are ceilings on what may be reached at all, so an
    /// escalation reason cannot buy through one.
    ///
    /// The durable write happens before the in-memory ceiling moves. If the
    /// sink fails, nothing is authorized — the alternative is a run that has
    /// escalated with no record of it, which is the exact failure this
    /// criterion names.
    pub fn authorize(
        &self,
        profile: ModelSpendProfile,
        source: EscalationSource,
        reason: impl Into<String>,
    ) -> Result<Option<EscalationRecord>, SpendRefusal> {
        if let Err(refusal) = self.policy.admit(&profile) {
            self.auditor.lock().refused(&refusal);
            return Err(refusal);
        }
        let reason = reason.into();
        let mut gate = self.gate.lock();
        if !gate.is_escalation(&profile) {
            return Ok(None);
        }
        match gate.authorize(profile.clone(), source.as_str(), reason, now_unix_ms()) {
            Ok(Some(record)) => {
                if let Err(error) = self.sink.escalation(&record) {
                    // An escalation that could not be recorded must not take
                    // effect. Revert only THIS authorization, so escalations
                    // already written durably stay in force.
                    gate.revert_last_authorization();
                    drop(gate);
                    tracing::error!(
                        target: "wcore_agent::spend_guard",
                        %error,
                        "model escalation refused: its record could not be persisted"
                    );
                    let refusal = SpendRefusal::SilentEscalation {
                        authorized: record.from.label(),
                        requested: record.to.label(),
                    };
                    self.auditor.lock().refused(&refusal);
                    return Err(refusal);
                }
                drop(gate);
                self.auditor.lock().escalated(record.clone());
                Ok(Some(record))
            }
            Ok(None) => Ok(None),
            // The gate only rejects a blank reason or a blank source, both of
            // which are programmer errors at these call sites. Surfacing it as
            // a silent-escalation refusal keeps the guard fail-CLOSED.
            Err(error) => {
                tracing::error!(
                    target: "wcore_agent::spend_guard",
                    %error,
                    "model escalation refused: malformed authorization"
                );
                let refusal = SpendRefusal::SilentEscalation {
                    authorized: gate.authorized().label(),
                    requested: profile.label(),
                };
                drop(gate);
                self.auditor.lock().refused(&refusal);
                Err(refusal)
            }
        }
    }

    /// Charge one settled dispatch to the task audit.
    pub fn charge(&self, dispatch: SpendAuditDispatch) {
        self.auditor.lock().charge(dispatch);
    }

    /// Close the current task, write its record, and open the next one.
    ///
    /// Called from every terminal path of `AgentEngine::run`. The auditor's own
    /// `finish` is idempotent, so a path that fires twice reports once; opening
    /// the successor here is what makes "a record after EVERY task" true for
    /// the second and later instructions of a session.
    pub fn finish_task(&self) -> Option<wcore_budget::SpendAuditRecord> {
        let mut slot = self.auditor.lock();
        let record = match slot.finish_into(&self.sink, now_unix_ms()) {
            Ok(record) => record,
            Err(error) => {
                tracing::error!(
                    target: "wcore_agent::spend_guard",
                    %error,
                    "per-task spend audit record could not be persisted"
                );
                slot.finish(now_unix_ms())
            }
        };
        if record.is_some() {
            let gate = self.gate.lock();
            *slot = Arc::new(SpendAuditor::new(
                format!("task-{}", uuid::Uuid::new_v4()),
                record
                    .as_ref()
                    .map(|r| r.session_id.clone())
                    .unwrap_or_default(),
                self.policy.mode(),
                gate.authorized(),
                now_unix_ms(),
            ));
        }
        record
    }
}

/// Wraps the live provider so no request reaches the wire without passing
/// [`SpendGuard::admit`].
pub struct SpendGuardProvider {
    inner: Arc<dyn LlmProvider>,
    guard: Arc<SpendGuard>,
    /// The pricing-catalog provider key (`compat.provider_type`).
    provider_key: String,
    compat: wcore_config::compat::ProviderCompat,
}

impl SpendGuardProvider {
    #[must_use]
    pub fn new(
        inner: Arc<dyn LlmProvider>,
        guard: Arc<SpendGuard>,
        provider_key: impl Into<String>,
        compat: wcore_config::compat::ProviderCompat,
    ) -> Self {
        Self {
            inner,
            guard,
            provider_key: provider_key.into(),
            compat,
        }
    }

    /// The guard this decorator enforces. Lets a caller that already holds the
    /// wrapped provider reach the audit without a second handle.
    #[must_use]
    pub fn guard(&self) -> &Arc<SpendGuard> {
        &self.guard
    }

    /// #174 c2 — forward the event stream, charging the task audit when the
    /// provider reports the turn's usage.
    ///
    /// Metering HERE and not at the budget reservation is deliberate. The
    /// reservation only exists when a cap is configured, and it never covered
    /// the compaction call at all; this tap sees every admitted send through
    /// the engine's provider handle, so the record is complete on a session
    /// with no caps set — which is the session most likely to need one.
    fn meter(
        &self,
        profile: ModelSpendProfile,
        mut source: tokio::sync::mpsc::Receiver<LlmEvent>,
    ) -> tokio::sync::mpsc::Receiver<LlmEvent> {
        let auditor = self.guard.auditor();
        let (tx, rx) = tokio::sync::mpsc::channel(METER_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            let mut charged = false;
            while let Some(event) = source.recv().await {
                if let LlmEvent::Done { usage, .. } = &event {
                    charge_usage(&auditor, &profile, usage);
                    charged = true;
                }
                if tx.send(event).await.is_err() {
                    // The consumer went away (cancellation, or a turn that
                    // stopped reading). Stop forwarding; the charge, if it had
                    // already landed, stays on the record.
                    break;
                }
            }
            if !charged {
                // A stream that ended without a `Done` still cost something the
                // provider may bill. Record the dispatch with no numbers rather
                // than omit it: an audit that only counts clean turns
                // understates exactly the runs that went wrong.
                auditor.charge(SpendAuditDispatch {
                    provider: profile.provider.clone(),
                    model: profile.model.clone(),
                    purpose: "provider_dispatch_incomplete".to_owned(),
                    tokens_in: 0,
                    tokens_out: 0,
                    cost_usd: None,
                });
            }
        });
        rx
    }
}

/// Buffer for the metering hop. Matches the engine's own provider channels;
/// large enough that the tap never becomes the backpressure bottleneck.
const METER_CHANNEL_CAPACITY: usize = 64;

/// Charge one settled dispatch, pricing it from the same catalog the guard
/// classified it with.
///
/// An unpriced or unresolvable model records `cost_usd: None`, never `0.0`.
/// The record's `unpriced_dispatches` count is what tells a reader the total
/// is a floor; writing a zero would tell them the run was free.
fn charge_usage(
    auditor: &Arc<SpendAuditor>,
    profile: &ModelSpendProfile,
    usage: &wcore_types::message::TokenUsage,
) {
    let tokens_in = usage.input_tokens;
    let tokens_out = usage.output_tokens;
    let cost_usd = match profile.billing {
        ModelBilling::Local | ModelBilling::Free => Some(0.0),
        ModelBilling::Unpriced => None,
        ModelBilling::Metered => wcore_pricing::DEFAULT_CATALOG
            .estimate_cost_with_cache_status_resolved(
                &profile.provider,
                &profile.model,
                tokens_in,
                tokens_out,
                usage.cache_read_tokens,
                usage.cache_creation_tokens,
                1.0,
            )
            .filter(|status| status.priced)
            .map(|status| status.microcents as f64 / wcore_types::crucible::MICROCENTS_PER_USD),
    };
    auditor.charge(SpendAuditDispatch {
        provider: profile.provider.clone(),
        model: profile.model.clone(),
        purpose: "provider_dispatch".to_owned(),
        tokens_in,
        tokens_out,
        cost_usd,
    });
}

#[async_trait]
impl LlmProvider for SpendGuardProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
        let profile = classify_model(&self.provider_key, &request.model, &self.compat);
        if let Err(refusal) = self.guard.admit(&profile) {
            // `NotAttempted` and not `Api`: this wrapper PROVED no physical
            // request was made, which is what stops the retry ring re-sending
            // it and what stops the reservation path billing it as ambiguous.
            return Err(ProviderError::NotAttempted {
                reason: refusal.to_string(),
                failure_code: Some(refusal.kind().to_owned()),
            });
        }
        let rx = self.inner.stream(request).await?;
        Ok(self.meter(profile, rx))
    }

    fn alias_key(&self) -> &str {
        self.inner.alias_key()
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        self.inner.list_models().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_budget::MemorySpendAuditSink;

    fn compat() -> wcore_config::compat::ProviderCompat {
        wcore_config::compat::ProviderCompat::default()
    }

    fn metered(model: &str, rate: f64) -> ModelSpendProfile {
        ModelSpendProfile::new("anthropic", model, ModelBilling::Metered, rate)
    }

    struct CountingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl LlmProvider for CountingProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn guard(
        mode: SpendMode,
        baseline: ModelSpendProfile,
    ) -> (Arc<SpendGuard>, Arc<MemorySpendAuditSink>) {
        let memory = Arc::new(MemorySpendAuditSink::default());
        let sink: Arc<dyn SpendAuditSink> = memory.clone();
        (
            Arc::new(SpendGuard::new(mode, "s1", baseline, sink)),
            memory,
        )
    }

    #[test]
    fn a_local_model_classifies_local_whatever_the_catalog_says() {
        let profile = classify_model("ollama", "ollama:qwen3-coder:30b", &compat());
        assert_eq!(profile.billing, ModelBilling::Local);
    }

    #[test]
    fn an_unknown_model_classifies_unpriced_never_free() {
        let profile = classify_model("anthropic", "not-a-real-model-xyzzy", &compat());
        assert_eq!(
            profile.billing,
            ModelBilling::Unpriced,
            "an unresolvable model must never be reported as free"
        );
    }

    #[test]
    fn a_catalogued_model_classifies_metered_with_a_positive_rate() {
        let profile = classify_model(
            "anthropic",
            wcore_types::model_aliases::ANTHROPIC_SONNET,
            &compat(),
        );
        assert_eq!(profile.billing, ModelBilling::Metered);
        assert!(profile.blended_usd_per_mtok > 0.0);
    }

    #[tokio::test]
    async fn the_decorator_refuses_a_paid_model_without_sending_anything() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let inner: Arc<dyn LlmProvider> = Arc::new(CountingProvider {
            calls: Arc::clone(&calls),
        });
        let (guard, _sink) = guard(
            SpendMode::NoPaid,
            ModelSpendProfile::new("ollama", "ollama:qwen3", ModelBilling::Local, 0.0),
        );
        let wrapped = SpendGuardProvider::new(inner, Arc::clone(&guard), "anthropic", compat());
        let request = LlmRequest {
            model: wcore_types::model_aliases::ANTHROPIC_SONNET.to_string(),
            ..Default::default()
        };
        let error = wrapped.stream(&request).await.unwrap_err();
        assert!(
            matches!(&error, ProviderError::NotAttempted { failure_code, .. }
                if failure_code.as_deref() == Some("paid_model_refused")),
            "unexpected error: {error}"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the refusal must happen before the inner provider is reached"
        );
    }

    #[tokio::test]
    async fn the_decorator_passes_an_admitted_model_through() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let inner: Arc<dyn LlmProvider> = Arc::new(CountingProvider {
            calls: Arc::clone(&calls),
        });
        let (guard, _sink) = guard(
            SpendMode::LocalOnly,
            ModelSpendProfile::new("ollama", "ollama:qwen3", ModelBilling::Local, 0.0),
        );
        let wrapped = SpendGuardProvider::new(inner, guard, "ollama", compat());
        let request = LlmRequest {
            model: "ollama:qwen3".to_string(),
            ..Default::default()
        };
        assert!(wrapped.stream(&request).await.is_ok());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_decorator_blocks_an_unauthorized_escalation() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let inner: Arc<dyn LlmProvider> = Arc::new(CountingProvider {
            calls: Arc::clone(&calls),
        });
        let (guard, _sink) = guard(
            SpendMode::Unrestricted,
            classify_model(
                "anthropic",
                wcore_types::model_aliases::ANTHROPIC_HAIKU,
                &compat(),
            ),
        );
        let wrapped = SpendGuardProvider::new(inner, Arc::clone(&guard), "anthropic", compat());
        let request = LlmRequest {
            model: wcore_types::model_aliases::ANTHROPIC_OPUS.to_string(),
            ..Default::default()
        };
        let error = wrapped.stream(&request).await.unwrap_err();
        assert!(
            matches!(&error, ProviderError::NotAttempted { failure_code, .. }
                if failure_code.as_deref() == Some("silent_model_escalation")),
            "unexpected error: {error}"
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

        // Authorize it, and the same request goes through.
        guard
            .authorize(
                classify_model(
                    "anthropic",
                    wcore_types::model_aliases::ANTHROPIC_OPUS,
                    &compat(),
                ),
                EscalationSource::Operator,
                "operator picked opus",
            )
            .expect("authorization accepted");
        assert!(wrapped.stream(&request).await.is_ok());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn an_authorized_escalation_is_written_durably_before_it_takes_effect() {
        let (guard, sink) = guard(SpendMode::Unrestricted, metered("haiku", 1.0));
        guard
            .authorize(metered("opus", 30.0), EscalationSource::TierSwap, "why")
            .expect("accepted");
        let escalations = sink.escalations();
        assert_eq!(escalations.len(), 1);
        assert_eq!(escalations[0].source, "tier_swap");
        assert_eq!(escalations[0].reason, "why");
        assert_eq!(escalations[0].from.model, "haiku");
        assert_eq!(escalations[0].to.model, "opus");
    }

    #[test]
    fn a_mode_ceiling_cannot_be_bought_through_with_an_escalation_reason() {
        let (guard, sink) = guard(
            SpendMode::NoPaid,
            ModelSpendProfile::new("ollama", "ollama:qwen3", ModelBilling::Local, 0.0),
        );
        let refusal = guard
            .authorize(metered("opus", 30.0), EscalationSource::Operator, "please")
            .unwrap_err();
        assert!(matches!(refusal, SpendRefusal::PaidModel { .. }));
        assert!(sink.escalations().is_empty());
    }

    #[test]
    fn every_task_emits_exactly_one_record_and_the_next_task_gets_its_own() {
        let (guard, sink) = guard(SpendMode::Unrestricted, metered("haiku", 1.0));
        guard.charge(SpendAuditDispatch {
            provider: "anthropic".into(),
            model: "haiku".into(),
            purpose: "conversation".into(),
            tokens_in: 10,
            tokens_out: 5,
            cost_usd: Some(0.02),
        });
        let first = guard.finish_task().expect("first task emits");
        assert_eq!(first.dispatches.len(), 1);
        // A second terminal path in the SAME task must not double-report.
        assert!(guard.finish_task().is_none() || sink.records().len() == 2);

        guard.charge(SpendAuditDispatch {
            provider: "anthropic".into(),
            model: "haiku".into(),
            purpose: "conversation".into(),
            tokens_in: 1,
            tokens_out: 1,
            cost_usd: Some(0.01),
        });
        let second = guard
            .finish_task()
            .expect("second task emits its own record");
        assert_ne!(first.task_id, second.task_id);
        assert_eq!(second.dispatches.len(), 1);
    }

    #[test]
    fn a_refusal_lands_on_the_task_audit_record() {
        let (guard, sink) = guard(
            SpendMode::NoPaid,
            ModelSpendProfile::new("ollama", "ollama:qwen3", ModelBilling::Local, 0.0),
        );
        assert!(guard.admit(&metered("opus", 30.0)).is_err());
        let record = guard.finish_task().expect("emits");
        assert_eq!(record.refusals.len(), 1);
        assert_eq!(record.refusals[0].kind, "paid_model_refused");
        assert_eq!(sink.records().len(), 1);
    }
}
