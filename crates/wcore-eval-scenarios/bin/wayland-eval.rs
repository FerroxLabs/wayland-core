//! `wayland-eval` — exact-artifact scenario evaluation driver.

use std::path::{Path, PathBuf};

use clap::Parser;
use sha2::{Digest, Sha256};
use wcore_eval_scenarios::Scenario;
use wcore_eval_scenarios::artifact::{
    ArtifactExpectation, SealedBinaryArtifact, seal_binary, select_candidate,
    verify_artifact_digest,
};
use wcore_eval_scenarios::catalog::{select_scenarios, standard_scenarios};
use wcore_eval_scenarios::fixtures::manifest::BoundCompositeFixtureManifest;
use wcore_eval_scenarios::providers::{
    ProviderAvailability, ProviderConfig, ProviderResolution, provider_override, resolve,
};
use wcore_eval_scenarios::receipt::{
    Evidence, EvidenceReceiptV1, ReceiptMetadataV1, ReceiptVerifier, VerificationPolicy,
};
use wcore_eval_scenarios::report::{ReceiptReports, render_receipt_reports};
use wcore_eval_scenarios::runner::{Failure, run_with_binary};
use wcore_eval_scenarios::scenario::{Platform, PlatformDisposition};

#[derive(Debug, Parser)]
#[command(
    name = "wayland-eval",
    about = "scenario eval harness for wayland-core"
)]
struct Cli {
    /// Print the selected scenario IDs, one per line, without executing them.
    #[arg(long)]
    list: bool,

    /// Validate and print the selected binary identity without running scenarios.
    #[arg(long, conflicts_with = "list")]
    verify_binary: bool,

    /// Select an exact scenario ID. Repeat to select multiple scenarios.
    #[arg(long, value_name = "ID", conflicts_with = "filter")]
    scenario: Vec<String>,

    /// Substring filter — only run scenarios whose name contains it.
    #[arg(long, conflicts_with = "scenario")]
    filter: Option<String>,

    /// Provider override — `deepseek` | `anthropic` | `openai` | `matrix`.
    #[arg(long, conflicts_with = "list")]
    provider: Option<String>,

    /// Exact wayland-core binary to evaluate. Overrides WCORE_EVAL_BIN.
    #[arg(long, value_name = "PATH", conflicts_with = "list")]
    binary: Option<PathBuf>,

    /// Provider API root override, used by deterministic loopback evaluations.
    #[arg(
        long,
        value_name = "URL",
        requires = "provider",
        conflicts_with = "list"
    )]
    base_url: Option<String>,

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

    /// Atomically persist the same status lines written to stdout.
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,

    /// Atomically persist a redacted local receipt and all report projections
    /// under one subdirectory per executed scenario/provider cell.
    #[arg(
        long,
        value_name = "DIR",
        conflicts_with_all = ["list", "verify_binary", "dry"]
    )]
    report_dir: Option<PathBuf>,

    /// Content-addressed deterministic fixture manifest bound into receipts.
    #[arg(
        long,
        value_name = "PATH",
        requires = "report_dir",
        conflicts_with_all = ["list", "verify_binary", "dry"]
    )]
    fixture_manifest: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    let code = execute(Cli::parse()).await;
    if code != 0 {
        std::process::exit(code);
    }
}

