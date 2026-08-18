//! v0.8.1 U6 — `SkillDrafter`. Turns a `DraftTrigger` into a draft skill
//! on disk + a `PromptStore` record so the next session's `SkillRouter`
//! (v0.8.1 U1) hydrates it as a seed pair.
//!
//! F-038 fix: drafts are now written in the directory format the loader expects:
//!   `<config_dir>/wayland-core/skills/auto-<sig>/SKILL.md`  — loader-visible
//!   `<config_dir>/wayland-core/skills/auto-<sig>/manifest.json` — metadata
//!
//! Generated drafts remain quarantined and are not registered into any
//! session-owned bundled catalog. F23 will provide governed promotion.
//!
//! The store record is best-effort: a failure does NOT prevent the on-
//! disk draft from landing, and is logged at WARN. Disk failure DOES bubble
//! up so a misconfigured path is visible.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use super::bucketer::DraftTrigger;

/// Tag in the `scorer` column so we can filter auto-drafts out of the
/// real GEPA winners (`bench` / `default`) when slicing the store.
///
/// The rows it tags carry `score: None` (#694). Nothing has ever executed a
/// fresh draft, so there is no measurement to record; the drafter used to
/// write a hardcoded `0.7` here, which `PromptStore::seed_pairs_for` scaled
/// into 4 simulated successes and fed to the live SkillRouter — a prior
/// stronger than a skill whose pass ratio had actually been measured. An
/// auto-draft now starts where an unmeasured arm belongs: cold. A real score
/// lands on these rows only once something measures one.
const AUTO_DRAFT_SCORER: &str = "auto_drafter";

pub struct SkillDrafter {
    skill_dir: PathBuf,
    prompt_store: Option<Arc<wcore_evolve::prompt_store::PromptStore>>,
    /// Override for the loader-visible skills root (the PRIMARY write target).
    /// `None` in production → resolved from `app_config_dir()`. Tests inject a
    /// tempdir so the primary write is hermetic and does not race with (or
    /// pollute) the real user config dir. See #564.
    loader_root: Option<PathBuf>,
    /// 23A-C1: override for the skill-governance root consulted before every
    /// write. `None` in production → resolved by `governance_root()`. Pinned by
    /// tests for the same #564 reason as `loader_root`: a suppression check that
    /// reads a process-global path would make concurrent tests race on one real
    /// directory, and would let a test revoke a skill in the developer's own
    /// profile.
    governance_root: Option<PathBuf>,
}

impl SkillDrafter {
    /// Construct. `skill_dir` is created on first `draft()` call. Pass
    /// `None` for `prompt_store` when running without memory (the draft
    /// still lands on disk; the SkillRouter just won't see it next boot).
    pub fn new(
        skill_dir: PathBuf,
        prompt_store: Option<Arc<wcore_evolve::prompt_store::PromptStore>>,
    ) -> Self {
        Self {
            skill_dir,
            prompt_store,
            loader_root: None,
            governance_root: None,
        }
    }

    /// Pin the governance root (23A-C1). Production leaves this unset and
    /// resolves the real user-level root; tests pass a tempdir.
    pub fn with_governance_root(mut self, root: PathBuf) -> Self {
        self.governance_root = Some(root);
        self
    }

    /// The governance store to consult before drafting, or `None` when no root
    /// can be resolved on this platform (in which case drafting proceeds
    /// un-suppressed, exactly as it did before 23A-C1).
    fn governance(&self) -> Option<wcore_skills::govern::GovernanceStore> {
        match &self.governance_root {
            Some(root) => Some(wcore_skills::govern::GovernanceStore::new(root.clone())),
            None => wcore_skills::govern::GovernanceStore::open_default().ok(),
        }
    }

    /// Test constructor: pin the loader-visible skills root to an explicit
    /// directory (a tempdir) so `draft()`'s primary write is hermetic instead
    /// of landing in the shared, process-global `app_config_dir()`. Without
    /// this, concurrent drafter tests race on one real config path (#564).
    #[cfg(test)]
    pub(crate) fn with_loader_root(
        skill_dir: PathBuf,
        loader_root: PathBuf,
        prompt_store: Option<Arc<wcore_evolve::prompt_store::PromptStore>>,
    ) -> Self {
        Self {
            skill_dir,
            prompt_store,
            loader_root: Some(loader_root),
            governance_root: None,
        }
    }

