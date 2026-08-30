//! The provider-neutral execution-backend contract — F25-01 in one place.
//!
//! Every surface F25-01 names is a NAMED type on this trait rather than a
//! free-form blob: declared capabilities, the effective policy for a task
//! (including where the egress decision came from and which secrets are
//! exposed), secret provisioning, artifact transfer, resource limits,
//! cancellation, attestation of backend identity, receipt emission and
//! lifecycle health.
//!
//! The trait is object-safe and `async_trait`-based so it composes with
//! `wcore_sandbox::backends::SandboxBackend`, which has the same shape.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{ExecError, Result};
use crate::policy::EffectivePolicy;
use crate::receipt::ExecutionReceipt;

/// Which transport family a reference backend belongs to. This is a
/// DIVERGENT field by construction: it is exactly what the four reference
/// backends are supposed to differ on, so it never enters the normalized
/// receipt body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Local,
    Container,
    Ssh,
    Cloud,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Local => "local",
            BackendKind::Container => "container",
            BackendKind::Ssh => "ssh",
            BackendKind::Cloud => "cloud",
        }
    }
}

/// Resource request and backend ceiling. Modelled on the F04 remote-execution
/// oracle's `ResourceBudget`, including its rule that a zero in ANY field is
/// invalid — a zero budget is not "unlimited", it is a malformed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceBudget {
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub wall_time_ms: u64,
    pub output_bytes: u64,
}

impl ResourceBudget {
    pub fn new(
        cpu_millis: u64,
        memory_bytes: u64,
        wall_time_ms: u64,
        output_bytes: u64,
    ) -> Result<Self> {
        let budget = Self {
            cpu_millis,
            memory_bytes,
            wall_time_ms,
            output_bytes,
        };
        budget.validate()?;
        Ok(budget)
    }

    pub fn validate(self) -> Result<()> {
        if self.cpu_millis == 0
            || self.memory_bytes == 0
            || self.wall_time_ms == 0
            || self.output_bytes == 0
        {
            return Err(ExecError::InvalidResourceBudget);
        }
        Ok(())
    }
}

/// Which resource a denial names. Same set as the oracle's `ResourceKind`, so
/// a production denial and a fixture denial are directly comparable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    CpuMillis,
    MemoryBytes,
    WallTimeMs,
    OutputBytes,
}

/// One file materialised into the task workspace before execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceFile {
    /// Relative path inside the workspace. Must not escape it.
    pub path: String,
    #[serde(with = "crate::b64")]
    pub bytes: Vec<u8>,
}

/// A deterministic unit of work submitted to any backend.
///
/// Determinism is what makes the four-way equivalence diff meaningful: a task
/// whose output embeds a timestamp, a hostname or a random value cannot prove
/// equivalence, so nothing here is allowed to vary per host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionTask {
    pub task_id: String,
    /// Per-task label carried onto every remote surface — the container label,
    /// the remote process-group marker, the cloud machine tag — so an orphan
    /// scan has something concrete to look for.
    pub nonce: String,
    pub workspace: Vec<WorkspaceFile>,
    #[serde(with = "crate::b64")]
    pub input: Vec<u8>,
    /// argv, never a shell string. Element 0 is the program.
    pub argv: Vec<String>,
    pub artifact_name: String,
    pub resources: ResourceBudget,
}

pub const INPUT_FILE_NAME: &str = "input.bin";

impl ExecutionTask {
    pub fn validate(&self) -> Result<()> {
        validate_identifier("task_id", &self.task_id)?;
        validate_identifier("nonce", &self.nonce)?;
        if self.argv.is_empty() {
            return Err(ExecError::MalformedTask("argv is empty".into()));
        }
        validate_relative_name("artifact_name", &self.artifact_name)?;
        for file in &self.workspace {
            validate_relative_name("workspace path", &file.path)?;
        }
        self.resources.validate()
    }

