//! `wcore-evolve` — CLI front-end to the W10B GEPA loop.
//!
//! Reads a seed skill (markdown file with frontmatter), runs the GEPA loop
//! against the W10A `DefaultScorer`, archives losers to the graveyard, and
//! prints an outcome summary. The seed is NOT modified in place — winners
//! flow through the `CuratorPort` boundary (the bin wires a logging adapter
//! so the smoke test stays offline; real curator integration is W11+).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use clap::Parser;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_eval::DefaultScorer;
use wcore_evolve::curator_handoff::{CuratorPort, Decision, Handoff, Lineage};
use wcore_evolve::evolve::TerminationReason;
use wcore_evolve::generation::BudgetStub;
use wcore_evolve::mutator::{
    LlmParaphraseProvider, Mutator, Paraphrase, ParaphraseProvider, PassthroughParaphraseProvider,
    Precondition, Reorder, SwapSynonym,
};
use wcore_evolve::{EvolveParams, NullTraceSink, evolve};
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;
use wcore_skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

#[derive(Parser)]
#[command(name = "wcore-evolve", about = "W10B F12 GEPA evolution loop")]
struct EvolveArgs {
    /// Path to the seed skill markdown file (frontmatter + body).
    #[arg(long)]
    seed_file: PathBuf,

    /// Stable id used as `parent_id` in lineage + graveyard entries.
    #[arg(long, default_value = "seed")]
    seed_name: String,

    /// Maximum generations to run.
    #[arg(long, default_value_t = 5)]
    generations: u32,

    /// Fan-out per generation.
    #[arg(long, default_value_t = 4)]
    fan_out: u32,

    /// Plateau window (default 3; MUST be >= number of mutator strategies in
    /// rotation per High 1 audit fix).
    #[arg(long, default_value_t = 3)]
    plateau_window: usize,

    /// Plateau min-delta.
    #[arg(long, default_value_t = 0.01)]
    plateau_min_delta: f64,

    /// Per-child wall-clock cap for mutator + score, in seconds (default 30).
    #[arg(long, default_value_t = 30)]
    child_timeout_secs: u64,

    /// Graveyard root directory. Loser candidates land at
    /// `<graveyard_root>/<run-id>/<generation>/<child>.json`.
    ///
    /// Default: `dirs::data_dir().unwrap_or_else(std::env::temp_dir)
    ///   .join("wayland/evolve/graveyard")` — resolves to
    ///   `~/Library/Application Support/wayland/evolve/graveyard` on macOS,
    ///   `~/.local/share/wayland/evolve/graveyard` on Linux,
    ///   `%APPDATA%\wayland\evolve\graveyard` on Windows.
    /// The path is created with `fs::create_dir_all` on first use.
    #[arg(long)]
    graveyard_root: Option<PathBuf>,

    /// Run id used in trace events + graveyard path. Defaults to a fresh UUID.
    #[arg(long)]
    run_id: Option<String>,

    /// Anthropic model to use for the Paraphrase mutator. When set, the bin
    /// wires a real `LlmParaphraseProvider` around an `AnthropicProvider`
    /// authenticated via `ANTHROPIC_API_KEY`. When unset (default), the bin
    /// stays offline with `PassthroughParaphraseProvider`.
    #[arg(long)]
    llm_paraphrase_model: Option<String>,

    /// Override the Anthropic base URL. Only used when
    /// `--llm-paraphrase-model` is set. Defaults to the public endpoint.
    #[arg(long, default_value = "https://api.anthropic.com")]
    llm_paraphrase_base_url: String,
}

fn resolve_graveyard_root(explicit: Option<PathBuf>) -> PathBuf {
    // High 2 audit fix: well-defined default for every platform.
    explicit.unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("wayland")
            .join("evolve")
            .join("graveyard")
    })
}

/// Logging adapter: records every winner submitted by the loop. The W9 F11
/// curator wiring is a separate integration job (W11+); this lets the CLI
/// surface land without the dep.
struct LoggingCurator;

#[async_trait]
impl CuratorPort for LoggingCurator {
    async fn submit(&self, body: &str, lineage: &Lineage) -> Result<Decision, String> {
        eprintln!(
            "curator: received candidate run_id={} parent_id={} child_index={} \
             mutation_kind={} score={:.3} body_len={}",
            lineage.run_id,
            lineage.parent_id,
            lineage.child_index,
            lineage.mutation_kind,
            lineage.score,
            body.len()
        );
        Ok(Decision::Promote)
    }
}

