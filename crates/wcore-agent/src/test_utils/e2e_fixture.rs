//! W7 (closes debt B.8): high-level fixture-builder DSL for end-to-end tests.
//!
//! Inherited from an upstream AionUI todo. The engine's existing test
//! primitives (`ScriptedProvider`, `TestSink`, `AgentBootstrap::build_for_test`,
//! `AgentEngine::run_synthetic_turn`) compose a working end-to-end harness,
//! but each test still pays a ~30–50 line setup tax (config boilerplate,
//! provider-script construction, memory seeding, event-stream parsing).
//!
//! `E2eFixture` is a thin builder that wraps the existing primitives and
//! gives multi-turn integration tests a sub-20-line setup. It does NOT
//! reinvent transport or session logic — every method composes the
//! existing helpers under the hood.
//!
//! # When to use this
//!
//! - Multi-turn or multi-event integration tests against a scripted provider.
//! - Tests that need a real in-memory `MemoryApi` for assertions
//!   (Curator / PUM / skills-lifecycle).
//! - Tests that want to assert on captured `ProtocolEvent`s without
//!   hand-parsing JSON.
//!
//! # When NOT to use this
//!
//! - Pure unit tests on a single module (use `#[cfg(test)]` inline).
//! - Type-only tests (`assert_impl_all!` and friends) — no engine needed.
//! - Fixture-replay tests (those use `wcore_agent::vcr`, a different harness).
//! - Tests that need real plugin loading — the underlying `build_for_test`
//!   does not run the plugin loader (it is a synchronous fast-path). If a
//!   future test needs plugin registration, extend `build_for_test` first
//!   and add a `.with_plugin()` method here.
//!
//! # Canonical usage
//!
//! See `crates/wcore-agent/tests/w9_direct_invocation_test.rs` (tagged
//! `// Example: e2e_fixture`). That test seeds five overlapping staged
//! procedures, drives one synthetic turn with a tool call, and asserts
//! that Curator + UserModelInferencer fire at session end. The setup is
//! ~12 lines of fixture code vs ~40 lines hand-rolled.
//!
//! ```ignore
//! use wcore_agent::test_utils::e2e_fixture::E2eFixture;
//!
//! let mut fx = E2eFixture::new()
//!     .with_provider_script(one_tool_call_script())
//!     .with_skills_lifecycle(true)
//!     .with_max_turns(1)
//!     .with_in_memory_backend()
//!     .build()
//!     .await;
//!
//! fx.seed_overlapping_staged_procedures("auto-w3-overlap-", 5).await;
//! let out = fx.send("invoke curator + pum at session end").await.unwrap();
//! assert!(out.events.iter_kinds().any(|k| k == "stream_start"));
//! ```

use std::sync::Arc;

use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_memory::api::MemoryApi;
use wcore_memory::v2_types::{
    AccessToken, Procedure, ProcedureId, ProcedureStatus, Tier, UserModel,
};
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use crate::bootstrap::AgentBootstrap;
use crate::engine::AgentEngine;
use crate::test_utils::{CapturedEvent, TestSinkHandle};

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for an `E2eFixture`. Construct with `E2eFixture::new()`, chain
/// `.with_*` methods, then call `.build().await` to materialise a
/// `Fixture`.
pub struct E2eFixtureBuilder {
    config: Config,
    script: Vec<LlmEvent>,
    install_in_memory_backend: bool,
}

impl Default for E2eFixtureBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl E2eFixtureBuilder {
    /// Start a fresh builder with a minimal OpenAI-shaped config, an
    /// empty provider script (callers MUST supply one before `.build()`
    /// or the engine will hang waiting for events), and no memory swap.
    pub fn new() -> Self {
        Self {
            config: minimal_config(),
            script: Vec::new(),
            install_in_memory_backend: false,
        }
    }

    /// Supply the full provider event stream this fixture replays.
    /// Replaces any previously-set script. Composes via
    /// `ScriptedProvider` under the hood.
    pub fn with_provider_script(mut self, events: Vec<LlmEvent>) -> Self {
        self.script = events;
        self
    }

    /// Convenience: single-text-turn script (one `TextDelta` followed by
    /// `Done { stop_reason = EndTurn }`). Matches
    /// `ScriptedProvider::single_text_turn`.
    pub fn with_single_text_turn(self, text: impl Into<String>) -> Self {
        self.with_provider_script(vec![
            LlmEvent::TextDelta(text.into()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::Stop,
                usage: TokenUsage::default(),
            },
        ])
    }

