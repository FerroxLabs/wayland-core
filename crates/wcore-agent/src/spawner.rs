use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use async_trait::async_trait;

use wcore_config::config::Config;
use wcore_protocol::events::WorkflowChildTerminalState;
use wcore_providers::LlmProvider;
use wcore_swarm::worktree::{TransactionWorkspace, WorktreeManager};
use wcore_swarm::{
    AgentReport, BlackboardCtx, DEFAULT_SHARD_SIZE, FleetDispatcher, FleetReducer, MeshAgent,
    ShardSummary,
};
use wcore_tools::bash::BashTool;
use wcore_tools::edit::EditTool;
use wcore_tools::glob::GlobTool;
use wcore_tools::grep::GrepTool;
use wcore_tools::read::ReadTool;
use wcore_tools::registry::ToolRegistry;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs};
use wcore_tools::workspace_policy::WorkspacePolicy;
use wcore_tools::write::WriteTool;
use wcore_types::execution_policy::EffectiveExecutionPolicy;
use wcore_types::message::{FinishReason, TokenUsage};
use wcore_types::spawner::{
    ChildDeliveryState, ChildDesiredState, ChildId, ChildOrigin, ChildParent, ChildPolicySnapshot,
    ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus, RequestedChildWorkspace,
    SHARED_READ_ONLY_CHILD_TOOLS,
};

use crate::agents::bus::{AgentBus, AgentMessage, now_ms, preview};
use crate::agents::channel_sink::ChannelSink;
use crate::engine::AgentEngine;
use crate::orchestration::council::ProviderResolver;
use crate::output::OutputSink;
use crate::output::null_sink::NullSink;
use crate::session_journal::SessionJournal;

// Re-export from wcore-types — single source of truth
pub use wcore_types::spawner::{ForkOverrides, Spawner, SubAgentConfig, SubAgentResult};

pub use crate::durable_spawner::{
    DurableAuthorityFailure, DurableAuthorityToken, DurableCancelDisposition,
    DurableChildSupervisor, DurableSessionAuthority, DurableSpawner, DurableSpawnerError,
    DurableSpawnerPoison,
};

const CHILD_POLICY_CONTRACT_VERSION: &str = "effective-execution-policy/v1";

/// Narrow host-facing control plane over the canonical bootstrapped child
/// runtime.
///
/// Bootstrap constructs this from the same [`Arc<AgentSpawner>`] installed in
/// Spawn, workflow, Crucible, and Anvil paths. Keeping that identity is
/// required because live cancellation tokens are owned by the durable spawner,
/// not by the journal-backed child store alone.
#[derive(Clone)]
pub struct HostChildController {
    spawner: Arc<AgentSpawner>,
}

impl HostChildController {
    pub(crate) fn new(spawner: Arc<AgentSpawner>) -> Self {
        Self { spawner }
    }

    /// Create host-originated child work through the canonical durable path.
    pub async fn spawn_child(&self, config: SubAgentConfig) -> SubAgentResult {
        self.spawner.spawn_host_child(config).await
    }

    /// Pin supervision to the currently bound session generation.
    pub fn supervisor(&self) -> Result<DurableChildSupervisor, DurableSpawnerError> {
        self.spawner.durable_child_supervisor()
    }
}

struct ResolvedProvider {
    provider: Arc<dyn LlmProvider>,
    provider_id: String,
    model: Option<String>,
}

struct PreResolvedChildLaunch {
    request: SubAgentConfig,
    overrides: ForkOverrides,
    provider: Arc<dyn LlmProvider>,
    provider_id: String,
    model: String,
    config: Config,
    policy: ChildPolicySnapshot,
    requested_workspace: RequestedChildWorkspace,
    authority: DurableAuthorityToken,
    parent_cancel: tokio_util::sync::CancellationToken,
}

/// Fully resolved execution inputs for one durable child launch.
///
/// The provider and child config are selected once. Durable declaration and
/// execution consume this value rather than re-reading mutable session state.
pub struct ResolvedChildLaunch {
    child_id: ChildId,
    request: SubAgentConfig,
    overrides: ForkOverrides,
    provider: Arc<dyn LlmProvider>,
    provider_id: String,
    model: String,
    config: Config,
    policy: ChildPolicySnapshot,
    requested_workspace: RequestedChildWorkspace,
    workspace: ChildWorkspace,
    workspace_root: PathBuf,
    authority_read_deny: Vec<PathBuf>,
    authority: DurableAuthorityToken,
    parent_cancel: tokio_util::sync::CancellationToken,
    /// Identity-bound lifecycle handle for a mutating child's standalone
    /// checkout. It is owned here so the checkout stays live for the child's
    /// whole execution and terminalizes exactly once when this launch is
    /// dropped (child terminal, cancellation, or pre-launch failure). `None`
    /// for a shared read-only child. Never cloned and never leaked.
    _transaction_workspace: Option<TransactionWorkspace>,
}

impl ResolvedChildLaunch {
    #[must_use]
    pub fn child_id(&self) -> &ChildId {
        &self.child_id
    }

    #[must_use]
    pub fn provider(&self) -> Arc<dyn LlmProvider> {
        Arc::clone(&self.provider)
    }

    #[must_use]
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    #[must_use]
    pub fn policy_snapshot(&self) -> &ChildPolicySnapshot {
        &self.policy
    }

    /// Workspace authority requested by the child tool set.
    #[must_use]
    pub fn requested_workspace(&self) -> RequestedChildWorkspace {
        self.requested_workspace
    }

    /// Workspace the runtime will actually use for this launch.
    #[must_use]
    pub fn workspace(&self) -> &ChildWorkspace {
        &self.workspace
    }

    /// Canonical execution root. It is intentionally excluded from durable
    /// records because host paths can contain user and repository names.
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    #[must_use]
    pub fn authority(&self) -> &DurableAuthorityToken {
        &self.authority
    }

    pub fn validate_record(&self, record: &DurableChildRecord) -> Result<(), DurableSpawnerError> {
        if record.child_id != self.child_id {
            return Err(DurableSpawnerError::EvidenceMismatch("child identity"));
        }
        if record.parent.session_id != self.authority.session_id() {
            return Err(DurableSpawnerError::EvidenceMismatch("parent session"));
        }
        if record.request.exact_digest()
            != DurableSpawner::request_digest(&self.request, &self.overrides)?
        {
            return Err(DurableSpawnerError::EvidenceMismatch("request digest"));
        }
        if record.provider.as_deref() != Some(self.provider_id.as_str()) {
            return Err(DurableSpawnerError::EvidenceMismatch("provider"));
        }
        if record.model.as_deref() != Some(self.model.as_str()) {
            return Err(DurableSpawnerError::EvidenceMismatch("model"));
        }
        if record.policy_snapshot != self.policy {
            return Err(DurableSpawnerError::EvidenceMismatch("policy snapshot"));
        }
        if record.workspace != self.workspace {
            return Err(DurableSpawnerError::EvidenceMismatch("workspace"));
        }
        Ok(())
    }

    fn durable_record(
        &self,
        origin: ChildOrigin,
        parent_call_id: Option<String>,
    ) -> Result<DurableChildRecord, DurableSpawnerError> {
        let now = crate::durable_spawner::now_unix_ms()?;
        Ok(DurableChildRecord {
            schema_version: DURABLE_CHILD_SCHEMA_VERSION,
            declaration_id: format!("declare-{}", uuid::Uuid::new_v4().simple()),
            parent: ChildParent {
                session_id: self.authority.session_id().to_owned(),
                turn_id: None,
                parent_child_id: None,
                workflow_run_id: None,
                graph_node_id: None,
                parent_call_id,
            },
            origin,
            request: ChildRequestEvidence::redacted(DurableSpawner::request_digest(
                &self.request,
                &self.overrides,
            )?),
            policy_snapshot: self.policy.clone(),
            provider: Some(self.provider_id.clone()),
            model: Some(self.model.clone()),
            workspace: self.workspace.clone(),
            child_id: self.child_id.clone(),
            status: DurableChildStatus::Prepared,
            desired_state: ChildDesiredState::Run,
            recovery: ChildRecoveryState::Clean,
            revision: 0,
            timestamps: ChildTimestamps {
                created_at_unix_ms: now,
                updated_at_unix_ms: now,
                queued_at_unix_ms: None,
                started_at_unix_ms: None,
                terminal_at_unix_ms: None,
            },
            result: None,
            delivery_target: None,
            delivery_state: ChildDeliveryState::NotRequired,
            attempt: 1,
            retry_of: None,
            applied_events: Default::default(),
        })
    }
}

fn child_policy_snapshot(
    policy: &EffectiveExecutionPolicy,
) -> Result<ChildPolicySnapshot, DurableSpawnerError> {
    let value = serde_json::to_value(policy)
        .map_err(|_| DurableSpawnerError::EvidenceMismatch("effective policy encoding"))?;
    let field = |name: &'static str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or(DurableSpawnerError::EvidenceMismatch(name))
    };
    let dangerous_activation_id_digest = value
        .get("dangerous_activation_id")
        .and_then(serde_json::Value::as_str)
        .map(|activation_id| {
            crate::session_journal::state_payload_digest(&serde_json::json!(activation_id))
        })
        .transpose()?;
    Ok(ChildPolicySnapshot {
        contract_version: CHILD_POLICY_CONTRACT_VERSION.to_owned(),
        exact_digest: crate::session_journal::state_payload_digest(&value)?,
        posture: field("posture")?,
        approvals: field("approvals")?,
        sandbox: field("sandbox")?,
        source: field("source")?,
        managed_floor_active: value
            .get("managed_floor_active")
            .and_then(serde_json::Value::as_bool)
            .ok_or(DurableSpawnerError::EvidenceMismatch(
                "managed_floor_active",
            ))?,
        dangerous_activation_id_digest,
    })
}

fn allocate_child_id() -> Result<ChildId, DurableSpawnerError> {
    ChildId::new(format!("child-{}", uuid::Uuid::new_v4().simple()))
        .map_err(|_| DurableSpawnerError::EvidenceMismatch("child identity"))
}

fn shared_child_workspace(parent: &Path) -> Result<ChildWorkspace, DurableSpawnerError> {
    let workspace_digest = crate::session_journal::state_payload_digest(&serde_json::json!({
        "workspace": parent.to_string_lossy(),
    }))?;
    Ok(ChildWorkspace {
        mode: ChildWorkspaceMode::SharedReadOnly,
        workspace_id: format!("shared-{workspace_digest}"),
    })
}

fn write_workspace_preparation_lease(
    control_root: &Path,
    child_id: &ChildId,
    checkout_root: &Path,
    pinned_head: &str,
) -> Result<(), DurableSpawnerError> {
    let leases = control_root.join("leases");
    std::fs::create_dir_all(&leases).map_err(|error| {
        DurableSpawnerError::WorkspacePreparation(format!(
            "could not create workspace lease directory: {error}"
        ))
    })?;
    make_private_directory(control_root)?;
    make_private_directory(&leases)?;
    let final_path = leases.join(format!("{}.json", child_id.as_str()));
    let mut temporary = tempfile::NamedTempFile::new_in(&leases).map_err(|error| {
        DurableSpawnerError::WorkspacePreparation(format!(
            "could not create workspace preparation lease: {error}"
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                DurableSpawnerError::WorkspacePreparation(format!(
                    "could not protect workspace preparation lease: {error}"
                ))
            })?;
    }
    serde_json::to_writer(
        temporary.as_file_mut(),
        &serde_json::json!({
            "schema": "wayland-child-workspace-lease/v1",
            "child_id": child_id.as_str(),
            "workspace_id": format!("isolated-{}", child_id.as_str()),
            "checkout_root": checkout_root,
            "pinned_head": pinned_head,
            "state": "preparing"
        }),
    )
    .map_err(|error| {
        DurableSpawnerError::WorkspacePreparation(format!(
            "could not encode workspace preparation lease: {error}"
        ))
    })?;
    temporary.flush().map_err(|error| {
        DurableSpawnerError::WorkspacePreparation(format!(
            "could not flush workspace preparation lease: {error}"
        ))
    })?;
    temporary.as_file().sync_all().map_err(|error| {
        DurableSpawnerError::WorkspacePreparation(format!(
            "could not sync workspace preparation lease: {error}"
        ))
    })?;
    temporary.persist_noclobber(&final_path).map_err(|error| {
        DurableSpawnerError::WorkspacePreparation(format!(
            "workspace preparation lease {} already exists or could not be persisted: {}",
            final_path.display(),
            error.error
        ))
    })?;
    sync_directory(&leases)?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), DurableSpawnerError> {
    #[cfg(unix)]
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            DurableSpawnerError::WorkspacePreparation(format!(
                "could not sync workspace lease directory: {error}"
            ))
        })?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn make_private_directory(path: &Path) -> Result<(), DurableSpawnerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(
            |error| {
                DurableSpawnerError::WorkspacePreparation(format!(
                    "could not protect orchestrator workspace state: {error}"
                ))
            },
        )?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

struct PreparedChildWorkspace {
    evidence: ChildWorkspace,
    root: PathBuf,
    authority_read_deny: Vec<PathBuf>,
    /// Retained standalone-checkout authority for a mutating child. `None` for a
    /// shared read-only child (which owns no transaction). When present, this
    /// handle owns the checkout on disk; dropping it terminalizes the
    /// transaction exactly once and cleans the checkout.
    transaction: Option<TransactionWorkspace>,
}

type WorkspaceAdmissionGate = tokio::sync::Mutex<()>;

fn workspace_admission_gate(
    control_root: &Path,
) -> Result<Arc<WorkspaceAdmissionGate>, DurableSpawnerError> {
    static GATES: OnceLock<
        Mutex<std::collections::HashMap<PathBuf, Weak<WorkspaceAdmissionGate>>>,
    > = OnceLock::new();
    let mut gates = GATES
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
        .map_err(|_| {
            DurableSpawnerError::WorkspacePreparation(
                "workspace admission registry is unavailable".to_owned(),
            )
        })?;
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(control_root).and_then(Weak::upgrade) {
        return Ok(gate);
    }
    let gate = Arc::new(WorkspaceAdmissionGate::new(()));
    gates.insert(control_root.to_path_buf(), Arc::downgrade(&gate));
    Ok(gate)
}

/// Name of the orchestrator control directory that `WorktreeManager` plants in
/// its swarm root (the delegated-checkouts root). It is not a child checkout and
/// must be excluded from retention accounting. Mirrors `wcore_swarm`'s internal
/// control-directory invariant, which the spawner already depends on when it
/// composes the `<worker>/checkout` child working directory.
const SWARM_CONTROL_DIR: &str = ".wayland-control";

fn retained_workspace_allocation_count(
    leases: &Path,
    checkouts: &Path,
    stop_after: usize,
) -> Result<usize, DurableSpawnerError> {
    let mut identities = HashSet::<OsString>::new();
    if leases.exists() {
        for entry in std::fs::read_dir(leases).map_err(|error| {
            DurableSpawnerError::WorkspacePreparation(format!(
                "could not enumerate workspace leases: {error}"
            ))
        })? {
            let entry = entry.map_err(|error| {
                DurableSpawnerError::WorkspacePreparation(format!(
                    "could not inspect workspace lease: {error}"
                ))
            })?;
            let metadata = std::fs::symlink_metadata(entry.path()).map_err(|error| {
                DurableSpawnerError::WorkspacePreparation(format!(
                    "could not inspect workspace lease metadata: {error}"
                ))
            })?;
            if !metadata.file_type().is_file()
                || entry.path().extension().and_then(|v| v.to_str()) != Some("json")
            {
                return Err(DurableSpawnerError::WorkspacePreparation(format!(
                    "refused unsafe workspace lease entry: {}",
                    entry.path().display()
                )));
            }
            let identity = entry
                .path()
                .file_stem()
                .map(OsString::from)
                .ok_or_else(|| {
                    DurableSpawnerError::WorkspacePreparation(
                        "workspace lease has no child identity".to_owned(),
                    )
                })?;
            identities.insert(identity);
            if identities.len() > stop_after {
                return Ok(identities.len());
            }
        }
    }
    if checkouts.exists() {
        for entry in std::fs::read_dir(checkouts).map_err(|error| {
            DurableSpawnerError::WorkspacePreparation(format!(
                "could not enumerate delegated checkouts: {error}"
            ))
        })? {
            let entry = entry.map_err(|error| {
                DurableSpawnerError::WorkspacePreparation(format!(
                    "could not inspect delegated checkout: {error}"
                ))
            })?;
            // The checkouts root doubles as the `WorktreeManager` swarm root, so
            // the orchestrator plants its own control directory
            // (`.wayland-control`) there alongside child checkouts. It is
            // infrastructure, not a retained child allocation; the canonical
            // `WorktreeManager::retained_worker_count` skips it the same way.
            // Counting it here inflates the quota by one and would reject a
            // near-cap admission that should be admitted.
            if entry.file_name().to_str() == Some(SWARM_CONTROL_DIR) {
                continue;
            }
            let metadata = std::fs::symlink_metadata(entry.path()).map_err(|error| {
                DurableSpawnerError::WorkspacePreparation(format!(
                    "could not inspect delegated checkout metadata: {error}"
                ))
            })?;
            if !metadata.file_type().is_dir() {
                return Err(DurableSpawnerError::WorkspacePreparation(format!(
                    "refused unsafe delegated checkout entry: {}",
                    entry.path().display()
                )));
            }
            identities.insert(entry.file_name());
            if identities.len() > stop_after {
                return Ok(identities.len());
            }
        }
    }
    Ok(identities.len())
}

/// #661 (fail-loud) — build a [`SubAgentResult`] from a sub-agent's terminal
/// [`AgentResult`](crate::engine::AgentResult).
///
/// A run that terminated abnormally — the turn cap, a budget/context ceiling,
/// the retry-cap guardrail, or the runaway-loop breaker — returns `Ok` with
/// empty text and a non-`Stop` finish reason. Copying that into
/// `is_error: false` made the parent LLM read it as "the sub-agent completed and
/// found nothing", so it reasoned from false info. Instead derive `is_error`
/// from the finish reason, and when the terminated body is empty synthesize a
/// cause line so the failure is legible rather than a silent empty success.
fn subagent_ok_result(name: String, result: crate::engine::AgentResult) -> SubAgentResult {
    // A clean EndTurn is `Stop`. `MaxTurns`/`Error` are unambiguous abnormal
    // terminations. `Length` is ambiguous: a run aborted at the context/budget
    // ceiling returns `Length` with EMPTY text (a real failure), but a complete
    // answer that ends exactly at the output-token cap also returns `Length`
    // WITH usable text — a degraded-but-usable answer, not a failure. Flagging
    // the latter would wrongly drop it from council quorum (is_usable), so treat
    // a non-empty `Length` as success; only an empty `Length` is an error.
    let is_error = match result.finish_reason {
        FinishReason::Stop => false,
        FinishReason::Length => result.text.trim().is_empty(),
        FinishReason::MaxTurns | FinishReason::Error => true,
    };
    let text = if is_error && result.text.trim().is_empty() {
        format!(
            "[sub-agent terminated without completing its task: {}]",
            describe_finish_reason(result.finish_reason)
        )
    } else {
        result.text
    };
    SubAgentResult {
        name,
        text,
        usage: result.usage,
        turns: result.turns,
        is_error,
    }
}

fn relay_subagent_terminal(sink: Option<&ChannelSink>, result: &SubAgentResult) {
    let Some(sink) = sink else {
        return;
    };
    let terminal_state = if result.is_error {
        WorkflowChildTerminalState::Failed
    } else {
        WorkflowChildTerminalState::Succeeded
    };
    let terminal_message = if result.is_error {
        result.text.clone()
    } else {
        format!(
            "sub-agent '{}' completed ({} turns)",
            result.name, result.turns
        )
    };
    sink.relay_terminal(terminal_state, &terminal_message);
}

/// Human-readable cause for an abnormal sub-agent termination.
fn describe_finish_reason(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::MaxTurns => "reached the turn limit before finishing",
        FinishReason::Length => "hit a context, budget, or output-length limit",
        FinishReason::Error => "ended with an error",
        // Not reachable from the error branch (Stop == clean completion), but
        // keep the match total.
        FinishReason::Stop => "stopped",
    }
}

/// v0.8.0 Task J — preview cap for `AgentMessage::FirstMessage.content_preview`.
/// Kept small so a chatty parent's prompts don't bloat the broadcast
/// channel; subscribers that need the full prompt can correlate via the
/// agent name + parent_call_id and look it up out-of-band.
const FIRST_MESSAGE_PREVIEW_CHARS: usize = 200;

