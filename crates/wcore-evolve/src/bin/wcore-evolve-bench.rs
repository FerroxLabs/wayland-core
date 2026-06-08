//! `wcore-evolve-bench` — M4.3 bench-driven GEPA entrypoint.
//!
//! Sibling to `wcore-evolve`, but swaps the structural `DefaultScorer`
//! for `wcore_eval::BenchScorer`, which grades candidates against the
//! M4.1 30-case mini-bench corpus (`crates/wcore-eval/data/bench/`).
//!
//! ## Why a separate binary?
//!
//! `DefaultScorer` and `BenchScorer` are complementary, not competing:
//!
//! - `DefaultScorer` (W10A) grades **skill metadata structure** — does
//!   the skill have a clear `when_to_use`, sensible `allowed_tools`,
//!   non-pathological body length, etc. Nine static checks.
//! - `BenchScorer` (M4.1) grades **end-to-end task outcomes** — runs
//!   every bench case through a `BenchRunner` and reports the pass
//!   ratio. Different consumers, different signals.
//!
//! Keeping them in distinct entrypoints means the existing
//! `wcore-evolve` smoke path is unchanged (no risk to W10B regression
//! tests) and the new bench-driven path is opt-in.
//!
//! ## Runner story
//!
//! `BenchRunner::run` is sync, while every real provider in this repo
//! is async. Plumbing async-into-sync correctly (blocking on a Tokio
//! runtime from inside a `BenchRunner::run` call dispatched by
//! `BenchScorer::outcomes`) is M4.4 / M4.5 territory. M4.3 ships a
//! **`CannedBenchRunner`**: a deterministic, file-driven runner that
//! mirrors the strategy's expected output for every case unless
//! overridden via `--canned-overrides`. That's enough to demonstrate
//! the bench-driven score actually flows through the GEPA loop and
//! drives candidate selection.
//!
//! Real LLM-backed runners plug in later by implementing `BenchRunner`
//! against an `Arc<dyn LlmProvider>` blocking shim; the bin's wiring
//! does not change.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use clap::Parser;

use wcore_eval::bench::{BenchCase, BenchCorpus, BenchMatchStrategy, BenchRunner, BenchScorer};
use wcore_eval::error::EvalError;
use wcore_evolve::curator_handoff::{CuratorPort, Decision, Handoff, Lineage};
use wcore_evolve::evolve::TerminationReason;
use wcore_evolve::generation::BudgetStub;
use wcore_evolve::mutator::{
    Mutator, Paraphrase, ParaphraseProvider, PassthroughParaphraseProvider, Precondition, Reorder,
    SwapSynonym,
};
use wcore_evolve::prompt_store::PromptStore;
use wcore_evolve::{EvolveParams, NullTraceSink, evolve};
use wcore_memory::db::Db;
use wcore_skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

/// Default eval-crate root — resolved at build time relative to this
/// crate's manifest dir, so `cargo run --bin wcore-evolve-bench` works
/// in a fresh checkout without flags. Overridable via
/// `--eval-crate-root` for downstream consumers that ship the corpus
/// somewhere else.
const DEFAULT_EVAL_CRATE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../wcore-eval");

#[derive(Parser)]
#[command(
    name = "wcore-evolve-bench",
    about = "M4.3 bench-driven GEPA evolution loop (BenchScorer over wcore-eval/data/bench)"
)]
struct EvolveBenchArgs {
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

    /// Plateau window. Default 3, MUST be >= number of mutator
    /// strategies in rotation (matches the W10B High 1 audit rule).
    #[arg(long, default_value_t = 3)]
    plateau_window: usize,

    /// Plateau min-delta.
    #[arg(long, default_value_t = 0.01)]
    plateau_min_delta: f64,

    /// Per-child wall-clock cap for mutator + score, in seconds.
    #[arg(long, default_value_t = 30)]
    child_timeout_secs: u64,

    /// Graveyard root directory. Defaults to
    /// `<data_dir>/wayland/evolve/graveyard` (see `wcore-evolve` for
    /// the per-OS path).
    #[arg(long)]
    graveyard_root: Option<PathBuf>,

    /// Run id used in trace events + graveyard path. Defaults to a
    /// process-id + millis stamp.
    #[arg(long)]
    run_id: Option<String>,

    /// Path to the `wcore-eval` crate root (the directory that
    /// contains `data/bench/`). Defaults to the sibling crate
    /// resolved at build time.
    #[arg(long, default_value = DEFAULT_EVAL_CRATE_ROOT)]
    eval_crate_root: PathBuf,

    /// Optional JSON file mapping `case_id` → forced output string.
    /// Cases not present in the file pass via the strategy's
    /// canonical output. Use this to perturb the bench and watch the
    /// GEPA loop's `parent_score` move.
    #[arg(long)]
    canned_overrides: Option<PathBuf>,

    /// Force one or more cases to fail by id, regardless of override
    /// file. Repeatable. Useful for quick smoke runs without crafting
    /// a JSON file. Output is the sentinel string `__force_fail__`.
    #[arg(long = "force-fail-case")]
    force_fail_cases: Vec<String>,
}

