//! Project-scoped approval allowlist (FerroxLabs/wayland#305 c2).
//!
//! # The problem this closes
//!
//! A network-exposed engine gates every mutating tool call by default, and
//! that is the right default: the API key must not be root-equivalent. But a
//! host that works in ONE checkout all day answers the SAME gate for the same
//! directory dozens of times a session — the symptom reported on #305/#287
//! ("per-command confirmations taking one to two hours for a directory
//! listing"). The two postures the server had were "gate everything" and
//! `--allow-all-tools` (root-equivalent, process-wide, set at launch). There
//! was nothing in between and nothing an operator could change while running.
//!
//! This is the in-between: an operator-managed list of PROJECT DIRECTORIES,
//! each individually `enabled`, reachable over REST. A session created with a
//! `cwd` under an ENABLED entry auto-resolves its approval gates; a session
//! under a listed-but-DISABLED entry still gates exactly as before.
//!
//! # Why an unlisted directory is REFUSED rather than gated
//!
//! `cwd` is client-supplied, and it is not merely a label: it becomes the
//! working directory the session's engine is built with. If any string were
//! accepted, a caller holding the API key could point a session at any path on
//! the host — a real authority expansion, wearing the clothes of a hint. So the
//! allowlist is the ONLY source of reachable directories: a `cwd` that no entry
//! covers is refused at `session/create`.
//!
//! That makes the empty allowlist (the default, and every pre-#305 deployment)
//! fail closed: no `cwd` is accepted at all, and a request that omits `cwd`
//! behaves exactly as it did before this module existed — the server's own
//! launch directory, gated.
//!
//! # Containment is component-wise, never a string prefix
//!
//! [`Path::starts_with`] compares whole components, so `/home/me/proj` does
//! NOT cover `/home/me/project-x`. A `str::starts_with` would, and that is the
//! classic way an allowlist grants a directory nobody added. Both the entry
//! path and the queried `cwd` are additionally required to be absolute and free
//! of `.`/`..` components, so no lexical trick (`/allowed/../etc`) can walk out
//! of a covered tree before the comparison happens.
//!
//! SYMLINKS are deliberately NOT resolved here. A symlink inside an approved
//! tree pointing outside it is a filesystem-containment question owned by the
//! session's `WorkspacePolicy`, which sees every path the tools actually touch;
//! re-deciding it from this list would put two authorities on one question and
//! let them disagree.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;

/// Why a path was refused by the allowlist.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AllowlistError {
    /// The path is not absolute, or carries a `.` / `..` component.
    #[error(
        "project path {0:?} must be an absolute, already-normalized path \
         (no '.' or '..' components)"
    )]
    NotNormalizedAbsolute(String),
    /// The allowlist file could not be read or written.
    #[error("project allowlist io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The allowlist file exists and does not parse.
    #[error("project allowlist at {path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// One project directory the operator has listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProjectEntry {
    /// Stable, path-derived id. Usable as a URL path segment (hex), so a
    /// DELETE does not have to carry a filesystem path in its route.
    pub id: String,
    /// The absolute project root.
    #[schema(value_type = String)]
    pub path: PathBuf,
    /// `true` — sessions under this root auto-resolve their approval gates.
    /// `false` — the root is known and reachable, and still gates every call.
    pub enabled: bool,
}

/// `GET /v1/approvals/projects` body.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProjectListResponse {
    pub projects: Vec<ProjectEntry>,
}

/// `PUT /v1/approvals/projects` body.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectUpsertRequest {
    /// Absolute, already-normalized project root.
    pub path: String,
    /// Whether sessions under it auto-resolve approvals. Defaults to `false`
    /// so adding an entry is never itself the act that grants authority.
    #[serde(default)]
    pub enabled: bool,
}

/// The operator's project allowlist.
///
/// In-memory by default; [`Self::backed_by`] additionally persists every
/// mutation, so the grants survive the Core restart that a Win/WSL host cannot
/// avoid (#305's whole subject).
#[derive(Debug)]
pub struct ProjectAllowlist {
    entries: RwLock<Vec<ProjectEntry>>,
    file: Option<PathBuf>,
}

