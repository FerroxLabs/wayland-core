//! T3-2 — Auto-Mode audit trail (sqlite-backed).
//!
//! Ported from Forge's `AuditTrail.ts` (Apache-2.0). Records every Auto-Mode
//! guardrail evaluation decision so operators can audit, replay, and debug
//! tool gating after the fact.
//!
//! Design choices vs. Forge:
//! - Eager-fail on construction: `AuditTrail::open` returns
//!   [`SwarmError::Audit`] if the sqlite handle can't be opened or the
//!   schema can't be applied. Forge degrades gracefully (no-op record /
//!   empty query) because better-sqlite3 is an optional native dep; in
//!   Rust we depend on `rusqlite` with the `bundled` feature, so a failure
//!   here is a real bug, not a missing-binary edge case.
//! - `record` returns `Result` so callers can route persistence errors
//!   into the engine's telemetry instead of silently dropping them.
//! - Schema migration is idempotent (CREATE TABLE / INDEX IF NOT EXISTS)
//!   so repeated `open` calls on the same DB file are safe.
//!
//! Schema mirrors Forge exactly so a future cross-tool audit aggregator
//! can read both stores with one query:
//!
//! ```sql
//! CREATE TABLE auto_mode_audit (
//!   id INTEGER PRIMARY KEY AUTOINCREMENT,
//!   timestamp INTEGER NOT NULL,        -- unix epoch millis
//!   tool_name TEXT NOT NULL,
//!   tool_args TEXT NOT NULL,           -- serialized JSON
//!   risk TEXT NOT NULL,                -- 'low' | 'medium' | 'high' | 'critical'
//!   outcome TEXT NOT NULL,             -- 'allow' | 'deny' | 'escalate' | 'override'
//!   guardrail TEXT NOT NULL,           -- which rule fired
//!   reason TEXT NOT NULL,              -- human-readable rationale
//!   session_id TEXT NOT NULL,
//!   duration_ms INTEGER                -- nullable
//! );
//! ```

use std::path::Path;
use std::str::FromStr;

use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};

use crate::error::{Result, SwarmError};

/// Risk classification for an audited tool invocation. Mirrors Forge's
/// `AuditDecision['risk']` union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditRisk {
    Low,
    Medium,
    High,
    Critical,
}

impl AuditRisk {
    /// Stable wire string (matches the TS source). Used for both the SQL
    /// column value and the JSON serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditRisk::Low => "low",
            AuditRisk::Medium => "medium",
            AuditRisk::High => "high",
            AuditRisk::Critical => "critical",
        }
    }
}

impl FromStr for AuditRisk {
    type Err = SwarmError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "low" => Ok(AuditRisk::Low),
            "medium" => Ok(AuditRisk::Medium),
            "high" => Ok(AuditRisk::High),
            "critical" => Ok(AuditRisk::Critical),
            other => Err(SwarmError::Audit(format!("unknown audit risk: {other}"))),
        }
    }
}

/// Terminal decision recorded for the tool. Mirrors Forge's
/// `AuditDecision['outcome']` taxonomy: allow, deny, escalate, override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    /// Guardrail permitted the tool to run.
    Allow,
    /// Guardrail blocked the tool.
    Deny,
    /// Guardrail escalated to the human for explicit approval.
    Escalate,
    /// Operator manually overrode a deny/escalate after the fact.
    Override,
}

impl AuditOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditOutcome::Allow => "allow",
            AuditOutcome::Deny => "deny",
            AuditOutcome::Escalate => "escalate",
            AuditOutcome::Override => "override",
        }
    }
}

impl FromStr for AuditOutcome {
    type Err = SwarmError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "allow" => Ok(AuditOutcome::Allow),
            "deny" => Ok(AuditOutcome::Deny),
            "escalate" => Ok(AuditOutcome::Escalate),
            "override" => Ok(AuditOutcome::Override),
            other => Err(SwarmError::Audit(format!("unknown audit outcome: {other}"))),
        }
    }
}

/// One row in the audit trail. `id` is `None` until the row has been
/// persisted (Forge's `Omit<AuditDecision, 'id'>` shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Auto-increment primary key. Populated by [`AuditTrail::query`] and
    /// friends; ignored on [`AuditTrail::record`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<i64>,
    /// Unix epoch milliseconds.
    pub timestamp: i64,
    pub tool_name: String,
    /// Serialized JSON of the tool's argument payload. Stored as TEXT so
    /// the schema is stable across argument shapes.
    pub tool_args: String,
    pub risk: AuditRisk,
    pub outcome: AuditOutcome,
    /// Which guardrail rule fired (e.g. `"path-allowlist"`).
    pub guardrail: String,
    /// Human-readable rationale for the decision.
    pub reason: String,
    pub session_id: String,
    /// Optional wall-clock duration of the guardrail evaluation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub duration_ms: Option<i64>,
}