/// Canned-output runner. Returns:
///   1. The override from `--canned-overrides` if the case id is in
///      the map.
///   2. A sentinel string for `--force-fail-case` ids (never matches).
///   3. Otherwise the strategy's canonical pass output, mirroring the
///      test-side runner in `tests/bench_scorer_determinism.rs`.
struct CannedBenchRunner {
    overrides: HashMap<String, String>,
    force_fail: std::collections::HashSet<String>,
}

impl CannedBenchRunner {
    fn new(
        overrides: HashMap<String, String>,
        force_fail: std::collections::HashSet<String>,
    ) -> Self {
        Self {
            overrides,
            force_fail,
        }
    }

    /// Produce a string the strategy will accept. Mirrors the logic in
    /// `tests/bench_scorer_determinism.rs` so the canonical "all pass"
    /// shape stays in lock-step with the corpus YAML.
    fn pass_output(case: &BenchCase) -> String {
        match &case.frontmatter.match_strategy {
            BenchMatchStrategy::Exact { expected } => expected.clone(),
            BenchMatchStrategy::ContainsAll { tokens } => {
                let mut out = String::with_capacity(64);
                out.push_str("[runner:");
                out.push_str(&case.frontmatter.id);
                out.push_str("] ");
                for t in tokens {
                    out.push_str(t);
                    out.push(' ');
                }
                out
            }
            BenchMatchStrategy::NumericEqual { expected, .. } => format!("{expected}"),
            BenchMatchStrategy::FileTreeMatches {
                expected_sha256, ..
            } => expected_sha256.clone(),
        }
    }
}

impl BenchRunner for CannedBenchRunner {
    fn run(&self, case: &BenchCase) -> Result<String, EvalError> {
        let id = &case.frontmatter.id;
        if self.force_fail.contains(id) {
            return Ok("__force_fail__".to_string());
        }
        if let Some(v) = self.overrides.get(id) {
            return Ok(v.clone());
        }
        Ok(Self::pass_output(case))
    }
}

fn resolve_graveyard_root(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("wayland")
            .join("evolve")
            .join("graveyard")
    })
}

/// Logging adapter that records every winner. Matches the `wcore-evolve`
/// bin so the CLI surface is consistent between the two entrypoints.
/// Real curator wiring lands in W11+.
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

fn load_canned_overrides(path: &std::path::Path) -> Result<HashMap<String, String>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("read canned-overrides {}: {e}", path.display()))?;
    let map: HashMap<String, String> = serde_json::from_str(&raw)
        .map_err(|e| format!("parse canned-overrides {}: {e}", path.display()))?;
    Ok(map)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = EvolveBenchArgs::parse();

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

    let corpus = match BenchCorpus::load(&args.eval_crate_root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "error: failed to load bench corpus from {}: {e}",
                args.eval_crate_root.display()
            );
            return ExitCode::from(2);
        }
    };

    let overrides = if let Some(p) = args.canned_overrides.as_deref() {
        match load_canned_overrides(p) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        HashMap::new()
    };

    let force_fail: std::collections::HashSet<String> =
        args.force_fail_cases.iter().cloned().collect();

    let runner: Arc<dyn BenchRunner> = Arc::new(CannedBenchRunner::new(overrides, force_fail));
    let scorer = Arc::new(BenchScorer::new(corpus, runner));

    let parent_id = seed_skill.name.clone();
    let run_id = args.run_id.clone().unwrap_or_else(|| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("bench-run-{}-{}", std::process::id(), now)
    });

    // M4.3 does not exercise the Paraphrase LLM provider in the
    // default smoke path — the bench-driven path is about validating
    // that scorer-substitution works end-to-end, not stress-testing
    // mutators. PassthroughParaphraseProvider keeps the run offline
    // and deterministic.
    let paraphrase_provider: Arc<dyn ParaphraseProvider> = Arc::new(PassthroughParaphraseProvider);
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
        scorer,
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
        } => format!("score_invalid(generation={generation},score_bits={score_bits:#x})"),
    };

    // M5.7 carryover #4 — persist the winning variant into the global
    // tier's `evolved_prompts` table so future runs (and the M3.6
    // skills prioritizer) can bootstrap the seed pool with past
    // winners. Resolves the global Db via paths::global_db_path so
    // cross-run persistence works; if the path is unresolvable (rare
    // CI envs without HOME) we log a warn and skip.
    match wcore_memory::paths::global_db_path() {
        Some(p) => match Db::open_global(p) {
            Ok(db) => {
                let store = PromptStore::new(Arc::new(db));
                if let Err(e) = store.record_outcome(&parent_id, "bench", &outcome) {
                    eprintln!("warn: PromptStore.record_outcome failed: {e}");
                }
            }
            Err(e) => eprintln!("warn: PromptStore disabled — Db::open_global failed: {e}"),
        },
        None => eprintln!("warn: PromptStore disabled — no global memory dir resolvable"),
    }

    println!("scorer=bench");
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
