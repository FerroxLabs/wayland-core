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

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use wcore_providers::{CircuitReporter, CircuitState, FailoverReceipt};

use crate::output::OutputSink;

pub struct ProtocolCircuitReporter {
    output: Arc<dyn OutputSink>,
    /// Failover sentences already said this session (#1133). Keyed on the
    /// rendered text: a chain working as designed fails over on every turn,
    /// and repeating the same line each time is noise, while a failover to a
    /// DIFFERENT provider is a new fact and is worth saying.
    announced: Mutex<HashSet<String>>,
}

impl ProtocolCircuitReporter {
    pub fn new(output: Arc<dyn OutputSink>) -> Self {
        Self {
            output,
            announced: Mutex::new(HashSet::new()),
        }
    }

    /// True the first time this exact sentence is offered.
    ///
    /// A poisoned lock must never swallow the notice — the same rule
    /// `wcore_config::config::emit_credential_notice_once` follows.
    fn first_time(&self, notice: &str) -> bool {
        match self.announced.lock() {
            Ok(mut seen) => seen.insert(notice.to_string()),
            Err(_) => true,
        }
    }
}

/// The operator-facing line for a chain that ROUTED AROUND its primary, or
/// `None` when the receipt records no successful selection.
///
/// #1133. A configured chain that works told the user nothing at all: the run
/// looked like an ordinary success while the answer came from a different
/// provider, a different model and a different bill. The receipt was already
/// built, already reported and already carried every fact in this sentence —
/// it just went to `emit_provider_failover_receipt`, which is a default no-op
/// on every sink except the JSON-stream one. So a TUI or headless user saw
/// nothing, and — worse — configuring a fallback REMOVED the `base_url`
/// diagnosis they would have got with no chain at all.
///
/// Split out and pure so the exact words are under test, the same shape as
/// `bootstrap::local_shell_notice`.
///
/// **The failover CLASS is deliberately not printed.** `receipt.reason` is a
/// `FailoverReason`, which classifies a refused connection and a read timeout
/// identically as `timeout` (the finding that made #1127 prefer
/// `primary_failure_code`). The precise code is not reachable here: it lives on
/// `ResilientProvider`, and widening `FailoverReceipt` to carry it would break
/// the desktop wire schema, which pins the receipt with
/// `additionalProperties: false`. A sentence that named the wrong fault class
/// would send the reader hunting timeouts for a closed port, so this one names
/// what is certainly true and points at the checks that discriminate.
fn failover_notice(receipt: &FailoverReceipt) -> Option<String> {
    let selected_provider = receipt.selected_provider.as_ref()?;
    let selected_model = receipt.selected_model.as_ref()?;
    Some(format!(
        "notice: provider failover — '{failed_provider}' ({failed_model}) did not serve this \
         request, so '{selected_provider}' ({selected_model}) answered it instead. This turn's \
         reply, tool behaviour and cost come from {selected_provider}/{selected_model}, not from \
         the provider you configured. Check '{failed_provider}' — its `base_url`, credentials \
         and quota — or set `[provider_chain] enabled = false` to stop routing around it and see \
         the underlying error instead.",
        failed_provider = receipt.failed_provider,
        failed_model = receipt.failed_model,
    ))
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
        // #1133 — the human half. The structured receipt below is for the
        // desktop host; `emit_provider_failover_receipt` is a default no-op on
        // every other sink, so without this line a TUI or headless user is
        // never told that a different provider is answering, at a different
        // price. `emit_info` is the channel the retry notices already use.
        if let Some(notice) = failover_notice(receipt)
            && self.first_time(&notice)
        {
            self.output.emit_info(&notice);
        }
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

#[cfg(test)]
mod failover_notice_tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use wcore_providers::{FailoverReason, FailoverReceipt};
    use wcore_types::message::FinishReason;

    #[derive(Default)]
    struct InfoSink {
        infos: StdMutex<Vec<String>>,
        receipts: StdMutex<usize>,
    }
    impl OutputSink for InfoSink {
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
        fn emit_info(&self, msg: &str) {
            self.infos.lock().unwrap().push(msg.to_string());
        }
        fn emit_provider_circuit_event(&self, _: &str, _: Option<&str>, _: &str, _: Option<&str>) {}
        fn emit_provider_failover_receipt(&self, _: serde_json::Value) {
            *self.receipts.lock().unwrap() += 1;
        }
    }

    fn served_by_fallback() -> FailoverReceipt {
        let mut receipt =
            FailoverReceipt::new(FailoverReason::Timeout, "anthropic", "claude-sonnet-4-6");
        receipt.selected_provider = Some("openai".into());
        receipt.selected_model = Some("gpt-4o".into());
        receipt
    }

    /// The sentence has to carry the four facts a user cannot otherwise get:
    /// who failed, what they asked for, who answered, and what to check.
    #[test]
    fn the_notice_names_both_providers_and_the_check() {
        let notice = failover_notice(&served_by_fallback()).expect("a served failover");
        for fact in [
            "anthropic",
            "claude-sonnet-4-6",
            "openai",
            "gpt-4o",
            "base_url",
        ] {
            assert!(notice.contains(fact), "the notice drops `{fact}`: {notice}");
        }
        // The class is deliberately absent: `Timeout` here is a REFUSED
        // connection as often as a real timeout, and naming it would send the
        // reader hunting the wrong fault.
        assert!(
            !notice.contains("timeout"),
            "the notice printed the coarse failover class, which cannot tell a refused \
             connection from a read timeout: {notice}"
        );
    }

    /// CAN-FAIL half of the same instrument: a receipt with no selection is a
    /// COLLAPSED chain, which ends on its own error message (`#1127`). A notice
    /// there would claim a provider served the turn when none did.
    #[test]
    fn a_chain_that_selected_nothing_says_nothing() {
        let collapsed =
            FailoverReceipt::new(FailoverReason::Timeout, "anthropic", "claude-sonnet-4-6");
        assert_eq!(
            failover_notice(&collapsed),
            None,
            "a chain that dispatched nobody announced a failover anyway"
        );
    }

    /// A chain working as designed fails over on EVERY turn. Saying it once is
    /// a notice; saying it every turn is noise the user learns to ignore.
    #[test]
    fn the_notice_is_said_once_per_session_and_the_receipt_every_time() {
        let sink = Arc::new(InfoSink::default());
        let reporter = ProtocolCircuitReporter::new(sink.clone() as Arc<dyn OutputSink>);
        let receipt = served_by_fallback();
        reporter.report_failover(&receipt);
        reporter.report_failover(&receipt);
        reporter.report_failover(&receipt);

        // Snapshot before asserting: a message that re-locks the same mutex the
        // assertion is already holding DEADLOCKS on failure, and a test that
        // hangs instead of failing is not an instrument.
        let said = sink.infos.lock().unwrap().clone();
        assert_eq!(said.len(), 1, "the failover notice repeated: {said:?}");
        // Instrument liveness: the suppression must be of the NOTICE only. If
        // the receipts had also stopped, the assertion above would pass for a
        // reporter that had simply gone dead after the first call.
        assert_eq!(
            *sink.receipts.lock().unwrap(),
            3,
            "the structured receipt was suppressed too — the host loses failover evidence"
        );

        // A failover to a DIFFERENT provider is a new fact, not a repeat.
        let mut elsewhere = served_by_fallback();
        elsewhere.selected_provider = Some("vertex".into());
        elsewhere.selected_model = Some("gemini-2.5-pro".into());
        reporter.report_failover(&elsewhere);
        let said = sink.infos.lock().unwrap().clone();
        assert_eq!(
            said.len(),
            2,
            "a failover to a different provider was suppressed as a repeat: {said:?}"
        );
    }
}
