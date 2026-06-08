// Per-tier connection pool for v2 cognitive memory.
//
// Each Db owns one connection per (logical) tier — session, project, global.
// The connections are kept behind `parking_lot::Mutex` and lazily opened.
// The same schema is applied to every tier; partition gating happens above
// in the dispatcher + gate, not in the DB layer.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use rusqlite::Connection;

use crate::error::{MemoryError, Result};
use crate::schema;
use crate::v2_types::Tier;

/// Register the sqlite-vec extension once per process. Subsequent calls
/// are no-ops via `std::sync::Once`. After this runs every newly-opened
/// SQLite Connection (bundled SQLite, in this workspace) gains the vec0
/// virtual-table family. M4.8 — see init_extensions for context.
fn register_sqlite_vec() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: sqlite-vec's entry point is a stable C ABI function with
        // exactly the SQLite-loadable-extension shape that rusqlite's
        // `sqlite3_auto_extension` expects:
        //   int (*)(sqlite3*, char**, const sqlite3_api_routines*)
        // We must transmute through a raw `*const ()` because sqlite-vec
        // exposes the function via its own bundled sqlite3.h binding,
        // which Rust sees as a nominally distinct type from
        // rusqlite::ffi's bundled binding (same ABI, different identity).
        // The Once guarantees we register exactly once for the process.
        let entry_ptr = sqlite_vec::sqlite3_vec_init as *const ();
        unsafe {
            let entry: unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::os::raw::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::os::raw::c_int = std::mem::transmute(entry_ptr);
            rusqlite::ffi::sqlite3_auto_extension(Some(entry));
        }
    });
}

/// M5.7 — name the per-dim sqlite-vec virtual table for a given embedder
/// dim. Stable wire format: `vec_episodes_<dim>` for every dim except
/// 384, which retains the legacy v3 name `vec_episodes` so existing
/// on-disk databases keep working without a destructive migration.
///
/// Kept as a free function (no `&self`) so callers can compute the name
/// from `Embedder::dim()` without holding a `Db` handle.
pub fn vec_table_name_for_dim(dim: usize) -> String {
    if dim == 384 {
        "vec_episodes".to_string()
    } else {
        format!("vec_episodes_{dim}")
    }
}

