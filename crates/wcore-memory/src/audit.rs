// Append-only audit log writer for MemoryAccessGate (M2).
//
// Lives in its own SQLite DB (audit.db) so the journal-of-decisions can be
// rotated independently of session/project/global memory. The schema is
// minimal — gate denials are the primary write rate.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::Connection;

use crate::error::{MemoryError, Result};
use crate::v2_types::{Partition, Tier};

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS audit_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    ts            INTEGER NOT NULL,
    token_kind    TEXT NOT NULL,
    agent_name    TEXT,
    partition     TEXT NOT NULL,
    tier          TEXT NOT NULL,
    op            TEXT NOT NULL,
    decision      TEXT NOT NULL,
    reason        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_log (ts);
CREATE INDEX IF NOT EXISTS idx_audit_decision ON audit_log (decision);
";

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub ts: i64,
    pub token_kind: String,
    pub agent_name: Option<String>,
    pub partition: Partition,
    pub tier: Tier,
    pub op: String,
    pub decision: String, // "allow" | "deny"
    pub reason: String,
}

pub struct AuditLog {
    pub(crate) conn: Arc<Mutex<Connection>>,
    pub(crate) path: PathBuf,
}

impl AuditLog {
    /// Path the audit log lives at (`:memory:` for the in-memory variant).
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl AuditLog {
    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path).map_err(MemoryError::Db)?;
        conn.execute_batch(SCHEMA_SQL).map_err(MemoryError::Db)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        })
    }

    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(MemoryError::Db)?;
        conn.execute_batch(SCHEMA_SQL).map_err(MemoryError::Db)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: PathBuf::from(":memory:"),
        })
    }

    pub fn record(&self, entry: AuditEntry) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO audit_log (ts, token_kind, agent_name, partition, tier, op, decision, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                entry.ts,
                entry.token_kind,
                entry.agent_name,
                entry.partition.as_str(),
                entry.tier.as_str(),
                entry.op,
                entry.decision,
                entry.reason,
            ],
        )
        .map_err(|e| MemoryError::Audit(e.to_string()))?;
        Ok(())
    }

    pub fn count(&self) -> Result<usize> {
        let conn = self.conn.lock();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
            .map_err(MemoryError::Db)?;
        Ok(n as usize)
    }

    pub fn count_denials(&self) -> Result<usize> {
        let conn = self.conn.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audit_log WHERE decision = 'deny'",
                [],
                |r| r.get(0),
            )
            .map_err(MemoryError::Db)?;
        Ok(n as usize)
    }

    /// W6 F17: count `op` occurrences within the last `window_secs` seconds.
    /// Used by `McpCurator` (in `wcore-agent`) to break keyword-overlap ties
    /// on recency. The previous `AuditLog` surface was `record`/`count`/
    /// `count_denials` only — no per-op read API (audit rev-2 finding 2).
    ///
    /// Returns a map from `op` -> count for every op that appeared inside
    /// the window. Ops outside the window are omitted. Errors propagate as
    /// `MemoryError::Db`; callers are expected to graceful-degrade to
    /// keyword-only ranking on read failure.
    pub fn recent_tool_uses(
        &self,
        window_secs: i64,
    ) -> Result<std::collections::HashMap<String, u64>> {
        let cutoff = now_secs().saturating_sub(window_secs);
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT op, COUNT(*) FROM audit_log WHERE ts >= ?1 GROUP BY op")
            .map_err(MemoryError::Db)?;
        let rows = stmt
            .query_map(rusqlite::params![cutoff], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })
            .map_err(MemoryError::Db)?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (op, n) = row.map_err(MemoryError::Db)?;
            out.insert(op, n);
        }
        Ok(out)
    }
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
