// M3.5.2 — telemetry event + sink trait basic shape.
//
// Verifies the `wcore_skills::telemetry` module exposes:
//  - `SkillOutcome` enum (Success / Failure)
//  - `SkillTelemetryEvent` struct with the documented fields
//  - `SkillTelemetrySink` trait with `record(&self, ev)`
// …and that a simple counting impl wired through `record` updates state.

use std::sync::atomic::{AtomicU64, Ordering};

use wcore_skills::telemetry::{SkillOutcome, SkillTelemetryEvent, SkillTelemetrySink};

struct Counting(AtomicU64);
impl SkillTelemetrySink for Counting {
    fn record(&self, _ev: SkillTelemetryEvent) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn telemetry_event_constructed_with_fields() {
    let ev = SkillTelemetryEvent {
        skill_name: "x".into(),
        session_id: Some("s1".into()),
        outcome: SkillOutcome::Success,
        latency_ms: 42,
        ts_secs: 1_700_000_000,
    };
    assert_eq!(ev.skill_name, "x");
    assert_eq!(ev.session_id.as_deref(), Some("s1"));
    assert_eq!(ev.latency_ms, 42);
    assert_eq!(ev.ts_secs, 1_700_000_000);
    assert!(matches!(ev.outcome, SkillOutcome::Success));
}

#[test]
fn sink_record_fires_for_each_event() {
    let s = Counting(AtomicU64::new(0));
    s.record(SkillTelemetryEvent {
        skill_name: "x".into(),
        session_id: None,
        outcome: SkillOutcome::Failure,
        latency_ms: 0,
        ts_secs: 0,
    });
    s.record(SkillTelemetryEvent {
        skill_name: "y".into(),
        session_id: None,
        outcome: SkillOutcome::Success,
        latency_ms: 1,
        ts_secs: 0,
    });
    assert_eq!(s.0.load(Ordering::SeqCst), 2);
}

#[test]
fn null_sink_is_a_zero_cost_default_sink() {
    use wcore_skills::telemetry::NullTelemetrySink;
    let s = NullTelemetrySink;
    // Just confirm it compiles + accepts events without panic.
    s.record(SkillTelemetryEvent {
        skill_name: "x".into(),
        session_id: None,
        outcome: SkillOutcome::Failure,
        latency_ms: 0,
        ts_secs: 0,
    });
}