/// Set owner-only (0o600) permissions on a memory file. Best-effort,
/// Unix-only; missing files and non-Unix platforms are silent no-ops.
pub(crate) fn harden_file_perms(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.exists() {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Set owner-only (0o700) permissions on a memory directory. Best-effort,
/// Unix-only.
pub(crate) fn harden_dir_perms(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.exists() {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// One open DB connection plus the file it lives in.
pub struct TierConn {
    pub path: PathBuf,
    pub conn: Mutex<Connection>,
}

impl TierConn {
    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            // Memory holds recalled episode summaries / extracted facts /
            // user-model k/v — at-rest content the user opted to memorize.
            // Lock the dir down so a shared host / group-readable checkout
            // can't read it. Mirrors the 0600 posture of secrets elsewhere.
            harden_dir_perms(parent);
        }
        // Register sqlite-vec BEFORE opening so this connection (and every
        // future one) sees the `vec0` module. sqlite3_auto_extension's
        // contract is forward-only — connections opened before
        // registration don't gain the extension.
        register_sqlite_vec();
        let mut conn = Connection::open(&path).map_err(MemoryError::Db)?;
        schema::apply_migrations(&mut conn)?;
        // Restrict the DB and its WAL/SHM sidecars to owner-only. The
        // sidecars exist once WAL journaling is active (schema migrations
        // run statements, so they're present by now in practice; harden
        // them best-effort regardless).
        harden_file_perms(&path);
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.clone().into_os_string();
            sidecar.push(suffix);
            harden_file_perms(std::path::Path::new(&sidecar));
        }
        Ok(Self {
            path,
            conn: Mutex::new(conn),
        })
    }

    /// In-memory variant (used by tests).
    pub fn open_memory() -> Result<Self> {
        register_sqlite_vec();
        let mut conn = Connection::open_in_memory().map_err(MemoryError::Db)?;
        schema::apply_migrations(&mut conn)?;
        Ok(Self {
            path: PathBuf::from(":memory:"),
            conn: Mutex::new(conn),
        })
    }
}

/// Three-tier DB pool. Project + session DBs are optional (None when the
/// dispatcher is constructed without a project root / session id).
pub struct Db {
    /// The session-tier connection. Interior-mutable so the engine can
    /// rebind it from the bootstrap-time `"boot"` placeholder id to the real
    /// session id once `init_session` runs (see [`Db::rebind_session`]) —
    /// without reconstructing `Db`, so every partition store sharing this
    /// `Arc<Db>` (and the dispatcher's attached trace sink + decay scheduler)
    /// stays wired through the swap.
    pub session: RwLock<Option<Arc<TierConn>>>,
    pub project: Option<Arc<TierConn>>,
    pub global: Arc<TierConn>,
}

impl Db {
    /// Open the global tier only — used by tests that only exercise the
    /// global path.
    pub fn open_global(global_path: PathBuf) -> Result<Self> {
        Ok(Self {
            session: RwLock::new(None),
            project: None,
            global: Arc::new(TierConn::open(global_path)?),
        })
    }

    /// Open all three tiers given resolved file paths.
    pub fn open(
        session_path: Option<PathBuf>,
        project_path: Option<PathBuf>,
        global_path: PathBuf,
    ) -> Result<Self> {
        let session = match session_path {
            Some(p) => Some(Arc::new(TierConn::open(p)?)),
            None => None,
        };
        let project = match project_path {
            Some(p) => Some(Arc::new(TierConn::open(p)?)),
            None => None,
        };
        let global = Arc::new(TierConn::open(global_path)?);
        Ok(Self {
            session: RwLock::new(session),
            project,
            global,
        })
    }

    /// In-memory pool (tests).
    pub fn open_memory() -> Result<Self> {
        Ok(Self {
            session: RwLock::new(Some(Arc::new(TierConn::open_memory()?))),
            project: Some(Arc::new(TierConn::open_memory()?)),
            global: Arc::new(TierConn::open_memory()?),
        })
    }

    /// Rebind the session-tier connection to a new on-disk DB file. Opens the
    /// new connection (applying the schema), then swaps it in atomically so
    /// every holder of this `Arc<Db>` reads the new file on its next access.
    ///
    /// Production bootstrap opens the session tier under a synthetic `"boot"`
    /// id (the real session id isn't known until `init_session`); this is how
    /// the engine moves session-tier reads/writes onto the real per-session
    /// file, giving true per-session isolation and a bounded, cleanable DB
    /// instead of one ever-growing shared `boot.db`.
    pub fn rebind_session(&self, session_path: PathBuf) -> Result<()> {
        let tc = Arc::new(TierConn::open(session_path)?);
        // The KG schema is intentionally NOT part of `apply_migrations`, so the
        // freshly-opened per-session connection lacks `kg_nodes`/`kg_edges`/
        // `kg_node_staleness`. Bootstrap runs `init_kg`/`init_staleness` on the
        // synthetic `"boot"` session DB, but this rebind swaps in a brand-new
        // connection — without re-running the init here, any dream/ingest cycle
        // that resolves `Tier::Session` hits "no such table: kg_edges". Mirror
        // bootstrap's gating + ordering (kg_nodes before its staleness FK), and
        // warn-but-continue so a KG init failure never breaks session rebinding.
        if crate::kg::kg_enabled() {
            let conn = tc.conn.lock();
            if let Err(e) = crate::kg::init_kg(&conn) {
                tracing::warn!("rebind_session: init_kg failed error={e}");
            } else if let Err(e) = crate::staleness::init_staleness(&conn) {
                tracing::warn!("rebind_session: init_staleness failed error={e}");
            }
        }
        *self.session.write() = Some(tc);
        Ok(())
    }

    /// Get the connection for a tier. Returns None if that tier wasn't
    /// configured (e.g. session DB on a project-only Memory handle).
    pub fn tier(&self, t: Tier) -> Option<Arc<TierConn>> {
        match t {
            Tier::Session => self.session.read().clone(),
            Tier::Project => self.project.clone(),
            Tier::Global => Some(self.global.clone()),
        }
    }

    /// Get the connection for a tier or fall back to the global tier — used
    /// by features that must persist even when the caller hasn't configured
    /// session/project (e.g. legacy import in a non-project context).
    pub fn tier_or_global(&self, t: Tier) -> Arc<TierConn> {
        self.tier(t).unwrap_or_else(|| self.global.clone())
    }

    /// M5.7 — ensure the per-dim `vec_episodes_<dim>` virtual table exists
    /// on every tier connection AND is recorded in the v4 registry.
    /// Idempotent: re-calling for the same dim is a no-op after the first
    /// successful create.
    ///
    /// `CREATE VIRTUAL TABLE USING vec0` cannot run inside a transaction
    /// (a SQLite v3 restriction sqlite-vec inherits), so we run the
    /// create with auto-commit and rely on `IF NOT EXISTS` for
    /// idempotency across crashes / re-opens.
    ///
    /// Called by `EpisodicPartition::record_with_embedding` on every
    /// write — the cost after first-create is one cheap registry SELECT.
    pub fn ensure_vec_table_for_dim(&self, dim: usize) -> Result<String> {
        let table = vec_table_name_for_dim(dim);
        for tc_opt in [
            self.session.read().clone(),
            self.project.clone(),
            Some(self.global.clone()),
        ]
        .into_iter()
        .flatten()
        {
            ensure_one_tier(tc_opt.as_ref(), &table, dim)?;
        }
        Ok(table)
    }
}

fn ensure_one_tier(tc: &TierConn, table: &str, dim: usize) -> Result<()> {
    let conn = tc.conn.lock();
    // Fast path: registry already has this dim AND the virtual table
    // physically exists. The two checks together cover the legacy
    // `vec_episodes` (which is seeded by the v4 migration even on
    // fresh dbs where v3 ran first) and any new per-dim tables.
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1 AND type = 'table'",
            [table],
            |r| r.get(0),
        )
        .map_err(MemoryError::Db)?;
    if exists == 0 {
        // CREATE VIRTUAL TABLE outside a transaction — sqlite-vec
        // refuses inside one. IF NOT EXISTS keeps it idempotent if
        // two writers race on a fresh db.
        let create_sql = format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {table} USING vec0(embedding float[{dim}])"
        );
        conn.execute_batch(&create_sql).map_err(MemoryError::Db)?;
    }
    // Registry insert — also idempotent.
    conn.execute(
        "INSERT OR IGNORE INTO vec_episodes_registry (dim, table_name) VALUES (?1, ?2)",
        rusqlite::params![dim as i64, table],
    )
    .map_err(MemoryError::Db)?;
    Ok(())
}

