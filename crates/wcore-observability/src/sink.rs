//! Span / trace emission sinks.
//!
//! `SpanSink` is the abstraction the agent loop targets. W1 provides two
//! always-built impls (`InMemorySink` for tests + diagnostics buffering,
//! `JsonStdoutSink` for dev) and one feature-gated impl (`OtlpSink`, behind
//! the `otlp` cargo feature, added in Task 7).

use std::sync::{Arc, Mutex};

use serde_json::Value;

/// Where a trace (serialized to a JSON `Value`) goes after the agent
/// loop finishes emitting it. Implementations are responsible for their own
/// I/O / network failure handling — the agent does not block on emission.
pub trait SpanSink: Send + Sync {
    fn emit(&self, trace: &Value);
}

/// M3.3 — minimal observability surface that the memory crate can call
/// without taking on the full `SpanSink` JSON-value contract. Implementors
/// translate the call into whatever schema they care about
/// (`ObservabilityMemoryTraceBridge` below routes it through
/// `MemoryOpTrace` + `SpanSink`).
///
/// The trait lives in `wcore-observability` (and is imported by
/// `wcore-memory::partition`) because the existing dependency direction is
/// `wcore-memory → wcore-observability` (memory's core inference uses
/// `wcore_observability::trace::TurnTrace`). Adding the reverse edge to
/// host the trait in `wcore-memory::api` would introduce a cycle.
pub trait MemoryTraceSink: Send + Sync {
    /// Emit one memory-op event. Implementations must not panic and must
    /// remain non-blocking — memory hot paths emit synchronously.
    fn emit(&self, op: &str, partition: &str, tier: &str, latency_ms: u64, success: bool);
}

/// M3.3 — adapter from `MemoryTraceSink` calls into a
/// `SpanSink`-backed JSON channel. Owned by callers (e.g.
/// `wcore-agent::bootstrap`) that already hold an `Arc<dyn SpanSink>`.
///
/// Each `emit` builds a `MemoryOpTrace`, serializes it to a JSON `Value`,
/// and forwards to the inner `SpanSink`. Serialization failure is
/// suppressed (the trace is dropped rather than panicking) so the memory
/// hot path is never destabilized by a sink bug.
pub struct ObservabilityMemoryTraceBridge {
    inner: Arc<dyn SpanSink>,
}

impl ObservabilityMemoryTraceBridge {
    pub fn new(inner: Arc<dyn SpanSink>) -> Self {
        Self { inner }
    }
}

impl MemoryTraceSink for ObservabilityMemoryTraceBridge {
    fn emit(&self, op: &str, partition: &str, tier: &str, latency_ms: u64, success: bool) {
        let trace = crate::trace::MemoryOpTrace::new(
            op.to_string(),
            partition.to_string(),
            tier.to_string(),
            latency_ms,
            success,
        );
        // Best-effort: drop the trace if serialization fails rather than
        // propagating a panic up through the memory hot path.
        if let Ok(value) = serde_json::to_value(&trace) {
            self.inner.emit(&value);
        }
    }
}

/// M5.3 — adapter from `BudgetEventSink` calls into a `SpanSink`-backed
/// JSON channel. Same shape as `ObservabilityMemoryTraceBridge` above:
/// owned by callers (e.g. `wcore-agent::bootstrap`) that already hold an
/// `Arc<dyn SpanSink>`, each `BudgetEvent` is serialized to a JSON `Value`
/// and forwarded. Serialization failure is dropped rather than propagated
/// so the budget charge hot path is never destabilized by a sink bug.
pub struct ObservabilityBudgetEventBridge {
    inner: Arc<dyn SpanSink>,
}

impl ObservabilityBudgetEventBridge {
    pub fn new(inner: Arc<dyn SpanSink>) -> Self {
        Self { inner }
    }
}

impl wcore_budget::BudgetEventSink for ObservabilityBudgetEventBridge {
    fn emit(&self, event: &wcore_budget::BudgetEvent) {
        if let Ok(value) = serde_json::to_value(event) {
            self.inner.emit(&value);
        }
    }
}

/// A sink that stashes every emitted trace in an `Arc<Mutex<Vec<Value>>>`.
/// Used in unit tests and as the in-process buffer underlying future
/// HITL-suspend trace-replay tooling.
#[derive(Default, Clone)]
pub struct InMemorySink {
    inner: Arc<Mutex<Vec<Value>>>,
}