/// W7 F2 sibling-parameter for `spawn_parallel`. Lives in `wcore-agent`
/// (NOT `wcore-types`) because `ChannelSink` wraps a tokio mpsc Sender —
/// the dep would reverse the crate-dep graph if hung off `SubAgentConfig`.
/// One `SpawnExtras` per `spawn_parallel_with_extras` call; per-task
/// fields (if needed later) can move into a `Vec<SpawnExtras>` indexed-
/// by-config — flagged for W8+.
#[derive(Clone, Default)]
pub struct SpawnExtras {
    /// When `Some`, the sub-agent's engine uses this sink instead of `NullSink`.
    /// Parent's `parent_call_id` is captured in the `ChannelSink` itself.
    pub channel_sink: Option<Arc<ChannelSink>>,
    /// Optional friendly-name forwarded into `SubAgentResult.name` so the parent
    /// can correlate relays with their originating spawn task.
    pub agent_name: Option<String>,
    /// Parent's `call_id` for the `SpawnTool` invocation — used by the
    /// parent-side drain task when wrapping `SubAgentRelay` in `SubAgentEvent`.
    pub parent_call_id: Option<String>,
}

/// v0.8.0 Task J — small RAII helper that ensures every spawn path
/// publishes exactly one terminal lifecycle event. The spawner builds
/// one of these immediately after `Spawned` is published; on drop with
/// the default `outcome` it logs a `Errored("dropped")` so a panic in
/// the engine can't leave subscribers waiting for a terminal event.
/// Successful spawn paths overwrite the outcome before drop.
struct LifecycleGuard {
    bus: Option<Arc<AgentBus>>,
    agent: String,
    outcome: TerminalOutcome,
}

/// Owns parallel child tasks so dropping a parent dispatch future aborts the
/// children instead of detaching them onto the Tokio runtime.
struct SpawnTaskSet(Vec<tokio::task::JoinHandle<SubAgentResult>>);

impl Drop for SpawnTaskSet {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

#[derive(Debug, Clone)]
enum TerminalOutcome {
    /// Default — nothing fired yet. Drop publishes `Errored("dropped before completion")`.
    Pending,
    /// Spawner already published `Completed` / `Errored` — drop is a no-op.
    Published,
}

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        if let (Some(bus), TerminalOutcome::Pending) = (&self.bus, &self.outcome) {
            bus.publish(AgentMessage::Errored {
                agent: self.agent.clone(),
                error: "sub-agent dropped before completion".to_string(),
            });
        }
    }
}

/// Keeps aggregate child depth durable for the whole child lifetime. The
/// in-process guard updates shared counters; this wrapper commits its release
/// before the child scope disappears.
struct ChildBudgetGuard {
    guard: Option<wcore_budget::AgentDepthGuard>,
    authority: Option<crate::budget_authority::SharedBudgetAuthorityCoordinator>,
}

impl Drop for ChildBudgetGuard {
    fn drop(&mut self) {
        let Some(authority) = self.authority.as_ref() else {
            self.guard.take();
            return;
        };
        let guard = self.guard.take();
        if let Err(error) = authority.lock().transaction(|_| drop(guard)) {
            tracing::error!(error = %error, "durable child budget release failed");
        }
    }
}

/// Spawns independent child agents that share the parent's LLM provider.
///
/// Sub-agents use a [`NullSink`] so their streaming output is silently
/// discarded.  Results are collected via `engine.run()` and returned to the
/// parent which emits them as a single `tool_result` event — matching the
/// Claude Code pattern where only the parent writes to stdout.
pub struct AgentSpawner {
    provider: Arc<dyn LlmProvider>,
    base_config: Config,
    /// Immutable sandbox selected by the parent session. Every child registry
    /// receives this exact `Arc`; spawning must never re-read process-global
    /// sandbox settings or select a different backend mid-session.
    sandbox_runtime: Arc<wcore_sandbox::SandboxRegistry>,
    /// Canonical parent workspace supplied by Bootstrap. Process-global cwd is
    /// never session authority: parallel sessions and child execution may
    /// legitimately have different roots in the same process.
    parent_workspace: Option<Arc<PathBuf>>,
    /// Immutable outbound-network authority inherited from the parent session.
    /// Child engines must never fall back to a process-global compatibility
    /// policy after the bootstrap task-local scope has exited.
    egress_policy: wcore_egress::SharedPolicy,
    /// Shared live posture authority for host-backed sessions. Read only when
    /// deriving a child config so runtime de-escalation applies to descendants
    /// that have not started yet.
    approval_manager: Option<Arc<wcore_protocol::ToolApprovalManager>>,
    /// v0.8.0 Task J — optional `AgentBus` for lifecycle event
    /// publication. `None` preserves the legacy "silent spawner"
    /// behaviour expected by older tests; production callers attach the
    /// engine's bus via `with_bus(...)`.
    bus: Option<Arc<AgentBus>>,
    /// Parent cancellation token. Every spawned child engine is bound to a
    /// `child_token()` of this, so a host cancel (Esc) propagates into running
    /// sub-agents and they stop at the next turn boundary instead of burning
    /// LLM calls to completion. Defaults to a detached, never-cancelled token
    /// for legacy callers; production attaches the engine's token via
    /// `with_cancel(...)`.
    cancel: tokio_util::sync::CancellationToken,
    /// Production session handle. Reads the active turn token at spawn time,
    /// so both per-turn host cancellation and immutable session expiry reach
    /// children. Legacy/test spawners continue to use `cancel`.
    session_runtime: Option<crate::cancel::SessionRuntimeHandle>,
    /// Crucible (Mixture-of-Providers) — optional resolver that turns a
    /// per-spawn `SubAgentConfig.provider` spec into a keyed provider. `None`
    /// (the default) preserves single-provider behaviour: every child inherits
    /// `self.provider`. Production bootstrap attaches a `CouncilProviderResolver`
    /// via `with_provider_resolver(...)`. MUST be propagated by
    /// `clone_for_spawn` or fleet/parallel proposers silently fall back to the
    /// parent provider (the cross-provider-diversity guard catches this).
    resolver: Option<Arc<dyn ProviderResolver>>,
    /// Crucible cost governance — the per-session/per-day spend tracker shared
    /// with the engine. `None` ⇒ no aggregate cap (the council enforces only its
    /// per-run pin). MUST be propagated by `clone_for_spawn`.
    budget_tracker: Option<Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>>,
    /// (session_id, user_id) the council charges against — same envelope as the
    /// parent turn. None ⇒ council spend is not charged. Propagated by clone_for_spawn.
    budget_identity: Option<(String, String)>,
    /// Provider-call admission tracker shared by the parent engine and every
    /// spawned child. This is distinct from the cap-less Crucible accumulator
    /// above: it enforces the finite session token/cost envelope.
    provider_budget_tracker: Option<Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>>,
    /// Sole production provider/execution authority. Raw tracker/view fields
    /// remain only for legacy and isolated tests.
    budget_authority: Option<crate::budget_authority::SharedBudgetAuthorityCoordinator>,
    /// Stable provider-budget identity shared with every child engine.
    budget_session_id: Option<String>,
    /// Parent execution envelope. Spawn/fork paths derive child views from it
    /// so token, cost, process, runtime, and active-agent usage roll up.
    execution_budget: Option<wcore_budget::ExecutionBudgetView>,
    /// Keeps the standalone session's budget watcher alive for exactly as
    /// long as the spawner and its clones can dispatch child work.
    budget_guard: Option<Arc<crate::cancel::BudgetGuard>>,
    /// Clone-shared journal authority. It is deliberately unbound while tools
    /// are built and is bound only after the engine owns a canonical session.
    durable_authority: DurableSessionAuthority,
    /// Resolver-produced session policy used to derive redacted child evidence.
    effective_policy: EffectiveExecutionPolicy,
    /// Clone-shared admission boundary for child engines started by parallel
    /// Spawn, workflow, mesh, swarm, and fleet paths. Topology limits describe
    /// how much work may be scheduled; this semaphore independently bounds the
    /// number of full child engines that may own resources at once.
    active_child_permits: Arc<tokio::sync::Semaphore>,
}

/// Provider-spend, execution, and cancellation authority inherited by a
/// transient spawner that is created after session bootstrap (for example a
/// Council judge or an Anvil seat). Keeping these handles together prevents a
/// convenience constructor from silently minting a fresh budget.
#[derive(Clone)]
pub struct SpawnerBudgetGovernance {
    provider_budget_tracker: Option<Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>>,
    budget_authority: Option<crate::budget_authority::SharedBudgetAuthorityCoordinator>,
    budget_session_id: String,
    execution_budget: Option<wcore_budget::ExecutionBudgetView>,
    cancel: tokio_util::sync::CancellationToken,
    budget_guard: Option<Arc<crate::cancel::BudgetGuard>>,
    active_child_permits: Arc<tokio::sync::Semaphore>,
}

impl SpawnerBudgetGovernance {
    pub fn new(
        provider_budget_tracker: Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>,
        budget_session_id: impl Into<String>,
        execution_budget: wcore_budget::ExecutionBudgetView,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            provider_budget_tracker: Some(provider_budget_tracker),
            budget_authority: None,
            budget_session_id: budget_session_id.into(),
            execution_budget: Some(execution_budget),
            cancel,
            budget_guard: None,
            active_child_permits: Arc::new(tokio::sync::Semaphore::new(
                wcore_swarm::MAX_CONCURRENT_WORKERS,
            )),
        }
    }

    pub(crate) fn from_authority(
        authority: crate::budget_authority::SharedBudgetAuthorityCoordinator,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Self {
        let budget_session_id = authority.lock().budget_session_id().to_owned();
        Self {
            provider_budget_tracker: None,
            budget_authority: Some(authority),
            budget_session_id,
            execution_budget: None,
            cancel,
            budget_guard: None,
            active_child_permits: Arc::new(tokio::sync::Semaphore::new(
                wcore_swarm::MAX_CONCURRENT_WORKERS,
            )),
        }
    }

    pub(crate) fn with_budget_guard(
        mut self,
        budget_guard: Arc<crate::cancel::BudgetGuard>,
    ) -> Self {
        self.budget_guard = Some(budget_guard);
        self
    }

    /// Stable Core session lineage shared by the parent and every transient
    /// child. Producer contracts use it only when no persisted host session
    /// identity has been bound to the output sink yet.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.budget_session_id
    }
}

impl AgentSpawner {
    pub fn new(provider: Arc<dyn LlmProvider>, config: Config) -> Self {
        let sandbox_runtime = ToolRegistry::new().sandbox_runtime();
        let effective_policy = EffectiveExecutionPolicy::baseline(&config.execution_policy);
        Self {
            provider,
            base_config: config,
            sandbox_runtime,
            parent_workspace: None,
            egress_policy: wcore_egress::default_policy(),
            approval_manager: None,
            bus: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            session_runtime: None,
            resolver: None,
            budget_tracker: None,
            budget_identity: None,
            provider_budget_tracker: None,
            budget_authority: None,
            budget_session_id: None,
            execution_budget: None,
            budget_guard: None,
            durable_authority: DurableSessionAuthority::new(),
            effective_policy,
            active_child_permits: Arc::new(tokio::sync::Semaphore::new(
                wcore_swarm::MAX_CONCURRENT_WORKERS,
            )),
        }
    }

    /// Install the one session authority shared by every transient spawner.
    pub(crate) fn with_durable_session_authority(
        mut self,
        authority: DurableSessionAuthority,
        effective_policy: EffectiveExecutionPolicy,
    ) -> Result<Self, DurableSpawnerError> {
        authority.install_effective_policy(effective_policy.clone())?;
        self.durable_authority = authority;
        self.effective_policy = effective_policy;
        Ok(self)
    }

    pub(crate) fn with_shared_durable_session_authority(
        self,
        authority: DurableSessionAuthority,
    ) -> Result<Self, DurableSpawnerError> {
        let effective_policy = authority.effective_policy()?;
        self.with_durable_session_authority(authority, effective_policy)
    }

    /// Bind this spawner and every clone it creates to one canonical session.
    ///
    /// Host adapters that construct an [`AgentSpawner`] outside
    /// [`AgentEngine`](crate::engine::AgentEngine) must call this before any
    /// child launch. A missing, mismatched, or non-canonical journal fails
    /// closed; no ephemeral execution fallback exists.
    pub fn bind_durable_session(
        &self,
        journal: SessionJournal,
        expected_session_id: &str,
    ) -> Result<(), DurableSpawnerError> {
        self.durable_authority
            .bind(journal, expected_session_id)
            .map(|_| ())
    }

    /// Return the canonical session currently owning durable child launches.
    pub fn durable_session_id(&self) -> Result<String, DurableSpawnerError> {
        self.durable_authority
            .token()
            .map(|token| token.session_id().to_owned())
    }

    /// Return a session-pinned supervisor over every durable child origin.
    pub fn durable_child_supervisor(&self) -> Result<DurableChildSupervisor, DurableSpawnerError> {
        self.durable_authority.supervisor()
    }

    /// Canonical host adapter for creating durable child work.
    pub async fn spawn_host_child(&self, sub_config: SubAgentConfig) -> SubAgentResult {
        self.spawn_one_with_origin(sub_config, ChildOrigin::Host)
            .await
    }

    /// Bind spawned children to the parent session's immutable sandbox.
    pub fn with_sandbox_runtime(mut self, runtime: Arc<wcore_sandbox::SandboxRegistry>) -> Self {
        self.sandbox_runtime = runtime;
        self
    }

    /// Bind the canonical parent workspace selected by Bootstrap.
    pub fn with_parent_workspace(
        mut self,
        workspace: impl AsRef<Path>,
    ) -> Result<Self, DurableSpawnerError> {
        let workspace = std::fs::canonicalize(workspace.as_ref()).map_err(|error| {
            DurableSpawnerError::WorkspacePreparation(format!(
                "parent workspace is unavailable: {error}"
            ))
        })?;
        if !workspace.is_dir() {
            return Err(DurableSpawnerError::WorkspacePreparation(
                "parent workspace is not a directory".to_owned(),
            ));
        }
        self.parent_workspace = Some(Arc::new(workspace));
        Ok(self)
    }

    /// Bind every spawned child engine to the parent's session-owned egress
    /// policy, including children created after bootstrap has returned.
    pub fn with_egress_policy(mut self, policy: wcore_egress::SharedPolicy) -> Self {
        self.egress_policy = policy;
        self
    }

    /// Bind child posture derivation to the host session's live manager.
    pub fn with_approval_manager(
        mut self,
        manager: Arc<wcore_protocol::ToolApprovalManager>,
    ) -> Self {
        self.approval_manager = Some(manager);
        self
    }

    /// Return the sandbox runtime inherited by spawned children.
    pub fn sandbox_runtime(&self) -> &Arc<wcore_sandbox::SandboxRegistry> {
        &self.sandbox_runtime
    }

    fn child_tool_registry(&self, launch: &ResolvedChildLaunch) -> ToolRegistry {
        build_tool_registry(
            &launch.overrides.allowed_tools,
            launch.requested_workspace,
            launch.workspace_root(),
            &launch.authority_read_deny,
            Arc::clone(&self.sandbox_runtime),
        )
    }