impl Default for ProjectAllowlist {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectAllowlist {
    /// An empty, non-persisted allowlist. Nothing is auto-approved and no
    /// `cwd` is accepted — the pre-#305 posture.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            file: None,
        }
    }

    /// Load from `path` (absent file = empty list) and persist every later
    /// mutation back to it.
    ///
    /// A file that EXISTS and does not parse is an error, never an empty list.
    /// Silently starting empty would hand the operator the fail-closed posture
    /// while their file says otherwise, and the only symptom would be gates
    /// they thought they had turned off — indistinguishable from the bug this
    /// module exists to fix.
    pub fn backed_by(path: impl Into<PathBuf>) -> Result<Self, AllowlistError> {
        let path = path.into();
        let entries = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Vec<ProjectEntry>>(&bytes).map_err(|source| {
                AllowlistError::Malformed {
                    path: path.clone(),
                    source,
                }
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(source) => {
                return Err(AllowlistError::Io {
                    path: path.clone(),
                    source,
                });
            }
        };
        Ok(Self {
            entries: RwLock::new(entries),
            file: Some(path),
        })
    }

    /// Every listed entry, ordered by path so two reads agree.
    pub async fn list(&self) -> Vec<ProjectEntry> {
        let mut out = self.entries.read().await.clone();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// Add `path`, or flip an existing entry's `enabled`. Returns the entry.
    pub async fn upsert(&self, path: &str, enabled: bool) -> Result<ProjectEntry, AllowlistError> {
        let root = normalized_absolute(path)?;
        let entry = ProjectEntry {
            id: entry_id(&root),
            path: root,
            enabled,
        };
        {
            let mut guard = self.entries.write().await;
            match guard.iter_mut().find(|e| e.id == entry.id) {
                Some(existing) => existing.enabled = enabled,
                None => guard.push(entry.clone()),
            }
        }
        self.persist().await?;
        Ok(entry)
    }

    /// Remove one entry by id. `false` means no such id (the caller's DELETE
    /// is then a clean 404, and a repeat of a successful DELETE is too).
    pub async fn remove(&self, id: &str) -> Result<bool, AllowlistError> {
        let removed = {
            let mut guard = self.entries.write().await;
            let before = guard.len();
            guard.retain(|e| e.id != id);
            guard.len() != before
        };
        if removed {
            self.persist().await?;
        }
        Ok(removed)
    }

    /// The LONGEST listed entry covering `cwd`, enabled or not.
    ///
    /// Longest wins so a nested entry can disable auto-approval for one
    /// subtree of an otherwise-enabled project: the more specific statement is
    /// the operator's more recent intent about that path.
    pub async fn covering(&self, cwd: &str) -> Result<Option<ProjectEntry>, AllowlistError> {
        let cwd = normalized_absolute(cwd)?;
        let guard = self.entries.read().await;
        Ok(guard
            .iter()
            .filter(|e| cwd.starts_with(&e.path))
            .max_by_key(|e| e.path.components().count())
            .cloned())
    }

    /// Whether the list has no entries — the fail-closed default state, which
    /// an operator surface should be able to state rather than imply.
    pub async fn is_empty(&self) -> bool {
        self.entries.read().await.is_empty()
    }

    async fn persist(&self) -> Result<(), AllowlistError> {
        let Some(path) = &self.file else {
            return Ok(());
        };
        let snapshot = self.list().await;
        let body =
            serde_json::to_vec_pretty(&snapshot).map_err(|source| AllowlistError::Malformed {
                path: path.clone(),
                source,
            })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| AllowlistError::Io {
                path: path.clone(),
                source,
            })?;
        }
        // `atomic_write` (tempfile + fsync + rename) so a crash mid-write
        // cannot leave a half-file that `backed_by` would then refuse to load,
        // locking the operator out of their own allowlist.
        wcore_config::atomic_io::atomic_write(path, &body).map_err(|source| AllowlistError::Io {
            path: path.clone(),
            source,
        })
    }
}

/// Reject anything that is not an absolute, already-normalized path.
fn normalized_absolute(path: &str) -> Result<PathBuf, AllowlistError> {
    let candidate = PathBuf::from(path);
    let normalized = candidate.is_absolute()
        && !candidate
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir));
    if normalized {
        Ok(candidate)
    } else {
        Err(AllowlistError::NotNormalizedAbsolute(path.to_string()))
    }
}

