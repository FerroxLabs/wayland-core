//! v0.8.1 U6 — `SkillDrafter`. Turns a `DraftTrigger` into a draft skill
//! on disk + a `PromptStore` record so the next session's `SkillRouter`
//! (v0.8.1 U1) hydrates it as a seed pair.
//!
//! F-038 fix: drafts are now written in the directory format the loader expects:
//!   `<config_dir>/wayland-core/skills/auto-<sig>/SKILL.md`  — loader-visible
//!   `<config_dir>/wayland-core/skills/auto-<sig>/manifest.json` — metadata
//!
//! A secondary write to `self.skill_dir` (WAYLAND_HOME-based) is attempted as
//! a best-effort fallback and logged on failure.
//!
//! Additionally, the draft is registered in-process via `register_bundled_skill`
//! so the current session's catalog sees it immediately (no next-boot wait).
//!
//! The store record is best-effort: a failure does NOT prevent the on-
//! disk draft from landing, and is logged at WARN. Disk failure DOES bubble
//! up so a misconfigured path is visible.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use super::bucketer::DraftTrigger;

/// Score baseline credited to the auto-drafted prompt in the
/// `evolved_prompts` table. The U1 SkillRouter's `seed_pairs_for` helper
/// scales this `0.0..=1.0` value by 5 to produce simulated successes —
/// `0.7` → 4 simulated successes, a "confident-but-not-pinned" weight
/// that puts the new skill ahead of cold-start arms without dominating
/// proven winners.
const AUTO_DRAFT_SCORE: f64 = 0.7;

/// Tag in the `scorer` column so we can filter auto-drafts out of the
/// real GEPA winners (`bench` / `default`) when slicing the store.
const AUTO_DRAFT_SCORER: &str = "auto_drafter";

