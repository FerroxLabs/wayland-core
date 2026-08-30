//! FerroxLabs/wayland#174 item 2 — the per-task SPEND AUDIT RECORD.
//!
//! The rest of this crate already knows what a run spent; until now nothing
//! wrote it down. A live meter answers "how much so far?" and disappears with
//! the process. An audit record answers "what did that task cost, on what, and
//! did anything try to escape its envelope?" after the fact, from disk.
//!
//! One record is produced per TASK — one `AgentEngine::run`, i.e. one user
//! instruction and every provider call the agent made answering it, including
//! its compaction calls and its retries. Sub-agents keep their own sessions and
//! therefore produce their own records.
//!
//! The record is append-only JSONL, one object per line, under a
//! cross-process advisory lock (the same discipline as
//! [`crate::daily::DailySpendStore`]) so concurrent lanes on one machine
//! cannot interleave half-lines.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::spend::{
    EscalationRecord, ModelSpendProfile, SPEND_SCHEMA_VERSION, SpendMode, SpendRefusal,
};

/// One provider dispatch, as the audit sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpendAuditDispatch {
    pub provider: String,
    pub model: String,
    /// What the dispatch was for: `conversation`, `compaction`, `fallback`.
    pub purpose: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Settled cost. `None` means the call happened at an unknown price —
    /// which is NOT zero, and is counted separately in the record so a total
    /// can never be read as complete when it is not.
    pub cost_usd: Option<f64>,
}

/// A refusal the guard issued during the task, preserved with its reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpendAuditRefusal {
    pub kind: String,
    pub detail: String,
}

impl From<&SpendRefusal> for SpendAuditRefusal {
    fn from(refusal: &SpendRefusal) -> Self {
        Self {
            kind: refusal.kind().to_owned(),
            detail: refusal.to_string(),
        }
    }
}

/// The per-task record. Serialized as one JSONL line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpendAuditRecord {
    pub schema_version: u32,
    /// Unique per task. Distinct from `session_id`: a session answers many
    /// instructions and therefore emits many records.
    pub task_id: String,
    pub session_id: String,
    pub mode: SpendMode,
    /// The model the task was authorized for when it started.
    pub baseline_model: String,
    pub started_unix_ms: u64,
    pub ended_unix_ms: u64,
    pub dispatches: Vec<SpendAuditDispatch>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Sum of the dispatches whose price was known.
    pub cost_usd: f64,
    /// How many dispatches ran at an unknown price. When this is non-zero,
    /// `cost_usd` is a LOWER BOUND and the reader must be told so — which is
    /// exactly why the count is a field and not a comment.
    pub unpriced_dispatches: u64,
    pub escalations: Vec<EscalationRecord>,
    pub refusals: Vec<SpendAuditRefusal>,
}

impl SpendAuditRecord {
    /// Whether `cost_usd` is the whole bill or only the part that could be
    /// priced.
    #[must_use]
    pub fn cost_is_complete(&self) -> bool {
        self.unpriced_dispatches == 0
    }

    /// One-line human summary, for a host that wants to print the audit
    /// rather than store it.
    #[must_use]
    pub fn summary(&self) -> String {
        let cost = if self.cost_is_complete() {
            format!("${:.4}", self.cost_usd)
        } else {
            format!(
                "${:.4}+ ({} dispatch(es) at an unknown price)",
                self.cost_usd, self.unpriced_dispatches
            )
        };
        format!(
            "spend audit {task}: {dispatches} dispatch(es), {tin} in / {tout} out tokens, \
             {cost}, {esc} escalation(s), {ref_} refusal(s), mode {mode}",
            task = self.task_id,
            dispatches = self.dispatches.len(),
            tin = self.tokens_in,
            tout = self.tokens_out,
            cost = cost,
            esc = self.escalations.len(),
            ref_ = self.refusals.len(),
            mode = self.mode.as_str(),
        )
    }
}

/// Failures while persisting an audit record.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SpendAuditError {
    #[error("spend audit log at {path} is unusable: {reason}")]
    Unusable { path: String, reason: String },
    #[error("spend audit record could not be serialized: {0}")]
    Serialize(String),
}

impl SpendAuditError {
    fn unusable(path: &Path, error: impl std::fmt::Display) -> Self {
        Self::Unusable {
            path: path.display().to_string(),
            reason: error.to_string(),
        }
    }
}

/// Where finished audit records go.
///
/// A trait rather than a concrete writer so a host can route records to its
/// own store, and so tests can assert on them without touching a filesystem.
pub trait SpendAuditSink: Send + Sync {
    /// Persist a finished per-task record.
    fn record(&self, record: &SpendAuditRecord) -> Result<(), SpendAuditError>;

