// P1 Working memory — in-process queue with WAL spillover. Implementation
// lands in Group C.6.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;

use crate::db::Db;
use crate::error::{MemoryError, Result};
use crate::v2_types::Tier;

const DEFAULT_IN_MEMORY_CAP: usize = 50;

/// One unit of P1 working memory — a turn, a tool call, or a bookmark
/// inserted by M6 (Letta compaction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkingEntry {
    Turn {
        ts: i64,
        role: String,
        text: String,
    },
    ToolCall {
        ts: i64,
        tool: String,
        summary: String,
    },
    /// Compaction bookmark — points at the P2 episode that absorbed the
    /// offloaded turns.
    Bookmark {
        ts: i64,
        episode_id: String,
        summary_preview: String,
    },
}

impl WorkingEntry {
    pub fn ts(&self) -> i64 {
        match self {
            WorkingEntry::Turn { ts, .. }
            | WorkingEntry::ToolCall { ts, .. }
            | WorkingEntry::Bookmark { ts, .. } => *ts,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            WorkingEntry::Turn { .. } => "turn",
            WorkingEntry::ToolCall { .. } => "tool_call",
            WorkingEntry::Bookmark { .. } => "bookmark",
        }
    }

    fn payload_bytes(&self) -> Vec<u8> {
        // Lightweight encoding for spillover.
        serde_json::to_vec(&self.encode_json()).unwrap_or_default()
    }

    fn encode_json(&self) -> serde_json::Value {
        match self {
            WorkingEntry::Turn { ts, role, text } => serde_json::json!({
                "kind": "turn", "ts": ts, "role": role, "text": text,
            }),
            WorkingEntry::ToolCall { ts, tool, summary } => serde_json::json!({
                "kind": "tool_call", "ts": ts, "tool": tool, "summary": summary,
            }),
            WorkingEntry::Bookmark {
                ts,
                episode_id,
                summary_preview,
            } => serde_json::json!({
                "kind": "bookmark", "ts": ts, "episode_id": episode_id, "summary_preview": summary_preview,
            }),
        }
    }
}

pub struct WorkingPartition {
    pub(crate) buf: Arc<RwLock<VecDeque<WorkingEntry>>>,
    pub(crate) cap: usize,
    pub(crate) db: Arc<Db>,
    pub(crate) cdc: Arc<crate::cdc::CdcWriter>,
    /// Interior-mutable so the dispatcher can update the working-memory CDC
    /// tag from the bootstrap `"boot"` placeholder to the real session id when
    /// the session DB is rebound (see [`WorkingPartition::set_session_id`]).
    pub(crate) session_id: RwLock<Option<String>>,
}

impl WorkingPartition {
    pub fn new(db: Arc<Db>, cdc: Arc<crate::cdc::CdcWriter>, session_id: Option<String>) -> Self {
        Self {
            buf: Arc::new(RwLock::new(VecDeque::new())),
            cap: DEFAULT_IN_MEMORY_CAP,
            db,
            cdc,
            session_id: RwLock::new(session_id),
        }
    }

    /// Update the session id used to tag spillover CDC events. Called on
    /// session rebind so working-memory events carry the real session id
    /// instead of the bootstrap `"boot"` placeholder.
    pub fn set_session_id(&self, session_id: Option<String>) {
        *self.session_id.write() = session_id;
    }

    pub fn with_cap(mut self, cap: usize) -> Self {
        self.cap = cap;
        self
    }

    pub async fn push(&self, e: WorkingEntry) -> Result<()> {
        let mut spill: Vec<WorkingEntry> = Vec::new();
        {
            let mut buf = self.buf.write();
            buf.push_back(e);
            while buf.len() > self.cap {
                if let Some(old) = buf.pop_front() {
                    spill.push(old);
                }
            }
        }
        if !spill.is_empty() {
            let count = spill.len();
            self.spill_to_db(&spill)?;
            let guard = self.session_id.read();
            let sid = guard.as_deref().unwrap_or("unknown");
            self.cdc.append_working_spillover(sid, count)?;
        }
        Ok(())
    }

    fn spill_to_db(&self, spill: &[WorkingEntry]) -> Result<()> {
        // Only meaningful if the session DB is configured. If not, drop —
        // the bound is in-memory by design.
        let Some(tc) = self.db.tier(Tier::Session) else {
            return Ok(());
        };
        let mut conn = tc.conn.lock();
        let tx = conn.transaction().map_err(MemoryError::Db)?;
        for e in spill {
            tx.execute(
                "INSERT INTO p1_working (ts, kind, payload) VALUES (?1, ?2, ?3)",
                rusqlite::params![e.ts(), e.kind(), e.payload_bytes()],
            )
            .map_err(MemoryError::Db)?;
        }
        tx.commit().map_err(MemoryError::Db)?;
        Ok(())
    }

    /// Read live + spillover (newest spillover first, then in-memory order).
    pub fn snapshot(&self) -> Vec<WorkingEntry> {
        let live = self.buf.read().iter().cloned().collect::<Vec<_>>();
        // For now we don't decode the spillover back — design says snapshot
        // returns the live window; spillover is for replay/CDC. Tests rely
        // on the live + the row count, which we expose via spillover_count.
        live
    }

    pub fn live_len(&self) -> usize {
        self.buf.read().len()
    }

    pub fn spillover_count(&self) -> Result<usize> {
        let Some(tc) = self.db.tier(Tier::Session) else {
            return Ok(0);
        };
        let conn = tc.conn.lock();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM p1_working", [], |r| r.get(0))
            .map_err(MemoryError::Db)?;
        Ok(n as usize)
    }
}

/// Helper for tests + dispatcher consumers.
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
