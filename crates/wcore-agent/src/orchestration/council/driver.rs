//! Shared, presentation-agnostic council driver: assemble → approve → run.
//! Both the CLI (TTY approver) and the engine (approval-bridge approver) drive a
//! council through this one path so the proposal card and governance are identical.
use async_trait::async_trait;
use wcore_config::config::Config;
use wcore_config::crucible::CrucibleConfig;
use wcore_pricing::DEFAULT_CATALOG;
use wcore_types::crucible::{CrucibleDecision, CruciblePlan, MICROCENTS_PER_USD};

use crate::spawner::{AgentSpawner, SubAgentConfig};

use super::assembler::{AssemblyPlan, AssemblyPolicy, assemble};
use super::gate::{CouncilDecision, GateConfig, Stakes, classify_task};
use super::plan_card::plan_to_card;
use super::roster::{ProposerSpec, Roster, clamp_temperature};
use super::run::{
    COUNCIL_PROPOSER_SYSTEM_PROMPT, CouncilOutcome, DEFAULT_PROPOSER_MAX_TOKENS, run_council,
};
use super::spend::CouncilSpend;

/// Default intra-family price floor (fraction of a family's flagship price)
/// below which a SKU is dropped as not-competent for a proposer slot.
const DEFAULT_PRICE_FLOOR_FRAC: f64 = 0.25;

/// Roster-selection overrides (mirrors the CLI's CrucibleArgs minus the task).
#[derive(Debug, Clone, Default)]
pub struct CouncilOverrides {
    /// Pin the candidate pool to exactly these specs.
    pub council: Option<Vec<String>>,
    /// Pin the aggregator to this spec.
    pub judge: Option<String>,
    /// Force a single direct answer.
    pub direct: bool,
    /// Force convening a council regardless of the gate.
    pub force_council: bool,
    /// Treat the task as High stakes — widest roster + strongest judge.
    pub deep: bool,
    /// Exclude these provider families from an auto roster.
    pub deny: Vec<String>,
}

/// What a driven council produced.
pub enum CouncilRunResult {
    /// A single direct answer from `spec`.
    Direct { spec: String, text: String },
    /// A fused council outcome, paired with the assembled plan it ran (the plan is
    /// carried so callers can emit the opt-in assembly-preference log, which needs
    /// the chosen roster's family mix — the outcome alone does not carry it).
    Council {
        plan: AssemblyPlan,
        // Boxed: CouncilOutcome is large, so an unboxed variant bloats every
        // CouncilRunResult (clippy::large_enum_variant). Box keeps the enum lean.
        outcome: Box<CouncilOutcome>,
    },
    /// The approver declined — no spend.
    Cancelled,
}

/// How a surface obtains a decision for a proposal card (TTY prompt, approval
/// bridge, etc.). Presentation lives in the impl, not the driver.
#[async_trait]
pub trait CouncilApprover: Send + Sync {
    async fn approve(&self, card: &CruciblePlan) -> anyhow::Result<CrucibleDecision>;
}

/// Map the gate to a [`CouncilDecision`], honoring the force flags.
pub fn build_gate(ov: &CouncilOverrides, task: &str) -> CouncilDecision {
    if ov.direct {
        return CouncilDecision::Direct {
            reason: "forced --direct".to_string(),
        };
    }
    if ov.force_council {
        return CouncilDecision::Council {
            reason: "forced --force-council".to_string(),
            stakes: if ov.deep { Stakes::High } else { Stakes::Med },
        };
    }
    let decision = classify_task(task, &GateConfig::default());
    // --deep escalates a convened council to High (widest roster + top judge).
    if ov.deep {
        if let CouncilDecision::Council { reason, .. } = decision {
            return CouncilDecision::Council {
                reason: format!("{reason} (--deep → High)"),
                stakes: Stakes::High,
            };
        }
        // Even a would-be Direct is convened at High under --deep.
        return CouncilDecision::Council {
            reason: "forced --deep → High".to_string(),
            stakes: Stakes::High,
        };
    }
    decision
}

/// Build the Assembler policy from `[crucible]` config + overrides.
pub fn build_policy(cfg: &CrucibleConfig, ov: &CouncilOverrides) -> AssemblyPolicy {
    AssemblyPolicy {
        deny_families: ov.deny.clone(),
        max_proposers: cfg.max_proposers,
        markup: cfg.flux_markup,
        cap_low_usd: cfg.cap_low_usd,
        cap_med_usd: cfg.cap_med_usd,
        cap_high_usd: cfg.cap_high_usd,
        price_floor_frac: DEFAULT_PRICE_FLOOR_FRAC,
        proposer_max_turns: cfg.proposer_max_turns,
        proposer_max_tokens: DEFAULT_PROPOSER_MAX_TOKENS,
    }
}

