//! User-authored corrections to the user model — the layer inference
//! cannot overwrite.
//!
//! # Why this is a separate store rather than a field on `UserBrief`
//!
//! Everything else in this crate is *inferred*: `LocalBackend::observe`
//! EMA-folds a style fingerprint on every turn, and the P5 partition's
//! `UserModelInferencer` re-derives and overwrites its whole key set at every
//! session end. A correction written into either of those structures is a
//! correction with a half-life.
//!
//! So corrections live in their own type, in their own file, reached through
//! their own API, with **no write path from any inference code**. That is not
//! a convention this module asks callers to respect — `CorrectionStore` is not
//! reachable from `UserModelBackend::observe` at all, so the clobber is
//! structurally impossible rather than merely discouraged.
//!
//! # Precedence
//!
//! A correction does not sit *alongside* the inferred value in the prompt; it
//! *replaces* it. [`crate::correction::Corrections::suppresses`] is what
//! `render_user_context_block` consults to drop the inferred line, so the model
//! sees exactly one value for a corrected subject. Rendering both would hand the
//! model a contradiction and let it pick, which is not user control.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::UserModelError;

/// One user-authored statement about themselves.
///
/// `key` is a dotted path naming what is being corrected (`name`, `summary`,
/// `expertise.rust`, `tags.editor`, `style`, or any free-form label). `value`
/// is stored verbatim — this layer does not parse or validate the user's own
/// words about themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserCorrection {
    /// Normalised dotted path. See [`normalise_key`].
    pub key: String,
    /// The user's value, verbatim.
    pub value: String,
    /// Unix epoch seconds the correction was made.
    #[serde(default)]
    pub ts_secs: i64,
}

/// Normalise a correction key so `Expertise.Rust` and `expertise.rust` are the
/// same key. Trims, lowercases, and collapses internal whitespace.
#[must_use]
pub fn normalise_key(key: &str) -> String {
    key.trim().to_lowercase().split_whitespace().collect()
}

/// The set of corrections for one user, in stable key order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Corrections {
    entries: BTreeMap<String, UserCorrection>,
}

