// P4 Procedural — skill artifacts with Thompson stats + status state machine.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cdc::CdcWriter;
use crate::db::Db;
use crate::error::{MemoryError, Result};
use crate::v2_types::{Procedure, ProcedureId, ProcedureStatus, Tier};

pub struct ProceduralPartition {
    pub(crate) db: Arc<Db>,
    pub(crate) cdc: Arc<CdcWriter>,
}

impl ProceduralPartition {
    pub fn new(db: Arc<Db>, cdc: Arc<CdcWriter>) -> Self {
        Self { db, cdc }
    }

    pub async fn upsert(&self, mut p: Procedure) -> Result<ProcedureId> {
        if p.ts == 0 {
            p.ts = now_secs();
        }
        let tc = self.db.tier_or_global(p.tier);
        {
            let conn = tc.conn.lock();
            conn.execute(
                "INSERT OR REPLACE INTO procedures (id, tier, ts, name, description, artifact, status, created_by, thompson_alpha, thompson_beta, use_count, success_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![
                    p.id.0.to_string(),
                    p.tier.as_str(),
                    p.ts,
                    p.name,
                    p.description,
                    p.artifact,
                    p.status.as_str(),
                    p.created_by,
                    p.thompson_alpha,
                    p.thompson_beta,
                    p.use_count as i64,
                    p.success_count as i64,
                ],
            )?;
        }
        self.cdc.append_procedure(p.tier, &p)?;
        Ok(p.id)
    }

    /// Transition a procedure to a new status. Returns AccessDenied if the
    /// transition isn't in the allow table.
    pub async fn transition(
        &self,
        id: &ProcedureId,
        tier: Tier,
        next: ProcedureStatus,
    ) -> Result<()> {
        let tc = self.db.tier_or_global(tier);
        // S2: read current status and apply the UPDATE inside a single
        // transaction so concurrent writers cannot observe a stale status
        // and allow an otherwise-invalid transition.
        {
            let conn = tc.conn.lock();
            let tx = conn.unchecked_transaction().map_err(MemoryError::Db)?;
            let current_str: String = tx.query_row(
                "SELECT status FROM procedures WHERE id = ?1",
                [id.0.to_string()],
                |r| r.get(0),
            )?;
            let current: ProcedureStatus = current_str
                .parse()
                .map_err(|e: String| MemoryError::Consolidation(e))?;
            if !current.can_transition_to(next) {
                return Err(MemoryError::AccessDenied {
                    partition: "procedural".into(),
                    tier: tier.to_string(),
                    reason: format!("invalid status transition {current} -> {next}"),
                });
            }
            tx.execute(
                "UPDATE procedures SET status = ?1 WHERE id = ?2",
                rusqlite::params![next.as_str(), id.0.to_string()],
            )?;
            tx.commit().map_err(MemoryError::Db)?;
        }
        self.cdc
            .append_procedure_status(tier, &id.0, next.as_str())?;
        Ok(())
    }

    /// Record a Thompson-stat update.
    pub async fn record_use(&self, id: &ProcedureId, tier: Tier, succeeded: bool) -> Result<()> {
        let tc = self.db.tier_or_global(tier);
        let (alpha, beta) = {
            let conn = tc.conn.lock();
            conn.execute(
                "UPDATE procedures SET use_count = use_count + 1, success_count = success_count + ?1,
                 thompson_alpha = thompson_alpha + ?2, thompson_beta = thompson_beta + ?3 WHERE id = ?4",
                rusqlite::params![
                    if succeeded { 1i64 } else { 0i64 },
                    if succeeded { 1.0f64 } else { 0.0f64 },
                    if succeeded { 0.0f64 } else { 1.0f64 },
                    id.0.to_string(),
                ],
            )?;
            conn.query_row(
                "SELECT thompson_alpha, thompson_beta FROM procedures WHERE id = ?1",
                [id.0.to_string()],
                |r| Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?)),
            )?
        };
        self.cdc
            .append_procedure_use(tier, &id.0, succeeded, alpha, beta)?;
        Ok(())
    }

    /// Return every procedure row at the given tier. Small by design
    /// (tens to hundreds of skills); no pagination.
    pub async fn list(&self, tier: Tier) -> Result<Vec<Procedure>> {
        let tc = self.db.tier_or_global(tier);
        let conn = tc.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, tier, ts, name, description, artifact, status, created_by, thompson_alpha, thompson_beta, use_count, success_count FROM procedures WHERE tier = ?1 ORDER BY ts DESC"
        )?;
        let rows = stmt.query_map([tier.as_str()], |row| {
            let id_s: String = row.get(0)?;
            let tier_s: String = row.get(1)?;
            let status_s: String = row.get(6)?;
            let parsed_status: ProcedureStatus = status_s
                .parse()
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(Procedure {
                id: ProcedureId(
                    uuid::Uuid::parse_str(&id_s).map_err(|_| rusqlite::Error::InvalidQuery)?,
                ),
                tier: tier_s.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                ts: row.get(2)?,
                name: row.get(3)?,
                description: row.get(4)?,
                artifact: row.get(5)?,
                status: parsed_status,
                created_by: row.get(7)?,
                thompson_alpha: row.get(8)?,
                thompson_beta: row.get(9)?,
                use_count: row.get::<_, i64>(10)? as u64,
                success_count: row.get::<_, i64>(11)? as u64,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(MemoryError::Db)?);
        }
        Ok(out)
    }

    pub async fn get(&self, id: &ProcedureId, tier: Tier) -> Result<Procedure> {
        let tc = self.db.tier_or_global(tier);
        let conn = tc.conn.lock();
        let r = conn.query_row(
            "SELECT id, tier, ts, name, description, artifact, status, created_by, thompson_alpha, thompson_beta, use_count, success_count FROM procedures WHERE id = ?1",
            [id.0.to_string()],
            |row| {
                let id_s: String = row.get(0)?;
                let tier_s: String = row.get(1)?;
                let status_s: String = row.get(6)?;
                let parsed_status: ProcedureStatus =
                    status_s.parse().map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok(Procedure {
                    id: ProcedureId(
                        uuid::Uuid::parse_str(&id_s).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    ),
                    tier: tier_s.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                    ts: row.get(2)?,
                    name: row.get(3)?,
                    description: row.get(4)?,
                    artifact: row.get(5)?,
                    status: parsed_status,
                    created_by: row.get(7)?,
                    thompson_alpha: row.get(8)?,
                    thompson_beta: row.get(9)?,
                    use_count: row.get::<_, i64>(10)? as u64,
                    success_count: row.get::<_, i64>(11)? as u64,
                })
            },
        );
        match r {
            Ok(p) => Ok(p),
            Err(rusqlite::Error::QueryReturnedNoRows) => Err(MemoryError::Consolidation(format!(
                "procedure {} not found",
                id.0
            ))),
            Err(e) => Err(MemoryError::Db(e)),
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
