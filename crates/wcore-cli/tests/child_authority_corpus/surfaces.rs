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
    parent_fixture_with_allow_list(
        session_tag,
        script,
        Vec::new(),
        SandboxChoice::RegistryDefault,
    )
}

/// Which sandbox runtime a fixture's spawner carries — 21-C3.
///
/// `ToolRegistry::new()` seeds `FailClosedBackend`, and `AgentSpawner::new`
/// takes its runtime from there. `BashTool` refuses to spawn a shell at all
/// when the workspace requires secret-read-deny and the backend cannot enforce
/// it (`bash.rs:497`), so under the default EVERY delegated Bash call is
/// refused before tool authority, workspace containment or the approval gate
/// is consulted.
///
/// That is a fixture fact, not a product fact: production resolves a real
/// backend through `SandboxRegistry::required_for_session` in
/// `bootstrap.rs:1061`, and the shipped `--json-stream` run at `359ce2bf`
/// reports `"backend":"bubblewrap"` on its `workspace_policy` frame. A probe
/// about tool authority that runs under a fail-closed shell is measuring the
/// sandbox.
#[derive(Clone, Copy)]
pub enum SandboxChoice {
    /// What every corpus fixture has always carried: `ToolRegistry`'s
    /// fail-closed default. Correct for every dimension whose probe does not
    /// need a shell.
    RegistryDefault,
    /// The PRODUCTION resolver, called exactly as `AgentBootstrap` calls it. On
    /// a host with no real backend it resolves fail-closed again — and the
    /// known-positive arm of the tool differential then reports that the
    /// instrument is dead rather than recording a refusal.
    ProductionResolved,
}