    /// Persist an escalation the moment it is authorized, without waiting for
    /// the task to end.
    ///
    /// Both surfaces exist because they answer to different failure modes: the
    /// per-task record is lost if the process dies mid-task, and an escalation
    /// is precisely the event most likely to precede a runaway that never
    /// reaches a clean end. Escalations are therefore written twice — once
    /// here, immediately, and again inside the task record.
    fn escalation(&self, record: &EscalationRecord) -> Result<(), SpendAuditError>;
}

/// Append-only JSONL sink under a cross-process advisory lock.
#[derive(Debug, Clone)]
pub struct JsonlSpendAuditSink {
    path: PathBuf,
}

impl JsonlSpendAuditSink {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Default location under a wayland-core home directory.
    #[must_use]
    pub fn default_path(home: &Path) -> PathBuf {
        home.join("spend-audit.jsonl")
    }

    fn append(&self, kind: &str, value: &serde_json::Value) -> Result<(), SpendAuditError> {
        let mut line = serde_json::to_vec(&serde_json::json!({
            "kind": kind,
            "payload": value,
        }))
        .map_err(|error| SpendAuditError::Serialize(error.to_string()))?;
        line.push(b'\n');

        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| SpendAuditError::unusable(&self.path, error))?;
        }
        let lock_path = self.path.with_extension("jsonl.lock");
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| SpendAuditError::unusable(&lock_path, error))?;
        let mut lock = fd_lock::RwLock::new(lock_file);
        let _guard = loop {
            match lock.write() {
                Ok(guard) => break guard,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(SpendAuditError::unusable(&lock_path, error)),
            }
        };

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| SpendAuditError::unusable(&self.path, error))?;
        file.write_all(&line)
            .map_err(|error| SpendAuditError::unusable(&self.path, error))?;
        file.sync_all()
            .map_err(|error| SpendAuditError::unusable(&self.path, error))?;
        Ok(())
    }
}

impl SpendAuditSink for JsonlSpendAuditSink {
    fn record(&self, record: &SpendAuditRecord) -> Result<(), SpendAuditError> {
        let value = serde_json::to_value(record)
            .map_err(|error| SpendAuditError::Serialize(error.to_string()))?;
        self.append("task_spend_audit", &value)
    }

    fn escalation(&self, record: &EscalationRecord) -> Result<(), SpendAuditError> {
        let value = serde_json::to_value(record)
            .map_err(|error| SpendAuditError::Serialize(error.to_string()))?;
        self.append("model_escalation", &value)
    }
}

/// In-memory sink. Used by tests and by hosts that render rather than store.
#[derive(Debug, Default)]
pub struct MemorySpendAuditSink {
    records: Mutex<Vec<SpendAuditRecord>>,
    escalations: Mutex<Vec<EscalationRecord>>,
}

impl MemorySpendAuditSink {
    #[must_use]
    pub fn records(&self) -> Vec<SpendAuditRecord> {
        self.records.lock().clone()
    }

    #[must_use]
    pub fn escalations(&self) -> Vec<EscalationRecord> {
        self.escalations.lock().clone()
    }
}

impl SpendAuditSink for MemorySpendAuditSink {
    fn record(&self, record: &SpendAuditRecord) -> Result<(), SpendAuditError> {
        self.records.lock().push(record.clone());
        Ok(())
    }

    fn escalation(&self, record: &EscalationRecord) -> Result<(), SpendAuditError> {
        self.escalations.lock().push(record.clone());
        Ok(())
    }
}

/// Accumulates one task's spend, then emits its record.
///
/// Cheap to construct and lock-free from the caller's side (one `Mutex`
/// inside), so a dispatch site can charge it without threading a `&mut`
/// through the engine's turn loop.
#[derive(Debug)]
pub struct SpendAuditor {
    inner: Mutex<AuditorState>,
}

#[derive(Debug)]
struct AuditorState {
    task_id: String,
    session_id: String,
    mode: SpendMode,
    baseline_model: String,
    started_unix_ms: u64,
    dispatches: Vec<SpendAuditDispatch>,
    escalations: Vec<EscalationRecord>,
    refusals: Vec<SpendAuditRefusal>,
    /// Set once the record has been emitted, so a second `finish` cannot
    /// double-report the same task.
    finished: bool,
}

impl SpendAuditor {
    #[must_use]
    pub fn new(
        task_id: impl Into<String>,
        session_id: impl Into<String>,
        mode: SpendMode,
        baseline: &ModelSpendProfile,
        started_unix_ms: u64,
    ) -> Self {
        Self {
            inner: Mutex::new(AuditorState {
                task_id: task_id.into(),
                session_id: session_id.into(),
                mode,
                baseline_model: baseline.label(),
                started_unix_ms,
                dispatches: Vec::new(),
                escalations: Vec::new(),
                refusals: Vec::new(),
                finished: false,
            }),
        }
    }

