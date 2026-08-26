//! 23A-C1: governed **promotion** for skills.
//!
//! # What promotion is
//!
//! `loader.rs` quarantines every generated draft — `metadata.disable_model_invocation
//! = true` — with a comment saying the quarantine holds "until F23 supplies a governed
//! promotion transaction". This module is that transaction. **Promotion is the act of
//! lifting that quarantine for one specific artifact**, and nothing else: it does not
//! copy files around, it does not edit the skill, and it does not change what the loader
//! discovers. A promoted skill is the same bytes, now visible to the model.
//!
//! That framing is what makes the operation auditable. Because the only thing promotion
//! changes is *reachability*, the whole of it can be recorded in one grant file, and the
//! question "what did the product make model-facing, from where, on whose say-so" has a
//! literal answer on disk.
//!
//! # The binding, and why it is a content digest
//!
//! A grant names a **content digest**, not just a skill name. The loader lifts quarantine
//! only when the bytes on disk still hash to the digest in the grant.
//!
//! This is deliberately the **opposite** key choice from revocation, and the asymmetry is
//! not an inconsistency — the two operations have opposite requirements:
//!
//! - **Revocation keys on name/signature, never content.** The drafter's trigger is
//!   designed to recur, so any trivial regeneration (a reworded body, a reordered list, an
//!   embedded timestamp) yields different bytes. A content-keyed revocation would be
//!   defeated by exactly the process it exists to stop. Revocation must be **loose** so it
//!   survives regeneration.
//! - **Promotion keys on content, never name alone.** A name-keyed grant would survive
//!   *mutation*: promote a reviewed skill, then let anything at all rewrite `SKILL.md`, and
//!   the new content inherits model-facing status that nobody reviewed. Promotion must be
//!   **strict** so it does not survive regeneration.
//!
//! So: revocation is sticky across content changes, promotion is brittle across them, and
//! both are correct. A promoted skill whose bytes change silently reverts to quarantine —
//! it fails **closed**, and `is_promoted` reports the digest mismatch so the reversion is
//! explicable rather than mysterious.
//!
//! # Refusal
//!
//! `promote` consults revocation **first** and refuses a revoked name or signature. This
//! is the resurrection fence. Without it the sequence "user revokes a draft" → "a governed
//! promotion re-materialises it" would hand back, model-facing, the exact artifact the user
//! removed — and the promotion path is the only path with the authority to do that, which
//! is why the fence belongs here rather than downstream.
//!
//! # The evaluation gate
//!
//! Promotion also requires **evidence**: a [`PromotionEvidence`] whose score clears its
//! own threshold. It is a required argument, not an option, so there is no promotion path
//! that forgets to ask — the refusal lives beside the resurrection fence rather than in
//! whichever caller happened to remember it.
//!
//! Governance owns the *rule* ("nothing becomes model-facing without a score that clears
//! its threshold") and deliberately not the *scorer*. `wcore-eval` depends on this crate,
//! so the dependency cannot run the other way; and it should not, because what counts as
//! good enough is an evaluation policy that is expected to change, while the rule is not.
//! `wcore-eval::promotion` is the production producer of this evidence.
//!
//! The evidence is copied into the grant. A grant therefore answers "on whose say-so, over
//! which bytes, **and against what score**" — the score is as much a part of the provenance
//! as the authority is, and a threshold that is later raised does not silently reinterpret
//! grants issued under the old one, because each grant carries the threshold it was judged
//! against.
//!
//! # Crash safety of the install
//!
//! `promote_new` materialises an artifact into the user's global skills directory. That
//! directory is the one this whole criterion exists to protect, and this program has
//! already measured data loss on an interrupted write into a live index. So the install
//! never writes into its final location:
//!
//! ```text
//!   1. build the whole tree in <skills_root>/../.promote-staging/<uuid>/
//!   2. fsync every file, then fsync the staging directory
//!   3. rename(staging, <skills_root>/<name>)      <-- atomic, the only observable step
//!   4. fsync the parent directory
//!   5. write the grant                            <-- last
//! ```
//!
//! Staging lives **outside** the skills tree, not inside it under a hidden name, because
//! `collect_skill_md` does not skip dot-directories: a half-built staging directory holding
//! a `SKILL.md` inside the skills root would be discovered and loaded as a skill. The
//! parent of the skills root is the same filesystem in every layout the product creates, so
//! the rename stays atomic.
//!
//! Step 3 is the only step an observer can see, and `rename(2)` is atomic, so **no kill can
//! leave a partially-written skill directory**. The step ordering also fails closed: a
//! crash between 3 and 5 leaves the artifact installed but *ungranted*, which the loader
//! quarantines. The reverse order would leave a grant pointing at bytes that were never
//! written, i.e. a promotion of nothing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::govern::{
    GovernError, GovernanceStore, JournalEvent, PROMOTIONS, create_dir_all, io_err, now_rfc3339,
    write_atomic,
};

