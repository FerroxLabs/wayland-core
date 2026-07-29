//! 23A-C1: governed **revocation** and **rollback** for auto-drafted skills.
//!
//! # Why this exists
//!
//! The autonomous `SkillDrafter` writes `auto-<sig>/{SKILL.md,manifest.json}` into the
//! user's global skills directory after N successful turns, with **no user action**, and
//! `observability.skills_lifecycle` defaults ON — including on a no-config first run. Before
//! this module there was no product surface that could undo that: the learn loop was
//! write-only. A directory the user owns was mutated and could not be un-mutated.
//!
//! Reversibility is therefore built before any further capability. The ordering matters:
//! a promotion path without a revocation path leaves the user strictly worse off.
//!
//! # Model
//!
//! A revocation is a durable statement of **user intent** ("do not give me this"), not a
//! filesystem delete. A plain delete is meaningless here because the drafter's trigger is
//! designed to recur — the next qualifying streak would silently recreate the artifact and
//! the user's explicit decision would be overridden by an automated process. So a revocation
//! writes a **tombstone** that `SkillDrafter::draft` consults and honours.
//!
//! Everything a revocation removes is retained, byte for byte, so `rollback` can restore the
//! exact prior state. `rollback` also clears the tombstone — otherwise the tombstone would
//! itself become a new irreversible mutation, reintroducing the defect one level up.
//!
//! # Crash safety
//!
//! Steps are ordered so that **no crash can leave the artifact deleted with no tombstone**:
//!
//! ```text
//!   1. snapshot bytes into generations/<id>/payload/   (nothing observable yet)
//!   2. write generations/<id>/revocation.json          (fsync'd via atomic_write)
//!   3. write tombstones/<id>.json                      (fsync'd -- suppression now durable)
//!   4. append journal.jsonl
//!   5. remove the skill directory                      (last, and only now)
//! ```
//!
//! A crash before (3) leaves an unreferenced generation and the artifact intact: the user
//! sees no change, and a re-run redoes the work. A crash between (3) and (5) leaves the
//! tombstone durable with the directory still present; `revoke` is idempotent and completes
//! it. The rejected ordering (remove first, tombstone after) has a window in which the draft
//! is gone with nothing suppressing it, so the next trigger recreates it — i.e. revocation
//! violated by precisely the failure mode governance exists to handle.
//!
//! # Identity
//!
//! A tombstone records **both** the drafter's content signature (read from `manifest.json`)
//! **and** the skill name, and `is_revoked` matches on **either**. Signature alone is the
//! better key — the name is a pure function of it today (`auto-<sig>`), so keying on the
//! derived value would break the moment the naming scheme changed. But `loader.rs` already
//! handles drafts whose `manifest.json` is missing or damaged, and for exactly those the
//! signature is unrecoverable. Recording both dominates either alone and costs one field.
//!
//! A content hash is deliberately **not** the key: any trivial regeneration (reworded body,
//! reordered list, embedded timestamp) yields a different hash and the revoked draft returns.
//! The hash is retained as integrity metadata only.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Subdirectory holding the retained bytes of every revocation.
const GENERATIONS: &str = "generations";
/// Subdirectory holding the active suppression index. One file per live revocation.
const TOMBSTONES: &str = "tombstones";
/// Append-only history. Never rewritten, never truncated, including by `rollback`.
const JOURNAL: &str = "journal.jsonl";
/// Payload subdirectory inside a generation.
const PAYLOAD: &str = "payload";

/// Hard cap on a single snapshot, so a pathological skill directory cannot fill the
/// user's disk during what is supposed to be a *remedial* operation.
const MAX_SNAPSHOT_BYTES: u64 = 32 * 1024 * 1024;
/// Hard cap on snapshot recursion depth.
const MAX_SNAPSHOT_DEPTH: usize = 8;

