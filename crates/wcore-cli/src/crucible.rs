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
    CouncilDecision, CouncilOutcome, CouncilProviderResolver, GateConfig, Roster, classify_task,
    run_council, validate_and_build,
};
use wcore_agent::spawner::{AgentSpawner, SubAgentConfig};
use wcore_config::config::{CliArgs, Config, load_merged_config_file};

/// Per-proposer output-token budget for the gated direct path (mirrors the
/// council executor's per-proposer cap).
const DIRECT_MAX_TOKENS: u32 = 4096;

/// The pricing catalog reports microcents; 1 USD = 100¢ = 100_000_000 µ¢.
const MICROCENTS_PER_USD: f64 = 100_000_000.0;

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
            format!("${:.4}", ps.cost_microcents as f64 / MICROCENTS_PER_USD)
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

/// Run the council over `task`, printing the fused answer to stdout. When
/// `auto` is set, a cheap gate first decides whether the task warrants a council
/// at all — a trivial ask is answered with a single direct call instead.
pub async fn run_crucible(task: &str, auto: bool) -> anyhow::Result<()> {
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

    // Council gate (`--auto`): a cheap, deterministic classifier decides whether
    // this task warrants the council's N× spend. A trivial ask is answered with
    // a single direct call on the first proposer; only a high-stakes / complex
    // task convenes the full council. Without `--auto` the council always runs.
    if auto && let CouncilDecision::Direct { reason } = classify_task(task, &GateConfig::default())
    {
        return run_direct(task, &roster, &spawner, &reason).await;
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

    let outcome = run_council(task, &roster, &spawner, &base)
        .await
        .map_err(|e| anyhow::anyhow!("council failed: {e}"))?;

    // Provenance + spend to stderr; the fused answer to stdout.
    eprint!("{}", render_provenance(&outcome));

    println!("{}", outcome.final_text);
    Ok(())
}

/// The gated direct path: answer with a single call on the first roster member
/// instead of convening the council. Used when `--auto` classifies the task as
/// not worth the council premium.
async fn run_direct(
    task: &str,
    roster: &Roster,
    spawner: &AgentSpawner,
    reason: &str,
) -> anyhow::Result<()> {
    // `validate_and_build` guarantees a non-empty roster.
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
            max_tokens: DIRECT_MAX_TOKENS,
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

        // Skip line names the member and its reason.
        assert!(rendered.contains(
            "crucible: skipped proposer 'vertex' (provider 'vertex' has no usable api key)"
        ));
        // Fusion line lists the fused providers.
        assert!(rendered.contains("crucible: fused 2 proposal(s) from [anthropic, openai]"));
        // Spend summary: totals + member count + USD (200_000 µ¢ = $0.0020).
        assert!(
            rendered
                .contains("crucible: spend = 180 in + 90 out tokens, ~$0.0020 across 2 member(s)")
        );
        // Priced member shows a dollar figure; unpriced member shows "unpriced".
        assert!(
            rendered.contains("crucible:   anthropic (claude-opus-4-7): 100 in / 50 out → $0.0020")
        );
        assert!(rendered.contains("crucible:   ollama (?): 80 in / 40 out → unpriced"));
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
}
