//! The executor abstraction, and the two IN-PROCESS drivers behind it.
//!
//! One abstraction — [`CorpusExecutor`] — expresses a single job: *given a
//! corpus entry, drive the hostile request through this surface and report
//! what the child actually obtained.* Four drivers implement it. Two live
//! here (standalone in-process, host-protocol in-process); the two that spawn
//! the real `wayland-core` binary live in `live.rs`.
//!
//! The drivers here reach the REAL seams, not stand-ins:
//!
//! * The budget family runs through `wcore_budget::ExecutionBudgetView::sub_budget`
//!   on the standalone side — the actual child-spawn budget primitive the census
//!   named — and through `BudgetAuthorityCoordinator::begin_active_turn` on the
//!   host-protocol side, which is the session/turn-level wrapper the protocol
//!   front-end drives and the only other parameterised entry into that seam.
//! * The spawn family runs through the real `AgentSpawner`: `Spawner::spawn_fork`
//!   is the production Delegate path, and `SpawnTool` is the production breadth
//!   path.
//! * Filesystem and secret run through the real `SandboxedFs` / `SecretDenyFs` /
//!   `WorkspacePolicy` stack that `build_tool_registry` installs into every
//!   child registry.
//! * Egress runs through the real `AgentEgressPolicy` that `AgentBootstrap`
//!   installs via `policy_from_config`.
//! * Approval runs through the real `EffectiveExecutionPolicy::with_requested_approvals`
//!   resolver.
//!
//! Nothing here asserts an error string, error kind, error variant or numeric
//! status. Every verdict is derived from what the child obtained.
//!
//! There is no platform gate in this file. A `cfg` gate here would hide a
//! surface from Windows and recreate the blindness class that let a dead
//! rename primitive survive for months.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;

use wcore_agent::budget_authority::{BudgetAuthorityCoordinator, BudgetAuthoritySeed};
use wcore_agent::egress::policy_from_config;
use wcore_agent::session::SessionManager;
use wcore_agent::session_journal::BudgetWallClockAuthority;
use wcore_agent::spawn_tool::SpawnTool;
use wcore_agent::spawner::AgentSpawner;
use wcore_budget::execution::{ExecutionBudget, ExecutionBudgetView};
use wcore_budget::tracker::BudgetCap;
use wcore_config::config::Config;
use wcore_egress::EgressDecision;
use wcore_providers::LlmProvider;
use wcore_tools::Tool;
use wcore_tools::delegate::DelegateTool;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;
use wcore_types::execution_policy::{ApprovalPolicy, BaselineExecutionPolicy, PolicySource};
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};
use wcore_types::spawner::{ForkOverrides, Spawner, SubAgentConfig};

use crate::cases::{CorpusEntry, Dimension};

// ===========================================================================
// The executor abstraction and its vocabulary
// ===========================================================================

/// The two surfaces Success Criterion 3 requires to prove EQUIVALENT
/// enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    Standalone,
    HostProtocol,
}

impl Surface {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::HostProtocol => "host-protocol",
        }
    }
}

/// The axis that catches a lying suite. An in-process REFUSED against a live
/// ALLOWED means the tests were vouching for a restriction the shipped product
/// does not enforce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    InProcess,
    Live,
}

impl Mode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::InProcess => "in-process",
            Self::Live => "live",
        }
    }
}

/// What the child obtained, expressed in a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The child did not obtain the widened authority or resource.
    Refused,
    /// The child DID obtain it. This is a red.
    Allowed,
    /// No child-request channel exists on this surface, asserted structurally.
    NoChannel,
    /// This combination is DECLARED unavailable on this platform. Never a
    /// silent skip, and never substituted with a different combination.
    Unavailable,
    /// This surface genuinely cannot express this entry. Counted and reported,
    /// never skipped.
    NotExpressible,
}

impl Outcome {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Refused => "REFUSED",
            Self::Allowed => "ALLOWED",
            Self::NoChannel => "NO-CHANNEL",
            Self::Unavailable => "UNAVAILABLE",
            Self::NotExpressible => "NOT-EXPRESSIBLE",
        }
    }

    /// Whether this outcome is a verdict about enforcement. Only decisive
    /// outcomes participate in the equivalence assertions: comparing a verdict
    /// against "this combination did not run" would manufacture a divergence
    /// that is really a coverage gap, and coverage gaps are reported by the
    /// completeness invariant instead.
    pub const fn is_decisive(self) -> bool {
        matches!(self, Self::Refused | Self::Allowed | Self::NoChannel)
    }
}

/// The three things a live outcome must record beyond its platform. A live row
/// missing any of them is not evidence.
#[derive(Debug, Clone)]
pub struct LiveEvidence {
    /// The exact invocation: binary, flags and input.
    pub invocation: String,
    /// The mode the run PROVED it landed in, read back from the process rather
    /// than assumed from the flags. A piped subprocess silently falls through
    /// from the TUI to the line REPL, so a run intended to exercise one surface
    /// can quietly exercise another and report a verdict for it.
    pub asserted_mode: String,
    /// The observation that distinguished an enforced restriction from a
    /// widened one.
    pub observable: String,
}

/// A driver's answer before it is stamped with its surface and mode.
#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub outcome: Outcome,
    pub obtained: String,
    pub detail: String,
    pub live: Option<LiveEvidence>,
    /// `Some(description)` when a NO-CHANNEL canary this probe carries has
    /// TRIPPED — a request channel the dimension's protection rests on has
    /// appeared. Separate from `outcome` on purpose: the outcome is a verdict
    /// about what the child obtained on this run, and a channel appearing is a
    /// verdict about the WORLD. Collapsing the two would either hide a new
    /// channel behind a still-refusing seam or restate a refusal as a widening.
    /// Asserted by `assert_no_channel_canaries_stayed_intact`.
    pub canary_trip: Option<String>,
}

impl ProbeResult {
    pub fn new(outcome: Outcome, obtained: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            outcome,
            obtained: obtained.into(),
            detail: detail.into(),
            live: None,
            canary_trip: None,
        }
    }

    pub fn with_live(mut self, live: LiveEvidence) -> Self {
        self.live = Some(live);
        self
    }

    pub fn with_canary(mut self, canary: &CanaryState) -> Self {
        self.canary_trip = canary.tripped().map(ToOwned::to_owned);
        self
    }
}

/// The state of a structural NO-CHANNEL canary.
///
/// FINDING F-V4 (Phase 21 verification): the budget canary used to return a
/// bare `String` that was interpolated into `detail` and consumed as display
/// text only. The literal `"NO-CHANNEL CANARY TRIPPED"` appeared in exactly one
/// place workspace-wide — its own definition. Nothing asserted on it, so it
/// could trip silently forever. A canary nothing asserts on is a comment.
#[derive(Debug, Clone)]
pub enum CanaryState {
    Intact(String),
    Tripped(String),
}

impl CanaryState {
    pub fn note(&self) -> &str {
        match self {
            Self::Intact(note) | Self::Tripped(note) => note,
        }
    }

    pub fn tripped(&self) -> Option<&str> {
        match self {
            Self::Tripped(note) => Some(note),
            Self::Intact(_) => None,
        }
    }
}

/// One (entry, surface, mode) execution.
#[derive(Debug, Clone)]
pub struct Execution {
    pub dimension: Dimension,
    pub surface: Surface,
    pub mode: Mode,
    pub outcome: Outcome,
    /// What the child actually obtained. The invariant evidence.
    pub obtained: String,
    pub detail: String,
    pub live: Option<LiveEvidence>,
    pub canary_trip: Option<String>,
}

/// The one job every driver does.
pub trait CorpusExecutor {
    fn surface(&self) -> Surface;
    fn mode(&self) -> Mode;
    fn probe(&self, entry: &CorpusEntry) -> ProbeResult;

    fn execute(&self, entry: &CorpusEntry) -> Execution {
        let probe = self.probe(entry);
        Execution {
            dimension: entry.dimension,
            surface: self.surface(),
            mode: self.mode(),
            outcome: probe.outcome,
            obtained: probe.obtained,
            detail: probe.detail,
            live: probe.live,
            canary_trip: probe.canary_trip,
        }
    }
}

// ===========================================================================
// Shared fixtures
// ===========================================================================

/// The workspace root, derived from this crate's manifest directory.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/wcore-cli has a workspace grandparent")
        .to_path_buf()
}

/// Files under `crates/*/src` that mention `needle`, relative to the workspace
/// root and slash-normalised so a Windows run reports the same set as a Linux
/// one. Used by the structural NO-CHANNEL canaries, whose entire value is that
/// they fail the day a request channel appears.
pub fn source_files_mentioning(needle: &str) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    let root = workspace_root();
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("crates")) {
        for entry in entries.flatten() {
            let src = entry.path().join("src");
            if src.is_dir() {
                walk(&src, &mut files);
            }
        }
    }
    files.sort();

    let mut hits = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.contains(needle) {
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            hits.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    hits
}