async fn execute(cli: Cli) -> i32 {
    let mut status = StatusOutput::default();
    let scenarios = match standard_scenarios()
        .and_then(|catalog| select_scenarios(catalog, &cli.scenario, cli.filter.as_deref()))
    {
        Ok(scenarios) => scenarios,
        Err(error) => return usage_error(error),
    };

    if cli.list {
        for scenario in scenarios {
            status.line(scenario.name);
        }
        return finish_output(&cli, &status, 0);
    }
    if cli.verify_binary {
        let code = match inspect_cli_artifact(&cli) {
            Ok(artifact) => {
                status.line(format!(
                    "VERIFIED sha256={} version={} source={} path={}",
                    artifact.sha256,
                    artifact.version,
                    artifact.source_commit,
                    artifact.path.display()
                ));
                0
            }
            Err(error) => usage_error(error),
        };
        return finish_output(&cli, &status, code);
    }
    if cli
        .budget
        .is_some_and(|budget| !budget.is_finite() || budget < 0.0)
    {
        return usage_error("--budget must be finite and non-negative");
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
        let platform = match scenario.resolve_platform(Platform::current(), strict) {
            Ok(platform) => platform,
            Err(error) => return usage_error(error),
        };
        if !matches!(platform, PlatformDisposition::Runnable) {
            plans.push((
                scenario,
                ProviderResolution {
                    runnable: Vec::new(),
                    skipped: Vec::new(),
                },
                platform,
            ));
            continue;
        }
        let mut resolution =
            match resolve(scenario.provider, provider_override, availability, strict) {
                Ok(resolution) => resolution,
                Err(error) => return usage_error(format!("{}: {error}", scenario.name)),
            };
        if let Some(base_url) = &cli.base_url {
            for provider in &mut resolution.runnable {
                provider.base_url = Some(base_url.clone());
            }
        }
        runnable_count += resolution.runnable.len();
        plans.push((scenario, resolution, platform));
    }

    if cli.dry {
        let skipped = print_skips(&plans, &mut status);
        let mut runnable = 0usize;
        let mut upper_bound_usd = 0.0;
        for (scenario, resolution, _) in &plans {
            for provider in &resolution.runnable {
                status.line(format!(
                    "PLAN {} {} os={} approval={} max_cost_usd={:.6}",
                    scenario.name,
                    provider.id,
                    Platform::current(),
                    scenario.approval,
                    scenario.max_total_cost_usd
                ));
                runnable += 1;
                upper_bound_usd += scenario.max_total_cost_usd;
            }
        }
        status.line(format!(
            "ESTIMATE upper_bound_usd={upper_bound_usd:.6} runnable={runnable} skip={skipped}"
        ));
        return finish_output(&cli, &status, 0);
    }

    if runnable_count == 0 {
        let skipped = print_skips(&plans, &mut status);
        status.line(format!("SUMMARY pass=0 fail=0 skip={skipped} aborted=0"));
        return finish_output(&cli, &status, 0);
    }

    let artifact = match inspect_cli_artifact(&cli) {
        Ok(artifact) => artifact,
        Err(error) => return usage_error(error),
    };
    let fixture_manifest = match load_fixture_manifest(&cli) {
        Ok(value) => value,
        Err(error) => return usage_error(error),
    };

    let mut passed = 0usize;
    let mut failed = 0usize;
    let skipped = print_skips(&plans, &mut status);
    let run_cells: Vec<_> = plans
        .iter()
        .flat_map(|(scenario, resolution, _)| {
            resolution
                .runnable
                .iter()
                .map(move |provider| (scenario, provider))
        })
        .collect();
    let mut aborted = 0usize;
    let mut total_cost = 0.0;
    for (index, (scenario, provider)) in run_cells.iter().enumerate() {
        if cli
            .budget
            .is_some_and(|budget| total_cost + scenario.max_total_cost_usd > budget)
        {
            aborted += print_aborted(&run_cells[index..], "budget", &mut status);
            break;
        }

        let mut cell_failed = false;
        if let Err(error) = verify_artifact_digest(&artifact) {
            failed += 1;
            cell_failed = true;
            status.line(format!(
                "FAIL {} {} artifact_integrity={error}",
                scenario.name, provider.id
            ));
        } else {
            let run_result = run_with_binary(scenario, provider, &artifact.path).await;
            if let Err(error) = verify_artifact_digest(&artifact) {
                failed += 1;
                cell_failed = true;
                status.line(format!(
                    "FAIL {} {} artifact_integrity={error}",
                    scenario.name, provider.id
                ));
            } else {
                match run_result {
                    Ok(mut result) => {
                        total_cost += result.cost_usd;
                        if cli.budget.is_some_and(|budget| total_cost > budget) {
                            result.failures.push(Failure::OverCost {
                                observed_usd: total_cost,
                                budget_usd: cli.budget.unwrap_or_default(),
                            });
                            result.passed = false;
                        }
                        match build_and_persist_receipt(
                            &cli,
                            &artifact,
                            scenario,
                            provider,
                            &result,
                            index,
                            fixture_manifest.as_ref(),
                        ) {
                            Ok(gate_passed) if gate_passed => {
                                passed += 1;
                                status.line(format!(
                                    "PASS {} {} os={} approval={}",
                                    scenario.name, provider.id, result.platform, result.approval
                                ));
                            }
                            Ok(_) => {
                                failed += 1;
                                cell_failed = true;
                                status.line(format!(
                                    "FAIL {} {} os={} approval={} failures={}",
                                    scenario.name,
                                    provider.id,
                                    result.platform,
                                    result.approval,
                                    result.failures.len()
                                ));
                            }
                            Err(error) => {
                                failed += 1;
                                cell_failed = true;
                                status.line(format!(
                                    "FAIL {} {} reason=receipt_error detail={error}",
                                    scenario.name, provider.id
                                ));
                            }
                        }
                    }
                    Err(error) => {
                        failed += 1;
                        cell_failed = true;
                        status.line(format!(
                            "FAIL {} {} reason=runner_error detail={}",
                            scenario.name,
                            provider.id,
                            safe_status_detail(
                                &error.to_string(),
                                provider.resolved_key().as_deref()
                            )
                        ));
                    }
                }
            }
        }

        if cell_failed && scenario.name == "canary" {
            aborted += print_aborted(&run_cells[index + 1..], "canary", &mut status);
            break;
        }
        if cli.budget.is_some_and(|budget| total_cost > budget) {
            aborted += print_aborted(&run_cells[index + 1..], "budget", &mut status);
            break;
        }
    }

    status.line(format!(
        "SUMMARY pass={passed} fail={failed} skip={skipped} aborted={aborted}"
    ));
    finish_output(&cli, &status, i32::from(failed > 0 || aborted > 0))
}

