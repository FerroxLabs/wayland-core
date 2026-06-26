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
    AssemblyPlan, CouncilApprover, CouncilDecision, CouncilOutcome, CouncilOverrides,
    CouncilProviderResolver, CouncilRunResult, DEFAULT_PROPOSER_MAX_TOKENS, GateConfig, Roster,
    Stakes, classify_task, drive_council, log_assembly, run_council, validate_and_build,
};
use wcore_agent::spawner::{AgentSpawner, SubAgentConfig};
use wcore_config::config::{CliArgs, Config, ConfigFile, load_merged_config_file};
use wcore_config::crucible::{AssemblyMode, CrucibleConfig};
use wcore_types::crucible::CrucibleDecision;

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

    // Roster-selection overrides for the shared driver (the task is passed
    // separately). An edited roster is re-resolved to runnable specs through a
    // fresh resolver — the original was moved into the spawner's Arc.
    let ov = CouncilOverrides {
        council: args.council.clone(),
        judge: args.judge.clone(),
        direct: args.direct,
        force_council: args.force_council,
        deep: args.deep,
        deny: args.deny.clone(),
    };
    let refilter = {
        let base = base.clone();
        let providers = cf.providers.clone();
        move |specs: &[String]| {
            CouncilProviderResolver::new(base.clone(), providers.clone()).resolvable_specs(specs)
        }
    };

    match drive_council(
        &args.task,
        runnable,
        &base,
        &cf.crucible,
        &ov,
        &spawner,
        &TtyApprover {
            auto_spend: cf.crucible.crucible_auto_spend,
        },
        &refilter,
    )
    .await?
    {
        CouncilRunResult::Direct { spec, text } => {
            eprintln!("crucible: direct answer via {spec}");
            println!("{text}");
        }
        CouncilRunResult::Council { plan, outcome } => {
            // Privacy-safe preference signal (opt-in; family-mix + cost only).
            log_assembly(&plan, &outcome.spend, &cf.crucible, None);
            eprint!("{}", render_provenance(&outcome));
            println!("{}", outcome.final_text);
        }
        CouncilRunResult::Cancelled => {
            eprintln!("crucible: cancelled — no spend.");
        }
    }
    Ok(())
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

/// The CLI's TTY/headless decision surface for the shared [`drive_council`]
/// driver. Renders the proposal card to stderr, then: interactive when stdin is a
/// TTY (prompt Y/n); otherwise fail-closed unless `auto_spend` is set. The driver
/// only re-assembles on Edit/ApprovePremium — a TTY never returns those, and a
/// non-TTY only Approves (with `auto_spend`) or errors, so the CLI's behavior is
/// byte-identical to the prior inline `decide` loop.
struct TtyApprover {
    auto_spend: bool,
}

#[async_trait::async_trait]
impl CouncilApprover for TtyApprover {
    async fn approve(
        &self,
        card: &wcore_types::crucible::CruciblePlan,
    ) -> anyhow::Result<CrucibleDecision> {
        use std::io::{IsTerminal, Write};
        eprint!("{}", render_card(card));
        if !std::io::stdin().is_terminal() {
            if self.auto_spend {
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
    fn build_policy_max_tokens_matches_council_default() {
        // The card is priced against `proposer_max_tokens`; the council spawns each
        // proposer with `DEFAULT_PROPOSER_MAX_TOKENS`. They MUST stay equal or the
        // certified ceiling lies. `build_policy` now lives in the shared driver; the
        // CLI just confirms the wiring still pins the council default.
        let policy = wcore_agent::orchestration::council::build_policy(
            &CrucibleConfig::default(),
            &CouncilOverrides::default(),
        );
        assert_eq!(policy.proposer_max_tokens, DEFAULT_PROPOSER_MAX_TOKENS);
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
