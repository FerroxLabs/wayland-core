//! `wayland-core crucible "<task>"` — run the cross-provider council (Crucible /
//! Mixture-of-Providers).
//!
//! Two modes, decided by `[crucible].assembly` + CLI overrides:
//!
//! - **Manual** (default, `assembly = "manual"`, no assembly flags): the roster
//!   comes verbatim from `[crucible].proposers` / `aggregator`. With `--auto` a
//!   cheap gate decides whether to convene at all. This path is byte-identical
//!   to the shipped behavior in roster selection + fused output; the post-quorum
//!   tail-latency cut (a hung straggler is cancelled at the global soft-deadline)
//!   applies to all councils as a strict latency improvement.
//! - **Auto** (`assembly = "auto"`, or any of `--council/--judge/--direct/
//!   --force-council/--deep/--deny`): the deterministic [`assemble`] picks a
//!   cost-effective, provider-diverse roster from the keyed candidate pool, and
//!   a pre-flight transparency line shows the plan before it runs.

use std::sync::Arc;

use wcore_agent::orchestration::council::{
    AssemblyPlan, AssemblyPolicy, CouncilDecision, CouncilOutcome, CouncilProviderResolver,
    CouncilSpend, DEFAULT_PROPOSER_MAX_TOKENS, GateConfig, ProposerSpec, Roster, Stakes, assemble,
    classify_task, log_assembly, plan_to_card, run_council, validate_and_build,
};
use wcore_agent::spawner::{AgentSpawner, SubAgentConfig};
use wcore_config::config::{CliArgs, Config, ConfigFile, load_merged_config_file};
use wcore_config::crucible::{AssemblyMode, CrucibleConfig};
use wcore_pricing::DEFAULT_CATALOG;
use wcore_types::crucible::CrucibleDecision;

/// Default intra-family price floor (fraction of a family's flagship price)
/// below which a SKU is dropped as not-competent for a proposer slot.
const DEFAULT_PRICE_FLOOR_FRAC: f64 = 0.25;

/// A cap-less per-user/day spend accumulator for council charging, built when the
/// council has a daily or per-run cap configured. The daily bound is enforced by
/// run_council's soft pre-check; this tracker must always record (no caps).
/// NOTE: a one-shot `wcore crucible` process starts fresh, so the daily envelope
/// binds within a process, not across invocations — cross-process persistence is
/// a later stage.
fn council_budget_tracker(
    cf: &ConfigFile,
) -> Option<std::sync::Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>> {
    (cf.crucible.daily_cap_usd.is_some() || cf.crucible.max_cost_usd.is_some()).then(|| {
        std::sync::Arc::new(parking_lot::Mutex::new(wcore_budget::BudgetTracker::new(
            wcore_budget::BudgetCap::default(),
        )))
    })
}

/// The CLI council charge identity: WAYLAND_USER_ID (default "default"), and a
/// per-process session id (cross-process daily accumulation is a later stage).
fn cli_budget_identity() -> (String, String) {
    let user = std::env::var("WAYLAND_USER_ID").unwrap_or_else(|_| "default".to_string());
    ("cli".to_string(), user)
}

/// CLI arguments for the `crucible` subcommand.
#[derive(Debug, Clone, Default)]
pub struct CrucibleArgs {
    /// The task for the council to work.
    pub task: String,
    /// Gate a MANUAL roster: a cheap classifier decides convene-vs-direct.
    pub auto: bool,
    /// Pin the auto candidate pool to exactly these specs (forces auto mode).
    pub council: Option<Vec<String>>,
    /// Pin the auto aggregator to this spec (forces auto mode).
    pub judge: Option<String>,
    /// Force a single direct answer (auto mode).
    pub direct: bool,
    /// Force convening a council regardless of the gate (auto mode).
    pub force_council: bool,
    /// Treat the task as High stakes — widest roster + strongest judge.
    pub deep: bool,
    /// Exclude these provider families from an auto roster.
    pub deny: Vec<String>,
}

/// Whether the auto Assembler should choose the roster (vs the manual path).
fn wants_auto(cfg: &CrucibleConfig, args: &CrucibleArgs) -> bool {
    cfg.assembly == AssemblyMode::Auto
        || args.council.is_some()
        || args.judge.is_some()
        || args.direct
        || args.force_council
        || args.deep
        || !args.deny.is_empty()
}