/// Split a `provider` / `provider:model` spec into parts (empty model → `None`).
fn split_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((p, m)) if !m.is_empty() => (p, Some(m)),
        _ => (spec, None),
    }
}

/// The stakes-tier cap (USD) from the (possibly-raised) Assembler policy. This is
/// the C1 fix: the judge-override cap-check must read the policy caps, which a
/// premium upgrade / edit may have raised — NOT the original config caps.
fn policy_cap_usd(policy: &AssemblyPolicy, stakes: Stakes) -> f64 {
    match stakes {
        Stakes::Low => policy.cap_low_usd,
        Stakes::Med => policy.cap_med_usd,
        Stakes::High => policy.cap_high_usd,
    }
}

/// Apply a `--judge` override to a convening plan: pin the aggregator and
/// RE-PRICE the roster with the ACTUAL judge, so the surfaced est cost is honest
/// and the tier cap is re-checked. The Assembler priced + cap-checked against
/// ITS chosen judge; a pinned judge can cost more, so without this the est line
/// would lie and the cap would be silently bypassed. The user pinned it
/// deliberately, so we surface a warning in `trims` and proceed (never silently
/// overspend, never silently mis-report). The cap comes from `policy` (the
/// possibly-raised caps), never the original config.
pub fn apply_judge_override(plan: &mut AssemblyPlan, judge: &str, policy: &AssemblyPolicy) {
    plan.aggregator = Some(judge.to_string());
    let proposers: Vec<(&str, Option<&str>)> = plan.members.iter().map(|s| split_spec(s)).collect();
    let est = CouncilSpend::estimate_preflight_microcents(
        &DEFAULT_CATALOG,
        &proposers,
        Some(split_spec(judge)),
        policy.proposer_max_turns,
        policy.proposer_max_tokens,
        policy.markup,
    );
    plan.est_cost_microcents = est.certified_microcents();
    plan.trims.push(format!("judge pinned → {judge}"));
    let cap = policy_cap_usd(policy, plan.stakes);
    match est.certified_microcents() {
        Some(c) if (c as f64 / MICROCENTS_PER_USD) > cap => plan.trims.push(format!(
            "WARNING: pinned judge est ${:.4} exceeds the ${cap:.4} {:?} cap",
            c as f64 / MICROCENTS_PER_USD,
            plan.stakes
        )),
        None => plan
            .trims
            .push("WARNING: pinned judge is unpriceable — cost not bounded".to_string()),
        _ => {}
    }
}

/// Apply the `--judge` override to a convening plan (re-prices + cap-checks). A
/// no-op when no judge override is set or the plan is Direct. Factored out so each
/// re-assemble in the drive loop re-pins the deliberately-chosen judge.
fn reapply_judge_override(plan: &mut AssemblyPlan, ov: &CouncilOverrides, policy: &AssemblyPolicy) {
    if plan.convene
        && let Some(j) = ov.judge.as_deref()
    {
        apply_judge_override(plan, j, policy);
    }
}

/// Build a runnable [`Roster`] from chosen member specs. The auto budget cap was
/// enforced by the Assembler (judge-inclusive pre-flight) — and re-checked +
/// surfaced if a judge override raised it — so the roster's own `max_cost_usd` is
/// left `None` to avoid a second, inconsistent (non-flux) ceiling.
pub fn roster_from_plan(
    members: &[String],
    aggregator: Option<String>,
    cfg: &CrucibleConfig,
) -> Roster {
    Roster {
        proposers: members
            .iter()
            .map(|s| ProposerSpec {
                spec: s.clone(),
                provider: s.split(':').next().unwrap_or(s).to_string(),
                model: s.split_once(':').map(|(_, m)| m.to_string()),
            })
            .collect(),
        aggregator,
        min_proposers: 1,
        proposer_max_turns: cfg.proposer_max_turns,
        proposer_concurrency: cfg.proposer_concurrency,
        proposer_deadline_s: cfg.proposer_deadline_s,
        global_deadline_s: cfg.global_deadline_s,
        max_cost_usd: None,
        flux_markup: cfg.flux_markup,
        daily_cap_usd: cfg.daily_cap_usd,
        // Crucible #3: clamp to the accepted band (same as the manual roster).
        proposer_temperature: clamp_temperature(cfg.proposer_temperature),
        aggregator_temperature: clamp_temperature(cfg.aggregator_temperature),
    }
}