#[derive(Debug, Error)]
pub enum GovernError {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("skill directory not found: {0}")]
    NotFound(String),

    #[error("no revocation with id '{0}'")]
    NoSuchRevocation(String),

    #[error(
        "cannot roll back '{name}': a directory already exists at {path}. \
         Remove or rename it first -- rollback never overwrites."
    )]
    RestoreTargetOccupied { name: String, path: String },

    #[error("refusing to snapshot {path}: {reason}")]
    RefusedSnapshot { path: String, reason: String },

    #[error("governance root could not be resolved on this platform")]
    NoRoot,
}

fn io_err(path: &Path, source: std::io::Error) -> GovernError {
    GovernError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// A recorded revocation. Serialised into `generations/<id>/revocation.json` and, while the
/// revocation is live, mirrored into `tombstones/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Revocation {
    /// Opaque identifier. Also the generation directory name.
    pub revocation_id: String,
    /// Skill name as it appeared on disk (the directory's file name).
    pub skill_name: String,
    /// Drafter content signature from `manifest.json`, when it was readable.
    /// `None` for drafts with a missing or damaged manifest -- see the module docs on
    /// why `is_revoked` must not depend on this field alone.
    pub signature: Option<String>,
    /// Absolute path the artifact was removed from, and the only path `rollback` restores to.
    pub source_dir: PathBuf,
    /// RFC3339 timestamp.
    pub revoked_at: String,
    /// Number of files retained.
    pub file_count: usize,
    /// Total retained bytes.
    pub byte_count: u64,
}

/// One line of `journal.jsonl`. The journal is append-only: a rollback appends a
/// `RolledBack` event, it does not remove the `Revoked` event that preceded it. History of
/// what the product did to the user's directory is never rewritten.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum JournalEvent {
    Revoked {
        revocation_id: String,
        skill_name: String,
        signature: Option<String>,
        source_dir: PathBuf,
        at: String,
    },
    RolledBack {
        revocation_id: String,
        skill_name: String,
        restored_to: PathBuf,
        at: String,
    },
    /// The drafter declined to recreate a revoked draft. This is the evidence that a
    /// revocation is doing work, as opposed to merely having deleted something once.
    DraftSuppressed {
        skill_name: String,
        signature: Option<String>,
        revocation_id: String,
        at: String,
    },
}

/// Governed store for skill revocations.
///
/// Construct with an explicit root wherever possible. `open_default` resolves the real
/// user-level root and is intended for production call sites only: tests that let a root
/// default to a process-global path race each other and pollute the developer's real
/// directory, which is bug #564 in this same subsystem.
#[derive(Debug, Clone)]
pub struct GovernanceStore {
    root: PathBuf,
}