    /// Bind the spawner to the parent engine's cancellation token so a host
    /// cancel propagates into every spawned sub-agent. Production bootstrap
    /// attaches the engine's `cancel_token()` here, alongside `with_bus(...)`.
    pub fn with_cancel(mut self, cancel: tokio_util::sync::CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Bind production spawning to the session's shared active-turn handle.
    pub(crate) fn with_session_runtime(
        mut self,
        runtime: crate::cancel::SessionRuntimeHandle,
    ) -> Self {
        self.session_runtime = Some(runtime);
        self
    }

    fn active_cancel_token(&self) -> tokio_util::sync::CancellationToken {
        self.session_runtime
            .as_ref()
            .map(crate::cancel::SessionRuntimeHandle::active_turn_token)
            .unwrap_or_else(|| self.cancel.clone())
    }

    /// v0.8.0 Task J — attach an `AgentBus` so every `spawn_one` /
    /// `spawn_parallel*` / `spawn_fork` call publishes lifecycle events
    /// (Spawned → FirstMessage → Completed | Errored). Builder pattern
    /// because production bootstrap (`bootstrap.rs`) constructs the
    /// spawner before the engine's bus is finalised — the bus pointer
    /// is attached at the end of `apply_initialize_outcome` once the
    /// engine has been built.
    pub fn with_bus(mut self, bus: Arc<AgentBus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Test/inspection helper — returns the attached `AgentBus` if any.
    pub fn bus(&self) -> Option<&Arc<AgentBus>> {
        self.bus.as_ref()
    }

    /// The attached council provider resolver, if any. The council executor
    /// reads it from the spawner so there is a single resolver source (the one
    /// that also keys per-proposer spawns) — no chance of a mismatched pair.
    pub fn provider_resolver(&self) -> Option<&Arc<dyn ProviderResolver>> {
        self.resolver.as_ref()
    }

    /// Crucible — attach a [`ProviderResolver`] so a `SubAgentConfig.provider`
    /// pin resolves to a keyed provider (a different LLM provider per council
    /// member). Builder pattern: production bootstrap constructs a
    /// `CouncilProviderResolver` once and attaches it here.
    pub fn with_provider_resolver(mut self, resolver: Arc<dyn ProviderResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Crucible — attach the shared per-session/day [`BudgetTracker`] so council
    /// member spend decrements the same envelope as the parent turn.
    pub fn with_budget_tracker(
        mut self,
        tracker: Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>,
    ) -> Self {
        self.budget_tracker = Some(tracker);
        self
    }

    /// The shared budget tracker, if one was attached.
    pub fn budget_tracker(&self) -> Option<&Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>> {
        self.budget_tracker.as_ref()
    }

    /// Crucible — the (session_id, user_id) the council charges against.
    pub fn with_budget_identity(
        mut self,
        session_id: impl Into<String>,
        user_id: impl Into<String>,
    ) -> Self {
        self.budget_identity = Some((session_id.into(), user_id.into()));
        self
    }

    /// The (session_id, user_id) for council charging, if set.
    pub fn budget_identity(&self) -> Option<&(String, String)> {
        self.budget_identity.as_ref()
    }

    /// Attach the finite provider-call envelope shared with the parent engine.
    pub fn with_provider_budget(
        mut self,
        tracker: Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>,
        session_id: impl Into<String>,
    ) -> Self {
        self.provider_budget_tracker = Some(tracker);
        self.budget_session_id = Some(session_id.into());
        self
    }

    /// Attach the parent execution envelope used to derive child budgets.
    pub fn with_execution_budget(mut self, budget: wcore_budget::ExecutionBudgetView) -> Self {
        self.execution_budget = Some(budget);
        self
    }

    /// Install one previously captured session governance bundle.
    pub fn with_budget_governance(mut self, governance: SpawnerBudgetGovernance) -> Self {
        self.provider_budget_tracker = governance.provider_budget_tracker;
        self.budget_authority = governance.budget_authority;
        self.budget_session_id = Some(governance.budget_session_id);
        self.execution_budget = governance.execution_budget;
        self.cancel = governance.cancel;
        self.budget_guard = governance.budget_guard;
        self.active_child_permits = governance.active_child_permits;
        self
    }

    /// Capture the complete finite envelope for a transient child spawner.
    /// Production Smart sessions always return `Some`; legacy/test spawners
    /// without an attached provider ledger return `None` rather than fabricating
    /// an independent allowance.
    pub fn budget_governance(&self) -> Option<SpawnerBudgetGovernance> {
        let mut governance = if let Some(authority) = self.budget_authority.as_ref() {
            SpawnerBudgetGovernance::from_authority(
                Arc::clone(authority),
                self.active_cancel_token(),
            )
        } else {
            SpawnerBudgetGovernance::new(
                Arc::clone(self.provider_budget_tracker.as_ref()?),
                self.budget_session_id.as_ref()?.clone(),
                self.execution_budget.as_ref()?.clone(),
                self.active_cancel_token(),
            )
        };
        governance.active_child_permits = Arc::clone(&self.active_child_permits);
        Some(match self.budget_guard.as_ref() {
            Some(guard) => governance.with_budget_guard(Arc::clone(guard)),
            None => governance,
        })
    }

    fn enter_child_budget(
        &self,
    ) -> Result<
        (
            Option<wcore_budget::ExecutionBudgetView>,
            Option<ChildBudgetGuard>,
        ),
        String,
    > {
        if let Some(authority) = self.budget_authority.as_ref() {
            let (child, guard) = authority
                .lock()
                .transaction(|mutation| {
                    let child = mutation.execution().sub_budget(None);
                    let guard = child.enter_agent();
                    if let Some(reason) = child.first_exceeded_reason() {
                        let observed = child.observed_for(reason);
                        let limit = child.limit_for(reason);
                        drop(guard);
                        return Err(format!(
                            "child agent not started: budget cap '{reason}' exceeded (limit {limit}, observed {observed})"
                        ));
                    }
                    Ok((child, guard))
                })
                .map_err(|error| error.to_string())??;
            return Ok((
                Some(child),
                Some(ChildBudgetGuard {
                    guard: Some(guard),
                    authority: Some(Arc::clone(authority)),
                }),
            ));
        }
        let Some(parent) = self.execution_budget.as_ref() else {
            return Ok((None, None));
        };
        let child = parent.sub_budget(None);
        let guard = child.enter_agent();
        if let Some(reason) = child.first_exceeded_reason() {
            let observed = child.observed_for(reason);
            let limit = child.limit_for(reason);
            drop(guard);
            return Err(format!(
                "child agent not started: budget cap '{reason}' exceeded (limit {limit}, observed {observed})"
            ));
        }
        Ok((
            Some(child),
            Some(ChildBudgetGuard {
                guard: Some(guard),
                authority: None,
            }),
        ))
    }

    fn bind_child_budget(
        &self,
        engine: &mut AgentEngine,
        execution_budget: Option<wcore_budget::ExecutionBudgetView>,
    ) -> Result<(), String> {
        if let Some(authority) = self.budget_authority.as_ref() {
            engine
                .inherit_budget_authority(Arc::clone(authority))
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        if let Some(budget) = execution_budget {
            engine.set_execution_budget(budget);
        }
        if let Some(tracker) = self.provider_budget_tracker.as_ref() {
            engine.set_budget_tracker(Arc::clone(tracker));
        }
        if let Some(session_id) = self.budget_session_id.as_ref() {
            engine.set_budget_session_id(session_id.clone());
        }
        Ok(())
    }

    /// Resolve the provider a given sub-agent should run on.
    ///
    /// - **Unpinned** (`sub.provider == None`): inherit the parent provider —
    ///   the single-provider default, regardless of whether a resolver is
    ///   attached.
    /// - **Pinned with a resolver**: resolve the spec to a keyed provider. A
    ///   resolution failure (unknown / keyless) is fatal *for that sub-agent*
    ///   and surfaces as an error [`SubAgentResult`] (the council skips
    ///   keyless members when building the roster, before they reach here).
    /// - **Pinned without a resolver**: a configuration error — a provider was
    ///   pinned but nothing can resolve it. Fail that sub-agent loudly rather
    ///   than silently running it on the parent provider.
    #[cfg(test)]
    fn provider_for(&self, sub: &SubAgentConfig) -> Result<Arc<dyn LlmProvider>, SubAgentResult> {
        self.resolve_provider_for(sub)
            .map(|resolved| resolved.provider)
    }

    fn resolve_provider_for(
        &self,
        sub: &SubAgentConfig,
    ) -> Result<ResolvedProvider, SubAgentResult> {
        match (&sub.provider, &self.resolver) {
            (None, _) => {
                let provider_id = if self.base_config.provider_label.trim().is_empty() {
                    self.base_config.compat.provider_type().to_owned()
                } else {
                    self.base_config.provider_label.clone()
                };
                Ok(ResolvedProvider {
                    provider: self.provider.clone(),
                    provider_id,
                    model: None,
                })
            }
            (Some(spec), Some(resolver)) => resolver
                .resolve_provider(spec)
                .map(|(provider, model)| {
                    let provider_id = spec.split_once(':').map_or(spec.as_str(), |(id, _)| id);
                    ResolvedProvider {
                        provider,
                        provider_id: provider_id.to_owned(),
                        model,
                    }
                })
                .map_err(|e| SubAgentResult::error(&sub.name, &format!("provider '{spec}': {e}"))),
            (Some(spec), None) => Err(SubAgentResult::error(
                &sub.name,
                &format!("provider '{spec}' pinned but no provider resolver is attached"),
            )),
        }
    }

    /// Resolve every mutable child input once and bind it to the active durable
    /// session generation. No journal write occurs here.
    pub fn resolve_durable_launch(
        &self,
        request: SubAgentConfig,
        overrides: ForkOverrides,
    ) -> Result<ResolvedChildLaunch, DurableSpawnerError> {
        let pre_resolved = self.pre_resolve_durable_launch(request, overrides)?;
        let requested_workspace = pre_resolved.requested_workspace;
        if requested_workspace != RequestedChildWorkspace::SharedReadOnly {
            return Err(DurableSpawnerError::WorkspacePreparation(
                "mutating children require asynchronous isolated-workspace preparation".to_owned(),
            ));
        }
        let parent = self.parent_workspace.as_ref().ok_or_else(|| {
            DurableSpawnerError::WorkspacePreparation(
                "parent workspace authority is not bound".to_owned(),
            )
        })?;
        let workspace = shared_child_workspace(parent)?;
        let child_id = allocate_child_id()?;
        self.resolve_durable_launch_in_workspace(
            pre_resolved,
            child_id,
            workspace,
            parent.as_ref().clone(),
            Vec::new(),
            None,
        )
    }

    /// Resolve a child against a workspace already created by a binding layer.
    /// The supplied value is realized execution evidence, not a request. A
    /// mutating request can therefore never be recorded as shared read-only.
    fn resolve_durable_launch_in_workspace(
        &self,
        pre_resolved: PreResolvedChildLaunch,
        child_id: ChildId,
        workspace: ChildWorkspace,
        workspace_root: PathBuf,
        authority_read_deny: Vec<PathBuf>,
        transaction: Option<TransactionWorkspace>,
    ) -> Result<ResolvedChildLaunch, DurableSpawnerError> {
        // The retained standalone-checkout handle (`transaction`) is owned by
        // this call for the duration of validation. Any early `return Err`
        // below drops it here, rolling back and cleaning the just-opened
        // transaction before the error propagates.
        let requested_workspace = pre_resolved.requested_workspace;
        if !requested_workspace.permits(workspace.mode) {
            return Err(DurableSpawnerError::EvidenceMismatch(
                "workspace does not satisfy requested authority",
            ));
        }
        let workspace_root = std::fs::canonicalize(&workspace_root).map_err(|_| {
            DurableSpawnerError::EvidenceMismatch("workspace root is not canonical")
        })?;
        let parent = self.parent_workspace.as_ref().ok_or_else(|| {
            DurableSpawnerError::WorkspacePreparation(
                "parent workspace authority is not bound".to_owned(),
            )
        })?;
        match workspace.mode {
            ChildWorkspaceMode::SharedReadOnly => {
                if workspace_root != **parent
                    || workspace != shared_child_workspace(parent)?
                    || !authority_read_deny.is_empty()
                    || transaction.is_some()
                {
                    return Err(DurableSpawnerError::EvidenceMismatch(
                        "shared workspace evidence",
                    ));
                }
            }
            ChildWorkspaceMode::Isolated => {
                let expected_id = format!("isolated-{}", child_id.as_str());
                let session_root = std::fs::canonicalize(&self.base_config.session.directory)
                    .map_err(|_| DurableSpawnerError::EvidenceMismatch("workspace session root"))?;
                // The realized child working directory is the `checkout`
                // subdirectory of the transaction root
                // (`<checkouts>/<child_id>/checkout`), not the transaction root
                // itself. Validate against that exact path.
                let expected_root = session_root
                    .join("delegated-workspaces")
                    .join("checkouts")
                    .join(child_id.as_str())
                    .join("checkout");
                if workspace.workspace_id != expected_id || workspace_root != expected_root {
                    return Err(DurableSpawnerError::EvidenceMismatch(
                        "isolated workspace identity or root",
                    ));
                }
                if !authority_read_deny
                    .iter()
                    .any(|path| path == parent.as_ref())
                    || authority_read_deny.iter().any(|path| {
                        workspace_root.starts_with(path) || path.starts_with(&workspace_root)
                    })
                {
                    return Err(DurableSpawnerError::EvidenceMismatch(
                        "isolated workspace authority deny roots",
                    ));
                }
                // A mutating launch must carry the exact standalone-checkout
                // handle allocated for this child; without it there is no
                // durable opening bound to the checkout on disk. Checked after
                // the identity/root/deny evidence so a forged workspace is
                // rejected on the specific evidence it falsified.
                if transaction.is_none() {
                    return Err(DurableSpawnerError::EvidenceMismatch(
                        "isolated workspace transaction authority",
                    ));
                }
            }
            ChildWorkspaceMode::External => {
                return Err(DurableSpawnerError::EvidenceMismatch(
                    "external workspace is not transactional isolation",
                ));
            }
        }
        Ok(ResolvedChildLaunch {
            child_id,
            request: pre_resolved.request,
            overrides: pre_resolved.overrides,
            provider: pre_resolved.provider,
            provider_id: pre_resolved.provider_id,
            model: pre_resolved.model,
            config: pre_resolved.config,
            policy: pre_resolved.policy,
            requested_workspace,
            workspace,
            workspace_root,
            authority_read_deny,
            authority: pre_resolved.authority,
            parent_cancel: pre_resolved.parent_cancel,
            _transaction_workspace: transaction,
        })
    }

    fn pre_resolve_durable_launch(
        &self,
        request: SubAgentConfig,
        overrides: ForkOverrides,
    ) -> Result<PreResolvedChildLaunch, DurableSpawnerError> {
        let parent_cancel = self.active_cancel_token();
        let authority = self.durable_authority.token()?;
        let requested_workspace = overrides.requested_workspace();
        if requested_workspace == RequestedChildWorkspace::IsolatedMutation
            && self.sandbox_runtime.bypasses_containment()
        {
            return Err(DurableSpawnerError::WorkspacePreparation(
                "transactional child isolation requires an enforcing sandbox backend".to_owned(),
            ));
        }
        let ResolvedProvider {
            provider,
            provider_id,
            model: provider_model,
        } = self
            .resolve_provider_for(&request)
            .map_err(|_| DurableSpawnerError::EvidenceMismatch("provider resolution"))?;
        let mut config = self.child_config(&request);
        let model = overrides
            .model
            .clone()
            .or_else(|| request.model.clone())
            .or(provider_model)
            .unwrap_or_else(|| config.model.clone());
        if provider_id.trim().is_empty() {
            return Err(DurableSpawnerError::EvidenceMismatch("resolved provider"));
        }
        if model.trim().is_empty() {
            return Err(DurableSpawnerError::EvidenceMismatch("resolved model"));
        }
        config.model.clone_from(&model);
        let runtime_policy = self
            .effective_policy
            .with_runtime_approvals(config.smart_approval_policy());
        let policy = child_policy_snapshot(&runtime_policy)?;
        Ok(PreResolvedChildLaunch {
            request,
            overrides,
            provider,
            provider_id,
            model,
            config,
            policy,
            requested_workspace,
            authority,
            parent_cancel,
        })
    }

    /// Allocate identity, prepare the realized workspace, and only then
    /// return a launch eligible for durable declaration.
    async fn prepare_durable_launch(
        &self,
        request: SubAgentConfig,
        overrides: ForkOverrides,
    ) -> Result<ResolvedChildLaunch, DurableSpawnerError> {
        let pre_resolved = self.pre_resolve_durable_launch(request, overrides)?;
        let child_id = allocate_child_id()?;
        let prepared = self
            .prepare_child_workspace(&child_id, pre_resolved.requested_workspace)
            .await?;
        // Move the retained checkout handle into resolution. If resolution
        // fails, it drops there and cleans the just-opened transaction; on
        // success it travels into the returned launch and lives until the child
        // reaches a terminal state.
        self.resolve_durable_launch_in_workspace(
            pre_resolved,
            child_id,
            prepared.evidence,
            prepared.root,
            prepared.authority_read_deny,
            prepared.transaction,
        )
    }

    async fn prepare_child_workspace(
        &self,
        child_id: &ChildId,
        requested: RequestedChildWorkspace,
    ) -> Result<PreparedChildWorkspace, DurableSpawnerError> {
        let parent = self.parent_workspace.as_ref().ok_or_else(|| {
            DurableSpawnerError::WorkspacePreparation(
                "parent workspace authority is not bound".to_owned(),
            )
        })?;
        match requested {
            RequestedChildWorkspace::SharedReadOnly => Ok(PreparedChildWorkspace {
                evidence: shared_child_workspace(parent)?,
                root: parent.as_ref().clone(),
                authority_read_deny: Vec::new(),
                transaction: None,
            }),
            RequestedChildWorkspace::IsolatedMutation => {
                let session_root = PathBuf::from(&self.base_config.session.directory);
                if !session_root.is_absolute() {
                    return Err(DurableSpawnerError::WorkspacePreparation(
                        "session directory must be absolute for delegated workspace retention"
                            .to_owned(),
                    ));
                }
                std::fs::create_dir_all(&session_root).map_err(|error| {
                    DurableSpawnerError::WorkspacePreparation(format!(
                        "could not create session state root: {error}"
                    ))
                })?;
                let session_root = std::fs::canonicalize(&session_root).map_err(|error| {
                    DurableSpawnerError::WorkspacePreparation(format!(
                        "could not resolve session state root: {error}"
                    ))
                })?;
                let control_root = session_root.join("delegated-workspaces");
                let checkouts = control_root.join("checkouts");
                let manager = WorktreeManager::new_with_workspace_root(parent, &checkouts)
                    .map_err(|error| {
                        DurableSpawnerError::WorkspacePreparation(error.to_string())
                    })?;
                let pinned_head = manager.pinned_head().await.map_err(|error| {
                    DurableSpawnerError::WorkspacePreparation(error.to_string())
                })?;
                let git_common_dir = manager.git_common_dir().await.map_err(|error| {
                    DurableSpawnerError::WorkspacePreparation(error.to_string())
                })?;
                let admission = workspace_admission_gate(&control_root)?;
                let _admission_guard = admission.lock().await;
                let retained = retained_workspace_allocation_count(
                    &control_root.join("leases"),
                    &checkouts,
                    wcore_swarm::MAX_RETAINED_WORKTREES,
                )?;
                if retained >= wcore_swarm::MAX_RETAINED_WORKTREES {
                    return Err(DurableSpawnerError::WorkspacePreparation(format!(
                        "delegated workspace evidence quota is full: {retained}/{}",
                        wcore_swarm::MAX_RETAINED_WORKTREES
                    )));
                }
                // Prove usable storage for this transaction plus the already
                // retained allocations before creating the standalone checkout.
                let active_workers = retained.saturating_add(1);
                let capacity =
                    manager
                        .workspace_capacity(active_workers)
                        .await
                        .map_err(|error| {
                            DurableSpawnerError::WorkspacePreparation(error.to_string())
                        })?;
                let worker_id = child_id.as_str();
                let branch = format!("wayland-child/{worker_id}");
                // Write-ahead the preparation lease naming the exact working
                // directory the child will run in — the `checkout` subdirectory
                // of the transaction root — before that checkout exists.
                let checkout_root = manager.swarm_root().join(worker_id).join("checkout");
                write_workspace_preparation_lease(
                    &control_root,
                    child_id,
                    &checkout_root,
                    &pinned_head,
                )?;
                // `create_isolated_checkout` returns an identity-bound
                // `TransactionWorkspace` that owns the retained checkout/scratch
                // authorities and an `Arc<TransactionCleanup>` whose Drop
                // deletes the checkout. It must therefore be retained for the
                // whole child lifetime and terminalized exactly once; it is
                // never a bare path.
                let workspace = manager
                    .create_isolated_checkout(worker_id, &branch, &pinned_head, capacity)
                    .await
                    .map_err(|error| {
                        DurableSpawnerError::WorkspacePreparation(error.to_string())
                    })?;
                // The realized child working directory is `workspace.checkout`.
                // If it cannot be canonicalized, `workspace` is dropped on this
                // error path, rolling back and cleaning the just-opened
                // transaction before the error propagates.
                let root = std::fs::canonicalize(&workspace.checkout).map_err(|error| {
                    DurableSpawnerError::WorkspacePreparation(format!(
                        "created workspace checkout {} could not be canonicalized; rolled back: {error}",
                        workspace.checkout.display()
                    ))
                })?;
                Ok(PreparedChildWorkspace {
                    evidence: ChildWorkspace {
                        mode: ChildWorkspaceMode::Isolated,
                        workspace_id: format!("isolated-{worker_id}"),
                    },
                    root,
                    authority_read_deny: vec![parent.as_ref().clone(), git_common_dir],
                    transaction: Some(workspace),
                })
            }
        }
    }

    /// Validate and declare against the same generation used during resolve.
    /// The authority mutex stays held through declaration, so a concurrent
    /// session switch cannot redirect the write to another journal.
    pub fn declare_resolved_child(
        &self,
        launch: &ResolvedChildLaunch,
        record: DurableChildRecord,
    ) -> Result<crate::durable_child::DurableChildWrite, DurableSpawnerError> {
        self.durable_authority
            .with_store(launch.authority(), |store| {
                launch.validate_record(&record)?;
                store.declare(record).map_err(DurableSpawnerError::Journal)
            })
    }

    /// Spawn a single sub-agent and wait for result.
    pub async fn spawn_one(&self, sub_config: SubAgentConfig) -> SubAgentResult {
        self.spawn_one_with_origin(sub_config, ChildOrigin::Spawn)
            .await
    }

    /// Spawn one child with its durable lifecycle origin preserved.
    pub async fn spawn_one_with_origin(
        &self,
        sub_config: SubAgentConfig,
        origin: ChildOrigin,
    ) -> SubAgentResult {
        self.spawn_durable(
            sub_config,
            ForkOverrides::default(),
            SpawnExtras::default(),
            origin,
        )
        .await
    }

    /// Spawn multiple sub-agents in parallel.
    ///
    /// W7 F2: legacy shim — delegates to `spawn_parallel_with_extras` with
    /// `SpawnExtras::default()` so behaviour is bit-identical to today's
    /// "anonymous Spawn" call sites. New callers that want sub-agent event
    /// relay should call `spawn_parallel_with_extras` directly.
    pub async fn spawn_parallel(&self, sub_configs: Vec<SubAgentConfig>) -> Vec<SubAgentResult> {
        self.spawn_parallel_with_extras(sub_configs, SpawnExtras::default())
            .await
    }

    /// W7 F2: parallel spawn with channel-sink wiring.
    ///
    /// When `extras.channel_sink` is `Some`, the sub-agent's engine uses it
    /// as its `OutputSink` so every event the sub-agent emits is relayed via
    /// `SubAgentRelay` to the parent for `SubAgentEvent` wrapping. When
    /// `None`, behaviour is bit-identical to the pre-W7 `spawn_parallel`.
    pub async fn spawn_parallel_with_extras(
        &self,
        sub_configs: Vec<SubAgentConfig>,
        extras: SpawnExtras,
    ) -> Vec<SubAgentResult> {
        self.spawn_parallel_with_extras_origin(sub_configs, extras, ChildOrigin::Spawn)
            .await
    }

    pub async fn spawn_parallel_with_extras_origin(
        &self,
        sub_configs: Vec<SubAgentConfig>,
        extras: SpawnExtras,
        origin: ChildOrigin,
    ) -> Vec<SubAgentResult> {
        let mut futures = SpawnTaskSet(
            sub_configs
                .into_iter()
                .map(|config| {
                    let spawner = self.clone_for_spawn();
                    let extras = extras.clone();
                    tokio::spawn(async move {
                        spawner
                            .spawn_one_with_active_permit(config, extras, origin)
                            .await
                    })
                })
                .collect(),
        );

        let mut results = Vec::new();
        for future in &mut futures.0 {
            match future.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(SubAgentResult {
                    name: "unknown".to_string(),
                    text: format!("Task join error: {}", e),
                    usage: TokenUsage::default(),
                    turns: 0,
                    is_error: true,
                }),
            }
        }
        results
    }

    /// #269 — route a parallel spawn through `FleetDispatcher` for
    /// hierarchical sharding. Each `SubAgentConfig` becomes one
    /// `MeshAgent`; the fleet shards them into batches of
    /// [`DEFAULT_SHARD_SIZE`] (10) and runs every shard concurrently as a
    /// `MeshDispatcher`. Each sub-agent's [`AgentBus`] `Spawned` event
    /// carries `parent_call_id = Some("fleet:<run_id>-shard-<i>-<j>")`
    /// so a subscriber can prove the Fleet path was taken (the wire-
    /// presence test in `fleet_dispatcher_wired_test.rs` checks this).
    ///
    /// `run_id` is a free-form label propagated into the fleet's
    /// blackboard topic prefix; callers in production pass the
    /// `SpawnTool` invocation id.
    pub async fn spawn_via_fleet(
        &self,
        sub_configs: Vec<SubAgentConfig>,
        run_id: impl Into<String>,
    ) -> Vec<SubAgentResult> {
        let tasks = sub_configs
            .into_iter()
            .map(|config| (config, SpawnExtras::default()))
            .collect();
        self.spawn_via_fleet_with_per_task_extras(tasks, run_id)
            .await
    }

    /// Fleet-sharded spawn with one output/terminal sink per task. Fleet keeps
    /// its shard-scoped bus correlation while the supplied `ChannelSink`
    /// independently carries the workflow node correlation to the host.
    pub async fn spawn_via_fleet_with_per_task_extras(
        &self,
        tasks_and_extras: Vec<(SubAgentConfig, SpawnExtras)>,
        run_id: impl Into<String>,
    ) -> Vec<SubAgentResult> {
        let run_id = run_id.into();
        let fleet = FleetDispatcher::new(run_id).with_shard_size(DEFAULT_SHARD_SIZE);

        // Build one MeshAgent per task. Each agent owns a clone of the
        // spawner (cheap — same Arc/Config plumbing the legacy
        // spawn_parallel path uses) and reports back the SubAgentResult
        // serialized into the AgentReport payload so the reducer can
        // reconstruct it on the orchestrator side.
        let agents: Vec<MeshAgent> = tasks_and_extras
            .into_iter()
            .map(|(sub_config, extras)| -> MeshAgent {
                let spawner = self.clone_for_spawn();
                Box::new(move |ctx: BlackboardCtx| {
                    Box::pin(async move {
                        // Wire-presence signal: tag the per-sub-agent
                        // Spawned event with the shard-scoped id so a
                        // bus subscriber can prove the Fleet path ran.
                        let mut extras = extras;
                        extras.parent_call_id = Some(format!("fleet:{}", ctx.agent_id));
                        let result = spawner
                            .spawn_one_with_active_permit(sub_config, extras, ChildOrigin::Fleet)
                            .await;
                        let succeeded = !result.is_error;
                        AgentReport {
                            agent_id: ctx.agent_id,
                            payload: sub_agent_result_to_payload(&result),
                            succeeded,
                        }
                    })
                })
            })
            .collect();

        // Reducer: flatten all shard summaries back into the original
        // Vec<SubAgentResult>. Order is shard_id-then-within-shard,
        // which matches input order modulo the shard boundary (the same
        // race-order property the legacy spawn_parallel path has).
        let reducer: FleetReducer<Vec<SubAgentResult>> =
            Box::new(|summaries: Vec<ShardSummary>| {
                summaries
                    .into_iter()
                    .flat_map(|s| {
                        // The shard's payload is the
                        // serde_json::Value::Array we built in
                        // `default_shard_reducer_into_results` below.
                        s.payload
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .map(payload_to_sub_agent_result)
                            .collect::<Vec<_>>()
                    })
                    .collect()
            });

        // Shard reducer factory: each shard collects its AgentReports'
        // payloads (already serialized SubAgentResults) into a JSON array
        // attached to the ShardSummary, so the FleetReducer above can
        // walk them in stable order.
        let shard_factory: Box<dyn Fn() -> wcore_swarm::ShardReducer + Send + Sync> =
            Box::new(|| Box::new(default_shard_reducer_into_results));

        match fleet.dispatch(agents, Some(shard_factory), reducer).await {
            Ok(results) => results,
            Err(err) => {
                // FleetDispatcher only errors on cap-exceeded or shard
                // join failure. Surface as a single error-result so the
                // SpawnTool caller's `is_error` aggregation still works.
                vec![SubAgentResult {
                    name: "fleet".to_string(),
                    text: format!("Fleet dispatch failed: {err}"),
                    usage: TokenUsage::default(),
                    turns: 0,
                    is_error: true,
                }]
            }
        }
    }

    /// v0.9.4 W1: per-task parallel spawn with individual extras per task.
    ///
    /// Unlike `spawn_parallel_with_extras` (one `SpawnExtras` shared across
    /// all tasks), this variant gives each task its own `SpawnExtras` so each
    /// sub-agent gets a distinct `ChannelSink` and `parent_call_id`. Required
    /// for N distinct `SubAgentView` rows in the bridge (C1/F8 relay fix).
    pub async fn spawn_parallel_with_per_task_extras(
        &self,
        tasks_and_extras: Vec<(SubAgentConfig, SpawnExtras)>,
    ) -> Vec<SubAgentResult> {
        self.spawn_parallel_with_per_task_extras_origin(tasks_and_extras, ChildOrigin::Spawn)
            .await
    }

    pub async fn spawn_parallel_with_per_task_extras_origin(
        &self,
        tasks_and_extras: Vec<(SubAgentConfig, SpawnExtras)>,
        origin: ChildOrigin,
    ) -> Vec<SubAgentResult> {
        let mut join_terminals = Vec::with_capacity(tasks_and_extras.len());
        let mut handles = Vec::with_capacity(tasks_and_extras.len());
        for (config, extras) in tasks_and_extras {
            let spawner = self.clone_for_spawn();
            join_terminals.push((config.name.clone(), extras.channel_sink.clone()));
            handles.push(tokio::spawn(async move {
                spawner
                    .spawn_one_with_active_permit(config, extras, origin)
                    .await
            }));
        }
        let mut futures = SpawnTaskSet(handles);

        let mut results = Vec::new();
        for (future, (name, terminal_sink)) in futures.0.iter_mut().zip(join_terminals) {
            match future.await {
                Ok(result) => results.push(result),
                Err(e) => {
                    let result = SubAgentResult {
                        name,
                        text: format!("Task join error: {e}"),
                        usage: TokenUsage::default(),
                        turns: 0,
                        is_error: true,
                    };
                    relay_subagent_terminal(terminal_sink.as_deref(), &result);
                    results.push(result);
                }
            }
        }
        results
    }

    /// W7 F2: per-task helper — mirrors `spawn_one`, but installs an
    /// `Arc<ChannelSink>` as `OutputSink` when `extras.channel_sink` is
    /// `Some`. Anonymous (None) call path is byte-identical to `spawn_one`.
    async fn spawn_one_with_extras(
        &self,
        sub_config: SubAgentConfig,
        extras: SpawnExtras,
        origin: ChildOrigin,
    ) -> SubAgentResult {
        self.spawn_durable(sub_config, ForkOverrides::default(), extras, origin)
            .await
    }

    async fn spawn_one_with_active_permit(
        &self,
        sub_config: SubAgentConfig,
        extras: SpawnExtras,
        origin: ChildOrigin,
    ) -> SubAgentResult {
        let name = sub_config.name.clone();
        let terminal_sink = extras.channel_sink.clone();
        let cancel = self.active_cancel_token();
        let permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let result = SubAgentResult::error(
                    &name,
                    "parent cancelled before child concurrency admission",
                );
                relay_subagent_terminal(terminal_sink.as_deref(), &result);
                return result;
            }
            permit = Arc::clone(&self.active_child_permits).acquire_owned() => {
                match permit {
                    Ok(permit) => permit,
                    Err(_) => {
                        let result = SubAgentResult::error(
                            &name,
                            "child concurrency admission is unavailable",
                        );
                        relay_subagent_terminal(terminal_sink.as_deref(), &result);
                        return result;
                    }
                }
            }
        };
        let result = self.spawn_one_with_extras(sub_config, extras, origin).await;
        drop(permit);
        result
    }

    async fn spawn_durable(
        &self,
        sub_config: SubAgentConfig,
        overrides: ForkOverrides,
        extras: SpawnExtras,
        origin: ChildOrigin,
    ) -> SubAgentResult {
        let name = sub_config.name.clone();
        let terminal_sink = extras.channel_sink.clone();
        let launch = match self.prepare_durable_launch(sub_config, overrides).await {
            Ok(launch) => launch,
            Err(error) => {
                let result = SubAgentResult::error(&name, &error.to_string());
                relay_subagent_terminal(terminal_sink.as_deref(), &result);
                return result;
            }
        };
        self.execute_durable_launch(launch, extras, origin).await
    }

    async fn execute_durable_launch(
        &self,
        launch: ResolvedChildLaunch,
        extras: SpawnExtras,
        origin: ChildOrigin,
    ) -> SubAgentResult {
        let name = launch.request.name.clone();
        let terminal_sink = extras.channel_sink.clone();
        let record = match launch.durable_record(origin, extras.parent_call_id.clone()) {
            Ok(record) => record,
            Err(error) => {
                let result = SubAgentResult::error(&name, &error.to_string());
                relay_subagent_terminal(terminal_sink.as_deref(), &result);
                return result;
            }
        };
        let admitted = match self.durable_authority.admit_resolved(
            launch.authority(),
            record,
            &launch.request,
            &launch.overrides,
            crate::durable_spawner::ResolvedExecutionEvidence {
                provider: &launch.provider_id,
                model: &launch.model,
                effective_policy_digest: &launch.policy.exact_digest,
            },
        ) {
            Ok(admitted) => admitted,
            Err(error) => {
                let result = SubAgentResult::error(&name, &error.to_string());
                relay_subagent_terminal(terminal_sink.as_deref(), &result);
                return result;
            }
        };
        let parent_cancel = launch.parent_cancel.clone();
        let child_cancel = parent_cancel.child_token();
        // `execute_resolved_launch` carries the full child-engine state machine.
        // Keep that large future off Tokio's worker stack before the durable
        // cancellation/terminal-evidence wrapper adds its own select state.
        let execution = Box::pin(self.execute_resolved_launch(launch, extras, child_cancel));
        match admitted
            .execute_with_parent_cancel(execution, parent_cancel)
            .await
        {
            Ok(result) => {
                relay_subagent_terminal(terminal_sink.as_deref(), &result);
                result
            }
            Err(error) => {
                let result = SubAgentResult::error(&name, &error.to_string());
                relay_subagent_terminal(terminal_sink.as_deref(), &result);
                result
            }
        }
    }

    async fn execute_resolved_launch(
        &self,
        launch: ResolvedChildLaunch,
        extras: SpawnExtras,
        child_cancel: tokio_util::sync::CancellationToken,
    ) -> SubAgentResult {
        let (child_budget, _agent_guard) = match self.enter_child_budget() {
            Ok(budget) => budget,
            Err(error) => {
                return SubAgentResult::error(&launch.request.name, &error);
            }
        };
        let tools = self.child_tool_registry(&launch);
        let output: Arc<dyn OutputSink> = match extras.channel_sink {
            Some(sink) => sink as Arc<dyn OutputSink>,
            None => Arc::new(NullSink),
        };
        let mut engine = AgentEngine::new_with_provider(
            Arc::clone(&launch.provider),
            launch.config.clone(),
            tools,
            output,
        );
        if let Err(error) = self.bind_child_budget(&mut engine, child_budget) {
            return SubAgentResult::error(&launch.request.name, &error);
        }
        engine.set_egress_policy(self.egress_policy.clone());
        engine.set_cancel_token(child_cancel);
        engine.set_initial_reasoning_effort(launch.overrides.effort.clone());

        self.publish_spawned(&launch.request.name, extras.parent_call_id);
        self.publish_first_message(&launch.request.name, &launch.request.prompt);
        let mut guard = self.lifecycle_guard(&launch.request.name);
        let result = engine.run(&launch.request.prompt, "").await;
        let out = match result {
            Ok(result) => {
                self.publish_completed(
                    &launch.request.name,
                    result.turns,
                    result.usage.output_tokens,
                );
                guard.outcome = TerminalOutcome::Published;
                subagent_ok_result(launch.request.name, result)
            }
            Err(error) => {
                self.publish_errored(&launch.request.name, &error.to_string());
                guard.outcome = TerminalOutcome::Published;
                SubAgentResult {
                    name: launch.request.name,
                    text: format!("Sub-agent error: {error}"),
                    usage: TokenUsage::default(),
                    turns: 0,
                    is_error: true,
                }
            }
        };
        drop(guard);
        out
    }

    /// Derive a sub-agent's [`Config`] from the parent's `base_config`.
    ///
    /// Security audit H-7 / M-9: this is the single place that builds a child
    /// config. It clones the parent's config (which carries the parent's
    /// `tools.auto_approve` and `tools.allow_list`) and applies only the
    /// per-spawn overrides — it deliberately does NOT flip `auto_approve` to
    /// `true`. The child therefore inherits the parent's approval posture, so a
    /// parent that prompts the operator for Bash/Write/Edit keeps doing so
    /// inside any sub-agent it delegates to.
    fn child_config(&self, sub_config: &SubAgentConfig) -> Config {
        let mut config = self.base_config.clone();
        if let Some(manager) = &self.approval_manager {
            config.set_smart_approval_policy(manager.current_approval_policy());
        }
        config.max_turns = Some(sub_config.max_turns);
        config.max_tokens = sub_config.max_tokens;
        // #112 — a per-spawn cap is ALWAYS deliberate: it must bind on the
        // wire and never be omitted. Without this, (a) a desktop-default
        // session on an omit-safe provider (flux/openrouter/gemini) would
        // omit the child's sized cap and let Spawn/council children emit the
        // served model's full ceiling, busting the sub-agent/CouncilSpend
        // worst-case math; and (b) a child pinned to a different provider
        // would decide omission from the PARENT's omitted-cap signal.
        config.max_tokens_explicit = true;
        // Crucible #3 — honor a per-spawn temperature override. `None` leaves the
        // base config's temperature in place (top-level base is `None`, so the
        // child engine omits the field unless this sets it).
        if let Some(temperature) = sub_config.temperature {
            config.temperature = Some(temperature);
        }
        if let Some(sp) = sub_config.system_prompt.clone() {
            config.system_prompt = Some(sp);
        }
        // Crucible T2 — honor a per-spawn model override. The provider pin
        // (T4) selects the upstream; this sets the model the child requests.
        if let Some(model) = &sub_config.model {
            config.model = model.clone();
        }
        config.session.enabled = false;
        // FIX F — the shadow workflow-detection heuristic is a TOP-LEVEL,
        // user-initiated-turn signal. Sub-agents spawned by a workflow (or any
        // delegation) run their own turns, which are intra-workflow, not user
        // turns; leaving the gate on would pollute the shadow log with recursive
        // detections. Force it off for every child engine — the top-level shadow
        // path (driven by the parent engine, built from the un-mutated config) is
        // unaffected.
        config.observability.workflow_detection_enabled = false;
        // B6 defense-in-depth — the LIVE workflow confirm gate is a top-level,
        // user-initiated pre-LLM intercept. Child engines already lack an
        // approval manager + protocol writer (so the gate's guard short-circuits
        // for them), but force the mode off here too so a workflow's sub-agents
        // can NEVER recursively re-enter the gate regardless of how they are
        // wired.
        config.observability.workflow_live_mode = false;
        config
    }

    pub(crate) fn clone_for_spawn(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            base_config: self.base_config.clone(),
            sandbox_runtime: Arc::clone(&self.sandbox_runtime),
            parent_workspace: self.parent_workspace.clone(),
            egress_policy: self.egress_policy.clone(),
            approval_manager: self.approval_manager.clone(),
            bus: self.bus.clone(),
            cancel: self.cancel.clone(),
            session_runtime: self.session_runtime.clone(),
            // CRITICAL (crucible): the resolver MUST be carried into every
            // cloned spawner. The fleet + parallel paths run each proposer on a
            // `clone_for_spawn()` copy; dropping the resolver here would make
            // pinned proposers silently fall back to the parent provider,
            // collapsing the cross-provider council into a single-provider one.
            resolver: self.resolver.clone(),
            // CRITICAL (crucible): the shared budget tracker MUST be carried into
            // every cloned spawner. If it isn't propagated, council members run
            // on the fleet/parallel `clone_for_spawn()` copies and silently lose
            // the per-session/day envelope.
            budget_tracker: self.budget_tracker.clone(),
            budget_identity: self.budget_identity.clone(),
            provider_budget_tracker: self.provider_budget_tracker.clone(),
            budget_authority: self.budget_authority.clone(),
            budget_session_id: self.budget_session_id.clone(),
            execution_budget: self.execution_budget.clone(),
            budget_guard: self.budget_guard.clone(),
            durable_authority: self.durable_authority.clone(),
            effective_policy: self.effective_policy.clone(),
            active_child_permits: Arc::clone(&self.active_child_permits),
        }
    }