impl Corrections {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// All corrections in stable key order.
    pub fn iter(&self) -> impl Iterator<Item = &UserCorrection> {
        self.entries.values()
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&UserCorrection> {
        self.entries.get(&normalise_key(key))
    }

    /// Whether a correction covers `key`, meaning the inferred value for that
    /// key must NOT be rendered.
    ///
    /// A whole-subject correction subsumes its children: correcting `expertise`
    /// suppresses every `expertise.<domain>` line, and correcting `style`
    /// suppresses the whole inferred style axis line. The reverse is not true —
    /// correcting `expertise.rust` leaves `expertise.python` inferred.
    #[must_use]
    pub fn suppresses(&self, key: &str) -> bool {
        let key = normalise_key(key);
        if self.entries.contains_key(&key) {
            return true;
        }
        // Any ancestor prefix of `key` that is itself corrected suppresses it.
        let mut cursor = key.as_str();
        while let Some(idx) = cursor.rfind('.') {
            cursor = &cursor[..idx];
            if self.entries.contains_key(cursor) {
                return true;
            }
        }
        false
    }

    fn upsert(&mut self, correction: UserCorrection) -> Option<UserCorrection> {
        self.entries.insert(correction.key.clone(), correction)
    }

    fn remove(&mut self, key: &str) -> Option<UserCorrection> {
        self.entries.remove(&normalise_key(key))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DiskState {
    users: HashMap<String, Corrections>,
}

/// Persistent store of user-authored corrections.
///
/// Persists to its own JSON file (`user-corrections.json`), deliberately NOT
/// the `user-model.json` that `LocalBackend` rewrites on every observation. A
/// shared file would put user-authored content inside a document an inference
/// loop overwrites wholesale.
#[derive(Clone)]
pub struct CorrectionStore {
    inner: Arc<RwLock<HashMap<String, Corrections>>>,
    persist_path: Option<PathBuf>,
}

impl std::fmt::Debug for CorrectionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CorrectionStore")
            .field("persist_path", &self.persist_path)
            .finish_non_exhaustive()
    }
}

impl CorrectionStore {
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            persist_path: None,
        }
    }

    /// Open (or create) a store backed by `path`.
    ///
    /// Unlike `LocalBackend::with_persistence`, a corrupt or unreadable file is
    /// an **error**, not a silent empty start. Losing inferred state costs a few
    /// turns of re-observation; silently discarding what a user explicitly told
    /// us about themselves and then telling them the correction is applied is
    /// the failure this whole module exists to prevent.
    pub fn with_persistence(path: impl Into<PathBuf>) -> Result<Self, UserModelError> {
        let path = path.into();
        let mut map = HashMap::new();
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            let state: DiskState = serde_json::from_slice(&bytes)?;
            map = state.users;
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(map)),
            persist_path: Some(path),
        })
    }

    /// Record a correction. Returns the previous correction for that key, if any,
    /// so a caller can report what actually changed rather than echoing the
    /// request back.
    ///
    /// Errors on persistence failure — see [`Self::persist`]. A correction the
    /// user was told was saved, which did not reach disk, would not survive the
    /// session end this layer exists to survive.
    pub async fn correct(
        &self,
        user_id: &str,
        key: &str,
        value: &str,
        ts_secs: i64,
    ) -> Result<Option<UserCorrection>, UserModelError> {
        let key = normalise_key(key);
        if key.is_empty() {
            return Err(UserModelError::Invalid(
                "correction key must not be empty".to_string(),
            ));
        }
        if value.trim().is_empty() {
            return Err(UserModelError::Invalid(
                "correction value must not be empty; use `forget` to remove a correction"
                    .to_string(),
            ));
        }
        let previous = {
            let mut guard = self.inner.write().await;
            let bucket = guard.entry(user_id.to_string()).or_default();
            bucket.upsert(UserCorrection {
                key,
                value: value.trim().to_string(),
                ts_secs,
            })
        };
        self.persist().await?;
        Ok(previous)
    }

    /// Drop a correction, returning it. `Ok(None)` means there was nothing to
    /// drop — callers must report that as a miss, never as a success.
    pub async fn forget(
        &self,
        user_id: &str,
        key: &str,
    ) -> Result<Option<UserCorrection>, UserModelError> {
        let removed = {
            let mut guard = self.inner.write().await;
            match guard.get_mut(user_id) {
                Some(bucket) => bucket.remove(key),
                None => None,
            }
        };
        if removed.is_some() {
            self.persist().await?;
        }
        Ok(removed)
    }

    /// All corrections for `user_id`, in stable key order.
    pub async fn corrections(&self, user_id: &str) -> Corrections {
        self.inner
            .read()
            .await
            .get(user_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Persist synchronously via temp-file + rename.
    ///
    /// Failures **propagate** rather than being logged and swallowed
    /// (`LocalBackend::persist` swallows, which is right for inferred state and
    /// wrong for this).
    async fn persist(&self) -> Result<(), UserModelError> {
        let Some(path) = self.persist_path.clone() else {
            return Ok(());
        };
        let snapshot = self.inner.read().await.clone();
        let bytes = serde_json::to_vec_pretty(&DiskState { users: snapshot })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &bytes)?;
        if std::fs::rename(&tmp, &path).is_err() {
            // Cross-filesystem rename fallback.
            std::fs::write(&path, &bytes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn normalise_key_is_case_and_space_insensitive() {
        assert_eq!(normalise_key("  Expertise.Rust "), "expertise.rust");
        assert_eq!(normalise_key("expertise . rust"), "expertise.rust");
    }

    #[tokio::test]
    async fn correction_round_trips_and_reports_previous() {
        let s = CorrectionStore::in_memory();
        assert!(
            s.correct("u", "expertise.rust", "expert", 10)
                .await
                .unwrap()
                .is_none(),
            "first write has no previous"
        );
        let prev = s
            .correct("u", "Expertise.Rust", "novice", 20)
            .await
            .unwrap()
            .expect("second write reports the first");
        assert_eq!(prev.value, "expert");
        let c = s.corrections("u").await;
        assert_eq!(c.len(), 1, "case variants must be ONE key, not two");
        assert_eq!(c.get("expertise.rust").unwrap().value, "novice");
    }

    #[tokio::test]
    async fn empty_value_is_refused_rather_than_stored() {
        let s = CorrectionStore::in_memory();
        assert!(s.correct("u", "name", "   ", 0).await.is_err());
        assert!(s.correct("u", "  ", "x", 0).await.is_err());
        assert!(s.corrections("u").await.is_empty());
    }

    #[tokio::test]
    async fn forget_reports_miss_rather_than_success() {
        let s = CorrectionStore::in_memory();
        assert!(
            s.forget("u", "nope").await.unwrap().is_none(),
            "a miss must be observable"
        );
        s.correct("u", "name", "Sean", 1).await.unwrap();
        assert_eq!(s.forget("u", "NAME").await.unwrap().unwrap().value, "Sean");
        assert!(s.corrections("u").await.is_empty());
    }

    #[tokio::test]
    async fn suppression_covers_descendants_but_not_siblings() {
        let s = CorrectionStore::in_memory();
        s.correct("u", "expertise", "all novice", 1).await.unwrap();
        s.correct("u", "tags.editor", "helix", 1).await.unwrap();
        let c = s.corrections("u").await;
        assert!(c.suppresses("expertise"));
        assert!(
            c.suppresses("expertise.rust"),
            "a whole-subject correction must suppress its children"
        );
        assert!(c.suppresses("tags.editor"));
        assert!(
            !c.suppresses("tags.shell"),
            "a sibling key must stay inferred"
        );
        assert!(!c.suppresses("name"));
    }

    #[tokio::test]
    async fn persistence_survives_reopen() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("user-corrections.json");
        let s = CorrectionStore::with_persistence(&path).unwrap();
        s.correct("u", "name", "Sean", 42).await.unwrap();
        let reopened = CorrectionStore::with_persistence(&path).unwrap();
        let c = reopened.corrections("u").await;
        assert_eq!(c.get("name").unwrap().value, "Sean");
        assert_eq!(c.get("name").unwrap().ts_secs, 42);
    }

    #[tokio::test]
    async fn corrupt_file_errors_rather_than_silently_starting_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("user-corrections.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(
            CorrectionStore::with_persistence(&path).is_err(),
            "silently discarding user-authored content is the failure this module prevents"
        );
    }

    /// The structural claim this module rests on: an inference fold cannot
    /// reach a correction. `LocalBackend::observe` writes only its own record;
    /// the correction store is a different type behind a different path.
    #[tokio::test]
    async fn observation_folding_cannot_touch_a_correction() {
        use crate::UserModelBackend;
        use crate::observation::Observation;

        let tmp = TempDir::new().unwrap();
        let backend =
            crate::LocalBackend::with_persistence(tmp.path().join("user-model.json")).unwrap();
        let store =
            CorrectionStore::with_persistence(tmp.path().join("user-corrections.json")).unwrap();
        store.correct("u", "style", "blunt", 1).await.unwrap();

        for i in 0..50 {
            backend
                .observe(
                    "u",
                    Observation {
                        style_fingerprint: Some([0.9, 0.9, 0.9, 0.9]),
                        ts_secs: i,
                        ..Observation::default()
                    },
                )
                .await
                .unwrap();
        }

        assert_eq!(
            store.corrections("u").await.get("style").unwrap().value,
            "blunt",
            "50 observations must not move a user-authored correction"
        );
    }
}