    /// Toggle the `observability.skills_lifecycle` config gate. When
    /// true, `fire_on_session_end` invokes Curator + UserModelInferencer
    /// directly (W3 invariant).
    pub fn with_skills_lifecycle(mut self, on: bool) -> Self {
        self.config.observability.skills_lifecycle = on;
        self
    }

    /// Override `max_turns` (default: 2). Set to 1 to drive the engine
    /// through `fire_on_session_end` after a single turn.
    pub fn with_max_turns(mut self, n: usize) -> Self {
        self.config.max_turns = Some(n);
        self
    }

    /// Replace the engine's default `NullMemory` with a real in-memory
    /// `Memory` (via `wcore_memory::open_for_test`). Required for tests
    /// that need to assert on Curator / PUM writes. Without this, the
    /// fixture inherits `NullMemory` from `build_for_test`.
    pub fn with_in_memory_backend(mut self) -> Self {
        self.install_in_memory_backend = true;
        self
    }

    /// Mutate the underlying `Config` directly. Escape hatch for tests
    /// that need to flip a field this builder hasn't grown a method
    /// for yet. Prefer adding a named `.with_*` method when a field
    /// becomes load-bearing across multiple tests.
    pub fn with_config_mut(mut self, f: impl FnOnce(&mut Config)) -> Self {
        f(&mut self.config);
        self
    }