/// Execute an [`AssemblyPlan`]: a single direct call, or a built roster council.
async fn execute_assembled(
    plan: &AssemblyPlan,
    task: &str,
    spawner: &AgentSpawner,
    base: &Config,
    cfg: &CrucibleConfig,
) -> anyhow::Result<CouncilRunResult> {
    if !plan.convene {
        let spec = plan
            .members
            .first()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("assembler produced no model to answer with"))?;
        let result = spawner
            .spawn_one(SubAgentConfig {
                name: spec.clone(),
                prompt: task.to_string(),
                max_turns: cfg.proposer_max_turns,
                max_tokens: DEFAULT_PROPOSER_MAX_TOKENS,
                system_prompt: Some(COUNCIL_PROPOSER_SYSTEM_PROMPT.to_string()),
                provider: Some(spec.clone()),
                model: spec.split_once(':').map(|(_, m)| m.to_string()),
                // Crucible #3: the Direct path is a single proposer-tier call.
                temperature: Some(clamp_temperature(cfg.proposer_temperature)),
            })
            .await;
        if result.is_error {
            anyhow::bail!("direct call failed: {}", result.text);
        }
        return Ok(CouncilRunResult::Direct {
            spec,
            text: result.text,
        });
    }

    let roster = roster_from_plan(&plan.members, plan.aggregator.clone(), cfg);
    let outcome = run_council(task, &roster, spawner, base)
        .await
        .map_err(|e| anyhow::anyhow!("council failed: {e}"))?;
    Ok(CouncilRunResult::Council {
        plan: plan.clone(),
        outcome: Box::new(outcome),
    })
}