/// Render the human-readable provenance + spend block a council run prints to
/// stderr (skipped members, fused providers, and the per-member token/cost
/// rollup). Pure so the exact operator-facing shape is unit-testable without
/// spawning a council. Each line is newline-terminated.
pub fn render_provenance(outcome: &CouncilOutcome) -> String {
    let mut out = String::new();

    // Any members skipped before spawn (keyless / unknown), with the reason.
    for s in &outcome.skipped {
        out.push_str(&format!(
            "crucible: skipped proposer '{}' ({})\n",
            s.spec, s.reason
        ));
    }
    out.push_str(&format!(
        "crucible: fused {} proposal(s) from [{}]\n",
        outcome.chosen_from.len(),
        outcome.chosen_from.join(", ")
    ));

    // Members that ran but did not contribute (errored, timed out, or cancelled
    // post-quorum at the soft-deadline) — shown so a 3-of-5 council is never
    // mistaken for a clean 5-of-5.
    for p in outcome.proposals.iter().filter(|p| p.is_error) {
        out.push_str(&format!(
            "crucible: member '{}' did not contribute ({}, {}ms)\n",
            p.provider,
            p.text.trim(),
            p.latency_ms
        ));
    }

    // Spend rollup: total + per-member token/cost breakdown.
    let spend = &outcome.spend;
    out.push_str(&format!(
        "crucible: spend = {} in + {} out tokens, ~${:.4} across {} member(s)\n",
        spend.total_input_tokens,
        spend.total_output_tokens,
        spend.total_cost_usd(),
        spend.per_provider.len()
    ));
    for ps in &spend.per_provider {
        let cost = if ps.priced {
            format!(
                "${:.4}",
                ps.cost_microcents as f64 / wcore_types::crucible::MICROCENTS_PER_USD
            )
        } else {
            "unpriced".to_string()
        };
        out.push_str(&format!(
            "crucible:   {} ({}): {} in / {} out → {cost}\n",
            ps.provider,
            ps.model.as_deref().unwrap_or("?"),
            ps.input_tokens,
            ps.output_tokens,
        ));
    }
    out
}

/// Render the auto Assembler's pre-flight plan — the real decision trace shown
/// before the council runs. Pure + unit-testable.
pub fn render_assembly_plan(plan: &AssemblyPlan) -> String {
    let mut out = String::new();
    if plan.convene {
        out.push_str(&format!(
            "crucible: auto-assembled {:?} council — {} proposer(s): [{}]\n",
            plan.stakes,
            plan.members.len(),
            plan.members.join(", ")
        ));
        if let Some(agg) = &plan.aggregator {
            out.push_str(&format!("crucible:   aggregator = {agg}\n"));
        }
    } else {
        out.push_str(&format!(
            "crucible: auto → direct ({:?}) — {}\n",
            plan.stakes,
            plan.members.first().map(String::as_str).unwrap_or("(none)")
        ));
    }
    if let Some(c) = plan.est_cost_microcents {
        out.push_str(&format!(
            "crucible:   est cost ~${:.4}\n",
            c as f64 / wcore_types::crucible::MICROCENTS_PER_USD
        ));
    }
    if !plan.trims.is_empty() {
        out.push_str(&format!("crucible:   trims: {}\n", plan.trims.join("; ")));
    }
    // R3: a High-stakes task that was trimmed/downgraded must never be silent.
    if plan.stakes == Stakes::High && (!plan.convene || !plan.trims.is_empty()) {
        out.push_str(
            "crucible:   note: High-stakes plan was reduced to fit the budget — \
             use --deep or raise [crucible].cap_high_usd to widen it\n",
        );
    }
    out.push_str(&format!("crucible:   reason: {}\n", plan.reason));
    out
}

/// Run the council over `task`. Dispatches manual vs auto by config + flags.
pub async fn run_crucible(args: CrucibleArgs) -> anyhow::Result<()> {
    let cf = load_merged_config_file(None)?;

    if !cf.crucible.enabled {
        anyhow::bail!(
            "the Crucible council is disabled. Set `enabled = true` under `[crucible]` \
             in your config and list `proposers = [\"provider\", ...]`."
        );
    }

    if wants_auto(&cf.crucible, &args) {
        return run_crucible_auto(&args, &cf).await;
    }

    // ---- MANUAL PATH (byte-identical to the shipped behavior) ----
    let roster = validate_and_build(&cf.crucible)
        .map_err(|e| anyhow::anyhow!("invalid [crucible] config: {e}"))?;

    let base = {
        let provider = roster.proposers.first().map(|p| p.provider.clone());
        Config::resolve(&CliArgs {
            provider,
            ..CliArgs::default()
        })?
    };
    wcore_agent::egress::install_egress_policy(&base);
    let provider = wcore_agent::bootstrap::create_provider_with_oauth(&base)?;

    let resolver = CouncilProviderResolver::new(base.clone(), cf.providers.clone());
    let mut spawner =
        AgentSpawner::new(provider, base.clone()).with_provider_resolver(Arc::new(resolver));
    if let Some(tracker) = council_budget_tracker(&cf) {
        let (sess, user) = cli_budget_identity();
        spawner = spawner
            .with_budget_tracker(tracker)
            .with_budget_identity(sess, user);
    }

    if args.auto
        && let CouncilDecision::Direct { reason } =
            classify_task(&args.task, &GateConfig::default())
    {
        return run_direct(&args.task, &roster, &spawner, &reason).await;
    }

    eprintln!(
        "crucible: convening {} proposer(s){}",
        roster.proposers.len(),
        roster
            .aggregator
            .as_deref()
            .map(|a| format!(", aggregator = {a}"))
            .unwrap_or_default()
    );

    let outcome = run_council(&args.task, &roster, &spawner, &base)
        .await
        .map_err(|e| anyhow::anyhow!("council failed: {e}"))?;

    eprint!("{}", render_provenance(&outcome));
    println!("{}", outcome.final_text);
    Ok(())
}