    /// Content address of the whole workspace. Path-ordered so two hosts that
    /// enumerate a directory differently still agree.
    pub fn workspace_sha256(&self) -> String {
        let ordered: BTreeMap<&str, &[u8]> = self
            .workspace
            .iter()
            .map(|f| (f.path.as_str(), f.bytes.as_slice()))
            .collect();
        let mut hasher = Sha256::new();
        for (path, bytes) in ordered {
            hasher.update((path.len() as u64).to_be_bytes());
            hasher.update(path.as_bytes());
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
        hex(&hasher.finalize())
    }

    pub fn input_sha256(&self) -> String {
        crate::receipt::sha256(&self.input)
    }
}

/// What a backend declares it can do, before anyone asks it to do anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub backend_id: String,
    pub kind: BackendKind,
    pub version: String,
    /// The ceiling this backend will accept. A task requesting more than any
    /// field of this is denied BEFORE acceptance.
    pub limits: ResourceBudget,
    pub supports_artifact_transfer: bool,
    pub supports_cancellation: bool,
    /// True only when the backend can OBSERVE a hibernation transition, not
    /// merely request one. See `HibernationObservation`.
    pub supports_hibernation: bool,
    pub secret_channel: SecretChannel,
}

/// How secrets reach the executing task, declared per backend so a caller can
/// refuse a backend whose channel it does not trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretChannel {
    /// Secrets are never provisioned into the task at all.
    None,
    /// Process environment of the sandboxed child on this host.
    LocalProcessEnv,
    /// Container environment, set at create time.
    ContainerEnv,
    /// Written to the remote side over the already-authenticated transport.
    RemoteTransport,
    /// Handed to the vendor API, which stores it for the machine.
    VendorManaged,
}

/// Why a backend claims to be available or unavailable, and from WHAT probe.
///
/// The basis is part of the answer, not decoration: a container backend that
/// answers from socket presence is claiming readiness it has not established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeBasis {
    /// The platform sandbox registry selected a real containment backend.
    SandboxBackendProbe,
    /// A real round-trip to the container daemon, not socket presence.
    DaemonPing,
    /// A real ssh connection that reached the far end.
    SshHandshake,
    /// A real authenticated call to the vendor API.
    VendorApiCall,
    /// No credential is configured, so the surface was never dialled.
    CredentialAbsent,
    /// The probe itself could not be run.
    ProbeFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Availability {
    pub available: bool,
    pub probe: ProbeBasis,
    pub detail: String,
}

impl Availability {
    pub fn up(probe: ProbeBasis, detail: impl Into<String>) -> Self {
        Self {
            available: true,
            probe,
            detail: detail.into(),
        }
    }

    pub fn down(probe: ProbeBasis, detail: impl Into<String>) -> Self {
        Self {
            available: false,
            probe,
            detail: detail.into(),
        }
    }
}

/// Lifecycle health, distinct from availability: a backend can be reachable
/// and still be degraded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub healthy: bool,
    pub detail: String,
    /// Tasks this backend currently believes it owns.
    pub live_tasks: usize,
}

/// What the backend observed about a hibernating machine, as opposed to what
/// it asked for.
///
/// `NotObserved` exists because of binding condition C1 recorded in
/// `25-01-panel-dissent.txt`: a backend that can only drive `stop` has NOT
/// observed hibernation and must not claim it. Making that a distinct variant
/// puts the condition in the type system instead of leaving it to reviewer
/// vigilance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HibernationObservation {
    /// The backend has no hibernation surface at all.
    NotApplicable,
    /// A hibernation surface exists but this run did not observe a transition.
    NotObserved { reason: String },
    /// Observed transitions, read back from the vendor rather than inferred
    /// from the request that asked for them.
    Observed { transitions: Vec<String> },
}

/// What a backend saw when it cleaned up after a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupObservation {
    pub task_id: String,
    pub backend_id: String,
    /// Free-text description of the enumeration that was actually performed.
    pub method: String,
    /// Residual surfaces still carrying the task nonce AFTER cleanup. Zero is
    /// the only passing value; a non-empty vector is an orphan finding.
    pub residual: Vec<String>,
}

impl CleanupObservation {
    pub fn is_clean(&self) -> bool {
        self.residual.is_empty()
    }
}