/// Drive a council: assemble → approve (re-assembling on Edit/ApprovePremium) →
/// execute. Presentation-agnostic; the `approver` owns the decision surface and
/// `refilter` re-resolves an edited roster to runnable specs.
#[allow(clippy::too_many_arguments)]
pub async fn drive_council(
    task: &str,
    runnable_pool: Vec<String>,
    base: &Config,
    cfg: &CrucibleConfig,
    ov: &CouncilOverrides,
    spawner: &AgentSpawner,
    approver: &dyn CouncilApprover,
    // re-filter an edited roster to runnable specs (caller supplies the resolver)
    refilter: &(dyn Fn(&[String]) -> Vec<String> + Send + Sync),
) -> anyhow::Result<CouncilRunResult> {
    let gate = build_gate(ov, task);
    let mut policy = build_policy(cfg, ov);
    let mut pool = runnable_pool;
    let mut plan = assemble(task, &pool, &DEFAULT_CATALOG, &gate, &policy);
    reapply_judge_override(&mut plan, ov, &policy);

    // The daily cap (USD → microcents) feeds the card's "today" line. day_spent is
    // None here: a one-shot CLI process starts a fresh envelope (cross-process
    // accumulation is a later stage), so showing a running total would be a lie.
    let day_cap_microcents = cfg.daily_cap_usd.map(|u| (u * MICROCENTS_PER_USD) as u64);

    // Assemble → decide → (re-assemble | execute) loop. Approve/Cancel terminate;
    // Edit/ApprovePremium raise caps / edit the pool and re-assemble in place. No
    // infinite-loop guard is needed: a surface can always Cancel.
    loop {
        let card = plan_to_card(&plan, &policy, None, None, day_cap_microcents);
        match approver.approve(&card).await? {
            CrucibleDecision::Approve => break,
            CrucibleDecision::Cancel => return Ok(CouncilRunResult::Cancelled),
            CrucibleDecision::ApprovePremium { ceiling_usd } => {
                // Raise every tier cap to the accepted ceiling and re-assemble: a
                // higher budget lets the Assembler pick a stronger roster.
                policy.cap_low_usd = ceiling_usd;
                policy.cap_med_usd = ceiling_usd;
                policy.cap_high_usd = ceiling_usd;
                plan = assemble(task, &pool, &DEFAULT_CATALOG, &gate, &policy);
                reapply_judge_override(&mut plan, ov, &policy);
            }
            CrucibleDecision::Edit { roster, budget_usd } => {
                // Override the candidate pool and/or the caps, then re-assemble.
                if let Some(specs) = roster {
                    pool = refilter(&specs);
                }
                if let Some(b) = budget_usd {
                    policy.cap_low_usd = b;
                    policy.cap_med_usd = b;
                    policy.cap_high_usd = b;
                }
                plan = assemble(task, &pool, &DEFAULT_CATALOG, &gate, &policy);
                reapply_judge_override(&mut plan, ov, &policy);
            }
        }
    }

    execute_assembled(&plan, task, spawner, base, cfg).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_policy_max_tokens_matches_council_default() {
        // The card is priced against `proposer_max_tokens`; the council spawns each
        // proposer with `DEFAULT_PROPOSER_MAX_TOKENS`. They MUST stay equal or the
        // certified ceiling lies — a one-sided edit fails here.
        let policy = build_policy(&CrucibleConfig::default(), &CouncilOverrides::default());
        assert_eq!(policy.proposer_max_tokens, DEFAULT_PROPOSER_MAX_TOKENS);
    }

    #[test]
    fn build_gate_honors_force_flags() {
        let direct = build_gate(
            &CouncilOverrides {
                direct: true,
                ..Default::default()
            },
            "x",
        );
        assert!(!direct.is_council());

        let forced = build_gate(
            &CouncilOverrides {
                force_council: true,
                ..Default::default()
            },
            "x",
        );
        assert!(forced.is_council());
        assert_eq!(forced.stakes(), Stakes::Med);

        let deep = build_gate(
            &CouncilOverrides {
                force_council: true,
                deep: true,
                ..Default::default()
            },
            "x",
        );
        assert_eq!(deep.stakes(), Stakes::High);
    }

    #[test]
    fn roster_from_plan_carries_concurrency() {
        let cfg = CrucibleConfig {
            proposer_concurrency: 7,
            ..Default::default()
        };
        let roster = roster_from_plan(
            &["openai:gpt-5".to_string(), "anthropic:opus".to_string()],
            Some("anthropic:opus".to_string()),
            &cfg,
        );
        assert_eq!(roster.proposer_concurrency, 7);
        assert_eq!(roster.proposers.len(), 2);
        assert_eq!(roster.aggregator.as_deref(), Some("anthropic:opus"));
        // The auto cap was already enforced pre-flight; the roster carries no
        // second ceiling.
        assert!(roster.max_cost_usd.is_none());
    }

    fn over_cap_plan() -> AssemblyPlan {
        AssemblyPlan {
            convene: true,
            members: vec!["deepseek:deepseek-v4-pro".to_string()],
            aggregator: Some("deepseek:deepseek-v4-pro".to_string()),
            est_cost_microcents: Some(1),
            stakes: Stakes::Med,
            reason: "t".to_string(),
            trims: vec![],
        }
    }

    fn policy_with_med_cap(cap_med_usd: f64) -> AssemblyPolicy {
        AssemblyPolicy {
            deny_families: vec![],
            max_proposers: 5,
            markup: 1.0,
            cap_low_usd: 0.02,
            cap_med_usd,
            cap_high_usd: 0.15,
            price_floor_frac: 0.25,
            proposer_max_turns: 4,
            proposer_max_tokens: DEFAULT_PROPOSER_MAX_TOKENS,
        }
    }

    #[test]
    fn judge_override_reprices_and_warns_when_over_cap() {
        // A cheap 1-proposer plan; pin an expensive judge under a tiny cap. The est
        // must be re-priced to the ACTUAL judge and a cap warning surfaced.
        let mut p = over_cap_plan();
        let policy = policy_with_med_cap(0.0001); // tiny → the opus judge will exceed it
        apply_judge_override(&mut p, "anthropic:claude-opus-4-7", &policy);
        assert_eq!(p.aggregator.as_deref(), Some("anthropic:claude-opus-4-7"));
        // Re-priced to the real (opus) judge — strictly above the seeded 1µ¢.
        assert!(p.est_cost_microcents.unwrap() > 1);
        assert!(p.trims.iter().any(|t| t.contains("judge pinned")));
        assert!(
            p.trims
                .iter()
                .any(|t| t.contains("WARNING") && t.contains("exceeds")),
            "an over-cap pinned judge must surface a warning: {:?}",
            p.trims
        );
    }

    #[test]
    fn c1_judge_override_reads_raised_policy_cap_not_original() {
        // C1 FIX: after a premium upgrade RAISES the policy cap above the judge's
        // est, apply_judge_override must compare against the RAISED cap and push NO
        // "exceeds cap" warning. (Before the fix it read the original config cap and
        // would warn even though the user had already accepted a higher ceiling.)
        // First confirm the same plan/judge DOES warn under a tiny cap, so the
        // no-warning result below is caused by the raised cap, not a priceless judge.
        let mut tight = over_cap_plan();
        apply_judge_override(
            &mut tight,
            "anthropic:claude-opus-4-7",
            &policy_with_med_cap(0.0001),
        );
        assert!(
            tight.trims.iter().any(|t| t.contains("exceeds")),
            "control: a tiny cap MUST still warn: {:?}",
            tight.trims
        );

        let mut p = over_cap_plan();
        // Raise the Med cap well above any plausible opus-judge est.
        apply_judge_override(
            &mut p,
            "anthropic:claude-opus-4-7",
            &policy_with_med_cap(1_000.0),
        );
        assert!(
            !p.trims.iter().any(|t| t.contains("exceeds")),
            "a raised policy cap must NOT trip the over-cap warning: {:?}",
            p.trims
        );
        // The repricing + pin note still happen — only the cap WARNING is suppressed.
        assert!(p.trims.iter().any(|t| t.contains("judge pinned")));
        assert!(p.est_cost_microcents.unwrap() > 1);
    }
}