/// The AUTO path: the Assembler chooses the roster from the keyed candidate pool.
async fn run_crucible_auto(args: &CrucibleArgs, cf: &ConfigFile) -> anyhow::Result<()> {
    // Base resolves against the SESSION DEFAULT provider — NOT a proposer — since
    // the auto premise is that no roster is listed. The base provider is a
    // never-used placeholder (every council member is pinned); it just resolves.
    let base = Config::resolve(&CliArgs::default())?;
    wcore_agent::egress::install_egress_policy(&base);
    let provider = wcore_agent::bootstrap::create_provider_with_oauth(&base)?;

    // Candidate pool: --council override, else proposers ∪ candidate_pool.
    let candidates = args.council.clone().unwrap_or_else(|| {
        let mut v = cf.crucible.proposers.clone();
        v.extend(cf.crucible.candidate_pool.clone());
        v
    });

    // Filter to runnable (keyed) specs on the concrete resolver before it moves
    // into the spawner's Arc.
    let resolver = CouncilProviderResolver::new(base.clone(), cf.providers.clone());
    let runnable = resolver.resolvable_specs(&candidates);
    if runnable.is_empty() {
        anyhow::bail!(
            "no runnable council candidates — list `proposers` / `candidate_pool` under \
             `[crucible]` (or pass --council) and ensure their providers are keyed."
        );
    }
    let mut spawner =
        AgentSpawner::new(provider, base.clone()).with_provider_resolver(Arc::new(resolver));
    if let Some(tracker) = council_budget_tracker(cf) {
        let (sess, user) = cli_budget_identity();
        spawner = spawner
            .with_budget_tracker(tracker)
            .with_budget_identity(sess, user);
    }

    let gate = build_gate(args);
    let mut policy = build_policy(&cf.crucible, args);
    let mut pool = runnable;
    let mut plan = assemble(&args.task, &pool, &DEFAULT_CATALOG, &gate, &policy);
    reapply_judge_override(&mut plan, args, &cf.crucible, &policy);

    // The daily cap (USD → microcents) feeds the card's "today" line. day_spent is
    // None here: a one-shot CLI process starts a fresh envelope (cross-process
    // accumulation is a later stage), so showing a running total would be a lie.
    let day_cap_microcents = cf
        .crucible
        .daily_cap_usd
        .map(|u| (u * wcore_types::crucible::MICROCENTS_PER_USD) as u64);

    // Assemble → decide → (re-assemble | execute) loop. The TTY only yields
    // Approve/Cancel in 3a; Edit/ApprovePremium are reachable via a (future) typed
    // non-TTY decision and re-assemble in place. No infinite-loop guard is needed:
    // a TTY can always Cancel, and a non-TTY never returns Edit/Premium in 3a.
    loop {
        let card = plan_to_card(&plan, &policy, None, None, day_cap_microcents);
        match decide(&card, cf.crucible.crucible_auto_spend)? {
            CrucibleDecision::Approve => break,
            CrucibleDecision::Cancel => {
                eprintln!("crucible: cancelled — no spend.");
                return Ok(());
            }
            CrucibleDecision::ApprovePremium { ceiling_usd } => {
                // Raise every tier cap to the accepted ceiling and re-assemble: a
                // higher budget lets the Assembler pick a stronger roster.
                policy.cap_low_usd = ceiling_usd;
                policy.cap_med_usd = ceiling_usd;
                policy.cap_high_usd = ceiling_usd;
                plan = assemble(&args.task, &pool, &DEFAULT_CATALOG, &gate, &policy);
                reapply_judge_override(&mut plan, args, &cf.crucible, &policy);
            }
            CrucibleDecision::Edit { roster, budget_usd } => {
                // Override the candidate pool and/or the caps, then re-assemble.
                if let Some(specs) = roster {
                    // Re-filter the edited roster to runnable (keyed) specs through a
                    // fresh resolver — the original was moved into the spawner's Arc.
                    let r = CouncilProviderResolver::new(base.clone(), cf.providers.clone());
                    pool = r.resolvable_specs(&specs);
                }
                if let Some(b) = budget_usd {
                    policy.cap_low_usd = b;
                    policy.cap_med_usd = b;
                    policy.cap_high_usd = b;
                }
                plan = assemble(&args.task, &pool, &DEFAULT_CATALOG, &gate, &policy);
                reapply_judge_override(&mut plan, args, &cf.crucible, &policy);
            }
        }
    }

    match execute_plan(&plan, &args.task, &spawner, &base, &cf.crucible).await? {
        AutoRun::Direct { spec, text } => {
            eprintln!("crucible: direct answer via {spec}");
            println!("{text}");
        }
        AutoRun::Council(outcome) => {
            // Privacy-safe preference signal (opt-in; family-mix + cost only).
            log_assembly(&plan, &outcome.spend, &cf.crucible, None);
            eprint!("{}", render_provenance(&outcome));
            println!("{}", outcome.final_text);
        }
    }
    Ok(())
}

