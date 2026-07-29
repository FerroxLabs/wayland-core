//! Per-item provenance for an import (F26-02).
//!
//! An imported item with no record of where it came from cannot be selectively
//! rolled back, cannot be re-synced without guessing, and cannot be judged when
//! its source is later found to be malicious. Every item this crate imports or
//! quarantines therefore carries a [`Provenance`]: the source tool, the source
//! version where the source declares one, the path relative to the source home,
//! a content digest, and the import time.
//!
//! # Why the digest is domain-separated
//!
//! `wcore_config::workspace_trust::fingerprint_workspace` hashes its executable
//! surface behind a domain prefix (`wayland-workspace-executable-surface-v1\0`)
//! so a digest computed for workspace trust can never be mistaken for one
//! computed elsewhere. [`item_digest`] uses the same idiom under its own
//! prefix, [`PROVENANCE_DOMAIN`], for exactly that reason: the two digests
//! address different questions and must not be confusable.
//!
//! # Why the relative path is normalized
//!
//! The digest binds the bytes to WHERE they came from, and the platform's
//! native rendering of a relative path is not stable across platforms
//! (`skills\a\SKILL.md` on Windows, `skills/a/SKILL.md` elsewhere). Digesting
//! the native rendering would make the same item digest differently on Windows
//! than on Linux, so a corpus exported on one and imported on the other would
//! read as tampered. [`normalize_relative_path`] imposes one rendering before
//! the bytes reach the hasher.

use std::collections::BTreeMap;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Domain separator for every provenance digest. Distinct from the
/// workspace-trust surface prefix by construction.
pub const PROVENANCE_DOMAIN: &[u8] = b"wayland-migrate-item-v1\0";

/// Schema version of an emitted provenance document.
pub const PROVENANCE_SCHEMA: u32 = 1;

/// File name a portable corpus carries its provenance records in.
pub const PROVENANCE_FILE: &str = "PROVENANCE.json";

/// Impose one platform-independent rendering on a source-relative path.
///
/// Backslashes become `/`, a leading `./` is dropped, and a leading `/` is
/// dropped so an accidentally-absolute path cannot digest differently from the
/// same item recorded relatively.
pub fn normalize_relative_path(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let trimmed = unified.trim_start_matches("./").trim_start_matches('/');
    trimmed.to_string()
}

/// Domain-separated content digest binding bytes to their normalized
/// source-relative path.
///
/// The framing (path, NUL, little-endian length, bytes, NUL) mirrors
/// `fingerprint_workspace` so that neither a path nor a body can be shifted
/// into the other's field to produce a colliding digest.
pub fn item_digest(relative_path: &str, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PROVENANCE_DOMAIN);
    let rel = normalize_relative_path(relative_path);
    hasher.update(rel.as_bytes());
    hasher.update([0]);
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    hasher.update([0]);
    format!("{:x}", hasher.finalize())
}

