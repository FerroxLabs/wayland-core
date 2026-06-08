// P5 Core — user-model k/v with system-only-write invariant.
//
// The gate refuses non-System tokens before this code runs. The internal
// `debug_assert!` here is belt-and-suspenders for any future caller that
// somehow bypasses the gate.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::cdc::CdcWriter;
use crate::db::Db;
use crate::error::{MemoryError, Result};
use crate::v2_types::{AccessToken, UserModel, UserModelEntry};

pub struct CorePartition {
    pub(crate) db: Arc<Db>,
    pub(crate) cdc: Arc<CdcWriter>,
}

impl CorePartition {
    pub fn new(db: Arc<Db>, cdc: Arc<CdcWriter>) -> Self {
        Self { db, cdc }
    }

    /// System-only write. Records a (key, value) into the global user_model
    /// table. Emits a CDC delta with old/new values.
    pub async fn update(&self, key: &str, value: Value, token: &AccessToken) -> Result<()> {
        debug_assert!(matches!(token, AccessToken::System));
        if !matches!(token, AccessToken::System) {
            return Err(MemoryError::AccessDenied {
                partition: "core".into(),
                tier: "global".into(),
                reason: "user_model write requires SystemToken".into(),
            });
        }
        let tc = self.db.global.clone();
        let ts = now_secs();
        let old_value: Value = {
            let conn = tc.conn.lock();
            let prev: rusqlite::Result<String> = conn.query_row(
                "SELECT value_json FROM user_model WHERE key = ?1",
                [key],
                |r| r.get(0),
            );
            match prev {
                Ok(s) => serde_json::from_str(&s).unwrap_or(Value::Null),
                Err(rusqlite::Error::QueryReturnedNoRows) => Value::Null,
                Err(e) => return Err(MemoryError::Db(e)),
            }
        };
        let value_str = serde_json::to_string(&value)
            .map_err(|e| MemoryError::Consolidation(format!("user_model serialize: {e}")))?;
        {
            let conn = tc.conn.lock();
            conn.execute(
                "INSERT INTO user_model (key, value_json, ts) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, ts = excluded.ts",
                rusqlite::params![key, value_str, ts],
            )?;
        }
        self.cdc.append_user_model_delta(key, &old_value, &value)?;
        Ok(())
    }

    pub async fn read_all(&self) -> Result<UserModel> {
        let tc = self.db.global.clone();
        let conn = tc.conn.lock();
        let mut stmt =
            conn.prepare("SELECT key, value_json, ts FROM user_model ORDER BY key ASC")?;
        let rows = stmt.query_map([], |r| {
            let key: String = r.get(0)?;
            let val_s: String = r.get(1)?;
            let ts: i64 = r.get(2)?;
            Ok(UserModelEntry {
                key,
                value: serde_json::from_str(&val_s).unwrap_or(Value::Null),
                ts,
            })
        })?;
        let mut entries = Vec::new();
        for r in rows {
            entries.push(r.map_err(MemoryError::Db)?);
        }
        Ok(UserModel { entries })
    }

    pub async fn read_key(&self, key: &str) -> Result<Option<Value>> {
        let tc = self.db.global.clone();
        let conn = tc.conn.lock();
        let r: rusqlite::Result<String> = conn.query_row(
            "SELECT value_json FROM user_model WHERE key = ?1",
            [key],
            |row| row.get(0),
        );
        match r {
            Ok(s) => Ok(Some(serde_json::from_str(&s).unwrap_or(Value::Null))),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
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