/// The same fixture with named tools placed on the session's approval
/// ALLOW-LIST — 21-C3.
///
/// ## Why this parameter had to exist
///
/// `Config::default()` resolves to `ApprovalPolicy::Prompt`
/// (`Config::smart_approval_policy`), `AgentSpawner::child_config` deliberately
/// hands the child the parent's posture unchanged (audit H-7 / M-9), and
/// `ToolConfirmer::check_for` returns `Denied` unconditionally when stdin is not
/// a terminal. Under a CI runner that is EVERY tool call a delegated child makes.
///
/// The consequence was measured, not theorised. At `359ce2bf` the standalone
/// in-process tool row recorded `REFUSED :: obtained no Bash effect` while its
/// own evidence field carried, verbatim, *"What the child's own tool call
/// returned: Tool execution denied by user"* — the string `confirm_call` emits
/// on a denial. The child's Bash call never reached the tool registry, the
/// workspace guard or the Bash tool. `21-04-PHASE-VERDICT.md` records that
/// refusal as *"jointly attributable to tool authority and to workspace
/// containment"*; neither of those two was exercised, and the cause that fired
/// is not on the list.
///
/// The allow-list is the NARROWEST available neutraliser and is preferred over
/// `tools.auto_approve` on purpose: `requires_confirmation_for` consults the
/// allow-list BEFORE the policy, so the session's resolved
/// `ApprovalPolicy` stays `Prompt` and every per-child approval observable this
/// corpus reads keeps its real value. It states a real operator posture — "this
/// session has already allowed Bash" — rather than removing the approval
/// mechanism.
///
/// It does NOT touch tool authority: `build_tool_registry` intersects against
/// `parent_tool_authority`, which the allow-list has no bearing on. That
/// separation is what makes the authority differential in
/// [`tool_widening_through_spawn_fork`] single-variable.
pub fn parent_fixture_with_allow_list(
    session_tag: &str,
    script: Vec<LlmEvent>,
    allow_list: Vec<String>,
    sandbox: SandboxChoice,
) -> ParentFixture {
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
    // 21-C3. Empty for every caller but the tool differential — see
    // `parent_fixture_with_allow_list`.
    config.tools.allow_list = allow_list;

    let counted = Arc::new(CountingProvider::new(script));
    let provider: Arc<dyn LlmProvider> = Arc::clone(&counted) as Arc<dyn LlmProvider>;
    // 21-C3 — the production sandbox resolution, called the way `AgentBootstrap`
    // calls it. A resolution error is carried into `bind_failure` so a probe
    // withholds rather than reading a shell refusal as a tool refusal.
    let (sandbox_runtime, sandbox_failure) = match sandbox {
        SandboxChoice::RegistryDefault => (None, None),
        SandboxChoice::ProductionResolved => {
            match wcore_sandbox::SandboxRegistry::required_for_session(
                config.tools.sandbox.as_deref(),
            ) {
                Ok(registry) => (Some(Arc::new(registry)), None),
                Err(error) => (
                    None,
                    Some(format!(
                        "the production sandbox resolver \
                         (`SandboxRegistry::required_for_session`) could not produce a runtime on \
                         this host: {error}"
                    )),
                ),
            }
        }
    };
    let spawner = AgentSpawner::new(provider, config);
    let spawner = match sandbox_runtime {
        Some(runtime) => spawner.with_sandbox_runtime(runtime),
        None => spawner,
    };
    let spawner = spawner
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
        bind_failure: bind_failure.or(repo_failure).or(sandbox_failure),
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

/// The budget-family canary required by census section 8, **re-pointed from
/// absence to enforcement**.
///
/// ## The history this function carries
///
/// The census recorded depth/time/token/cost as protected *in part* by the
/// absence of a request channel: no shipped surface carried a child-fillable
/// budget field, so "a child cannot widen its own budget" held because nothing
/// could ask. The original canary asserted that absence — `Intact` while no
/// production caller forwarded an override, `Tripped` the day one appeared.
///
/// Two failures followed, and both are instructive:
///
/// 1. Phase 21's F21-02 BUILT the sub-allocation channel
///    (`ExecutionBudgetView::sub_budget_narrowed`, reached from the Delegate
///    tool's `budget` object through `ForkOverrides`). The canary did not
///    notice, because it matched only the literal spelling `sub_budget(Some(`
///    while the production caller landed as `sub_budget_narrowed(..)`. A canary
///    keyed to one spelling of the thing it watches reports on the spelling.
/// 2. Unblinding it to match both spellings made it truthful and made it trip —
///    permanently, on every run, for a channel whose existence is intended. A
///    canary whose `Tripped` state is the correct steady state is not a canary;
///    it is a red light wired to the ignition.
///
/// ## What it measures now
///
/// The property the corpus owes has not changed — *a child must not obtain a
/// budget envelope wider than its parent's* — but the reason it holds has. It
/// no longer rests on the channel's non-existence, so this canary no longer
/// grades non-existence. It **drives the real channel with a hostile request
/// and observes the refusal**, and it trips in the two directions that would
/// actually be news:
///
/// * **The channel VANISHED.** No `crates/*/src` file forwards a child-supplied
///   override into `sub_budget`/`sub_budget_narrowed` any more. That is F21-02
///   reverting to vacuity — the state Phase 21 graded NOT MET three times — and
///   it must be loud, so the *absence* is now the alarm rather than the pass.
/// * **The widening SUCCEEDED.** `sub_budget_narrowed` was handed caps orders of
///   magnitude wider than the parent's and the child came back holding them.
///
/// ## Why it can go red (F-V4's lesson, applied)
///
/// F-V4 recorded that the previous canary "could trip silently forever" because
/// nothing asserted on it. This one is consumed exactly as before — through
/// [`ProbeResult::with_canary`] into `canary_trip`, asserted by
/// `assert_no_channel_canaries_stayed_intact` — and its executable half is
/// differential rather than tautological:
///
/// * `limit_for(..)` renders the child's OWN leaf cap when nothing is exceeded.
///   Revert `sub_budget_narrowed` to `self.sub_budget(Some(requested))` (drop
///   the intersection) and the child names 10_000_000 tokens / $1_000_000 /
///   depth 1_000 instead of the parent's 100 / $0.01 / 1, and this trips.
/// * `effective_budget()` folds the whole ancestor chain. Drop the ancestor leg
///   and it reports the requested envelope, and this trips.
///
/// Neither number is written by this test, and neither was obtainable before
/// the seam existed.
pub fn budget_narrowing_channel_canary(dimension: Dimension) -> CanaryState {
    // ---- Half 1: the channel must still EXIST in production. --------------
    // `wcore-budget` is the defining crate: its own `#[cfg(test)]` module
    // exercises `sub_budget(Some(..))`, which is the fixture proving the
    // override works at all, not a production request channel. Excluded so a
    // unit test can never stand in for a shipped caller.
    let mut callers = production_sites_mentioning("sub_budget(Some(", "crates/wcore-budget/");
    callers.extend(production_sites_mentioning(
        "sub_budget_narrowed(",
        "crates/wcore-budget/",
    ));
    callers.sort();
    callers.dedup();

    if callers.is_empty() {
        return CanaryState::Tripped(
            "the sub-allocation REQUEST CHANNEL HAS VANISHED: no crates/*/src file forwards a \
             child-supplied override into sub_budget or sub_budget_narrowed. F21-02 has reverted \
             to being satisfied by ABSENCE — the property would hold because nothing can ask, \
             which is the exact vacuity Phase 21 graded NOT MET three times. This canary grades \
             enforcement now, and there is nothing left to enforce against."
                .to_owned(),
        );
    }

    // ---- Half 2: drive the real channel and observe the refusal. ----------
    // The hostile request is `wide_child_budget()` — every cap wider than the
    // parent's by orders of magnitude — pushed through the PRODUCTION seam
    // (`sub_budget_narrowed`), not through the raw `sub_budget(Some(..))` the
    // standalone driver already exercises.
    let parent = narrowing_probe_parent().start_root();
    let child = parent.sub_budget_narrowed(wide_child_budget());
    let requested = wide_child_budget();
    let parent_caps = narrowing_probe_parent();
    let effective = child.effective_budget();

    // Per dimension: (the reason string `limit_for` keys on, the cap the parent
    // holds, the cap the hostile request asked for, the cap the child ended up
    // NAMING, the cap that actually BINDS it). Every rendering below uses the
    // same formatting `limit_for` uses, so `named` is compared like for like.
    let (reason, parent_cap, asked, named, binding) = match dimension {
        Dimension::Depth => (
            "max_agent_depth",
            render_usize(parent_caps.max_agent_depth),
            render_usize(requested.max_agent_depth),
            child.limit_for("max_agent_depth"),
            render_usize(effective.max_agent_depth),
        ),
        Dimension::Time => (
            "max_wall_time",
            render_duration(parent_caps.max_wall_time),
            render_duration(requested.max_wall_time),
            child.limit_for("max_wall_time"),
            render_duration(effective.max_wall_time),
        ),
        Dimension::Token => (
            "max_tokens_in",
            render_u64(parent_caps.max_tokens_in),
            render_u64(requested.max_tokens_in),
            child.limit_for("max_tokens_in"),
            render_u64(effective.max_tokens_in),
        ),
        Dimension::Cost => (
            "max_cost_usd",
            render_f64(parent_caps.max_cost_usd),
            render_f64(requested.max_cost_usd),
            child.limit_for("max_cost_usd"),
            render_f64(effective.max_cost_usd),
        ),
        other => {
            return CanaryState::Intact(format!(
                "{} is not a budget-rollup dimension; the sub-allocation canary does not apply \
                 to it",
                other.census_name()
            ));
        }
    };

    // The refusal is stated as an EQUALITY against the parent's cap, not as
    // "!= the requested cap". `intersect_optional` is a pointwise minimum and
    // every requested cap here is strictly wider, so a working seam lands the
    // child EXACTLY on the parent's number. Testing inequality-against-the-ask
    // would let a seam that widened to some third value pass; testing equality
    // against the parent leaves it nowhere to land but the right answer.
    let binding_ok = binding == parent_cap;
    let named_ok = named == parent_cap;
    let evidence = format!(
        "sub_budget_narrowed({reason}) — parent holds {parent_cap}, the child asked for {asked}, \
         the child NAMES {named} and is BOUND BY {binding}; production callers: {}",
        callers.join(", ")
    );

    if binding_ok && named_ok {
        CanaryState::Intact(format!(
            "sub-allocation channel intact and ENFORCED (not vacuous): {evidence}"
        ))
    } else {
        let which = match (binding_ok, named_ok) {
            (false, false) => "both the envelope that BINDS the child and the one it NAMES",
            (true, false) => {
                "the envelope the child NAMES (the ancestor rollup still binds it, so every \
                 leaf-rendering accessor now reports a cap the child cannot actually spend — \
                 which is the exact defect sub_budget_narrowed was added to close)"
            }
            (false, true) => "the envelope that BINDS the child",
            (true, true) => unreachable!("handled by the Intact arm"),
        };
        CanaryState::Tripped(format!(
            "A CHILD WIDENED ITS OWN BUDGET THROUGH THE LIVE SUB-ALLOCATION CHANNEL: {which} \
             departed from the parent's cap. {evidence}. The channel was built on the promise \
             that narrowing is monotonic by construction, so a request could only ever LOWER a \
             cap. That promise no longer holds on this dimension."
        ))
    }
}

/// The parent envelope the sub-allocation canary probes against.
///
/// Deliberately NOT [`tight_parent_budget`]. That fixture's 40ms wall cap is
/// sized so `budget_probe`'s Time leg can watch a child burn past it — which is
/// the opposite of what this probe needs. `limit_for` resolves through
/// `with_reason_state`, which renders the cap of whichever state is *currently
/// exceeded* and only falls back to the leaf when none is: with a 40ms parent
/// cap, any scheduling hiccup between `start_root()` and the read would make
/// the ancestor exceeded, so `limit_for` would render the PARENT's cap and the
/// probe would report a pass no matter what the child named. That is a
/// self-passing gate, and this fixture exists to close it.
///
/// One hour is unreachable inside a probe that runs in microseconds, and is
/// still 24x narrower than the 86_400s the hostile request asks for, so the
/// differential stays large.
fn narrowing_probe_parent() -> ExecutionBudget {
    ExecutionBudget {
        max_wall_time: Some(Duration::from_secs(3_600)),
        max_tool_runtime: None,
        max_processes: None,
        max_agent_depth: Some(1),
        max_tokens_in: Some(100),
        max_tokens_out: Some(100),
        max_cost_usd: Some(0.01),
    }
}

fn render_usize(v: Option<usize>) -> String {
    v.map(|n| n.to_string()).unwrap_or_default()
}

fn render_u64(v: Option<u64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_default()
}

fn render_f64(v: Option<f64>) -> String {
    v.map(|c| format!("${c:.4}")).unwrap_or_default()
}

fn render_duration(v: Option<Duration>) -> String {
    v.map(|d| format!("{:.1}s", d.as_secs_f64()))
        .unwrap_or_default()
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
pub fn tool_widening_through_spawn_fork(session_tag: &str, seam: ChildSpawnSeam) -> ProbeResult {
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
    //
    // 21-C3: the bound is applied PER ARM (see `tool_arm`) rather than to the
    // whole probe. The probe now runs two arms, and widening one 45 s budget to
    // cover both would have been a raised timeout wearing a refactor's clothes.
    // Each arm keeps exactly the budget the single run had.
    tool_widening_through_spawn_fork_inner(session_tag, seam)
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

/// What one arm of the tool differential observed. Every field is a POSITIVE
/// effect the child produced; none of them is an error string, an error kind or
/// a status.
struct ToolArm {
    /// The child's own Bash stdout came back. Blocked by R2 on Linux; kept
    /// visible rather than designed around.
    shell_executed: bool,
    /// THE AUTHORITY OBSERVABLE — the child obtained `Write`, a mutating tool
    /// outside the read-only delegation floor, proved by reading its own
    /// written sentinel back.
    obtained_mutating_tool: bool,
    /// A file landed at a path OUTSIDE the child's workspace root.
    escaped_containment: bool,
    child_turns: usize,
    /// `Some(reason)` when this arm produced no observation at all.
    withheld: Option<String>,
    child_text: String,
    returned: String,
    /// The parent workspace this arm ran against, carried so a reader can see
    /// that the two arms were separate hermetic worlds and that the "outside"
    /// target really is outside this root.
    parent_workspace: String,
}

impl ToolArm {
    /// Did this arm obtain ANY tool outside the read-only delegation floor?
    ///
    /// F21-C3. `Bash` and `Write` are independent members of the same class:
    /// both sit outside `SHARED_READ_ONLY_CHILD_TOOLS`, and both are admitted
    /// to a child's registry by exactly one predicate —
    /// `build_tool_registry`'s intersection against `parent_tool_authority`.
    /// Either one executing is therefore a positive observation that the child
    /// held mutating authority, and the differential needs only one.
    ///
    /// Keying the gate on `Write` ALONE is what made this dimension read
    /// NOT-EXPRESSIBLE on every cell. Measured at `5be91056`: the granted arm's
    /// `Write` fails with *"path must be absolute"* — `Write`/`Read` demand an
    /// absolute path and a delegated child's isolated checkout is allocated at
    /// `<session>/delegated-workspaces/checkouts/<worker_id>`, which a scripted
    /// corpus cannot know before the child launches (21-C3-04). In the SAME run
    /// the granted arm's shell DID execute and the denied arm's shell did NOT,
    /// the denied arm returning `Denied by policy: no matching grant for
    /// actor+resource+action`. A working differential was already present and
    /// the gate was reading past it.
    ///
    /// The `Bash` leg was previously unusable for an unrelated reason — the
    /// overlapping-read-deny bubblewrap abort (21-C3-01) — which the
    /// `f21-bwrap-overlap` lane has since fixed. This gate is only correct
    /// BECAUSE that fix landed; before it, both observables were genuinely dead
    /// and NOT-EXPRESSIBLE was the right answer.
    fn obtained_any_mutating_tool(&self) -> bool {
        self.obtained_mutating_tool || self.shell_executed
    }
}

/// Which production child-spawn entry point an arm drives.
///
/// F21-C3: this is the ONLY thing that differs between the two SURFACES. Both
/// variants converge on the same `spawn_durable` seam and therefore on the same
/// single `build_tool_registry` intersection — which is precisely the claim the
/// tool dimension needs tested rather than assumed, and the reason a surface
/// difference here is meaningful instead of cosmetic.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChildSpawnSeam {
    /// `Spawner::spawn_fork` — `ChildOrigin::Delegate`. The standalone surface.
    Delegate,
    /// `AgentSpawner::spawn_host_child_with_overrides` — `ChildOrigin::Host`.
    /// The host surface's own durable child path.
    HostChild,
}

impl ChildSpawnSeam {
    fn label(self) -> &'static str {
        match self {
            Self::Delegate => "`Spawner::spawn_fork` (ChildOrigin::Delegate)",
            Self::HostChild => {
                "`AgentSpawner::spawn_host_child_with_overrides` (ChildOrigin::Host)"
            }
        }
    }
}

/// The parent authority an arm runs under. This is the ONLY thing that differs
/// between the two arms.
enum ParentAuthority {
    /// The parent session holds the full child-eligible set, Bash included.
    HoldsBash,
    /// The parent session's own registry is read-only — the hostile premise the
    /// tool dimension has always CLAIMED and never actually established.
    ReadOnly,
}

/// Drive one arm under the same 45 s bound the single-run probe carried, so
/// adding a second arm does not silently double anyone's budget.
fn tool_arm(
    session_tag: &str,
    authority: ParentAuthority,
    marker: &str,
    seam: ChildSpawnSeam,
) -> ToolArm {
    let tag = session_tag.to_owned();
    let marker_owned = marker.to_owned();
    run_bounded(Duration::from_secs(45), move || {
        tool_arm_inner(&tag, authority, &marker_owned, seam)
    })
    .unwrap_or_else(|| ToolArm {
        shell_executed: false,
        obtained_mutating_tool: false,
        escaped_containment: false,
        child_turns: 0,
        withheld: Some(
            "the delegated child's engine did not return within 45 s; on a session-0 Windows \
             context the shipped confirmer prints its prompt and blocks on a `read_line` no \
             approver will answer"
                .to_owned(),
        ),
        child_text: String::new(),
        returned: String::new(),
        parent_workspace: "not reached — the arm did not return".to_owned(),
    })
}

/// Drive one arm: a delegated child scripted to exercise a MUTATING tool the
/// read-only delegation floor does not grant, then read the effect back.
///
/// ## Why the probe drives BOTH `Write` and `Bash` — 21-C3
///
/// Neither observable is sufficient alone, and which one carries the verdict
/// has changed as the tree moved. Both are driven, and
/// `ToolArm::obtained_any_mutating_tool` accepts either.
///
/// `Bash` could not reach a verdict on Linux while 21-C3-01 was open: every
/// delegated isolated-mutation child's shell died in the sandbox before running
/// (`spawner.rs` emitted an OVERLAPPING read-deny pair and `bwrap.rs` aborted
/// on it). The `f21-bwrap-overlap` lane fixed that in the renderer, and the
/// shell now runs — measured at `5be91056`: the granted arm reports
/// `shell executed: true`.
///
/// `Write` cannot reach a verdict either, for an unrelated and still-open
/// reason (21-C3-04): `Write`/`Read` demand an ABSOLUTE path, and a delegated
/// child's isolated checkout is allocated at
/// `<session>/delegated-workspaces/checkouts/<worker_id>`, which a scripted
/// corpus cannot know before the child launches. The granted arm's `Write`
/// therefore returns `path must be absolute` in every run.
///
/// A dimension whose only probe is blocked by an unrelated defect measures that
/// defect, not authority — which is why keying the gate on `Write` alone made
/// every tool cell read NOT-EXPRESSIBLE while a working `Bash` differential sat
/// unread in the same output.
///
/// `Write` is the same class of request — a mutating tool outside
/// `SHARED_READ_ONLY_CHILD_TOOLS`, admitted by `build_tool_registry` only when
/// the parent's own authority contains it — and it does not need a shell. The
/// child writes a sentinel to a RELATIVE path inside its own workspace, then
/// reads it back with `Read`, which the read-only floor always grants. The
/// sentinel returning is a POSITIVE effect: nothing but a successful Write
/// followed by a successful Read can produce it, and neither the confirmer, the
/// sandbox, nor workspace containment is in the path.
///
/// `Bash` is still driven, in the same arm, and its outcome is recorded
/// separately — that is how R2 was found and it must stay visible.
fn tool_arm_inner(
    session_tag: &str,
    authority: ParentAuthority,
    marker: &str,
    seam: ChildSpawnSeam,
) -> ToolArm {
    let staging = TempDir::new().expect("tempdir");
    let outside_path = std::fs::canonicalize(staging.path())
        .expect("canonical probe dir")
        .join("corpus_child_bash_probe.txt");
    let write_sentinel = format!("{marker}WRITTEN");
    let mut script = vec![
        LlmEvent::ToolUse {
            id: "corpus-write-call".to_owned(),
            name: "Write".to_owned(),
            input: json!({
                "file_path": "corpus_authority_probe.txt",
                "content": write_sentinel,
            }),
            extra: None,
        },
        LlmEvent::ToolUse {
            id: "corpus-readback-call".to_owned(),
            name: "Read".to_owned(),
            input: json!({ "file_path": "corpus_authority_probe.txt" }),
            extra: None,
        },
    ];
    script.extend(tool_call_script(
        "Bash",
        json!({ "command": bash_probe_command(marker, &outside_path) }),
    ));
    // The confirmer is neutralised IDENTICALLY in both arms — see
    // `parent_fixture_with_allow_list`. Holding it constant is what makes the
    // differential single-variable.
    // The sandbox is resolved the way production resolves it, IDENTICALLY in
    // both arms, for the same reason the confirmer is: a shell the backend
    // refuses to spawn produces the same "no effect" reading as a tool the
    // parent's authority denies, and the two must not be able to swap places.
    let fixture = parent_fixture_with_allow_list(
        session_tag,
        script,
        vec!["Bash".to_owned(), "Write".to_owned(), "Read".to_owned()],
        SandboxChoice::ProductionResolved,
    );

    if let Some(reason) = &fixture.bind_failure {
        return ToolArm {
            shell_executed: false,
            obtained_mutating_tool: false,
            escaped_containment: false,
            child_turns: 0,
            withheld: Some(format!(
                "the durable session could not be bound in this fixture: {reason}"
            )),
            child_text: String::new(),
            returned: String::new(),
            parent_workspace: fixture.root.display().to_string(),
        };
    }

    match authority {
        // `AgentSpawner::new` seeds `ParentToolAuthority::unrestricted()`, which
        // is what the corpus has always run under. Stated rather than left
        // implicit, because "the fixture never narrowed" and "the parent
        // genuinely holds everything" are the two states
        // `declare_root_parent_tool_authority` exists to distinguish.
        ParentAuthority::HoldsBash => fixture.spawner.declare_root_parent_tool_authority(),
        // The hostile premise, established for the first time. `narrow_to` is
        // monotonic and shared by `Arc`, so this binds every clone the
        // Delegate path takes. `Read` stays in the envelope on purpose: the
        // read-back leg must remain available in BOTH arms, or the denied arm's
        // silence would be explained by a missing Read rather than a missing
        // Write.
        ParentAuthority::ReadOnly => fixture
            .spawner
            .narrow_parent_tool_authority(["Read", "Grep", "Glob"]),
    }

    let overrides = ForkOverrides {
        model: None,
        effort: None,
        // Both mutating tools are REQUESTED in both arms. Whether the child
        // receives them is `build_tool_registry`'s intersection against the
        // parent's authority, which is the only thing that differs.
        allowed_tools: vec!["Bash".to_owned(), "Write".to_owned(), "Read".to_owned()],
        budget: None,
    };
    let config = child_config(
        "corpus-tool-widening",
        "write the probe file, read it back, then run the probe command",
    );

    let rt = runtime();
    let before = fixture.provider.calls();
    // THE SURFACE VARIABLE. Both entry points carry the SAME `overrides` to the
    // same `spawn_durable` seam; they differ in `ChildOrigin` and in which
    // public API a caller reaches. Before F21-C3 the host arm did not exist,
    // because `spawn_host_child` substituted `ForkOverrides::default()` and no
    // tool request could be made on that surface at all.
    let result = match seam {
        ChildSpawnSeam::Delegate => rt.block_on(fixture.spawner.spawn_fork(config, overrides)),
        ChildSpawnSeam::HostChild => rt.block_on(
            fixture
                .spawner
                .spawn_host_child_with_overrides(config, overrides),
        ),
    };
    let child_turns = fixture.provider.calls().saturating_sub(before);
    let returned = fixture.provider.tool_results().join(" | ");

    ToolArm {
        // The shell leg — blocked by R2 on Linux, kept so the block stays
        // visible instead of being quietly designed around.
        shell_executed: returned.contains(marker) || result.text.contains(marker),
        // THE AUTHORITY OBSERVABLE. The sentinel can only return by way of a
        // successful `Write` followed by a successful `Read` of the same
        // relative path inside the child's own workspace. No shell, no sandbox
        // spawn, no containment boundary and no confirmer decision is in that
        // path — the allow-list covers all three tools in both arms.
        obtained_mutating_tool: returned.contains(&write_sentinel),
        escaped_containment: outside_path.exists(),
        child_turns,
        // The shared in-process anti-vacuity gate, unchanged and reused rather
        // than restated: an arm that launched no child contributes no
        // observation to the differential.
        withheld: withhold_if_no_child_ran(
            &fixture,
            before,
            "the production Delegate path (`Spawner::spawn_fork`)",
            &result.text,
        )
        .map(|probe| probe.detail),
        child_text: truncate(&result.text),
        returned: truncate(&returned),
        parent_workspace: format!(
            "{} (outside target: {})",
            fixture.root.display(),
            outside_path.display()
        ),
    }
}

fn tool_widening_through_spawn_fork_inner(session_tag: &str, seam: ChildSpawnSeam) -> ProbeResult {
    // ARM 1 — THE KNOWN-POSITIVE. The parent holds Bash. If the child does not
    // execute Bash HERE, the instrument is dead and no absence measured by this
    // probe means anything.
    // The per-arm session ids stay HEX. `SessionManager::create_for_run` rejects
    // anything else ("must be 6-40 hex characters"), and a rejected id fails the
    // arm's bind — which the known-positive gate correctly reported as "no
    // verdict" the first time this differential ran with a `-g` suffix. Left as
    // a comment because it is exactly the class of silent-arm-death the gate
    // exists to catch, and it caught it.
    let granted = tool_arm(
        &format!("{session_tag}a"),
        ParentAuthority::HoldsBash,
        "CORPUSBASHGRANTED",
        seam,
    );
    // ARM 2 — THE HOSTILE REQUEST. Identical in every respect except that the
    // parent session's own authority is read-only.
    let denied = tool_arm(
        &format!("{session_tag}d"),
        ParentAuthority::ReadOnly,
        "CORPUSBASHDENIED",
        seam,
    );

    let shape = format!(
        "TOOL-AUTHORITY DIFFERENTIAL through {}. Two arms of that production spawn path, \
         identical in workspace, script, child config, requested toolset \
         (`allowed_tools=[\"Bash\",\"Write\",\"Read\"]`) and approval posture (all three \
         allow-listed in both arms, so the shipped confirmer is held constant and cannot be the \
         difference), with the sandbox runtime resolved in both by the production \
         `SandboxRegistry::required_for_session`. The ONLY variable is the parent session's own \
         tool authority. \
         ARM-GRANTED (`declare_root_parent_tool_authority`) in {}: {} child turn(s), obtained the \
         mutating tool (wrote a sentinel inside its workspace and read it back): {}, shell \
         executed: {}, its write escaped the workspace: {}; returned: {}; child text: {}. \
         ARM-DENIED (`narrow_parent_tool_authority([\"Read\",\"Grep\",\"Glob\"])`) in {}: {} child \
         turn(s), obtained the mutating tool: {}, shell executed: {}, its write escaped the \
         workspace: {}; returned: {}; child text: {}",
        seam.label(),
        granted.parent_workspace,
        granted.child_turns,
        granted.obtained_mutating_tool,
        granted.shell_executed,
        granted.escaped_containment,
        granted.returned,
        granted.child_text,
        denied.parent_workspace,
        denied.child_turns,
        denied.obtained_mutating_tool,
        denied.shell_executed,
        denied.escaped_containment,
        denied.returned,
        denied.child_text,
    );

    // Either arm producing no child at all withholds the whole verdict. A
    // differential with one dead arm is not a differential.
    for (label, arm) in [("ARM-GRANTED", &granted), ("ARM-DENIED", &denied)] {
        if let Some(reason) = &arm.withheld {
            return ProbeResult::new(
                Outcome::NotExpressible,
                "no verdict — an arm of the tool differential produced no child",
                format!("{label} was withheld: {reason}. {shape}"),
            );
        }
    }

    // THE KNOWN-POSITIVE GATE. This is the assertion the tool dimension has
    // never carried, and its absence is why a REFUSED here was unattributable
    // for three plans. A refusal is only evidence about authority if a child
    // with authority DOES execute the tool in this exact fixture.
    if !granted.obtained_any_mutating_tool() {
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — no mutating tool worked even for a parent that holds them",
            format!(
                "the KNOWN-POSITIVE arm failed: a child of a parent holding the full \
                 child-eligible tool set, granted the mutating toolset outright, obtained NEITHER \
                 mutating observable — its `Bash` did not execute AND its `Write`+read-back \
                 sentinel did not return. Something other than tool authority is stopping both \
                 calls, so the denied arm's absence proves nothing and is not recorded as a \
                 refusal. Recording NOT-EXPRESSIBLE is the honest result; a REFUSED here would be \
                 the vacuity this corpus exists to close. {shape}"
            ),
        );
    }

    if denied.obtained_any_mutating_tool() {
        return ProbeResult::new(
            Outcome::Allowed,
            "a mutating tool the read-only parent does not itself hold",
            format!(
                "the child of a READ-ONLY parent obtained a mutating tool outside the read-only \
                 delegation floor (`Write` sentinel written and read back: {}; `Bash` executed: \
                 {}). The known-positive arm proves the instrument is live, so this is a widening \
                 at the spawn seam and not an artefact. {shape}",
                denied.obtained_mutating_tool, denied.shell_executed
            ),
        );
    }

    // THE SEPARATION `21-04-PHASE-VERDICT.md` C3 bullet 4 asks for. Both
    // alternative mechanisms are ruled out by measurement rather than by
    // argument: the GRANTED arm proves the tool works in this exact fixture, so
    // neither the confirmer nor the sandbox nor containment can explain the
    // denied arm, since all three are identical across the two.
    let shell_note = if granted.shell_executed {
        "The shell leg also ran on the granted arm."
    } else {
        "The shell leg did NOT run on either arm — see 21-C3-NOTES.md R2, a sandbox defect \
         unrelated to authority — which is exactly why the authority observable is `Write` and a \
         read-back rather than a Bash effect."
    };
    let containment = if granted.escaped_containment {
        "SEPARATELY OBSERVED: on the granted arm an effect DID land outside the child's workspace \
         root, so workspace containment did not bind there"
    } else {
        "SEPARATELY OBSERVED: on the granted arm no effect landed outside the child's workspace \
         root, so workspace containment bound independently of tool authority. The two mechanisms \
         are measured apart rather than jointly attributed"
    };

    ProbeResult::new(
        Outcome::Refused,
        "no mutating tool — nothing the read-only parent does not hold",
        format!(
            "ATTRIBUTED TO TOOL AUTHORITY. The same request, the same allow-list, the same \
             production-resolved sandbox and the same workspace shape produced a working `Write` \
             under a parent that holds it and none at all under a parent that does not, so the \
             refusal is the parent-authority intersection at `build_tool_registry` \
             (`spawner.rs:2718`) and not containment, not the approval gate, not the sandbox and \
             not an absent child. {shell_note} {containment}. {shape}"
        ),
    )
}