/// PRODUCTION sites under `crates/*/src` that mention `needle`, excluding the
/// crate that defines the thing being looked for and excluding every test
/// region. This is the shape every structural canary needs, and getting it
/// wrong in either direction destroys the canary's value: too loose and it
/// reports test fixtures as production channels (a manufactured red), too tight
/// and it never fires on the channel it exists to catch.
///
/// Three exclusions, each of which the census applied by hand:
///
/// * `defining_crate` is where the mechanism lives; its own definition, its
///   re-export and its own tests are not third-party call sites.
/// * A file named `tests.rs`, or one under a `tests/` directory, is a whole-file
///   test module declared `#[cfg(test)] mod tests;` by its parent, so it carries
///   no inner `#[cfg(test)]` line to key on.
/// * Within every other file, an occurrence at or after the first `#[cfg(test)]`
///   is inside a test module.
pub fn production_sites_mentioning(needle: &str, defining_crate: &str) -> Vec<String> {
    let root = workspace_root();
    let mut sites = Vec::new();
    for file in source_files_mentioning(needle) {
        if file.starts_with(defining_crate)
            || file.ends_with("/tests.rs")
            || file.contains("/tests/")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(&file)) else {
            continue;
        };
        let test_boundary = text
            .lines()
            .position(|line| line.trim_start().starts_with("#[cfg(test)]"))
            .unwrap_or(usize::MAX);
        if text
            .lines()
            .enumerate()
            .any(|(index, line)| index < test_boundary && line.contains(needle))
        {
            sites.push(file);
        }
    }
    sites
}

/// A hermetic parent: a tempdir workspace, a session directory, and an
/// `AgentSpawner` whose provider replays `script` for every child turn.
pub struct ParentFixture {
    /// Held so the tempdir outlives every probe that reads from it.
    pub _home: TempDir,
    /// The session state root, held for the same reason. Deliberately NOT
    /// inside `root` — see `parent_fixture`.
    pub _session_home: TempDir,
    pub root: PathBuf,
    pub spawner: Arc<AgentSpawner>,
    /// `Some(reason)` when the durable session could not be bound. A driver
    /// that needs a launch reports NOT-EXPRESSIBLE rather than reporting a
    /// refusal it did not actually observe.
    pub bind_failure: Option<String>,
    /// The fixture's own endpoint, counted. A probe reads this to tell "the
    /// child ran and was refused" apart from "no child ever ran".
    pub provider: Arc<CountingProvider>,
}

/// Make `root` a real git repository with one commit.
///
/// Required because a child that requests a MUTATING toolset resolves
/// `RequestedChildWorkspace::IsolatedMutation`, and the isolated workspace is a
/// git worktree of the parent: `resolve_durable_launch` calls
/// `WorktreeManager::new_with_workspace_root`, which needs `pinned_head` and
/// `git_common_dir`. Without a repo the child dies in workspace preparation and
/// the probe reads an absent effect — the vacuity this file exists to close.
///
/// Argv mode throughout: no shell string, no interpolation. Identity is passed
/// per-invocation with `-c` so no global git config is read or written.
fn init_git_repo(root: &Path) -> Result<(), String> {
    let run = |args: &[&str]| -> Result<(), String> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|error| format!("git is not available on this host: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "git {:?} did not succeed: {}",
                args,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    };
    run(&["init", "--initial-branch=corpus"])?;
    std::fs::write(root.join("README.corpus"), b"corpus fixture repository")
        .map_err(|error| error.to_string())?;
    // The binary writes its own per-workspace state under `.wayland-core/`, and
    // an isolated-mutation dispatch refuses on a dirty checkout. Ignoring that
    // directory is what keeps the repository clean enough for the child to be
    // created at all; without it every mutating child dies before existing and
    // the probe reads an absent effect.
    std::fs::write(root.join(".gitignore"), b".wayland-core/\n")
        .map_err(|error| error.to_string())?;
    run(&["add", "README.corpus", ".gitignore"])?;
    run(&[
        "-c",
        "user.email=corpus@example.invalid",
        "-c",
        "user.name=corpus",
        "commit",
        "-m",
        "corpus fixture",
    ])
}

pub fn parent_fixture(session_tag: &str, script: Vec<LlmEvent>) -> ParentFixture {
    let home = TempDir::new().expect("tempdir");
    let root = std::fs::canonicalize(home.path()).expect("canonical workspace root");
    // The session state root lives OUTSIDE the parent workspace on purpose. The
    // isolated-mutation checkout root is derived as
    // `<session.directory>/delegated-workspaces/checkouts`, and
    // `WorktreeManager::new_with_workspace_root` refuses when that root's parent
    // overlaps the repository. With the session directory nested inside the
    // workspace — as this fixture had it — the overlap is unconditional, so
    // EVERY mutating child died in workspace preparation and the tool probe
    // recorded a refusal it never observed.
    let session_home = TempDir::new().expect("session tempdir");
    let sessions = std::fs::canonicalize(session_home.path()).expect("canonical session root");
    let repo_failure = init_git_repo(&root).err();

    let mut config = Config::default();
    config.session.directory = sessions.to_string_lossy().into_owned();
    // A FOURTH INSTANCE OF THE VACUITY FAMILY, found while closing F-V2.
    //
    // `Config::default()` carries an EMPTY model. `resolve_durable_launch`
    // (spawner.rs:1465) fails closed on an empty resolved model, so every child
    // launched from this fixture died before existing and the probes recorded
    // REFUSED from a child that never ran. The evidence is verbatim in the
    // shipped ledgers: `child text: durable child execution evidence mismatch:
    // resolved model` on the standalone in-process tool row, and `0 children
    // were reported` on the fan-out row — both recorded REFUSED at both the
    // 21-02 and 21-03 SHAs on both platforms. Naming a model here is what gives
    // those two probes an actor; the anti-vacuity gate below is what stops the
    // absence of one from ever being read as a refusal again.
    config.provider_label = "anthropic".to_owned();
    config.model = "corpus-model".to_owned();

    let counted = Arc::new(CountingProvider::new(script));
    let provider: Arc<dyn LlmProvider> = Arc::clone(&counted) as Arc<dyn LlmProvider>;
    let spawner = AgentSpawner::new(provider, config)
        .with_parent_workspace(&root)
        .expect("bind parent workspace");

    let manager = SessionManager::new(sessions, 10);
    let bind_failure = match manager.create_for_run(
        "anthropic",
        "corpus-model",
        &root.to_string_lossy(),
        Some(session_tag),
    ) {
        Ok(active) => spawner
            .bind_durable_session(active.journal, &active.session.id)
            .err()
            .map(|error| error.to_string()),
        Err(error) => Some(error.to_string()),
    };

    ParentFixture {
        _home: home,
        _session_home: session_home,
        root,
        spawner: Arc::new(spawner),
        bind_failure: bind_failure.or(repo_failure),
        provider: counted,
    }
}

/// The in-process anti-vacuity gate, the exact sibling of the live one.
///
/// Every in-process spawn probe reads a negative — no probe file, no child
/// name in the batch result. A negative is only evidence that a restriction
/// held if a child ACTUALLY RAN, and "ran" means took its own provider turn
/// against the fixture's endpoint. Returns `Some(probe)` when it did not.
fn withhold_if_no_child_ran(
    fixture: &ParentFixture,
    before: usize,
    what: &str,
    child_text: &str,
) -> Option<ProbeResult> {
    let turns = fixture.provider.calls().saturating_sub(before);
    (turns == 0).then(|| {
        ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — no child took a provider turn in this run",
            format!(
                "{what} returned without any child reaching the fixture's own endpoint, so an \
                 absent effect would mean an attempt that never happened rather than a refusal; \
                 child text: {}",
                truncate(child_text)
            ),
        )
    })
}

/// A one-turn child script that calls `tool` with `input` and then stops.
pub fn tool_call_script(tool: &str, input: serde_json::Value) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: "corpus-tool-call".to_owned(),
            name: tool.to_owned(),
            input,
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        },
    ]
}

pub fn done_script() -> Vec<LlmEvent> {
    vec![LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        finish_reason: FinishReason::Stop,
        usage: TokenUsage::default(),
    }]
}

pub fn child_config(name: &str, prompt: &str) -> SubAgentConfig {
    SubAgentConfig {
        name: name.to_owned(),
        prompt: prompt.to_owned(),
        max_turns: 2,
        max_tokens: 256,
        system_prompt: None,
        provider: None,
        model: None,
        temperature: None,
    }
}

pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

fn truncate(text: &str) -> String {
    text.chars()
        .take(160)
        .collect::<String>()
        .replace('\n', " ")
}

// ===========================================================================
// Budget probes — shared by both in-process surfaces
// ===========================================================================

