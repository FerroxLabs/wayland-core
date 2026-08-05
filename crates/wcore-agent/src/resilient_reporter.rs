//! W7 F8 adapter: bridges `wcore_providers::CircuitReporter` to a
//! parent `OutputSink::emit_provider_circuit_event`. Lives in
//! `wcore-agent` so the dep direction stays correct
//! (wcore-providers → wcore-types only; wcore-agent depends on both).
//!
//! Bootstrap constructs a `ProtocolCircuitReporter` when a
//! `ResilientProvider` is configured and hands it to the provider
//! constructor; the reporter relays every state transition through
//! the parent's `OutputSink` for `ProtocolEvent::ProviderCircuitEvent`
//! emission.

use std::sync::Arc;

use wcore_providers::{CircuitReporter, CircuitState, FailoverReceipt};

use crate::output::OutputSink;

pub struct ProtocolCircuitReporter {
    output: Arc<dyn OutputSink>,
}

impl ProtocolCircuitReporter {
    pub fn new(output: Arc<dyn OutputSink>) -> Self {
        Self { output }
    }
}

impl CircuitReporter for ProtocolCircuitReporter {
    fn report(
        &self,
        primary: &str,
        fallback: Option<&str>,
        state: CircuitState,
        error: Option<&str>,
    ) {
        self.output
            .emit_provider_circuit_event(primary, fallback, state.as_str(), error);

        // F05-TRUTH-3 (`CONT-*` cache economics) runtime outcome proof.
        //
        // `Open` is the only transition that is an *outcome*. It is reached when
        // the `CooldownTracker`'s accumulated failures cross the configured
        // threshold, and reaching it changes where the next request is routed —
        // the provider is skipped. `Closed` is the resting state every session
        // starts in, and `HalfOpen` is a trial, not a change of destination;
        // emitting on either would make the proof unfalsifiable, which is the
        // defect the F05 stage vocabulary exists to prevent.
        //
        // Deliberately NOT emitted from construction: a `CooldownTracker` is
        // built on every session bootstrap, so an occurrence tied to
        // construction would fire unconditionally and prove nothing.
        if matches!(state, CircuitState::Open) {
            for activation in crate::capability_activation::successful_occurrence(
                wcore_protocol::events::CapabilityId::CooldownTracker,
            ) {
                self.output.emit_capability_activation(&activation);
            }
        }
    }

