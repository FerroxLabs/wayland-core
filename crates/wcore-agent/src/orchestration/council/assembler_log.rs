//! Privacy-safe preference logging for the auto Assembler — the learning signal
//! for a future BetaScorer, started day one but harmless until then.
//!
//! Each opt-in auto council appends ONE JSON line capturing only the *shape* of
//! the decision: stakes class, the provider-FAMILY mix, the aggregator family,
//! and est-vs-actual cost. It NEVER records raw task text, the `reason`/`trims`
//! trace, model specs beyond their family, or any credential — so the log is a
//! preference signal, not a transcript. Gated by `[crucible].log_assembly`
//! (default OFF); a write failure is swallowed (logging must never break a run).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use wcore_config::crucible::CrucibleConfig;

use super::assembler::AssemblyPlan;
use super::resolver::family;
use super::spend::CouncilSpend;

/// Build the privacy-safe JSONL preference line. Contains ONLY the stakes class,
/// provider-family mix, aggregator family, and est-vs-actual microcents — no task
/// text, no `reason`/`trims`, no model specs beyond their family, no keys.
pub fn assembly_log_line(plan: &AssemblyPlan, actual: &CouncilSpend) -> String {
    let proposer_families: Vec<String> = plan.members.iter().map(|s| family(s)).collect();
    let aggregator_family = plan.aggregator.as_deref().map(family);
    serde_json::json!({
        "stakes": format!("{:?}", plan.stakes),
        "convene": plan.convene,
        "proposer_families": proposer_families,
        "aggregator_family": aggregator_family,
        "est_microcents": plan.est_cost_microcents,
        "actual_microcents": actual.total_cost_microcents,
    })
    .to_string()
}

/// Append the preference line to `crucible-assembly.jsonl` under the user config
/// dir (or `override_dir` in tests), gated by `cfg.log_assembly` (default OFF).
/// Best-effort: any failure (no config dir, mkdir/open/write error) is silently
/// ignored — a council run must never fail because of logging.
pub fn log_assembly(
    plan: &AssemblyPlan,
    actual: &CouncilSpend,
    cfg: &CrucibleConfig,
    override_dir: Option<PathBuf>,
) {
    if !cfg.log_assembly {
        return;
    }
    let Some(base) = override_dir.or_else(dirs::config_dir) else {
        return;
    };
    let dir = base.join("wayland-core");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("crucible-assembly.jsonl");
    let line = assembly_log_line(plan, actual);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::council::gate::Stakes;

    fn plan() -> AssemblyPlan {
        AssemblyPlan {
            convene: true,
            members: vec![
                "openai:gpt-5".to_string(),
                "anthropic:claude-opus-4-7".to_string(),
            ],
            aggregator: Some("anthropic:claude-opus-4-7".to_string()),
            est_cost_microcents: Some(200_000),
            stakes: Stakes::Med,
            // Deliberately stuff task-derived text into the trace fields to prove
            // they are NEVER logged.
            reason: "SECRET_TASK_TEXT fix the auth bug".to_string(),
            trims: vec!["SECRET_TRIM".to_string()],
        }
    }

    fn spend() -> CouncilSpend {
        CouncilSpend {
            total_cost_microcents: 180_000,
            ..Default::default()
        }
    }

    #[test]
    fn line_has_family_mix_and_cost_but_never_task_text_or_model() {
        let line = assembly_log_line(&plan(), &spend());
        // Family mix + est-vs-actual cost are present.
        assert!(line.contains("openai"));
        assert!(line.contains("anthropic"));
        assert!(line.contains("200000"), "est cost");
        assert!(line.contains("180000"), "actual cost");
        assert!(line.contains("Med"));
        // Task text, trace, and model specs beyond family are NEVER present.
        assert!(!line.contains("SECRET_TASK_TEXT"));
        assert!(!line.contains("SECRET_TRIM"));
        assert!(
            !line.contains("gpt-5"),
            "model spec beyond family must not leak"
        );
        assert!(!line.contains("claude-opus"));
    }

    #[test]
    fn opt_out_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = CrucibleConfig {
            log_assembly: false,
            ..Default::default()
        };
        log_assembly(&plan(), &spend(), &cfg, Some(tmp.path().to_path_buf()));
        let path = tmp
            .path()
            .join("wayland-core")
            .join("crucible-assembly.jsonl");
        assert!(!path.exists(), "opt-out must write nothing");
    }

    #[test]
    fn opt_in_appends_one_line_per_call() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = CrucibleConfig {
            log_assembly: true,
            ..Default::default()
        };
        log_assembly(&plan(), &spend(), &cfg, Some(tmp.path().to_path_buf()));
        log_assembly(&plan(), &spend(), &cfg, Some(tmp.path().to_path_buf()));
        let path = tmp
            .path()
            .join("wayland-core")
            .join("crucible-assembly.jsonl");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 2, "one appended line per call");
        assert!(content.contains("openai"));
        assert!(!content.contains("SECRET_TASK_TEXT"));
    }
}