    /// Charge one settled dispatch.
    pub fn charge(&self, dispatch: SpendAuditDispatch) {
        self.inner.lock().dispatches.push(dispatch);
    }

    /// #1203 — re-key the open task onto the session it actually belongs to.
    ///
    /// The engine installs its spend guard before a session exists, so the
    /// auditor is constructed with a placeholder. The record is not written
    /// until [`Self::finish`], so the id can still be corrected — and it must
    /// be, or one conversation's records land under an identity nothing else
    /// in the system uses and its authorized spend can never be totalled.
    ///
    /// A no-op once the record has been emitted: what was written is what the
    /// log says, and rewriting the accumulator afterwards would only make the
    /// in-memory state disagree with the file.
    pub fn rebind_session(&self, session_id: &str) {
        let mut state = self.inner.lock();
        if state.finished || state.session_id == session_id {
            return;
        }
        state.session_id = session_id.to_owned();
    }

    /// The session this task's record will be keyed by.
    #[must_use]
    pub fn session_id(&self) -> String {
        self.inner.lock().session_id.clone()
    }

    /// Note an authorized escalation.
    pub fn escalated(&self, record: EscalationRecord) {
        self.inner.lock().escalations.push(record);
    }

    /// Note a refused dispatch.
    pub fn refused(&self, refusal: &SpendRefusal) {
        self.inner.lock().refusals.push(refusal.into());
    }

    /// How many dispatches have been charged so far.
    #[must_use]
    pub fn dispatch_count(&self) -> usize {
        self.inner.lock().dispatches.len()
    }

    /// Close the task and produce its record.
    ///
    /// Returns `None` on a second call. The engine has several terminal paths
    /// (answered, budget-stopped, cancelled, errored) and every one of them
    /// must emit; making the SECOND emission the no-op is what lets every path
    /// call this unconditionally without any of them having to know whether
    /// another already did.
    #[must_use]
    pub fn finish(&self, ended_unix_ms: u64) -> Option<SpendAuditRecord> {
        let mut state = self.inner.lock();
        if state.finished {
            return None;
        }
        state.finished = true;
        let mut tokens_in = 0u64;
        let mut tokens_out = 0u64;
        let mut cost_usd = 0.0f64;
        let mut unpriced_dispatches = 0u64;
        for dispatch in &state.dispatches {
            tokens_in = tokens_in.saturating_add(dispatch.tokens_in);
            tokens_out = tokens_out.saturating_add(dispatch.tokens_out);
            match dispatch.cost_usd {
                Some(usd) if usd.is_finite() => cost_usd += usd,
                _ => unpriced_dispatches += 1,
            }
        }
        Some(SpendAuditRecord {
            schema_version: SPEND_SCHEMA_VERSION,
            task_id: state.task_id.clone(),
            session_id: state.session_id.clone(),
            mode: state.mode,
            baseline_model: state.baseline_model.clone(),
            started_unix_ms: state.started_unix_ms,
            ended_unix_ms,
            dispatches: state.dispatches.clone(),
            tokens_in,
            tokens_out,
            cost_usd,
            unpriced_dispatches,
            escalations: state.escalations.clone(),
            refusals: state.refusals.clone(),
        })
    }

