//! v0.9.0 Wave-1 B7 — live engine state accessor for `wayland_status` +
//! `wayland_telemetry_query`.
//!
//! The introspection backend is purely in-process: it reads counters,
//! tool-call histograms, recent errors, and provider-health flags that
//! the engine (and the TUI status bar) already maintain. This module
//! defines the read-only [`SessionStateReader`] trait the backend
//! depends on, plus a writable [`InMemorySessionState`] default that
//! the engine populates as it runs.
//!
//! ## Why a fresh struct (no `SessionState`)
//!
//! The existing `Session` in `session.rs` is the **persisted chat
//! history** (messages + token totals serialised to disk). The live
//! introspection surface needs counters that move per-call — tool
//! invocations, recent errors, per-provider health — which never
//! belonged in the on-disk session blob. Rather than overload that
//! type, B7 introduces a separate `InMemorySessionState` whose owner
//! is the runtime (engine / bootstrap), and writes are O(1)
//! atomic-counter / `Mutex<HashMap>` mutations.
//!
//! ## Wire-up
//!
//! `bootstrap.rs` constructs one `Arc<InMemorySessionState>`,
//! passes it to `build_introspection_backend`, and (in a follow-up
//! commit outside B7's scope) threads the same `Arc` to the engine
//! so per-turn counters land in the same struct the tools read.
//! For Wave-1 the engine has not yet been re-wired; the backend
//! happily reports zero counters when nothing has written, and the
//! tool tests below populate state directly to exercise the read
//! side end-to-end.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// A single error captured by the engine for `wayland_status` recent-error
/// reporting. Kept intentionally minimal — the introspection tool only
/// surfaces "what went wrong recently?" not a full structured error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineError {
    /// When the error was observed.
    pub timestamp: DateTime<Utc>,
    /// Human-readable message. Provider name should be prefixed at the
    /// call site (e.g. `"anthropic: 429 rate limited"`).
    pub message: String,
}

/// Coarse-grained provider health flag. Engine writes this when a
/// provider call succeeds / errors; the tool reads it as part of the
/// status snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealthStatus {
    Ok,
    Degraded,
    Down,
}

/// Per-provider health record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub status: ProviderHealthStatus,
    pub last_check: DateTime<Utc>,
}

/// Read-only view of the runtime's session state. Implementations live
/// behind `Arc<dyn SessionStateReader>` so the introspection backend
/// has no knowledge of writer-side concurrency primitives.
pub trait SessionStateReader: Send + Sync + 'static {
    fn token_count_input(&self) -> u64;
    fn token_count_output(&self) -> u64;
    /// Map: tool name → cumulative invocation count.
    fn tool_call_count(&self) -> HashMap<String, u64>;
    /// Most-recent `n` engine errors, newest first.
    fn recent_errors(&self, n: usize) -> Vec<EngineError>;
    /// Snapshot of per-provider health flags.
    fn provider_health(&self) -> HashMap<String, ProviderHealth>;
    /// Active model identifier (e.g. `"claude-opus-4-5"`).
    fn active_model(&self) -> String;
    /// When the current session started (used by `wayland_status` to
    /// report `session_duration_secs`).
    fn session_started_at(&self) -> DateTime<Utc>;
}

/// Cap on the number of recent errors retained in memory. The TUI
/// status bar only ever asks for the last ~3-5; we keep more so
/// `wayland_telemetry_query` can ask for larger windows without
/// the engine having to back-fill.
const RECENT_ERRORS_CAP: usize = 64;

/// Default in-memory implementation. Engine ownership: each session
/// constructs one and shares the `Arc` with the introspection backend
/// + per-turn writer paths.
pub struct InMemorySessionState {
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
    tool_calls: Mutex<HashMap<String, u64>>,
    recent_errors: Mutex<Vec<EngineError>>,
    provider_health: Mutex<HashMap<String, ProviderHealth>>,
    active_model: Mutex<String>,
    session_started_at: DateTime<Utc>,
}

impl InMemorySessionState {
    /// Construct an empty state stamped with the supplied model name and
    /// the current wall-clock time as the session start.
    pub fn new(active_model: impl Into<String>) -> Self {
        Self {
            input_tokens: AtomicU64::new(0),
            output_tokens: AtomicU64::new(0),
            tool_calls: Mutex::new(HashMap::new()),
            recent_errors: Mutex::new(Vec::new()),
            provider_health: Mutex::new(HashMap::new()),
            active_model: Mutex::new(active_model.into()),
            session_started_at: Utc::now(),
        }
    }