/// Directory name for in-progress installs.
///
/// Intended as a sibling of the skills root. It is **not always** one: `staging_root_for`
/// takes the *parent of the directory being written*, so for a namespaced skill such as the
/// auto-drafter's `skills/auto/auto-<sig>/` it resolves to `skills/.promote-staging` —
/// inside the tree `collect_skill_md` walks. Guaranteeing otherwise is not possible in
/// general: `rename(2)` needs the staging area on the target's filesystem, and skills roots
/// nest arbitrarily via `--add-dir`, `$WAYLAND_HOME` and project roots.
///
/// So discovery is fenced by name instead — `loader::collect_skill_md` skips this directory
/// (F23A-C1-H4). That skip is the load-bearing guarantee; the location is a best effort.
pub(crate) const STAGING: &str = ".promote-staging";

/// Hard cap on a promotion digest walk, mirroring the snapshot caps in `govern.rs`.
const MAX_DIGEST_DEPTH: usize = 8;

/// A governed promotion grant. Serialised to `promotions/<promotion_id>.json`.
///
/// This is the "what was promoted, from where, on whose authority" record that makes the
/// operation checkable after the fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Promotion {
    pub promotion_id: String,
    /// Skill directory name, as the loader will see it.
    pub skill_name: String,
    /// Drafter content signature, when the artifact carried a readable manifest.
    pub signature: Option<String>,
    /// The reviewed P4 procedure this grant was issued against, when promotion came
    /// through the procedure path. `None` for a direct promotion of an on-disk artifact.
    pub procedure_id: Option<String>,
    /// Where the artifact lives. The grant applies to this path and no other.
    pub target_dir: PathBuf,
    /// `sha256:<hex>` over the artifact's canonical tree bytes. **The binding.**
    pub content_digest: String,
    /// Who authorised this. Free-form because the product has no identity system; it
    /// records the invoking surface so the journal can distinguish an explicit user
    /// command from anything automated.
    pub authority: String,
    pub promoted_at: String,
    pub file_count: usize,
    pub byte_count: u64,
    /// The evaluation this grant was issued against.
    ///
    /// `Option` for one reason only: grants written before the gate existed have no
    /// evidence, and reading them back as "scored 0" would be a fabrication. Every grant
    /// this code writes carries `Some`. A `None` in the wild means "issued before the
    /// product evaluated anything", which is a fact worth being able to say.
    #[serde(default)]
    pub evidence: Option<PromotionEvidence>,
}

/// Machine-checkable evidence that an artifact was evaluated before it was promoted.
///
/// Carries the threshold as well as the score. A grant read a year later has to be
/// interpretable on its own: "0.71" means nothing without the number it had to beat, and
/// looking the threshold up at read time would silently re-judge old grants under new
/// policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromotionEvidence {
    /// Which evaluator produced `score`, so a reader knows whose judgement they inherit.
    pub evaluator: String,
    /// Combined score, in `[0.0, 1.0]`.
    pub score: f64,
    /// The cutoff `score` had to reach.
    pub threshold: f64,
    /// The evaluator's own verdict word, recorded verbatim for the audit trail.
    pub verdict: String,
}

impl PromotionEvidence {
    /// Does this evidence permit promotion?
    ///
    /// Non-finite values refuse. `NaN >= x` is already false, so a NaN score would refuse
    /// anyway; the explicit check is here so a NaN *threshold* — which would otherwise make
    /// every comparison false and look like a scorer bug — refuses for a stated reason.
    pub fn clears(&self) -> bool {
        self.score.is_finite() && self.threshold.is_finite() && self.score >= self.threshold
    }
}