/// Apply the `--judge` override to a convening plan (re-prices + cap-checks). A
/// no-op when no `--judge` was passed or the plan is Direct. Factored out so each
/// re-assemble in the decide loop re-pins the deliberately-chosen judge.
fn reapply_judge_override(
    plan: &mut AssemblyPlan,
    args: &CrucibleArgs,
    cfg: &CrucibleConfig,
    policy: &AssemblyPolicy,
) {
    if plan.convene
        && let Some(j) = args.judge.as_deref()
    {
        apply_judge_override(plan, j, cfg, policy);
    }
}

/// Render the typed [`CruciblePlan`] proposal card — the human-facing decision
/// surface shown before any spend. Pure + unit-testable; each line is
/// newline-terminated. A `None` cost ALWAYS renders "price unknown", never "$0".
fn render_card(card: &wcore_types::crucible::CruciblePlan) -> String {
    use wcore_types::crucible::CouncilRole;

    let mut out = String::new();
    out.push_str(if card.convene {
        "crucible plan (council)\n"
    } else {
        "crucible plan (direct)\n"
    });
    out.push_str(&format!("crucible:   stakes: {}\n", card.stakes));
    if let Some(focus) = &card.focus {
        out.push_str(&format!("crucible:   focus: {focus}\n"));
    }
    for m in &card.members {
        let role = match m.role {
            CouncilRole::Proposer => "proposer",
            CouncilRole::Judge => "judge",
        };
        out.push_str(&format!("crucible:   {role}  {}  ({})\n", m.spec, m.vendor));
    }
    match card.ceiling_usd() {
        Some(c) => out.push_str(&format!("crucible:   ceiling ~ ${c:.4}\n")),
        None => out.push_str("crucible:   ceiling: price unknown\n"),
    }
    if let Some(b) = card.baseline_usd() {
        out.push_str(&format!("crucible:   one strong model alone ~ ${b:.4}\n"));
    }
    if card.convene {
        out.push_str(if card.judge_independent {
            "crucible:   judge: independent\n"
        } else {
            "crucible:   judge: shares a proposer vendor\n"
        });
    }
    if let Some(cap) = card.day_cap_microcents {
        let spent = card.day_spent_microcents.unwrap_or(0) as f64
            / wcore_types::crucible::MICROCENTS_PER_USD;
        let cap = cap as f64 / wcore_types::crucible::MICROCENTS_PER_USD;
        out.push_str(&format!("crucible:   today: ${spent:.4} / ${cap:.4}\n"));
    }
    out.push_str(&format!("crucible:   reason: {}\n", card.reason));
    for t in &card.trims {
        out.push_str(&format!("crucible:   note: {t}\n"));
    }
    out
}

/// Decide how to proceed with a council plan. Interactive when stdin is a TTY;
/// otherwise fail-closed unless `crucible_auto_spend` is set.
fn decide(
    card: &wcore_types::crucible::CruciblePlan,
    auto_spend: bool,
) -> anyhow::Result<CrucibleDecision> {
    use std::io::{IsTerminal, Write};
    eprint!("{}", render_card(card));
    if !std::io::stdin().is_terminal() {
        if auto_spend {
            eprintln!("crucible: non-interactive + crucible_auto_spend=true → auto-approving.");
            return Ok(CrucibleDecision::Approve);
        }
        anyhow::bail!(
            "crucible: refusing to spend in a non-interactive session. Re-run in a terminal to \
             approve, or set `crucible_auto_spend = true` under [crucible] to allow headless runs."
        );
    }
    eprint!("Proceed? [Y]es / [n]o (no spend): ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    match line.trim().to_ascii_lowercase().as_str() {
        "" | "y" | "yes" => Ok(CrucibleDecision::Approve),
        _ => Ok(CrucibleDecision::Cancel),
    }
}