fn parse_seed_skill(path: &std::path::Path, name: &str) -> Result<SkillMetadata, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read seed file: {e}"))?;
    // Minimal parse: split off frontmatter if present, treat the rest as
    // body content. The W10A harness loads its skills the same way.
    let body = match raw.strip_prefix("---\n") {
        Some(rest) => match rest.split_once("\n---\n") {
            Some((_fm, body)) => body.to_string(),
            None => raw.clone(),
        },
        None => raw.clone(),
    };
    let body_trimmed = body.trim_end_matches('\n').to_string();
    let content_length = body_trimmed.len();
    Ok(SkillMetadata {
        name: name.to_string(),
        display_name: None,
        description: String::new(),
        has_user_specified_description: false,
        allowed_tools: vec![],
        argument_hint: None,
        argument_names: vec![],
        when_to_use: None,
        version: None,
        model: None,
        disable_model_invocation: false,
        user_invocable: true,
        execution_context: ExecutionContext::Inline,
        agent: None,
        effort: None,
        shell: None,
        paths: vec![],
        artifacts: vec![],
        hooks_raw: None,
        source: SkillSource::Bundled,
        loaded_from: LoadedFrom::Bundled,
        content_length,
        content: body_trimmed,
        skill_root: None,
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = EvolveArgs::parse();
    let graveyard_root = resolve_graveyard_root(args.graveyard_root.clone());
    if let Err(e) = std::fs::create_dir_all(&graveyard_root) {
        eprintln!(
            "error: could not create graveyard root {}: {e}",
            graveyard_root.display()
        );
        return ExitCode::from(2);
    }

    let seed_skill = match parse_seed_skill(&args.seed_file, &args.seed_name) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let parent_id = seed_skill.name.clone();
    let run_id = args.run_id.clone().unwrap_or_else(|| {
        // Lightweight unique id without pulling in `uuid` here — process id
        // + wall-clock millis is enough for one-shot smoke runs.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("run-{}-{}", std::process::id(), now)
    });

    // Wave PA / W10B.1: when `--llm-paraphrase-model` is set, wire the real
    // `LlmParaphraseProvider` around an `AnthropicProvider`. Otherwise stay
    // offline with `PassthroughParaphraseProvider` so the smoke run keeps
    // working without network.
    let paraphrase_provider: Arc<dyn ParaphraseProvider> =
        if let Some(model) = args.llm_paraphrase_model.clone() {
            let api_key = match std::env::var("ANTHROPIC_API_KEY") {
                Ok(k) if !k.is_empty() => k,
                _ => {
                    eprintln!(
                        "error: --llm-paraphrase-model set but ANTHROPIC_API_KEY is unset or empty"
                    );
                    return ExitCode::from(2);
                }
            };
            let provider: Arc<dyn LlmProvider> = Arc::new(AnthropicProvider::new(
                &api_key,
                &args.llm_paraphrase_base_url,
                ProviderCompat::anthropic_defaults(),
                DebugConfig::default(),
            ));
            Arc::new(LlmParaphraseProvider::new(provider, model))
        } else {
            Arc::new(PassthroughParaphraseProvider)
        };
    let mutators: Vec<Arc<dyn Mutator>> = vec![
        Arc::new(Paraphrase {
            provider: paraphrase_provider,
            temperature: 0.0,
        }),
        Arc::new(Reorder),
        Arc::new(SwapSynonym),
        Arc::new(Precondition),
    ];

    let params = EvolveParams {
        seed_skill,
        max_generations: args.generations,
        fan_out: args.fan_out,
        plateau_window: args.plateau_window,
        plateau_min_delta: args.plateau_min_delta,
        budget: Arc::new(BudgetStub::unbounded()),
        graveyard_root: graveyard_root.clone(),
        run_id: run_id.clone(),
        run_seed: run_id.clone(),
        child_timeout: Duration::from_secs(args.child_timeout_secs),
        scorer: Arc::new(DefaultScorer::default()),
        mutators,
        trace_sink: Arc::new(NullTraceSink),
    };

    let outcome = match evolve(params).await {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: evolve failed: {e}");
            return ExitCode::from(1);
        }
    };

    let baseline = outcome.parent_score.dimensions.combined;
    let best = outcome
        .best_candidate
        .as_ref()
        .map(|c| c.score.dimensions.combined)
        .unwrap_or(baseline);
    let termination = match &outcome.termination {
        TerminationReason::GenerationCeiling => "generation_ceiling".to_string(),
        TerminationReason::Plateau { window, min_delta } => {
            format!("plateau(window={window},min_delta={min_delta})")
        }
        TerminationReason::BudgetExhausted => "budget_exhausted".to_string(),
        TerminationReason::NoImprovementFound => "no_improvement".to_string(),
        TerminationReason::ScoreInvalid {
            generation,
            score_bits,
        } => {
            // Wave RC (audit MAJOR #9) — scorer produced a non-finite
            // top score; loop broke out rather than spinning.
            format!("score_invalid(generation={generation},score_bits={score_bits:#x})")
        }
    };

    println!("run_id={run_id}");
    println!("parent_id={parent_id}");
    println!("generations_run={}", outcome.generations_run);
    println!("termination={termination}");
    println!("parent_score={baseline:.3}");
    println!("best_score={best:.3}");
    println!("losers_archived={}", outcome.all_scored.len());
    println!("graveyard_root={}", graveyard_root.display());

    if let Some(winner) = outcome.best_candidate.as_ref() {
        let handoff = Handoff::new(Arc::new(LoggingCurator));
        match handoff.promote(winner, &parent_id, &run_id).await {
            Ok(Decision::Promote) => println!("curator_decision=promote"),
            Ok(Decision::Archive) => println!("curator_decision=archive"),
            Err(e) => {
                eprintln!("curator hand-off failed: {e}");
                return ExitCode::from(1);
            }
        }
    } else {
        println!("curator_decision=skipped_no_winner");
    }

    ExitCode::SUCCESS
}