/// Why a promotion was refused. Each variant is a governance decision, not an I/O failure.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// The artifact is revoked. The resurrection fence.
    Revoked { skill_name: String },
    /// Nothing at the requested path.
    NoSuchSkill { skill_name: String },
    /// A promotion already covers these exact bytes.
    AlreadyPromoted {
        skill_name: String,
        promotion_id: String,
    },
    /// The install target is occupied and promotion never overwrites.
    TargetOccupied { path: String },
    /// The artifact was evaluated and did not clear the promotion threshold.
    EvalBelowThreshold {
        skill_name: String,
        evaluator: String,
        score: f64,
        threshold: f64,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Revoked { skill_name } => write!(
                f,
                "refusing to promote '{skill_name}': it is revoked. A revoked skill is a \
                 standing user decision and promotion does not override it. To undo the \
                 revocation, roll it back first (`wayland-core --skills-rollback <id>`), \
                 then promote."
            ),
            Refusal::NoSuchSkill { skill_name } => {
                write!(f, "no skill named '{skill_name}' is installed")
            }
            Refusal::AlreadyPromoted {
                skill_name,
                promotion_id,
            } => write!(
                f,
                "'{skill_name}' is already promoted at these exact bytes (grant {promotion_id})"
            ),
            Refusal::TargetOccupied { path } => write!(
                f,
                "refusing to install over {path}: promotion never overwrites an existing \
                 directory. Remove or rename it first."
            ),
            Refusal::EvalBelowThreshold {
                skill_name,
                evaluator,
                score,
                threshold,
            } => write!(
                f,
                "refusing to promote '{skill_name}': {evaluator} scored it {score:.3}, below \
                 the {threshold:.3} promotion threshold. A generated skill stays data until \
                 it earns model-facing status. Review and repair the artifact, then promote \
                 again -- the score is recomputed from the bytes on disk every time."
            ),
        }
    }
}

/// The outcome of asking whether an artifact is promoted.
///
/// Three states rather than a `bool`, because "not promoted" and "promoted, but the bytes
/// changed since" are different facts and collapsing them is what would make a silent
/// reversion to quarantine look like a bug.
#[derive(Debug, Clone, PartialEq)]
pub enum PromotionState {
    NotPromoted,
    Promoted(Box<Promotion>),
    /// A grant exists for this name but the bytes on disk no longer match it.
    DigestMismatch {
        promotion_id: String,
        granted: String,
        found: String,
    },
}

impl PromotionState {
    /// Quarantine lifts only in the `Promoted` state.
    pub fn lifts_quarantine(&self) -> bool {
        matches!(self, PromotionState::Promoted(_))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PromoteError {
    #[error(transparent)]
    Govern(#[from] GovernError),
    #[error("{0}")]
    Refused(Refusal),
}

impl GovernanceStore {
    fn promotions_dir(&self) -> PathBuf {
        self.root().join(PROMOTIONS)
    }

    /// Every promotion grant on record, whether or not its bytes still match.
    pub fn promotions(&self) -> Result<Vec<Promotion>, GovernError> {
        let dir = self.promotions_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(&dir, e)),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            // A torn grant must not make every other grant unreadable. Skipping one
            // fails CLOSED -- that skill stays quarantined -- which is the safe
            // direction for a file whose whole purpose is granting visibility.
            match std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Promotion>(&b).ok())
            {
                Some(p) => out.push(p),
                None => tracing::warn!(
                    target: "wcore_skills::promote",
                    path = %path.display(),
                    "unreadable promotion grant skipped; that skill stays quarantined"
                ),
            }
        }
        out.sort_by(|a, b| a.promoted_at.cmp(&b.promoted_at));
        Ok(out)
    }