    /// Clone every inherited child authority while pinning an already-resolved
    /// provider. The returned spawner accepts an unpinned child request, so the
    /// provider cannot be resolved a second time between council admission and
    /// durable launch.
    pub(crate) fn clone_for_resolved_provider(
        &self,
        provider: Arc<dyn LlmProvider>,
        provider_id: String,
        model: Option<String>,
    ) -> Self {
        let mut cloned = self.clone_for_spawn();
        cloned.provider = provider;
        cloned.base_config.provider_label = provider_id;
        if let Some(model) = model {
            cloned.base_config.model = model;
        }
        cloned
    }

    #[doc(hidden)]
    pub fn clone_for_resolved_config(
        &self,
        provider: Arc<dyn LlmProvider>,
        config: Config,
    ) -> Self {
        let mut cloned = self.clone_for_spawn();
        cloned.provider = provider;
        cloned.base_config =
            Self::overlay_resolved_provider_config(self.base_config.clone(), config);
        cloned
    }

    /// Rebind a pre-resolved provider seat to the canonical session authority.
    ///
    /// Provider resolution is allowed to select provider credentials and wire
    /// behavior. It must not replace the session-owned approval posture,
    /// execution policy, workspace trust, security policy, or budget caps that
    /// enforcement and durable receipts describe.
    pub(crate) fn with_session_authority_config(mut self, authority: &Config) -> Self {
        self.base_config =
            Self::overlay_resolved_provider_config(authority.clone(), self.base_config);
        self
    }

    fn overlay_resolved_provider_config(mut authority: Config, resolved: Config) -> Config {
        authority.provider_label = resolved.provider_label;
        authority.provider = resolved.provider;
        authority.api_key = resolved.api_key;
        authority.base_url = resolved.base_url;
        authority.provider_organization = resolved.provider_organization;
        authority.provider_region = resolved.provider_region;
        authority.model = resolved.model;
        authority.temperature = resolved.temperature;
        authority.thinking = resolved.thinking;
        authority.prompt_caching = resolved.prompt_caching;
        authority.prompt_caching_min_prefix_tokens = resolved.prompt_caching_min_prefix_tokens;
        authority.compat = resolved.compat;
        authority.bedrock = resolved.bedrock;
        authority.vertex = resolved.vertex;

        // Approval remains session authority. A resolved machinery seat may
        // request auto-approval, but it cannot mint a child-only Bypass while
        // the canonical durable receipt still says Prompt/Managed. Until a
        // bounded delegated approval exists, non-interactive seats fail closed
        // when the canonical policy requires consent.
        authority
    }

    // ---- v0.8.0 Task J: lifecycle publish helpers ----

    fn publish_spawned(&self, agent: &str, parent_call_id: Option<String>) {
        if let Some(bus) = &self.bus {
            bus.publish(AgentMessage::Spawned {
                agent: agent.to_string(),
                parent_call_id,
                timestamp_ms: now_ms(),
            });
        }
    }

    fn publish_first_message(&self, agent: &str, content: &str) {
        if let Some(bus) = &self.bus {
            bus.publish(AgentMessage::FirstMessage {
                agent: agent.to_string(),
                content_preview: preview(content, FIRST_MESSAGE_PREVIEW_CHARS),
            });
        }
    }

    fn publish_completed(&self, agent: &str, turns: usize, output_tokens: u64) {
        if let Some(bus) = &self.bus {
            bus.publish(AgentMessage::Completed {
                agent: agent.to_string(),
                turns,
                output_tokens,
            });
        }
    }

    fn publish_errored(&self, agent: &str, error: &str) {
        if let Some(bus) = &self.bus {
            bus.publish(AgentMessage::Errored {
                agent: agent.to_string(),
                error: error.to_string(),
            });
        }
    }

    fn lifecycle_guard(&self, agent: &str) -> LifecycleGuard {
        LifecycleGuard {
            bus: self.bus.clone(),
            agent: agent.to_string(),
            outcome: TerminalOutcome::Pending,
        }
    }
}

#[async_trait]
impl Spawner for AgentSpawner {
    async fn spawn_fork(
        &self,
        sub_config: SubAgentConfig,
        overrides: ForkOverrides,
    ) -> SubAgentResult {
        self.spawn_durable(
            sub_config,
            overrides,
            SpawnExtras::default(),
            ChildOrigin::Delegate,
        )
        .await
    }

    async fn spawn_fork_with_origin(
        &self,
        sub_config: SubAgentConfig,
        overrides: ForkOverrides,
        origin: ChildOrigin,
    ) -> SubAgentResult {
        self.spawn_durable(sub_config, overrides, SpawnExtras::default(), origin)
            .await
    }
}