/// What an orphan scan found. Plan 25-04 prosecutes this hostilely; the
/// contract only has to make it answerable per backend kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanScan {
    pub backend_id: String,
    pub kind: BackendKind,
    /// The nonce the scan was RESTRICTED to, or `None` when it was not
    /// restricted to one run at all.
    ///
    /// A sentinel string was the obvious alternative and is the wrong shape:
    /// "this scan covered every run" and "this scan covered the run called
    /// `<sentinel>`" are different facts, and a reader that cannot tell them
    /// apart is how core#366 went unseen — every caller supplied a nonce that
    /// could not match a leftover, and the answer read as "no orphans".
    pub nonce: Option<String>,
    pub method: String,
    pub found: Vec<String>,
    /// False when the scan could not actually enumerate — an unscannable
    /// surface must never be reported as zero orphans.
    pub enumerated: bool,
}

/// Which runs an orphan scan covers.
///
/// # Why this is a parameter and not a convention (core#366 d2)
///
/// The scope used to be implicit: `scan_orphans(nonce)` was the only shape a
/// caller could write, so a caller that wanted "every run" had no way to say
/// so and silently got "the one run whose nonce I already hold" — a question
/// that can never return a previous run's leftover. The remedy that names one
/// call site and moves it is the same shape as the defect: it leaves every
/// OTHER caller, and every caller written next month, with the same silent
/// default. `wayland-core backend scan` was exactly that caller, and it
/// reported a MEASURED zero over a real labelled leftover.
///
/// So there is no default. Every entry point that is not a backend
/// implementing its own enumeration takes an `OrphanScope`, which means the
/// complete set of callers and what each one asks is derived BY THE COMPILER —
/// a new caller cannot compile without choosing — rather than from a
/// hand-written list that is stale the moment somebody adds the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanScope<'a> {
    /// One run's residue, and nothing else. What `cancel()` wants: it
    /// re-enumerates by the cancelled task's own nonce to check its own
    /// removal, and a wider scope would make it report other tasks' surfaces
    /// as its residual.
    Nonce(&'a str),
    /// Every surface this product created, whatever run created it —
    /// including runs this process never held a nonce for.
    AnyNonce,
}

impl<'a> OrphanScope<'a> {
    /// The run this scope names, or `None` when it names no single run.
    ///
    /// # The one place the MEANING of a scope is written down
    ///
    /// [`ExecutionBackend::scan_orphans_in_scope`] routes a scope to an
    /// enumeration, and [`OrphanScan::nonce`] records the scope the answer
    /// came back in. Those are two hand-written statements about the same
    /// fact, and until they were checked against each other a dispatch arm
    /// could answer a NARROWER question than it was asked and nothing would
    /// say so — see the guard in `scan_orphans_in_scope`, whose red arm is
    /// exactly that mutation.
    pub fn nonce(&self) -> Option<&'a str> {
        match *self {
            OrphanScope::Nonce(nonce) => Some(nonce),
            OrphanScope::AnyNonce => None,
        }
    }

    /// The scope in words, for an operator-facing line.
    pub fn label(&self) -> String {
        match self {
            OrphanScope::Nonce(nonce) => format!("run {nonce} only"),
            OrphanScope::AnyNonce => "EVERY run (unscoped, core#366)".to_string(),
        }
    }
}

/// The provider-neutral execution backend.
#[async_trait]
pub trait ExecutionBackend: Send + Sync {
    fn capabilities(&self) -> &BackendCapabilities;

    /// Liveness, from a REAL probe. Implementations state which probe in the
    /// returned `ProbeBasis`.
    async fn availability(&self) -> Availability;

    /// The policy that WOULD apply to this task: the egress decision and its
    /// source, and the exact set of secret names that would be exposed.
    /// Computed before acceptance so a caller can refuse.
    fn effective_policy(&self, task: &ExecutionTask) -> Result<EffectivePolicy>;

    /// Run the task to a terminal state and emit an attested receipt.
    ///
    /// A resource request the backend cannot satisfy is denied BEFORE
    /// acceptance: the returned receipt then carries a `ResourceDenied`
    /// terminal and NO `task_accepted` event.
    async fn execute(&self, task: &ExecutionTask) -> Result<ExecutionReceipt>;

