//! `wayland-eval` — exact-artifact scenario evaluation driver.

use std::path::PathBuf;

use clap::Parser;
use wcore_eval_scenarios::Scenario;
use wcore_eval_scenarios::artifact::{ArtifactExpectation, inspect_binary, select_candidate};
use wcore_eval_scenarios::catalog::{select_scenarios, standard_scenarios};
use wcore_eval_scenarios::providers::{
    ProviderAvailability, ProviderResolution, provider_override, resolve,
};
use wcore_eval_scenarios::runner::run_with_binary;

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

    /// Exact wayland-core binary to evaluate. Overrides WCORE_EVAL_BIN.
    #[arg(long, value_name = "PATH", conflicts_with = "list")]
    binary: Option<PathBuf>,

    /// Full 40-hex source commit the selected binary must report.
    #[arg(long, value_name = "SHA", conflicts_with = "list")]
    expected_source_commit: Option<String>,

    /// Missing required providers become failures instead of skips.
    #[arg(long, conflicts_with = "list")]
    strict: bool,

    /// Print the cost estimate and exit without calling any provider.
    #[arg(long, conflicts_with = "list")]
    dry: bool,

    /// Hard USD ceiling for the whole run.
    #[arg(long, conflicts_with = "list")]
    budget: Option<f64>,
}

#[tokio::main]
async fn main() {
    let code = execute(Cli::parse()).await;
    if code != 0 {
        std::process::exit(code);
    }
}

async fn execute(cli: Cli) -> i32 {
    let scenarios = match standard_scenarios()
        .and_then(|catalog| select_scenarios(catalog, &cli.scenario, cli.filter.as_deref()))
    {
        Ok(scenarios) => scenarios,
        Err(error) => return usage_error(error),
    };

    if cli.list {
        for scenario in scenarios {
            println!("{}", scenario.name);
        }
        return 0;
    }
    if cli.dry {
        return usage_error("--dry cost estimation is not implemented");
    }
    if cli.budget.is_some_and(|budget| budget < 0.0) {
        return usage_error("--budget must be non-negative");
    }

    let environment_provider = std::env::var("WCORE_EVAL_PROVIDER").ok();
    let provider_override =
        match provider_override(cli.provider.as_deref(), environment_provider.as_deref()) {
            Ok(provider) => provider,
            Err(error) => return usage_error(error),
        };
    let availability = ProviderAvailability::from_environment();
    let mut plans = Vec::with_capacity(scenarios.len());
    let mut runnable_count = 0usize;
    for scenario in scenarios {
        let strict = cli.strict || scenario.strict;
        let resolution = match resolve(scenario.provider, provider_override, availability, strict) {
            Ok(resolution) => resolution,
            Err(error) => return usage_error(format!("{}: {error}", scenario.name)),
        };
        runnable_count += resolution.runnable.len();
        plans.push((scenario, resolution));
    }

    if runnable_count == 0 {
        let skipped = print_skips(&plans);
        println!("SUMMARY pass=0 fail=0 skip={skipped}");
        return 0;
    }

    let Some(expected_source_commit) = cli.expected_source_commit.as_deref() else {
        return usage_error("--expected-source-commit is required when scenarios are runnable");
    };
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(workspace_root) = manifest_dir.parent().and_then(|path| path.parent()) else {
        return usage_error("could not resolve workspace root");
    };
    let environment_binary = std::env::var_os("WCORE_EVAL_BIN");
    let candidate = match select_candidate(
        cli.binary.as_deref(),
        environment_binary.as_deref(),
        workspace_root,
    ) {
        Ok(candidate) => candidate,
        Err(error) => return usage_error(error),
    };
    let artifact = match inspect_binary(
        &candidate,
        ArtifactExpectation {
            version: env!("CARGO_PKG_VERSION"),
            source_commit: expected_source_commit,
        },
    ) {
        Ok(artifact) => artifact,
        Err(error) => return usage_error(error),
    };

    let mut passed = 0usize;
    let mut failed = 0usize;
    let skipped = print_skips(&plans);
    let mut total_cost = 0.0;
    'scenarios: for (scenario, resolution) in &plans {
        for provider in &resolution.runnable {
            match run_with_binary(scenario, provider, &artifact.path).await {
                Ok(result) if result.passed => {
                    total_cost += result.cost_usd;
                    passed += 1;
                    println!("PASS {} {}", scenario.name, provider.id);
                }
                Ok(result) => {
                    total_cost += result.cost_usd;
                    failed += 1;
                    println!(
                        "FAIL {} {} failures={}",
                        scenario.name,
                        provider.id,
                        result.failures.len()
                    );
                }
                Err(error) => {
                    failed += 1;
                    println!("FAIL {} {} runner={error}", scenario.name, provider.id);
                }
            }
            if let Some(budget) = cli.budget
                && total_cost > budget
            {
                failed += 1;
                eprintln!("wayland-eval: run cost ${total_cost:.6} exceeded --budget ${budget:.6}");
                break 'scenarios;
            }
        }
    }

    println!("SUMMARY pass={passed} fail={failed} skip={skipped}");
    i32::from(failed > 0)
}

fn print_skips(plans: &[(Scenario, ProviderResolution)]) -> usize {
    let mut count = 0;
    for (scenario, resolution) in plans {
        for skip in &resolution.skipped {
            println!(
                "SKIP {} {} missing={}",
                scenario.name, skip.provider, skip.missing_key
            );
            count += 1;
        }
    }
    count
}

fn usage_error(error: impl std::fmt::Display) -> i32 {
    eprintln!("wayland-eval: {error}");
    2
}