pub struct SkillDrafter {
    skill_dir: PathBuf,
    prompt_store: Option<Arc<wcore_evolve::prompt_store::PromptStore>>,
    /// Override for the loader-visible skills root (the PRIMARY write target).
    /// `None` in production → resolved from `app_config_dir()`. Tests inject a
    /// tempdir so the primary write is hermetic and does not race with (or
    /// pollute) the real user config dir. See #564.
    loader_root: Option<PathBuf>,
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
        }
    }

    /// Test constructor: pin the loader-visible skills root to an explicit
    /// directory (a tempdir) so `draft()`'s primary write is hermetic instead
    /// of landing in the shared, process-global `app_config_dir()`. Without
    /// this, concurrent drafter tests race on one real config path (#564).
    #[cfg(test)]
    fn with_loader_root(
        skill_dir: PathBuf,
        loader_root: PathBuf,
        prompt_store: Option<Arc<wcore_evolve::prompt_store::PromptStore>>,
    ) -> Self {
        Self {
            skill_dir,
            prompt_store,
            loader_root: Some(loader_root),
        }
    }

    /// Draft a candidate skill from the trigger.
    ///
    /// F-038: writes in directory format (`<skill_name>/SKILL.md`) to two
    /// locations so the loader can discover the draft on next boot:
    ///
    /// 1. **Loader-visible path** — `<config_dir>/wayland-core/skills/auto-<sig>/SKILL.md`
    ///    (matches `user_skills_dir()` in `wcore-skills::paths`).
    /// 2. **Legacy path** — `<self.skill_dir>/auto-<sig>/SKILL.md` (the path
    ///    bootstrap wires from `$WAYLAND_HOME/skills/auto/`). Written as a
    ///    best-effort fallback; failure is logged but does NOT abort the draft.
    ///
    /// The manifest JSON is written alongside the `SKILL.md` in the skill
    /// subdirectory as `manifest.json`.
    ///
    /// If configured, also records into the PromptStore for SkillRouter
    /// hydration on next boot.
    pub fn draft(&self, trigger: &DraftTrigger) -> Result<DraftResult, DraftError> {
        let name = format!("auto-{}", trigger.signature);
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
        let md_path = loader_dir.join("SKILL.md");
        std::fs::create_dir_all(&loader_dir)?;
        std::fs::write(&md_path, &body)?;

        let json_path = loader_dir.join("manifest.json");
        let manifest = serde_json::json!({
            "name": name,
            "auto_drafted": true,
            "drafted_at": chrono::Utc::now().to_rfc3339(),
            "signature": trigger.signature,
            "evidence_count": trigger.trajectories.len(),
            "needs_review": true,
            "score": AUTO_DRAFT_SCORE,
            "scorer": AUTO_DRAFT_SCORER,
        });
        std::fs::write(&json_path, serde_json::to_string_pretty(&manifest)?)?;

        // Secondary write: legacy path under self.skill_dir (WAYLAND_HOME/skills/auto/).
        // Best-effort — a failure here does not abort the draft.
        let legacy_dir = self.skill_dir.join(&name);
        if let Err(e) = std::fs::create_dir_all(&legacy_dir)
            .and_then(|_| std::fs::write(legacy_dir.join("SKILL.md"), &body))
        {
            tracing::warn!(
                target: "wcore_agent::auto_skill",
                error = %e,
                skill = %name,
                legacy_dir = %legacy_dir.display(),
                "SkillDrafter: legacy path write failed (loader-visible path succeeded)"
            );
        }

        // In-process registration: make the draft reachable in the CURRENT
        // session's skill catalog without waiting for next boot.
        // This is best-effort — registration failures are logged and swallowed.
        {
            use wcore_skills::bundled::{BundledSkillDefinition, register_bundled_skill};
            // Leak the strings so they satisfy the 'static bound of BundledSkillDefinition.
            // Plugin lifetime == process lifetime — the leak is intentional.
            let static_name: &'static str = Box::leak(name.clone().into_boxed_str());
            let static_desc: &'static str = Box::leak(
                format!(
                    "Auto-drafted skill from {} successful turns",
                    trigger.trajectories.len()
                )
                .into_boxed_str(),
            );
            let static_content: &'static str = Box::leak(body.clone().into_boxed_str());
            register_bundled_skill(BundledSkillDefinition {
                name: static_name,
                description: static_desc,
                when_to_use: None,
                argument_hint: None,
                allowed_tools: &[],
                model: None,
                disable_model_invocation: false,
                user_invocable: true,
                context: None,
                agent: None,
                files: &[],
                content: static_content,
            });
            tracing::debug!(
                target: "wcore_agent::auto_skill",
                skill = %name,
                "auto-draft registered in-process via bundled-skill registry"
            );
        }

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
                score: AUTO_DRAFT_SCORE,
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
    out.push_str(
        "> NOTE: This skill was auto-drafted from a streak of successful turns with similar \
         task signatures. Review and edit before treating it as canonical.\n\n",
    );
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
        let tmp = tempfile::tempdir().unwrap();
        let drafter = SkillDrafter::with_loader_root(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
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
        assert!((row.score - AUTO_DRAFT_SCORE).abs() < 1e-9);
        assert_eq!(row.scorer, "auto_drafter");
        assert!(
            row.metadata
                .as_ref()
                .is_some_and(|m| m.contains("auto_drafted"))
        );
    }

    #[test]
    fn drafted_skill_hydrates_router_seed_via_auto_drafter_scorer() {
        // Closed-loop read-back guard: the row the drafter writes in
        // session 1 must become a NONZERO router seed in session 2 via the
        // exact helper bootstrap calls — `seed_pairs_for(.., "auto_drafter",
        // ..)`. Regression target for the gap where bootstrap only hydrated
        // scorer="bench", so auto-drafts were written but never read.
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(wcore_memory::db::Db::open_memory().unwrap());
        let store = Arc::new(wcore_evolve::prompt_store::PromptStore::new(db));
        let drafter = SkillDrafter::with_loader_root(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
            Some(store.clone()),
        );
        let res = drafter.draft(&fake_trigger()).unwrap();

        let pairs = store
            .seed_pairs_for(std::slice::from_ref(&res.name), "auto_drafter", 1)
            .unwrap();
        // AUTO_DRAFT_SCORE (0.7) × 5 = 3.5 → round → 4 simulated successes.
        assert_eq!(
            pairs,
            vec![(res.name.clone(), 4)],
            "auto-drafted skill must hydrate as a 4-success router seed"
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
}
