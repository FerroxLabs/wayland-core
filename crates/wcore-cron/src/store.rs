//! Cron job persistence.
//!
//! [`CronStore`] is the trait the runner reads/writes through. A
//! production [`FileCronStore`] persists to a JSON file under
//! `~/.wayland/cron/jobs.json` (honoring `WAYLAND_HOME` when set, like
//! the rest of the codebase). Writes go through a tempfile + rename so
//! concurrent crashes don't corrupt the file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{CronError, CronJob, Result};

/// File / env constants. Mirrors the rest of the codebase.
const WAYLAND_HOME_ENV: &str = "WAYLAND_HOME";
const WAYLAND_HOME_DIRNAME: &str = ".wayland";
const CRON_SUBDIR: &str = "cron";
const JOBS_FILE: &str = "jobs.json";
const HISTORY_FILE: &str = "history.jsonl";

/// Per-host integrity key file, stored alongside `jobs.json` at mode 0600.
/// Created on first engine write. The keyed MAC over the canonical job set
/// is what distinguishes engine-authored files from a direct tamper.
const INTEGRITY_KEY_FILE: &str = ".integrity.key";

/// Resolve the default JSON store path:
/// `$WAYLAND_HOME/cron/jobs.json` if `WAYLAND_HOME` is set, else
/// `~/.wayland/cron/jobs.json`. Returns `None` only if neither
/// `WAYLAND_HOME` nor `$HOME` can be resolved (extremely rare).
pub fn default_store_path() -> Option<PathBuf> {
    let home = std::env::var_os(WAYLAND_HOME_ENV)
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(WAYLAND_HOME_DIRNAME)))?;
    Some(home.join(CRON_SUBDIR).join(JOBS_FILE))
}

/// Resolve the default JSONL history path:
/// `$WAYLAND_HOME/cron/history.jsonl` (parallel to `jobs.json`).
pub fn default_history_path() -> Option<PathBuf> {
    let home = std::env::var_os(WAYLAND_HOME_ENV)
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(WAYLAND_HOME_DIRNAME)))?;
    Some(home.join(CRON_SUBDIR).join(HISTORY_FILE))
}

/// Reads + writes of the job set.
///
/// Designed to be cheap to clone (`Arc` on the impl side) and safe
/// across tasks. The runner clones one handle per tick.
#[async_trait]
pub trait CronStore: Send + Sync {
    async fn list(&self) -> Result<Vec<CronJob>>;
    async fn insert(&self, job: CronJob) -> Result<()>;
    async fn update(&self, job: CronJob) -> Result<()>;
    async fn remove(&self, id: &str) -> Result<()>;
    async fn set_enabled(&self, id: &str, enabled: bool) -> Result<()>;

    /// Jobs eligible for *unattended auto-fire* (M-19). Defaults to [`list`]
    /// for in-memory / non-file stores; [`FileCronStore`] overrides this to
    /// withhold jobs from a tampered or foreign-owned `jobs.json`. The runner
    /// calls this (never bare `list`) so a hostile writer can't get the 30s
    /// tick to execute jobs it authored.
    async fn list_for_run(&self) -> Result<Vec<CronJob>> {
        self.list().await
    }
}

/// JSON-file backed store. Read-then-write under one async mutex so
/// updates are serialized; tempfile-then-rename so partial writes can't
/// corrupt the persisted file.
#[derive(Clone)]
pub struct FileCronStore {
    path: PathBuf,
    inner: Arc<Mutex<()>>,
}

#[derive(Default, Serialize, Deserialize)]
struct JobsFile {
    jobs: Vec<CronJob>,
    /// M-19: keyed integrity tag over the canonical `jobs` array. Stamped on
    /// every engine write. Absent on legacy/Desktop-app files (those are
    /// loaded but flagged untrusted so the runner can withhold auto-fire).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    integrity: Option<String>,
}

