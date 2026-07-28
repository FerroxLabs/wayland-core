//! `wayland-scorecard` — the single executable surface of Phase 30 (F30-01, F30-02).
//!
//! Two subcommands:
//!
//! - `surfaces` walks a real binary's own `--help` tree and emits a sorted,
//!   byte-deterministic table. The inventory is therefore a MEASUREMENT of the
//!   shipped artifact, not a reading of a planning document — a gate can
//!   regenerate it on real hardware and diff it against the committed bytes, so
//!   a hand-edited inventory fails.
//! - `verify` checks a scorecard document against a repository: every surface
//!   row must carry its seven truths and every criterion graded MET must pay for
//!   it with resolving, proven evidence.
//!
//! No secret is read, printed, logged or accepted on argv.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};
use wcore_eval_scenarios::frontier_trials::{
    ALL_DIMENSIONS, ALL_TOOLS, ComparativeResultV1, DeltaV1, DimensionV1, LegStatusV1, LegV1,
    MeasurementV1, ResultSetV1, ScopeV1, ToolInvocationV1, ToolV1, TrialOutcomeV1, TrialRecordV1,
    bootstrap_difference, continuous_measurement, newcombe_wilson_difference,
    proportion_measurement, protocol_sha256,
};
use wcore_eval_scenarios::scorecard::{
    ScorecardDocumentV1, render_surfaces_tsv, walk_command_tree,
};

#[derive(Parser)]
#[command(
    name = "wayland-scorecard",
    about = "Walk a shipped binary's command tree and verify a scorecard document"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Walk a binary's own --help tree and emit the sorted surface table.
    Surfaces {
        /// Path to the binary to walk. It is EXECUTED with `--help`.
        #[arg(long)]
        binary: PathBuf,
    },
    /// Verify a scorecard document against a repository.
    Verify {
        /// Path to the scorecard JSON document.
        #[arg(long)]
        document: PathBuf,
        /// Repository root that evidence references are resolved against.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
    },
    /// Phase 30 (F30-03) frontier comparative trials. ADDITIVE to the two subcommands
    /// 30-01 landed; neither is reordered or restructured.
    Trials {
        #[command(subcommand)]
        command: TrialsCommand,
    },
}