fn print_aborted(
    cells: &[(&Scenario, &wcore_eval_scenarios::providers::ProviderConfig)],
    reason: &str,
    status: &mut StatusOutput,
) -> usize {
    for (scenario, provider) in cells {
        status.line(format!(
            "ABORTED {} {} reason={reason}",
            scenario.name, provider.id
        ));
    }
    cells.len()
}

fn inspect_cli_artifact(cli: &Cli) -> Result<SealedBinaryArtifact, String> {
    let expected_source_commit = cli.expected_source_commit.as_deref().ok_or_else(|| {
        "--expected-source-commit is required for binary verification".to_string()
    })?;
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| "could not resolve workspace root".to_string())?;
    let environment_binary = std::env::var_os("WCORE_EVAL_BIN");
    let candidate = select_candidate(
        cli.binary.as_deref(),
        environment_binary.as_deref(),
        workspace_root,
    )
    .map_err(|error| error.to_string())?;
    seal_binary(
        &candidate,
        ArtifactExpectation {
            version: env!("CARGO_PKG_VERSION"),
            source_commit: expected_source_commit,
        },
    )
    .map_err(|error| error.to_string())
}

fn print_skips(
    plans: &[(Scenario, ProviderResolution, PlatformDisposition)],
    status: &mut StatusOutput,
) -> usize {
    let mut count = 0;
    for (scenario, resolution, platform) in plans {
        if let PlatformDisposition::Skipped { current, supported } = platform {
            let supported = supported
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            status.line(format!(
                "SKIP {} os={} approval={} unsupported_os={} supported={}",
                scenario.name, current, scenario.approval, current, supported
            ));
            count += 1;
            continue;
        }
        for skip in &resolution.skipped {
            status.line(format!(
                "SKIP {} {} os={} approval={} missing={}",
                scenario.name,
                skip.provider,
                Platform::current(),
                scenario.approval,
                skip.missing_key
            ));
            count += 1;
        }
    }
    count
}

fn usage_error(error: impl std::fmt::Display) -> i32 {
    eprintln!("wayland-eval: {error}");
    2
}

#[derive(Default)]
struct StatusOutput {
    lines: Vec<String>,
}

impl StatusOutput {
    fn line(&mut self, line: impl Into<String>) {
        let line = line.into();
        println!("{line}");
        self.lines.push(line);
    }

    fn bytes(&self) -> Vec<u8> {
        let mut output = self.lines.join("\n").into_bytes();
        if !output.is_empty() {
            output.push(b'\n');
        }
        output
    }
}

fn finish_output(cli: &Cli, status: &StatusOutput, code: i32) -> i32 {
    let Some(path) = &cli.output else {
        return code;
    };
    if let Err(error) = wcore_config::atomic_write(path, &status.bytes()) {
        eprintln!(
            "wayland-eval: could not persist output to {}: {error}",
            path.display()
        );
        return 2;
    }
    code
}