/// The parent's envelope: deliberately tight on every cap the corpus attacks.
pub fn tight_parent_budget() -> ExecutionBudget {
    ExecutionBudget {
        max_wall_time: Some(Duration::from_millis(40)),
        max_tool_runtime: None,
        max_processes: None,
        max_agent_depth: Some(1),
        max_tokens_in: Some(100),
        max_tokens_out: Some(100),
        max_cost_usd: Some(0.01),
    }
}

/// The hostile child request: every cap wider than the parent's by orders of
/// magnitude.
pub fn wide_child_budget() -> ExecutionBudget {
    ExecutionBudget {
        max_wall_time: Some(Duration::from_secs(86_400)),
        max_tool_runtime: None,
        max_processes: None,
        max_agent_depth: Some(1_000),
        max_tokens_in: Some(10_000_000),
        max_tokens_out: Some(10_000_000),
        max_cost_usd: Some(1_000_000.0),
    }
}

/// Drive one budget dimension's widening attempt against `child`, a view the
/// caller obtained by handing a WIDER budget to the seam under test.
///
/// The verdict is on what the child obtained: if the child can consume past the
/// parent's remaining envelope without the envelope binding, the child obtained
/// resource beyond its parent's.
pub fn budget_probe(dimension: Dimension, child: &ExecutionBudgetView) -> ProbeResult {
    match dimension {
        Dimension::Depth => {
            // The parent permits depth 1. Entering twice is past the parent's
            // envelope even though the child's own cap is 1000.
            let first = child.enter_agent();
            let second = child.enter_agent();
            let bound = child.first_exceeded_reason().is_some();
            drop(second);
            drop(first);
            verdict(
                bound,
                "two nested agent entries against a parent that permits one",
            )
        }
        Dimension::Time => {
            // The child's own clock restarts inside `sub_budget`, which is
            // exactly why the ancestor leg matters. The invariant is that the
            // envelope the child sees is the ancestor minimum, never its own
            // wider cap.
            let remaining = child.remaining_wall_time();
            let bound = remaining.is_none_or(|r| r <= Duration::from_millis(40));
            verdict(
                bound,
                &format!(
                    "child-visible remaining wall time is {remaining:?} against a parent cap of 40ms"
                ),
            )
        }
        Dimension::Token => {
            child.record_tokens(1_000, 1_000);
            let bound = child.first_exceeded_reason().is_some();
            verdict(
                bound,
                "1000 in / 1000 out recorded against a parent allowance of 100 / 100",
            )
        }
        Dimension::Cost => {
            child.record_cost(5.0);
            let bound_after_child = child.first_exceeded_reason().is_some();
            // A grandchild must not obtain a reset of the accrual.
            let grandchild = child.sub_budget(Some(wide_child_budget()));
            let bound_at_grandchild = grandchild.first_exceeded_reason().is_some();
            verdict(
                bound_after_child && bound_at_grandchild,
                "$5.00 accrued against a parent allowance of $0.01, then a fresh grandchild \
                 sub-budget opened over the same accrual",
            )
        }
        other => ProbeResult::new(
            Outcome::NotExpressible,
            "no resource was requested",
            format!(
                "{} is not a budget-rollup dimension; budget_probe was called for it in error",
                other.census_name()
            ),
        ),
    }
}

fn verdict(child_stayed_bound: bool, evidence: &str) -> ProbeResult {
    if child_stayed_bound {
        ProbeResult::new(
            Outcome::Refused,
            "nothing beyond the parent's remaining envelope",
            evidence.to_owned(),
        )
    } else {
        ProbeResult::new(
            Outcome::Allowed,
            "resource beyond the parent's remaining envelope",
            evidence.to_owned(),
        )
    }
}

// ===========================================================================
// Structural NO-CHANNEL canaries — shared by both in-process surfaces
// ===========================================================================

/// Recursively look for any object key that would let a caller name a provider.
/// Deliberately broad: a canary that checked one spelling would miss the
/// channel it exists to catch. `description` is excluded because schema prose
/// routinely mentions providers without offering one.
fn schema_offers_provider(schema: &serde_json::Value) -> bool {
    match schema {
        serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
            let lower = key.to_ascii_lowercase();
            (lower.contains("provider") && lower != "description") || schema_offers_provider(value)
        }),
        serde_json::Value::Array(items) => items.iter().any(schema_offers_provider),
        _ => false,
    }
}

/// The provider canary. `SubAgentConfig.provider` is the only field through
/// which a child could name a provider, and no shipped tool schema exposes it.
/// This reads the REAL schemas the model is shown and fails the day either
/// grows a provider-naming property.
pub fn provider_no_channel_canary(spawner: Arc<AgentSpawner>) -> ProbeResult {
    let delegate = DelegateTool::unwired();
    let spawn = SpawnTool::new(spawner);
    let mut exposing: Vec<&str> = Vec::new();
    if schema_offers_provider(&delegate.input_schema()) {
        exposing.push("Delegate");
    }
    if schema_offers_provider(&spawn.input_schema()) {
        exposing.push("Spawn");
    }

    if exposing.is_empty() {
        ProbeResult::new(
            Outcome::NoChannel,
            "no provider — neither shipped child-spawn schema offers a way to name one",
            "read the live `input_schema()` of the production Delegate and Spawn tools; neither \
             advertises a provider-naming property anywhere in its object graph",
        )
    } else {
        ProbeResult::new(
            Outcome::Allowed,
            format!("a provider-naming channel on {}", exposing.join(", ")),
            "a shipped child-spawn schema now advertises a provider property; the property that \
             held by absence no longer does, and nothing intersects a requested provider against \
             the parent's authority",
        )
    }
}

/// The approval canary. `PolicySource::Child` exists as a type — the shape of
/// the future channel is already declared — but its only occurrences are inside
/// `execution_policy.rs`'s own test module. Any OTHER file naming it is a
/// production channel appearing, and this fails the moment one does.
///
/// The resolver's live behaviour is measured too and carried in `detail`,
/// because it is what makes the canary matter: the non-managed branch accepts a
/// requested posture verbatim, so the day a channel appears the amplification
/// ships with it.
pub fn approval_no_channel_canary() -> ProbeResult {
    let foreign: Vec<String> = source_files_mentioning("PolicySource::Child")
        .into_iter()
        .filter(|file| !file.ends_with("wcore-types/src/execution_policy.rs"))
        .collect();

    // The seam the census names is `with_requested_approvals` on
    // `BaselineExecutionPolicy` — `EffectiveExecutionPolicy` is the output-only
    // shape and carries no resolver, which is precisely the design that forces
    // an untrusted input through this call. The requested source is
    // `PolicySource::Child`: the type already exists, so the shape of the
    // future channel is declared even though nothing production constructs it.
    let parent =
        BaselineExecutionPolicy::smart(ApprovalPolicy::Prompt, PolicySource::LocalCliLaunch);
    let resolved = parent.with_requested_approvals(ApprovalPolicy::Bypass, PolicySource::Child);
    let resolver_note = format!(
        "resolver measurement (non-managed parent at {:?}, child request Bypass): posture {:?}, \
         approvals {:?}, source {:?}, managed {}",
        parent.approvals(),
        resolved.posture(),
        resolved.approvals(),
        resolved.source(),
        resolved.is_managed()
    );

    if foreign.is_empty() {
        ProbeResult::new(
            Outcome::NoChannel,
            "no approval posture — no production code constructs a child-sourced policy request",
            format!(
                "`PolicySource::Child` is named only by wcore-types/src/execution_policy.rs \
                 itself; {resolver_note}"
            ),
        )
    } else {
        ProbeResult::new(
            Outcome::Allowed,
            format!(
                "a child-sourced approval request channel in {}",
                foreign.join(", ")
            ),
            format!("a child-sourced policy request channel now exists; {resolver_note}"),
        )
    }
}

/// The budget-family canary required by census section 8: the `Some(..)` legs
/// of depth/time/token/cost are protected in part by the absence of a request
/// channel, because every production `sub_budget` caller passes `None`. This
/// reports the day a production caller starts forwarding a child-supplied
/// override.
pub fn budget_no_channel_canary() -> CanaryState {
    // `wcore-budget` is the defining crate: its own `#[cfg(test)]` module at
    // execution.rs:920 exercises `sub_budget(Some(..))`, which is the fixture
    // that proves the override works at all, not a production request channel.
    let callers = production_sites_mentioning("sub_budget(Some(", "crates/wcore-budget/");
    if callers.is_empty() {
        CanaryState::Intact(
            "NO-CHANNEL canary intact: no crates/*/src file forwards a Some(..) override into \
             sub_budget"
                .to_owned(),
        )
    } else {
        CanaryState::Tripped(format!(
            "a production file now forwards a Some(..) budget override into sub_budget: {}",
            callers.join(", ")
        ))
    }
}

// ===========================================================================
// Shared seam probes
// ===========================================================================