/// Convenience: list every table+index name in the connection. Used by the
/// migration test.
pub fn list_objects(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type IN ('table','index','view','trigger') ORDER BY name",
    )?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_memory_applies_schema() {
        let tc = TierConn::open_memory().unwrap();
        let conn = tc.conn.lock();
        let names = list_objects(&conn).unwrap();
        assert!(names.contains(&"episodes".into()), "{names:?}");
        assert!(names.contains(&"facts".into()), "{names:?}");
        assert!(names.contains(&"procedures".into()), "{names:?}");
        assert!(names.contains(&"user_model".into()), "{names:?}");
        assert!(names.contains(&"cdc_log".into()), "{names:?}");
        assert!(names.contains(&"schema_version".into()), "{names:?}");
        assert!(names.contains(&"legacy_import_marker".into()), "{names:?}");
        assert!(names.contains(&"p1_working".into()), "{names:?}");
        // At least one index for episodes(tier, ts).
        assert!(
            names.iter().any(|n| n == "idx_episodes_tier_ts"),
            "missing idx: {names:?}"
        );
    }

    #[test]
    fn open_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("memory.db");
        let _a = TierConn::open(path.clone()).unwrap();
        // Re-open: schema migration must not error.
        let _b = TierConn::open(path).unwrap();
    }

    #[test]
    fn rebind_session_swaps_session_connection_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let boot = tmp.path().join("sessions").join("boot.db");
        let real = tmp.path().join("sessions").join("real-abc123.db");
        let global = tmp.path().join("global.db");

        let db = Db::open(Some(boot.clone()), None, global).unwrap();
        // Opens on the bootstrap "boot" file.
        assert_eq!(db.tier(Tier::Session).unwrap().path, boot);

        // Rebind to the real per-session file.
        db.rebind_session(real.clone()).unwrap();
        let after = db.tier(Tier::Session).unwrap();
        assert_eq!(after.path, real, "session tier must point at the real db");
        assert_ne!(after.path, boot, "must no longer use the boot db");
        // The new connection is live (schema applied) — a trivial query works.
        assert!(after.conn.lock().execute_batch("SELECT 1;").is_ok());
    }

    #[test]
    fn schema_version_is_one() {
        let tc = TierConn::open_memory().unwrap();
        let conn = tc.conn.lock();
        let v = schema::current_schema_version(&conn).unwrap();
        assert_eq!(v, schema::CURRENT_VERSION);
    }

    #[cfg(unix)]
    #[test]
    fn open_hardens_db_and_dir_perms() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("memory");
        let path = dir.join("memory.db");
        let tc = TierConn::open(path.clone()).unwrap();
        // Force WAL sidecars to materialize so we can assert on them.
        {
            let conn = tc.conn.lock();
            conn.execute_batch(
                "PRAGMA journal_mode=WAL; \
                 CREATE TABLE IF NOT EXISTS _t(x); INSERT INTO _t VALUES (1);",
            )
            .unwrap();
        }
        // Re-harden in case the WAL files appeared after open (best-effort).
        harden_file_perms(&path);
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.clone().into_os_string();
            sidecar.push(suffix);
            harden_file_perms(std::path::Path::new(&sidecar));
        }

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "memory dir should be 0700, got {dir_mode:o}"
        );

        let db_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(db_mode, 0o600, "db file should be 0600, got {db_mode:o}");

        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.clone().into_os_string();
            sidecar.push(suffix);
            let sc = std::path::PathBuf::from(&sidecar);
            if sc.exists() {
                let m = std::fs::metadata(&sc).unwrap().permissions().mode() & 0o777;
                assert_eq!(m, 0o600, "{suffix} sidecar should be 0600, got {m:o}");
            }
        }
    }
}
