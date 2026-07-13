//! `wayland-eval` — CLI driver for the scenario harness.
//!
//! F01a owns the canonical catalog and deterministic selection surface. The
//! execution/report path remains fail-closed until the later F01 slices.

use clap::Parser;
use wcore_eval_scenarios::catalog::{select_scenarios, standard_scenarios};

#[derive(Debug, Parser)]
#[command(
    name = "wayland-eval",
    about = "scenario eval harness for wayland-core"
)]
struct Cli {
    /// Print the selected scenario IDs, one per line, without executing them.
    #[arg(long)]
    list: bool,

    /// Select an exact scenario ID. Repeat to select multiple scenarios.
    #[arg(long, value_name = "ID", conflicts_with = "filter")]
    scenario: Vec<String>,

    /// Substring filter — only run scenarios whose name contains it.
    #[arg(long, conflicts_with = "scenario")]
    filter: Option<String>,

    /// Provider override — `deepseek` | `anthropic` | `openai`.
    #[arg(long, conflicts_with = "list")]
    provider: Option<String>,

    /// Strict mode (per cross-audit M-2): missing API keys become
    /// FAIL, not SKIP.
    #[arg(long, conflicts_with = "list")]
    strict: bool,

    /// Print the cost estimate and exit without calling any provider.
    #[arg(long, conflicts_with = "list")]
    dry: bool,

    /// Hard USD ceiling for the whole run.
    #[arg(long, conflicts_with = "list")]
    budget: Option<f64>,
}

fn main() {
    let cli = Cli::parse();
    let scenarios = match standard_scenarios()
        .and_then(|catalog| select_scenarios(catalog, &cli.scenario, cli.filter.as_deref()))
    {
        Ok(scenarios) => scenarios,
        Err(error) => {
            eprintln!("wayland-eval: {error}");
            std::process::exit(2);
        }
    };

    if cli.list {
        for scenario in scenarios {
            println!("{}", scenario.name);
        }
        return;
    }

    let _run_options = (cli.provider, cli.strict, cli.dry, cli.budget);
    eprintln!(
        "wayland-eval: execution is not wired yet; selected {} scenario(s)",
        scenarios.len()
    );
    std::process::exit(2);
}