/// Outcome of validating a freshly-read jobs file (M-19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobsTrust {
    /// File carries a valid integrity tag for the current host key — these
    /// jobs were written by this engine and are safe to auto-run.
    Trusted,
    /// File has no integrity tag (legacy or Desktop-app authored). Loaded for
    /// visibility but the runner must NOT auto-fire it.
    Untagged,
    /// File carries an integrity tag that does NOT match — direct tamper.
    /// Jobs are withheld entirely.
    Tampered,
}

/// M-19: compute a keyed MAC over the canonical job set. This is a minimal,
/// dependency-free keyed hash (wcore-cron cannot pull in a real HMAC crate
/// without a Cargo.toml change / dependency churn). It is NOT a cryptographic
/// HMAC, but it raises the bar from "any writer fires" to "a writer must also
/// possess the 0600 host key", which closes the unattended-tamper amplifier.
fn integrity_tag(key: &[u8], jobs: &[CronJob]) -> Result<String> {
    // Canonicalize the jobs (stable serde order) so the tag is stable across
    // reads/writes of the same logical content.
    let canonical = serde_json::to_vec(jobs)?;
    Ok(keyed_hash_hex(key, &canonical))
}

/// Constant-time byte comparison for the integrity tag (avoid leaking the
/// match prefix length via early return). Lengths differing is an immediate
/// mismatch; equal-length inputs are compared without short-circuit.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Restrict a file to owner read/write (0600) on Unix. No-op elsewhere.
#[cfg(unix)]
fn set_owner_only_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_owner_only_perms(_path: &Path) {}

// Minimal FFI for the running user's real uid. Declared locally because
// wcore-cron does not (and should not, to keep its dep surface tiny) depend
// on the `libc` crate. `getuid` is always-succeeds and async-signal-safe.
#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

/// FNV-1a-based keyed hash over `key || data || key`. 128-bit output as hex.
/// Two independent FNV streams (different offset bases) widen the digest to
/// 128 bits to make blind collisions impractical for the threat model
/// (offline tamper of a local file).
fn keyed_hash_hex(key: &[u8], data: &[u8]) -> String {
    fn fnv1a(seed: u64, parts: &[&[u8]]) -> u64 {
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut h = seed;
        for part in parts {
            for &b in *part {
                h ^= b as u64;
                h = h.wrapping_mul(PRIME);
            }
        }
        h
    }
    let lo = fnv1a(0xcbf2_9ce4_8422_2325, &[key, data, key]);
    let hi = fnv1a(0x9e37_79b9_7f4a_7c15, &[key, data, key]);
    format!("{lo:016x}{hi:016x}")
}