/// #269 — fleet sharding helper: serialize a `SubAgentResult` into the
/// `AgentReport.payload` `serde_json::Value` so the fleet reducer can
/// reconstruct it from the shard summary's payload array. Lossless for
/// the wire-format fields we care about (name/text/usage/turns/is_error).
fn sub_agent_result_to_payload(r: &SubAgentResult) -> serde_json::Value {
    serde_json::json!({
        "name": r.name,
        "text": r.text,
        "input_tokens": r.usage.input_tokens,
        "output_tokens": r.usage.output_tokens,
        "cache_creation_tokens": r.usage.cache_creation_tokens,
        "cache_read_tokens": r.usage.cache_read_tokens,
        "turns": r.turns,
        "is_error": r.is_error,
    })
}

/// #269 — fleet sharding helper: inverse of
/// [`sub_agent_result_to_payload`]. Defensive defaults so a malformed
/// payload (theoretically impossible — we always produce it ourselves)
/// surfaces as an error result rather than panicking.
fn payload_to_sub_agent_result(v: serde_json::Value) -> SubAgentResult {
    let name = v
        .get("name")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let text = v
        .get("text")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let usage = TokenUsage {
        input_tokens: v.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
        output_tokens: v.get("output_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
        cache_creation_tokens: v
            .get("cache_creation_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
        cache_read_tokens: v
            .get("cache_read_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
    };
    let turns = v.get("turns").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
    let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(true);
    SubAgentResult {
        name,
        text,
        usage,
        turns,
        is_error,
    }
}

/// #269 — fleet sharding helper: shard reducer that stuffs each
/// `AgentReport.payload` (already a serialized `SubAgentResult`) into a
/// JSON array attached to the `ShardSummary`. The fleet reducer then
/// walks shards in stable order and rehydrates the per-task results.
fn default_shard_reducer_into_results(shard_id: usize, reports: Vec<AgentReport>) -> ShardSummary {
    let successes = reports.iter().filter(|r| r.succeeded).count();
    let failures = reports.iter().filter(|r| !r.succeeded).count();
    let payload =
        serde_json::Value::Array(reports.into_iter().map(|r| r.payload).collect::<Vec<_>>());
    ShardSummary {
        shard_id,
        agent_count: successes + failures,
        successes,
        failures,
        payload,
    }
}

type ToolFactory = fn() -> Box<dyn wcore_tools::Tool>;

/// Sub-agent tools that can read but not mutate host state. When a spawn
/// requests no explicit `allowed_tools`, the child is restricted to this
/// read-only subset (security audit H-7 / M-9): an empty `toolsets` on the
/// model-facing `Delegate`/`Spawn` tool must NOT silently grant the child
/// Bash/Write/Edit. Destructive tools require explicit opt-in via `allowed`.
fn build_tool_registry(
    allowed: &[String],
    requested_workspace: RequestedChildWorkspace,
    workspace_root: &Path,
    authority_read_deny: &[PathBuf],
    sandbox_runtime: Arc<wcore_sandbox::SandboxRegistry>,
) -> ToolRegistry {
    let all: &[(&str, ToolFactory)] = &[
        ("Read", || Box::new(ReadTool::new(None))),
        ("Write", || Box::new(WriteTool::new(None))),
        ("Edit", || Box::new(EditTool::new(None))),
        ("Bash", || Box::new(BashTool)),
        ("Grep", || Box::new(GrepTool)),
        ("Glob", || Box::new(GlobTool)),
    ];

    let mut registry = ToolRegistry::new();
    registry.set_sandbox_runtime(sandbox_runtime);
    let workspace_policy = Arc::new(
        WorkspacePolicy::contained(workspace_root)
            .with_authority_read_deny(authority_read_deny.iter().cloned())
            .with_authority_write_deny(authority_read_deny.iter().cloned())
            .with_git_authority_env_deny(),
    );
    let jail = SandboxedFs::new(
        SecretDenyFs::new(RealFs, Arc::clone(&workspace_policy)),
        workspace_root,
    );
    registry.set_tool_vfs(Arc::new(jail));
    registry.set_workspace_policy(workspace_policy);
    for (name, make_tool) in all {
        // Security audit H-7 / M-9: an empty `allowed` list no longer means
        // "register everything". It defaults to a read-only subset so a
        // `Delegate` call that omits `toolsets` can never hand a sub-agent
        // Bash/Write/Edit. Callers that genuinely need destructive tools must
        // name them explicitly in `allowed`.
        let permitted = if allowed.is_empty() {
            SHARED_READ_ONLY_CHILD_TOOLS.contains(name)
        } else {
            allowed.iter().any(|a| a.as_str() == *name)
        };
        let permitted = permitted
            && (requested_workspace == RequestedChildWorkspace::IsolatedMutation
                || SHARED_READ_ONLY_CHILD_TOOLS.contains(name));
        if permitted {
            registry.register(make_tool());
        }
    }
    registry
}

#[cfg(test)]
pub(crate) fn bind_test_durable_session(
    spawner: &AgentSpawner,
    root: &std::path::Path,
    session_id: &str,
) {
    let manager = crate::session::SessionManager::new(root.to_path_buf(), 10);
    let active = manager
        .create_for_run(
            "test-provider",
            "test-model",
            &root.to_string_lossy(),
            Some(session_id),
        )
        .unwrap();
    spawner
        .bind_durable_session(active.journal, &active.session.id)
        .unwrap();
}

#[cfg(test)]
mod spawn_task_set_tests {
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::oneshot;
    use wcore_config::config::Config;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::{FinishReason, StopReason, TokenUsage};

    use super::{
        AgentSpawner, SpawnTaskSet, SpawnerBudgetGovernance, SubAgentConfig, SubAgentResult,
        bind_test_durable_session,
    };

    struct DropNotify(Option<oneshot::Sender<()>>);

    impl Drop for DropNotify {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }

    #[tokio::test]
    async fn dropping_spawn_task_set_aborts_and_drops_children() {
        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let child = tokio::spawn(async move {
            let _drop_notify = DropNotify(Some(dropped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
            SubAgentResult::error("unreachable", "unreachable")
        });
        let tasks = SpawnTaskSet(vec![child]);
        started_rx.await.expect("child must start");

        drop(tasks);

        tokio::time::timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("aborted child must be dropped promptly")
            .expect("drop notifier must fire");
    }

    struct HangingProvider {
        started: Mutex<Option<oneshot::Sender<()>>>,
        dropped: Mutex<Option<oneshot::Sender<()>>>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmProvider for HangingProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(tx) = self.started.lock().expect("started mutex").take() {
                let _ = tx.send(());
            }
            let _drop_notify = DropNotify(self.dropped.lock().expect("dropped mutex").take());
            std::future::pending().await
        }
    }

    struct CountingErrorProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmProvider for CountingErrorProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::Connection("test provider called".into()))
        }
    }

    struct ActiveCallGuard<'a>(&'a AtomicUsize);

    impl Drop for ActiveCallGuard<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct PeakConcurrencyProvider {
        active: AtomicUsize,
        peak: AtomicUsize,
        calls: AtomicUsize,
        release: tokio::sync::Semaphore,
    }

    #[async_trait]
    impl LlmProvider for PeakConcurrencyProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            let _active = ActiveCallGuard(&self.active);
            self.release
                .acquire()
                .await
                .expect("test release semaphore remains open")
                .forget();
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            tx.send(LlmEvent::TextDelta(
                "peak concurrency probe completed".into(),
            ))
            .await
            .expect("test receiver remains open");
            tx.send(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                usage: TokenUsage::default(),
            })
            .await
            .expect("test receiver remains open");
            Ok(rx)
        }
    }

    fn bounded_child(name: &str) -> SubAgentConfig {
        SubAgentConfig {
            name: name.into(),
            prompt: "perform bounded work".into(),
            max_turns: 1,
            max_tokens: 16,
            system_prompt: None,
            provider: None,
            model: None,
            temperature: None,
        }
    }

    #[tokio::test]
    async fn parallel_children_cannot_bypass_parent_agent_cap() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(CountingErrorProvider {
            calls: AtomicUsize::new(0),
        });
        let budget = wcore_budget::ExecutionBudget {
            max_agent_depth: Some(0),
            ..Default::default()
        }
        .start_root();
        let config = Config {
            model: "test-model".into(),
            provider_label: "test-provider".into(),
            ..Config::default()
        };
        let spawner = AgentSpawner::new(provider.clone(), config)
            .with_parent_workspace(dir.path())
            .unwrap()
            .with_execution_budget(budget);
        bind_test_durable_session(&spawner, dir.path(), "f190020");

        let results = spawner
            .spawn_parallel(vec![bounded_child("one"), bounded_child("two")])
            .await;

        assert!(results.iter().all(|result| result.is_error));
        assert!(
            results
                .iter()
                .all(|result| result.text.contains("max_agent_depth"))
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn parallel_spawn_caps_active_child_engines_across_shared_calls() {
        const CHILDREN_PER_CALL: usize = 50;
        const TOTAL_CHILDREN: usize = CHILDREN_PER_CALL * 2;

        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(PeakConcurrencyProvider {
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            release: tokio::sync::Semaphore::new(0),
        });
        let config = Config {
            model: "test-model".into(),
            provider_label: "test-provider".into(),
            ..Config::default()
        };
        let spawner = AgentSpawner::new(provider.clone(), config)
            .with_parent_workspace(dir.path())
            .unwrap();
        bind_test_durable_session(&spawner, dir.path(), "f200020");
        let cloned = spawner.clone_for_spawn();
        let first = (0..CHILDREN_PER_CALL)
            .map(|index| bounded_child(&format!("first-{index}")))
            .collect();
        let second = (0..CHILDREN_PER_CALL)
            .map(|index| bounded_child(&format!("second-{index}")))
            .collect();

        let run = tokio::spawn(async move {
            tokio::join!(spawner.spawn_parallel(first), cloned.spawn_parallel(second))
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while provider.active.load(Ordering::SeqCst) < wcore_swarm::MAX_CONCURRENT_WORKERS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the shared active-child limit must fill");
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(
            provider.active.load(Ordering::SeqCst),
            wcore_swarm::MAX_CONCURRENT_WORKERS,
            "queued children must not start provider work above the shared limit"
        );
        assert_eq!(
            provider.peak.load(Ordering::SeqCst),
            wcore_swarm::MAX_CONCURRENT_WORKERS
        );

        provider.release.add_permits(TOTAL_CHILDREN);
        let (first_results, second_results) = tokio::time::timeout(Duration::from_secs(15), run)
            .await
            .expect("all queued children must run after permits are released")
            .expect("parallel spawn task must not panic");
        assert_eq!(first_results.len(), CHILDREN_PER_CALL);
        assert_eq!(second_results.len(), CHILDREN_PER_CALL);
        assert_eq!(provider.calls.load(Ordering::SeqCst), TOTAL_CHILDREN);
        assert_eq!(provider.active.load(Ordering::SeqCst), 0);
        assert_eq!(
            provider.peak.load(Ordering::SeqCst),
            wcore_swarm::MAX_CONCURRENT_WORKERS
        );
    }

    #[tokio::test]
    async fn queued_parallel_child_honors_parent_cancellation_before_start() {
        const TOTAL_CHILDREN: usize = wcore_swarm::MAX_CONCURRENT_WORKERS + 1;

        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(PeakConcurrencyProvider {
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            release: tokio::sync::Semaphore::new(0),
        });
        let cancel = tokio_util::sync::CancellationToken::new();
        let config = Config {
            model: "test-model".into(),
            provider_label: "test-provider".into(),
            ..Config::default()
        };
        let spawner = AgentSpawner::new(provider.clone(), config)
            .with_parent_workspace(dir.path())
            .unwrap()
            .with_cancel(cancel.clone());
        bind_test_durable_session(&spawner, dir.path(), "f200021");
        let children = (0..TOTAL_CHILDREN)
            .map(|index| bounded_child(&format!("cancel-{index}")))
            .collect();

        let run = tokio::spawn(async move { spawner.spawn_parallel(children).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while provider.active.load(Ordering::SeqCst) < wcore_swarm::MAX_CONCURRENT_WORKERS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the active-child limit must fill before cancellation");

        cancel.cancel();
        let results = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("cancellation must release active and queued children")
            .expect("parallel spawn task must not panic");
        assert_eq!(results.len(), TOTAL_CHILDREN);
        assert!(results.iter().all(|result| result.is_error));
        assert!(results.iter().any(|result| {
            result
                .text
                .contains("parent cancelled before child concurrency admission")
        }));
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            wcore_swarm::MAX_CONCURRENT_WORKERS,
            "the queued child must never reach the provider"
        );
        assert_eq!(provider.active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn child_provider_call_uses_parent_session_reservation() {
        let provider = Arc::new(CountingErrorProvider {
            calls: AtomicUsize::new(0),
        });
        let tracker = Arc::new(parking_lot::Mutex::new(wcore_budget::BudgetTracker::new(
            wcore_budget::BudgetCap::builder()
                .per_session_tokens(1)
                .build(),
        )));
        let spawner = AgentSpawner::new(provider.clone(), Config::default())
            .with_provider_budget(tracker, "shared-session");

        let result = spawner.spawn_one(bounded_child("bounded")).await;

        assert!(result.is_error, "budget refusal must fail the child loudly");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn transient_governance_reuses_parent_authority_handles() {
        let provider: Arc<dyn LlmProvider> = Arc::new(CountingErrorProvider {
            calls: AtomicUsize::new(0),
        });
        let provider_budget = Arc::new(parking_lot::Mutex::new(wcore_budget::BudgetTracker::new(
            wcore_budget::BudgetCap::builder()
                .per_session_tokens(32)
                .build(),
        )));
        let execution_budget = wcore_budget::ExecutionBudget {
            max_tokens_in: Some(1),
            ..Default::default()
        }
        .start_root();
        let cancel = tokio_util::sync::CancellationToken::new();
        let parent = AgentSpawner::new(Arc::clone(&provider), Config::default())
            .with_provider_budget(Arc::clone(&provider_budget), "shared-session")
            .with_execution_budget(execution_budget.clone())
            .with_cancel(cancel.clone());
        let governance = parent
            .budget_governance()
            .expect("a fully governed parent must export its existing handles");

        let transient =
            AgentSpawner::new(provider, Config::default()).with_budget_governance(governance);

        assert!(Arc::ptr_eq(
            transient
                .provider_budget_tracker
                .as_ref()
                .expect("provider budget must transfer"),
            &provider_budget,
        ));
        assert_eq!(
            transient.budget_session_id.as_deref(),
            Some("shared-session")
        );
        transient
            .execution_budget
            .as_ref()
            .expect("execution budget must transfer")
            .record_tokens(2, 0);
        assert_eq!(
            execution_budget.first_exceeded_reason(),
            Some("max_tokens_in"),
            "the transferred view must share the parent's execution ledger"
        );
        cancel.cancel();
        assert!(
            transient.active_cancel_token().is_cancelled(),
            "the transient spawner must inherit parent cancellation"
        );
    }

    #[test]
    fn durable_child_depth_commits_admission_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let journal = crate::session_journal::SessionJournal::open(
            dir.path().join("session.journal"),
            "session",
        )
        .unwrap();
        let session = json!({
            "id": "session",
            "schema_version": 1,
            "messages": [],
        });
        journal
            .append(crate::session_journal::SessionEvent::SessionImported {
                source_schema_version: 1,
                session_digest: crate::session_journal::state_payload_digest(&session).unwrap(),
                session,
            })
            .unwrap();
        let authority = crate::budget_authority::BudgetAuthorityCoordinator::bind(
            crate::budget_authority::BudgetAuthorityConfig {
                journal: Some(journal.clone()),
                budget_session_id: "session-budget".to_owned(),
                provider_caps: wcore_budget::BudgetCap::default(),
                preserve_committed_session_extensions: false,
                execution_policy: wcore_budget::ExecutionBudget {
                    max_agent_depth: Some(2),
                    ..Default::default()
                },
                wall_clock: crate::session_journal::BudgetWallClockAuthority::ActiveRuntime,
                process_cleanup_proof: None,
            },
        )
        .unwrap()
        .into_shared();
        let provider: Arc<dyn LlmProvider> = Arc::new(CountingErrorProvider {
            calls: AtomicUsize::new(0),
        });
        let spawner = AgentSpawner::new(provider, Config::default()).with_budget_governance(
            SpawnerBudgetGovernance::from_authority(
                Arc::clone(&authority),
                tokio_util::sync::CancellationToken::new(),
            ),
        );
        let initial_epoch = authority.lock().authority_epoch();

        let (_child, guard) = spawner
            .enter_child_budget()
            .expect("durable authority admits the child");
        let admitted = journal.state().unwrap().budget_authority.unwrap();
        assert_eq!(admitted.authority_epoch, initial_epoch + 1);
        let admitted_view =
            wcore_budget::ExecutionBudgetView::from_snapshot(admitted.execution_root).unwrap();
        assert_eq!(admitted_view.observed_for("max_agent_depth"), "1");

        drop(guard);
        let released = journal.state().unwrap().budget_authority.unwrap();
        assert_eq!(released.authority_epoch, initial_epoch + 2);
        let released_view =
            wcore_budget::ExecutionBudgetView::from_snapshot(released.execution_root).unwrap();
        assert_eq!(released_view.observed_for("max_agent_depth"), "0");
        assert_eq!(authority.lock().authority_epoch(), released.authority_epoch);
    }

    #[tokio::test]
    async fn cancelling_legacy_parallel_spawn_aborts_running_children() {
        let dir = tempfile::tempdir().unwrap();
        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let provider = Arc::new(HangingProvider {
            started: Mutex::new(Some(started_tx)),
            dropped: Mutex::new(Some(dropped_tx)),
            calls: AtomicUsize::new(0),
        });
        let config = Config {
            model: "test-model".into(),
            provider_label: "test-provider".into(),
            ..Config::default()
        };
        let spawner = AgentSpawner::new(provider.clone(), config)
            .with_parent_workspace(dir.path())
            .unwrap();
        bind_test_durable_session(&spawner, dir.path(), "f190021");
        let child = SubAgentConfig {
            name: "hanging-child".into(),
            prompt: "wait".into(),
            max_turns: 2,
            max_tokens: 16,
            system_prompt: None,
            provider: None,
            model: None,
            temperature: None,
        };

        // This is the model-facing legacy no-relay path. Its own cancellation
        // must abort the raw child task even though the session token remains live.
        let parent = tokio::spawn(async move { spawner.spawn_parallel(vec![child]).await });
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .expect("legacy child must reach the provider")
            .expect("started notifier must fire");

        parent.abort();
        let _ = parent.await;

        tokio::time::timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("legacy child must be dropped promptly")
            .expect("drop notifier must fire");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            1,
            "an aborted child must not resume provider activity"
        );
    }
}

#[cfg(test)]
mod crucible_provider_resolution_tests {
    //! Crucible T2/T4 — per-spawn provider resolution + model override.
    //!
    //! These guard the cross-provider council at the spawn layer: a pinned
    //! `SubAgentConfig.provider` must resolve to *that* provider (not the
    //! parent), an unpinned spawn must inherit the parent, and a cloned
    //! spawner (the relay/fleet path) must still carry the resolver.

    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::mpsc;
    use wcore_config::config::Config;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::spawner::{
        ChildDeliveryState, ChildDeliveryTarget, ChildDesiredState, ChildId, ChildOrigin,
        ChildParent, ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace,
        ChildWorkspaceMode, DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus,
        RequestedChildWorkspace,
    };

    use super::{AgentSpawner, DurableSessionAuthority, ForkOverrides, SubAgentConfig};
    use crate::orchestration::council::{ProviderResolver, ResolveError};

    /// A provider that never streams — identity is all these tests check.
    struct StubProvider;

    #[async_trait]
    impl LlmProvider for StubProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            Err(ProviderError::Connection("stub".into()))
        }
    }

    /// Test resolver mapping a spec string to a specific provider `Arc`.
    struct MapResolver {
        map: HashMap<String, Arc<dyn LlmProvider>>,
    }

    impl ProviderResolver for MapResolver {
        fn resolve_provider(
            &self,
            spec: &str,
        ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
            self.map
                .get(spec)
                .cloned()
                .map(|p| (p, None))
                .ok_or_else(|| ResolveError::Unknown(spec.to_string()))
        }
    }

    fn sub(name: &str, provider: Option<&str>) -> SubAgentConfig {
        SubAgentConfig {
            name: name.into(),
            prompt: "x".into(),
            max_turns: 1,
            max_tokens: 16,
            system_prompt: None,
            provider: provider.map(|s| s.into()),
            model: None,
            temperature: None,
        }
    }

    fn resolver_mapping(specs: &[(&str, Arc<dyn LlmProvider>)]) -> Arc<dyn ProviderResolver> {
        let map = specs
            .iter()
            .map(|(s, p)| (s.to_string(), p.clone()))
            .collect();
        Arc::new(MapResolver { map })
    }

    #[test]
    fn provider_for_unpinned_returns_parent() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent.clone(), Config::default());
        let got = spawner.provider_for(&sub("p", None)).expect("unpinned ok");
        assert!(Arc::ptr_eq(&got, &parent));
    }

    #[test]
    fn provider_for_pinned_returns_resolved_not_parent() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let pinned: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent.clone(), Config::default())
            .with_provider_resolver(resolver_mapping(&[("openai", pinned.clone())]));
        let got = spawner
            .provider_for(&sub("p", Some("openai")))
            .expect("pinned ok");
        assert!(Arc::ptr_eq(&got, &pinned), "pinned provider must be used");
        assert!(!Arc::ptr_eq(&got, &parent), "parent must NOT be used");
    }

    #[test]
    fn provider_for_pinned_without_resolver_errors() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default());
        // `Arc<dyn LlmProvider>` is not Debug, so match instead of expect_err.
        let err = match spawner.provider_for(&sub("p", Some("openai"))) {
            Err(e) => e,
            Ok(_) => panic!("pinned-without-resolver must error"),
        };
        assert!(err.is_error);
        assert!(err.text.contains("no provider resolver"));
    }

    #[test]
    fn provider_for_unknown_pinned_errors() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default())
            .with_provider_resolver(resolver_mapping(&[]));
        let err = match spawner.provider_for(&sub("p", Some("nope"))) {
            Err(e) => e,
            Ok(_) => panic!("unknown pinned provider must error"),
        };
        assert!(err.is_error);
    }

    #[test]
    fn clone_for_spawn_preserves_resolver() {
        // The footgun guard: a cloned spawner (relay/fleet path) must still
        // resolve pinned providers — else proposers silently use the parent.
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let pinned: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default())
            .with_provider_resolver(resolver_mapping(&[("openai", pinned.clone())]));
        let cloned = spawner.clone_for_spawn();
        let got = cloned
            .provider_for(&sub("p", Some("openai")))
            .expect("cloned spawner resolves");
        assert!(
            Arc::ptr_eq(&got, &pinned),
            "cloned spawner must still resolve the pinned provider"
        );
    }

    #[test]
    fn clone_for_spawn_preserves_exact_parent_egress_policy() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let policy: wcore_egress::SharedPolicy = Arc::new(wcore_egress::AllowAllPolicy);
        let spawner =
            AgentSpawner::new(parent, Config::default()).with_egress_policy(policy.clone());
        let cloned = spawner.clone_for_spawn();

        assert!(Arc::ptr_eq(&spawner.egress_policy, &policy));
        assert!(Arc::ptr_eq(&cloned.egress_policy, &policy));
    }

    #[test]
    fn child_config_applies_model_override() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default());
        let mut c = sub("p", None);
        c.model = Some("claude-opus-4-8".into());
        let cfg = spawner.child_config(&c);
        assert_eq!(cfg.model, "claude-opus-4-8");
    }

    /// #112 — a per-spawn cap is always deliberate: the child config must mark
    /// it EXPLICIT so the child engine never omits the wire max-tokens field,
    /// even when the parent session omitted `--max-tokens` on an omit-safe
    /// provider (flux/openrouter/gemini). Otherwise Spawn/council children on
    /// a desktop-default flux session would drop their sized cap on the wire
    /// and could emit the served model's full ceiling, busting the sub-agent /
    /// CouncilSpend worst-case math.
    #[test]
    fn child_config_marks_per_spawn_cap_explicit() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        // Parent: omit-safe provider compat AND an omitted (defaulted) cap —
        // the exact configuration where the parent itself WOULD omit.
        let base = Config {
            compat: wcore_config::compat::ProviderCompat::flux_router_defaults(),
            max_tokens_explicit: false,
            ..Config::default()
        };
        assert!(base.compat.omit_max_tokens_when_unsized());
        let spawner = AgentSpawner::new(parent, base);
        let cfg = spawner.child_config(&sub("p", None));
        assert!(
            cfg.max_tokens_explicit,
            "a spawned child's per-spawn cap must read as explicit (never omitted on the wire)"
        );
    }

    #[test]
    fn budget_tracker_attaches_and_survives_clone_for_spawn() {
        let tracker = std::sync::Arc::new(parking_lot::Mutex::new(
            wcore_budget::BudgetTracker::new(wcore_budget::BudgetCap::default()),
        ));
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let s = AgentSpawner::new(parent, Config::default()).with_budget_tracker(tracker.clone());
        assert!(s.budget_tracker().is_some());
        assert!(s.clone_for_spawn().budget_tracker().is_some());
    }

    #[test]
    fn resolved_launch_preserves_override_model_request_evidence_and_shared_binding() {
        let dir = tempfile::tempdir().unwrap();
        let manager = crate::session::SessionManager::new(dir.path().to_path_buf(), 10);
        let session = manager
            .create("test", "parent-model", "/tmp", Some("f19000c"))
            .unwrap();
        manager.persist_first_message(&session).unwrap();
        let active = manager.load_for_run(&session.id).unwrap();
        let authority = DurableSessionAuthority::new();
        let token = authority.bind(active.journal, &session.id).unwrap();

        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let pinned: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default())
            .with_provider_resolver(resolver_mapping(&[("openai", pinned.clone())]))
            .with_parent_workspace(dir.path())
            .unwrap()
            .with_durable_session_authority(
                authority,
                wcore_types::execution_policy::EffectiveExecutionPolicy::baseline(
                    &Config::default().execution_policy,
                ),
            )
            .unwrap();
        let mut request = sub("child", Some("openai"));
        request.model = Some("request-model".into());
        let overrides = ForkOverrides {
            model: Some("override-model".into()),
            effort: Some("high".into()),
            allowed_tools: vec!["Read".into(), "Grep".into()],
        };
        let launch = spawner
            .resolve_durable_launch(request.clone(), overrides.clone())
            .unwrap();

        assert!(Arc::ptr_eq(&launch.provider(), &pinned));
        assert_eq!(launch.provider_id(), "openai");
        assert_eq!(launch.model(), "override-model");
        assert_eq!(launch.config().model, "override-model");
        assert_eq!(launch.authority(), &token);
        assert_eq!(
            spawner
                .clone_for_spawn()
                .resolve_durable_launch(request.clone(), overrides.clone())
                .unwrap()
                .authority(),
            &token,
            "transient clones must observe the exact session generation"
        );

        let exact = super::DurableSpawner::request_digest(&request, &overrides).unwrap();
        let mut different_effort = overrides.clone();
        different_effort.effort = Some("low".into());
        assert_ne!(
            exact,
            super::DurableSpawner::request_digest(&request, &different_effort).unwrap()
        );
        let mut different_tools = overrides;
        different_tools.allowed_tools = vec!["Read".into()];
        assert_ne!(
            exact,
            super::DurableSpawner::request_digest(&request, &different_tools).unwrap()
        );
    }

    #[test]
    fn durable_record_reports_requested_and_realized_workspace_truth() {
        let dir = tempfile::tempdir().unwrap();
        let manager = crate::session::SessionManager::new(dir.path().to_path_buf(), 10);
        let session = manager
            .create("test", "parent-model", "/tmp", Some("f200001"))
            .unwrap();
        manager.persist_first_message(&session).unwrap();
        let active = manager.load_for_run(&session.id).unwrap();
        let authority = DurableSessionAuthority::new();
        authority.bind(active.journal, &session.id).unwrap();

        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default())
            .with_parent_workspace(dir.path())
            .unwrap()
            .with_durable_session_authority(
                authority,
                wcore_types::execution_policy::EffectiveExecutionPolicy::baseline(
                    &Config::default().execution_policy,
                ),
            )
            .unwrap();
        let mut request = sub("workspace-child", None);
        request.model = Some("workspace-model".into());

        let read_only = spawner
            .resolve_durable_launch(request.clone(), ForkOverrides::default())
            .unwrap();
        assert_eq!(
            read_only.requested_workspace(),
            RequestedChildWorkspace::SharedReadOnly
        );
        assert_eq!(
            read_only.workspace().mode,
            ChildWorkspaceMode::SharedReadOnly
        );
        assert!(read_only.workspace().workspace_id.starts_with("shared-"));
        let read_only_record = read_only.durable_record(ChildOrigin::Spawn, None).unwrap();
        assert_eq!(read_only_record.workspace, read_only.workspace().clone());

        let mutating = ForkOverrides {
            allowed_tools: vec!["Write".into()],
            ..ForkOverrides::default()
        };
        assert!(
            spawner
                .resolve_durable_launch(request.clone(), mutating.clone())
                .is_err(),
            "mutating launch must not fall back to the parent workspace"
        );

        let forged_root = spawner
            .pre_resolve_durable_launch(request.clone(), mutating.clone())
            .unwrap();
        assert!(
            spawner
                .resolve_durable_launch_in_workspace(
                    forged_root,
                    ChildId::new("isolated-f20-child").unwrap(),
                    ChildWorkspace {
                        mode: ChildWorkspaceMode::Isolated,
                        workspace_id: "isolated-f20-child".into(),
                    },
                    dir.path().to_path_buf(),
                    vec![dir.path().to_path_buf()],
                    None,
                )
                .is_err(),
            "an internal caller cannot forge an isolated workspace root"
        );

        let forged_id = spawner
            .pre_resolve_durable_launch(request.clone(), mutating.clone())
            .unwrap();
        assert!(
            spawner
                .resolve_durable_launch_in_workspace(
                    forged_id,
                    ChildId::new("different-f20-child").unwrap(),
                    ChildWorkspace {
                        mode: ChildWorkspaceMode::Isolated,
                        workspace_id: "isolated-f20-child".into(),
                    },
                    dir.path().to_path_buf(),
                    vec![dir.path().to_path_buf()],
                    None,
                )
                .is_err(),
            "an internal caller cannot forge child/workspace identity"
        );

        let forged_deny = spawner
            .pre_resolve_durable_launch(request, mutating)
            .unwrap();
        assert!(
            spawner
                .resolve_durable_launch_in_workspace(
                    forged_deny,
                    ChildId::new("isolated-f20-child").unwrap(),
                    ChildWorkspace {
                        mode: ChildWorkspaceMode::Isolated,
                        workspace_id: "isolated-f20-child".into(),
                    },
                    dir.path().to_path_buf(),
                    Vec::new(),
                    None,
                )
                .is_err(),
            "an internal caller cannot omit parent authority deny roots"
        );
    }

    fn record_for_launch(
        id: &str,
        launch: &super::ResolvedChildLaunch,
        request: &SubAgentConfig,
        overrides: &ForkOverrides,
    ) -> DurableChildRecord {
        DurableChildRecord {
            schema_version: DURABLE_CHILD_SCHEMA_VERSION,
            declaration_id: format!("declare-{id}"),
            child_id: ChildId::new(id).unwrap(),
            parent: ChildParent {
                session_id: launch.authority().session_id().to_owned(),
                turn_id: None,
                parent_child_id: None,
                workflow_run_id: None,
                graph_node_id: None,
                parent_call_id: None,
            },
            origin: ChildOrigin::Spawn,
            request: ChildRequestEvidence::redacted(
                super::DurableSpawner::request_digest(request, overrides).unwrap(),
            ),
            policy_snapshot: launch.policy_snapshot().clone(),
            provider: Some(launch.provider_id().to_owned()),
            model: Some(launch.model().to_owned()),
            workspace: ChildWorkspace {
                mode: ChildWorkspaceMode::Isolated,
                workspace_id: "workspace-f19".into(),
            },
            status: DurableChildStatus::Prepared,
            desired_state: ChildDesiredState::Run,
            recovery: ChildRecoveryState::Clean,
            revision: 0,
            timestamps: ChildTimestamps {
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
                queued_at_unix_ms: None,
                started_at_unix_ms: None,
                terminal_at_unix_ms: None,
            },
            result: None,
            delivery_target: Some(ChildDeliveryTarget::SessionOutbox),
            delivery_state: ChildDeliveryState::Pending,
            attempt: 1,
            retry_of: None,
            applied_events: Default::default(),
        }
    }

    #[test]
    fn stale_resolved_launch_cannot_declare_into_switched_session() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let manager_a = crate::session::SessionManager::new(dir.path().join("a"), 10);
        let session_a = manager_a
            .create("test", "model-a", "/tmp", Some("f19000d"))
            .unwrap();
        manager_a.persist_first_message(&session_a).unwrap();
        let active_a = manager_a.load_for_run(&session_a.id).unwrap();
        authority.bind(active_a.journal, &session_a.id).unwrap();

        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default())
            .with_parent_workspace(dir.path())
            .unwrap()
            .with_durable_session_authority(
                authority.clone(),
                wcore_types::execution_policy::EffectiveExecutionPolicy::baseline(
                    &Config::default().execution_policy,
                ),
            )
            .unwrap();
        let request = sub("stale-child", None);
        let overrides = ForkOverrides {
            model: Some("model-a".into()),
            ..ForkOverrides::default()
        };
        let launch = spawner
            .resolve_durable_launch(request.clone(), overrides.clone())
            .unwrap();
        let record = record_for_launch("stale-child", &launch, &request, &overrides);

        let manager_b = crate::session::SessionManager::new(dir.path().join("b"), 10);
        let session_b = manager_b
            .create("test", "model-b", "/tmp", Some("f19000e"))
            .unwrap();
        manager_b.persist_first_message(&session_b).unwrap();
        let active_b = manager_b.load_for_run(&session_b.id).unwrap();
        let token_b = authority.bind(active_b.journal, &session_b.id).unwrap();

        assert!(matches!(
            spawner.declare_resolved_child(&launch, record),
            Err(super::DurableSpawnerError::StaleAuthority { .. })
        ));
        authority
            .with_store(&token_b, |store| {
                assert!(store.list()?.is_empty(), "stale launch must not write B");
                Ok(())
            })
            .unwrap();
    }
}