    /// Close the task and write its record to `sink`, if it has not already
    /// been closed.
    pub fn finish_into(
        &self,
        sink: &Arc<dyn SpendAuditSink>,
        ended_unix_ms: u64,
    ) -> Result<Option<SpendAuditRecord>, SpendAuditError> {
        match self.finish(ended_unix_ms) {
            Some(record) => {
                sink.record(&record)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }
}

/// Wall-clock milliseconds since the epoch, saturating at 0 before it.
#[must_use]
pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::{EscalationGate, ModelBilling};

    fn metered(model: &str, rate: f64) -> ModelSpendProfile {
        ModelSpendProfile::new("anthropic", model, ModelBilling::Metered, rate)
    }

    fn dispatch(purpose: &str, tin: u64, tout: u64, cost: Option<f64>) -> SpendAuditDispatch {
        SpendAuditDispatch {
            provider: "anthropic".into(),
            model: "haiku".into(),
            purpose: purpose.into(),
            tokens_in: tin,
            tokens_out: tout,
            cost_usd: cost,
        }
    }

    #[test]
    fn a_finished_task_totals_its_dispatches() {
        let auditor = SpendAuditor::new(
            "t1",
            "s1",
            SpendMode::Unrestricted,
            &metered("haiku", 1.0),
            10,
        );
        auditor.charge(dispatch("conversation", 100, 20, Some(0.25)));
        auditor.charge(dispatch("compaction", 900, 80, Some(0.75)));
        let record = auditor.finish(50).expect("first finish emits");
        assert_eq!(record.dispatches.len(), 2);
        assert_eq!(record.tokens_in, 1_000);
        assert_eq!(record.tokens_out, 100);
        assert!((record.cost_usd - 1.0).abs() < 1e-9);
        assert!(record.cost_is_complete());
        assert_eq!(record.started_unix_ms, 10);
        assert_eq!(record.ended_unix_ms, 50);
    }

    #[test]
    fn an_unpriced_dispatch_is_counted_not_treated_as_zero() {
        let auditor = SpendAuditor::new(
            "t1",
            "s1",
            SpendMode::Unrestricted,
            &metered("haiku", 1.0),
            0,
        );
        auditor.charge(dispatch("conversation", 10, 10, Some(0.5)));
        auditor.charge(dispatch("conversation", 10, 10, None));
        let record = auditor.finish(1).expect("emits");
        assert_eq!(record.unpriced_dispatches, 1);
        assert!(!record.cost_is_complete());
        // The summary must SAY the total is a floor, or a reader will take
        // $0.50 for the whole bill.
        assert!(
            record.summary().contains("unknown price"),
            "{}",
            record.summary()
        );
    }

    #[test]
    fn finish_is_idempotent_so_every_terminal_path_may_call_it() {
        let auditor = SpendAuditor::new("t1", "s1", SpendMode::NoPaid, &metered("haiku", 1.0), 0);
        assert!(auditor.finish(1).is_some());
        assert!(auditor.finish(2).is_none());
    }

    #[test]
    fn escalations_and_refusals_reach_the_record() {
        let auditor = SpendAuditor::new(
            "t1",
            "s1",
            SpendMode::Unrestricted,
            &metered("haiku", 1.0),
            0,
        );
        let mut gate = EscalationGate::new("s1", metered("haiku", 1.0));
        let escalation = gate
            .authorize(metered("opus", 30.0), "operator", "approved", 5)
            .unwrap()
            .unwrap();
        auditor.escalated(escalation);
        let refusal = SpendRefusal::PaidModel {
            mode: "no-paid".into(),
            target: "anthropic/opus".into(),
            billing: "metered".into(),
        };
        auditor.refused(&refusal);
        let record = auditor.finish(9).expect("emits");
        assert_eq!(record.escalations.len(), 1);
        assert_eq!(record.escalations[0].reason, "approved");
        assert_eq!(record.refusals.len(), 1);
        assert_eq!(record.refusals[0].kind, "paid_model_refused");
    }

    #[test]
    fn the_jsonl_sink_appends_one_line_per_event_and_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sink = JsonlSpendAuditSink::new(dir.path().join("spend-audit.jsonl"));
        let auditor =
            SpendAuditor::new("t1", "s1", SpendMode::LocalOnly, &metered("haiku", 1.0), 0);
        auditor.charge(dispatch("conversation", 5, 5, Some(0.1)));
        let record = auditor.finish(1).expect("emits");
        sink.record(&record).expect("write record");

        let mut gate = EscalationGate::new("s1", metered("haiku", 1.0));
        let escalation = gate
            .authorize(metered("opus", 30.0), "tier_swap", "needed reasoning", 2)
            .unwrap()
            .unwrap();
        sink.escalation(&escalation).expect("write escalation");

        let raw = std::fs::read_to_string(sink.path()).expect("read back");
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 2, "one JSONL line per event: {raw}");
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("line 1 is json");
        assert_eq!(first["kind"], "task_spend_audit");
        let parsed: SpendAuditRecord =
            serde_json::from_value(first["payload"].clone()).expect("record round-trips");
        assert_eq!(parsed, record);
        let second: serde_json::Value = serde_json::from_str(lines[1]).expect("line 2 is json");
        assert_eq!(second["kind"], "model_escalation");
        assert_eq!(second["payload"]["reason"], "needed reasoning");
    }

    #[test]
    fn finish_into_writes_once_and_only_once() {
        let memory = Arc::new(MemorySpendAuditSink::default());
        let sink: Arc<dyn SpendAuditSink> = memory.clone();
        let auditor = SpendAuditor::new(
            "t1",
            "s1",
            SpendMode::Unrestricted,
            &metered("haiku", 1.0),
            0,
        );
        assert!(auditor.finish_into(&sink, 1).expect("ok").is_some());
        assert!(auditor.finish_into(&sink, 2).expect("ok").is_none());
        assert_eq!(memory.records().len(), 1);
    }
}