impl InMemorySink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<Value> {
        self.inner.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SpanSink for InMemorySink {
    fn emit(&self, trace: &Value) {
        if let Ok(mut g) = self.inner.lock() {
            g.push(trace.clone());
        }
    }
}

/// A `SpanSink` wrapper that scrubs credential/PII patterns from each trace's
/// serialized JSON before forwarding to the inner sink. Scrubbing is always
/// on — it is purely defensive and has negligible cost on the fast-bail-out
/// path (no match → zero allocation via `Cow::Borrowed`).
///
/// Construct via `PiiScrubbingSink::wrap(inner)`. The inner sink receives
/// the scrubbed JSON value; if re-parsing fails (should never happen for
/// well-formed JSON) the original value is forwarded unmodified so no trace
/// is silently dropped.
pub struct PiiScrubbingSink {
    inner: Arc<dyn SpanSink>,
}

impl PiiScrubbingSink {
    pub fn wrap(inner: Arc<dyn SpanSink>) -> Self {
        Self { inner }
    }
}

impl SpanSink for PiiScrubbingSink {
    fn emit(&self, trace: &serde_json::Value) {
        use wcore_safety::PIIScrubber;

        // Serialize → scrub → re-parse. The round-trip is necessary because
        // credentials can appear inside any string field at arbitrary nesting.
        // On the hot path (no credentials present) `scrub` returns
        // `Cow::Borrowed` so the only allocation is `to_string()`.
        let scrubbed = match serde_json::to_string(trace) {
            Ok(raw) => {
                let clean = PIIScrubber.scrub(&raw);
                // Re-parse only when something was actually replaced.
                match clean {
                    std::borrow::Cow::Borrowed(_) => {
                        // Nothing changed — forward original value as-is.
                        self.inner.emit(trace);
                        return;
                    }
                    std::borrow::Cow::Owned(ref s) => serde_json::from_str::<serde_json::Value>(s)
                        .unwrap_or_else(|_| trace.clone()),
                }
            }
            // Serialization failure shouldn't happen for a Value, but if it
            // does forward the original rather than dropping the trace.
            Err(_) => trace.clone(),
        };
        self.inner.emit(&scrubbed);
    }
}

/// Sink that writes one JSON line per trace to stdout. Useful for `wcore`
/// invocations outside the JSON-stream-protocol mode where the host doesn't
/// consume `TraceEvent` directly.
pub struct JsonStdoutSink;

impl SpanSink for JsonStdoutSink {
    fn emit(&self, trace: &Value) {
        // Best-effort; trace emission must never propagate panic.
        if let Ok(line) = serde_json::to_string(trace) {
            println!("{line}");
        }
    }
}

// ── OTLP sink (feature-gated) ───────────────────────────────────────────────
//
// Only compiles when the `otlp` feature is enabled. Default builds skip the
// opentelemetry crates entirely so the engine binary stays inside the
// §2.2 binary-size budget.

#[cfg(feature = "otlp")]
pub use otlp_impl::{OtlpSink, OtlpSinkError};

#[cfg(feature = "otlp")]
mod otlp_impl {
    use opentelemetry::trace::{Tracer, TracerProvider as _};
    use opentelemetry::{KeyValue, global};
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::runtime;
    use opentelemetry_sdk::trace::TracerProvider as SdkTracerProvider;
    use serde_json::Value;
    use std::sync::OnceLock;
    use thiserror::Error;

    use super::SpanSink;
    use crate::SOURCE_PRODUCT;

    /// Static handle so multiple agent sessions share one exporter.
    static PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

    /// Errors raised by `OtlpSink::new`. AGENTS.md forbids `.expect()` in
    /// production code paths; this error type lets callers handle a flaky
    /// endpoint / TLS / proxy condition without panicking.
    #[derive(Debug, Error)]
    pub enum OtlpSinkError {
        /// The OTLP HTTP exporter could not be constructed (DNS, TLS, proxy,
        /// or invalid endpoint URL).
        #[error("OTLP exporter build failed: {0}")]
        Exporter(#[from] opentelemetry::trace::TraceError),
    }

