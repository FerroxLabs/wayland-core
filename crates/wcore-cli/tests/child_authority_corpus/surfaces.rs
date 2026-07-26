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
use wcore_agent::test_utils::ScriptedProvider;
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
}

impl ProbeResult {
    pub fn new(outcome: Outcome, obtained: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            outcome,
            obtained: obtained.into(),
            detail: detail.into(),
            live: None,
        }
    }

    pub fn with_live(mut self, live: LiveEvidence) -> Self {
        self.live = Some(live);
        self
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

/// A hermetic parent: a tempdir workspace, a session directory, and an
/// `AgentSpawner` whose provider replays `script` for every child turn.
pub struct ParentFixture {
    /// Held so the tempdir outlives every probe that reads from it.
    pub _home: TempDir,
    pub root: PathBuf,
    pub spawner: Arc<AgentSpawner>,
    /// `Some(reason)` when the durable session could not be bound. A driver
    /// that needs a launch reports NOT-EXPRESSIBLE rather than reporting a
    /// refusal it did not actually observe.
    pub bind_failure: Option<String>,
}

pub fn parent_fixture(session_tag: &str, script: Vec<LlmEvent>) -> ParentFixture {
    let home = TempDir::new().expect("tempdir");
    let root = std::fs::canonicalize(home.path()).expect("canonical workspace root");
    let sessions = root.join("sessions");
    std::fs::create_dir_all(&sessions).expect("session directory");

    let mut config = Config::default();
    config.session.directory = sessions.to_string_lossy().into_owned();

    let provider: Arc<dyn LlmProvider> = Arc::new(ScriptedProvider::new(script));
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
        root,
        spawner: Arc::new(spawner),
        bind_failure,
    }
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
pub fn budget_no_channel_canary() -> String {
    let callers = source_files_mentioning("sub_budget(Some(");
    if callers.is_empty() {
        "NO-CHANNEL canary intact: no crates/*/src file forwards a Some(..) override into \
         sub_budget"
            .to_owned()
    } else {
        format!(
            "NO-CHANNEL CANARY TRIPPED: a production file now forwards a Some(..) budget override \
             into sub_budget: {}",
            callers.join(", ")
        )
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
    };
    let config = child_config(
        "corpus-tool-widening",
        "run the probe command with the Bash tool",
    );

    let rt = runtime();
    let result = rt.block_on(fixture.spawner.spawn_fork(config, overrides));

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
                "the delegated child produced no Bash effect on disk; parent workspace {}; \
                 child text: {}",
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

/// Production sites that construct an `EgressClient` directly. An occurrence
/// after the file's first `#[cfg(test)]` is test code — the convention this
/// workspace follows without exception in the files the census checked by hand —
/// and is excluded, so the canary fires on a real bypass route rather than on a
/// test fixture.
fn production_egress_client_sites() -> Vec<String> {
    let mut sites = Vec::new();
    let root = workspace_root();
    for file in source_files_mentioning("EgressClient::new()") {
        let Ok(text) = std::fs::read_to_string(root.join(&file)) else {
            continue;
        };
        let test_boundary = text
            .lines()
            .position(|line| line.trim_start().starts_with("#[cfg(test)]"))
            .unwrap_or(usize::MAX);
        let production = text
            .lines()
            .enumerate()
            .any(|(index, line)| index < test_boundary && line.contains("EgressClient::new()"));
        if production {
            sites.push(file);
        }
    }
    sites
}

/// Breadth beyond the parent's cap, attempted through the real `SpawnTool`
/// against the real topology configuration. The invariant is measured from the
/// number of children the request actually produced, not from any refusal text.
pub fn fan_out_probe(session_tag: &str) -> ProbeResult {
    let fixture = parent_fixture(session_tag, done_script());
    let tool = SpawnTool::new(Arc::clone(&fixture.spawner));
    // Topology::Spawn is the default SpawnTool topology and its documented cap.
    let cap = 5usize;
    let over_cap = cap + 3;

    let tasks: Vec<serde_json::Value> = (0..over_cap)
        .map(|i| json!({ "name": format!("corpuschild{i}"), "prompt": "no-op" }))
        .collect();

    let rt = runtime();
    let result = rt.block_on(tool.execute(json!({ "tasks": tasks })));

    // The discriminator is whether the over-cap request produced over-cap work.
    // A refused request never reaches a child; an accepted one reports each
    // child it ran by name.
    let named = (0..over_cap)
        .filter(|i| result.content.contains(&format!("corpuschild{i}")))
        .count();

    if named > cap {
        ProbeResult::new(
            Outcome::Allowed,
            format!("breadth of {named} children against a parent cap of {cap}"),
            format!(
                "a batch of {over_cap} was requested against the Spawn topology cap of {cap} and \
                 {named} children were reported"
            ),
        )
    } else {
        ProbeResult::new(
            Outcome::Refused,
            format!("no breadth beyond the parent cap of {cap}"),
            format!(
                "a batch of {over_cap} was requested against the Spawn topology cap of {cap}; \
                 {named} children were reported"
            ),
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
                let mut probe = budget_probe(entry.dimension, &child);
                probe.detail = format!(
                    "ExecutionBudgetView::sub_budget(Some(wider)) at the child-spawn seam; {}; {}",
                    probe.detail,
                    budget_no_channel_canary()
                );
                probe
            }
            Dimension::Provider => {
                let fixture = parent_fixture("corpus-standalone-provider", Vec::new());
                provider_no_channel_canary(Arc::clone(&fixture.spawner))
            }
            Dimension::Approval => approval_no_channel_canary(),
            Dimension::Tool => tool_widening_through_spawn_fork("corpus-standalone-tool"),
            Dimension::Filesystem => filesystem_escape_probe(),
            Dimension::Secret => secret_read_probe(),
            Dimension::Egress => egress_probe(),
            Dimension::FanOut => fan_out_probe("corpus-standalone-fanout"),
        }
    }
}

// ===========================================================================
// Driver 2 — host protocol, in process
// ===========================================================================

/// The host-protocol in-process driver.
///
/// `wcore_protocol::commands::ProtocolCommand` exposes NO direct child-spawn
/// command. The host reaches children two ways: through `Message`, after which
/// the model issues `Delegate`/`Spawn`, and through the in-process
/// `HostChildController` that `AgentBootstrap` hands the front-end
/// (`bootstrap.rs:2226`). Both land on the same `AgentSpawner`. So equivalence
/// IS constructible — termination state 2 does not apply — but the protocol
/// surface reaches the seam through the session/turn authority the front-end
/// binds rather than through a raw `sub_budget` at the spawn seam. That
/// distinction is what keeps the cross-surface comparison from being a
/// tautology on the budget family: `BudgetAuthorityCoordinator::begin_active_turn`
/// here against `ExecutionBudgetView::sub_budget` on the standalone side.
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
                        let mut probe = budget_probe(entry.dimension, &child);
                        probe.detail = format!(
                            "BudgetAuthorityCoordinator::begin_active_turn(turn, Some(wider)) — \
                             the session/turn seam the protocol front-end drives; {}; {}",
                            probe.detail,
                            budget_no_channel_canary()
                        );
                        probe
                    }
                    Err(reason) => ProbeResult::new(
                        Outcome::NotExpressible,
                        "no resource was requested",
                        format!("the session budget authority could not be bound: {reason}"),
                    ),
                }
            }
            // Both surfaces reach the same schemas, the same resolver, the same
            // VFS stack, the same chokepoint and the same spawner. Driving them
            // from the protocol-bound path is the point of the comparison: if
            // the two ever diverge, the weaker path is a bypass of the stronger
            // and the property is false overall.
            Dimension::Provider => {
                let fixture = parent_fixture("corpus-protocol-provider", Vec::new());
                provider_no_channel_canary(Arc::clone(&fixture.spawner))
            }
            Dimension::Approval => approval_no_channel_canary(),
            Dimension::Tool => tool_widening_through_spawn_fork("corpus-protocol-tool"),
            Dimension::Filesystem => filesystem_escape_probe(),
            Dimension::Secret => secret_read_probe(),
            Dimension::Egress => egress_probe(),
            Dimension::FanOut => fan_out_probe("corpus-protocol-fanout"),
        }
    }
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