    /// Draft a candidate skill from the trigger.
    ///
    /// Writes one canonical directory-format draft at the loader-visible path.
    /// Generated provenance is atomically published before `SKILL.md`, so an
    /// interrupted write fails closed as a quarantined/incomplete artifact.
    ///
    /// If configured, also records into the PromptStore for SkillRouter
    /// hydration on next boot.
    pub fn draft(&self, trigger: &DraftTrigger) -> Result<DraftResult, DraftError> {
        let name = format!("auto-{}", trigger.signature);

        // 23A-C1: a revocation is a durable statement of user intent, so it must be
        // honoured HERE -- before any write. Without this check a revoke would only
        // delete a file, and the next qualifying streak would silently recreate it,
        // overriding the user's explicit decision with an automated one. That is the
        // difference between a revocation and an `rm`.
        //
        // Checked against BOTH the name and the drafter signature (see
        // `wcore_skills::govern` on why either key alone is insufficient).
        //
        // A store that cannot be resolved or read must not block drafting outright --
        // that would let one unreadable file disable the learn loop entirely -- so the
        // suppression check degrades to "not revoked" and says so loudly.
        if let Some(store) = self.governance()
            && store.is_revoked(&name, Some(&trigger.signature))
        {
            if let Err(e) = store.record_suppression(&name, Some(&trigger.signature)) {
                tracing::warn!(
                    target: "wcore_agent::auto_skill",
                    error = %e,
                    skill = %name,
                    "could not journal draft suppression; draft still suppressed"
                );
            }
            tracing::info!(
                target: "wcore_agent::auto_skill",
                skill = %name,
                "draft suppressed: this skill was revoked by the user"
            );
            return Err(DraftError::Revoked { name });
        }

        let body = compose_body(&name, trigger);

        // Primary write: loader-visible directory format under the config dir.
        // This matches `user_skills_dir()` = `<config_dir>/wayland-core/skills/`.
        // Tests pin `loader_root` to a tempdir (#564) so this hermetic path does
        // not resolve to the shared, process-global `app_config_dir()`.
        let loader_dir = self
            .loader_root
            .clone()
            .or_else(wcore_config::config::app_config_dir)
            .map(|d| d.join("skills").join(&name))
            .unwrap_or_else(|| self.skill_dir.join(&name));
        std::fs::create_dir_all(&loader_dir)?;
        let json_path = loader_dir.join("manifest.json");
        let manifest = serde_json::json!({
            "name": name,
            "auto_drafted": true,
            "drafted_at": chrono::Utc::now().to_rfc3339(),
            "signature": trigger.signature,
            "evidence_count": trigger.trajectories.len(),
            "needs_review": true,
            // Explicitly null, not omitted: the field exists and is
            // unmeasured, which is a different statement from "no opinion".
            "score": serde_json::Value::Null,
            "scorer": AUTO_DRAFT_SCORER,
        });
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        wcore_config::atomic_write(&json_path, &manifest_bytes)?;

        let md_path = loader_dir.join("SKILL.md");
        wcore_config::atomic_write(&md_path, body.as_bytes())?;

        if let Some(store) = &self.prompt_store {
            let metadata = serde_json::json!({
                "auto_drafted": true,
                "evidence_count": trigger.trajectories.len(),
                "signature": trigger.signature,
            });
            let row = wcore_evolve::prompt_store::EvolvedPrompt {
                id: Uuid::new_v4().to_string(),
                skill_name: name.clone(),
                parent_id: None,
                prompt_body: body.clone(),
                // Unmeasured. See AUTO_DRAFT_SCORER.
                score: None,
                scorer: AUTO_DRAFT_SCORER.to_string(),
                generation: 0,
                created_at: Utc::now().timestamp(),
                metadata: Some(metadata.to_string()),
            };
            if let Err(e) = store.record_variant(&row) {
                tracing::warn!(
                    target: "wcore_agent::auto_skill",
                    error = %e,
                    skill = %name,
                    "PromptStore::record_variant failed; on-disk draft still written"
                );
            } else {
                tracing::debug!(
                    target: "wcore_agent::auto_skill",
                    skill = %name,
                    "recorded auto-draft into PromptStore for SkillRouter hydration"
                );
            }
        }

        Ok(DraftResult {
            name,
            md_path,
            json_path,
        })
    }
}