    /// Convenience: wrap in an `Arc` for handing to backends.
    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }

    // ── Writer-side methods (engine call sites) ──────────────────────

    /// Add to the input/output token totals atomically.
    pub fn add_token_usage(&self, input: u64, output: u64) {
        self.input_tokens.fetch_add(input, Ordering::Relaxed);
        self.output_tokens.fetch_add(output, Ordering::Relaxed);
    }

    /// Increment the per-tool call counter.
    pub fn record_tool_call(&self, tool: &str) {
        *self.tool_calls.lock().entry(tool.to_string()).or_insert(0) += 1;
    }

    /// Append a new error to the recent-errors ring (oldest evicted).
    pub fn push_error(&self, message: impl Into<String>) {
        let mut errs = self.recent_errors.lock();
        errs.push(EngineError {
            timestamp: Utc::now(),
            message: message.into(),
        });
        if errs.len() > RECENT_ERRORS_CAP {
            let drop_n = errs.len() - RECENT_ERRORS_CAP;
            errs.drain(0..drop_n);
        }
    }

    /// Record / update health for a provider.
    pub fn set_provider_health(&self, provider: &str, status: ProviderHealthStatus) {
        self.provider_health.lock().insert(
            provider.to_string(),
            ProviderHealth {
                status,
                last_check: Utc::now(),
            },
        );
    }

    /// Swap the active model label (e.g. after a `/model` switch).
    pub fn set_active_model(&self, model: impl Into<String>) {
        *self.active_model.lock() = model.into();
    }
}

impl Default for InMemorySessionState {
    fn default() -> Self {
        Self::new("unknown")
    }
}

impl SessionStateReader for InMemorySessionState {
    fn token_count_input(&self) -> u64 {
        self.input_tokens.load(Ordering::Relaxed)
    }

    fn token_count_output(&self) -> u64 {
        self.output_tokens.load(Ordering::Relaxed)
    }

    fn tool_call_count(&self) -> HashMap<String, u64> {
        self.tool_calls.lock().clone()
    }

    fn recent_errors(&self, n: usize) -> Vec<EngineError> {
        let errs = self.recent_errors.lock();
        let take = n.min(errs.len());
        // Newest first.
        errs.iter().rev().take(take).cloned().collect()
    }

    fn provider_health(&self) -> HashMap<String, ProviderHealth> {
        self.provider_health.lock().clone()
    }

    fn active_model(&self) -> String {
        self.active_model.lock().clone()
    }

    fn session_started_at(&self) -> DateTime<Utc> {
        self.session_started_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_zero() {
        let s = InMemorySessionState::default();
        assert_eq!(s.token_count_input(), 0);
        assert_eq!(s.token_count_output(), 0);
        assert!(s.tool_call_count().is_empty());
        assert!(s.recent_errors(10).is_empty());
        assert!(s.provider_health().is_empty());
        assert_eq!(s.active_model(), "unknown");
    }

    #[test]
    fn add_token_usage_accumulates() {
        let s = InMemorySessionState::new("claude-opus");
        s.add_token_usage(10, 20);
        s.add_token_usage(5, 7);
        assert_eq!(s.token_count_input(), 15);
        assert_eq!(s.token_count_output(), 27);
    }

    #[test]
    fn record_tool_call_increments() {
        let s = InMemorySessionState::default();
        s.record_tool_call("read");
        s.record_tool_call("read");
        s.record_tool_call("bash");
        let counts = s.tool_call_count();
        assert_eq!(counts.get("read"), Some(&2));
        assert_eq!(counts.get("bash"), Some(&1));
    }

    #[test]
    fn recent_errors_returns_newest_first_up_to_n() {
        let s = InMemorySessionState::default();
        s.push_error("first");
        s.push_error("second");
        s.push_error("third");
        let last_two = s.recent_errors(2);
        assert_eq!(last_two.len(), 2);
        assert_eq!(last_two[0].message, "third");
        assert_eq!(last_two[1].message, "second");
    }

    #[test]
    fn recent_errors_capped() {
        let s = InMemorySessionState::default();
        for i in 0..(RECENT_ERRORS_CAP + 10) {
            s.push_error(format!("err-{i}"));
        }
        // Asking for more than cap clamps to the cap.
        let all = s.recent_errors(usize::MAX);
        assert_eq!(all.len(), RECENT_ERRORS_CAP);
        // Newest must be the very last push.
        assert_eq!(
            all[0].message,
            format!("err-{}", RECENT_ERRORS_CAP + 10 - 1)
        );
    }

    #[test]
    fn provider_health_updates_replace_prior() {
        let s = InMemorySessionState::default();
        s.set_provider_health("anthropic", ProviderHealthStatus::Ok);
        s.set_provider_health("anthropic", ProviderHealthStatus::Down);
        let h = s.provider_health();
        assert_eq!(
            h.get("anthropic").map(|p| p.status),
            Some(ProviderHealthStatus::Down)
        );
    }

    #[test]
    fn set_active_model_round_trips() {
        let s = InMemorySessionState::new("a");
        assert_eq!(s.active_model(), "a");
        s.set_active_model("b");
        assert_eq!(s.active_model(), "b");
    }

    #[test]
    fn session_started_at_is_stable() {
        let s = InMemorySessionState::default();
        let t1 = s.session_started_at();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let t2 = s.session_started_at();
        assert_eq!(t1, t2);
    }
}