fn build_and_persist_receipt(
    cli: &Cli,
    artifact: &SealedBinaryArtifact,
    scenario: &Scenario,
    provider: &ProviderConfig,
    result: &wcore_eval_scenarios::ScenarioResult,
    index: usize,
    fixture_manifest: Option<&LoadedFixtureManifest>,
) -> Result<bool, String> {
    let fixture_sha256 = match fixture_manifest {
        Some(manifest) => manifest.verify_sha256()?,
        None => {
            format!(
                "{:x}",
                Sha256::digest(format!("{}:{}", artifact.sha256, scenario.name))
            )
        }
    };
    let receipt = EvidenceReceiptV1::from_scenario_result(
        ReceiptMetadataV1 {
            run_id: format!(
                "{}-{index}-{}-{}",
                &artifact.sha256[..12],
                scenario.name,
                provider.id
            ),
            source_commit: artifact.source_commit.clone(),
            binary_sha256: artifact.sha256.clone(),
            fixture_sha256,
            model: provider.model.clone(),
            build: Evidence::Unavailable {
                code: "local_non_authoritative".to_string(),
            },
        },
        result,
        scenario.max_total_cost_usd,
    )
    .map_err(|error| error.to_string())?;
    ReceiptVerifier::new()
        .verify(&receipt, &VerificationPolicy::default())
        .map_err(|error| error.to_string())?;
    let cell_passed = receipt.body.results[0].passed;

    if let Some(report_dir) = &cli.report_dir {
        let forbidden_secrets = provider.resolved_key().into_iter().collect::<Vec<_>>();
        let reports = render_receipt_reports(&receipt, &forbidden_secrets)
            .map_err(|error| error.to_string())?;
        persist_receipt_reports(
            report_dir,
            &cell_directory_name(index, scenario.name, provider),
            &reports,
        )?;
    }
    Ok(cell_passed)
}

struct LoadedFixtureManifest {
    root: PathBuf,
    binding: BoundCompositeFixtureManifest,
}

impl LoadedFixtureManifest {
    fn verify_sha256(&self) -> Result<String, String> {
        self.binding.verify(&self.root).map_err(|error| {
            format!("fixture artifacts no longer match their manifest: {error}")
        })?;
        Ok(self.binding.manifest().fixture_sha256().to_string())
    }
}

fn load_fixture_manifest(cli: &Cli) -> Result<Option<LoadedFixtureManifest>, String> {
    let Some(path) = &cli.fixture_manifest else {
        if authority_evidence_requested() {
            return Err(
                "--fixture-manifest is required when authoritative evidence is requested"
                    .to_string(),
            );
        }
        return Ok(None);
    };
    let bytes = std::fs::read(path).map_err(|error| {
        format!(
            "could not read fixture manifest {}: {error}",
            path.display()
        )
    })?;
    let binding: BoundCompositeFixtureManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid fixture manifest {}: {error}", path.display()))?;
    let root = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    binding
        .verify(&root)
        .map_err(|error| format!("invalid fixture manifest {}: {error}", path.display()))?;
    Ok(Some(LoadedFixtureManifest { root, binding }))
}

fn authority_evidence_requested() -> bool {
    std::env::var("WCORE_EVAL_REQUIRE_AUTHORITY_EVIDENCE").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn persist_receipt_reports(
    root: &std::path::Path,
    cell: &str,
    reports: &ReceiptReports,
) -> Result<(), String> {
    std::fs::create_dir_all(root)
        .map_err(|error| format!("could not create report root {}: {error}", root.display()))?;
    let destination = root.join(cell);
    if destination.exists() {
        return Err(format!(
            "report destination already exists: {}",
            destination.display()
        ));
    }
    let staging = root.join(format!(".{cell}.tmp-{}", std::process::id()));
    std::fs::create_dir(&staging).map_err(|error| {
        format!(
            "could not create report staging directory {}: {error}",
            staging.display()
        )
    })?;
    let write_result = [
        ("receipt.json", reports.json.as_bytes()),
        ("events.jsonl", reports.jsonl.as_bytes()),
        ("junit.xml", reports.junit.as_bytes()),
        ("report.txt", reports.console.as_bytes()),
        ("report.md", reports.markdown.as_bytes()),
    ]
    .into_iter()
    .try_for_each(|(name, bytes)| {
        wcore_config::atomic_write(staging.join(name), bytes)
            .map_err(|error| format!("could not persist {name}: {error}"))
    });
    if let Err(error) = write_result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&staging, &destination) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!(
            "could not publish report directory {}: {error}",
            destination.display()
        ));
    }
    Ok(())
}

fn cell_directory_name(index: usize, scenario: &str, provider: &ProviderConfig) -> String {
    let safe_scenario = scenario
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{index:03}-{safe_scenario}-{}", provider.id)
}

fn safe_status_detail(detail: &str, secret: Option<&str>) -> String {
    let detail = secret.filter(|secret| !secret.is_empty()).map_or_else(
        || detail.to_string(),
        |secret| detail.replace(secret, "[REDACTED]"),
    );
    detail
        .replace(['\r', '\n'], " ")
        .chars()
        .take(240)
        .collect()
}