/// Stable id for a project root: FNV-1a 64 of the path's byte representation,
/// hex-encoded.
///
/// Path-DERIVED rather than random so the same directory keeps its id across
/// restarts and across two hosts talking to the same Core — a Desktop that
/// cached an id does not have to re-read the list to delete an entry. It is a
/// naming scheme, not a security boundary: `upsert` keys on it, so a collision
/// would surface as a refused duplicate rather than as one project silently
/// inheriting another's grant.
fn entry_id(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Platform-correct absolute path for a test fixture.
    fn abs(rest: &str) -> String {
        if cfg!(windows) {
            format!("C:\\{}", rest.replace('/', "\\"))
        } else {
            format!("/{rest}")
        }
    }

    #[tokio::test]
    async fn empty_allowlist_covers_nothing() {
        let list = ProjectAllowlist::new();
        assert!(list.is_empty().await);
        assert!(
            list.covering(&abs("home/me/proj")).await.unwrap().is_none(),
            "the default allowlist must cover no directory at all"
        );
    }

    /// The defect a `str::starts_with` implementation would have: a sibling
    /// whose name merely begins with an approved root's name.
    #[tokio::test]
    async fn a_sibling_sharing_a_name_prefix_is_not_covered() {
        let list = ProjectAllowlist::new();
        list.upsert(&abs("home/me/proj"), true).await.unwrap();

        assert!(
            list.covering(&abs("home/me/proj/src"))
                .await
                .unwrap()
                .is_some(),
            "a real child of the approved root must be covered"
        );
        assert!(
            list.covering(&abs("home/me/project-x"))
                .await
                .unwrap()
                .is_none(),
            "'project-x' merely shares a NAME PREFIX with 'proj'; a string-prefix \
             containment test would grant it an approval nobody added"
        );
    }

    #[tokio::test]
    async fn a_relative_or_dotdot_path_is_refused_on_both_sides() {
        let list = ProjectAllowlist::new();
        assert!(list.upsert("relative/path", true).await.is_err());
        assert!(list.upsert(&abs("home/me/../etc"), true).await.is_err());

        list.upsert(&abs("home/me/proj"), true).await.unwrap();
        assert!(
            list.covering(&abs("home/me/proj/../../etc")).await.is_err(),
            "an un-normalized cwd must be refused, not lexically 'covered'"
        );
    }

    #[tokio::test]
    async fn upsert_flips_enabled_in_place_and_keeps_one_entry() {
        let list = ProjectAllowlist::new();
        let first = list.upsert(&abs("srv/app"), false).await.unwrap();
        let second = list.upsert(&abs("srv/app"), true).await.unwrap();
        assert_eq!(first.id, second.id, "the id is path-derived and stable");
        assert_eq!(list.list().await.len(), 1);
        assert!(list.list().await[0].enabled);
    }

    #[tokio::test]
    async fn the_longest_covering_entry_wins() {
        let list = ProjectAllowlist::new();
        list.upsert(&abs("srv"), true).await.unwrap();
        list.upsert(&abs("srv/app/vendor"), false).await.unwrap();

        let outer = list.covering(&abs("srv/app/src")).await.unwrap().unwrap();
        assert!(outer.enabled, "the only covering entry is the enabled root");

        let inner = list
            .covering(&abs("srv/app/vendor/dep"))
            .await
            .unwrap()
            .unwrap();
        assert!(
            !inner.enabled,
            "the more specific entry decides; a nested disable must not be \
             overridden by the enabled parent"
        );
    }

    #[tokio::test]
    async fn remove_is_idempotent() {
        let list = ProjectAllowlist::new();
        let entry = list.upsert(&abs("srv/app"), true).await.unwrap();
        assert!(list.remove(&entry.id).await.unwrap());
        assert!(
            !list.remove(&entry.id).await.unwrap(),
            "a repeated delete reports 'nothing removed', never an error"
        );
    }

    #[tokio::test]
    async fn a_backed_allowlist_survives_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nested").join("acp-projects.json");

        let first = ProjectAllowlist::backed_by(&file).unwrap();
        first.upsert(&abs("srv/app"), true).await.unwrap();

        let reloaded = ProjectAllowlist::backed_by(&file).unwrap();
        let covering = reloaded.covering(&abs("srv/app/src")).await.unwrap();
        assert!(
            covering.is_some_and(|e| e.enabled),
            "a grant must survive the Core restart that #305 is about"
        );
    }

    #[tokio::test]
    async fn a_malformed_file_is_an_error_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("acp-projects.json");
        std::fs::write(&file, b"{ not json").unwrap();
        assert!(
            matches!(
                ProjectAllowlist::backed_by(&file),
                Err(AllowlistError::Malformed { .. })
            ),
            "starting empty would silently give the operator a different \
             permission posture than the one their file states"
        );
    }
}