    /// Cancel a task by id, from this process or another one, and report what
    /// cleanup was observed. Closing a local connection is NOT cancellation.
    async fn cancel(&self, task_id: &str) -> Result<CleanupObservation>;

    async fn health(&self) -> Result<Health>;

    /// Enumerate surfaces still carrying `nonce`.
    ///
    /// DELIBERATELY still nonce-scoped. `cancel()` wants exactly one run's
    /// residue — it re-enumerates by the cancelled task's nonce to check its
    /// own removal, and widening this would make it report other tasks'
    /// surfaces as its own leftovers. core#366 d2: this contract does not move.
    async fn scan_orphans(&self, nonce: &str) -> Result<OrphanScan>;

    /// Enumerate every surface THIS PRODUCT created, whatever run created it.
    ///
    /// # The question `scan_orphans` structurally cannot answer (core#366)
    ///
    /// A nonce is fresh per run, so a scan for the nonce a caller is holding
    /// can never return a PREVIOUS run's leftover: not from `cancel()`, whose
    /// nonce comes from the live registry a finished run has already cleared,
    /// and not from ordinary operation. The product could only ever ask "are
    /// there orphans from the run whose nonce I already hold". This is the
    /// other question — "are there surfaces left over from ANY run" — and it
    /// is the one that finds a task that ran once, wedged, and was never
    /// submitted again.
    ///
    /// # This REPORTS. It does not reclaim (core#366 d6)
    ///
    /// DECIDED, not left open. #365's submit-path reclaim may remove a
    /// conflicting container because it holds the exact task id it is about to
    /// use, which is a claim on that name. An unscoped scan holds no claim on
    /// anything it finds: the label says "some wayland run created this", not
    /// "this run is over". A found surface may belong to a LIVE task in
    /// another process (the registry is per-process state) or to another
    /// tenant of a shared daemon, and destroying either would be a worse
    /// failure than the leak this exists to surface. Reclamation stays with
    /// the paths that hold a claim.
    ///
    /// # The default is "cannot", never "clean"
    ///
    /// A backend with no unscoped enumeration answers `enumerated: false` and
    /// an empty `found`. Per this module's standing rule, an un-enumerated
    /// surface is NOT a clean surface, and a new backend inherits the honest
    /// answer rather than a silent zero.
    async fn scan_orphans_any_nonce(&self) -> Result<OrphanScan> {
        let capabilities = self.capabilities();
        Ok(OrphanScan {
            backend_id: capabilities.backend_id.clone(),
            kind: capabilities.kind,
            nonce: None,
            method: format!(
                "{} has NO unscoped enumeration: it cannot list the surfaces it created \
                 without being told which run to look for",
                capabilities.backend_id
            ),
            found: Vec::new(),
            enumerated: false,
        })
    }

    /// Scan in an EXPLICIT scope. The one entry point for a caller that is not
    /// a backend enumerating its own surfaces.
    ///
    /// core#366 d2. This dispatches to the two methods above and adds no
    /// behaviour of its own, which is the point: the nonce-scoped contract
    /// `cancel()` depends on is untouched, and at the same time no caller can
    /// reach the scanner without stating which of the two questions it is
    /// asking. `scan_orphans` is still there and still nonce-scoped, so
    /// choosing the scoped question stays possible; what is no longer possible
    /// is getting it by default and never knowing there was another one.
    async fn scan_orphans_in_scope(&self, scope: OrphanScope<'_>) -> Result<OrphanScan> {
        let scan = match scope {
            OrphanScope::Nonce(nonce) => self.scan_orphans(nonce).await?,
            OrphanScope::AnyNonce => self.scan_orphans_any_nonce().await?,
        };
        // FAIL CLOSED ON THE ROUTING ITSELF (core#366 d2).
        //
        // Making the scope a parameter moved the caller set from a note to the
        // compiler, and left exactly one hand-written decision behind: these
        // two arms. A reviewer mutated the unscoped arm to
        // `self.scan_orphans("<a nonce nobody holds>")`. It compiled, the
        // whole suite stayed green, and `backend scan` went back to reporting
        // `count 0 (MEASURED)` over a real labelled leftover — the defect this
        // ticket exists to close, fully restored, invisibly.
        //
        // So the routing is not trusted either. The scan states the scope it
        // was taken in; the caller stated the scope it asked for; a dispatch
        // that answers a different question than it was asked is an ERROR
        // here rather than a MEASURED zero downstream. The asymmetry is the
        // usual one: a loud failure costs an operator a re-run, while a quiet
        // narrower answer is indistinguishable from a clean host.
        if scan.nonce.as_deref() != scope.nonce() {
            return Err(ExecError::Exec(format!(
                "core#366 d2: a scan asked in scope `{}` came back declaring scope {:?}. \
                 A scan that answers a narrower question than the caller asked reports \
                 zero over a leftover it never looked for, which is precisely this \
                 defect. backend={} method={}",
                scope.label(),
                scan.nonce,
                scan.backend_id,
                scan.method
            )));
        }
        Ok(scan)
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).expect("nibble is < 16"));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).expect("nibble is < 16"));
    }
    out
}