impl GovernanceStore {
    /// Use an explicit governance root. Nothing is created until first write.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolve the user-level governance root.
    ///
    /// Mirrors `wcore_config::config::wayland_config_dir`'s resolution order so
    /// `WAYLAND_HOME` hermetically sandboxes governance state exactly as it sandboxes every
    /// other file the engine touches -- without which a profile's revocations would leak
    /// across profiles. The final fallback differs deliberately: `dirs::data_dir()` rather
    /// than `dirs::config_dir()`, because retained snapshot bytes are bulk *data*, not
    /// configuration, and on Linux `~/.config` is commonly dotfile-tracked and synced.
    ///
    /// The root is always a **sibling** of the skills directory, never inside it, so
    /// retained bytes can never be mistaken for installed skills by the loader.
    pub fn open_default() -> Result<Self, GovernError> {
        Ok(Self::new(governance_root().ok_or(GovernError::NoRoot)?))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn generations_dir(&self) -> PathBuf {
        self.root.join(GENERATIONS)
    }

    fn tombstones_dir(&self) -> PathBuf {
        self.root.join(TOMBSTONES)
    }

    fn journal_path(&self) -> PathBuf {
        self.root.join(JOURNAL)
    }

    /// Revoke the skill directory at `skill_dir`.
    ///
    /// Retains every byte first, makes the suppression durable second, and removes the
    /// directory last. Idempotent under crash: if a tombstone for this directory already
    /// exists but the directory is still present (a crash between steps 3 and 5), the
    /// existing revocation is completed rather than duplicated.
    pub fn revoke(&self, skill_dir: &Path) -> Result<Revocation, GovernError> {
        if !skill_dir.is_dir() {
            return Err(GovernError::NotFound(skill_dir.display().to_string()));
        }
        let skill_name = skill_dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .ok_or_else(|| GovernError::NotFound(skill_dir.display().to_string()))?;

        // Crash-recovery path: a durable tombstone whose source directory still exists means
        // a previous run died between making suppression durable and removing the artifact.
        // Finish that operation instead of starting a second one, so retries converge.
        if let Some(existing) = self
            .live_revocations()?
            .into_iter()
            .find(|r| r.source_dir == skill_dir)
        {
            remove_dir_all(skill_dir)?;
            return Ok(existing);
        }

        let signature = read_signature(skill_dir);
        let revocation_id = new_revocation_id();
        let generation = self.generations_dir().join(&revocation_id);
        let payload = generation.join(PAYLOAD);

        // ---- 1. snapshot. Nothing is observable to the rest of the product yet. ----
        let (file_count, byte_count) = copy_tree(skill_dir, &payload)?;

        let record = Revocation {
            revocation_id: revocation_id.clone(),
            skill_name: skill_name.clone(),
            signature: signature.clone(),
            source_dir: skill_dir.to_path_buf(),
            revoked_at: now_rfc3339(),
            file_count,
            byte_count,
        };
        let encoded = serde_json::to_vec_pretty(&record)?;

        // ---- 2. the generation's own record ----
        write_atomic(&generation.join("revocation.json"), &encoded)?;

        // ---- 3. the suppression index. AFTER the bytes are retained, BEFORE removal. ----
        let tombstones = self.tombstones_dir();
        create_dir_all(&tombstones)?;
        write_atomic(&tombstones.join(format!("{revocation_id}.json")), &encoded)?;

        // ---- 4. history ----
        self.append_journal(&JournalEvent::Revoked {
            revocation_id: revocation_id.clone(),
            skill_name,
            signature,
            source_dir: skill_dir.to_path_buf(),
            at: record.revoked_at.clone(),
        })?;

        // ---- 5. the destructive step, last ----
        remove_dir_all(skill_dir)?;

        Ok(record)
    }

    /// Restore a revoked skill to the exact directory it was removed from, byte for byte,
    /// and clear its tombstone so the drafter is no longer suppressed for it.
    ///
    /// Refuses rather than overwrites if something already occupies the target: a rollback
    /// that silently clobbered a user's hand-edited skill would be the same class of defect
    /// this module exists to remove.
    pub fn rollback(&self, revocation_id: &str) -> Result<PathBuf, GovernError> {
        let tombstone = self.tombstones_dir().join(format!("{revocation_id}.json"));
        let record: Revocation = match std::fs::read(&tombstone) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(_) => {
                return Err(GovernError::NoSuchRevocation(revocation_id.to_string()));
            }
        };

        if record.source_dir.exists() {
            return Err(GovernError::RestoreTargetOccupied {
                name: record.skill_name.clone(),
                path: record.source_dir.display().to_string(),
            });
        }

        let payload = self.generations_dir().join(revocation_id).join(PAYLOAD);
        if !payload.is_dir() {
            return Err(GovernError::NoSuchRevocation(revocation_id.to_string()));
        }

        // Restore the bytes BEFORE clearing suppression. If this order were reversed a crash
        // in between would leave the draft un-suppressed and absent, so the drafter would
        // recreate it from scratch and the user's retained version would be orphaned.
        copy_tree(&payload, &record.source_dir)?;

        std::fs::remove_file(&tombstone).map_err(|e| io_err(&tombstone, e))?;

        self.append_journal(&JournalEvent::RolledBack {
            revocation_id: revocation_id.to_string(),
            skill_name: record.skill_name.clone(),
            restored_to: record.source_dir.clone(),
            at: now_rfc3339(),
        })?;

        Ok(record.source_dir)
    }