/// Optional filter for [`AuditTrail::query`]. Mirrors Forge's
/// `{ sessionId?: string; outcome?: string }`.
#[derive(Debug, Clone, Default)]
pub struct AuditQuery {
    pub session_id: Option<String>,
    pub outcome: Option<AuditOutcome>,
}

const SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS auto_mode_audit (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        timestamp INTEGER NOT NULL,
        tool_name TEXT NOT NULL,
        tool_args TEXT NOT NULL,
        risk TEXT NOT NULL,
        outcome TEXT NOT NULL,
        guardrail TEXT NOT NULL,
        reason TEXT NOT NULL,
        session_id TEXT NOT NULL,
        duration_ms INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_audit_session_ts ON auto_mode_audit(session_id, timestamp DESC);
    CREATE INDEX IF NOT EXISTS idx_audit_outcome_ts ON auto_mode_audit(outcome, timestamp DESC);
    CREATE INDEX IF NOT EXISTS idx_audit_risk_ts    ON auto_mode_audit(risk, timestamp DESC);
";

/// SQLite-backed Auto-Mode audit log.
///
/// Single connection, single thread of access (wrap in a Mutex if you need
/// to share across tasks — the swarm's audit recorder is intentionally
/// owned by one supervisor).
pub struct AuditTrail {
    conn: Connection,
}

impl AuditTrail {
    /// Open (or create) an audit DB at `path` and apply the schema. The
    /// schema migration is idempotent — calling `open` twice on the same
    /// file is safe and a no-op the second time.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|e| SwarmError::Audit(format!("open audit db: {e}")))?;
        // Forge sets WAL for crash safety + concurrent reads. Mirror it.
        // pragma_update returns an error only for invalid pragmas; surface it.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| SwarmError::Audit(format!("set WAL journal_mode: {e}")))?;
        conn.execute_batch(SCHEMA_SQL)
            .map_err(|e| SwarmError::Audit(format!("apply audit schema: {e}")))?;
        Ok(Self { conn })
    }

    /// In-memory store, for tests. Schema is applied immediately.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| SwarmError::Audit(format!("open in-memory audit db: {e}")))?;
        conn.execute_batch(SCHEMA_SQL)
            .map_err(|e| SwarmError::Audit(format!("apply audit schema: {e}")))?;
        Ok(Self { conn })
    }

    /// Append one decision to the audit log. The event's `id` field is
    /// ignored on input; the persisted `rowid` is returned for callers who
    /// want to back-reference the row.
    pub fn record(&self, event: &AuditEvent) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO auto_mode_audit \
                 (timestamp, tool_name, tool_args, risk, outcome, guardrail, reason, session_id, duration_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    event.timestamp,
                    event.tool_name,
                    event.tool_args,
                    event.risk.as_str(),
                    event.outcome.as_str(),
                    event.guardrail,
                    event.reason,
                    event.session_id,
                    event.duration_ms,
                ],
            )
            .map_err(|e| SwarmError::Audit(format!("record audit event: {e}")))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Query the audit log with optional filters. Always ordered by
    /// `timestamp DESC` (most recent first), matching Forge.
    pub fn query(&self, filter: &AuditQuery) -> Result<Vec<AuditEvent>> {
        let mut conditions: Vec<&str> = Vec::new();
        let mut params_dyn: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(sid) = &filter.session_id {
            conditions.push("session_id = ?");
            params_dyn.push(rusqlite::types::Value::Text(sid.clone()));
        }
        if let Some(outcome) = &filter.outcome {
            conditions.push("outcome = ?");
            params_dyn.push(rusqlite::types::Value::Text(outcome.as_str().to_string()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {} ", conditions.join(" AND "))
        };
        let sql = format!(
            "SELECT id, timestamp, tool_name, tool_args, risk, outcome, guardrail, reason, session_id, duration_ms \
             FROM auto_mode_audit {where_clause}ORDER BY timestamp DESC, id DESC"
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| SwarmError::Audit(format!("prepare query: {e}")))?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params_dyn
            .iter()
            .map(|v| v as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), Self::row_to_event)
            .map_err(|e| SwarmError::Audit(format!("execute query: {e}")))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| SwarmError::Audit(format!("decode row: {e}")))?);
        }
        Ok(out)
    }

    /// Most recent `n` events across all sessions, `timestamp DESC`.
    pub fn recent(&self, n: usize) -> Result<Vec<AuditEvent>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, timestamp, tool_name, tool_args, risk, outcome, guardrail, reason, session_id, duration_ms \
                 FROM auto_mode_audit ORDER BY timestamp DESC, id DESC LIMIT ?1",
            )
            .map_err(|e| SwarmError::Audit(format!("prepare recent: {e}")))?;
        // sqlite LIMIT takes an integer; cast via i64 (clamping above i64::MAX
        // is fine — sqlite treats any negative as "no limit", but `usize`
        // can't underflow, so any value we pass is a valid >=0 bound).
        let lim: i64 = n.try_into().unwrap_or(i64::MAX);
        let rows = stmt
            .query_map([lim], Self::row_to_event)
            .map_err(|e| SwarmError::Audit(format!("execute recent: {e}")))?;
        let mut out = Vec::with_capacity(n);
        for row in rows {
            out.push(row.map_err(|e| SwarmError::Audit(format!("decode row: {e}")))?);
        }
        Ok(out)
    }

    /// Total number of recorded events.
    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM auto_mode_audit", [], |r| r.get(0))
            .map_err(|e| SwarmError::Audit(format!("count: {e}")))?;
        Ok(n.max(0) as u64)
    }

    fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEvent> {
        let risk_s: String = row.get(4)?;
        let outcome_s: String = row.get(5)?;
        // SwarmError -> rusqlite::Error::FromSqlConversionFailure so the
        // error surfaces through query_map's normal Result path.
        let risk = AuditRisk::from_str(&risk_s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                )),
            )
        })?;
        let outcome = AuditOutcome::from_str(&outcome_s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                )),
            )
        })?;

        Ok(AuditEvent {
            id: Some(row.get(0)?),
            timestamp: row.get(1)?,
            tool_name: row.get(2)?,
            tool_args: row.get(3)?,
            risk,
            outcome,
            guardrail: row.get(6)?,
            reason: row.get(7)?,
            session_id: row.get(8)?,
            duration_ms: row.get(9)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample(ts: i64, session: &str, outcome: AuditOutcome) -> AuditEvent {
        AuditEvent {
            id: None,
            timestamp: ts,
            tool_name: "Bash".into(),
            tool_args: r#"{"cmd":"ls"}"#.into(),
            risk: AuditRisk::Medium,
            outcome,
            guardrail: "path-allowlist".into(),
            reason: "args matched allowlist".into(),
            session_id: session.into(),
            duration_ms: Some(7),
        }
    }

    #[test]
    fn record_and_query_round_trip() {
        let trail = AuditTrail::in_memory().unwrap();
        let ev = sample(1000, "s1", AuditOutcome::Allow);
        let id = trail.record(&ev).unwrap();
        assert!(id >= 1, "rowid should be positive");

        let rows = trail.query(&AuditQuery::default()).unwrap();
        assert_eq!(rows.len(), 1);
        let got = &rows[0];
        assert_eq!(got.id, Some(id));
        assert_eq!(got.timestamp, 1000);
        assert_eq!(got.tool_name, "Bash");
        assert_eq!(got.risk, AuditRisk::Medium);
        assert_eq!(got.outcome, AuditOutcome::Allow);
        assert_eq!(got.duration_ms, Some(7));
        assert_eq!(got.session_id, "s1");
    }

    #[test]
    fn query_filter_by_outcome_and_session() {
        let trail = AuditTrail::in_memory().unwrap();
        trail
            .record(&sample(10, "s1", AuditOutcome::Allow))
            .unwrap();
        trail.record(&sample(20, "s1", AuditOutcome::Deny)).unwrap();
        trail.record(&sample(30, "s2", AuditOutcome::Deny)).unwrap();
        trail
            .record(&sample(40, "s2", AuditOutcome::Escalate))
            .unwrap();

        let denies = trail
            .query(&AuditQuery {
                session_id: None,
                outcome: Some(AuditOutcome::Deny),
            })
            .unwrap();
        assert_eq!(denies.len(), 2);
        assert!(denies.iter().all(|e| e.outcome == AuditOutcome::Deny));

        let s1 = trail
            .query(&AuditQuery {
                session_id: Some("s1".into()),
                outcome: None,
            })
            .unwrap();
        assert_eq!(s1.len(), 2);
        assert!(s1.iter().all(|e| e.session_id == "s1"));

        let s1_deny = trail
            .query(&AuditQuery {
                session_id: Some("s1".into()),
                outcome: Some(AuditOutcome::Deny),
            })
            .unwrap();
        assert_eq!(s1_deny.len(), 1);
        assert_eq!(s1_deny[0].timestamp, 20);
    }

    #[test]
    fn recent_orders_by_timestamp_desc() {
        let trail = AuditTrail::in_memory().unwrap();
        for ts in [100, 50, 300, 200, 150] {
            trail
                .record(&sample(ts, "s1", AuditOutcome::Allow))
                .unwrap();
        }
        let recent = trail.recent(3).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].timestamp, 300);
        assert_eq!(recent[1].timestamp, 200);
        assert_eq!(recent[2].timestamp, 150);

        // recent(n) with n > total returns everything.
        let all = trail.recent(99).unwrap();
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn count_tracks_inserts() {
        let trail = AuditTrail::in_memory().unwrap();
        assert_eq!(trail.count().unwrap(), 0);
        for i in 0..7 {
            trail.record(&sample(i, "s1", AuditOutcome::Allow)).unwrap();
        }
        assert_eq!(trail.count().unwrap(), 7);
    }

    #[test]
    fn schema_migration_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.sqlite");
        {
            let trail = AuditTrail::open(&path).unwrap();
            trail.record(&sample(1, "s1", AuditOutcome::Allow)).unwrap();
        }
        // Reopen — schema CREATE IF NOT EXISTS must not destroy data or error.
        let trail2 = AuditTrail::open(&path).unwrap();
        assert_eq!(trail2.count().unwrap(), 1);
        // A third open is also fine.
        let trail3 = AuditTrail::open(&path).unwrap();
        assert_eq!(trail3.count().unwrap(), 1);
    }

    #[test]
    fn record_all_risk_and_outcome_variants() {
        let trail = AuditTrail::in_memory().unwrap();
        for (i, (risk, outcome)) in [
            (AuditRisk::Low, AuditOutcome::Allow),
            (AuditRisk::Medium, AuditOutcome::Deny),
            (AuditRisk::High, AuditOutcome::Escalate),
            (AuditRisk::Critical, AuditOutcome::Override),
        ]
        .iter()
        .enumerate()
        {
            let mut ev = sample(i as i64 + 1, "s1", *outcome);
            ev.risk = *risk;
            trail.record(&ev).unwrap();
        }
        let rows = trail.query(&AuditQuery::default()).unwrap();
        assert_eq!(rows.len(), 4);
        // timestamp DESC: highest ts (critical/override) first.
        assert_eq!(rows[0].risk, AuditRisk::Critical);
        assert_eq!(rows[0].outcome, AuditOutcome::Override);
        assert_eq!(rows[3].risk, AuditRisk::Low);
        assert_eq!(rows[3].outcome, AuditOutcome::Allow);
    }

    #[test]
    fn corrupted_db_open_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("not-a-db.sqlite");
        // Write garbage bytes large enough that sqlite's "is this a DB?"
        // header check rejects the file (sqlite needs >=100 byte header).
        std::fs::write(&path, vec![0xAAu8; 4096]).unwrap();
        match AuditTrail::open(&path) {
            Ok(_) => panic!("expected open to fail on corrupted db"),
            Err(SwarmError::Audit(msg)) => {
                assert!(
                    !msg.is_empty(),
                    "audit error should carry a non-empty diagnostic"
                );
            }
            Err(other) => panic!("expected SwarmError::Audit, got {other:?}"),
        }
    }

    #[test]
    fn from_str_rejects_unknown_variants() {
        assert!(AuditRisk::from_str("nope").is_err());
        assert!(AuditOutcome::from_str("explode").is_err());
        // Sanity: round-trip every known variant.
        for r in [
            AuditRisk::Low,
            AuditRisk::Medium,
            AuditRisk::High,
            AuditRisk::Critical,
        ] {
            assert_eq!(AuditRisk::from_str(r.as_str()).unwrap(), r);
        }
        for o in [
            AuditOutcome::Allow,
            AuditOutcome::Deny,
            AuditOutcome::Escalate,
            AuditOutcome::Override,
        ] {
            assert_eq!(AuditOutcome::from_str(o.as_str()).unwrap(), o);
        }
    }
}