fn compose_body(name: &str, trigger: &DraftTrigger) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Auto-drafted skill: {name}\n\n"));
    out.push_str(wcore_skills::draft::RELEASED_AUTO_DRAFT_NOTE);
    out.push_str("\n\n");
    out.push_str(&format!("Signature: `{}`\n\n", trigger.signature));
    out.push_str(&format!(
        "Evidence: {} successful turns\n\n",
        trigger.trajectories.len()
    ));
    out.push_str("## When to use\n\n");
    out.push_str("Apply when the task shape resembles:\n\n");
    for (i, t) in trigger.trajectories.iter().take(3).enumerate() {
        out.push_str(&format!(
            "{}. `{}` -> {}\n",
            i + 1,
            truncate(&t.user_input, 80),
            t.summary
        ));
    }
    out.push_str("\n## Approach\n\nDerive from the successful trajectories above. ");
    out.push_str("Refine after human review.\n");
    out
}

#[derive(Debug)]
pub struct DraftResult {
    pub name: String,
    pub md_path: PathBuf,
    pub json_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    /// 23A-C1: the user revoked this draft, so it was not recreated. A distinct
    /// variant rather than a silent `Ok`, so a caller can tell "suppressed by an
    /// explicit user decision" apart from "drafted successfully" and from a real
    /// failure. Callers should treat this as a normal outcome, not an error to
    /// surface as a fault.
    #[error("draft '{name}' was not created: the user revoked this skill")]
    Revoked { name: String },
}