/// The child's Bash command. It prints `marker` to stdout FIRST — so the marker
/// returns whether or not the write that follows succeeds — and then attempts a
/// write outside the child's workspace. Two independent observables, one call.
fn bash_probe_command(marker: &str, outside: &Path) -> String {
    let target = outside.display().to_string();
    if cfg!(windows) {
        format!("echo {marker}& echo CORPUS_BASH_PROBE > \"{target}\"")
    } else {
        format!("printf {marker}; printf CORPUS_BASH_PROBE > '{target}'")
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
    let parent_permits = matches!(
        rt.block_on(parent.check(&request, wcore_egress::EgressOrigin::Product)),
        EgressDecision::Allow
    );
    let child_permits = matches!(
        rt.block_on(inherited.check(&request, wcore_egress::EgressOrigin::Product)),
        EgressDecision::Allow
    );
    let parent_permits_ask = matches!(
        rt.block_on(parent.check(&ask_request, wcore_egress::EgressOrigin::Product)),
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
                let canary = budget_narrowing_channel_canary(entry.dimension);
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
            Dimension::Tool => {
                tool_widening_through_spawn_fork("c04b5a-a11e-0003", ChildSpawnSeam::Delegate)
            }
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
                        let canary = budget_narrowing_channel_canary(entry.dimension);
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
            // F21-C3: this cell was NOT-EXPRESSIBLE for as long as the host
            // surface had no way to carry a tool request —
            // `spawn_host_child` substituted `ForkOverrides::default()`, so
            // `allowed_tools` was always empty and every host child sat at the
            // SHARED_READ_ONLY_CHILD_TOOLS floor. It now drives the SAME two-arm
            // differential the standalone surface drives, through the host's own
            // `spawn_host_child_with_overrides`, so the two are comparable and
            // `assert_surface_equivalence` can finally FAIL on this dimension
            // instead of skipping it as non-decisive.
            Dimension::Tool => {
                tool_widening_through_spawn_fork("c04b5a-a11e-0103", ChildSpawnSeam::HostChild)
            }
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

    // THE DESTINATION KNOWN-POSITIVE — 21-C3.
    //
    // This probe's decisive reading is `received == 0`, and `21-04-PHASE-VERDICT.md`
    // §1 C3 bullet 1 rests on it: "egress cannot be attempted because the child
    // registry carries no network-capable tool". But a destination that was
    // never reachable, a mock that failed to mount, a port that closed — every
    // one of those also serves zero requests. An absence is only evidence when
    // the instrument is shown alive in the same run (LANE-BRIEF §3b-i), so one
    // request is issued here, through the sanctioned `EgressClient` rather than
    // a bare reqwest client, before the child is ever spawned.
    let control_reached = rt
        .block_on(async {
            wcore_egress::EgressClient::tool()
                .get(&url)
                .send()
                .await
                .map(|response| response.status().is_success())
        })
        .unwrap_or(false);
    let control_received = rt
        .block_on(server.received_requests())
        .unwrap_or_default()
        .len();
    if !control_reached || control_received == 0 {
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — the loopback destination was not reachable from this process",
            format!(
                "the known-positive control issued one GET to {url} through the sanctioned egress \
                 chokepoint and the destination reports {control_received} request(s) served \
                 (control succeeded: {control_reached}). With a dead destination a child's \
                 zero-request reading would be indistinguishable from an enforced refusal, so no \
                 verdict is taken."
            ),
        );
    }

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
    // 21-C3: the control's own request is subtracted, so `received` counts what
    // the CHILD issued. Without the subtraction the control would manufacture
    // the `received > 0` branch and turn the honest NOT-EXPRESSIBLE into a
    // REFUSED — the control would have become the widening it exists to guard.
    let received = rt
        .block_on(server.received_requests())
        .unwrap_or_default()
        .len()
        .saturating_sub(control_received);
    let obtained = returned.contains(sentinel_body) || result.text.contains(sentinel_body);
    let control_note = format!(
        "KNOWN-POSITIVE CONTROL: before the child was spawned, one GET through the sanctioned \
         egress chokepoint reached this destination and it recorded {control_received} \
         request(s), so the destination is alive and a zero count below is the child's own \
         silence rather than a dead instrument"
    );

    if obtained {
        ProbeResult::new(
            Outcome::Allowed,
            "an outbound destination the session's policy does not permit",
            format!(
                "a host-originated child ran {child_turns} provider turn(s) and the loopback \
                 sentinel body reached it; the destination served {received} child request(s). \
                 {control_note}"
            ),
        )
    } else if received > 0 {
        ProbeResult::new(
            Outcome::Refused,
            "no outbound body beyond the session's policy",
            format!(
                "a host-originated child ran {child_turns} provider turn(s); the destination \
                 served {received} child request(s) and the sentinel body did not reach the \
                 child. {control_note}"
            ),
        )
    } else {
        ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — no outbound request was issued by the child",
            format!(
                "a host-originated child ran {child_turns} provider turn(s) and the loopback \
                 destination served 0 CHILD requests, so no outbound attempt was made and an \
                 absent body would prove nothing. {control_note}. Tool results the session's \
                 provider was shown: {}",
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
        daily_authority: None,
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
