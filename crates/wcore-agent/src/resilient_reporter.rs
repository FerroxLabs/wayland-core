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

use wcore_providers::{CircuitReporter, CircuitState};

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
}