#[cfg(test)]
mod production_durable_spawn_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use tokio::sync::{Notify, mpsc, oneshot};
    use wcore_config::config::Config;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::{FinishReason, StopReason, TokenUsage};
    use wcore_types::spawner::{
        ChildDesiredState, ChildOrigin, ChildRecoveryState, ChildWorkspaceMode, DurableChildStatus,
    };

    use super::{
        AgentSpawner, DurableCancelDisposition, DurableSessionAuthority, ForkOverrides,
        SWARM_CONTROL_DIR, SpawnExtras, SubAgentConfig,
    };
    use crate::durable_child::DurableChildStore;

    struct ControlledProvider {
        calls: AtomicUsize,
        started: parking_lot::Mutex<Option<oneshot::Sender<()>>>,
        release: Arc<Notify>,
        wait_for_release: bool,
    }

    impl ControlledProvider {
        fn immediate() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                started: parking_lot::Mutex::new(None),
                release: Arc::new(Notify::new()),
                wait_for_release: false,
            })
        }

        fn blocked(started: oneshot::Sender<()>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                started: parking_lot::Mutex::new(Some(started)),
                release: Arc::new(Notify::new()),
                wait_for_release: true,
            })
        }
    }

    #[async_trait]
    impl LlmProvider for ControlledProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = self.started.lock().take() {
                let _ = started.send(());
            }
            if self.wait_for_release {
                self.release.notified().await;
            }
            let (tx, rx) = mpsc::channel(2);
            tx.send(LlmEvent::TextDelta("durable child completed".into()))
                .await
                .unwrap();
            tx.send(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                usage: TokenUsage {
                    input_tokens: 3,
                    output_tokens: 4,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            })
            .await
            .unwrap();
            Ok(rx)
        }
    }

    fn child(name: &str) -> SubAgentConfig {
        SubAgentConfig {
            name: name.into(),
            prompt: "perform durable work".into(),
            max_turns: 1,
            max_tokens: 16,
            system_prompt: None,
            provider: None,
            model: None,
            temperature: None,
        }
    }

    fn canonical_binding(
        root: &std::path::Path,
        seed: &str,
        authority: &DurableSessionAuthority,
    ) -> (
        crate::session::SessionManager,
        crate::session_journal::SessionJournal,
        super::DurableAuthorityToken,
    ) {
        let manager = crate::session::SessionManager::new(root.to_path_buf(), 10);
        let session = manager
            .create("test", "test-model", "/tmp", Some(seed))
            .unwrap();
        manager.persist_first_message(&session).unwrap();
        let active = manager.load_for_run(&session.id).unwrap();
        let journal = active.journal.clone();
        let token = authority.bind(active.journal, &session.id).unwrap();
        (manager, journal, token)
    }

    fn bound_spawner(
        provider: Arc<dyn LlmProvider>,
        authority: DurableSessionAuthority,
        workspace: &std::path::Path,
    ) -> AgentSpawner {
        let config = Config {
            model: "test-model".into(),
            provider_label: "test-provider".into(),
            ..Config::default()
        };
        let policy = wcore_types::execution_policy::EffectiveExecutionPolicy::baseline(
            &config.execution_policy,
        );
        AgentSpawner::new(provider, config)
            .with_parent_workspace(workspace)
            .unwrap()
            .with_durable_session_authority(authority, policy)
            .unwrap()
    }

    fn bound_spawner_with_session_root(
        provider: Arc<dyn LlmProvider>,
        authority: DurableSessionAuthority,
        workspace: &std::path::Path,
        session_root: &std::path::Path,
    ) -> AgentSpawner {
        let mut config = Config {
            model: "test-model".into(),
            provider_label: "test-provider".into(),
            ..Config::default()
        };
        config.session.directory = session_root.display().to_string();
        let policy = wcore_types::execution_policy::EffectiveExecutionPolicy::baseline(
            &config.execution_policy,
        );
        AgentSpawner::new(provider, config)
            .with_parent_workspace(workspace)
            .unwrap()
            .with_durable_session_authority(authority, policy)
            .unwrap()
    }

    #[cfg(target_os = "linux")]
    async fn run_git(repo: &std::path::Path, args: &[&str]) {
        let mut command = wcore_config::shell::shell_command_argv("git", args);
        command.current_dir(repo);
        let output = command.output().await.expect("run git fixture command");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(target_os = "linux")]
    async fn init_git_workspace(repo: &std::path::Path) {
        run_git(repo, &["init"]).await;
        run_git(repo, &["config", "user.email", "wayland@example.invalid"]).await;
        run_git(repo, &["config", "user.name", "Wayland Test"]).await;
        std::fs::write(repo.join("README.md"), "transaction fixture\n").unwrap();
        run_git(repo, &["add", "README.md"]).await;
        run_git(repo, &["commit", "-m", "fixture"]).await;
    }

    #[cfg(target_os = "linux")]
    fn prefill_workspace_leases(session_root: &std::path::Path, count: usize) {
        let leases = session_root.join("delegated-workspaces/leases");
        std::fs::create_dir_all(&leases).unwrap();
        for index in 0..count {
            std::fs::write(leases.join(format!("prefill-{index}.json")), b"{}\n").unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    fn retained_lease_count(session_root: &std::path::Path) -> usize {
        std::fs::read_dir(session_root.join("delegated-workspaces/leases"))
            .unwrap()
            .count()
    }

    // Count admitted *child* checkouts, excluding the orchestrator control
    // directory (`.wayland-control`) that WorktreeManager plants in the swarm
    // root alongside child checkouts.
    #[cfg(target_os = "linux")]
    fn child_checkout_count(session_root: &std::path::Path) -> usize {
        std::fs::read_dir(session_root.join("delegated-workspaces/checkouts"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_str() != Some(SWARM_CONTROL_DIR))
            .count()
    }

    #[cfg(target_os = "linux")]
    fn dangerous_sandbox_runtime() -> Arc<wcore_sandbox::SandboxRegistry> {
        use wcore_types::execution_policy::{
            ApprovalPolicy, BaselineExecutionPolicy, DangerousLaunchRequest, PolicySource,
            resolve_dangerous_launch,
        };

        let baseline =
            BaselineExecutionPolicy::smart(ApprovalPolicy::Prompt, PolicySource::Default);
        let grant = resolve_dangerous_launch(
            &baseline,
            DangerousLaunchRequest::cli(60, "f20-hostile-bypass"),
            10_000,
        )
        .unwrap();
        Arc::new(wcore_sandbox::SandboxRegistry::dangerous(&grant))
    }

    #[tokio::test]
    async fn unbound_production_spawn_fails_before_provider_execution() {
        let dir = tempfile::tempdir().unwrap();
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner = AgentSpawner::new(provider_dyn, Config::default())
            .with_parent_workspace(dir.path())
            .unwrap();

        let result = spawner
            .spawn_one_with_origin(child("unbound"), ChildOrigin::Workflow)
            .await;

        assert!(result.is_error);
        assert!(result.text.contains("session authority is not bound"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn mutating_spawn_from_non_git_workspace_fails_before_provider_execution() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let (_manager, _journal, _token) =
            canonical_binding(&state.path().join("journal"), "f2000001", &authority);
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner = bound_spawner_with_session_root(
            provider_dyn,
            authority,
            workspace.path(),
            &state.path().join("sessions"),
        );

        let result = spawner
            .spawn_durable(
                child("non-git-mutator"),
                ForkOverrides {
                    allowed_tools: vec!["Write".into()],
                    ..ForkOverrides::default()
                },
                SpawnExtras::default(),
                ChildOrigin::Workflow,
            )
            .await;

        assert!(result.is_error);
        assert!(result.text.contains("workspace preparation failed"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn invalid_provider_fails_before_workspace_identity_or_allocation() {
        let workspace = tempfile::tempdir().unwrap();
        init_git_workspace(workspace.path()).await;
        let state = tempfile::tempdir().unwrap();
        let sessions = state.path().join("sessions");
        let authority = DurableSessionAuthority::new();
        let (_manager, journal, _token) =
            canonical_binding(&state.path().join("journal"), "f2000101", &authority);
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner =
            bound_spawner_with_session_root(provider_dyn, authority, workspace.path(), &sessions);
        let mut request = child("invalid-provider-mutator");
        request.provider = Some("missing-provider:model".into());

        let result = spawner
            .spawn_durable(
                request,
                ForkOverrides {
                    allowed_tools: vec!["Write".into()],
                    ..ForkOverrides::default()
                },
                SpawnExtras::default(),
                ChildOrigin::Workflow,
            )
            .await;

        assert!(result.is_error);
        assert!(
            result.text.contains("provider resolution"),
            "{}",
            result.text
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert!(!sessions.join("delegated-workspaces").exists());
        assert!(DurableChildStore::new(journal).list().unwrap().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn dangerous_sandbox_rejects_transactional_mutation_before_allocation() {
        let workspace = tempfile::tempdir().unwrap();
        init_git_workspace(workspace.path()).await;
        let state = tempfile::tempdir().unwrap();
        let sessions = state.path().join("sessions");
        let authority = DurableSessionAuthority::new();
        let (_manager, journal, _token) =
            canonical_binding(&state.path().join("journal"), "f2000102", &authority);
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner =
            bound_spawner_with_session_root(provider_dyn, authority, workspace.path(), &sessions)
                .with_sandbox_runtime(dangerous_sandbox_runtime());

        let result = spawner
            .spawn_durable(
                child("dangerous-mutator"),
                ForkOverrides {
                    allowed_tools: vec!["Write".into()],
                    ..ForkOverrides::default()
                },
                SpawnExtras::default(),
                ChildOrigin::Workflow,
            )
            .await;

        assert!(result.is_error);
        assert!(
            result
                .text
                .contains("requires an enforcing sandbox backend"),
            "{}",
            result.text
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert!(!sessions.join("delegated-workspaces").exists());
        assert!(DurableChildStore::new(journal).list().unwrap().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn full_workspace_quota_rejects_without_new_lease_checkout_or_parent_ref() {
        let workspace = tempfile::tempdir().unwrap();
        init_git_workspace(workspace.path()).await;
        let state = tempfile::tempdir().unwrap();
        let sessions = state.path().join("sessions");
        prefill_workspace_leases(&sessions, wcore_swarm::MAX_RETAINED_WORKTREES);
        let authority = DurableSessionAuthority::new();
        let (_manager, journal, _token) =
            canonical_binding(&state.path().join("journal"), "f2000103", &authority);
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner =
            bound_spawner_with_session_root(provider_dyn, authority, workspace.path(), &sessions);
        let parent_head = {
            let mut command =
                wcore_config::shell::shell_command_argv("git", &["rev-parse", "--verify", "HEAD"]);
            command.current_dir(workspace.path());
            String::from_utf8(command.output().await.unwrap().stdout)
                .unwrap()
                .trim()
                .to_owned()
        };

        let result = spawner
            .spawn_durable(
                child("quota-full-mutator"),
                ForkOverrides {
                    allowed_tools: vec!["Write".into()],
                    ..ForkOverrides::default()
                },
                SpawnExtras::default(),
                ChildOrigin::Workflow,
            )
            .await;

        assert!(result.is_error);
        assert!(
            result.text.contains("evidence quota is full"),
            "{}",
            result.text
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            retained_lease_count(&sessions),
            wcore_swarm::MAX_RETAINED_WORKTREES
        );
        // The checkouts root doubles as the WorktreeManager swarm root, which
        // always contains the orchestrator's `.wayland-control` directory once a
        // manager is constructed (it is planted before the quota check). That is
        // infrastructure, not a child checkout — the rejection must leave zero
        // *child* checkouts behind, so exclude the control directory.
        assert_eq!(child_checkout_count(&sessions), 0);
        let mut command =
            wcore_config::shell::shell_command_argv("git", &["rev-parse", "--verify", "HEAD"]);
        command.current_dir(workspace.path());
        let after = String::from_utf8(command.output().await.unwrap().stdout)
            .unwrap()
            .trim()
            .to_owned();
        assert_eq!(after, parent_head);
        assert!(DurableChildStore::new(journal).list().unwrap().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn concurrent_near_cap_admits_exactly_one_retained_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        init_git_workspace(workspace.path()).await;
        let state = tempfile::tempdir().unwrap();
        let sessions = state.path().join("sessions");
        prefill_workspace_leases(
            &sessions,
            wcore_swarm::MAX_RETAINED_WORKTREES.saturating_sub(1),
        );
        let authority = DurableSessionAuthority::new();
        let (_manager, journal, _token) =
            canonical_binding(&state.path().join("journal"), "f2000104", &authority);
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner =
            bound_spawner_with_session_root(provider_dyn, authority, workspace.path(), &sessions);
        let overrides = ForkOverrides {
            allowed_tools: vec!["Write".into()],
            ..ForkOverrides::default()
        };

        let (left, right) = tokio::join!(
            spawner.prepare_durable_launch(child("near-cap-left"), overrides.clone()),
            spawner.prepare_durable_launch(child("near-cap-right"), overrides),
        );

        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let quota_error = left.err().or_else(|| right.err()).unwrap().to_string();
        assert!(
            quota_error.contains("evidence quota is full"),
            "{quota_error}"
        );
        assert_eq!(
            retained_lease_count(&sessions),
            wcore_swarm::MAX_RETAINED_WORKTREES
        );
        // Exactly one launch was admitted (see the `is_ok` sum above). Both
        // launch handles are consumed by the `.err()` chain above, so the
        // admitted child's `TransactionWorkspace` has been dropped and its
        // checkout removed by transaction cleanup; the only directory remaining
        // in the swarm root is the orchestrator's `.wayland-control`.
        assert_eq!(
            std::fs::read_dir(sessions.join("delegated-workspaces/checkouts"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert!(DurableChildStore::new(journal).list().unwrap().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn isolated_workspace_identity_and_lease_survive_failed_declaration() {
        let workspace = tempfile::tempdir().unwrap();
        init_git_workspace(workspace.path()).await;
        let state = tempfile::tempdir().unwrap();
        let sessions = state.path().join("sessions");
        let authority = DurableSessionAuthority::new();
        let (_manager_a, journal_a, _token_a) =
            canonical_binding(&state.path().join("a"), "f2000002", &authority);
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner = bound_spawner_with_session_root(
            provider_dyn,
            authority.clone(),
            workspace.path(),
            &sessions,
        );
        let launch = spawner
            .prepare_durable_launch(
                child("isolated-mutator"),
                ForkOverrides {
                    allowed_tools: vec!["Write".into(), "Bash".into()],
                    ..ForkOverrides::default()
                },
            )
            .await
            .expect("prepare isolated workspace");
        let record = launch
            .durable_record(ChildOrigin::Workflow, None)
            .expect("construct durable record");

        assert_eq!(record.child_id, *launch.child_id());
        assert_eq!(record.workspace, *launch.workspace());
        assert_eq!(record.workspace.mode, ChildWorkspaceMode::Isolated);
        assert!(
            record
                .workspace
                .workspace_id
                .ends_with(launch.child_id().as_str())
        );
        assert!(launch.workspace_root().starts_with(&sessions));
        assert!(!launch.workspace_root().starts_with(workspace.path()));
        assert!(launch.workspace_root().join(".git").is_dir());
        launch.validate_record(&record).unwrap();
        assert_eq!(launch.authority_read_deny.len(), 2);
        let registry = spawner.child_tool_registry(&launch);
        assert!(registry.get("Write").is_some());
        assert!(registry.get("Bash").is_some());
        let child_policy = registry.workspace_policy().expect("child workspace policy");
        assert_eq!(child_policy.root(), launch.workspace_root());
        let writable = child_policy.writable_roots();
        for authority_root in &launch.authority_read_deny {
            assert!(
                writable.iter().all(|allowed| {
                    !authority_root.starts_with(allowed) && !allowed.starts_with(authority_root)
                }),
                "authority root leaked into child write grants: {}",
                authority_root.display()
            );
        }

        let lease = sessions
            .join("delegated-workspaces/leases")
            .join(format!("{}.json", launch.child_id().as_str()));
        let lease_value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&lease).expect("durable workspace lease"))
                .unwrap();
        assert_eq!(lease_value["child_id"], launch.child_id().as_str());
        assert_eq!(lease_value["workspace_id"], record.workspace.workspace_id);
        assert_eq!(
            lease_value["checkout_root"],
            launch.workspace_root().to_string_lossy().as_ref()
        );

        let (_manager_b, journal_b, _token_b) =
            canonical_binding(&state.path().join("b"), "f2000003", &authority);
        let result = spawner
            .execute_durable_launch(launch, SpawnExtras::default(), ChildOrigin::Workflow)
            .await;

        assert!(result.is_error);
        assert!(result.text.contains("authority is stale"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert!(lease.is_file(), "failed declaration lost workspace locator");
        assert!(DurableChildStore::new(journal_a).list().unwrap().is_empty());
        assert!(DurableChildStore::new(journal_b).list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn production_spawn_commits_exact_origin_and_terminal_result_once() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let (_manager, _journal, token) = canonical_binding(dir.path(), "f1920001", &authority);
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner = bound_spawner(provider_dyn, authority.clone(), dir.path());

        let result = spawner
            .spawn_one_with_origin(child("workflow-child"), ChildOrigin::Workflow)
            .await;

        assert!(!result.is_error, "{}", result.text);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        authority
            .with_store(&token, |store| {
                let records = store.list()?;
                assert_eq!(records.len(), 1);
                let record = &records[0];
                assert_eq!(record.origin, ChildOrigin::Workflow);
                assert_eq!(record.provider.as_deref(), Some("test-provider"));
                assert_eq!(record.model.as_deref(), Some("test-model"));
                assert_eq!(record.status, DurableChildStatus::Succeeded);
                assert!(record.result.is_some());
                Ok(())
            })
            .unwrap();
    }

    #[tokio::test]
    async fn anvil_seat_clone_preserves_session_authority_and_terminalizes_once() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let (_manager, _journal, token) = canonical_binding(dir.path(), "f1920006", &authority);
        let template_provider: Arc<dyn LlmProvider> = ControlledProvider::immediate();
        let template = bound_spawner(template_provider, authority.clone(), dir.path());
        let seat_provider = ControlledProvider::immediate();
        let seat_provider_dyn: Arc<dyn LlmProvider> = seat_provider.clone();
        let seat_config = Config {
            model: "anvil-model".into(),
            provider_label: "anvil-provider".into(),
            ..Config::default()
        };
        let seat = template.clone_for_resolved_config(seat_provider_dyn, seat_config);

        let result = seat
            .spawn_one_with_origin(child("anvil-builder"), ChildOrigin::Anvil)
            .await;

        assert!(!result.is_error, "{}", result.text);
        assert_eq!(seat_provider.calls.load(Ordering::SeqCst), 1);
        authority
            .with_store(&token, |store| {
                let records = store.list()?;
                assert_eq!(records.len(), 1);
                let record = &records[0];
                assert_eq!(record.parent.session_id, "f1920006");
                assert_eq!(record.origin, ChildOrigin::Anvil);
                assert_eq!(record.provider.as_deref(), Some("anvil-provider"));
                assert_eq!(record.model.as_deref(), Some("anvil-model"));
                assert_eq!(record.status, DurableChildStatus::Succeeded);
                Ok(())
            })
            .unwrap();
    }

    #[tokio::test]
    async fn authority_switch_before_atomic_admission_never_calls_provider_or_declares_child() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let (_manager_a, journal_a, _token_a) =
            canonical_binding(&dir.path().join("a"), "f1920004", &authority);
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner = bound_spawner(provider_dyn, authority.clone(), dir.path());

        // Resolve under A, pause before the exact production admission helper,
        // then switch the shared authority to B. Admission must reject the
        // stale token before it creates or polls the provider future.
        let launch = spawner
            .resolve_durable_launch(child("stale-before-admission"), ForkOverrides::default())
            .unwrap();
        let (_manager_b, journal_b, _token_b) =
            canonical_binding(&dir.path().join("b"), "f1920005", &authority);
        let result = spawner
            .execute_durable_launch(launch, SpawnExtras::default(), ChildOrigin::Workflow)
            .await;

        assert!(result.is_error);
        assert!(
            result
                .text
                .contains("durable child launch authority is stale")
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert!(DurableChildStore::new(journal_a).list().unwrap().is_empty());
        assert!(DurableChildStore::new(journal_b).list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn in_flight_child_finishes_in_captured_session_after_authority_switch() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let (_manager_a, journal_a, _token_a) =
            canonical_binding(&dir.path().join("a"), "f1920002", &authority);
        let (started_tx, started_rx) = oneshot::channel();
        let provider = ControlledProvider::blocked(started_tx);
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner = bound_spawner(provider_dyn, authority.clone(), dir.path());

        let task = tokio::spawn(async move {
            spawner
                .spawn_one_with_origin(child("session-a-child"), ChildOrigin::Pipeline)
                .await
        });
        started_rx.await.unwrap();

        let (_manager_b, _journal_b, token_b) =
            canonical_binding(&dir.path().join("b"), "f1920003", &authority);
        provider.release.notify_waiters();
        let result = task.await.unwrap();
        assert!(!result.is_error, "{}", result.text);

        let records_a = DurableChildStore::new(journal_a).list().unwrap();
        assert_eq!(records_a.len(), 1);
        assert_eq!(records_a[0].origin, ChildOrigin::Pipeline);
        assert_eq!(records_a[0].status, DurableChildStatus::Succeeded);
        authority
            .with_store(&token_b, |store| {
                assert!(store.list()?.is_empty());
                Ok(())
            })
            .unwrap();
    }

    #[tokio::test]
    async fn host_child_uses_the_session_supervisor_for_admission_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let (_manager, _journal, _token) = canonical_binding(dir.path(), "f1920009", &authority);
        let (started_tx, started_rx) = oneshot::channel();
        let provider = ControlledProvider::blocked(started_tx);
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner = Arc::new(bound_spawner(provider_dyn, authority, dir.path()));
        let supervisor = spawner.durable_child_supervisor().unwrap();
        let task_spawner = Arc::clone(&spawner);
        let task =
            tokio::spawn(async move { task_spawner.spawn_host_child(child("host-created")).await });
        started_rx.await.unwrap();

        let records = supervisor.list().unwrap();
        assert_eq!(records.len(), 1);
        let child_id = records[0].child_id.clone();
        assert_eq!(records[0].origin, ChildOrigin::Host);
        assert_eq!(records[0].status, DurableChildStatus::Running);
        assert_eq!(
            supervisor.inspect(&child_id).unwrap(),
            Some(records[0].clone())
        );
        assert_eq!(
            supervisor.request_cancel(&child_id).unwrap(),
            DurableCancelDisposition::Signalled
        );

        let result = task.await.unwrap();
        assert!(result.is_error);
        assert_eq!(result.name, "host-created");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        let record = supervisor.inspect(&child_id).unwrap().unwrap();
        assert_eq!(record.status, DurableChildStatus::Cancelled);
        assert_eq!(record.desired_state, ChildDesiredState::Cancel);
        assert_eq!(record.recovery, ChildRecoveryState::Clean);
        assert!(record.result.is_none());
    }

    #[tokio::test]
    async fn stale_supervisor_never_follows_a_session_rebind() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let (_manager_a, _journal_a, _token_a) =
            canonical_binding(&dir.path().join("a"), "f192000a", &authority);
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let spawner = bound_spawner(provider_dyn, authority.clone(), dir.path());
        let stale = spawner.durable_child_supervisor().unwrap();

        let (_manager_b, _journal_b, _token_b) =
            canonical_binding(&dir.path().join("b"), "f192000b", &authority);
        let result = spawner.spawn_host_child(child("session-b-host")).await;
        assert!(!result.is_error, "{}", result.text);
        let current = spawner.durable_child_supervisor().unwrap();
        let record = current.list().unwrap().pop().unwrap();

        assert!(stale.list().is_err());
        assert!(stale.inspect(&record.child_id).is_err());
        assert!(stale.request_cancel(&record.child_id).is_err());
        assert_eq!(current.inspect(&record.child_id).unwrap(), Some(record));
    }

    #[tokio::test]
    async fn resolved_child_keeps_the_turn_token_that_created_it() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let (_manager, _journal, _token) = canonical_binding(dir.path(), "f192000c", &authority);
        let provider = ControlledProvider::immediate();
        let provider_dyn: Arc<dyn LlmProvider> = provider.clone();
        let root = tokio_util::sync::CancellationToken::new();
        let mut guard = crate::cancel::SessionRuntimeGuard::new(root);
        let first_turn = tokio_util::sync::CancellationToken::new();
        guard.set_active_turn(first_turn);
        let spawner = bound_spawner(provider_dyn, authority, dir.path())
            .with_session_runtime(guard.observer());
        let launch = spawner
            .resolve_durable_launch(child("old-turn-child"), ForkOverrides::default())
            .unwrap();

        guard.set_active_turn(tokio_util::sync::CancellationToken::new());
        let result = spawner
            .execute_durable_launch(launch, SpawnExtras::default(), ChildOrigin::Workflow)
            .await;

        assert!(result.is_error);
        assert_eq!(result.name, "old-turn-child");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let record = spawner.durable_child_supervisor().unwrap().list().unwrap();
        assert_eq!(record.len(), 1);
        assert_eq!(record[0].status, DurableChildStatus::Cancelled);
    }
}

#[cfg(test)]
mod phase7_tests {
    use std::sync::Arc;

    use super::{AgentSpawner, ForkOverrides, SubAgentConfig, build_tool_registry};
    use wcore_config::config::Config;
    use wcore_providers::LlmProvider;
    use wcore_types::spawner::RequestedChildWorkspace;

    fn test_sandbox_runtime() -> Arc<wcore_sandbox::SandboxRegistry> {
        wcore_tools::registry::ToolRegistry::new().sandbox_runtime()
    }

    #[test]
    fn tc_7_1_fork_overrides_default_values() {
        let o = ForkOverrides::default();
        assert!(o.model.is_none());
        assert!(o.effort.is_none());
        assert!(o.allowed_tools.is_empty());
        assert_eq!(
            o.requested_workspace(),
            RequestedChildWorkspace::SharedReadOnly
        );
    }

    #[test]
    fn workspace_request_classification_is_conservative() {
        for tools in [vec!["Read"], vec!["Read", "Grep", "Glob"]] {
            let overrides = ForkOverrides {
                allowed_tools: tools.into_iter().map(str::to_owned).collect(),
                ..ForkOverrides::default()
            };
            assert_eq!(
                overrides.requested_workspace(),
                RequestedChildWorkspace::SharedReadOnly
            );
        }

        for tool in ["Write", "Edit", "Bash", "FutureTool", "read"] {
            let overrides = ForkOverrides {
                allowed_tools: vec![tool.to_owned()],
                ..ForkOverrides::default()
            };
            assert_eq!(
                overrides.requested_workspace(),
                RequestedChildWorkspace::IsolatedMutation,
                "unknown or mutating tool {tool:?} must request isolation"
            );
        }
    }

    // Security audit H-7 / M-9: an empty `allowed` list must default to the
    // READ-ONLY subset (Read/Grep/Glob) — never the full toolset. A `Delegate`
    // call that omits `toolsets` must not silently grant the child
    // Bash/Write/Edit.
    #[test]
    fn tc_7_40_build_tool_registry_empty_allowed_is_read_only() {
        let root = tempfile::tempdir().unwrap();
        let registry = build_tool_registry(
            &[],
            RequestedChildWorkspace::SharedReadOnly,
            root.path(),
            &[],
            test_sandbox_runtime(),
        );
        // Read-only tools ARE registered.
        for name in &["Read", "Grep", "Glob"] {
            assert!(
                registry.get(name).is_some(),
                "read-only tool '{name}' should be registered by default"
            );
        }
        // Destructive tools are NOT registered without explicit opt-in.
        for name in &["Write", "Edit", "Bash"] {
            assert!(
                registry.get(name).is_none(),
                "destructive tool '{name}' must NOT be registered on an empty toolset (H-7)"
            );
        }
    }

    // Security audit H-7: destructive tools are reachable ONLY when explicitly
    // named in `allowed` (the opt-in path).
    #[test]
    fn tc_7_42_build_tool_registry_destructive_requires_opt_in() {
        let root = tempfile::tempdir().unwrap();
        let registry = build_tool_registry(
            &["Bash".to_string(), "Write".to_string()],
            RequestedChildWorkspace::IsolatedMutation,
            root.path(),
            &[],
            test_sandbox_runtime(),
        );
        assert!(
            registry.get("Bash").is_some(),
            "explicit Bash opt-in honored"
        );
        assert!(
            registry.get("Write").is_some(),
            "explicit Write opt-in honored"
        );
        // A read-only tool not in the explicit list is excluded (explicit list
        // is authoritative — it is NOT additive over the read-only default).
        assert!(
            registry.get("Read").is_none(),
            "Read excluded when an explicit allow-list omits it"
        );
    }

    #[test]
    fn tc_7_43_build_tool_registry_filters_to_allowed() {
        let root = tempfile::tempdir().unwrap();
        let allowed = vec!["Bash".to_string(), "Read".to_string()];
        let registry = build_tool_registry(
            &allowed,
            RequestedChildWorkspace::IsolatedMutation,
            root.path(),
            &[],
            test_sandbox_runtime(),
        );
        assert!(registry.get("Bash").is_some());
        assert!(registry.get("Read").is_some());
        assert!(registry.get("Write").is_none());
    }

    #[test]
    fn shared_registry_rejects_explicit_mutating_tools_and_bash() {
        let root = tempfile::tempdir().unwrap();
        let requested = vec![
            "Read".to_owned(),
            "Write".to_owned(),
            "Edit".to_owned(),
            "Bash".to_owned(),
        ];
        let registry = build_tool_registry(
            &requested,
            RequestedChildWorkspace::SharedReadOnly,
            root.path(),
            &[],
            test_sandbox_runtime(),
        );

        assert!(registry.get("Read").is_some());
        for name in ["Write", "Edit", "Bash"] {
            assert!(registry.get(name).is_none(), "shared child exposed {name}");
        }
    }

    #[test]
    fn child_registry_inherits_exact_parent_sandbox_runtime() {
        let root = tempfile::tempdir().unwrap();
        let runtime = test_sandbox_runtime();
        let registry = build_tool_registry(
            &[],
            RequestedChildWorkspace::SharedReadOnly,
            root.path(),
            &[],
            Arc::clone(&runtime),
        );

        assert!(Arc::ptr_eq(&runtime, &registry.sandbox_runtime()));
    }

    #[test]
    fn cloned_spawner_preserves_exact_parent_sandbox_runtime() {
        let root = tempfile::tempdir().unwrap();
        let runtime = test_sandbox_runtime();
        let provider: Arc<dyn LlmProvider> =
            Arc::new(crate::test_utils::ScriptedProvider::new(Vec::new()));
        let spawner = AgentSpawner::new(provider, Config::default())
            .with_parent_workspace(root.path())
            .unwrap()
            .with_sandbox_runtime(Arc::clone(&runtime));
        let cloned = spawner.clone_for_spawn();

        assert!(Arc::ptr_eq(&runtime, spawner.sandbox_runtime()));
        assert!(Arc::ptr_eq(&runtime, cloned.sandbox_runtime()));
        assert_eq!(spawner.parent_workspace, cloned.parent_workspace);
        let expected = std::fs::canonicalize(root.path()).unwrap();
        assert_eq!(
            cloned.parent_workspace.as_ref().map(|path| path.as_path()),
            Some(expected.as_path())
        );
    }

    #[test]
    fn tc_7_sub_agent_config_original_fields_intact() {
        let config = SubAgentConfig {
            name: "test-agent".to_string(),
            prompt: "do the task".to_string(),
            max_turns: 5,
            max_tokens: 1024,
            system_prompt: Some("you are helpful".to_string()),
            provider: None,
            model: None,
            temperature: None,
        };
        assert_eq!(config.name, "test-agent");
        assert_eq!(config.max_turns, 5);
    }
}

#[cfg(test)]
mod posture_inheritance_tests {
    //! Security audit H-7 / M-9 — a spawned sub-agent must inherit the parent's
    //! approval posture. The bug was `config.tools.auto_approve = true` forced
    //! on every spawn, so a parent that prompts for Bash/Write/Edit was
    //! silently bypassed by a `Delegate`/`Spawn` call. These tests assert the
    //! child config built by `AgentSpawner::child_config` carries the parent's
    //! typed approval policy, legacy `auto_approve`, and `allow_list` unchanged.

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use tokio::sync::mpsc;
    use wcore_config::compat::ProviderCompat;
    use wcore_config::config::{Config, ProviderType, ToolsConfig};
    use wcore_protocol::ToolApprovalManager;
    use wcore_protocol::commands::SessionMode;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_types::execution_policy::{
        ApprovalPolicy, BaselineExecutionPolicy, EffectiveExecutionPolicy, ManagedDangerousPolicy,
    };
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::spawner::{
        ChildDeliveryState, ChildDesiredState, ChildRecoveryState, DurableChildStatus,
    };

    use super::{AgentSpawner, SubAgentConfig};
    use crate::confirm::ToolConfirmer;
    use crate::durable_child::DurableChildStore;

    /// Minimal `LlmProvider` stub — `child_config` never calls `stream`, so an
    /// immediate error return is sufficient to satisfy the trait bound.
    struct NeverProvider;

    #[async_trait]
    impl LlmProvider for NeverProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            Err(ProviderError::Connection("never called".into()))
        }
    }

    struct CountingNeverProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmProvider for CountingNeverProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::Connection("never called".into()))
        }
    }

    fn config_with_posture(auto_approve: bool, allow_list: Vec<String>) -> Config {
        Config {
            tools: ToolsConfig {
                auto_approve,
                allow_list,
                skills: wcore_config::config::SkillsPermissionConfig::default(),
                verify_edits: false,
                windows_shell: None,
                env_passthrough: Vec::new(),
                sandbox: None,
                allow_no_sandbox: None,
            },
            ..Default::default()
        }
    }

    fn sub_config() -> SubAgentConfig {
        SubAgentConfig {
            name: "child".to_string(),
            prompt: "do the task".to_string(),
            max_turns: 3,
            max_tokens: 512,
            system_prompt: None,
            provider: None,
            model: None,
            temperature: None,
        }
    }

    #[test]
    fn parent_auto_approve_false_yields_child_auto_approve_false() {
        let parent = config_with_posture(false, vec!["Read".to_string()]);
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), parent);

        let child = spawner.child_config(&sub_config());

        assert!(
            !child.tools.auto_approve,
            "child must inherit parent's auto_approve=false (H-7 / M-9)"
        );
        assert_eq!(
            child.tools.allow_list,
            vec!["Read".to_string()],
            "child must inherit parent's allow_list unchanged"
        );
    }

    #[test]
    fn parent_auto_approve_true_is_still_honored() {
        // The fix must not invert behavior for a parent that genuinely opted
        // into auto-approve — the child still auto-approves in that case.
        let parent = config_with_posture(true, vec![]);
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), parent);

        let child = spawner.child_config(&sub_config());

        assert!(
            child.tools.auto_approve,
            "child must inherit parent's auto_approve=true"
        );
    }

    #[test]
    fn typed_posture_tracks_live_manager_through_child_clones() {
        let mut parent = config_with_posture(false, vec![]);
        parent.set_smart_approval_policy(ApprovalPolicy::AutoEdit);
        let manager = Arc::new(ToolApprovalManager::new());
        manager.set_mode(SessionMode::AutoEdit);
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), parent)
            .with_approval_manager(Arc::clone(&manager));

        let direct_child = spawner.child_config(&sub_config());

        assert_eq!(
            direct_child.smart_approval_policy(),
            ApprovalPolicy::AutoEdit,
            "direct children must inherit the typed AutoEdit posture"
        );
        manager.set_mode(SessionMode::Default);
        let cloned_child = spawner.clone_for_spawn().child_config(&sub_config());
        assert_eq!(cloned_child.smart_approval_policy(), ApprovalPolicy::Prompt);
        assert!(
            !cloned_child.tools.auto_approve,
            "runtime de-escalation must revoke bypass for fleet/parallel children"
        );
    }

    #[test]
    fn routed_seat_preserves_canonical_policy_and_budget_authority() {
        let mut authority = config_with_posture(false, vec!["Read".into()]);
        authority.execution_policy =
            BaselineExecutionPolicy::managed(ApprovalPolicy::Prompt, ManagedDangerousPolicy::Deny);
        authority.budget.max_cost_usd = Some(3.0);
        authority.session_cap = Some(wcore_budget::BudgetConfig {
            max_tokens_out: Some(2_048),
            max_cost_usd: Some(1.0),
            ..Default::default()
        });

        let mut routed = Config {
            provider_label: "flux-router".into(),
            provider: ProviderType::FluxRouter,
            api_key: "route-secret".into(),
            base_url: "https://router.invalid/v1".into(),
            model: "flux-auto".into(),
            compat: ProviderCompat::flux_router_defaults(),
            ..Config::default()
        };
        routed.tools.auto_approve = true;
        routed.execution_policy = BaselineExecutionPolicy::smart(
            ApprovalPolicy::Bypass,
            wcore_types::execution_policy::PolicySource::UserConfig,
        );
        routed.budget = wcore_budget::BudgetConfig::default();
        routed.session_cap = None;

        let canonical_receipt_policy =
            EffectiveExecutionPolicy::baseline(&authority.execution_policy);
        let template = AgentSpawner::new(Arc::new(NeverProvider), authority.clone());
        let seat = template.clone_for_resolved_config(Arc::new(NeverProvider), routed.clone());
        let child = seat.child_config(&sub_config());

        assert_eq!(child.provider, ProviderType::FluxRouter);
        assert_eq!(child.provider_label, "flux-router");
        assert_eq!(child.model, "flux-auto");
        assert_eq!(child.compat.provider_type(), "flux-router");
        assert!(!child.tools.auto_approve);
        assert_eq!(child.execution_policy, authority.execution_policy);
        assert_eq!(child.budget, authority.budget);
        assert_eq!(child.session_cap, authority.session_cap);
        assert_eq!(
            EffectiveExecutionPolicy::baseline(&child.execution_policy),
            canonical_receipt_policy,
            "durable receipt policy must match child-engine enforcement",
        );
        assert_eq!(
            child.smart_approval_policy(),
            canonical_receipt_policy.approvals(),
            "legacy confirmer posture must agree with the durable receipt",
        );
        let confirmer = ToolConfirmer::with_policy(
            child.smart_approval_policy(),
            child.tools.allow_list.clone(),
        );
        assert!(
            confirmer.requires_confirmation("Bash"),
            "the real child confirmer must not bypass a Managed/Prompt floor",
        );

        let standalone = AgentSpawner::new(Arc::new(NeverProvider), routed)
            .with_session_authority_config(&authority);
        let standalone_child = standalone.child_config(&sub_config());
        assert_eq!(
            standalone_child.execution_policy,
            authority.execution_policy
        );
        assert_eq!(standalone_child.budget, authority.budget);
        assert_eq!(standalone_child.session_cap, authority.session_cap);
        assert_eq!(standalone_child.provider, ProviderType::FluxRouter);
        assert_eq!(standalone_child.model, "flux-auto");
        assert_eq!(
            standalone_child.smart_approval_policy(),
            canonical_receipt_policy.approvals(),
        );
    }

    /// FIX F — workflow shadow-detection is a top-level/user-turn signal. A
    /// child engine spawned by a workflow must have the gate OFF even when the
    /// parent has it ON, so sub-agent turns don't pollute the shadow log with
    /// recursive intra-workflow detections. Asserted on the cached gate at the
    /// child-config seam (`child_config` is the single place children are built).
    #[test]
    fn child_config_disables_workflow_detection_even_when_parent_enables_it() {
        let mut parent = Config::default();
        parent.observability.workflow_detection_enabled = true;
        // B6 defense-in-depth: the live confirm gate must also be forced off for
        // children so a workflow's sub-agents can never recursively re-enter it.
        parent.observability.workflow_live_mode = true;
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), parent);

        let child = spawner.child_config(&sub_config());

        assert!(
            !child.observability.workflow_detection_enabled,
            "workflow-spawned child must have workflow_detection forced off"
        );
        assert!(
            !child.observability.workflow_live_mode,
            "workflow-spawned child must have the live confirm gate forced off"
        );
    }

    /// Crucible enhancement #1 — a council member must get a minimal,
    /// council-specific system prompt instead of inheriting the host one. With
    /// the parent carrying a sentinel host prompt, the child config built from a
    /// `SubAgentConfig` that supplies an explicit `system_prompt` must equal that
    /// minimal prompt and must NOT contain the host sentinel (which would mean
    /// the multi-K-token host prompt is being re-billed × N members).
    #[test]
    fn council_proposer_system_prompt_replaces_host_prompt() {
        let parent = Config {
            system_prompt: Some("HOST-SECRET-PROMPT".to_string()),
            ..Config::default()
        };
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), parent);

        let sub = SubAgentConfig {
            name: "p".to_string(),
            prompt: "task".to_string(),
            max_turns: 2,
            max_tokens: 16,
            system_prompt: Some("MINIMAL COUNCIL".to_string()),
            provider: None,
            model: None,
            temperature: None,
        };
        let child = spawner.child_config(&sub);

        assert_eq!(
            child.system_prompt.as_deref(),
            Some("MINIMAL COUNCIL"),
            "child must use the explicit minimal council system prompt"
        );
        assert!(
            !child.system_prompt.unwrap().contains("HOST-SECRET-PROMPT"),
            "child must NOT inherit the host system prompt (no re-billing × N)"
        );
    }

    #[tokio::test]
    async fn production_spawner_and_clones_read_latest_session_turn() {
        let root = tokio_util::sync::CancellationToken::new();
        let mut guard = crate::cancel::SessionRuntimeGuard::new(root);
        let runtime = guard.observer();
        let first_turn = tokio_util::sync::CancellationToken::new();
        guard.set_active_turn(first_turn.clone());
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), Config::default())
            .with_session_runtime(runtime.clone());
        let first_child = spawner.active_cancel_token().child_token();

        let second_turn = tokio_util::sync::CancellationToken::new();
        guard.set_active_turn(second_turn.clone());
        let cloned_spawner = spawner.clone_for_spawn();
        let second_child = cloned_spawner.active_cancel_token().child_token();

        first_turn.cancel();
        assert!(first_child.is_cancelled());
        assert!(
            !second_child.is_cancelled(),
            "a completed prior turn must not cancel a later child"
        );
        second_turn.cancel();
        assert!(second_child.is_cancelled());
    }

    /// Rank 7 — a host cancel must propagate into spawned sub-agents. With the
    /// parent token already fired, the child engine observes `is_cancelled()`
    /// at its first turn boundary and returns WITHOUT reaching the provider
    /// (`NeverProvider::stream` errors with "never called" if hit). The absence
    /// of that error proves the child inherited the parent's cancel token.
    #[tokio::test]
    async fn cancelled_parent_short_circuits_spawned_child() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let provider = Arc::new(CountingNeverProvider {
            calls: AtomicUsize::new(0),
        });
        let config = Config {
            model: "test-model".into(),
            provider_label: "test-provider".into(),
            ..Config::default()
        };
        let spawner = AgentSpawner::new(provider.clone(), config)
            .with_parent_workspace(dir.path())
            .unwrap()
            .with_cancel(cancel);
        let manager = crate::session::SessionManager::new(dir.path().to_path_buf(), 10);
        let active = manager
            .create_for_run(
                "test-provider",
                "test-model",
                &dir.path().to_string_lossy(),
                Some("f1920007"),
            )
            .unwrap();
        let journal = active.journal.clone();
        spawner
            .bind_durable_session(active.journal, &active.session.id)
            .unwrap();

        let result = spawner.spawn_one(sub_config()).await;

        assert!(result.is_error);
        assert_eq!(result.name, "child");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let records = DurableChildStore::new(journal).list().unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.status, DurableChildStatus::Cancelled);
        assert_eq!(record.desired_state, ChildDesiredState::Cancel);
        assert_eq!(record.recovery, ChildRecoveryState::Clean);
        assert!(record.result.is_none());
        assert_eq!(record.delivery_state, ChildDeliveryState::NotRequired);
        assert!(record.timestamps.terminal_at_unix_ms.is_some());
    }
}

