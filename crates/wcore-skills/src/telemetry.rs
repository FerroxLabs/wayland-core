//! M3.5 — skill invocation telemetry.
//!
//! `SkillTool` emits one [`SkillTelemetryEvent`] after every skill execution
//! (success and failure). A [`SkillTelemetrySink`] impl persists those events.
//!
//! Two production sinks live here:
//!
//! - [`NullTelemetrySink`] — discards events; used when memory is disabled.
//! - [`ProceduralSkillTelemetrySink`] — writes events into the procedural
//!   partition of an `Arc<dyn MemoryApi>` via
//!   [`MemoryApi::record_skill_use`]. Lives here (not in `wcore-memory`)
//!   because `wcore-skills` already depends on `wcore-memory`; the reverse
//!   edge would create a cycle.
//!
//! Procedural is a **partition**, not a tier. The sink writes to
//! `Tier::Project`; per-tier policy lives in the dispatcher.

use std::sync::Arc;

use wcore_memory::api::MemoryApi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillOutcome {
    Success,
    Failure,
}

#[derive(Debug, Clone)]
pub struct SkillTelemetryEvent {
    pub skill_name: String,
    pub session_id: Option<String>,
    pub outcome: SkillOutcome,
    pub latency_ms: u64,
    pub ts_secs: i64,
}

/// Implementors persist (or forward) telemetry events. The default
/// [`NullTelemetrySink`] discards events; production wiring threads
/// [`ProceduralSkillTelemetrySink`] from this same module.
///
/// `record` is sync (no `async`) so it can be called from anywhere in the
/// agent loop without contaminating the call site with `.await`. Sinks
/// that need async I/O (the procedural sink does) spawn a detached tokio
/// task internally.
pub trait SkillTelemetrySink: Send + Sync {
    fn record(&self, ev: SkillTelemetryEvent);
}

/// No-op sink for tests and CLI flows where memory is disabled.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullTelemetrySink;

impl SkillTelemetrySink for NullTelemetrySink {
    fn record(&self, _ev: SkillTelemetryEvent) {}
}

/// `SkillTelemetrySink` that persists events into the procedural partition
/// of an `Arc<dyn MemoryApi>` via `record_skill_use`.
///
/// `record()` is sync (trait requirement); it spawns a tokio task to call
/// the async `record_skill_use`. Errors are logged via `tracing::warn!` and
/// dropped — telemetry must never crash the agent loop.
pub struct ProceduralSkillTelemetrySink {
    memory: Arc<dyn MemoryApi>,
}

impl ProceduralSkillTelemetrySink {
    pub fn new(memory: Arc<dyn MemoryApi>) -> Self {
        Self { memory }
    }
}

impl SkillTelemetrySink for ProceduralSkillTelemetrySink {
    fn record(&self, ev: SkillTelemetryEvent) {
        let memory = self.memory.clone();
        // Detached task; telemetry is fire-and-forget.
        tokio::spawn(async move {
            let succeeded = matches!(ev.outcome, SkillOutcome::Success);
            if let Err(e) = memory
                .record_skill_use(&ev.skill_name, succeeded, ev.latency_ms)
                .await
            {
                tracing::warn!(
                    target: "wcore_skills::telemetry",
                    skill = %ev.skill_name,
                    error = %e,
                    "record_skill_use failed; dropping telemetry event"
                );
            }
        });
    }
}