/// Digest a whole directory subtree, path-ordered, under the same domain.
///
/// Used when the imported item is a directory (a skill is a directory carrying
/// `SKILL.md` plus its assets). Entries are sorted by their normalized relative
/// path so two walks of one tree digest identically.
pub fn tree_item_digest(entries: &BTreeMap<String, Vec<u8>>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PROVENANCE_DOMAIN);
    hasher.update(b"tree\0");
    for (rel, bytes) in entries {
        let rel = normalize_relative_path(rel);
        hasher.update(rel.as_bytes());
        hasher.update([0]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

/// Where one imported item came from — **and where it landed**.
///
/// # Why the destination is part of the record
///
/// A provenance document keyed only by the PEER's identity answers "what did
/// this migration read?" but not "where did this file on my disk come from?",
/// and the second question is the one an operator actually asks. After a real
/// import the Wayland skills root holds hundreds of directories that are
/// indistinguishable from the user's own; the on-disk name is
/// [`sanitize_component`](super::content)-ed and may be digest-disambiguated on
/// a collision, so it cannot be reversed back to a peer path by inspection.
///
/// `QuarantineEntry` already got this right — it carries `stored_path` beside
/// its `Provenance`, so a contained payload is traceable in both directions.
/// [`written_path`](Self::written_path) gives the IMPORTED side the same
/// property under one vocabulary, and [`ProvenanceDocument::resolve_path`] is
/// the reverse lookup it makes possible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The peer tool the item was read from, e.g. `hermes` / `openclaw`, or
    /// `wayland-core` for a profile export.
    pub source_tool: String,
    /// The version the SOURCE declares, when it declares one. `None` is an
    /// honest absence, not a placeholder — a peer that records no version must
    /// not be given a fabricated one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    /// Path relative to the source home, normalized (see
    /// [`normalize_relative_path`]).
    pub source_path: String,
    /// Domain-separated digest of the item's content.
    pub digest: String,
    /// RFC 3339 UTC, second precision — when the import recorded this item.
    pub imported_at: String,
    /// Where the bytes landed, relative to the Wayland config dir and
    /// `/`-separated — `skills/<name>` for a live data skill,
    /// `migrate-imported/personas/<n>.md` for a staged persona,
    /// `migrate-quarantine/payloads/<n>` for a contained item.
    ///
    /// `None` only for a record that names no destination at all (a legacy
    /// document written before this field existed). An absent destination is
    /// left absent rather than defaulted to a plausible-looking path: a
    /// fabricated destination is worse than a missing one, because it reads as
    /// evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_path: Option<String>,
    /// Set when this item's bytes were byte-identical to an item already
    /// written in the same run, so [`written_path`](Self::written_path) points
    /// at content another identity wrote.
    ///
    /// Recorded rather than elided because without it a reader of
    /// `written_path` would conclude two peer items each produced their own
    /// copy, and a later selective rollback would delete a directory a second
    /// identity still depends on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deduplicated_with: Option<String>,
}

impl Provenance {
    pub fn new(
        source_tool: impl Into<String>,
        source_version: Option<String>,
        source_path: &str,
        digest: impl Into<String>,
    ) -> Self {
        Self {
            source_tool: source_tool.into(),
            source_version,
            source_path: normalize_relative_path(source_path),
            digest: digest.into(),
            imported_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            written_path: None,
            deduplicated_with: None,
        }
    }

    /// Record WHERE the bytes landed, relative to the Wayland config dir.
    ///
    /// Consuming-self so a call site cannot construct a record, forget the
    /// destination, and still have it look complete: every producer in this
    /// crate now ends in `.landed_at(...)`, which is greppable.
    #[must_use]
    pub fn landed_at(mut self, written_path: &str) -> Self {
        self.written_path = Some(normalize_relative_path(written_path));
        self
    }

    /// Mark this record as pointing at bytes another identity wrote.
    #[must_use]
    pub fn deduplicated_with(mut self, id: impl Into<String>) -> Self {
        self.deduplicated_with = Some(id.into());
        self
    }

    /// Same as [`Self::new`] with an explicit timestamp, so a test can pin the
    /// one field that legitimately varies between runs.
    pub fn with_time(
        source_tool: impl Into<String>,
        source_version: Option<String>,
        source_path: &str,
        digest: impl Into<String>,
        imported_at: impl Into<String>,
    ) -> Self {
        Self {
            source_tool: source_tool.into(),
            source_version,
            source_path: normalize_relative_path(source_path),
            digest: digest.into(),
            imported_at: imported_at.into(),
            written_path: None,
            deduplicated_with: None,
        }
    }
}

/// The provenance document a portable corpus carries.
///
/// Keyed by ITEM IDENTITY — the same key the dry-run plan publishes and that
/// selection addresses — so selection, quarantine, provenance and any later
/// rollback all name one item the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceDocument {
    pub schema: u32,
    pub entries: BTreeMap<String, Provenance>,
}

impl Default for ProvenanceDocument {
    fn default() -> Self {
        Self {
            schema: PROVENANCE_SCHEMA,
            entries: BTreeMap::new(),
        }
    }
}

impl ProvenanceDocument {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: impl Into<String>, provenance: Provenance) {
        self.entries.insert(id.into(), provenance);
    }

    pub fn get(&self, id: &str) -> Option<&Provenance> {
        self.entries.get(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Deterministic rendering — `BTreeMap` key order, so two runs over one
    /// corpus serialize byte-identically.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Answer "where did the artifact at this path come from?".
    ///
    /// `query` is a path relative to the Wayland config dir; an absolute path
    /// under a known home should be made relative by the caller. Matching is
    /// **prefix-by-component**, not substring: a skill is imported as a
    /// DIRECTORY, so the operator's question is nearly always about a file
    /// inside one (`skills/notes/SKILL.md`) rather than about the directory
    /// itself. A raw `starts_with` would additionally match `skills/notes-2`,
    /// which is a different item — and on a real import, disambiguated names
    /// like `notes-<digest>` sit right beside their base name, so that
    /// off-by-one is not hypothetical.
    ///
    /// Returns every matching identity, longest destination first, so a nested
    /// destination beats the ancestor that contains it. More than one match is
    /// possible and is not an error: deduplicated identities deliberately share
    /// one destination, and returning only the first would silently hide that
    /// a second peer item also lives there.
    pub fn resolve_path(&self, query: &str) -> Vec<(&str, &Provenance)> {
        let q = normalize_relative_path(query);
        let mut hits: Vec<(&str, &Provenance)> = self
            .entries
            .iter()
            .filter(|(_, p)| match p.written_path.as_deref() {
                Some(w) => path_covers(w, &q),
                None => false,
            })
            .map(|(id, p)| (id.as_str(), p))
            .collect();
        hits.sort_by(|a, b| {
            let la = a.1.written_path.as_deref().map(str::len).unwrap_or(0);
            let lb = b.1.written_path.as_deref().map(str::len).unwrap_or(0);
            lb.cmp(&la).then_with(|| a.0.cmp(b.0))
        });
        hits
    }

    /// Every record that names no destination.
    ///
    /// Exposed so the honesty property is checkable by a caller rather than
    /// only by inspection: this is the F26-GRADE-H1 shape one level up — a
    /// provenance entry with no `written_path` claims an item was recorded
    /// without saying where it is.
    pub fn without_destination(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|(_, p)| p.written_path.is_none())
            .map(|(id, _)| id.as_str())
            .collect()
    }
}