    /// Every revocation currently in force. The generations of rolled-back revocations are
    /// retained on disk but are not listed here, because they no longer suppress anything.
    pub fn live_revocations(&self) -> Result<Vec<Revocation>, GovernError> {
        let dir = self.tombstones_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            // An absent tombstones directory means nothing has ever been revoked. That is a
            // legitimate empty state, not an error.
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(&dir, e)),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            // A torn or hand-mangled tombstone must not make the whole suppression set
            // unreadable -- that would silently un-suppress every other revocation.
            match std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Revocation>(&b).ok())
            {
                Some(r) => out.push(r),
                None => {
                    tracing::warn!(
                        target: "wcore_skills::govern",
                        path = %path.display(),
                        "unreadable tombstone skipped; other revocations remain in force"
                    );
                }
            }
        }
        out.sort_by(|a, b| a.revoked_at.cmp(&b.revoked_at));
        Ok(out)
    }

    /// Is a draft with this name and/or signature currently revoked?
    ///
    /// Matches on **either** key. See the module docs: signature is the better key, but is
    /// unavailable for drafts with a damaged manifest, and name alone breaks if the naming
    /// scheme changes. Either-match covers both.
    pub fn is_revoked(&self, skill_name: &str, signature: Option<&str>) -> bool {
        let live = match self.live_revocations() {
            Ok(l) => l,
            Err(e) => {
                // Fail OPEN on a read error would silently resurrect revoked drafts. Fail
                // CLOSED (report revoked) would block all drafting on one bad file. Neither
                // is safe as a blanket rule, so report not-revoked but make the failure
                // loud: the alternative hides a governance outage entirely.
                tracing::error!(
                    target: "wcore_skills::govern",
                    error = %e,
                    "could not read revocations; treating '{skill_name}' as not revoked"
                );
                return false;
            }
        };
        live.iter().any(|r| {
            r.skill_name == skill_name
                || match (r.signature.as_deref(), signature) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                }
        })
    }

    /// Record that the drafter declined to recreate a revoked draft.
    ///
    /// This is what makes a revocation *provable*: without it, "the draft did not come back"
    /// is indistinguishable from "the drafter never fired", which is the manufactured-green
    /// failure mode of universal denial.
    pub fn record_suppression(
        &self,
        skill_name: &str,
        signature: Option<&str>,
    ) -> Result<(), GovernError> {
        let revocation_id = self
            .live_revocations()?
            .into_iter()
            .find(|r| {
                r.skill_name == skill_name
                    || match (r.signature.as_deref(), signature) {
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    }
            })
            .map(|r| r.revocation_id)
            .unwrap_or_default();
        self.append_journal(&JournalEvent::DraftSuppressed {
            skill_name: skill_name.to_string(),
            signature: signature.map(str::to_string),
            revocation_id,
            at: now_rfc3339(),
        })
    }

    /// Read the append-only journal. Torn trailing records are skipped, not fatal.
    pub fn journal(&self) -> Result<Vec<JournalEvent>, GovernError> {
        let path = self.journal_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(&path, e)),
        };
        Ok(text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect())
    }

    fn append_journal(&self, event: &JournalEvent) -> Result<(), GovernError> {
        use std::io::Write;
        create_dir_all(&self.root)?;
        let path = self.journal_path();
        let mut line = serde_json::to_string(event)?;
        line.push('\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| io_err(&path, e))?;
        f.write_all(line.as_bytes()).map_err(|e| io_err(&path, e))?;
        // Durability matters more than the ~1ms: the journal is the audit record of what the
        // product did to a directory the user owns.
        f.sync_all().map_err(|e| io_err(&path, e))?;
        Ok(())
    }
}

