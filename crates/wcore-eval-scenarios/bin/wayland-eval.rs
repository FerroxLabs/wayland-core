//! `wayland-eval` — CLI driver for the scenario harness.
//!
//! **T5** owns the real implementation (filter / provider / dry-run
//! / strict / budget flags; console + JSON + Markdown reports). T1/T2
//! ship this stub so the binary compiles, the `[[bin]]` target lands
//! in the workspace, and `just eval` has a callable entry point to
//! shape against.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "wayland-eval",
    about = "scenario eval harness for wayland-core"
)]
struct Cli {
    /// Substring filter — only run scenarios whose name contains it.
    #[arg(long)]
    filter: Option<String>,

    /// Provider override — `deepseek` | `anthropic` | `openai` (T5).
    #[arg(long)]
    provider: Option<String>,

    /// Strict mode (per cross-audit M-2): missing API keys become
    /// FAIL, not SKIP.
    #[arg(long)]
    strict: bool,

    /// Print the cost estimate and exit without calling any provider.
    #[arg(long)]
    dry: bool,

    /// Hard USD ceiling for the whole run. T5 enforces; this stub
    /// just parses it.
    #[arg(long)]
    budget: Option<f64>,
}

fn main() {
    let _cli = Cli::parse();
    eprintln!(
        "wayland-eval: T5 will wire the real driver. \
         T1/T2 ship the runner core + scaffold only — see \
         crates/wcore-eval-scenarios/src/lib.rs and .blackboard/EVAL-HARNESS-PLAN-2026-05-23.md."
    );
    std::process::exit(2);
}