/// Split a `provider` / `provider:model` spec into parts (empty model → `None`).
fn split_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((p, m)) if !m.is_empty() => (p, Some(m)),
        _ => (spec, None),
    }
}

/// The stakes-tier cap (USD) from config.
fn cap_usd_for(cfg: &CrucibleConfig, stakes: Stakes) -> f64 {
    match stakes {
        Stakes::Low => cfg.cap_low_usd,
        Stakes::Med => cfg.cap_med_usd,
        Stakes::High => cfg.cap_high_usd,
    }
}

/// Apply a `--judge` override to a convening plan: pin the aggregator and
/// RE-PRICE the roster with the ACTUAL judge, so the surfaced est cost is honest
/// and the tier cap is re-checked. The Assembler priced + cap-checked against
/// ITS chosen judge; a pinned judge can cost more, so without this the est line
/// would lie and the cap would be silently bypassed. The user pinned it
/// deliberately, so we surface a warning in `trims` and proceed (never silently
/// overspend, never silently mis-report).
fn apply_judge_override(
    plan: &mut AssemblyPlan,
    judge: &str,
    cfg: &CrucibleConfig,
    policy: &AssemblyPolicy,
) {
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
    let cap = cap_usd_for(cfg, plan.stakes);
    match est.certified_microcents() {
        Some(c) if (c as f64 / wcore_types::crucible::MICROCENTS_PER_USD) > cap => {
            plan.trims.push(format!(
                "WARNING: pinned judge est ${:.4} exceeds the ${cap:.4} {:?} cap",
                c as f64 / wcore_types::crucible::MICROCENTS_PER_USD,
                plan.stakes
            ))
        }
        None => plan
            .trims
            .push("WARNING: pinned judge is unpriceable — cost not bounded".to_string()),
        _ => {}
    }
}

/// Map the gate to a [`CouncilDecision`], honoring the force flags.
fn build_gate(args: &CrucibleArgs) -> CouncilDecision {
    if args.direct {
        return CouncilDecision::Direct {
            reason: "forced --direct".to_string(),
        };
    }
    if args.force_council {
        return CouncilDecision::Council {
            reason: "forced --force-council".to_string(),
            stakes: if args.deep { Stakes::High } else { Stakes::Med },
        };
    }
    let decision = classify_task(&args.task, &GateConfig::default());
    // --deep escalates a convened council to High (widest roster + top judge).
    if args.deep {
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

/// Build the Assembler policy from `[crucible]` config + CLI overrides.
fn build_policy(cfg: &CrucibleConfig, args: &CrucibleArgs) -> AssemblyPolicy {
    AssemblyPolicy {
        deny_families: args.deny.clone(),
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

/// What an auto run produced.
enum AutoRun {
    Direct { spec: String, text: String },
    Council(CouncilOutcome),
}

/// Execute an [`AssemblyPlan`]: a single direct call, or a built roster council.
async fn execute_plan(
    plan: &AssemblyPlan,
    task: &str,
    spawner: &AgentSpawner,
    base: &Config,
    cfg: &CrucibleConfig,
) -> anyhow::Result<AutoRun> {
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
                system_prompt: None,
                provider: Some(spec.clone()),
                model: spec.split_once(':').map(|(_, m)| m.to_string()),
            })
            .await;
        if result.is_error {
            anyhow::bail!("direct call failed: {}", result.text);
        }
        return Ok(AutoRun::Direct {
            spec,
            text: result.text,
        });
    }

    let roster = roster_from_plan(&plan.members, plan.aggregator.clone(), cfg);
    let outcome = run_council(task, &roster, spawner, base)
        .await
        .map_err(|e| anyhow::anyhow!("council failed: {e}"))?;
    Ok(AutoRun::Council(outcome))
}

/// Build a runnable [`Roster`] from chosen member specs. The auto budget cap was
/// enforced by the Assembler (judge-inclusive pre-flight) — and re-checked +
/// surfaced if a `--judge` override raised it — so the roster's own
/// `max_cost_usd` is left `None` to avoid a second, inconsistent (non-flux)
/// ceiling.
fn roster_from_plan(
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
    }
}