#[cfg(test)]
mod fail_loud_tests {
    use super::{relay_subagent_terminal, subagent_ok_result};
    use crate::agents::channel_sink::{
        ChannelSink, SubAgentRelay, SubAgentTerminalRelay, TERMINAL_CAPACITY,
    };
    use wcore_protocol::events::WorkflowChildTerminalState;
    use wcore_types::message::{FinishReason, StopReason, TokenUsage};

    fn agent_result(text: &str, finish: FinishReason) -> crate::engine::AgentResult {
        crate::engine::AgentResult {
            text: text.to_string(),
            // stop_reason is hardcoded to MaxTurns by finish_run_terminated
            // regardless of the real cause, which is why subagent_ok_result
            // branches on finish_reason, not stop_reason.
            stop_reason: StopReason::MaxTurns,
            finish_reason: finish,
            usage: TokenUsage::default(),
            usage_delta: TokenUsage::default(),
            turns: 3,
            active_window_percent: None,
            agent_run_id: None,
        }
    }

    #[test]
    fn terminated_empty_run_is_error_with_synthesized_cause() {
        // #661: a sub-agent that hit the turn cap with no output must be an
        // error carrying a legible cause, not a silent empty success.
        let out = subagent_ok_result("child".into(), agent_result("", FinishReason::MaxTurns));
        assert!(out.is_error, "a non-Stop finish must be flagged is_error");
        assert!(
            out.text.contains("terminated") && out.text.contains("turn limit"),
            "empty terminated body gets a cause line, got: {}",
            out.text
        );
    }

