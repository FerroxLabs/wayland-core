//! Scenario contract — the user-facing builder + struct that test code
//! constructs and hands to [`crate::runner::run`].
//!
//! Plan reference: §2.1. The shape here matches the public API in the
//! plan so T6/T7/T8 dispatch agents can write scenarios directly against
//! this surface without further refactor.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::assertions::{Assertion, TraceAssertion};
use crate::providers::ProviderChoice;

/// Closure type used by [`Scenario::setup`] / [`Scenario::cleanup`].
/// Extracted as a type alias to satisfy `clippy::type_complexity` and
/// keep the `Scenario` struct readable.
pub type ScenarioHook = Arc<dyn Fn(&Path) -> anyhow::Result<()> + Send + Sync>;

/// One scenario.
///
/// Construct via [`Scenario::new`] + builder methods, then hand to
/// [`crate::runner::run`]. Fields are `pub` so the runner (in this
/// crate) can read them without going through accessors — external
/// callers should use the builder.
pub struct Scenario {
    pub name: &'static str,
    pub category: Category,
    pub turns: Vec<Turn>,
    /// Pre-run hook that scaffolds fixture files inside the scenario's
    /// tempdir (cwd). Boxed for object safety; `Arc` so the runner can
    /// clone the scenario without giving up ownership of the closure.
    pub setup: Option<ScenarioHook>,
    pub cleanup: Option<ScenarioHook>,
    /// Hard wall-time budget for the WHOLE scenario, enforced by the
    /// runner via `tokio::time::timeout` + `kill_on_drop` (M-1).
    pub max_total_time: Duration,
    /// Hard USD ceiling for the whole scenario. The engine's per-scenario
    /// `[budget]` block (seeded via [`crate::tempenv::TempEnv`]) is the
    /// enforcement mechanism; the runner ALSO records observed cost from
    /// the trailing `SessionCost` event and reports OverCost as a
    /// `Failure` if exceeded.
    pub max_total_cost_usd: f64,
    pub provider: ProviderChoice,
    /// M-2: when `true`, missing-API-key turns into FAIL, not SKIP.
    /// `just eval-matrix` sets this for tag-time runs.
    pub strict: bool,
    /// D3: tool-approval posture. `Yolo` (default) spawns with `--yolo` so the
    /// engine auto-approves everything (the persona happy-path). The other
    /// variants spawn WITHOUT `--yolo` (engine `Default` mode → it emits
    /// `ApprovalRequired` per tool) and the runner auto-responds — letting QA
    /// scenarios exercise the real trust/approval gate.
    pub approval: ApprovalPolicy,
}

/// How the runner answers the engine's tool-approval gate (D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPolicy {
    /// Spawn with `--yolo`; the engine auto-approves. No `ApprovalRequired`
    /// events are emitted. This is the default (persona happy-path).
    Yolo,
    /// Spawn without `--yolo`; the runner approves every `ApprovalRequired`.
    ApproveAll,
    /// Spawn without `--yolo`; the runner denies every `ApprovalRequired`.
    DenyAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Code,
    Research,
    Project,
    Multiturn,
    Failure,
    Interactive,
    Drift,
    Coverage,
    Hardening,
}