impl FileCronStore {
    /// Construct from an explicit path. Caller is responsible for
    /// ensuring parents are creatable.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            inner: Arc::new(Mutex::new(())),
        }
    }

    /// Construct using [`default_store_path`]. Returns an error if the
    /// store path can't be resolved.
    pub fn from_default_path() -> Result<Self> {
        let path = default_store_path().ok_or_else(|| {
            CronError::Store("cannot resolve WAYLAND_HOME or $HOME for cron store".into())
        })?;
        Ok(Self::new(path))
    }

    /// Path the store writes to. Useful for diagnostics + tests.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Path to the per-host integrity key (sibling of `jobs.json`).
    fn key_path(&self) -> PathBuf {
        match self.path.parent() {
            Some(parent) => parent.join(INTEGRITY_KEY_FILE),
            None => PathBuf::from(INTEGRITY_KEY_FILE),
        }
    }

    /// Load the host integrity key, creating it (0600) on first use. The key
    /// is a random 32-byte token derived from `uuid` v4 entropy (no extra
    /// crate). Returns `None` only if the key can neither be read nor created
    /// (e.g. unwritable dir) — callers then treat all files as untrusted.
    fn integrity_key(&self) -> Option<Vec<u8>> {
        let kp = self.key_path();
        if let Ok(bytes) = std::fs::read(&kp)
            && !bytes.is_empty()
        {
            return Some(bytes);
        }
        // Generate 32 bytes from two v4 UUIDs.
        let mut key = Vec::with_capacity(32);
        key.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        key.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        if let Some(parent) = kp.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&kp, &key).is_err() {
            return None;
        }
        set_owner_only_perms(&kp);
        Some(key)
    }

    /// M-19: ownership + permission gate. On Unix, hard-refuse to load a jobs
    /// file that is not owned by the running user (cross-user tamper), and WARN
    /// (not refuse, to preserve legacy/Desktop files) when the mode is more
    /// permissive than 0600. Returns `Ok(())` when safe (or on non-Unix where
    /// the check does not apply).
    #[cfg(unix)]
    fn check_ownership_and_perms(path: &Path) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            // Missing file is fine — there's nothing to trust.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        // SAFETY: getuid is always-succeeds and thread-safe.
        let uid = unsafe { libc_getuid() };
        if meta.uid() != uid {
            return Err(CronError::Store(format!(
                "refusing to load cron jobs from {}: owned by uid {} not running uid {}",
                path.display(),
                meta.uid(),
                uid
            )));
        }
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            // Group/other-accessible: the file may carry secrets-in-prompts and
            // predates 0600 hardening (legacy / Desktop-app authored). Since we
            // already proved the file is owned by the running user above, just
            // self-heal — tighten to 0600 in place rather than waiting for the
            // next engine `write_file`. Only warn if the tightening fails to
            // clear the group/other bits, so a clean boot stays quiet.
            set_owner_only_perms(path);
            let still_loose = std::fs::metadata(path)
                .map(|m| m.permissions().mode() & 0o077 != 0)
                .unwrap_or(true);
            if still_loose {
                tracing::warn!(
                    target: "wcore_cron::store",
                    path = %path.display(),
                    mode = format!("{mode:o}"),
                    "cron jobs.json is group/other-accessible; expected 0600 (could not auto-tighten)"
                );
            }
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn check_ownership_and_perms(_path: &Path) -> Result<()> {
        Ok(())
    }

    /// Read + classify trust in one shot (M-19). Performs the ownership/perm
    /// gate first (hard refuse on failure), then parses, then verifies the
    /// integrity tag against the host key.
    fn read_validated(&self) -> Result<(JobsFile, JobsTrust)> {
        Self::check_ownership_and_perms(&self.path)?;
        let file = self.read_file()?;
        let trust = match (&file.integrity, self.integrity_key()) {
            (Some(tag), Some(key)) => {
                let expected = integrity_tag(&key, &file.jobs)?;
                if constant_time_eq(tag.as_bytes(), expected.as_bytes()) {
                    JobsTrust::Trusted
                } else {
                    JobsTrust::Tampered
                }
            }
            // Tag present but no key reachable — cannot verify, treat as tamper
            // to fail closed.
            (Some(_), None) => JobsTrust::Tampered,
            // No tag at all — legacy / Desktop-app authored.
            (None, _) => JobsTrust::Untagged,
        };
        Ok((file, trust))
    }

    fn read_file(&self) -> Result<JobsFile> {
        match std::fs::read(&self.path) {
            Ok(bytes) if bytes.is_empty() => Ok(JobsFile::default()),
            Ok(bytes) => {
                // First try a strict parse. This is the fast path for
                // engine-written files (correct schema, all fields present).
                if let Ok(file) = serde_json::from_slice::<JobsFile>(&bytes) {
                    return Ok(file);
                }
                // Lenient fallback: the Desktop app writes a `jobs` array whose
                // entries have a different schema (no `target`, no `created_at`,
                // `schedule` instead of `expression`, extra fields like `name`,
                // `state`, `prompt`, etc.). Rather than failing the whole tick,
                // parse the array element-by-element and skip entries that don't
                // conform to `CronJob`. A WARN for skipped entries helps
                // diagnose without silently hiding breakage on engine-written
                // files.
                #[derive(serde::Deserialize)]
                struct RawJobsFile {
                    #[serde(default)]
                    jobs: Vec<serde_json::Value>,
                }
                let raw: RawJobsFile = serde_json::from_slice(&bytes).map_err(CronError::Serde)?;
                let jobs = raw
                    .jobs
                    .into_iter()
                    .filter_map(|v| match serde_json::from_value::<CronJob>(v) {
                        Ok(j) => Some(j),
                        Err(e) => {
                            tracing::debug!(
                                target: "wcore_cron::store",
                                error = %e,
                                "skipping incompatible job entry (Desktop-app schema?)"
                            );
                            None
                        }
                    })
                    .collect();
                // Lenient path is only reached when the strict (engine) schema
                // failed — so there is no trustworthy integrity tag here.
                Ok(JobsFile {
                    jobs,
                    integrity: None,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(JobsFile::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn write_file(&self, file: &JobsFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // M-19: stamp a fresh integrity tag over the canonical job set so
        // engine writes are distinguishable from a direct tamper. If the host
        // key can't be obtained (unwritable dir), write without a tag — the
        // file then loads as Untagged (visible, not auto-run) rather than
        // failing the write outright.
        let stamped = JobsFile {
            jobs: file.jobs.clone(),
            integrity: self
                .integrity_key()
                .map(|key| integrity_tag(&key, &file.jobs))
                .transpose()?,
        };
        // Tempfile + rename for atomic publish. Tempfile lives in the
        // same directory so the rename stays within one filesystem.
        let bytes = serde_json::to_vec_pretty(&stamped)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        // Lock the published file down to 0600 so the ownership/perm gate on
        // the next load passes and so secrets-in-prompts aren't world-readable.
        set_owner_only_perms(&tmp);
        std::fs::rename(&tmp, &self.path)?;
        set_owner_only_perms(&self.path);
        Ok(())
    }
}

#[async_trait]
impl CronStore for FileCronStore {
    async fn list(&self) -> Result<Vec<CronJob>> {
        let _guard = self.inner.lock().await;
        // Visibility path: still enforce the ownership/perm gate (a
        // foreign-owned file is refused outright), but return whatever loads
        // so `cron status`/CLI can show legacy + Desktop-app jobs. Auto-fire
        // trust is enforced separately in `list_for_run`.
        let (file, _trust) = self.read_validated()?;
        Ok(file.jobs)
    }

    async fn list_for_run(&self) -> Result<Vec<CronJob>> {
        let _guard = self.inner.lock().await;
        let (file, trust) = self.read_validated()?;
        match trust {
            JobsTrust::Trusted => Ok(file.jobs),
            JobsTrust::Tampered => {
                tracing::warn!(
                    target: "wcore_cron::store",
                    path = %self.path.display(),
                    "cron jobs.json integrity tag mismatch (tamper) — withholding ALL jobs from auto-fire"
                );
                Ok(Vec::new())
            }
            JobsTrust::Untagged => {
                // Legacy / Desktop-app authored: no engine-stamped tag. Fail
                // closed for unattended execution — these are not provably
                // engine-authored, so do not auto-fire them.
                if !file.jobs.is_empty() {
                    tracing::warn!(
                        target: "wcore_cron::store",
                        path = %self.path.display(),
                        count = file.jobs.len(),
                        "cron jobs.json has no integrity tag — withholding from auto-fire until rewritten by the engine"
                    );
                }
                Ok(Vec::new())
            }
        }
    }

    async fn insert(&self, job: CronJob) -> Result<()> {
        let _guard = self.inner.lock().await;
        let mut file = self.read_file()?;
        // Replace-or-append by id so retries are idempotent.
        if let Some(slot) = file.jobs.iter_mut().find(|j| j.id == job.id) {
            *slot = job;
        } else {
            file.jobs.push(job);
        }
        self.write_file(&file)
    }

    async fn update(&self, job: CronJob) -> Result<()> {
        let _guard = self.inner.lock().await;
        let mut file = self.read_file()?;
        let slot = file
            .jobs
            .iter_mut()
            .find(|j| j.id == job.id)
            .ok_or_else(|| CronError::NotFound(job.id.clone()))?;
        *slot = job;
        self.write_file(&file)
    }

    async fn remove(&self, id: &str) -> Result<()> {
        let _guard = self.inner.lock().await;
        let mut file = self.read_file()?;
        let before = file.jobs.len();
        file.jobs.retain(|j| j.id != id);
        if file.jobs.len() == before {
            return Err(CronError::NotFound(id.to_string()));
        }
        self.write_file(&file)
    }

    async fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let _guard = self.inner.lock().await;
        let mut file = self.read_file()?;
        let job = file
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| CronError::NotFound(id.to_string()))?;
        job.enabled = enabled;
        self.write_file(&file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::Target;
    use tempfile::tempdir;

    fn mk_job(expr: &str, cmd: &str) -> CronJob {
        CronJob::new(
            expr,
            Target::Slash {
                command: cmd.into(),
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let store = FileCronStore::new(path.clone());

        assert!(store.list().await.unwrap().is_empty());

        let a = mk_job("0 9 * * *", "/a");
        let b = mk_job("*/5 * * * *", "/b");
        store.insert(a.clone()).await.unwrap();
        store.insert(b.clone()).await.unwrap();

        let listed = store.list().await.unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|j| j.id == a.id));
        assert!(listed.iter().any(|j| j.id == b.id));
    }

    #[tokio::test]
    async fn insert_replaces_by_id() {
        let dir = tempdir().unwrap();
        let store = FileCronStore::new(dir.path().join("jobs.json"));
        let mut j = mk_job("0 9 * * *", "/a");
        store.insert(j.clone()).await.unwrap();
        j.expression = "*/10 * * * *".to_string();
        store.insert(j.clone()).await.unwrap();
        let listed = store.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].expression, "*/10 * * * *");
    }

    #[tokio::test]
    async fn update_known_only() {
        let dir = tempdir().unwrap();
        let store = FileCronStore::new(dir.path().join("jobs.json"));
        let j = mk_job("0 9 * * *", "/a");
        let r = store.update(j).await;
        assert!(matches!(r, Err(CronError::NotFound(_))));
    }

    #[tokio::test]
    async fn remove_and_enable() {
        let dir = tempdir().unwrap();
        let store = FileCronStore::new(dir.path().join("jobs.json"));
        let j = mk_job("0 9 * * *", "/a");
        store.insert(j.clone()).await.unwrap();

        store.set_enabled(&j.id, false).await.unwrap();
        let listed = store.list().await.unwrap();
        assert!(!listed[0].enabled);

        store.set_enabled(&j.id, true).await.unwrap();
        let listed = store.list().await.unwrap();
        assert!(listed[0].enabled);

        store.remove(&j.id).await.unwrap();
        assert!(store.list().await.unwrap().is_empty());

        let r = store.remove(&j.id).await;
        assert!(matches!(r, Err(CronError::NotFound(_))));
    }

    /// Desktop-app jobs.json has a completely different schema — entries have
    /// `name`, `schedule`, `state`, `prompt`, etc. but no `target`,
    /// `created_at`. The store must skip them silently rather than failing the
    /// whole `tick_once_with_history` call.
    #[tokio::test]
    async fn desktop_app_schema_entries_are_skipped_gracefully() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");

        // One Desktop-app-style entry (no `target`, no `created_at`), followed
        // by one valid engine-side entry.
        let valid = mk_job("*/5 * * * *", "/ping");
        let raw = serde_json::json!({
            "jobs": [
                {
                    "id": "desktop-entry-1",
                    "name": "GitHub Grabber",
                    "schedule": "00 09 * * *",
                    "state": "scheduled",
                    "enabled": true,
                    "prompt": "some long prompt"
                },
                serde_json::to_value(&valid).unwrap()
            ]
        });
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();

        let store = FileCronStore::new(path);
        let listed = store.list().await.unwrap();
        // Desktop entry skipped; valid engine entry retained.
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, valid.id);
    }

    #[tokio::test]
    async fn empty_or_missing_file_reads_as_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("jobs.json");
        let store = FileCronStore::new(path);
        // Missing file ok.
        assert!(store.list().await.unwrap().is_empty());

        // Empty file ok.
        let dir2 = tempdir().unwrap();
        let path2 = dir2.path().join("jobs.json");
        std::fs::write(&path2, b"").unwrap();
        let store2 = FileCronStore::new(path2);
        assert!(store2.list().await.unwrap().is_empty());
    }

    // ----- M-19: integrity protection of jobs.json -----

    /// Engine-written jobs carry a valid integrity tag and ARE eligible for
    /// auto-fire via `list_for_run`.
    #[tokio::test]
    async fn engine_written_jobs_are_trusted_for_run() {
        let dir = tempdir().unwrap();
        let store = FileCronStore::new(dir.path().join("jobs.json"));
        let j = mk_job("0 9 * * *", "/a");
        store.insert(j.clone()).await.unwrap();

        let runnable = store.list_for_run().await.unwrap();
        assert_eq!(runnable.len(), 1, "engine-stamped job must be runnable");
        assert_eq!(runnable[0].id, j.id);
    }

    /// A directly-written `jobs.json` with no integrity tag (legacy / Desktop
    /// app / fresh attacker file) is visible via `list` but WITHHELD from
    /// auto-fire via `list_for_run`.
    #[tokio::test]
    async fn untagged_jobs_are_withheld_from_auto_fire() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let attacker = mk_job("*/1 * * * *", "/exfil");
        let raw = serde_json::json!({ "jobs": [ serde_json::to_value(&attacker).unwrap() ] });
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();

        let store = FileCronStore::new(path);
        // Visible for diagnostics...
        assert_eq!(store.list().await.unwrap().len(), 1);
        // ...but never auto-fired.
        assert!(
            store.list_for_run().await.unwrap().is_empty(),
            "untagged (non-engine-authored) jobs must not auto-fire"
        );
    }

    /// Tampering with an engine-stamped file (editing a job after the tag was
    /// written) invalidates the tag; ALL jobs are withheld from auto-fire.
    #[tokio::test]
    async fn tampered_jobs_file_is_rejected_for_run() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let store = FileCronStore::new(path.clone());

        // Engine writes a legit job (valid tag stamped).
        let legit = mk_job("0 9 * * *", "/legit");
        store.insert(legit.clone()).await.unwrap();
        assert_eq!(store.list_for_run().await.unwrap().len(), 1);

        // Attacker edits the on-disk job content but leaves the stale tag in
        // place (they don't possess the keyed-MAC algorithm output for the new
        // content). Simulate by rewriting the `jobs` array while keeping the
        // file's existing `integrity` string.
        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let stale_tag = on_disk["integrity"].clone();
        let evil = mk_job("*/1 * * * *", "/pwned");
        let tampered = serde_json::json!({
            "jobs": [ serde_json::to_value(&evil).unwrap() ],
            "integrity": stale_tag,
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();

        // The mismatched tag means NO jobs are eligible for auto-fire.
        assert!(
            store.list_for_run().await.unwrap().is_empty(),
            "tampered jobs.json must yield zero runnable jobs"
        );
    }

    #[test]
    fn keyed_hash_is_key_dependent_and_stable() {
        let data = b"the same canonical jobs payload";
        let a = keyed_hash_hex(b"key-one", data);
        let b = keyed_hash_hex(b"key-two", data);
        assert_ne!(a, b, "different keys must yield different tags");
        assert_eq!(
            a,
            keyed_hash_hex(b"key-one", data),
            "same key+data must be stable"
        );
    }

    #[test]
    fn constant_time_eq_basic() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