    #[test]
    fn token_capped_answer_with_text_is_usable_not_error() {
        // A complete answer that ends exactly at the output-token cap comes back
        // as Length WITH text — degraded-but-usable, not a failure. Flagging it
        // would wrongly drop it from council quorum. Keep text, is_error=false.
        let out = subagent_ok_result(
            "child".into(),
            agent_result("the answer", FinishReason::Length),
        );
        assert!(!out.is_error, "a non-empty Length result must stay usable");
        assert_eq!(out.text, "the answer");
    }

    #[test]
    fn empty_length_termination_is_error_with_cause() {
        // An EMPTY Length (the context/budget-ceiling abort path) produced no
        // answer → error with a synthesized cause, not a silent empty success.
        let out = subagent_ok_result("child".into(), agent_result("", FinishReason::Length));
        assert!(
            out.is_error,
            "an empty Length termination is a real failure"
        );
        assert!(
            out.text.contains("context, budget, or output-length limit"),
            "cause line names the limit, got: {}",
            out.text
        );
    }

    #[test]
    fn clean_completion_is_success() {
        // A clean EndTurn (FinishReason::Stop) is the only unconditional success.
        let out = subagent_ok_result("child".into(), agent_result("done", FinishReason::Stop));
        assert!(!out.is_error);
        assert_eq!(out.text, "done");
    }

    #[tokio::test]
    async fn final_result_drives_typed_terminal_disposition() {
        let (stream_tx, _stream_rx) = tokio::sync::mpsc::channel::<SubAgentRelay>(1);
        let (terminal_tx, mut terminal_rx) =
            tokio::sync::mpsc::channel::<SubAgentTerminalRelay>(TERMINAL_CAPACITY);
        let sink = ChannelSink::new_with_terminal(
            "workflow:scan".into(),
            "scan".into(),
            stream_tx,
            terminal_tx,
        );
        let result = subagent_ok_result("scan".into(), agent_result("", FinishReason::MaxTurns));

        relay_subagent_terminal(Some(&sink), &result);

        let terminal = terminal_rx.recv().await.expect("terminal result");
        assert_eq!(terminal.terminal_state, WorkflowChildTerminalState::Failed);
        assert_eq!(terminal.relay.inner["type"], "error");
        assert!(
            terminal.relay.inner["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("turn limit"))
        );
    }
}