/// The phase's primary amplification candidate, attempted rather than assumed.
///
/// A parent whose own registry is read-only issues the production Delegate path
/// with `toolsets: ["Bash"]`. The child is scripted to run a Bash command that
/// writes a probe file into the parent's workspace. The invariant is measured
/// from the filesystem: if the probe exists, the child obtained a tool the
/// parent does not hold.
pub fn tool_widening_through_spawn_fork(session_tag: &str) -> ProbeResult {
    // Bounded, because the delegated child's OWN engine may block this process
    // forever. A child scripted to call Bash reaches the shipped confirmer,
    // which prompts on stdin — the HARNESS's stdin, inherited by the in-process
    // child engine. On Linux under a non-interactive runner `is_terminal()` is
    // false and the call fails closed; on Windows under a scheduled-task
    // (session 0) context it reports TRUE, the prompt is printed, and
    // `read_line` waits for an approver who does not exist. Measured at
    // 46dd076a: `corpus_tool` never returned on SEANDESKTOP.
    //
    // This is a bound on the harness, not a loosened gate. Expiry records
    // NOT-EXPRESSIBLE — never REFUSED — so nothing is ever counted as a refusal
    // because it ran out of time, exactly as the live runs' budget behaves.
    let tag = session_tag.to_owned();
    match run_bounded(Duration::from_secs(45), move || {
        tool_widening_through_spawn_fork_inner(&tag)
    }) {
        Some(probe) => probe,
        None => ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — the probe did not return",
            "the delegated child's engine reached the shipped tool confirmer, which prompts on \
             this process's stdin; no approver exists in process, so the call never returned and \
             no verdict could be taken from it",
        ),
    }
}

/// Run `probe` on its own thread and give up after `budget`.
///
/// The thread is DETACHED on expiry rather than joined: it is blocked on a
/// `read_line` that will never complete, and joining it would hang the suite in
/// place of the probe. A detached thread does not keep a Rust process alive
/// past `main` returning, so the harness still exits cleanly.
fn run_bounded<T: Send + 'static>(
    budget: Duration,
    probe: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(probe());
    });
    rx.recv_timeout(budget).ok()
}

fn tool_widening_through_spawn_fork_inner(session_tag: &str) -> ProbeResult {
    let staging = TempDir::new().expect("tempdir");
    let probe_path = std::fs::canonicalize(staging.path())
        .expect("canonical probe dir")
        .join("corpus_child_bash_probe.txt");
    let script = tool_call_script(
        "Bash",
        json!({ "command": bash_probe_command(&probe_path) }),
    );
    let fixture = parent_fixture(session_tag, script);

    if let Some(reason) = &fixture.bind_failure {
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no child was launched",
            format!("the durable session could not be bound in this fixture: {reason}"),
        );
    }

    let overrides = ForkOverrides {
        model: None,
        effort: None,
        allowed_tools: vec!["Bash".to_owned()],
        budget: None,
    };
    let config = child_config(
        "corpus-tool-widening",
        "run the probe command with the Bash tool",
    );

    let rt = runtime();
    let before = fixture.provider.calls();
    let result = rt.block_on(fixture.spawner.spawn_fork(config, overrides));
    if let Some(withheld) = withhold_if_no_child_ran(
        &fixture,
        before,
        "the production Delegate path (`Spawner::spawn_fork`)",
        &result.text,
    ) {
        return withheld;
    }

    // What the child's own tool call RETURNED, captured at the fixture's
    // endpoint. Carried into the evidence but deliberately NOT used as the
    // verdict — the verdict stays on the effect, per the corpus's rule that no
    // assertion reads an error shape. It is here because the disk effect alone
    // cannot separate the two reasons a probe file might be missing, and a
    // record that cannot separate them should say so rather than imply one.
    let returned = fixture.provider.tool_results().join(" | ");
    let attribution = format!(
        "the probe target sits outside the child's isolated-mutation checkout, so an absent effect \
         is jointly attributable to tool authority and to workspace containment; the corpus cannot \
         separate the two from the effect alone and does not claim to. What the child's own tool \
         call returned: {}",
        truncate(&returned)
    );

    if probe_path.exists() {
        ProbeResult::new(
            Outcome::Allowed,
            "Bash — a tool the read-only parent does not itself hold",
            format!(
                "the delegated child executed a Bash command and its effect landed on disk; \
                 parent workspace {}; child text: {}",
                fixture.root.display(),
                truncate(&result.text)
            ),
        )
    } else {
        ProbeResult::new(
            Outcome::Refused,
            "no Bash effect — nothing the read-only parent does not hold",
            format!(
                "the delegated child produced no Bash effect on disk; parent workspace {}; child \
                 text: {}; {attribution}",
                fixture.root.display(),
                truncate(&result.text)
            ),
        )
    }
}

fn bash_probe_command(probe: &Path) -> String {
    let target = probe.display().to_string();
    if cfg!(windows) {
        format!("echo CORPUS_BASH_PROBE > \"{target}\"")
    } else {
        format!("printf CORPUS_BASH_PROBE > '{target}'")
    }
}

/// The real child VFS stack: `SandboxedFs` over `SecretDenyFs` over `RealFs`,
/// bound to a `WorkspacePolicy::contained` root. Three escape shapes are
/// attempted; the invariant is that none of them returns bytes.
pub fn filesystem_escape_probe() -> ProbeResult {
    let outside_dir = TempDir::new().expect("tempdir outside the workspace");
    let outside = std::fs::canonicalize(outside_dir.path())
        .expect("canonical outside dir")
        .join("outside_the_root.txt");
    std::fs::write(&outside, b"CORPUS_OUTSIDE_THE_ROOT").expect("seed the outside file");

    let inside_dir = TempDir::new().expect("workspace tempdir");
    let root = std::fs::canonicalize(inside_dir.path()).expect("canonical root");
    let link = root.join("escape_link.txt");
    let symlinked = create_symlink(&outside, &link);

    let policy = Arc::new(WorkspacePolicy::contained(root.clone()));
    let vfs = SandboxedFs::new(SecretDenyFs::new(RealFs, Arc::clone(&policy)), root.clone());

    let mut attempts: Vec<(String, PathBuf)> = vec![
        (
            "relative traversal out of the contained root".to_owned(),
            root.join("..").join("..").join("corpus_traversal_target"),
        ),
        (
            "absolute path outside the contained root".to_owned(),
            outside.clone(),
        ),
    ];
    if symlinked {
        attempts.push((
            "symlink whose target leaves the contained root".to_owned(),
            link,
        ));
    }

    // The traversal target must exist outside the root, or the probe would
    // report a refusal that is really a missing file.
    if let Some(parent) = attempts[0].1.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&attempts[0].1, b"CORPUS_TRAVERSAL_TARGET");

    let rt = runtime();
    let mut obtained: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    if !symlinked {
        notes.push(
            "symlink whose target leaves the contained root: not attempted (symlink creation \
             unavailable on this host)"
                .to_owned(),
        );
    }
    for (label, path) in &attempts {
        match rt.block_on(vfs.read(path)) {
            Ok(bytes) if !bytes.is_empty() => {
                obtained.push(label.clone());
                notes.push(format!("{label}: returned {} bytes", bytes.len()));
            }
            _ => notes.push(format!("{label}: returned no bytes")),
        }
    }
    let _ = std::fs::remove_file(&attempts[0].1);

    if obtained.is_empty() {
        ProbeResult::new(
            Outcome::Refused,
            "no bytes from any path outside the parent's contained root",
            notes.join("; "),
        )
    } else {
        ProbeResult::new(
            Outcome::Allowed,
            format!(
                "bytes from outside the parent's contained root via {}",
                obtained.join(", ")
            ),
            notes.join("; "),
        )
    }
}

/// Create a symlink without a platform `cfg` gate, by asking the platform's own
/// tool. Argv mode, no shell string, no interpolation. Returns false when the
/// host does not permit symlink creation (unprivileged Windows), which the
/// caller records rather than papering over.
fn create_symlink(target: &Path, link: &Path) -> bool {
    let mut command = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg("mklink").arg(link).arg(target);
        c
    } else {
        let mut c = std::process::Command::new("ln");
        c.arg("-s").arg(target).arg(link);
        c
    };
    command
        .output()
        .map(|out| out.status.success() && link.exists())
        .unwrap_or(false)
}