    pub struct OtlpSink {
        tracer: opentelemetry_sdk::trace::Tracer,
    }

    impl OtlpSink {
        /// Initialise the OTLP exporter against `endpoint` (e.g.
        /// `http://localhost:4318/v1/traces`). Returns an error if the
        /// exporter or the provider cannot be built. Idempotent: repeated
        /// successful calls reuse the static provider; a failed build leaves
        /// `PROVIDER` un-initialised so callers can retry.
        ///
        /// Per AGENTS.md "no `.expect()` in production code", construction is
        /// fully fallible: DNS / TLS / proxy state can flip at runtime and the
        /// caller must surface the error rather than crashing the agent.
        pub fn new(endpoint: &str) -> Result<Self, OtlpSinkError> {
            // Build the exporter BEFORE entering `get_or_init` so the
            // build failure can propagate. `get_or_init` cannot host a
            // fallible closure — its signature is `FnOnce() -> T`.
            //
            // If a previous successful call already initialised PROVIDER,
            // we skip the build entirely and reuse the static handle.
            let provider = if let Some(existing) = PROVIDER.get() {
                existing
            } else {
                let exporter = opentelemetry_otlp::SpanExporter::builder()
                    .with_http()
                    .with_endpoint(endpoint)
                    .build()?; // propagates TraceError via #[from]
                let resource = Resource::new(vec![KeyValue::new("service.name", SOURCE_PRODUCT)]);
                let built = SdkTracerProvider::builder()
                    .with_resource(resource)
                    .with_batch_exporter(exporter, runtime::Tokio)
                    .build();
                // `set` returns Err if another thread won the race; in that
                // case reuse the winner's provider. The remaining .expect()s
                // are guarded by control-flow that just wrote the slot — the
                // invariant is proven inline.
                match PROVIDER.set(built) {
                    // SAFETY: `set` returned Ok above, so the
                    // OnceCell is now populated; `get` cannot return
                    // None on the very next line.
                    Ok(()) => PROVIDER.get().expect("just-set provider must be present"),
                    // SAFETY: the race-winner branch — `set` only
                    // returns Err when another thread already wrote
                    // the OnceCell, so `get` returns Some.
                    Err(_) => PROVIDER
                        .get()
                        .expect("race-winner provider must be present"),
                }
            };
            let tracer = provider.tracer("wcore-agent");
            global::set_tracer_provider(provider.clone());
            Ok(Self { tracer })
        }
    }

    impl SpanSink for OtlpSink {
        fn emit(&self, trace: &Value) {
            // Each turn trace becomes one span with the full JSON serialised
            // as an attribute. The W1 wave doesn't model nested provider /
            // tool spans — that level of granularity ships when W7 sub-agent
            // traces land. For now the span carries everything the host can
            // consume from `TraceEvent`.
            let span_name = trace
                .get("turn")
                .and_then(|v| v.as_u64())
                .map(|n| format!("turn-{n}"))
                .unwrap_or_else(|| "turn".to_string());
            let _span = self
                .tracer
                .span_builder(span_name)
                .with_attributes(vec![
                    KeyValue::new("source_product", SOURCE_PRODUCT),
                    KeyValue::new(
                        "trace.json",
                        serde_json::to_string(trace).unwrap_or_default(),
                    ),
                ])
                .start(&self.tracer);
            // Span ends on drop.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn in_memory_sink_starts_empty() {
        let s = InMemorySink::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert!(s.snapshot().is_empty());
    }

    #[test]
    fn in_memory_sink_records_emitted_traces_in_order() {
        let s = InMemorySink::new();
        s.emit(&json!({ "turn": 0 }));
        s.emit(&json!({ "turn": 1 }));
        s.emit(&json!({ "turn": 2 }));

        let snap = s.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0]["turn"], 0);
        assert_eq!(snap[1]["turn"], 1);
        assert_eq!(snap[2]["turn"], 2);
    }

    #[test]
    fn in_memory_sink_shares_state_across_clones() {
        let s = InMemorySink::new();
        let s2 = s.clone();
        s.emit(&json!({ "from": "first" }));
        s2.emit(&json!({ "from": "second" }));

        assert_eq!(s.len(), 2, "clones must share the inner buffer");
        assert_eq!(s2.snapshot()[0]["from"], "first");
    }
}