/// True when `written` is `query` itself or a path-component ancestor of it.
fn path_covers(written: &str, query: &str) -> bool {
    if written == query {
        return true;
    }
    // The ancestor case, guarded on the separator so `a/b` does not cover
    // `a/bc`.
    query.len() > written.len()
        && query.as_bytes()[written.len()] == b'/'
        && query.starts_with(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_stable_across_runs_and_path_renderings() {
        let bytes = b"---\nname: a\n---\nbody" as &[u8];
        let unix = item_digest("skills/a/SKILL.md", bytes);
        let windows = item_digest("skills\\a\\SKILL.md", bytes);
        let dotted = item_digest("./skills/a/SKILL.md", bytes);
        assert_eq!(unix, windows, "platform path rendering must not change it");
        assert_eq!(unix, dotted, "a leading ./ must not change it");
        assert_eq!(unix, item_digest("skills/a/SKILL.md", bytes), "not stable");
        // Positive half: a different path or different bytes DOES change it, so
        // the equalities above are not the equality of a constant.
        assert_ne!(unix, item_digest("skills/b/SKILL.md", bytes));
        assert_ne!(unix, item_digest("skills/a/SKILL.md", b"other"));
    }

    #[test]
    fn digest_is_domain_separated_from_the_workspace_trust_surface() {
        // The framing binds path to body: moving bytes across the boundary must
        // not produce a collision.
        let a = item_digest("ab", b"c");
        let b = item_digest("a", b"bc");
        assert_ne!(a, b, "path/body framing is ambiguous");
        assert!(
            PROVENANCE_DOMAIN != b"wayland-workspace-executable-surface-v1\0",
            "the migrate digest must not reuse the workspace-trust domain"
        );
    }

    #[test]
    fn tree_digest_is_order_independent_and_content_sensitive() {
        let mut a = BTreeMap::new();
        a.insert("SKILL.md".to_string(), b"one".to_vec());
        a.insert("assets/x".to_string(), b"two".to_vec());
        let mut b = BTreeMap::new();
        b.insert("assets/x".to_string(), b"two".to_vec());
        b.insert("SKILL.md".to_string(), b"one".to_vec());
        assert_eq!(tree_item_digest(&a), tree_item_digest(&b));
        let mut c = b.clone();
        c.insert("SKILL.md".to_string(), b"changed".to_vec());
        assert_ne!(tree_item_digest(&a), tree_item_digest(&c));
    }

    #[test]
    fn provenance_document_round_trips_and_orders_by_key() {
        let mut doc = ProvenanceDocument::new();
        doc.insert(
            "skill:zed",
            Provenance::with_time("hermes", None, "skills/zed", "d1", "2026-01-01T00:00:00Z"),
        );
        doc.insert(
            "skill:amy",
            Provenance::with_time(
                "hermes",
                Some("1.2.3".into()),
                "skills\\amy",
                "d2",
                "2026-01-01T00:00:00Z",
            ),
        );
        let json = doc.to_json().unwrap();
        assert_eq!(json, doc.to_json().unwrap(), "rendering must be stable");
        assert!(
            json.find("skill:amy").unwrap() < json.find("skill:zed").unwrap(),
            "entries must render in key order"
        );
        assert!(json.contains("\"source_path\": \"skills/amy\""), "{json}");
        // A version the source did not declare is ABSENT, not invented.
        let back = ProvenanceDocument::from_json(&json).unwrap();
        assert_eq!(back, doc);
        assert_eq!(back.get("skill:zed").unwrap().source_version, None);
        assert_eq!(
            back.get("skill:amy").unwrap().source_version.as_deref(),
            Some("1.2.3")
        );
    }
}