/// Resolve the user-level governance root. See [`GovernanceStore::open_default`].
pub fn governance_root() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("WAYLAND_SKILLS_GOVERNANCE_DIR") {
        return Some(PathBuf::from(explicit));
    }
    if let Ok(wh) = std::env::var("WAYLAND_HOME") {
        return Some(PathBuf::from(wh).join("skills-governance"));
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        return Some(
            PathBuf::from(xdg)
                .join("wayland-core")
                .join("skills-governance"),
        );
    }
    dirs::data_dir().map(|d| d.join("wayland-core").join("skills-governance"))
}

/// Read the drafter's content signature out of a draft's `manifest.json`.
///
/// Returns `None` for a missing, unparseable, or signature-less manifest -- the exact case
/// `loader.rs` already tolerates -- rather than failing the revocation. A draft whose
/// manifest is damaged is still revocable by name.
pub fn read_signature(skill_dir: &Path) -> Option<String> {
    let bytes = std::fs::read(skill_dir.join("manifest.json")).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("signature")?.as_str().map(str::to_string)
}

fn new_revocation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn create_dir_all(p: &Path) -> Result<(), GovernError> {
    std::fs::create_dir_all(p).map_err(|e| io_err(p, e))
}

fn write_atomic(p: &Path, bytes: &[u8]) -> Result<(), GovernError> {
    if let Some(parent) = p.parent() {
        create_dir_all(parent)?;
    }
    wcore_config::atomic_write(p, bytes).map_err(|e| io_err(p, e))
}

fn remove_dir_all(p: &Path) -> Result<(), GovernError> {
    std::fs::remove_dir_all(p).map_err(|e| io_err(p, e))
}

/// Copy a directory tree, refusing symlinks and enforcing size/depth caps.
///
/// Symlinks are refused rather than followed: a symlink inside a skill directory pointing
/// outside it would otherwise let a snapshot copy arbitrary user files into the governance
/// store, and let a rollback write them back out. Returns `(file_count, byte_count)`.
fn copy_tree(from: &Path, to: &Path) -> Result<(usize, u64), GovernError> {
    let mut files = 0usize;
    let mut bytes = 0u64;
    copy_tree_inner(from, to, 0, &mut files, &mut bytes)?;
    Ok((files, bytes))
}

fn copy_tree_inner(
    from: &Path,
    to: &Path,
    depth: usize,
    files: &mut usize,
    bytes: &mut u64,
) -> Result<(), GovernError> {
    if depth > MAX_SNAPSHOT_DEPTH {
        return Err(GovernError::RefusedSnapshot {
            path: from.display().to_string(),
            reason: format!("directory nesting exceeds {MAX_SNAPSHOT_DEPTH} levels"),
        });
    }
    create_dir_all(to)?;
    let entries = std::fs::read_dir(from).map_err(|e| io_err(from, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err(from, e))?;
        let src = entry.path();
        // `symlink_metadata` does NOT follow the link, which is the whole point.
        let meta = std::fs::symlink_metadata(&src).map_err(|e| io_err(&src, e))?;
        if meta.file_type().is_symlink() {
            return Err(GovernError::RefusedSnapshot {
                path: src.display().to_string(),
                reason: "symlinks are not copied; a link could escape the skill directory"
                    .to_string(),
            });
        }
        let dst = to.join(entry.file_name());
        if meta.is_dir() {
            copy_tree_inner(&src, &dst, depth + 1, files, bytes)?;
        } else {
            *bytes = bytes.saturating_add(meta.len());
            if *bytes > MAX_SNAPSHOT_BYTES {
                return Err(GovernError::RefusedSnapshot {
                    path: from.display().to_string(),
                    reason: format!("snapshot exceeds {MAX_SNAPSHOT_BYTES} bytes"),
                });
            }
            std::fs::copy(&src, &dst).map_err(|e| io_err(&src, e))?;
            *files += 1;
        }
    }
    Ok(())
}
