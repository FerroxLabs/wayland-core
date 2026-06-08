//! M5.9 — token revocation: trait + sqlite-backed default impl.
//!
//! Layering: this module opens its own sqlite handle rather than reusing
//! `wcore-memory::db::Db`. Per `AGENTS.md` the permissions crate sits at the
//! mid layer with `wcore-types` / `wcore-config` only; depending on
//! `wcore-memory` would invert the graph (the agent crate depends on both).
//! The schema is forward-only and trivially small, so the extra handle is
//! cheap.
//!
//! Default impl is gated behind the `sqlite-revocation` cargo feature
//! (default-on) so embedders that ship their own revocation backing can drop
//! the rusqlite dep without forking this crate.

#[cfg(feature = "sqlite-revocation")]
use crate::error::DenyReason;
use crate::error::PolicyResult;

/// Storage for revoked token ids.
///
/// Implementations must be safe to share across threads — the engine holds
/// a single `Arc<dyn RevocationStore>` per session and calls it from any
/// task that verifies a token.
pub trait RevocationStore: Send + Sync + std::fmt::Debug {
    /// Mark the given token id as revoked. Idempotent: revoking a token id
    /// twice is a no-op, not an error.
    fn revoke(&self, token_id: &str) -> PolicyResult<()>;

    /// Returns `true` if the token id was previously revoked.
    fn is_revoked(&self, token_id: &str) -> PolicyResult<bool>;
}

#[cfg(feature = "sqlite-revocation")]
pub use sqlite_backend::SqliteRevocationStore;

#[cfg(feature = "sqlite-revocation")]
mod sqlite_backend {
    use super::{DenyReason, PolicyResult, RevocationStore};
    use std::path::Path;
    use std::sync::Mutex;

    /// Default `RevocationStore` impl backed by a bundled sqlite file.
    ///
    /// Schema (forward-only):
    /// ```sql
    /// CREATE TABLE IF NOT EXISTS revoked (
    ///     token_id   TEXT PRIMARY KEY,
    ///     revoked_at INTEGER NOT NULL    -- unix-ms wall clock
    /// );
    /// ```
    /// `INSERT OR IGNORE` makes `revoke()` idempotent without a SELECT round.
    pub struct SqliteRevocationStore {
        conn: Mutex<rusqlite::Connection>,
    }

    impl std::fmt::Debug for SqliteRevocationStore {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SqliteRevocationStore")
                .finish_non_exhaustive()
        }
    }

    impl SqliteRevocationStore {
        /// Open or create the revocation store at `path`. Creates the
        /// containing directory if needed.
        pub fn open(path: impl AsRef<Path>) -> PolicyResult<Self> {
            let conn = rusqlite::Connection::open(path.as_ref())
                .map_err(|e| DenyReason::Storage(e.to_string()))?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS revoked (\
                     token_id TEXT PRIMARY KEY,\
                     revoked_at INTEGER NOT NULL\
                 )",
            )
            .map_err(|e| DenyReason::Storage(e.to_string()))?;
            Ok(Self {
                conn: Mutex::new(conn),
            })
        }
    }

    impl RevocationStore for SqliteRevocationStore {
        fn revoke(&self, token_id: &str) -> PolicyResult<()> {
            let now = chrono::Utc::now().timestamp_millis();
            let c = self
                .conn
                .lock()
                .map_err(|_| DenyReason::Storage("revocation store mutex poisoned".into()))?;
            c.execute(
                "INSERT OR IGNORE INTO revoked (token_id, revoked_at) VALUES (?1, ?2)",
                rusqlite::params![token_id, now],
            )
            .map_err(|e| DenyReason::Storage(e.to_string()))?;
            Ok(())
        }

        fn is_revoked(&self, token_id: &str) -> PolicyResult<bool> {
            let c = self
                .conn
                .lock()
                .map_err(|_| DenyReason::Storage("revocation store mutex poisoned".into()))?;
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM revoked WHERE token_id = ?1",
                    rusqlite::params![token_id],
                    |r| r.get(0),
                )
                .map_err(|e| DenyReason::Storage(e.to_string()))?;
            Ok(n > 0)
        }
    }
}

#[cfg(test)]
mod tests {
    //! Internal unit coverage for the trait contract via a stub impl. The
    //! sqlite-backed default is exercised in `tests/revocation.rs`.

    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct InMemStub {
        ids: Mutex<std::collections::HashSet<String>>,
    }

    impl RevocationStore for InMemStub {
        fn revoke(&self, token_id: &str) -> PolicyResult<()> {
            self.ids.lock().unwrap().insert(token_id.to_string());
            Ok(())
        }
        fn is_revoked(&self, token_id: &str) -> PolicyResult<bool> {
            Ok(self.ids.lock().unwrap().contains(token_id))
        }
    }

    #[test]
    fn revoke_is_idempotent_and_visible() {
        let store = InMemStub::default();
        assert!(!store.is_revoked("abc").unwrap());
        store.revoke("abc").unwrap();
        store.revoke("abc").unwrap(); // second call must not error
        assert!(store.is_revoked("abc").unwrap());
        assert!(!store.is_revoked("def").unwrap());
    }
}