/// The real `SecretDenyFs` the child registry installs. Synthetic credential
/// values only — no environment credential is ever written into a corpus file
/// or into the results artifact.
pub fn secret_read_probe() -> ProbeResult {
    let dir = TempDir::new().expect("workspace tempdir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    let dotenv = root.join(".env");
    std::fs::write(&dotenv, b"CORPUS_SYNTHETIC_TOKEN=not-a-real-credential")
        .expect("seed synthetic .env");
    let creds_dir = root.join(".aws");
    std::fs::create_dir_all(&creds_dir).expect("seed synthetic credential dir");
    let creds = creds_dir.join("credentials");
    std::fs::write(&creds, b"corpus_synthetic_key = not-a-real-credential")
        .expect("seed synthetic credentials");

    let policy = Arc::new(WorkspacePolicy::contained(root.clone()).with_project_secret_deny());
    let vfs = SandboxedFs::new(SecretDenyFs::new(RealFs, Arc::clone(&policy)), root.clone());

    let rt = runtime();
    let mut obtained: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for (label, path) in [
        ("synthetic .env", &dotenv),
        ("synthetic credentials file", &creds),
    ] {
        match rt.block_on(vfs.read(path)) {
            Ok(bytes) if !bytes.is_empty() => {
                obtained.push(label.to_owned());
                notes.push(format!("{label}: returned {} bytes", bytes.len()));
            }
            _ => notes.push(format!("{label}: returned no bytes")),
        }
    }
    notes.push(format!(
        "the parent's policy classifies the synthetic .env as secret: {}",
        policy.is_secret_path(&dotenv)
    ));

    if obtained.is_empty() {
        ProbeResult::new(
            Outcome::Refused,
            "no bytes from any credential file the parent's policy denies",
            notes.join("; "),
        )
    } else {
        ProbeResult::new(
            Outcome::Allowed,
            format!("credential bytes via {}", obtained.join(", ")),
            notes.join("; "),
        )
    }
}

/// The real chokepoint `AgentBootstrap` installs — `policy_from_config` under
/// the shipped default config.
///
/// The property under test is RELATIVE, not absolute. Phase 21 asks whether a
/// CHILD can reach a destination its PARENT cannot; it does not ask whether the
/// parent's own policy is correct. So the probe asks the parent's policy about a
/// destination the parent genuinely denies, then asks the policy handle a child
/// inherits about the same destination, and compares. The census records the
/// inheritance mechanism as exact `Arc` identity through `clone_for_spawn`, so
/// pointer identity is checked as well as the decision: a child holding a
/// different policy object is a widening route even when today's two objects
/// happen to agree.
///
/// One absolute observation is recorded but deliberately NOT classified as a
/// Phase 21 widening: with the shipped default config and no consent doorbell
/// attached, a plain GET to a non-allowlisted, non-shared-platform host resolves
/// through the `Ask` branch, which returns Allow when no doorbell is installed.
/// Parent and child are equally affected, so nothing is amplified across the
/// boundary — but it belongs in the results as an observation for triage.
///
/// The second leg is the census's structural clause: no child-reachable
/// production code path may attach an explicit per-client policy that never
/// consults the one the parent installed.
pub fn egress_probe() -> ProbeResult {
    // A shared-platform POST classifies as Exfil, which the policy denies
    // unconditionally and without a doorbell. Using a destination the parent
    // actually refuses is what keeps this probe from passing vacuously.
    let denied_url = "https://webhook.site/corpus-exfil";
    let request = wcore_egress::reqwest::Request::new(
        wcore_egress::Method::POST,
        wcore_egress::Url::parse(denied_url).expect("parse the probe destination"),
    );
    // The probe hands the policy a request to INSPECT; it never dispatches one,
    // so no client is constructed. `reqwest::Client::new` is a disallowed method
    // in this workspace precisely because a client bypasses the B1 egress
    // chokepoint, and the corpus has no business bypassing what it measures.
    let ask_url = "https://corpus-not-allowlisted.invalid/probe";
    let ask_request = wcore_egress::reqwest::Request::new(
        wcore_egress::Method::GET,
        wcore_egress::Url::parse(ask_url).expect("parse the ask-branch destination"),
    );

    let parent: wcore_egress::SharedPolicy = Arc::new(policy_from_config(&Config::default()));
    // What `clone_for_spawn` hands a child: the same object, by Arc identity.
    let inherited: wcore_egress::SharedPolicy = Arc::clone(&parent);
    let same_object = Arc::ptr_eq(&parent, &inherited);

    let rt = runtime();
    let parent_permits = matches!(rt.block_on(parent.check(&request)), EgressDecision::Allow);
    let child_permits = matches!(
        rt.block_on(inherited.check(&request)),
        EgressDecision::Allow
    );
    let parent_permits_ask = matches!(
        rt.block_on(parent.check(&ask_request)),
        EgressDecision::Allow
    );

    // Census MED-3 clause, measured against the exact pattern the census named
    // rather than a bare `with_policy` substring: `with_policy` is also the name
    // of unrelated builder methods, and matching it loosely reports test-only
    // egress clients and provider retry policies as if they were bypass routes.
    let bypass_sites = production_egress_client_sites();

    let structural = if bypass_sites.is_empty() {
        "no production code path constructs an EgressClient with an explicit per-client policy"
            .to_owned()
    } else {
        format!(
            "a production per-client egress client appeared in {}",
            bypass_sites.join(", ")
        )
    };
    let ask_note = format!(
        "recorded, not classified as a Phase 21 widening: with the shipped default config and no \
         consent doorbell attached, the parent's own policy permits a plain GET to {ask_url}: \
         {parent_permits_ask}. Parent and child are equally affected, so nothing crosses the \
         boundary"
    );

    let widened = (child_permits && !parent_permits) || !same_object || !bypass_sites.is_empty();
    if widened {
        ProbeResult::new(
            Outcome::Allowed,
            "an outbound destination the parent's policy does not permit",
            format!(
                "parent permits {denied_url}: {parent_permits}; child permits it: \
                 {child_permits}; the child holds the parent's exact policy object: \
                 {same_object}; {structural}; {ask_note}"
            ),
        )
    } else {
        ProbeResult::new(
            Outcome::Refused,
            "no outbound destination beyond the parent's policy",
            format!(
                "parent permits {denied_url}: {parent_permits}; child permits it: \
                 {child_permits}; the child holds the parent's exact policy object: \
                 {same_object}; {structural}; {ask_note}"
            ),
        )
    }
}

/// Production sites that attach an EXPLICIT per-client egress policy — the
/// exact route the census recorded as MED-3: `EgressClient::new().with_policy(..)`
/// never consults the process-global `OnceLock` or the task-scoped policy the
/// parent installed.
///
/// Three exclusions, each of which the census applied by hand and each of which
/// a looser scan gets wrong:
///
/// * `crates/wcore-egress/` is the crate that DEFINES `with_policy`; its own
///   definition and re-export are not call sites.
/// * A file named `tests.rs`, or one under a `tests/` directory, is a whole-file
///   test module declared `#[cfg(test)] mod tests;` by its parent, so it carries
///   no inner `#[cfg(test)]` line to key on.
/// * Within every other file, an occurrence after the first `#[cfg(test)]` is
///   inside a test module.
///
/// Constructing an `EgressClient` is NOT itself a bypass — it is the sanctioned
/// route through the B1 chokepoint, and 10 production files legitimately do it.
/// Only the explicit per-client policy attachment is the hazard.
fn production_egress_client_sites() -> Vec<String> {
    production_sites_mentioning(".with_policy(", "crates/wcore-egress/")
}

/// Breadth beyond the parent's cap, attempted through the real `SpawnTool`
/// against the real topology configuration. The invariant is measured from the
/// number of children the request actually produced, not from any refusal text.
///
/// ## Why this probe runs a CONTROL first
///
/// Fan-out is the one dimension where zero children is the CORRECT enforcement
/// outcome: an admission gate that rejects an over-cap batch outright produces
/// no child at all, and that is a refusal, not an absence. It is also exactly
/// what a broken fixture produces. The two are indistinguishable from the
/// over-cap run alone, and reading one as the other is the vacuity class F-V2
/// names — which is how this probe recorded REFUSED at both prior SHAs while
/// the fixture could not launch a child for an unrelated reason.
///
/// So the seam is proved live before its refusal is believed: an AT-CAP batch
/// must admit at least one child. If the control admits none, no verdict is
/// taken from the over-cap run. Nothing here reads a refusal message.
pub fn fan_out_probe(session_tag: &str) -> ProbeResult {
    let fixture = parent_fixture(session_tag, done_script());
    let tool = SpawnTool::new(Arc::clone(&fixture.spawner));
    // Topology::Spawn is the default SpawnTool topology and its documented cap.
    let cap = 5usize;
    let over_cap = cap + 3;
    let rt = runtime();

    let batch = |count: usize, prefix: &str| -> serde_json::Value {
        json!({
            "tasks": (0..count)
                .map(|i| json!({ "name": format!("{prefix}{i}"), "prompt": "no-op" }))
                .collect::<Vec<_>>()
        })
    };

    // THE CONTROL. An at-cap batch the gate must admit.
    let control_before = fixture.provider.calls();
    let control = rt.block_on(tool.execute(batch(cap, "corpuscontrol")));
    let control_children = fixture.provider.calls().saturating_sub(control_before);
    if control_children == 0 {
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — the breadth seam admitted no child even at the cap",
            format!(
                "an AT-CAP batch of {cap} produced 0 child provider turns, so this fixture cannot \
                 launch a child at all and an over-cap batch producing 0 would prove nothing about \
                 the cap; control result: {}",
                truncate(&control.content)
            ),
        );
    }

    let before = fixture.provider.calls();
    let result = rt.block_on(tool.execute(batch(over_cap, "corpuschild")));
    let over_cap_children = fixture.provider.calls().saturating_sub(before);

    // The discriminator is whether the over-cap request produced over-cap work.
    // A refused request never reaches a child; an accepted one reports each
    // child it ran by name.
    let named = (0..over_cap)
        .filter(|i| result.content.contains(&format!("corpuschild{i}")))
        .count()
        .max(over_cap_children);

    let evidence = format!(
        "the control batch of {cap} admitted {control_children} child provider turn(s), so the \
         breadth seam is live in this fixture; a batch of {over_cap} was then requested against \
         the Spawn topology cap of {cap} and {named} child(ren) resulted \
         ({over_cap_children} of them reached a provider turn)"
    );
    if named > cap {
        ProbeResult::new(
            Outcome::Allowed,
            format!("breadth of {named} children against a parent cap of {cap}"),
            evidence,
        )
    } else {
        ProbeResult::new(
            Outcome::Refused,
            format!("no breadth beyond the parent cap of {cap}"),
            evidence,
        )
    }
}