    /// Is the artifact at `skill_dir` promoted **at its current bytes**?
    pub fn promotion_state(&self, skill_dir: &Path) -> Result<PromotionState, GovernError> {
        let name = match skill_dir.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => return Ok(PromotionState::NotPromoted),
        };
        let grants = self.promotions()?;
        let Some(grant) = grants.iter().rev().find(|p| p.skill_name == name) else {
            return Ok(PromotionState::NotPromoted);
        };
        let found = content_digest(skill_dir)?;
        if found == grant.content_digest {
            Ok(PromotionState::Promoted(Box::new(grant.clone())))
        } else {
            Ok(PromotionState::DigestMismatch {
                promotion_id: grant.promotion_id.clone(),
                granted: grant.content_digest.clone(),
                found,
            })
        }
    }

    /// Promote an artifact that is **already installed** on disk.
    ///
    /// Mutates nothing under the user's skills directory: the only write is the grant
    /// itself, in the governance root. So this path has no interruption hazard against the
    /// user's data at all — the worst an interrupted run can do is fail to grant, and an
    /// ungranted skill is a quarantined skill.
    pub fn promote_existing(
        &self,
        skill_dir: &Path,
        procedure_id: Option<&str>,
        authority: &str,
        evidence: &PromotionEvidence,
    ) -> Result<Promotion, PromoteError> {
        if !skill_dir.is_dir() {
            let refusal = Refusal::NoSuchSkill {
                skill_name: skill_dir.display().to_string(),
            };
            self.record_refusal(&skill_dir.display().to_string(), &refusal)?;
            return Err(PromoteError::Refused(refusal));
        }
        let skill_name = skill_dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let signature = crate::govern::read_signature(skill_dir);

        // ---- the resurrection fence, before anything else ----
        if self.is_revoked(&skill_name, signature.as_deref()) {
            let refusal = Refusal::Revoked {
                skill_name: skill_name.clone(),
            };
            self.record_refusal(&skill_name, &refusal)?;
            return Err(PromoteError::Refused(refusal));
        }

        // ---- the evaluation gate ----
        if !evidence.clears() {
            let refusal = Refusal::EvalBelowThreshold {
                skill_name: skill_name.clone(),
                evaluator: evidence.evaluator.clone(),
                score: evidence.score,
                threshold: evidence.threshold,
            };
            self.record_refusal(&skill_name, &refusal)?;
            return Err(PromoteError::Refused(refusal));
        }

        if let PromotionState::Promoted(existing) = self.promotion_state(skill_dir)? {
            let refusal = Refusal::AlreadyPromoted {
                skill_name: skill_name.clone(),
                promotion_id: existing.promotion_id.clone(),
            };
            return Err(PromoteError::Refused(refusal));
        }

        let (file_count, byte_count) = tree_stats(skill_dir)?;
        let grant = Promotion {
            promotion_id: uuid::Uuid::new_v4().to_string(),
            skill_name,
            signature,
            procedure_id: procedure_id.map(str::to_string),
            target_dir: skill_dir.to_path_buf(),
            content_digest: content_digest(skill_dir)?,
            authority: authority.to_string(),
            promoted_at: now_rfc3339(),
            file_count,
            byte_count,
            evidence: Some(evidence.clone()),
        };
        self.write_grant(&grant)?;
        Ok(grant)
    }

    /// Materialise a reviewed artifact into `skills_root/<name>` and promote it.
    ///
    /// See the module docs for the staged-then-renamed install and why the ordering is
    /// what it is. `files` is `(relative path, bytes)`.
    pub fn promote_new(
        &self,
        skills_root: &Path,
        skill_name: &str,
        files: &[(String, Vec<u8>)],
        procedure_id: Option<&str>,
        authority: &str,
        evidence: &PromotionEvidence,
    ) -> Result<Promotion, PromoteError> {
        // ---- the resurrection fence, before any filesystem work ----
        if self.is_revoked(skill_name, None) {
            let refusal = Refusal::Revoked {
                skill_name: skill_name.to_string(),
            };
            self.record_refusal(skill_name, &refusal)?;
            return Err(PromoteError::Refused(refusal));
        }

        // ---- the evaluation gate, still before any filesystem work ----
        if !evidence.clears() {
            let refusal = Refusal::EvalBelowThreshold {
                skill_name: skill_name.to_string(),
                evaluator: evidence.evaluator.clone(),
                score: evidence.score,
                threshold: evidence.threshold,
            };
            self.record_refusal(skill_name, &refusal)?;
            return Err(PromoteError::Refused(refusal));
        }

        let target = skills_root.join(skill_name);
        if target.exists() {
            let refusal = Refusal::TargetOccupied {
                path: target.display().to_string(),
            };
            self.record_refusal(skill_name, &refusal)?;
            return Err(PromoteError::Refused(refusal));
        }

        let staged = self.stage_install(skills_root, files)?;

        // The single observable step. Atomic: the target either does not exist or is
        // the complete tree. There is no third state for a kill to land in.
        create_dir_all(skills_root)?;
        if let Err(e) = std::fs::rename(&staged, &target) {
            // Leave nothing behind on a failed publish.
            let _ = std::fs::remove_dir_all(&staged);
            return Err(PromoteError::Govern(io_err(&target, e)));
        }
        sync_dir(skills_root);

        let (file_count, byte_count) = tree_stats(&target)?;
        let grant = Promotion {
            promotion_id: uuid::Uuid::new_v4().to_string(),
            skill_name: skill_name.to_string(),
            signature: crate::govern::read_signature(&target),
            procedure_id: procedure_id.map(str::to_string),
            target_dir: target.clone(),
            content_digest: content_digest(&target)?,
            authority: authority.to_string(),
            promoted_at: now_rfc3339(),
            file_count,
            byte_count,
            evidence: Some(evidence.clone()),
        };
        self.write_grant(&grant)?;
        Ok(grant)
    }

    /// Build the complete tree under a staging directory outside the skills root, and
    /// fsync it, so the subsequent rename publishes durable bytes.
    fn stage_install(
        &self,
        skills_root: &Path,
        files: &[(String, Vec<u8>)],
    ) -> Result<PathBuf, GovernError> {
        let staging_root = staging_root_for(skills_root);
        create_dir_all(&staging_root)?;
        let staged = staging_root.join(uuid::Uuid::new_v4().to_string());
        create_dir_all(&staged)?;

        for (rel, bytes) in files {
            let dst = resolve_under(&staged, rel).ok_or_else(|| GovernError::RefusedSnapshot {
                path: rel.clone(),
                reason: "artifact path escapes the skill directory".to_string(),
            })?;
            if let Some(parent) = dst.parent() {
                create_dir_all(parent)?;
            }
            // `atomic_write` fsyncs the file. Combined with the directory fsync below,
            // everything the rename publishes is already durable.
            wcore_config::atomic_write(&dst, bytes).map_err(|e| io_err(&dst, e))?;
        }
        sync_dir(&staged);
        Ok(staged)
    }

    /// Remove staging directories left behind by interrupted installs.
    ///
    /// Orphans are inert -- they sit outside the skills tree so nothing loads them -- but
    /// an unbounded pile of them is still the product littering a directory the user owns.
    /// Returns how many were removed.
    pub fn cleanup_staging(&self, skills_root: &Path) -> usize {
        let root = staging_root_for(skills_root);
        let Ok(entries) = std::fs::read_dir(&root) else {
            return 0;
        };
        let mut n = 0;
        for e in entries.flatten() {
            if e.path().is_dir() && std::fs::remove_dir_all(e.path()).is_ok() {
                n += 1;
            }
        }
        n
    }

    /// Withdraw every grant matching this name or signature.
    pub(crate) fn withdraw_promotions_for(
        &self,
        skill_name: &str,
        signature: Option<&str>,
        reason: &str,
    ) -> Result<usize, GovernError> {
        let mut n = 0;
        for p in self.promotions()? {
            let matches = p.skill_name == skill_name
                || match (p.signature.as_deref(), signature) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                };
            if !matches {
                continue;
            }
            let path = self
                .promotions_dir()
                .join(format!("{}.json", p.promotion_id));
            std::fs::remove_file(&path).map_err(|e| io_err(&path, e))?;
            self.append_journal(&JournalEvent::PromotionWithdrawn {
                promotion_id: p.promotion_id.clone(),
                skill_name: p.skill_name.clone(),
                reason: reason.to_string(),
                at: now_rfc3339(),
            })?;
            n += 1;
        }
        Ok(n)
    }

    fn write_grant(&self, grant: &Promotion) -> Result<(), GovernError> {
        let encoded = serde_json::to_vec_pretty(grant)?;
        write_atomic(
            &self
                .promotions_dir()
                .join(format!("{}.json", grant.promotion_id)),
            &encoded,
        )?;
        self.append_journal(&JournalEvent::Promoted {
            promotion_id: grant.promotion_id.clone(),
            skill_name: grant.skill_name.clone(),
            signature: grant.signature.clone(),
            procedure_id: grant.procedure_id.clone(),
            content_digest: grant.content_digest.clone(),
            authority: grant.authority.clone(),
            target_dir: grant.target_dir.clone(),
            at: grant.promoted_at.clone(),
        })
    }

    fn record_refusal(&self, skill_name: &str, refusal: &Refusal) -> Result<(), GovernError> {
        self.append_journal(&JournalEvent::PromotionRefused {
            skill_name: skill_name.to_string(),
            reason: refusal.to_string(),
            at: now_rfc3339(),
        })
    }
}