    /// Materialise the fixture. Constructs an `AgentEngine` via
    /// `AgentBootstrap::build_for_test`, optionally swaps in a real
    /// in-memory `MemoryApi`, and returns a `Fixture` ready to `.send()`.
    pub async fn build(self) -> Fixture {
        // Composes the existing build_for_test under the hood — does
        // NOT reimplement engine construction.
        let (mut engine, sink_handle) = AgentBootstrap::build_for_test(self.config, self.script);

        let memory: Option<Arc<dyn MemoryApi>> = if self.install_in_memory_backend {
            // `open_for_test` is purely in-memory and ignores the
            // path argument; we pass `std::env::temp_dir()` for API
            // symmetry without taking a `tempfile` dep on the
            // production crate.
            let mem: Arc<dyn MemoryApi> = Arc::new(
                wcore_memory::open_for_test(&std::env::temp_dir())
                    .await
                    .expect("open_for_test in-memory backend"),
            );
            engine.set_memory_api(mem.clone());
            Some(mem)
        } else {
            None
        };

        Fixture {
            engine,
            sink_handle,
            memory,
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// Materialised fixture: an `AgentEngine` plus typed handles for sending
/// turns, inspecting captured events, and seeding memory.
pub struct Fixture {
    engine: AgentEngine,
    sink_handle: TestSinkHandle,
    memory: Option<Arc<dyn MemoryApi>>,
}

impl Fixture {
    /// Drive one synthetic turn. Composes `AgentEngine::run_synthetic_turn`
    /// under the hood; returns the events captured during that turn
    /// (NOT the cumulative buffer — see `all_events()` for that).
    ///
    /// **Per-turn slice rationale:** integration tests typically assert
    /// "this turn emitted X, Y, Z" not "the whole session contains X, Y,
    /// Z". Slicing on the size-at-start prefix gives the per-turn view
    /// without forcing every test to bookkeep indices manually.
    pub async fn send(&mut self, input: &str) -> Result<TurnOutput, crate::engine::AgentError> {
        let before = self.sink_handle.snapshot().len();
        let out = self.engine.run_synthetic_turn(input).await?;
        let after_all = self.sink_handle.snapshot();
        let turn_events = if after_all.len() > before {
            after_all[before..].to_vec()
        } else {
            Vec::new()
        };
        Ok(TurnOutput {
            final_text: out.final_text,
            turns: out.turns,
            events: EventCollector::new(turn_events),
        })
    }

    /// Snapshot of every event the sink has captured so far across all
    /// turns. Use this for end-of-session assertions; use the
    /// `TurnOutput.events` returned by `.send()` for per-turn assertions.
    pub fn all_events(&self) -> EventCollector {
        EventCollector::new(self.sink_handle.snapshot())
    }

    /// Borrow the underlying `TestSinkHandle` for direct access (e.g.
    /// passing into another helper that wants its own snapshot).
    pub fn sink_handle(&self) -> &TestSinkHandle {
        &self.sink_handle
    }

    /// Borrow the in-memory `MemoryApi` if one was installed via
    /// `.with_in_memory_backend()`. Returns `None` for fixtures using
    /// the default `NullMemory`.
    pub fn memory(&self) -> Option<&Arc<dyn MemoryApi>> {
        self.memory.as_ref()
    }

    /// Mutable access to the underlying engine. Escape hatch for the
    /// occasional test that needs to flip an engine knob the fixture
    /// hasn't grown a method for. Prefer a named method when a use
    /// becomes common.
    pub fn engine_mut(&mut self) -> &mut AgentEngine {
        &mut self.engine
    }

    /// Read the current `user_model` from the installed in-memory
    /// backend. Panics if no in-memory backend was installed — call
    /// `.with_in_memory_backend()` on the builder first.
    pub async fn user_model(&self) -> UserModel {
        self.require_memory()
            .user_model(AccessToken::System)
            .await
            .expect("read user_model from in-memory backend")
    }

    /// List procedures at a tier from the installed in-memory backend.
    /// Panics if no in-memory backend was installed.
    pub async fn list_procedures(&self, tier: Tier) -> Vec<Procedure> {
        self.require_memory()
            .list_procedures(tier, AccessToken::System)
            .await
            .expect("list_procedures from in-memory backend")
    }

    /// Seed N overlapping `Staged` procedures with the given name prefix.
    /// Each name is `{prefix}{i}` for `i` in `0..count`. ID is derived
    /// from the name via UUIDv5 (deterministic across runs).
    ///
    /// Used by W3-style tests to give Curator dedup work. Panics if no
    /// in-memory backend was installed.
    pub async fn seed_overlapping_staged_procedures(&self, prefix: &str, count: usize) {
        let mem = self.require_memory();
        for i in 0..count {
            let name = format!("{prefix}{i}");
            let id = ProcedureId(uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                name.as_bytes(),
            ));
            let p = Procedure {
                id,
                tier: Tier::Project,
                ts: 0,
                name,
                description: "Auto-drafted from grep read edit bash".into(),
                artifact: "---\n---\n".into(),
                status: ProcedureStatus::Staged,
                created_by: "e2e_fixture".into(),
                thompson_alpha: 1.0,
                thompson_beta: 1.0,
                use_count: 0,
                success_count: 0,
            };
            mem.upsert_procedure(p, AccessToken::System)
                .await
                .expect("seed staged procedure");
        }
    }

    fn require_memory(&self) -> &Arc<dyn MemoryApi> {
        self.memory.as_ref().expect(
            "this fixture has no in-memory backend installed; call \
             .with_in_memory_backend() on the builder first",
        )
    }
}

// ---------------------------------------------------------------------------
// TurnOutput + EventCollector
// ---------------------------------------------------------------------------

/// One synthetic turn's worth of output.
#[derive(Debug)]
pub struct TurnOutput {
    /// Final text the engine emitted on this turn (the `engine.run()` return).
    pub final_text: String,
    /// Turn count returned by `engine.run()` (always ≥ 1 on success).
    pub turns: usize,
    /// Events captured *during this turn* (not the cumulative buffer).
    pub events: EventCollector,
}

/// Wrapper around a `Vec<CapturedEvent>` (serde-JSON `ProtocolEvent` form)
/// with typed accessors so tests don't hand-roll `event["type"].as_str()`.
#[derive(Debug, Clone, Default)]
pub struct EventCollector {
    events: Vec<CapturedEvent>,
}

impl EventCollector {
    pub fn new(events: Vec<CapturedEvent>) -> Self {
        Self { events }
    }

    /// All captured events as their raw serde-JSON wire form.
    pub fn raw(&self) -> &[CapturedEvent] {
        &self.events
    }

    /// How many events were captured.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// True iff no events were captured.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Iterate over events in order.
    pub fn iter(&self) -> std::slice::Iter<'_, CapturedEvent> {
        self.events.iter()
    }

    /// Iterate over the `type` field of each event. Skips events whose
    /// `type` is missing or non-string (which would indicate a serde
    /// regression in `ProtocolEvent`, not a normal runtime state).
    pub fn iter_kinds(&self) -> impl Iterator<Item = &str> {
        self.events.iter().filter_map(|e| e["type"].as_str())
    }

    /// True iff at least one captured event has the given `type` tag.
    /// Use snake_case (matches the `#[serde(rename_all = "snake_case")]`
    /// on `ProtocolEvent`): `"stream_start"`, `"text_delta"`,
    /// `"tool_request"`, `"stream_end"`, etc.
    pub fn has_event(&self, kind: &str) -> bool {
        self.iter_kinds().any(|k| k == kind)
    }

    /// Count events of a given `type` tag.
    pub fn count_events(&self, kind: &str) -> usize {
        self.iter_kinds().filter(|k| *k == kind).count()
    }

    /// Concatenation of every `text_delta` event's `text` field, in
    /// emission order. Matches what a host would render to the user.
    pub fn text(&self) -> String {
        self.events
            .iter()
            .filter(|e| e["type"].as_str() == Some("text_delta"))
            .filter_map(|e| e["text"].as_str())
            .collect::<String>()
    }

    /// First event matching the given `type` tag, if any.
    pub fn find(&self, kind: &str) -> Option<&CapturedEvent> {
        self.events
            .iter()
            .find(|e| e["type"].as_str() == Some(kind))
    }
}

/// Canonical entry-point alias. `E2eFixture::new()` returns the builder
/// so test setup reads as `E2eFixture::new().with_*().build().await`.
///
/// ```ignore
/// let fx = E2eFixture::new().with_single_text_turn("hi").build().await;
/// ```
pub type E2eFixture = E2eFixtureBuilder;

// ---------------------------------------------------------------------------
// Minimal config helper
// ---------------------------------------------------------------------------

/// Minimal OpenAI-shaped `Config` for fixture tests. Mirrors the
/// hand-rolled `minimal_config()` that appears in every existing e2e
/// test (`w7_pre0_test_driver`, `w9_direct_invocation_test`, etc.) —
/// extracting it here closes the boilerplate budget.
fn minimal_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(2),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Inline tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn single_text_turn_emits_stream_start_and_text_delta() {
        let mut fx = E2eFixture::new()
            .with_single_text_turn("hello fixture")
            .build()
            .await;

        let out = fx.send("anything").await.unwrap();
        assert!(out.turns >= 1);
        assert!(out.events.has_event("stream_start"));
        assert!(out.events.has_event("text_delta"));
        assert_eq!(out.events.text(), "hello fixture");
        assert!(out.events.find("text_delta").is_some());
    }