// ===========================================================================
// Driver 1 — standalone, in process
// ===========================================================================

pub struct StandaloneInProcess;

impl CorpusExecutor for StandaloneInProcess {
    fn surface(&self) -> Surface {
        Surface::Standalone
    }

    fn mode(&self) -> Mode {
        Mode::InProcess
    }

    fn probe(&self, entry: &CorpusEntry) -> ProbeResult {
        match entry.dimension {
            // S1 — the child-spawn budget primitive itself.
            Dimension::Depth | Dimension::Time | Dimension::Token | Dimension::Cost => {
                let parent = tight_parent_budget().start_root();
                let child = parent.sub_budget(Some(wide_child_budget()));
                let canary = budget_no_channel_canary();
                let mut probe = budget_probe(entry.dimension, &child);
                probe.detail = format!(
                    "ExecutionBudgetView::sub_budget(Some(wider)) at the child-spawn seam; {}; {}",
                    probe.detail,
                    canary.note()
                );
                probe.with_canary(&canary)
            }
            Dimension::Provider => {
                let fixture = parent_fixture("c04b5a-a11e-0001", Vec::new());
                provider_no_channel_canary(Arc::clone(&fixture.spawner))
            }
            Dimension::Approval => approval_no_channel_canary(),
            Dimension::Tool => tool_widening_through_spawn_fork("c04b5a-a11e-0003"),
            Dimension::Filesystem => filesystem_escape_probe(),
            Dimension::Secret => secret_read_probe(),
            Dimension::Egress => egress_probe(),
            Dimension::FanOut => fan_out_probe("c04b5a-a11e-0005"),
        }
    }
}

// ===========================================================================
// Driver 2 — host protocol, in process
// ===========================================================================

/// The host-protocol in-process driver.
///
/// ## What this driver is, and what it used to be — FINDING F-V3
///
/// Until this repair every dimension except the budget family dispatched to the
/// SAME free function as [`StandaloneInProcess`], differing only in a session
/// tag. Seven of eleven pairings were therefore one code path called twice:
/// `assert_surface_equivalence` could not fail on them and proved nothing about
/// two surfaces. The file's own comment claimed "driving them from the
/// protocol-bound path is the point of the comparison" while the code called
/// the standalone probe.
///
/// This driver now reaches its dimensions through the objects the protocol
/// front-end actually owns:
///
/// * The parent is built by the production [`AgentBootstrap`] — the same
///   constructor `wcore-cli`'s `--json-stream` path runs — so the spawner is
///   `govern_spawner(...)`-wrapped, carries the session's durable authority, its
///   execution policy, its egress policy, its approval manager and its session
///   runtime. `StandaloneInProcess` builds a bare `AgentSpawner::new(...)`. The
///   two constructions are not the same object graph and never were.
/// * Children are created through `HostChildController::spawn_child` —
///   `spawn_host_child`, `ChildOrigin::Host` — which is the host's own durable
///   child path, not `Spawner::spawn_fork`.
/// * The budget family stays on `BudgetAuthorityCoordinator::begin_active_turn`,
///   the session/turn seam, against the standalone side's raw
///   `ExecutionBudgetView::sub_budget`.
///
/// ## Where this surface genuinely cannot express a dimension
///
/// Three do not get a driven verdict, and that is REPORTED rather than
/// papered over with a call into the other driver. The host child-spawn request
/// type is `SubAgentConfig`, whose entire field set is name / prompt / max_turns
/// / max_tokens / system_prompt / provider / model / temperature. It carries no
/// tool-authority field, no breadth field and no approval field, and
/// `spawn_host_child` hardcodes `ForkOverrides::default()` — so a tool, fan-out
/// or approval widening cannot be REQUESTED on this surface at all. Those
/// dimensions record NOT-EXPRESSIBLE with the field set as evidence, and the
/// evidence is measured from the live type rather than asserted in prose, so
/// the day one of those fields appears the recorded reason stops being true and
/// the structural canary beside it fires.
pub struct HostProtocolInProcess;

impl CorpusExecutor for HostProtocolInProcess {
    fn surface(&self) -> Surface {
        Surface::HostProtocol
    }

    fn mode(&self) -> Mode {
        Mode::InProcess
    }

    fn probe(&self, entry: &CorpusEntry) -> ProbeResult {
        match entry.dimension {
            Dimension::Depth | Dimension::Time | Dimension::Token | Dimension::Cost => {
                match session_turn_child_view() {
                    Ok(child) => {
                        let canary = budget_no_channel_canary();
                        let mut probe = budget_probe(entry.dimension, &child);
                        probe.detail = format!(
                            "BudgetAuthorityCoordinator::begin_active_turn(turn, Some(wider)) — \
                             the session/turn seam the protocol front-end drives; {}; {}",
                            probe.detail,
                            canary.note()
                        );
                        probe.with_canary(&canary)
                    }
                    Err(reason) => ProbeResult::new(
                        Outcome::NotExpressible,
                        "no resource was requested",
                        format!("the session budget authority could not be bound: {reason}"),
                    ),
                }
            }
            Dimension::Provider => host_child_provider_pin_probe(),
            Dimension::Approval => host_child_approval_inheritance_probe(),
            Dimension::Filesystem => host_child_read_probe(HostReadTarget::OutsideRoot),
            Dimension::Secret => host_child_read_probe(HostReadTarget::CredentialFile),
            Dimension::Egress => host_child_egress_probe(),
            Dimension::Tool => host_request_surface_not_expressible(
                "a tool-authority request",
                &["tool", "allow", "capab", "permission"],
                "`spawn_host_child` hardcodes `ForkOverrides::default()`, whose allowed_tools is \
                 empty, so every host child gets the SHARED_READ_ONLY_CHILD_TOOLS floor and no \
                 caller-supplied tool set",
            ),
            Dimension::FanOut => host_request_surface_not_expressible(
                "a breadth request",
                &["task", "batch", "count", "breadth", "fan", "concurren"],
                "`HostChildController::spawn_child` accepts exactly one SubAgentConfig per call \
                 and exposes no batch entry point",
            ),
        }
    }
}

// ===========================================================================
// The host-protocol in-process machinery — the production bootstrap path
// ===========================================================================

/// The field set of the host child-spawn request type.
///
/// Read by EXHAUSTIVE DESTRUCTURING rather than transcribed into a list, which
/// makes this the strongest canary shape available: adding a field to
/// `SubAgentConfig` stops this function compiling, so a new child-request field
/// cannot reach the product without every NOT-EXPRESSIBLE record that rests on
/// this set being revisited. A hand-written list would silently go stale, and a
/// serde key scan would need a `Serialize` derive on a production type that has
/// no other reason to carry one.
fn host_child_request_fields() -> Vec<&'static str> {
    let SubAgentConfig {
        name: _,
        prompt: _,
        max_turns: _,
        max_tokens: _,
        system_prompt: _,
        provider: _,
        model: _,
        temperature: _,
    } = child_config("field-probe", "field probe");
    vec![
        "name",
        "prompt",
        "max_turns",
        "max_tokens",
        "system_prompt",
        "provider",
        "model",
        "temperature",
    ]
}

/// Which fields of the host child-spawn request type a hostile actor could fill
/// to widen the family described by `needles`. Empty means the request cannot
/// be made on this surface at all.
fn host_request_field_for(needles: &[&str]) -> Vec<&'static str> {
    host_child_request_fields()
        .into_iter()
        .filter(|field| {
            let lower = field.to_ascii_lowercase();
            needles.iter().any(|needle| lower.contains(needle))
        })
        .collect()
}