/// A protocol-command (D2) the runner sends OUTSIDE the per-turn `Message`
/// path. Carried by [`Turn::pre_commands`] and lowered to the json-stream
/// wire form by the runner.
///
/// These mirror `wcore_protocol::commands::ProtocolCommand` variants but are
/// kept as a harness-local enum so the scenario crate doesn't take a
/// build-time dependency on the command shapes (the runner serializes them
/// to JSON by hand, same as it does for `message`/`stop`).
#[derive(Debug, Clone)]
pub enum TurnCommand {
    /// `set_config` — e.g. a model swap. The runner sends
    /// `{"type":"set_config","model":...}`; the engine applies it and (on a
    /// real change) emits `config_changed`.
    SetConfig {
        model: Option<String>,
        thinking: Option<String>,
        effort: Option<String>,
    },
    /// `set_mode` — e.g. switch the approval posture. `mode` is the
    /// snake_case wire tag (`"default"`, `"auto_edit"`, `"force"`). The
    /// engine emits `config_changed` carrying the new `current_mode`.
    SetMode { mode: &'static str },
}

/// One conversational turn inside a [`Scenario`].
///
/// A scenario can have N turns; multi-turn mechanics use the json-stream
/// `StreamEnd` event to demarcate boundaries (per H-2 — text markers
/// don't exist on the wire).
pub struct Turn {
    pub prompt: String,
    pub max_time: Duration,
    pub max_steps: usize,
    pub expected_tools: Vec<&'static str>,
    pub forbidden_tools: Vec<&'static str>,
    pub output_assertions: Vec<Assertion>,
    pub trace_assertions: Vec<TraceAssertion>,
    /// D2: protocol commands the runner sends BEFORE this turn's `Message`
    /// (e.g. `set_config` model swap, `set_mode`). The resulting
    /// `config_changed` / `info` events are captured into
    /// [`crate::runner::ScenarioResult::info_events`] for assertion.
    pub pre_commands: Vec<TurnCommand>,
    /// D2: when `true`, the runner sends a `stop` command mid-turn — right
    /// after the first event of this turn arrives — to exercise cancellation.
    /// The turn is expected to halt (the engine breaks its run future and
    /// emits `stream_end`).
    pub stop_mid_turn: bool,
}

// --- Scenario builder ------------------------------------------------------

impl Scenario {
    /// Start a scenario with default budgets. The defaults are
    /// deliberately conservative; every scenario in the v1 matrix
    /// overrides them via the builder.
    pub fn new(name: &'static str, category: Category) -> Self {
        Self {
            name,
            category,
            turns: Vec::new(),
            setup: None,
            cleanup: None,
            max_total_time: Duration::from_secs(120),
            max_total_cost_usd: 0.10,
            provider: ProviderChoice::Default,
            strict: false,
            approval: ApprovalPolicy::Yolo,
        }
    }

    pub fn turn(mut self, turn: Turn) -> Self {
        self.turns.push(turn);
        self
    }

    pub fn setup<F>(mut self, f: F) -> Self
    where
        F: Fn(&Path) -> anyhow::Result<()> + Send + Sync + 'static,
    {
        self.setup = Some(Arc::new(f));
        self
    }

    pub fn cleanup<F>(mut self, f: F) -> Self
    where
        F: Fn(&Path) -> anyhow::Result<()> + Send + Sync + 'static,
    {
        self.cleanup = Some(Arc::new(f));
        self
    }

    pub fn max_total_time(mut self, d: Duration) -> Self {
        self.max_total_time = d;
        self
    }

    pub fn max_total_cost_usd(mut self, usd: f64) -> Self {
        self.max_total_cost_usd = usd;
        self
    }

    pub fn provider(mut self, p: ProviderChoice) -> Self {
        self.provider = p;
        self
    }

    /// Set the tool-approval posture (D3). Default [`ApprovalPolicy::Yolo`].
    pub fn approval(mut self, policy: ApprovalPolicy) -> Self {
        self.approval = policy;
        self
    }

    pub fn strict(mut self, on: bool) -> Self {
        self.strict = on;
        self
    }

    /// Convenience — async run via the runner with a resolved provider.
    /// Full multi-provider matrix dispatch lands in T5 (the binary).
    pub async fn run_with(
        &self,
        provider: &crate::providers::ProviderConfig,
    ) -> anyhow::Result<crate::runner::ScenarioResult> {
        crate::runner::run(self, provider).await
    }
}

impl Turn {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            max_time: Duration::from_secs(90),
            max_steps: 8,
            expected_tools: Vec::new(),
            forbidden_tools: Vec::new(),
            output_assertions: Vec::new(),
            trace_assertions: Vec::new(),
            pre_commands: Vec::new(),
            stop_mid_turn: false,
        }
    }

    pub fn max_time(mut self, d: Duration) -> Self {
        self.max_time = d;
        self
    }

    pub fn max_steps(mut self, n: usize) -> Self {
        self.max_steps = n;
        self
    }

    pub fn expect_tool(mut self, name: &'static str) -> Self {
        self.expected_tools.push(name);
        self
    }

    pub fn forbid_tool(mut self, name: &'static str) -> Self {
        self.forbidden_tools.push(name);
        self
    }

    pub fn assert(mut self, a: Assertion) -> Self {
        self.output_assertions.push(a);
        self
    }

    pub fn trace(mut self, t: TraceAssertion) -> Self {
        self.trace_assertions.push(t);
        self
    }

    /// D2: queue a protocol command (`set_config` / `set_mode`) to send
    /// BEFORE this turn's user `Message`. Multiple commands send in order.
    pub fn pre_command(mut self, cmd: TurnCommand) -> Self {
        self.pre_commands.push(cmd);
        self
    }

    /// D2: send a `stop` command mid-turn (right after this turn's first
    /// event) to exercise cancellation. The turn is expected to halt.
    pub fn stop_mid_turn(mut self) -> Self {
        self.stop_mid_turn = true;
        self
    }
}