    #[tokio::test]
    async fn in_memory_backend_exposes_user_model_and_procedures() {
        let fx = E2eFixture::new()
            .with_single_text_turn("noop")
            .with_in_memory_backend()
            .build()
            .await;

        // Memory is installed; user_model and list_procedures are
        // callable. Fresh backend → both should be empty.
        let um = fx.user_model().await;
        assert!(um.entries.is_empty());
        let procs = fx.list_procedures(Tier::Project).await;
        assert!(procs.is_empty());
    }

    #[tokio::test]
    async fn seed_overlapping_staged_writes_n_procedures() {
        let fx = E2eFixture::new()
            .with_single_text_turn("noop")
            .with_in_memory_backend()
            .build()
            .await;
        fx.seed_overlapping_staged_procedures("test-seed-", 3).await;
        let procs = fx.list_procedures(Tier::Project).await;
        assert_eq!(procs.len(), 3);
        assert!(
            procs
                .iter()
                .all(|p| matches!(p.status, ProcedureStatus::Staged))
        );
    }

    #[tokio::test]
    async fn per_turn_event_slice_is_disjoint_from_prior_turns() {
        let mut fx = E2eFixture::new()
            .with_single_text_turn("turn-1-only")
            .with_max_turns(5)
            .build()
            .await;

        let t1 = fx.send("first").await.unwrap();
        let t1_len = t1.events.len();
        assert!(!t1.events.is_empty());

        // Second send replays the same script; the per-turn slice
        // returned MUST NOT include events from turn 1.
        let t2 = fx.send("second").await.unwrap();
        assert!(!t2.events.is_empty());
        // The cumulative all_events() view should be the sum, not the
        // size of either turn alone.
        let cumulative = fx.all_events().len();
        assert_eq!(cumulative, t1_len + t2.events.len());
    }
}