pub fn validate_identifier(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 {
        return Err(ExecError::MalformedTask(format!(
            "{field} must be 1..=128 bytes"
        )));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ExecError::MalformedTask(format!(
            "{field} must be ascii alphanumeric, '-', '_' or '.'"
        )));
    }
    Ok(())
}

/// Reject anything that could escape the workspace. This runs on both the
/// local and the remote side of every transport.
pub(crate) fn validate_relative_name(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 {
        return Err(ExecError::MalformedTask(format!(
            "{field} must be 1..=256 bytes"
        )));
    }
    if value.starts_with('/') || value.starts_with('\\') || value.contains("..") {
        return Err(ExecError::MalformedTask(format!(
            "{field} must be a relative path that cannot escape the workspace"
        )));
    }
    if value.contains(':') {
        return Err(ExecError::MalformedTask(format!(
            "{field} must not carry a drive or scheme separator"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_in_any_budget_field_is_invalid_not_unlimited() {
        assert!(ResourceBudget::new(0, 1, 1, 1).is_err());
        assert!(ResourceBudget::new(1, 0, 1, 1).is_err());
        assert!(ResourceBudget::new(1, 1, 0, 1).is_err());
        assert!(ResourceBudget::new(1, 1, 1, 0).is_err());
        assert!(ResourceBudget::new(1, 1, 1, 1).is_ok());
    }

    #[test]
    fn workspace_digest_is_order_independent() {
        let a = WorkspaceFile {
            path: "a.txt".into(),
            bytes: b"alpha".to_vec(),
        };
        let b = WorkspaceFile {
            path: "b.txt".into(),
            bytes: b"beta".to_vec(),
        };
        let one = ExecutionTask {
            task_id: "t1".into(),
            nonce: "n1".into(),
            workspace: vec![a.clone(), b.clone()],
            input: b"x".to_vec(),
            argv: vec!["cat".into()],
            artifact_name: "out.bin".into(),
            resources: ResourceBudget::new(1, 1, 1, 1).unwrap(),
        };
        let two = ExecutionTask {
            workspace: vec![b, a],
            ..one.clone()
        };
        assert_eq!(one.workspace_sha256(), two.workspace_sha256());
    }

    #[test]
    fn workspace_paths_cannot_escape() {
        assert!(validate_relative_name("p", "../etc/passwd").is_err());
        assert!(validate_relative_name("p", "/etc/passwd").is_err());
        assert!(validate_relative_name("p", "C:\\windows").is_err());
        assert!(validate_relative_name("p", "sub/dir/file.txt").is_ok());
    }

    #[test]
    fn identifiers_reject_shell_metacharacters() {
        assert!(validate_identifier("task_id", "ok-task_1.2").is_ok());
        assert!(validate_identifier("task_id", "bad;rm -rf /").is_err());
        assert!(validate_identifier("task_id", "$(whoami)").is_err());
        assert!(validate_identifier("task_id", "").is_err());
    }
}