/// Truncate at a char boundary so multibyte input doesn't panic.
fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::auto_skill::bucketer::DraftTrigger;
    use crate::auto_skill::recorder::{TurnOutcome, TurnTrajectory};

    fn fake_trigger() -> DraftTrigger {
        DraftTrigger {
            signature: "code-refactor-review".to_string(),
            trajectories: (0..3)
                .map(|i| TurnTrajectory {
                    user_input: format!("refactor the code (turn {i})"),
                    picked_skill: None,
                    outcome: TurnOutcome::Success,
                    summary: format!("{} turn", i + 1),
                    timestamp: Utc::now(),
                })
                .collect(),
        }
    }

    #[test]
    fn draft_writes_md_and_json_to_skill_dir() {
        let legacy_root = tempfile::tempdir().unwrap();
        let loader_root = tempfile::tempdir().unwrap();
        let drafter = SkillDrafter::with_loader_root(
            legacy_root.path().to_path_buf(),
            loader_root.path().to_path_buf(),
            None,
        );
        let trigger = fake_trigger();
        let res = drafter.draft(&trigger).unwrap();

        assert!(res.md_path.exists(), "md file must be written");
        assert!(res.json_path.exists(), "json file must be written");
        assert!(res.name.starts_with("auto-"));

        let body = std::fs::read_to_string(&res.md_path).unwrap();
        assert!(body.contains("Auto-drafted skill"));
        assert!(body.contains("code-refactor-review"));

        let manifest_text = std::fs::read_to_string(&res.json_path).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest_text).unwrap();
        assert_eq!(manifest["auto_drafted"], serde_json::Value::Bool(true));
        assert_eq!(manifest["needs_review"], serde_json::Value::Bool(true));
        assert_eq!(manifest["evidence_count"], serde_json::json!(3));
        assert_eq!(manifest["scorer"], serde_json::json!("auto_drafter"));
        assert_eq!(
            manifest["score"],
            serde_json::Value::Null,
            "a draft nothing has run must not publish a score"
        );
        assert!(
            !legacy_root.path().join(&res.name).exists(),
            "F06 must publish one canonical draft only"
        );
    }

    // -----------------------------------------------------------------
    // 23A-C1 — revocation must actually stop the drafter
    // -----------------------------------------------------------------

    /// The full customer journey, driven through the real `draft()` entry point:
    /// the product writes into the user's directory, the user revokes it, the
    /// product stops recreating it, and the user can undo the revocation and get
    /// the exact prior bytes back.
    ///
    /// The positive legs matter as much as the negative one. A suppression that
    /// simply refused everything would satisfy "it did not come back" while
    /// destroying the feature, so this asserts drafting works BEFORE the
    /// revocation and AFTER the rollback.
    #[test]
    fn revoked_draft_is_not_recreated_and_rollback_restores_it() {
        let legacy_root = tempfile::tempdir().unwrap();
        let loader_root = tempfile::tempdir().unwrap();
        let gov_root = tempfile::tempdir().unwrap();

        let drafter = SkillDrafter::with_loader_root(
            legacy_root.path().to_path_buf(),
            loader_root.path().to_path_buf(),
            None,
        )
        .with_governance_root(gov_root.path().to_path_buf());
        let store = wcore_skills::govern::GovernanceStore::new(gov_root.path().to_path_buf());
        let trigger = fake_trigger();

        // 1. Positive leg: drafting works normally.
        let first = drafter
            .draft(&trigger)
            .expect("drafting must work before revocation");
        let skill_dir = first.md_path.parent().unwrap().to_path_buf();
        let original_body = std::fs::read_to_string(&first.md_path).unwrap();
        assert!(skill_dir.is_dir());

        // 2. The user revokes it.
        let rec = store.revoke(&skill_dir).unwrap();
        assert!(
            !skill_dir.exists(),
            "revoke must remove it from the user's directory"
        );

        // 3. Negative leg: the same trigger fires again and is suppressed.
        let err = drafter
            .draft(&trigger)
            .expect_err("a revoked draft must NOT be recreated");
        assert!(
            matches!(err, DraftError::Revoked { .. }),
            "expected a Revoked outcome, got {err:?}"
        );
        assert!(
            !skill_dir.exists(),
            "the revoked draft must still be absent after a re-trigger"
        );

        // The suppression is journalled, so "it did not come back" is provable
        // rather than merely observed.
        let suppressions = store
            .journal()
            .unwrap()
            .into_iter()
            .filter(|e| {
                matches!(
                    e,
                    wcore_skills::govern::JournalEvent::DraftSuppressed { .. }
                )
            })
            .count();
        assert_eq!(suppressions, 1, "the suppression must be recorded");

        // 4. Rollback returns the exact prior bytes.
        store.rollback(&rec.revocation_id).unwrap();
        assert!(skill_dir.is_dir(), "rollback must restore the directory");
        assert_eq!(
            std::fs::read_to_string(&first.md_path).unwrap(),
            original_body,
            "rollback must restore the exact prior bytes"
        );

        // 5. Positive leg again: drafting works once more after the rollback.
        drafter
            .draft(&trigger)
            .expect("after rollback the drafter must no longer be suppressed");
    }

    /// Revoking one draft must not stop a different one being written. Without
    /// this, a blanket "suppress everything" bug would pass the test above.
    #[test]
    fn revoking_one_draft_does_not_suppress_a_different_draft() {
        let legacy_root = tempfile::tempdir().unwrap();
        let loader_root = tempfile::tempdir().unwrap();
        let gov_root = tempfile::tempdir().unwrap();

        let drafter = SkillDrafter::with_loader_root(
            legacy_root.path().to_path_buf(),
            loader_root.path().to_path_buf(),
            None,
        )
        .with_governance_root(gov_root.path().to_path_buf());
        let store = wcore_skills::govern::GovernanceStore::new(gov_root.path().to_path_buf());

        let first = drafter.draft(&fake_trigger()).unwrap();
        store.revoke(first.md_path.parent().unwrap()).unwrap();

        let mut other = fake_trigger();
        other.signature = "a-completely-different-signature".to_string();
        let second = drafter
            .draft(&other)
            .expect("an unrelated draft must still be written");

        assert!(second.md_path.exists());
        assert_ne!(second.name, first.name);
    }

    /// With no governance root resolvable and nothing revoked, behaviour is
    /// exactly as it was before 23A-C1 — the check adds no new failure mode.
    #[test]
    fn drafting_is_unaffected_when_nothing_has_been_revoked() {
        let legacy_root = tempfile::tempdir().unwrap();
        let loader_root = tempfile::tempdir().unwrap();
        let gov_root = tempfile::tempdir().unwrap();

        let drafter = SkillDrafter::with_loader_root(
            legacy_root.path().to_path_buf(),
            loader_root.path().to_path_buf(),
            None,
        )
        .with_governance_root(gov_root.path().to_path_buf());

        let res = drafter.draft(&fake_trigger()).unwrap();
        assert!(res.md_path.exists());
        assert!(res.json_path.exists());
    }

    #[test]
    fn draft_records_into_prompt_store_when_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(wcore_memory::db::Db::open_memory().unwrap());
        let store = Arc::new(wcore_evolve::prompt_store::PromptStore::new(db));
        let drafter = SkillDrafter::with_loader_root(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
            Some(store.clone()),
        );
        let trigger = fake_trigger();
        let res = drafter.draft(&trigger).unwrap();

        // The store now has at least one row keyed on the new skill name
        // with scorer="auto_drafter".
        let rows = store.best_for_skill(&res.name, "auto_drafter", 10).unwrap();
        assert_eq!(rows.len(), 1, "expected one auto_drafter row after draft");
        let row = &rows[0];
        assert_eq!(row.skill_name, res.name);
        assert_eq!(
            row.score, None,
            "the drafter must record the absence of a measurement, not a number"
        );
        assert_eq!(row.scorer, "auto_drafter");
        assert!(
            row.metadata
                .as_ref()
                .is_some_and(|m| m.contains("auto_drafted"))
        );
    }

    #[test]
    fn auto_draft_row_is_read_back_but_seeds_the_router_only_once_measured() {
        // Closed-loop read-back guard, in the shape #694 left it. The row the
        // drafter writes in session 1 must still be reachable in session 2
        // through the exact helper bootstrap calls — `seed_pairs_for(..,
        // "auto_drafter", ..)`. Regression target for the gap where bootstrap
        // only hydrated scorer="bench", so auto-drafts were written but never
        // read.
        //
        // What changed is what the read-back is allowed to produce. While the
        // draft is unmeasured it contributes NO prior. The third leg proves
        // Layer 1b is still live wiring rather than dead code: give the same
        // name a real measurement and the seed appears.
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(wcore_memory::db::Db::open_memory().unwrap());
        let store = Arc::new(wcore_evolve::prompt_store::PromptStore::new(db));
        let drafter = SkillDrafter::with_loader_root(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
            Some(store.clone()),
        );
        let res = drafter.draft(&fake_trigger()).unwrap();

        // 1. The row is there, under the auto_drafter scorer.
        let rows = store.best_for_skill(&res.name, "auto_drafter", 10).unwrap();
        assert_eq!(rows.len(), 1, "the drafted row must be readable back");

        // 2. Unmeasured → no seed at all.
        let pairs = store
            .seed_pairs_for(std::slice::from_ref(&res.name), "auto_drafter", 1)
            .unwrap();
        assert!(
            pairs.is_empty(),
            "an unmeasured draft must contribute no router prior, got {pairs:?}"
        );

        // 3. Once something actually measures it, the same seam pays out.
        store
            .record_variant(&wcore_evolve::prompt_store::EvolvedPrompt {
                id: Uuid::new_v4().to_string(),
                skill_name: res.name.clone(),
                parent_id: None,
                prompt_body: "measured variant".to_string(),
                score: Some(0.8),
                scorer: AUTO_DRAFT_SCORER.to_string(),
                generation: 1,
                created_at: Utc::now().timestamp(),
                metadata: None,
            })
            .unwrap();
        let pairs = store
            .seed_pairs_for(std::slice::from_ref(&res.name), "auto_drafter", 1)
            .unwrap();
        assert_eq!(
            pairs,
            vec![(res.name.clone(), 4)],
            "a measured 0.8 must still hydrate as a 4-success router seed"
        );
    }

    #[test]
    fn draft_succeeds_without_prompt_store() {
        let tmp = tempfile::tempdir().unwrap();
        let drafter = SkillDrafter::with_loader_root(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
            None,
        );
        // No prompt store wired — disk write still works.
        let trigger = fake_trigger();
        let res = drafter.draft(&trigger).unwrap();
        assert!(res.md_path.exists());
        assert!(res.json_path.exists());
    }

    /// #694 — the auto-drafter fabricates a measurement.
    ///
    /// Nothing has ever executed the drafted skill, yet the drafter writes a
    /// `score` into `evolved_prompts` in the SAME field and the SAME units as
    /// a real, measured GEPA pass ratio. Bootstrap's Layer 1 (`bench`) and
    /// Layer 1b (`auto_drafter`) then push both through `seed_pairs_for` into
    /// the same Beta prior, so the router cannot tell them apart.
    ///
    /// The consequence under test: an auto-draft with ZERO measurements is
    /// handed a STRONGER router prior than a skill whose 0.5 pass ratio was
    /// actually measured, and therefore wins the live per-turn pick.
    #[test]
    fn unmeasured_auto_draft_must_not_outseed_a_measured_skill() {
        use wcore_dispatch::DecisionRouter;

        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(wcore_memory::db::Db::open_memory().unwrap());
        let store = Arc::new(wcore_evolve::prompt_store::PromptStore::new(db));

        // A genuinely measured arm: the GEPA bench harness really ran this
        // skill and recorded a 0.5 pass ratio.
        let measured_name = "measured-skill".to_string();
        store
            .record_variant(&wcore_evolve::prompt_store::EvolvedPrompt {
                id: Uuid::new_v4().to_string(),
                skill_name: measured_name.clone(),
                parent_id: None,
                prompt_body: "measured body".to_string(),
                score: Some(0.5),
                scorer: "bench".to_string(),
                generation: 0,
                created_at: Utc::now().timestamp(),
                metadata: None,
            })
            .unwrap();

        // The unmeasured arm: an auto-draft. Never executed, never scored.
        let drafter = SkillDrafter::with_loader_root(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
            Some(store.clone()),
        );
        let drafted = drafter.draft(&fake_trigger()).unwrap();

        // Exactly the two bootstrap seams.
        let measured_seed = store
            .seed_pairs_for(std::slice::from_ref(&measured_name), "bench", 1)
            .unwrap();
        let drafted_seed = store
            .seed_pairs_for(std::slice::from_ref(&drafted.name), "auto_drafter", 1)
            .unwrap();
        let measured_successes = measured_seed.first().map(|p| p.1).unwrap_or(0);

        // Exact, not comparative. "fewer successes than the measured arm" is
        // satisfied by a smaller fabricated number, which is the same defect
        // with the volume turned down. The contract is that an unmeasured row
        // contributes NO prior.
        assert!(
            drafted_seed.is_empty(),
            "an unmeasured auto-draft must contribute no router prior at all, \
             got {drafted_seed:?}"
        );
        assert!(
            measured_successes > 0,
            "control: the measured skill must still seed ({measured_successes}), \
             or the assertion above passes for the wrong reason"
        );

        // ...and the same property expressed in live routing.
        //
        // Why not a majority vote: with Beta priors the measured arm (3
        // successes) takes the majority for ANY drafted seed of 2 or less, so
        // a majority assertion only reddens on a near-total reintroduction of
        // the defect and a partial regression -- a fabricated 0.3, say -- sails
        // through it. Assert the exact property instead: routing with whatever
        // the store yields for the draft must be INDISTINGUISHABLE from
        // routing with no draft prior at all. Any nonzero prior moves the
        // distribution, however small.
        let candidates = vec![measured_name.clone(), drafted.name.clone()];
        let picks = |draft_seed: &[(String, u64)]| -> (usize, usize) {
            let mut drafted_picks = 0usize;
            let mut measured_picks = 0usize;
            for seed in 0..600u64 {
                let mut router = wcore_skills::SkillRouter::with_seed(seed);
                router.restore_seeds(measured_seed.clone());
                router.restore_seeds(draft_seed.to_vec());
                let pick = router
                    .choose(wcore_skills::SkillRouterInput {
                        task: "some task",
                        candidates: &candidates,
                    })
                    .unwrap();
                if pick == drafted.name {
                    drafted_picks += 1;
                } else {
                    measured_picks += 1;
                }
            }
            (drafted_picks, measured_picks)
        };

        let observed = picks(&drafted_seed);
        let no_prior_at_all = picks(&[]);
        assert_eq!(
            observed, no_prior_at_all,
            "live routing must treat the unmeasured draft exactly as an arm \
             carrying no prior: got {observed:?} vs {no_prior_at_all:?} \
             (drafted, measured) picks over 600 seeds"
        );
        assert!(
            observed.1 > observed.0,
            "live routing preferred the UNMEASURED auto-draft {}/600 times vs \
             {}/600 for the measured skill",
            observed.0,
            observed.1
        );

        // Positive control for the equality above: the SMALLEST partial
        // regression representable -- one simulated success -- must make the
        // two differ. Otherwise the equality could hold because the comparison
        // is blind rather than because the prior is absent.
        let partial_regression = picks(&[(drafted.name.clone(), 1)]);
        assert_ne!(
            partial_regression, no_prior_at_all,
            "a 1-success prior is invisible to this comparison, so the \
             assertion above proves nothing"
        );
    }
}
