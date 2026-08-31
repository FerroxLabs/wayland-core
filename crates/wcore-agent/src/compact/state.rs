use wcore_config::compact::CompactConfig;

/// Runtime state for the compaction circuit breaker.
///
/// Tracks consecutive autocompact failures so we can stop retrying
/// after `config.max_failures` consecutive failures.
#[derive(Debug, Clone)]
pub struct CompactState {
    /// Number of consecutive autocompact failures.
    pub consecutive_failures: u32,
    /// CONSERVATIVE watermark: `max(provider_reported, local_estimate)`,
    /// with historical thinking counted. Drives the EMERGENCY hard-stop,
    /// which must over-estimate so it never blows the context window.
    pub last_input_tokens: u64,
    /// Finding #174 — REAL-pressure watermark for the AUTO-compaction
    /// trigger. Prefers the provider-reported billed input; falls back to
    /// the local estimate (with wire-dropped thinking EXCLUDED) only when
    /// real usage isn't known yet (e.g. the first turn). Tracking real
    /// pressure here — rather than the inflated `max()` — stops auto
    /// compaction from firing prematurely.
    pub last_real_input_tokens: u64,
    /// FerroxLabs/wayland#1172 — this route's SERVED context window, learned
    /// from the `usage` the provider already returns.
    ///
    /// A self-hosted endpoint's ADVERTISED window is not the number that
    /// binds: measured against a real `qwen3:8b` on stock Ollama, the model
    /// advertised 40,960 while the loaded slot was 4,096 and the server
    /// silently discarded the head of every oversized prompt down to it. Only
    /// what came BACK reveals that, and it costs no extra request.
    pub served_window: wcore_config::context_window::ServedWindowTracker,
    /// FerroxLabs/wayland#1230 -- the slot the endpoint STATED it is serving,
    /// as opposed to the one `served_window` above INFERS after the fact.
    ///
    /// The two are complementary and neither replaces the other. The tracker
    /// needs no cooperation from the endpoint but cannot answer until a prompt
    /// has already been truncated twice; this one answers before the first
    /// request but only for an endpoint that will say (today: an operator-
    /// declared Ollama, via `/api/ps`). `None` is "did not say", never "the
    /// window is fine" -- exactly the reading `served_window` carries.
    pub stated_window: Option<u64>,
    /// How many times this session has asked. Bounded by
    /// `AgentEngine::STATED_WINDOW_PROBE_ATTEMPTS`, because a stock Ollama
    /// unloads an idle model and then reports nothing at all: turn 1 can be
    /// legitimately unanswerable and turn 2 legitimately answerable, but an
    /// endpoint that will never answer must not be asked once per turn
    /// forever.
    pub stated_window_probes: u32,
    /// B7 — the user's ORIGINAL instruction, captured once and re-folded
    /// verbatim into every compaction.
    ///
    /// The engine's other verbatim carve-out is the TRAILING user message,
    /// which in a tool-driven run is a `tool_result` and carries none of the
    /// user's intent. Without this pin the instruction is handed wholesale to
    /// the summarizer, and measurement showed it gone from the wire and from
    /// the resumable session mirror after the first compaction.
    ///
    /// Captured on the first compaction and never overwritten, so repeated
    /// compactions re-fold the same string instead of nesting summaries.
    pub pinned_instruction: Option<String>,
}

impl CompactState {
    pub fn new() -> Self {
        Self {
            consecutive_failures: 0,
            last_input_tokens: 0,
            last_real_input_tokens: 0,
            served_window: Default::default(),
            stated_window: None,
            stated_window_probes: 0,
            pinned_instruction: None,
        }
    }

    /// Check whether the circuit breaker has tripped.
    pub fn is_circuit_broken(&self, config: &CompactConfig) -> bool {
        self.consecutive_failures >= config.max_failures
    }

    /// Record a successful autocompact — resets the failure counter.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Record a failed autocompact — increments the failure counter.
    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
    }
}

impl Default for CompactState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CompactConfig {
        CompactConfig {
            max_failures: 3,
            ..Default::default()
        }
    }

    #[test]
    fn new_state_not_circuit_broken() {
        let state = CompactState::new();
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.last_input_tokens, 0);
        assert!(!state.is_circuit_broken(&test_config()));
    }

    #[test]
    fn circuit_breaker_trips_at_max_failures() {
        let config = test_config();
        let mut state = CompactState::new();

        state.record_failure();
        assert!(!state.is_circuit_broken(&config));
        state.record_failure();
        assert!(!state.is_circuit_broken(&config));
        state.record_failure();
        assert!(state.is_circuit_broken(&config));
    }

    #[test]
    fn success_resets_failure_counter() {
        let config = test_config();
        let mut state = CompactState::new();

        state.record_failure();
        state.record_failure();
        assert_eq!(state.consecutive_failures, 2);

        state.record_success();
        assert_eq!(state.consecutive_failures, 0);
        assert!(!state.is_circuit_broken(&config));
    }

    #[test]
    fn circuit_breaker_with_max_failures_one() {
        let config = CompactConfig {
            max_failures: 1,
            ..Default::default()
        };
        let mut state = CompactState::new();

        assert!(!state.is_circuit_broken(&config));
        state.record_failure();
        assert!(state.is_circuit_broken(&config));
    }

    #[test]
    fn default_impl_matches_new() {
        let a = CompactState::new();
        let b = CompactState::default();
        assert_eq!(a.consecutive_failures, b.consecutive_failures);
        assert_eq!(a.last_input_tokens, b.last_input_tokens);
        assert_eq!(a.last_real_input_tokens, b.last_real_input_tokens);
    }
}