/// Staging root: a sibling of the skills root, never inside it. See the module docs.
pub(crate) fn staging_root_for(skills_root: &Path) -> PathBuf {
    match skills_root.parent() {
        Some(p) => p.join(STAGING),
        // A skills root with no parent is a filesystem root; nothing sane installs there,
        // but falling back inside it is still better than panicking.
        None => skills_root.join(STAGING),
    }
}

/// Join `rel` under `base`, rejecting absolute paths and any `..` component.
fn resolve_under(base: &Path, rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return None;
    }
    for c in p.components() {
        match c {
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(base.join(p))
}

/// Best-effort directory fsync, so a rename's effects survive a power loss.
///
/// Advisory: a failure here weakens durability but does not make the operation wrong, and
/// on platforms where opening a directory for sync is not supported it is a no-op by
/// design rather than an error to propagate.
pub(crate) fn sync_dir(dir: &Path) {
    if let Ok(f) = std::fs::File::open(dir) {
        let _ = f.sync_all();
    }
}

/// `sha256:<hex>` over a directory's canonical contents.
///
/// Deterministic and unambiguous: entries are walked in sorted order, and each contributes
/// its relative path and its bytes **each length-prefixed**. Without the length prefixes a
/// file `ab` containing `c` and a file `a` containing `bc` would hash identically, so a
/// mutation could be hidden by moving bytes across the path/content boundary.
///
/// Symlinks are refused rather than followed, matching `govern::copy_tree`: a digest that
/// followed links would cover bytes outside the skill directory, and could change without
/// the skill changing at all.
pub fn content_digest(dir: &Path) -> Result<String, GovernError> {
    let mut hasher = Sha256::new();
    let mut entries = Vec::new();
    collect_files(dir, dir, 0, &mut entries)?;
    entries.sort();
    for rel in entries {
        let abs = dir.join(&rel);
        let bytes = std::fs::read(&abs).map_err(|e| io_err(&abs, e))?;
        // Normalise separators so a digest taken on Windows matches one taken on Unix.
        let rel_s = rel.to_string_lossy().replace('\\', "/");
        hasher.update((rel_s.len() as u64).to_le_bytes());
        hasher.update(rel_s.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn collect_files(
    base: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<PathBuf>,
) -> Result<(), GovernError> {
    if depth > MAX_DIGEST_DEPTH {
        return Err(GovernError::RefusedSnapshot {
            path: dir.display().to_string(),
            reason: format!("directory nesting exceeds {MAX_DIGEST_DEPTH} levels"),
        });
    }
    let entries = std::fs::read_dir(dir).map_err(|e| io_err(dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| io_err(&path, e))?;
        if meta.file_type().is_symlink() {
            return Err(GovernError::RefusedSnapshot {
                path: path.display().to_string(),
                reason: "symlinks are not digested; a link could escape the skill directory"
                    .to_string(),
            });
        }
        if meta.is_dir() {
            collect_files(base, &path, depth + 1, out)?;
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

fn tree_stats(dir: &Path) -> Result<(usize, u64), GovernError> {
    let mut files = Vec::new();
    collect_files(dir, dir, 0, &mut files)?;
    let mut bytes = 0u64;
    for rel in &files {
        let abs = dir.join(rel);
        if let Ok(m) = std::fs::metadata(&abs) {
            bytes = bytes.saturating_add(m.len());
        }
    }
    Ok((files.len(), bytes))
}