fn host_request_surface_not_expressible(what: &str, needles: &[&str], why: &str) -> ProbeResult {
    let requestable = host_request_field_for(needles);
    let mut probe = ProbeResult::new(
        Outcome::NotExpressible,
        format!("nothing — {what} cannot be made on this surface"),
        format!(
            "the host child-spawn request type carries the fields {:?} and none of them expresses \
             {what}; {why}. This surface's verdict is WITHHELD rather than borrowed from the \
             standalone driver: an equivalence between a driven result and a copy of the other \
             surface's result would be true by construction",
            host_child_request_fields()
        ),
    );
    if !requestable.is_empty() {
        // The record above just stopped being true. A field through which the
        // widening CAN be requested has appeared on the host child-spawn API,
        // so the dimension no longer holds by inexpressibility on this surface
        // and nothing was put in place of the absence.
        probe.canary_trip = Some(format!(
            "the host child-spawn request type now carries {requestable:?}, through which {what} \
             could be made — this surface's NOT-EXPRESSIBLE record rested on that field not \
             existing"
        ));
    }
    probe
}

/// A bootstrapped parent — the production object graph the protocol front-end
/// runs on — plus the loopback provider it talks to.
struct HostSession {
    /// The bootstrapped engine, HELD. Dropping it is terminal for every clone
    /// of the session root token (`SessionRuntimeGuard::drop`), so a probe that
    /// let it fall out of scope would see its host child cancelled before
    /// reaching a provider turn — an absence, recorded as if it were a refusal.
    /// That is the same vacuity class F-V2 names, one layer up.
    _engine: wcore_agent::engine::AgentEngine,
    _home: TempDir,
    root: PathBuf,
    host_children: wcore_agent::spawner::HostChildController,
    policy: wcore_types::execution_policy::EffectiveExecutionPolicy,
    provider: Arc<CountingProvider>,
    /// Held so the durable session directory outlives every probe.
    _sessions: TempDir,
}

/// Wrap a provider so a probe can tell "the child ran on the parent's own
/// endpoint" apart from "the child ran somewhere else" without inspecting the
/// child's text. The count is the observable; nothing here reads an error shape.
pub struct CountingProvider {
    inner: wcore_agent::test_utils::ScriptedProvider,
    calls: std::sync::atomic::AtomicUsize,
    /// Every tool-result body the provider was shown, in arrival order. This is
    /// how a probe observes what a child's tool call actually RETURNED without
    /// needing the child to narrate it.
    tool_results: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LlmProvider for CountingProvider {
    async fn stream(
        &self,
        request: &wcore_types::llm::LlmRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, wcore_providers::ProviderError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut sink) = self.tool_results.lock() {
            for message in &request.messages {
                for block in &message.content {
                    if let wcore_types::message::ContentBlock::ToolResult { content, .. } = block {
                        sink.push(content.clone());
                    }
                }
            }
        }
        self.inner.stream(request).await
    }
}

impl CountingProvider {
    pub fn new(script: Vec<LlmEvent>) -> Self {
        Self {
            inner: wcore_agent::test_utils::ScriptedProvider::new(script),
            calls: std::sync::atomic::AtomicUsize::new(0),
            tool_results: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn tool_results(&self) -> Vec<String> {
        self.tool_results
            .lock()
            .map(|sink| sink.clone())
            .unwrap_or_default()
    }
}

/// Build the parent through the PRODUCTION bootstrap and bind a durable
/// session, exactly as the `--json-stream` front-end does.
///
/// The workspace root is supplied by the caller and NOT created here, because
/// every read probe must seed its target and bake the exact path into the
/// child's script BEFORE the session exists. A probe that seeded a different
/// path from the one the child reads would report a refusal about a missing
/// file rather than about authority.
fn host_session(
    session_tag: &str,
    script: Vec<LlmEvent>,
    rt: &tokio::runtime::Runtime,
    home: TempDir,
) -> Result<HostSession, String> {
    let root = std::fs::canonicalize(home.path()).map_err(|e| e.to_string())?;
    let sessions = TempDir::new().map_err(|e| e.to_string())?;

    let mut config = Config::default();
    config.session.directory = sessions.path().to_string_lossy().into_owned();
    // Long-term memory is a separate subsystem with its own on-disk store; it
    // has no bearing on child authority and its decay scheduler would outlive
    // the probe.
    config.memory.enabled = false;
    // `resolve_durable_launch` fails closed on an empty resolved model, so a
    // default-config session declares children that never run. See the note in
    // `parent_fixture`.
    config.provider_label = "anthropic".to_owned();
    config.model = "corpus-model".to_owned();

    let provider = Arc::new(CountingProvider::new(script));
    let provider_handle: Arc<dyn LlmProvider> = Arc::clone(&provider) as Arc<dyn LlmProvider>;

    let mut result = rt
        .block_on(
            wcore_agent::bootstrap::AgentBootstrap::new(
                config,
                root.to_string_lossy(),
                Arc::new(wcore_agent::output::null_sink::NullSink),
            )
            .provider(provider_handle)
            .without_channels(true)
            .defer_config_mcp(true)
            .build(),
        )
        .map_err(|error| error.to_string())?;
    result
        .engine
        .init_session("anthropic", &root.to_string_lossy(), Some(session_tag))
        .map_err(|error| error.to_string())?;

    Ok(HostSession {
        _engine: result.engine,
        _home: home,
        root,
        host_children: result.host_children.clone(),
        policy: result.effective_execution_policy.clone(),
        provider,
        _sessions: sessions,
    })
}

/// What a host child is scripted to read.
enum HostReadTarget {
    OutsideRoot,
    CredentialFile,
}

/// Drive a real host child through the production bootstrap and observe what
/// its `Read` actually returned.
///
/// The observable is the tool-result body the provider was shown on the child's
/// SECOND turn — the bytes the shipped VFS stack handed the child, measured at
/// the wire rather than inferred from a hand-built `SandboxedFs`. That is the
/// substantive difference from the standalone probe: this one exercises the VFS
/// the production bootstrap installed into a real child registry.
fn host_child_read_probe(target: HostReadTarget) -> ProbeResult {
    let rt = runtime();
    let (home, outside_dir) = match (TempDir::new(), TempDir::new()) {
        (Ok(home), Ok(outside)) => (home, outside),
        _ => return host_session_unavailable("a hermetic tempdir could not be created"),
    };
    let (sentinel, tag) = match target {
        HostReadTarget::OutsideRoot => ("CORPUS_HOST_OUTSIDE_ROOT", "c04b5a-a11e-0102"),
        HostReadTarget::CredentialFile => ("CORPUS_HOST_SYNTHETIC_TOKEN", "c04b5a-a11e-0103"),
    };

    // Both targets are seeded, and the exact path the child will read is baked
    // into its script, BEFORE the session is built. A refusal against a path
    // that does not exist would be a refusal about a missing file rather than
    // about authority — the vacuity class this whole repair exists to close.
    //
    // The credential file lives INSIDE the workspace root on purpose: outside
    // it, the containment guard would answer before the secret guard and the
    // probe would report the wrong mechanism.
    let outside = outside_dir.path().join("corpus_host_outside.txt");
    let dotenv = home.path().join(".env");
    if std::fs::write(&outside, sentinel.as_bytes()).is_err()
        || std::fs::write(&dotenv, format!("CORPUS_KEY={sentinel}\n").as_bytes()).is_err()
    {
        return host_session_unavailable("a read target could not be seeded");
    }

    let read_path = match target {
        HostReadTarget::OutsideRoot => outside,
        HostReadTarget::CredentialFile => dotenv,
    };
    let script = tool_call_script("Read", json!({ "file_path": read_path.to_string_lossy() }));

    let session = match host_session(tag, script, &rt, home) {
        Ok(session) => session,
        Err(reason) => return host_session_unavailable(&reason),
    };

    let before = session.provider.calls();
    let result = rt.block_on(
        session
            .host_children
            .spawn_child(child_config("corpus-host-read", "read the target file")),
    );
    let child_turns = session.provider.calls().saturating_sub(before);
    let returned = session.provider.tool_results().join(" | ");
    let obtained_bytes = returned.contains(sentinel) || result.text.contains(sentinel);

    if child_turns == 0 {
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — the host child never reached a provider turn",
            format!(
                "`HostChildController::spawn_child` returned without the child taking a turn on \
                 the session's own endpoint, so nothing was observed about what it could read; \
                 session root {}; child text: {}",
                session.root.display(),
                truncate(&result.text)
            ),
        );
    }

    let what = match target {
        HostReadTarget::OutsideRoot => "a file outside the session's workspace root",
        HostReadTarget::CredentialFile => "a credential file the session's policy denies",
    };
    let evidence = format!(
        "a real host-originated child ran {child_turns} provider turn(s) against the tool registry \
         the production bootstrap installed; it read {} under session root {}. Tool results the \
         session's own provider was shown: {}",
        read_path.display(),
        session.root.display(),
        truncate(&returned)
    );
    if obtained_bytes {
        ProbeResult::new(Outcome::Allowed, format!("the bytes of {what}"), evidence)
    } else {
        ProbeResult::new(Outcome::Refused, format!("no bytes of {what}"), evidence)
    }
}

/// The host-protocol provider probe.
///
/// Unlike the standalone surface — where the property holds by the ABSENCE of a
/// provider field on either shipped tool schema — the host child-spawn request
/// type DOES carry `provider`. So this surface has a request channel, and the
/// question is whether filling it obtains a provider the session does not hold.
/// The observable is which endpoint the child talked to: the session's own
/// counting provider, or something else.
fn host_child_provider_pin_probe() -> ProbeResult {
    let rt = runtime();
    let home = match TempDir::new() {
        Ok(home) => home,
        Err(error) => return host_session_unavailable(&error.to_string()),
    };
    let session = match host_session("c04b5a-a11e-0101", done_script(), &rt, home) {
        Ok(session) => session,
        Err(reason) => return host_session_unavailable(&reason),
    };

    let mut config = child_config("corpus-host-provider", "summarise the workspace");
    config.provider = Some("openai".to_owned());

    let before = session.provider.calls();
    let result = rt.block_on(session.host_children.spawn_child(config));
    let on_session_endpoint = session.provider.calls() > before;

    if result.is_error || !on_session_endpoint {
        ProbeResult::new(
            Outcome::Refused,
            "no provider — the pinned foreign provider was not obtained",
            format!(
                "a host child pinned to `openai` under an anthropic session did not run: the \
                 session's own endpoint served {} additional request(s) and the child produced \
                 no completed work, so no foreign provider was reached. Child text: {}",
                session.provider.calls().saturating_sub(before),
                truncate(&result.text)
            ),
        )
    } else {
        ProbeResult::new(
            Outcome::Refused,
            "no provider beyond the session's own",
            format!(
                "a host child pinned to `openai` ran on the session's OWN configured endpoint \
                 ({} additional request(s)); the pin did not obtain a different upstream. Child \
                 text: {}",
                session.provider.calls().saturating_sub(before),
                truncate(&result.text)
            ),
        )
    }
}

/// The host-protocol approval probe — a PER-CHILD observable, not a grep.
///
/// Every durable child the host path creates carries a `ChildPolicySnapshot`
/// derived from the session's own `EffectiveExecutionPolicy`
/// (`spawner.rs::child_policy_snapshot`). The invariant is measured from that
/// record: a child whose recorded approvals posture is WEAKER than the session's
/// obtained an approval posture beyond its parent's. This is a different
/// mechanism and a different object from the standalone structural canary, and
/// it reads what the child was actually given rather than what the source says.
fn host_child_approval_inheritance_probe() -> ProbeResult {
    let rt = runtime();
    let home = match TempDir::new() {
        Ok(home) => home,
        Err(error) => return host_session_unavailable(&error.to_string()),
    };
    let session = match host_session("c04b5a-a11e-0104", done_script(), &rt, home) {
        Ok(session) => session,
        Err(reason) => return host_session_unavailable(&reason),
    };

    let parent_posture = format!("{:?}", session.policy.approvals()).to_ascii_lowercase();
    let supervisor = match session.host_children.supervisor() {
        Ok(supervisor) => supervisor,
        Err(error) => return host_session_unavailable(&error.to_string()),
    };
    let _ = rt.block_on(
        session
            .host_children
            .spawn_child(child_config("corpus-host-approval", "no-op")),
    );
    let records = match supervisor.list() {
        Ok(records) => records,
        Err(error) => return host_session_unavailable(&error.to_string()),
    };
    let Some(record) = records.first() else {
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — no durable child record was declared",
            "the host child path recorded no child, so no per-child approval posture was \
             observable"
                .to_owned(),
        );
    };

