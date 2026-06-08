// M9 — CDC changelog writer.
//
// Append-only journal of every mutation across the 5 partitions. Per-tier
// JSONL files + an in-memory mirror for tests. Monotonic `seq` per tier.
//
// Covers all 10 mutation paths (audit F3):
//   F.1a inserts:        episode, fact, procedure, legacy_import
//   F.1b state changes:  episode_update, fact_supersede, procedure_status,
//                        procedure_use, user_model_delta, working_spillover,
//                        decay_archive
//
// The audit log (gate denials) is intentionally NOT routed through CDC —
// audit.db is itself a journal and would otherwise become a journal-of-
// journals.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{MemoryError, Result};
use crate::v2_types::{Episode, Fact, Procedure, Tier};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdcEntry {
    pub seq: u64,
    pub ts: i64,
    pub tier: String,
    pub partition: String,
    pub op: String,
    pub target_id: Option<String>,
    pub source_product: Option<String>,
    pub payload: Value,
}

/// Replay-payload mirrors of the partition records (no embeddings,
/// minimal field set sufficient to reconstruct the row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodePayload {
    pub id: String,
    pub tier: String,
    pub ts: i64,
    pub episode_type: String,
    pub summary: String,
    pub atomic_facts: Vec<String>,
    pub source: String,
    pub source_product: String,
    pub session_id: Option<String>,
    pub project_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactPayload {
    pub id: String,
    pub tier: String,
    pub ts: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedurePayload {
    pub id: String,
    pub tier: String,
    pub ts: i64,
    pub name: String,
    pub status: String,
    pub created_by: String,
}

/// CdcWriter — clone-cheap (Arc inside).
#[derive(Clone, Default)]
pub struct CdcWriter {
    pub(crate) inner: Arc<Mutex<CdcState>>,
}

#[derive(Default)]
pub(crate) struct CdcState {
    pub(crate) entries: Vec<CdcEntry>,
    pub(crate) seqs: std::collections::HashMap<String, u64>,
    pub(crate) sinks: std::collections::HashMap<String, File>,
}

impl CdcWriter {
    /// Stub mode (used by tests that don't care about CDC).
    pub fn new_stub() -> Self {
        Self::default()
    }

    /// Build a writer that journals into the given per-tier JSONL paths.
    /// The files are opened in append mode; parent dirs are created.
    pub fn new_with_sinks(
        session: Option<PathBuf>,
        project: Option<PathBuf>,
        global: Option<PathBuf>,
    ) -> Result<Self> {
        let mut sinks = std::collections::HashMap::new();
        for (tier_name, path) in [
            ("session", session),
            ("project", project),
            ("global", global),
        ] {
            if let Some(p) = path {
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                    // Changelogs replay every mutation (episode summaries,
                    // extracted facts, user-model deltas). Lock the dir to
                    // owner-only, matching the DB-tier posture.
                    crate::db::harden_dir_perms(parent);
                }
                let f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&p)
                    .map_err(|e| MemoryError::Cdc(format!("open {p:?}: {e}")))?;
                // Restrict the JSONL changelog to owner-only at rest.
                crate::db::harden_file_perms(&p);
                sinks.insert(tier_name.to_string(), f);
            }
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(CdcState {
                entries: Vec::new(),
                seqs: Default::default(),
                sinks,
            })),
        })
    }

    pub fn entries(&self) -> Vec<CdcEntry> {
        self.inner.lock().entries.clone()
    }

    fn append(
        &self,
        tier: Tier,
        partition: &str,
        op: &str,
        target_id: Option<String>,
        source_product: Option<String>,
        payload: Value,
    ) -> Result<()> {
        let mut state = self.inner.lock();
        let tier_key = tier.as_str().to_string();
        let seq = {
            let entry = state.seqs.entry(tier_key.clone()).or_insert(0);
            *entry += 1;
            *entry
        };
        let cdc = CdcEntry {
            seq,
            ts: now_secs(),
            tier: tier_key.clone(),
            partition: partition.to_string(),
            op: op.to_string(),
            target_id,
            source_product,
            payload,
        };
        // JSONL write (best-effort; if disk fails, also record in-mem).
        if let Some(f) = state.sinks.get_mut(&tier_key) {
            let line = serde_json::to_string(&cdc)
                .map_err(|e| MemoryError::Cdc(format!("serialize: {e}")))?;
            writeln!(f, "{line}")
                .map_err(|e| MemoryError::Cdc(format!("write {tier_key}: {e}")))?;
            f.flush()
                .map_err(|e| MemoryError::Cdc(format!("flush {tier_key}: {e}")))?;
        }
        state.entries.push(cdc);
        Ok(())
    }

    // ---- F.1a insert paths (4) ----
    pub fn append_episode(&self, tier: Tier, ep: &Episode) -> Result<()> {
        let payload = serde_json::to_value(EpisodePayload {
            id: ep.id.0.to_string(),
            tier: ep.tier.as_str().into(),
            ts: ep.ts,
            episode_type: ep.episode_type.clone(),
            summary: ep.summary.clone(),
            atomic_facts: ep.atomic_facts.clone(),
            source: ep.source.clone(),
            source_product: ep.source_product.clone(),
            session_id: ep.session_id.clone(),
            project_root: ep.project_root.clone(),
        })
        .map_err(|e| MemoryError::Cdc(format!("serialize episode: {e}")))?;
        self.append(
            tier,
            "episodic",
            "insert",
            Some(ep.id.0.to_string()),
            Some(ep.source_product.clone()),
            payload,
        )
    }

    pub fn append_fact(&self, tier: Tier, f: &Fact) -> Result<()> {
        let payload = serde_json::to_value(FactPayload {
            id: f.id.0.to_string(),
            tier: f.tier.as_str().into(),
            ts: f.ts,
            subject: f.subject.clone(),
            predicate: f.predicate.clone(),
            object: f.object.clone(),
            confidence: f.confidence,
        })
        .map_err(|e| MemoryError::Cdc(format!("serialize fact: {e}")))?;
        self.append(
            tier,
            "semantic",
            "insert",
            Some(f.id.0.to_string()),
            None,
            payload,
        )
    }

    pub fn append_procedure(&self, tier: Tier, p: &Procedure) -> Result<()> {
        let payload = serde_json::to_value(ProcedurePayload {
            id: p.id.0.to_string(),
            tier: p.tier.as_str().into(),
            ts: p.ts,
            name: p.name.clone(),
            status: p.status.as_str().into(),
            created_by: p.created_by.clone(),
        })
        .map_err(|e| MemoryError::Cdc(format!("serialize procedure: {e}")))?;
        self.append(
            tier,
            "procedural",
            "insert",
            Some(p.id.0.to_string()),
            None,
            payload,
        )
    }

    pub fn append_legacy_import(&self, yaml_dir: &str, episode_count: usize) -> Result<()> {
        self.append(
            Tier::Global,
            "episodic",
            "legacy_import",
            None,
            Some("wcore-memory-v1".into()),
            serde_json::json!({
                "yaml_dir": yaml_dir,
                "episode_count": episode_count,
            }),
        )
    }

    // ---- F.1b update + state-transition paths (6) ----
    pub fn append_episode_update(&self, tier: Tier, ep_id: &Uuid, delta: &Value) -> Result<()> {
        self.append(
            tier,
            "episodic",
            "update",
            Some(ep_id.to_string()),
            None,
            delta.clone(),
        )
    }

    pub fn append_fact_supersede(&self, tier: Tier, old_id: &Uuid, new_id: &Uuid) -> Result<()> {
        self.append(
            tier,
            "semantic",
            "supersede",
            Some(old_id.to_string()),
            None,
            serde_json::json!({"old": old_id.to_string(), "new": new_id.to_string()}),
        )
    }

    pub fn append_procedure_status(&self, tier: Tier, p_id: &Uuid, new_status: &str) -> Result<()> {
        self.append(
            tier,
            "procedural",
            "status_transition",
            Some(p_id.to_string()),
            None,
            serde_json::json!({"new_status": new_status}),
        )
    }

    pub fn append_procedure_use(
        &self,
        tier: Tier,
        p_id: &Uuid,
        succeeded: bool,
        alpha: f64,
        beta: f64,
    ) -> Result<()> {
        self.append(
            tier,
            "procedural",
            "use",
            Some(p_id.to_string()),
            None,
            serde_json::json!({"succeeded": succeeded, "alpha": alpha, "beta": beta}),
        )
    }

    pub fn append_user_model_delta(&self, key: &str, old: &Value, new: &Value) -> Result<()> {
        self.append(
            Tier::Global,
            "core",
            "delta",
            Some(key.to_string()),
            None,
            serde_json::json!({"key": key, "old": old, "new": new}),
        )
    }

    pub fn append_working_spillover(&self, session_id: &str, row_count: usize) -> Result<()> {
        self.append(
            Tier::Session,
            "working",
            "spillover",
            Some(session_id.to_string()),
            None,
            serde_json::json!({"row_count": row_count}),
        )
    }

    pub fn append_decay_archive(&self, tier: Tier, ep_id: &Uuid, new_score: f64) -> Result<()> {
        self.append(
            tier,
            "episodic",
            "decay_archive",
            Some(ep_id.to_string()),
            None,
            serde_json::json!({"new_score": new_score}),
        )
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