    fn report_failover(&self, receipt: &FailoverReceipt) {
        match serde_json::to_value(receipt) {
            Ok(receipt) => self.output.emit_provider_failover_receipt(receipt),
            Err(error) => self.output.emit_info(&format!(
                "provider failover receipt serialization failed: {error}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wcore_types::message::FinishReason;

    /// (primary, fallback?, state, error?) captured per report.
    type ReportedEvent = (String, Option<String>, String, Option<String>);

    #[derive(Default)]
    struct Rec {
        events: Mutex<Vec<ReportedEvent>>,
        receipts: Mutex<Vec<serde_json::Value>>,
        activations: Mutex<Vec<(String, String)>>,
    }
    impl OutputSink for Rec {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
        fn emit_provider_circuit_event(
            &self,
            primary: &str,
            fallback: Option<&str>,
            state: &str,
            error: Option<&str>,
        ) {
            self.events.lock().unwrap().push((
                primary.into(),
                fallback.map(String::from),
                state.into(),
                error.map(String::from),
            ));
        }

        fn emit_provider_failover_receipt(&self, receipt: serde_json::Value) {
            self.receipts.lock().unwrap().push(receipt);
        }

        fn emit_capability_activation(
            &self,
            activation: &wcore_protocol::events::CapabilityActivation,
        ) {
            self.activations.lock().unwrap().push((
                format!("{:?}", activation.capability),
                format!("{:?}", activation.stage),
            ));
        }
    }

    /// Stages recorded for one capability, in emission order.
    fn stages_for(rec: &Rec, capability: &str) -> Vec<String> {
        rec.activations
            .lock()
            .unwrap()
            .iter()
            .filter(|(cap, _)| cap == capability)
            .map(|(_, stage)| stage.clone())
            .collect()
    }

    #[test]
    fn reports_open_state_with_fallback_and_error() {
        let rec = Arc::new(Rec::default());
        let reporter = ProtocolCircuitReporter::new(rec.clone() as Arc<dyn OutputSink>);
        reporter.report(
            "primary-provider",
            Some("fallback-provider"),
            CircuitState::Open,
            Some("3 failures in 30s"),
        );
        let events = rec.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "primary-provider");
        assert_eq!(events[0].1.as_deref(), Some("fallback-provider"));
        assert_eq!(events[0].2, "open");
        assert_eq!(events[0].3.as_deref(), Some("3 failures in 30s"));
    }

    #[test]
    fn reports_closed_state_without_fallback() {
        let rec = Arc::new(Rec::default());
        let reporter = ProtocolCircuitReporter::new(rec.clone() as Arc<dyn OutputSink>);
        reporter.report("primary", None, CircuitState::Closed, None);
        let events = rec.events.lock().unwrap();
        assert_eq!(events[0].2, "closed");
        assert!(events[0].1.is_none());
        assert!(events[0].3.is_none());
    }

    /// F05-TRUTH-3 runtime outcome proof, run in BOTH directions in one test so
    /// the pass and the fail share an instrument (LANE-BRIEF §3b-iii).
    ///
    /// CAN-PASS: an `Open` transition — the cooldown tracker's failures crossed
    /// the threshold and routing changed — emits the full
    /// `reached → outcome_changed → observed` triple.
    ///
    /// CAN-FAIL: `Closed` and `HalfOpen` emit nothing. Without this half, an
    /// assertion that the triple is present would be satisfied by a reporter
    /// that emitted it on every call, which is not a proof of anything.
    #[test]
    fn cooldown_occurrence_fires_on_open_and_on_nothing_else() {
        const COOLDOWN: &str = "CooldownTracker";

        // ---- CAN-FAIL direction first, so a later positive cannot be a leftover ----
        let quiet = Arc::new(Rec::default());
        let reporter = ProtocolCircuitReporter::new(quiet.clone() as Arc<dyn OutputSink>);
        reporter.report("p", None, CircuitState::Closed, None);
        reporter.report("p", None, CircuitState::HalfOpen, None);
        assert!(
            stages_for(&quiet, COOLDOWN).is_empty(),
            "a resting or trial breaker emitted a runtime outcome proof; the occurrence \
             would then fire on every session and distinguish nothing. Got: {:?}",
            stages_for(&quiet, COOLDOWN)
        );
        // Instrument liveness for the negative: the circuit events themselves DID
        // arrive, so the empty activation list above is an absence in the thing
        // under test and not a dead sink.
        assert_eq!(
            quiet.events.lock().unwrap().len(),
            2,
            "instrument dead: the sink recorded no circuit events at all, so the \
             absence of activations above proves nothing"
        );

        // ---- CAN-PASS direction ----
        let tripped = Arc::new(Rec::default());
        let reporter = ProtocolCircuitReporter::new(tripped.clone() as Arc<dyn OutputSink>);
        reporter.report(
            "p",
            Some("fb"),
            CircuitState::Open,
            Some("3 failures in 30s"),
        );
        assert_eq!(
            stages_for(&tripped, COOLDOWN),
            vec![
                "Reached".to_string(),
                "OutcomeChanged".to_string(),
                "Observed".to_string()
            ],
            "an opened circuit did not produce the F05 runtime outcome triple"
        );
    }

    /// The occurrence must be scoped to its own capability. Without this, a
    /// reporter that emitted the triple for every `CapabilityId` would pass the
    /// test above while making every other row's proof worthless.
    #[test]
    fn the_open_occurrence_names_only_the_cooldown_tracker() {
        let rec = Arc::new(Rec::default());
        let reporter = ProtocolCircuitReporter::new(rec.clone() as Arc<dyn OutputSink>);
        reporter.report("p", None, CircuitState::Open, None);

        let capabilities: Vec<String> = rec
            .activations
            .lock()
            .unwrap()
            .iter()
            .map(|(cap, _)| cap.clone())
            .collect();
        assert!(
            !capabilities.is_empty(),
            "instrument dead: no activations recorded at all"
        );
        assert!(
            capabilities.iter().all(|c| c == "CooldownTracker"),
            "the circuit reporter claimed an outcome for a capability it does not \
             observe: {capabilities:?}"
        );
    }

    #[test]
    fn serializes_typed_failover_receipt_without_losing_cost_or_reason() {
        use wcore_providers::{CandidateReceipt, FailoverReason, PricingEvidence};

        let rec = Arc::new(Rec::default());
        let reporter = ProtocolCircuitReporter::new(rec.clone() as Arc<dyn OutputSink>);
        let mut receipt =
            FailoverReceipt::new(FailoverReason::RateLimit, "anthropic", "claude-sonnet-4-6");
        receipt.candidates.push(CandidateReceipt {
            provider: "openai".into(),
            model: "gpt-5".into(),
            region: Some("us-east".into()),
            disposition: Ok(()),
            failure_reason: None,
            cooldown_reason: None,
            retry_after_ms: Some(1_500),
            pricing: PricingEvidence {
                source: "cached_live".into(),
                age_seconds: Some(30),
                stale: false,
                priced: true,
                estimated_microcents: Some(4242),
            },
        });
        receipt.selected_provider = Some("openai".into());
        receipt.selected_model = Some("gpt-5".into());

        reporter.report_failover(&receipt);

        let receipts = rec.receipts.lock().unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0]["reason"], "rate_limit");
        assert_eq!(receipts[0]["selected_provider"], "openai");
        assert_eq!(
            receipts[0]["candidates"][0]["pricing"]["estimated_microcents"],
            4242
        );
    }
}