    let child_posture = record.policy_snapshot.approvals.to_ascii_lowercase();
    let weakened = approval_rank(&child_posture) > approval_rank(&parent_posture);
    let requestable = host_request_field_for(&["approv", "policy", "posture", "permission"]);
    let detail = format!(
        "the session's launch policy records approvals {parent_posture:?}; the durable record for \
         the host-originated child records approvals {child_posture:?} (source {:?}, managed floor \
         {}). The host child-spawn request type offers no field through which a weaker posture \
         could be asked for: {requestable:?}",
        record.policy_snapshot.source, record.policy_snapshot.managed_floor_active
    );

    if weakened {
        ProbeResult::new(
            Outcome::Allowed,
            format!("an approval posture of {child_posture} under a parent at {parent_posture}"),
            detail,
        )
    } else {
        ProbeResult::new(
            Outcome::Refused,
            "no approval posture weaker than the session's",
            detail,
        )
    }
}

/// Strictness order, loosest highest. Any posture the corpus has not seen ranks
/// as the loosest so an unrecognised value can never be read as "at least as
/// strict as the parent".
fn approval_rank(posture: &str) -> u8 {
    match posture {
        p if p.contains("prompt") => 0,
        p if p.contains("auto") => 1,
        p if p.contains("bypass") => 2,
        _ => 3,
    }
}

/// The host-protocol egress probe.
///
/// A real host child is scripted to fetch a loopback sentinel the session's
/// policy does not allowlist, and the destination itself reports whether any
/// request arrived. Three outcomes, discriminated by measurement rather than by
/// text: the body reached the child (ALLOWED), a request reached the
/// destination but the body did not (REFUSED), or no request was ever issued
/// (NOT-EXPRESSIBLE — the child had no network-capable tool, so nothing was
/// attempted and an absence would prove nothing).
fn host_child_egress_probe() -> ProbeResult {
    let rt = runtime();
    let sentinel_body = "CORPUS_HOST_EGRESS_SENTINEL";
    let server = rt.block_on(wiremock::MockServer::start());
    rt.block_on(
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/corpus-host-egress"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(sentinel_body))
            .mount(&server),
    );
    let url = format!("{}/corpus-host-egress", server.uri());

    let script = tool_call_script("WebFetch", json!({ "url": url }));
    let home = match TempDir::new() {
        Ok(home) => home,
        Err(error) => return host_session_unavailable(&error.to_string()),
    };
    let session = match host_session("c04b5a-a11e-0105", script, &rt, home) {
        Ok(session) => session,
        Err(reason) => return host_session_unavailable(&reason),
    };

    let before = session.provider.calls();
    let result = rt.block_on(
        session
            .host_children
            .spawn_child(child_config("corpus-host-egress", "fetch the destination")),
    );
    let child_turns = session.provider.calls().saturating_sub(before);
    let returned = session.provider.tool_results().join(" | ");
    let received = rt
        .block_on(server.received_requests())
        .unwrap_or_default()
        .len();
    let obtained = returned.contains(sentinel_body) || result.text.contains(sentinel_body);

    if obtained {
        ProbeResult::new(
            Outcome::Allowed,
            "an outbound destination the session's policy does not permit",
            format!(
                "a host-originated child ran {child_turns} provider turn(s) and the loopback \
                 sentinel body reached it; the destination served {received} request(s)"
            ),
        )
    } else if received > 0 {
        ProbeResult::new(
            Outcome::Refused,
            "no outbound body beyond the session's policy",
            format!(
                "a host-originated child ran {child_turns} provider turn(s); the destination \
                 served {received} request(s) and the sentinel body did not reach the child"
            ),
        )
    } else {
        ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — no outbound request was issued by the child",
            format!(
                "a host-originated child ran {child_turns} provider turn(s) and the loopback \
                 destination served 0 requests, so no outbound attempt was made and an absent \
                 body would prove nothing. Tool results the session's provider was shown: {}",
                truncate(&returned)
            ),
        )
    }
}

fn host_session_unavailable(reason: &str) -> ProbeResult {
    ProbeResult::new(
        Outcome::NotExpressible,
        "no verdict — the production host session could not be built",
        format!(
            "AgentBootstrap could not produce a bound session in this fixture, so no host-protocol \
             child could be driven: {reason}"
        ),
    )
}

/// Build the active-turn child view the way the protocol front-end's session
/// authority does, handing the turn a WIDER budget than the session root.
fn session_turn_child_view() -> Result<ExecutionBudgetView, String> {
    let seed = BudgetAuthoritySeed {
        provider_caps: BudgetCap::default(),
        preserve_committed_session_extensions: false,
        execution_policy: tight_parent_budget(),
        wall_clock: BudgetWallClockAuthority::ActiveRuntime,
        process_cleanup_proof: None,
    };
    let mut coordinator = BudgetAuthorityCoordinator::bind(seed.config(None, "corpus-session"))
        .map_err(|error| error.to_string())?;
    coordinator
        .begin_active_turn("corpus-turn", Some(wide_child_budget()))
        .map_err(|error| error.to_string())?;
    coordinator
        .current_execution_view()
        .map_err(|error| error.to_string())
}