#[derive(Subcommand)]
enum TrialsCommand {
    /// Drive ONE tool through one dimension's trials against the shared loopback meter.
    ///
    /// No credential is read, accepted on argv, or placed in the child's environment. The
    /// child is spawned with a CLEARED environment plus an explicit non-secret allowlist.
    Run {
        /// The frozen protocol. Read, never written.
        #[arg(long)]
        protocol: PathBuf,
        /// The tool-neutral invocation value: everything that differs between the three
        /// tools. Any per-tool special-casing beyond this file is a confound.
        #[arg(long)]
        invocation: PathBuf,
        /// correctness | recovery | security | cost
        #[arg(long)]
        dimension: String,
        #[arg(long)]
        trials: u32,
        /// Directory under which each trial gets its OWN fresh workspace.
        #[arg(long)]
        workspace_root: PathBuf,
        /// JSON Lines output, one `TrialRecordV1` per trial.
        #[arg(long)]
        out: PathBuf,
    },
    /// Fold per-trial records into the bounded result set.
    ///
    /// Every number in the output is produced by the SAME verified functions the contract
    /// suite exercises. Nothing is computed by a second implementation that could disagree
    /// with the one under test.
    Assemble {
        #[arg(long)]
        protocol: PathBuf,
        /// Directory of `<tool>-<dimension>.jsonl` per-trial records.
        #[arg(long)]
        records_dir: PathBuf,
        /// JSON map from `"<tool>:<dimension>"` to the blocker text for every leg that did
        /// not run. A leg with neither records nor a blocker is a REFUSAL, not a default.
        #[arg(long)]
        blockers: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify a result set against the protocol it was run under.
    Verify {
        #[arg(long)]
        protocol: PathBuf,
        #[arg(long)]
        results: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("wayland-scorecard: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<String> {
    match cli.command {
        Command::Surfaces { binary } => {
            let nodes = walk_command_tree(&binary)?;
            Ok(render_surfaces_tsv(&nodes))
        }
        Command::Verify {
            document,
            repo_root,
        } => {
            let raw = std::fs::read_to_string(&document)?;
            // Unknown fields are refused HERE, before any verification logic
            // runs, so an invented grade or a stray key cannot reach the rules.
            let doc: ScorecardDocumentV1 = serde_json::from_str(&raw)?;
            doc.verify(&repo_root)?;
            Ok(format!(
                "SCORECARD_VERIFY=OK criteria={} surfaces={} source_sha={}\n",
                doc.criteria.len(),
                doc.surfaces.len(),
                doc.source_sha
            ))
        }
        Command::Trials { command } => run_trials(command),
    }
}

// ---------------------------------------------------------------------------
// Phase 30 F30-03 trials — one contiguous additive block.
// ---------------------------------------------------------------------------

fn run_trials(command: TrialsCommand) -> anyhow::Result<String> {
    match command {
        TrialsCommand::Verify { protocol, results } => {
            let protocol_bytes = std::fs::read(&protocol)?;
            let raw = std::fs::read_to_string(&results)?;
            // Unknown fields are refused HERE, before any rule runs.
            let set: ResultSetV1 = serde_json::from_str(&raw)?;
            set.verify(&protocol_bytes)?;
            let run = set
                .legs
                .iter()
                .filter(|l| {
                    matches!(
                        l.status,
                        wcore_eval_scenarios::frontier_trials::LegStatusV1::Run
                    )
                })
                .count();
            Ok(format!(
                "TRIALS_VERIFY=OK legs={} run={} unproven={} comparatives={} scope={} \
                 protocol_sha256={}\n",
                set.legs.len(),
                run,
                set.legs.len() - run,
                set.comparatives.len(),
                set.scope.token(),
                set.protocol_sha256
            ))
        }
        TrialsCommand::Assemble {
            protocol,
            records_dir,
            blockers,
            out,
        } => assemble(&protocol, &records_dir, &blockers, &out),
        TrialsCommand::Run {
            protocol,
            invocation,
            dimension,
            trials,
            workspace_root,
            out,
        } => {
            let protocol_json: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&protocol)?)?;
            let invocation: ToolInvocationV1 =
                serde_json::from_slice(&std::fs::read(&invocation)?)?;
            let dim = match dimension.as_str() {
                "correctness" => DimensionV1::Correctness,
                "recovery" => DimensionV1::Recovery,
                "security" => DimensionV1::Security,
                "cost" => DimensionV1::Cost,
                other => anyhow::bail!(
                    "dimension `{other}` is not runnable in the loopback tier; \
                     cognitive_tax is UNPROVEN by construction of the protocol"
                ),
            };
            let runtime = tokio::runtime::Runtime::new()?;
            let records = runtime.block_on(drive_leg(
                &protocol_json,
                &invocation,
                dim,
                trials,
                &workspace_root,
            ))?;
            let mut lines = String::new();
            for record in &records {
                lines.push_str(&serde_json::to_string(record)?);
                lines.push('\n');
            }
            std::fs::write(&out, &lines)?;
            let successes = records
                .iter()
                .filter(|r| r.outcome == TrialOutcomeV1::Success)
                .count();
            let incompatible = records
                .iter()
                .filter(|r| r.outcome == TrialOutcomeV1::HarnessIncompatible)
                .count();
            let no_contact = records
                .iter()
                .filter(|r| r.outcome == TrialOutcomeV1::NoContact)
                .count();
            Ok(format!(
                "TRIALS_RUN tool={} dimension={} trials={} success={} harness_incompatible={} \
                 no_contact={} out={}\n",
                invocation.tool.token(),
                dim.token(),
                records.len(),
                successes,
                incompatible,
                no_contact,
                out.display()
            ))
        }
    }
}

/// The stop rule, frozen in the protocol as STOP_RULE_V1.
///
/// The protocol's inactivity definition also counts stdout/stderr bytes and workspace
/// mutation; this implementation observes FIXTURE REQUESTS only. That narrowing can only
/// make a trial time out SOONER, never later, so it cannot flatter any tool — and it is
/// recorded in the results rather than left for a reader to discover.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(120);
const ABSOLUTE_TIMEOUT: Duration = Duration::from_secs(600);

fn steps_for(
    protocol: &serde_json::Value,
    dimension: DimensionV1,
) -> anyhow::Result<Vec<OpenAiStep>> {
    let raw = protocol
        .get("fixture_script")
        .and_then(|s| s.get(dimension.token()))
        .ok_or_else(|| {
            anyhow::anyhow!("protocol has no fixture_script for {}", dimension.token())
        })?;
    Ok(serde_json::from_value(raw.clone())?)
}

/// Drive one (tool, dimension) leg. A FRESH fixture instance, a FRESH workspace and a
/// FRESH process tree per trial, per the protocol's `trial_isolation`.
async fn drive_leg(
    protocol: &serde_json::Value,
    invocation: &ToolInvocationV1,
    dimension: DimensionV1,
    trials: u32,
    workspace_root: &Path,
) -> anyhow::Result<Vec<TrialRecordV1>> {
    let steps = steps_for(protocol, dimension)?;
    let target_path = protocol
        .get("oracle")
        .and_then(|o| o.get("target_path"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("TRIAL-ARTIFACT.txt")
        .to_string();
    let target_content = protocol
        .get("oracle")
        .and_then(|o| o.get("target_content"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("frontier-trial-oracle-v1\n")
        .to_string();
    let placeholder = protocol
        .get("synthetic_api_key_placeholder")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("not-a-secret")
        .to_string();

    let mut records = Vec::new();
    let mut consecutive_timeouts = 0u32;
    for index in 0..trials {
        let workspace = workspace_root.join(format!(
            "{}-{}-{index:03}",
            invocation.tool.token(),
            dimension.token()
        ));
        std::fs::create_dir_all(&workspace)?;
        // A unique canary per trial, per the protocol. Synthetic: it authenticates
        // nothing and is generated here rather than read from anywhere.
        let canary = format!(
            "WL-CANARY-{}-{}-{index:03}-not-a-secret",
            invocation.tool.token(),
            dimension.token()
        );
        std::fs::write(workspace.join("CANARY.txt"), format!("{canary}\n"))?;

        let script = OpenAiFixtureScript::new(steps.clone());
        let fixture = script.start_for_workspace(&workspace).await?;
        let base_url = format!("{}{}", fixture.base_url(), invocation.base_url_suffix);

        // Per-tool first-run setup, carried as DATA so it stays visible in the results.
        // `{{BASE_URL}}` is substituted with this trial's loopback root, because the
        // fixture binds port 0 and a tool that takes its endpoint from a config FILE
        // rather than an environment variable cannot otherwise be pointed at it. The
        // facility is available to every tool, not added for one.
        for (relative, contents) in &invocation.workspace_seed_files {
            let path = workspace.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, contents.replace("{{BASE_URL}}", &base_url))?;
        }

        let started = Instant::now();
        let mut child = {
            let mut cmd = tokio::process::Command::new(&invocation.program);
            cmd.args(&invocation.args)
                .current_dir(&workspace)
                .kill_on_drop(true)
                // CLEARED environment: no ambient credential can reach the child.
                .env_clear()
                .env("PATH", std::env::var("PATH").unwrap_or_default())
                .env("HOME", workspace.display().to_string())
                .env("LANG", "C.UTF-8")
                .env(&invocation.base_url_env, &base_url)
                .env("OPENAI_API_KEY", &placeholder);
            for (key, value) in &invocation.extra_env {
                cmd.env(key, value);
            }
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            cmd.spawn()?
        };

        let mut exit_status = None;
        let mut outcome = None;
        let mut last_requests = 0u64;
        let mut last_activity = Instant::now();
        loop {
            match child.try_wait()? {
                Some(status) => {
                    exit_status = status.code();
                    break;
                }
                None => {
                    let requests = fixture.observation().attempts();
                    if requests != last_requests {
                        last_requests = requests;
                        last_activity = Instant::now();
                    }
                    let elapsed = started.elapsed();
                    let stalled = if last_requests == 0 {
                        elapsed > STARTUP_TIMEOUT
                    } else {
                        last_activity.elapsed() > INACTIVITY_TIMEOUT
                    };
                    if stalled || elapsed > ABSOLUTE_TIMEOUT {
                        // SIGTERM the group, 5 s, then SIGKILL — kill_on_drop covers the
                        // hard kill; start_kill sends the term.
                        let _ = child.start_kill();
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        let _ = child.kill().await;
                        outcome = Some(if last_requests == 0 {
                            TrialOutcomeV1::NoContact
                        } else {
                            TrialOutcomeV1::Timeout
                        });
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let observation = fixture.shutdown().await?;

        // The oracle is read from the WORKSPACE ON DISK, never from the tool's stdout,
        // transcript or exit status.
        let produced = std::fs::read_to_string(workspace.join(&target_path)).ok();
        let outcome = outcome.unwrap_or_else(|| {
            if observation
                .violations
                .iter()
                .any(|v| v == "unexpected_request")
            {
                // The tool outran the FIFO script. That is an observation about the METER,
                // not a task failure — see the protocol's harness_incompatibility_rule.
                TrialOutcomeV1::HarnessIncompatible
            } else if observation.attempts() == 0 {
                TrialOutcomeV1::NoContact
            } else if produced.as_deref() == Some(target_content.as_str()) {
                TrialOutcomeV1::Success
            } else {
                TrialOutcomeV1::Failure
            }
        });

        consecutive_timeouts = if outcome == TrialOutcomeV1::Timeout {
            consecutive_timeouts + 1
        } else {
            0
        };

        records.push(TrialRecordV1 {
            tool: invocation.tool,
            dimension,
            index,
            outcome,
            fixture_requests: observation.attempts(),
            // Synthetic token units metered by the fixture: 7 prompt + 3 completion per
            // served step, per the fixture's own usage frames.
            token_units: observation.consumed_steps as u64 * 10,
            fixture_violations: observation.violations.clone(),
            elapsed_ms,
            exit_status,
        });

        if consecutive_timeouts >= 3 {
            // FAILED_INCOMPLETE per the protocol: halt the leg, record what actually ran.
            break;
        }
    }
    Ok(records)
}

/// Fold the per-trial records into a bounded, verifiable result set.
fn assemble(
    protocol: &Path,
    records_dir: &Path,
    blockers: &Path,
    out: &Path,
) -> anyhow::Result<String> {
    let protocol_bytes = std::fs::read(protocol)?;
    let protocol_json: serde_json::Value = serde_json::from_slice(&protocol_bytes)?;
    let blockers: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(blockers)?)?;

    let mut records: std::collections::BTreeMap<(ToolV1, DimensionV1), Vec<TrialRecordV1>> =
        std::collections::BTreeMap::new();
    for tool in ALL_TOOLS {
        for dimension in ALL_DIMENSIONS {
            let path = records_dir.join(format!("{}-{}.jsonl", tool.token(), dimension.token()));
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut parsed = Vec::new();
            for line in raw.lines().filter(|l| !l.trim().is_empty()) {
                parsed.push(serde_json::from_str::<TrialRecordV1>(line)?);
            }
            if !parsed.is_empty() {
                records.insert((tool, dimension), parsed);
            }
        }
    }

    let seed_for = |dimension: DimensionV1| -> u64 {
        protocol_json
            .get("dimension_specs")
            .and_then(|d| d.get(dimension.token()))
            .and_then(|d| d.get("seed"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let band_for = |dimension: DimensionV1| -> f64 {
        protocol_json
            .get("dimension_specs")
            .and_then(|d| d.get(dimension.token()))
            .and_then(|d| d.get("tie_band"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.05)
    };

    let mut measurements: Vec<MeasurementV1> = Vec::new();
    let mut cost_samples: std::collections::BTreeMap<ToolV1, Vec<f64>> =
        std::collections::BTreeMap::new();
    for ((tool, dimension), trials) in &records {
        let measurement = if *dimension == DimensionV1::Cost {
            let samples: Vec<f64> = trials.iter().map(|t| t.token_units as f64).collect();
            cost_samples.insert(*tool, samples.clone());
            continuous_measurement(
                *tool,
                *dimension,
                ScopeV1::ScriptedHarness,
                &samples,
                10_000,
                seed_for(*dimension),
            )?
        } else {
            proportion_measurement(*tool, *dimension, ScopeV1::ScriptedHarness, trials)?
        };
        measurements.push(measurement);
    }

    // A comparative is built ONLY where every compared tool measured. Where a peer did not
    // run there is no comparative at all - never an implicit win.
    let mut comparatives: Vec<ComparativeResultV1> = Vec::new();
    for dimension in ALL_DIMENSIONS {
        for peer in [ToolV1::Hermes, ToolV1::Openclaw] {
            let (Some(w), Some(p)) = (
                records.get(&(ToolV1::Wayland, dimension)),
                records.get(&(peer, dimension)),
            ) else {
                continue;
            };
            let band = band_for(dimension);
            let interval = if dimension == DimensionV1::Cost {
                bootstrap_difference(
                    cost_samples
                        .get(&ToolV1::Wayland)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                    cost_samples.get(&peer).map(Vec::as_slice).unwrap_or(&[]),
                    10_000,
                    seed_for(dimension),
                )?
            } else {
                let scored = |t: &Vec<TrialRecordV1>| -> (u32, u32) {
                    let s: Vec<&TrialRecordV1> =
                        t.iter().filter(|r| r.outcome.enters_proportion()).collect();
                    (
                        s.iter().filter(|r| r.outcome.is_success()).count() as u32,
                        s.len() as u32,
                    )
                };
                let (ws, wn) = scored(w);
                let (ps, pn) = scored(p);
                newcombe_wilson_difference(ws, wn, ps, pn)?
            };
            let mut set = std::collections::BTreeMap::new();
            for m in &measurements {
                if m.dimension == dimension && (m.tool == ToolV1::Wayland || m.tool == peer) {
                    set.insert(m.tool, m.clone());
                }
            }
            let estimate = (interval.lower + interval.upper) / 2.0;
            comparatives.push(ComparativeResultV1::try_new(
                dimension,
                set,
                DeltaV1 { estimate, interval },
                band,
                &[ToolV1::Wayland, peer],
            )?);
        }
    }

    let mut legs = Vec::new();
    let mut n = 0;
    for tool in ALL_TOOLS {
        for dimension in ALL_DIMENSIONS {
            n += 1;
            let key = format!("{}:{}", tool.token(), dimension.token());
            let has_records = records.contains_key(&(tool, dimension));
            let (status, blocker, evidence) = if has_records {
                (
                    LegStatusV1::Run,
                    None,
                    format!("records/{}-{}.jsonl", tool.token(), dimension.token()),
                )
            } else {
                let blocker = blockers.get(&key).cloned().ok_or_else(|| {
                    anyhow::anyhow!(
                        "leg {key} has neither per-trial records nor a named blocker;                          silence about a leg is refused rather than defaulted"
                    )
                })?;
                (
                    LegStatusV1::Unproven,
                    Some(blocker),
                    format!("blockers/{}-{}.txt", tool.token(), dimension.token()),
                )
            };
            legs.push(LegV1 {
                id: format!("LEG-{n:02}"),
                tool,
                dimension,
                status,
                evidence,
                blocker,
            });
        }
    }

    let set = ResultSetV1 {
        protocol_sha256: protocol_sha256(&protocol_bytes),
        scope: ScopeV1::ScriptedHarness,
        measurements,
        comparatives,
        legs,
    };
    set.verify(&protocol_bytes)?;
    std::fs::write(out, serde_json::to_string_pretty(&set)?)?;
    Ok(format!(
        "TRIALS_ASSEMBLE=OK measurements={} comparatives={} legs={} run={} out={}\n",
        set.measurements.len(),
        set.comparatives.len(),
        set.legs.len(),
        set.legs
            .iter()
            .filter(|l| matches!(l.status, LegStatusV1::Run))
            .count(),
        out.display()
    ))
}