/// The gated direct path (MANUAL mode): answer with a single call on the first
/// roster member instead of convening the council.
async fn run_direct(
    task: &str,
    roster: &Roster,
    spawner: &AgentSpawner,
    reason: &str,
) -> anyhow::Result<()> {
    let first = roster
        .proposers
        .first()
        .expect("validated roster is non-empty");
    eprintln!(
        "crucible: direct mode ({reason}) — answering with '{}' instead of a council",
        first.spec
    );

    let result = spawner
        .spawn_one(SubAgentConfig {
            name: first.spec.clone(),
            prompt: task.to_string(),
            max_turns: roster.proposer_max_turns,
            max_tokens: DEFAULT_PROPOSER_MAX_TOKENS,
            system_prompt: None,
            provider: Some(first.spec.clone()),
            model: first.model.clone(),
        })
        .await;
    if result.is_error {
        anyhow::bail!("direct call failed: {}", result.text);
    }
    println!("{}", result.text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_agent::orchestration::council::{
        CouncilSpend, Proposal, ProviderSpend, SkippedProposer,
    };
    use wcore_types::message::TokenUsage;

    fn proposal(provider: &str, model: Option<&str>) -> Proposal {
        Proposal {
            provider: provider.to_string(),
            model: model.map(str::to_string),
            text: "answer".to_string(),
            is_error: false,
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
            latency_ms: 12,
        }
    }

    #[test]
    fn render_shows_skips_fusion_and_per_member_spend() {
        let outcome = CouncilOutcome {
            final_text: "FUSED".to_string(),
            proposals: vec![
                proposal("anthropic", Some("claude-opus-4-7")),
                proposal("openai", Some("gpt-5")),
            ],
            skipped: vec![SkippedProposer {
                spec: "vertex".to_string(),
                reason: "provider 'vertex' has no usable api key".to_string(),
            }],
            chosen_from: vec!["anthropic".to_string(), "openai".to_string()],
            spend: CouncilSpend {
                per_provider: vec![
                    ProviderSpend {
                        provider: "anthropic".to_string(),
                        model: Some("claude-opus-4-7".to_string()),
                        input_tokens: 100,
                        output_tokens: 50,
                        cost_microcents: 200_000,
                        priced: true,
                    },
                    ProviderSpend {
                        provider: "ollama".to_string(),
                        model: None,
                        input_tokens: 80,
                        output_tokens: 40,
                        cost_microcents: 0,
                        priced: false,
                    },
                ],
                total_input_tokens: 180,
                total_output_tokens: 90,
                total_cost_microcents: 200_000,
            },
        };

        let rendered = render_provenance(&outcome);

        assert!(rendered.contains(
            "crucible: skipped proposer 'vertex' (provider 'vertex' has no usable api key)"
        ));
        assert!(rendered.contains("crucible: fused 2 proposal(s) from [anthropic, openai]"));
        assert!(
            rendered
                .contains("crucible: spend = 180 in + 90 out tokens, ~$0.0020 across 2 member(s)")
        );
        assert!(
            rendered.contains("crucible:   anthropic (claude-opus-4-7): 100 in / 50 out → $0.0020")
        );
        assert!(rendered.contains("crucible:   ollama (?): 80 in / 40 out → unpriced"));
    }

    #[test]
    fn render_shows_non_contributing_members() {
        let mut errored = proposal("slow", Some("m"));
        errored.is_error = true;
        errored.text = "proposer timed out (per-proposer deadline)".to_string();
        errored.latency_ms = 1000;
        let outcome = CouncilOutcome {
            final_text: "FUSED".to_string(),
            proposals: vec![proposal("openai", Some("gpt-5")), errored],
            skipped: vec![],
            chosen_from: vec!["openai".to_string()],
            spend: CouncilSpend::default(),
        };
        let rendered = render_provenance(&outcome);
        assert!(rendered.contains(
            "crucible: member 'slow' did not contribute (proposer timed out (per-proposer deadline), 1000ms)"
        ));
    }

    #[test]
    fn render_handles_no_skips() {
        let outcome = CouncilOutcome {
            final_text: "x".to_string(),
            proposals: vec![],
            skipped: vec![],
            chosen_from: vec!["openai".to_string()],
            spend: CouncilSpend::default(),
        };
        let rendered = render_provenance(&outcome);
        assert!(!rendered.contains("skipped"));
        assert!(rendered.contains("crucible: fused 1 proposal(s) from [openai]"));
    }

    fn plan(convene: bool, stakes: Stakes) -> AssemblyPlan {
        AssemblyPlan {
            convene,
            members: vec![
                "openai:gpt-5".to_string(),
                "anthropic:claude-opus-4-7".to_string(),
            ],
            aggregator: convene.then(|| "anthropic:claude-opus-4-7".to_string()),
            est_cost_microcents: Some(200_000),
            stakes,
            reason: "test trace".to_string(),
            trims: vec![],
        }
    }

    #[test]
    fn render_assembly_plan_council_shows_members_judge_cost_reason() {
        let r = render_assembly_plan(&plan(true, Stakes::Med));
        assert!(r.contains(
            "auto-assembled Med council — 2 proposer(s): [openai:gpt-5, anthropic:claude-opus-4-7]"
        ));
        assert!(r.contains("aggregator = anthropic:claude-opus-4-7"));
        assert!(r.contains("est cost ~$0.0020"));
        assert!(r.contains("reason: test trace"));
    }

    #[test]
    fn render_assembly_plan_direct_path() {
        let mut p = plan(false, Stakes::Low);
        p.members = vec!["openai:gpt-5-mini".to_string()];
        let r = render_assembly_plan(&p);
        assert!(r.contains("auto → direct (Low) — openai:gpt-5-mini"));
    }

    #[test]
    fn render_assembly_plan_high_downgrade_is_surfaced() {
        // A High plan reduced to a Direct must carry the non-silent note (R3).
        let mut p = plan(false, Stakes::High);
        p.trims = vec!["judge↓ to x".to_string()];
        let r = render_assembly_plan(&p);
        assert!(r.contains("High-stakes plan was reduced"));
        assert!(r.contains("--deep"));
    }

    #[test]
    fn wants_auto_only_when_assembly_or_a_flag_is_set() {
        let manual = CrucibleConfig::default(); // assembly = Manual
        // Plain (or just --auto) stays manual.
        assert!(!wants_auto(&manual, &CrucibleArgs::default()));
        assert!(!wants_auto(
            &manual,
            &CrucibleArgs {
                auto: true,
                ..Default::default()
            }
        ));
        // Any assembly flag flips to auto.
        assert!(wants_auto(
            &manual,
            &CrucibleArgs {
                deep: true,
                ..Default::default()
            }
        ));
        assert!(wants_auto(
            &manual,
            &CrucibleArgs {
                deny: vec!["openai".to_string()],
                ..Default::default()
            }
        ));
        assert!(wants_auto(
            &manual,
            &CrucibleArgs {
                council: Some(vec!["openai:gpt-5".to_string()]),
                ..Default::default()
            }
        ));
        // assembly = "auto" flips it even with no flags.
        let auto = CrucibleConfig {
            assembly: AssemblyMode::Auto,
            ..Default::default()
        };
        assert!(wants_auto(&auto, &CrucibleArgs::default()));
    }

    #[test]
    fn judge_override_reprices_and_warns_when_over_cap() {
        // A cheap 1-proposer plan; pin an expensive judge under a tiny cap. The
        // est must be re-priced to the ACTUAL judge and a cap warning surfaced.
        let mut p = AssemblyPlan {
            convene: true,
            members: vec!["deepseek:deepseek-v4-pro".to_string()],
            aggregator: Some("deepseek:deepseek-v4-pro".to_string()),
            est_cost_microcents: Some(1),
            stakes: Stakes::Med,
            reason: "t".to_string(),
            trims: vec![],
        };
        let cfg = CrucibleConfig {
            cap_med_usd: 0.0001, // tiny → the opus judge will exceed it
            ..Default::default()
        };
        let policy = AssemblyPolicy {
            deny_families: vec![],
            max_proposers: 5,
            markup: 1.0,
            cap_low_usd: 0.02,
            cap_med_usd: 0.0001,
            cap_high_usd: 0.15,
            price_floor_frac: 0.25,
            proposer_max_turns: 4,
            proposer_max_tokens: 4096,
        };
        apply_judge_override(&mut p, "anthropic:claude-opus-4-7", &cfg, &policy);
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
    fn build_gate_honors_force_flags() {
        let direct = build_gate(&CrucibleArgs {
            direct: true,
            ..Default::default()
        });
        assert!(!direct.is_council());

        let forced = build_gate(&CrucibleArgs {
            force_council: true,
            ..Default::default()
        });
        assert!(forced.is_council());
        assert_eq!(forced.stakes(), Stakes::Med);

        let deep = build_gate(&CrucibleArgs {
            force_council: true,
            deep: true,
            ..Default::default()
        });
        assert_eq!(deep.stakes(), Stakes::High);
    }

    #[test]
    fn build_policy_max_tokens_matches_council_default() {
        // The card is priced against `proposer_max_tokens`; the council spawns each
        // proposer with `DEFAULT_PROPOSER_MAX_TOKENS`. They MUST stay equal or the
        // certified ceiling lies — a one-sided edit fails here.
        let policy = build_policy(&CrucibleConfig::default(), &CrucibleArgs::default());
        assert_eq!(
            policy.proposer_max_tokens,
            wcore_agent::orchestration::council::DEFAULT_PROPOSER_MAX_TOKENS
        );
    }

    use wcore_types::crucible::{CouncilMemberCard, CouncilRole, CruciblePlan};

    fn member(spec: &str, vendor: &str, role: CouncilRole) -> CouncilMemberCard {
        CouncilMemberCard {
            spec: spec.to_string(),
            vendor: vendor.to_string(),
            role,
        }
    }

    #[test]
    fn render_card_council_shows_roles_ceiling_baseline_and_judge() {
        let card = CruciblePlan {
            convene: true,
            members: vec![
                member(
                    "deepseek:deepseek-v4-pro",
                    "deepseek",
                    CouncilRole::Proposer,
                ),
                member("openai:gpt-5", "openai", CouncilRole::Proposer),
                member("anthropic:claude-opus-4-8", "anthropic", CouncilRole::Judge),
            ],
            stakes: "med".into(),
            focus: Some("c-suite".into()),
            ceiling_microcents: Some(210_000_000),
            single_model_baseline_microcents: Some(45_000_000),
            day_spent_microcents: None,
            day_cap_microcents: None,
            judge_independent: true,
            reason: "diverse cross-vendor".into(),
            trims: vec![],
        };
        let r = render_card(&card);
        assert!(r.contains("crucible plan (council)"));
        assert!(r.contains("stakes: med"));
        assert!(r.contains("focus: c-suite"));
        assert!(r.contains("proposer  deepseek:deepseek-v4-pro  (deepseek)"));
        assert!(r.contains("judge  anthropic:claude-opus-4-8  (anthropic)"));
        assert!(r.contains("ceiling ~ $2.1000"));
        assert!(r.contains("one strong model alone ~ $0.4500"));
        assert!(r.contains("judge: independent"));
        assert!(r.contains("reason: diverse cross-vendor"));
    }

    #[test]
    fn render_card_direct_plan_omits_judge_line() {
        let card = CruciblePlan {
            convene: false,
            members: vec![member("openai:gpt-5-mini", "openai", CouncilRole::Proposer)],
            stakes: "low".into(),
            focus: None,
            ceiling_microcents: Some(5_000_000),
            single_model_baseline_microcents: None,
            day_spent_microcents: None,
            day_cap_microcents: None,
            judge_independent: true,
            reason: "single model suffices".into(),
            trims: vec![],
        };
        let r = render_card(&card);
        assert!(r.contains("crucible plan (direct)"));
        assert!(r.contains("proposer  openai:gpt-5-mini  (openai)"));
        assert!(r.contains("ceiling ~ $0.0500"));
        // Direct plans never print a judge line and have no baseline here.
        assert!(!r.contains("judge:"));
        assert!(!r.contains("one strong model alone"));
    }

    #[test]
    fn render_card_unpriceable_ceiling_says_price_unknown_not_zero() {
        let card = CruciblePlan {
            convene: true,
            members: vec![member("flux:flux-pinned-x", "flux", CouncilRole::Proposer)],
            stakes: "high".into(),
            focus: None,
            ceiling_microcents: None,
            single_model_baseline_microcents: None,
            day_spent_microcents: None,
            day_cap_microcents: None,
            judge_independent: true,
            reason: "unpriced flux".into(),
            trims: vec![],
        };
        let r = render_card(&card);
        assert!(r.contains("ceiling: price unknown"));
        // The no-$0-surprise rule: an unpriceable ceiling never renders as money.
        assert!(!r.contains("$0"));
        assert!(!r.contains("ceiling ~"));
    }

    #[test]
    fn render_card_daily_line_present_only_with_cap() {
        let base = CruciblePlan {
            convene: false,
            members: vec![member("openai:gpt-5", "openai", CouncilRole::Proposer)],
            stakes: "low".into(),
            focus: None,
            ceiling_microcents: Some(1_000_000),
            single_model_baseline_microcents: None,
            day_spent_microcents: None,
            day_cap_microcents: Some(2_000_000_000),
            judge_independent: true,
            reason: "x".into(),
            trims: vec![],
        };
        // With a cap: the today line appears, spent defaulting to $0.0000.
        let r = render_card(&base);
        assert!(r.contains("today: $0.0000 / $20.0000"));
        // Without a cap: the today line is omitted entirely.
        let no_cap = CruciblePlan {
            day_cap_microcents: None,
            ..base
        };
        assert!(!render_card(&no_cap).contains("today:"));
    }
}
