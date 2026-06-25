//! `wayland-core crucible "<task>"` — run the cross-provider council (Crucible /
//! Mixture-of-Providers).
//!
//! Self-contained one-shot handler (mirrors `workflow::run_workflow`): it loads
//! the on-disk `[crucible]` + `[providers]` blocks (which `Config::resolve`
//! drops), validates the roster, builds a resolver-carrying spawner, runs the
//! council, and prints the fused answer to stdout. Provenance + skipped members
//! go to stderr.

use std::sync::Arc;

use wcore_agent::orchestration::council::{
    CouncilProviderResolver, run_council, validate_and_build,
};
use wcore_agent::spawner::AgentSpawner;
use wcore_config::config::{CliArgs, Config, load_merged_config_file};

/// Run the council over `task`, printing the fused answer to stdout.
pub async fn run_crucible(task: &str) -> anyhow::Result<()> {
    // The `[crucible]` + `[providers]` blocks live on the on-disk ConfigFile,
    // which `Config::resolve` consumes — load the merged file directly.
    let cf = load_merged_config_file(None)?;

    if !cf.crucible.enabled {
        anyhow::bail!(
            "the Crucible council is disabled. Set `enabled = true` under `[crucible]` \
             in your config and list `proposers = [\"provider\", ...]`."
        );
    }

    // Hard validation at load — empty roster, bounds, malformed specs, unknown
    // aggregator all fail here rather than mid-run.
    let roster = validate_and_build(&cf.crucible)
        .map_err(|e| anyhow::anyhow!("invalid [crucible] config: {e}"))?;

    // A resolved base config + a default provider. Every council member is
    // pinned, so the spawner's own provider is only a never-used placeholder;
    // the base config carries the session policy surface + credential store.
    //
    // Resolve the base against the FIRST proposer's provider so the user only
    // needs their council providers keyed — not a separate session-default key.
    // (The base provider is a never-used placeholder since every council member
    // is pinned; it just has to resolve.)
    let base = {
        let provider = roster.proposers.first().map(|p| p.provider.clone());
        Config::resolve(&CliArgs {
            provider,
            ..CliArgs::default()
        })?
    };
    wcore_agent::egress::install_egress_policy(&base);
    let provider = wcore_agent::bootstrap::create_provider_with_oauth(&base)?;

    // The keyed resolver pulls each proposer/aggregator's OWN credentials from
    // the `[providers]` map.
    let resolver = CouncilProviderResolver::new(base.clone(), cf.providers.clone());
    let spawner =
        AgentSpawner::new(provider, base.clone()).with_provider_resolver(Arc::new(resolver));

    eprintln!(
        "crucible: convening {} proposer(s){}",
        roster.proposers.len(),
        roster
            .aggregator
            .as_deref()
            .map(|a| format!(", aggregator = {a}"))
            .unwrap_or_default()
    );

    let outcome = run_council(task, &roster, &spawner, &base)
        .await
        .map_err(|e| anyhow::anyhow!("council failed: {e}"))?;

    // Provenance + any skipped members to stderr; the answer to stdout.
    for s in &outcome.skipped {
        eprintln!("crucible: skipped proposer '{}' ({})", s.spec, s.reason);
    }
    eprintln!(
        "crucible: fused {} proposal(s) from [{}]",
        outcome.chosen_from.len(),
        outcome.chosen_from.join(", ")
    );

    // Spend rollup (stderr): total + per-member token/cost breakdown.
    let spend = &outcome.spend;
    eprintln!(
        "crucible: spend = {} in + {} out tokens, ~${:.4} across {} member(s)",
        spend.total_input_tokens,
        spend.total_output_tokens,
        spend.total_cost_usd(),
        spend.per_provider.len()
    );
    for ps in &spend.per_provider {
        let cost = if ps.priced {
            format!("${:.4}", ps.cost_microcents as f64 / 100_000_000.0)
        } else {
            "unpriced".to_string()
        };
        eprintln!(
            "crucible:   {} ({}): {} in / {} out → {cost}",
            ps.provider,
            ps.model.as_deref().unwrap_or("?"),
            ps.input_tokens,
            ps.output_tokens,
        );
    }

    println!("{}", outcome.final_text);
    Ok(())
}
