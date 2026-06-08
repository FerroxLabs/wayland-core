//! M3.3 — observability surface for memory operations.
//!
//! Verifies `MemoryOpTrace` schema + `MemoryTraceSink` trait + the
//! `ObservabilityMemoryTraceBridge` that adapts `MemoryTraceSink` calls
//! into JSON `SpanSink::emit` invocations.

use std::sync::Arc;

use wcore_observability::sink::{MemoryTraceSink, ObservabilityMemoryTraceBridge};
use wcore_observability::trace::MemoryOpTrace;

#[test]
fn memory_op_trace_carries_source_product() {
    let t = MemoryOpTrace::new(
        "record_episode".into(),
        "episodic".into(),
        "project".into(),
        12,
        true,
    );
    assert_eq!(t.op, "record_episode");
    assert_eq!(t.partition, "episodic");
    assert_eq!(t.tier, "project");
    assert_eq!(t.latency_ms, 12);
    assert!(t.success);
    assert_eq!(t.source_product, wcore_observability::SOURCE_PRODUCT);
}

#[test]
fn memory_op_trace_serializes_with_all_fields() {
    let t = MemoryOpTrace::new("search".into(), "episodic".into(), "-".into(), 0, false);
    let v = serde_json::to_value(&t).unwrap();
    assert_eq!(v["op"], "search");
    assert_eq!(v["partition"], "episodic");
    assert_eq!(v["tier"], "-");
    assert_eq!(v["latency_ms"], 0);
    assert_eq!(v["success"], false);
    assert_eq!(v["source_product"], wcore_observability::SOURCE_PRODUCT);
}

#[test]
fn memory_trace_bridge_forwards_into_span_sink() {
    let span_sink = wcore_observability::sink::InMemorySink::new();
    let bridge = ObservabilityMemoryTraceBridge::new(Arc::new(span_sink.clone()));

    bridge.emit("search", "episodic", "project", 5, true);
    bridge.emit("record_episode", "episodic", "session", 12, true);

    let captured = span_sink.snapshot();
    assert_eq!(
        captured.len(),
        2,
        "two emits should produce two span values"
    );

    assert_eq!(captured[0]["op"], "search");
    assert_eq!(captured[0]["partition"], "episodic");
    assert_eq!(captured[0]["tier"], "project");
    assert_eq!(captured[0]["latency_ms"], 5);
    assert_eq!(captured[0]["success"], true);

    assert_eq!(captured[1]["op"], "record_episode");
    assert_eq!(captured[1]["latency_ms"], 12);
}

#[test]
fn memory_trace_bridge_records_failure_outcome() {
    let span_sink = wcore_observability::sink::InMemorySink::new();
    let bridge = ObservabilityMemoryTraceBridge::new(Arc::new(span_sink.clone()));
    bridge.emit("get_episode", "episodic", "global", 3, false);

    let captured = span_sink.snapshot();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0]["success"], false);
}
