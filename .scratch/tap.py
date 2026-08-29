p = 'crates/wcore-agent/src/spend_guard.rs'
s = open(p).read()
def sub1(old, new, label):
    global s
    assert s.count(old) == 1, f"{label}: expected 1, found {s.count(old)}"
    s = s.replace(old, new, 1)

sub1("""    async fn stream(
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
        self.inner.stream(request).await
    }""",
"""    async fn stream(
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
    }""", "stream")

sub1("""    /// The guard this decorator enforces. Lets a caller that already holds the
    /// wrapped provider reach the audit without a second handle.
    #[must_use]
    pub fn guard(&self) -> &Arc<SpendGuard> {
        &self.guard
    }
}""",
"""    /// The guard this decorator enforces. Lets a caller that already holds the
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
                usage.cache_read_input_tokens,
                usage.cache_creation_input_tokens,
                1.0,
            )
            .filter(|status| status.priced)
            .map(|status| {
                status.microcents as f64 / wcore_types::crucible::MICROCENTS_PER_USD
            }),
    };
    auditor.charge(SpendAuditDispatch {
        provider: profile.provider.clone(),
        model: profile.model.clone(),
        purpose: "provider_dispatch".to_owned(),
        tokens_in,
        tokens_out,
        cost_usd,
    });
}""", "meter")

open(p,'w').write(s)
print('ok')
