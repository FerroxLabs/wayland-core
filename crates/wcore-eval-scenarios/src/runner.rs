//! The json-stream runner core (T2).
//!
//! Spawn `wayland-core --json-stream`, drive per-turn via
//! `message` / `stream_end` events, capture stderr + the trailing
//! `session_cost`, enforce wall-time hygiene (`kill_on_drop` + explicit
//! `start_kill` on `Elapsed`), build a [`ScenarioResult`].
//!
//! **What ISN'T here (deferred):**
//! - Per-turn assertion firing (T3 — `assertions::Assertion::check`).
//! - Tool-trace cross-validation against the session JSON file (T3).
//! - DeepSeek `reasoning_content` normalization (T3, per L-5).
//!
//! The runner does collect each `tool_result` event into a flat
//! [`ToolTrace`] so smoke + future T3 assertions can read it.
//!
//! ## Wire-format note
//!
//! `wcore_protocol::events::ProtocolEvent` derives `Serialize` only
//! (host-facing emit-side schema). Hosts decode as `serde_json::Value`
//! and dispatch by the `"type"` tag — same model the engine itself
//! uses to read host `ProtocolCommand` lines. We do the same here.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use crate::child_env::ChildEnvironment;
use crate::cost::CostReport;
use crate::providers::{ProviderConfig, ProviderId};
use crate::redaction::SecretRedactor;
use crate::stderr_capture::StderrCapture;
use crate::tempenv::{self, TempEnvOptions};
use crate::trace::{ToolTrace, TraceEntry};

/// Outcome of one scenario × provider run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub name: String,
    pub provider: ProviderId,
    pub platform: crate::scenario::Platform,
    pub approval: crate::scenario::ApprovalPolicy,
    pub passed: bool,
    pub failures: Vec<Failure>,
    pub wall_time: Duration,
    pub cost_usd: f64,
    pub trace: ToolTrace,
    pub final_text: String,
    pub stderr_tail: String,
    pub turn_results: Vec<TurnResult>,
    /// The agent's working directory for this run (the tempenv root). Artifact
    /// assertions (`FileExists`/`FileContains`/`FileParsesAs`) resolve their
    /// relative paths against this. NOTE: the tempenv is deleted when the run
    /// finishes, so artifact checks happen inside `run()` before that — this
    /// field records where they ran for reporting.
    pub workdir: PathBuf,
    /// Time from process spawn to the first `ready` event — the engine's
    /// cold-boot/bootstrap latency (MCP connect attempts, plugin/skill load,
    /// memory open). A precise usability/perf metric distinct from LLM turn
    /// time. `Duration::ZERO` if the run failed before `ready`.
    pub boot_time: Duration,
    /// D1/D2: `info` event messages emitted across the run (slash-command
    /// acknowledgements like "style updated", mode changes, engine notices).
    /// Asserted via [`crate::assertions::Assertion::InfoContains`].
    pub info_events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResult {
    pub turn: usize,
    pub prompt: String,
    pub assistant_text: String,
    pub wall_time: Duration,
}

/// All the ways a scenario can fail. The runner collects EVERY failure
/// (no short-circuit on first) so reports show the full story.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Failure {
    OverTime {
        observed_secs: f64,
        budget_secs: f64,
    },
    OverCost {
        observed_usd: f64,
        budget_usd: f64,
    },
    CostMissing,
    Crashed {
        stderr_tail: String,
        exit: i32,
    },
    Hung {
        stderr_tail: String,
    },
    ExpectedToolMissing(String),
    ForbiddenToolUsed(String),
    AssertionFailed {
        assertion: String,
        observed: String,
    },
    TraceFailed {
        assertion: String,
        observed: String,
    },
    StepsExceeded {
        observed: usize,
        budget: usize,
    },
    SessionBrick {
        error: String,
    },
    /// M-2: scenario required a key that wasn't set AND scenario.strict
    /// was true. Lenient mode (default) turns this into a SKIP at the
    /// caller layer, not a Failure.
    SkippedInStrict {
        missing_key: String,
    },
    /// Process plumbing error (couldn't spawn, couldn't write stdin,
    /// invalid wire data, etc.) — surface so the test layer doesn't
    /// silently swallow it.
    RunnerError(String),
    SecretDetected {
        sink: String,
    },
}

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("could not locate wayland-core binary: {0}")]
    BinaryMissing(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Locate the `wayland-core` binary the runner should spawn.
///
/// Resolution order (first hit wins):
/// 1. `WCORE_EVAL_BIN` env var — explicit override; tests can pin a
///    specific build.
/// 2. `target/release/wayland-core` then `target/debug/wayland-core`
///    by walking up from `CARGO_MANIFEST_DIR` two levels (mirrors the
///    pattern in `crates/wcore-cli/tests/release_binary_smoke.rs`).
///
/// `CARGO_BIN_EXE_wayland-core` is NOT available here — Cargo only
/// exposes that for binaries owned by the same crate as the test. We
/// live in a different crate (`wcore-eval-scenarios`), so we must
/// discover the artifact.
pub fn discover_binary() -> Result<PathBuf, SpawnError> {
    if let Ok(p) = std::env::var("WCORE_EVAL_BIN") {
        let pb = PathBuf::from(&p);
        if pb.exists() {
            return Ok(pb);
        }
        return Err(SpawnError::BinaryMissing(format!(
            "WCORE_EVAL_BIN={p} but the file does not exist"
        )));
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/<crate>/Cargo.toml ⇒ workspace root is two levels up.
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| {
            SpawnError::BinaryMissing(format!(
                "CARGO_MANIFEST_DIR={} has fewer than 2 ancestors",
                manifest_dir.display()
            ))
        })?;

    let bin_name = if cfg!(windows) {
        "wayland-core.exe"
    } else {
        "wayland-core"
    };

    for profile in ["release", "debug"] {
        let cand = workspace_root.join("target").join(profile).join(bin_name);
        if cand.exists() {
            return Ok(cand);
        }
    }

    Err(SpawnError::BinaryMissing(format!(
        "no wayland-core binary at {}/target/{{release,debug}}/{bin_name}; \
         pre-build it (`cargo build -p wcore-cli`) or set WCORE_EVAL_BIN",
        workspace_root.display()
    )))
}

/// Spawn `wayland-core` configured for a scenario run — `--yolo`
/// (approval bypass; PTY scenarios use no-yolo separately), `--json-stream`
/// (the only mode that emits `session_cost` per C-2), per-provider
/// `--provider` + `--model` (H-5 — engine default is empty for DeepSeek),
/// `cwd = env.path()`, stdin/stdout/stderr piped, `kill_on_drop(true)`
/// (M-1 — tokio's timeout does NOT kill the child).
///
/// `pub` so smoke tests in `tests/smoke.rs` can drive plumbing directly
/// without going through the full assertion pipeline (T3's job).
pub fn spawn_for_run(
    bin: &std::path::Path,
    cwd: &std::path::Path,
    provider: &ProviderConfig,
    yolo: bool,
    wayland_home: Option<&std::path::Path>,
) -> Result<Child, SpawnError> {
    let mut cmd = Command::new(bin);
    let isolated_home = wayland_home.unwrap_or(cwd);
    ChildEnvironment::build(cwd, isolated_home, Some(provider))?.apply_tokio(&mut cmd);
    // Build an allowlisted child environment before adding any scenario
    // arguments. Credentials enter only through the generic API_KEY variable;
    // they never appear in argv or the persisted scenario config.
    // D3: only force-approve when the scenario's policy is `Yolo`. Without
    // `--yolo` the engine boots in `Default` approval mode and emits
    // `ApprovalRequired` per mutating tool, which the runner answers per policy.
    if yolo {
        cmd.arg("--yolo");
    }
    cmd.arg("--json-stream")
        .arg("--provider")
        .arg(provider.id.cli_name())
        .arg("--model")
        .arg(&provider.model)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // `wayland_config_dir()` = `$WAYLAND_HOME` resolves the global config layer:
    // MCP servers, skills dir, and memory DBs. D4 cross-session runs pass ONE
    // persistent home so those carry across sessions. Persona/coverage runs pass
    // their per-run tempdir (an EMPTY global layer) for true hermeticity —
    // stripping the var (`None`) instead falls back to the developer's real home
    // and dials their actual MCP servers. `None` remains for callers that
    // genuinely want the host default.
    Ok(cmd.spawn()?)
}

/// Spawn the binary with arbitrary args + no provider seeding — used by
/// the `wayland-core --help` smoke test that does NOT touch the engine.
/// stdout/stderr piped so the caller can collect output; stdin null so
/// the process exits immediately when `--help` finishes.
pub fn spawn_with_args(bin: &std::path::Path, args: &[&str]) -> Result<Child, SpawnError> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(cmd.spawn()?)
}

/// Drive one scenario × provider to completion and return the result.
///
/// **T3 fills in**: assertion-firing, per-turn segmentation refinements,
/// tool-trace cross-validation against the session JSON file.
pub async fn run(
    scenario: &crate::scenario::Scenario,
    provider: &ProviderConfig,
) -> anyhow::Result<ScenarioResult> {
    let bin = discover_binary().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    run_with_binary(scenario, provider, &bin).await
}

/// Drive a scenario against a caller-validated binary artifact. The CLI uses
/// this path so artifact selection and provenance cannot be replaced by the
/// legacy developer auto-discovery between validation and spawn.
pub async fn run_with_binary(
    scenario: &crate::scenario::Scenario,
    provider: &ProviderConfig,
    bin: &std::path::Path,
) -> anyhow::Result<ScenarioResult> {
    // Persona path: a fresh hermetic throwaway env per run. WAYLAND_HOME points
    // at the tempdir (NOT stripped): `wayland_config_dir()` then resolves to the
    // empty tempdir, so the global config layer — the developer's real MCP
    // servers, skills, and memory under `~/Library/Application Support/
    // wayland-core` — is NOT loaded. (Stripping the var, the prior behaviour,
    // FELL BACK to that real dir, so every eval boot dialed the user's MCP
    // servers — slow and flaky: an occasional handshake stall hung boot to the
    // wall-time guard.) The seeded provider key + any setup-appended `[mcp.*]`
    // live in the cwd-walk PROJECT layer (`<tempdir>/.wayland-core/config.toml`),
    // which loads regardless of WAYLAND_HOME, so this only empties the global
    // layer. The cross-session harness drives `run_session_in` directly against
    // its own persistent home instead.
    let env = tempenv::build_with(
        provider,
        &TempEnvOptions {
            budget_max_cost_usd: (scenario.max_total_cost_usd > 0.0)
                .then_some(scenario.max_total_cost_usd),
        },
    )?;
    run_session_in(scenario, provider, bin, env.path(), Some(env.path())).await
}

/// Drive ONE session of a scenario inside an already-prepared working
/// directory `cwd`, returning the assembled + asserted [`ScenarioResult`].
///
/// Split out of [`run`] so the cross-session harness (D4) can drive several
/// sessions against ONE shared persistent home (`wayland_home = Some(home)`),
/// while the persona path keeps its hermetic throwaway env
/// (`wayland_home = None`, which strips `WAYLAND_HOME`). The caller owns the
/// working dir's lifetime (a `TempDir` for personas; a held cross-session env).
pub(crate) async fn run_session_in(
    scenario: &crate::scenario::Scenario,
    provider: &ProviderConfig,
    bin: &std::path::Path,
    cwd: &std::path::Path,
    wayland_home: Option<&std::path::Path>,
) -> anyhow::Result<ScenarioResult> {
    let redactor = SecretRedactor::from_secret(provider.resolved_key());
    // Run the scenario's setup hook BEFORE spawning the engine. The closure
    // seeds the working dir — input files to probe, fixture scripts (mock MCP
    // server, shell hooks), and config appends (`[mcp.servers.*]`,
    // `[[hooks.*]]`) onto the tempenv-seeded `.wayland-core/config.toml`. This
    // was previously assigned on `Scenario` but never invoked, so any
    // setup-dependent scenario silently degraded; D6/D7/coverage need it.
    let outcome = match &scenario.setup {
        Some(setup) => match setup(cwd) {
            Ok(()) => run_session_body(scenario, provider, bin, cwd, wayland_home).await,
            Err(error) => Err(anyhow::anyhow!("scenario setup failed: {error}")),
        },
        None => run_session_body(scenario, provider, bin, cwd, wayland_home).await,
    };

    let cleanup_error = scenario
        .cleanup
        .as_ref()
        .and_then(|cleanup| cleanup(cwd).err());

    let result = match (outcome, cleanup_error) {
        (Ok(mut result), Some(error)) => {
            result.failures.push(Failure::RunnerError(format!(
                "scenario cleanup failed: {error}"
            )));
            result.passed = false;
            Ok(result)
        }
        (Err(run_error), Some(cleanup_error)) => Err(anyhow::anyhow!(
            "{run_error}; scenario cleanup failed: {cleanup_error}"
        )),
        (outcome, None) => outcome,
    };
    result
        .map(|result| redactor.result(result))
        .map_err(|error| anyhow::anyhow!(redactor.text(error.to_string()).0))
}

async fn run_session_body(
    scenario: &crate::scenario::Scenario,
    provider: &ProviderConfig,
    bin: &std::path::Path,
    cwd: &std::path::Path,
    wayland_home: Option<&std::path::Path>,
) -> anyhow::Result<ScenarioResult> {
    let start = Instant::now();

    let mut child = spawn_for_run(
        bin,
        cwd,
        provider,
        scenario.approval == crate::scenario::ApprovalPolicy::Yolo,
        wayland_home,
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // Detach stderr first so we never deadlock on a full pipe.
    let stderr = child.stderr.take().expect("piped stderr");
    let stderr_cap = StderrCapture::spawn(stderr);

    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");

    // Outer wall-time guard. On Elapsed we MUST start_kill + wait —
    // tokio::time::timeout only cancels the future, not the child.
    let drive = drive_session(stdin, stdout, scenario);
    let result = tokio::time::timeout(scenario.max_total_time, drive).await;

    let (turn_results, trace, final_text, cost_report, hit_internal_error, boot_time, info_events) =
        match result {
            Ok(Ok(drive_out)) => (
                drive_out.turn_results,
                drive_out.trace,
                drive_out.final_text,
                drive_out.cost,
                drive_out.runner_error,
                drive_out.boot_time,
                drive_out.info_events,
            ),
            Ok(Err(e)) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                let stderr_tail = stderr_cap.snapshot();
                let failure = e.downcast_ref::<TurnTimeout>().map_or_else(
                    || Failure::RunnerError(e.to_string()),
                    |timeout| Failure::OverTime {
                        observed_secs: timeout.observed.as_secs_f64(),
                        budget_secs: timeout.budget.as_secs_f64(),
                    },
                );
                return Ok(ScenarioResult {
                    name: scenario.name.to_string(),
                    provider: provider.id,
                    platform: crate::scenario::Platform::current(),
                    approval: scenario.approval,
                    passed: false,
                    failures: vec![failure],
                    wall_time: start.elapsed(),
                    cost_usd: 0.0,
                    trace: ToolTrace::default(),
                    final_text: String::new(),
                    stderr_tail,
                    turn_results: Vec::new(),
                    workdir: cwd.to_path_buf(),
                    boot_time: Duration::ZERO,
                    info_events: Vec::new(),
                });
            }
            Err(_elapsed) => {
                // M-1: timeout fired. Kill explicitly, reap, then record
                // Hung with the stderr tail snapshot.
                let _ = child.start_kill();
                let _ = child.wait().await;
                let stderr_tail = stderr_cap.snapshot();
                return Ok(ScenarioResult {
                    name: scenario.name.to_string(),
                    provider: provider.id,
                    platform: crate::scenario::Platform::current(),
                    approval: scenario.approval,
                    passed: false,
                    failures: vec![Failure::Hung {
                        stderr_tail: stderr_tail.clone(),
                    }],
                    wall_time: start.elapsed(),
                    cost_usd: 0.0,
                    trace: ToolTrace::default(),
                    final_text: String::new(),
                    stderr_tail,
                    turn_results: Vec::new(),
                    workdir: cwd.to_path_buf(),
                    boot_time: Duration::ZERO,
                    info_events: Vec::new(),
                });
            }
        };

    // Normal-path child shutdown. The drive loop already sent `stop`
    // and consumed the trailing `session_cost`; the child should exit
    // promptly. Give a short grace, then kill if it lingers.
    let shutdown = tokio::time::timeout(Duration::from_secs(8), child.wait()).await;
    let exit_code = match shutdown {
        Ok(Ok(status)) => status.code().unwrap_or(0),
        Ok(Err(_)) | Err(_) => {
            // The child either errored on `wait()` or did not exit within the
            // grace window (it produced its output but hung on shutdown). Kill
            // it and surface a NON-zero sentinel so the `exit_code != 0` gate
            // below records `Crashed` — never silently report a clean exit for
            // a binary that couldn't exit (cross-audit finding #8).
            let _ = child.start_kill();
            let _ = child.wait().await;
            -1
        }
    };

    let stderr_tail = stderr_cap.snapshot();
    let cost_usd = cost_report.as_ref().map(|c| c.total_usd).unwrap_or(0.0);
    let wall_time = start.elapsed();

    let mut failures: Vec<Failure> = Vec::new();
    if let Some(err) = hit_internal_error {
        failures.push(Failure::RunnerError(err));
    }
    if exit_code != 0 {
        failures.push(Failure::Crashed {
            stderr_tail: stderr_tail.clone(),
            exit: exit_code,
        });
    }
    if cost_report.is_none() {
        failures.push(Failure::CostMissing);
    }

    // Soft cost budget (cross-audit finding #3). The hard wall-time kill is the
    // outer `tokio::time::timeout` (→ `Hung`); this records a scenario that
    // completed but blew its declared dollar ceiling. NOTE: openai-compat
    // providers (incl. DeepSeek) carry $0 pricing rows in `ProviderCompat`, so
    // `cost_usd` is 0.0 and this never fires there — wall-time is the real
    // runaway guard for those. It does protect priced providers (OpenAI/Anthropic).
    if scenario.max_total_cost_usd > 0.0 && cost_usd > scenario.max_total_cost_usd {
        failures.push(Failure::OverCost {
            observed_usd: cost_usd,
            budget_usd: scenario.max_total_cost_usd,
        });
    }

    // T3 (Wave 0): fire assertions now that check() is implemented.
    // Walk every turn's output_assertions against the turn's assistant text,
    // and trace_assertions against the full accumulated trace at turn end.
    //
    // We use `final_text` for single-turn scenarios and per-turn text for
    // multi-turn. The runner accumulates per-turn text in `turn_results`.
    for turn_result in &turn_results {
        // Find the matching Scenario turn to get its assertions.
        let maybe_turn = scenario.turns.get(turn_result.turn);
        if let Some(turn_def) = maybe_turn {
            for assertion in &turn_def.output_assertions {
                // Result-level (Wave-1.1) assertions need the completed
                // ScenarioResult (stderr_tail / cost_usd) — defer them to the
                // post-build pass below (finding #4). Artifact assertions check
                // the filesystem (the agent's cwd, still alive here); text
                // assertions check the turn's assistant text.
                if assertion.is_result_level() {
                    continue;
                }
                let outcome = if assertion.is_artifact() {
                    assertion.check_artifacts(cwd)
                } else {
                    assertion.check(&turn_result.assistant_text)
                };
                if let Err(observed) = outcome {
                    failures.push(Failure::AssertionFailed {
                        assertion: format!("{assertion:?}"),
                        observed,
                    });
                }
            }
            for trace_assertion in &turn_def.trace_assertions {
                if let Err(observed) = trace_assertion.check(&trace) {
                    failures.push(Failure::TraceFailed {
                        assertion: format!("{trace_assertion:?}"),
                        observed,
                    });
                }
            }
        }
    }

    // Per-turn expected/forbidden tool checks. Scoped to the turn the tool
    // fired in (finding #6) so a `Write` in turn 1 doesn't vacuously satisfy a
    // turn-2 `expect_tool("Write")` — the multi-turn marketer rewrite must
    // actually re-invoke the tool in turn 2.
    for (turn_idx, turn_def) in scenario.turns.iter().enumerate() {
        let observed_steps = trace
            .entries
            .iter()
            .filter(|entry| entry.turn == turn_idx)
            .count();
        if observed_steps > turn_def.max_steps {
            failures.push(Failure::StepsExceeded {
                observed: observed_steps,
                budget: turn_def.max_steps,
            });
        }
        for expected_tool in &turn_def.expected_tools {
            if trace.count_in_turn(expected_tool, turn_idx) == 0 {
                failures.push(Failure::ExpectedToolMissing(format!(
                    "{expected_tool} (turn {turn_idx})"
                )));
            }
        }
        for forbidden_tool in &turn_def.forbidden_tools {
            if trace.count_in_turn(forbidden_tool, turn_idx) > 0 {
                failures.push(Failure::ForbiddenToolUsed(format!(
                    "{forbidden_tool} (turn {turn_idx})"
                )));
            }
        }
    }

    // Build the result, then run result-level (Wave-1.1) assertions against it
    // and fold their failures in (finding #4 — these were never dispatched
    // before, so StderrContains / CostWithinTolerance silently no-op'd).
    let mut result = ScenarioResult {
        name: scenario.name.to_string(),
        provider: provider.id,
        platform: crate::scenario::Platform::current(),
        approval: scenario.approval,
        passed: false,
        failures,
        wall_time,
        cost_usd,
        trace,
        final_text,
        stderr_tail,
        turn_results,
        workdir: cwd.to_path_buf(),
        boot_time,
        info_events,
    };
    let mut result_level_failures: Vec<Failure> = Vec::new();
    for turn_def in &scenario.turns {
        for assertion in &turn_def.output_assertions {
            if assertion.is_result_level()
                && let Err(observed) = assertion.check_result(&result)
            {
                result_level_failures.push(Failure::AssertionFailed {
                    assertion: format!("{assertion:?}"),
                    observed,
                });
            }
        }
    }
    result.failures.extend(result_level_failures);
    result.passed = result.failures.is_empty();
    Ok(result)
}

/// Output of the inner stdin/stdout-driving loop. Pulled out so the
/// outer `tokio::time::timeout(...)` can wrap it cleanly.
struct DriveOutput {
    turn_results: Vec<TurnResult>,
    trace: ToolTrace,
    final_text: String,
    cost: Option<CostReport>,
    /// Set when the child closed stdout before we saw all expected
    /// events. Reported via `Failure::RunnerError`.
    runner_error: Option<String>,
    /// Time from drive start (≈ process spawn) to the first `ready` event.
    boot_time: Duration,
    /// `info` event messages captured across the run (D1/D2).
    info_events: Vec<String>,
}

#[derive(Debug, Error)]
#[error(
    "turn {turn} exceeded its {budget_secs:.3}s deadline after {observed_secs:.3}s",
    budget_secs = .budget.as_secs_f64(),
    observed_secs = .observed.as_secs_f64()
)]
struct TurnTimeout {
    turn: usize,
    observed: Duration,
    budget: Duration,
}

async fn read_turn_event<R>(
    reader: &mut BufReader<R>,
    turn: usize,
    started: Instant,
    budget: Duration,
) -> anyhow::Result<Option<Value>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let observed = started.elapsed();
    let Some(remaining) = budget.checked_sub(observed) else {
        return Err(TurnTimeout {
            turn,
            observed,
            budget,
        }
        .into());
    };

    match tokio::time::timeout(remaining, read_one_event(reader)).await {
        Ok(event) => event,
        Err(_) => Err(TurnTimeout {
            turn,
            observed: started.elapsed(),
            budget,
        }
        .into()),
    }
}

/// Build the approval-response command for a tool `call_id` under the given
/// policy, or `None` for `Yolo` (no response needed). Shared by the
/// `tool_request` (normal-tool gate) and `approval_required` (Script-tool gate)
/// paths so both answer the engine identically.
fn approval_command(
    policy: crate::scenario::ApprovalPolicy,
    call_id: &str,
) -> Option<serde_json::Value> {
    use crate::scenario::ApprovalPolicy;
    match policy {
        ApprovalPolicy::Yolo => None,
        ApprovalPolicy::ApproveAll => Some(serde_json::json!({
            "type": "tool_approve",
            "call_id": call_id,
            "scope": "once",
            "answer": null,
        })),
        ApprovalPolicy::DenyAll => Some(serde_json::json!({
            "type": "tool_deny",
            "call_id": call_id,
            "reason": "denied by eval approval policy",
        })),
    }
}

/// D2: lower a harness [`crate::scenario::TurnCommand`] to its json-stream
/// wire form. Mirrors the `set_config` / `set_mode` shapes in
/// `wcore_protocol::commands::ProtocolCommand` (snake_case `type` tag).
/// Only the fields the harness exercises are emitted; the engine
/// serde-defaults the rest (`thinking_budget`, `compaction`).
fn turn_command_to_json(cmd: &crate::scenario::TurnCommand) -> serde_json::Value {
    use crate::scenario::TurnCommand;
    match cmd {
        TurnCommand::SetConfig {
            model,
            thinking,
            effort,
        } => {
            let mut obj = serde_json::Map::new();
            obj.insert("type".into(), serde_json::json!("set_config"));
            if let Some(m) = model {
                obj.insert("model".into(), serde_json::json!(m));
            }
            if let Some(t) = thinking {
                obj.insert("thinking".into(), serde_json::json!(t));
            }
            if let Some(e) = effort {
                obj.insert("effort".into(), serde_json::json!(e));
            }
            serde_json::Value::Object(obj)
        }
        TurnCommand::SetMode { mode } => serde_json::json!({
            "type": "set_mode",
            "mode": mode,
        }),
    }
}

/// D2: fold a `config_changed` event into `info_events` as a synthetic line.
///
/// `ScenarioResult` surfaces protocol notices through `info_events` (asserted
/// by [`crate::assertions::Assertion::InfoContains`]). Rather than add a
/// parallel typed field, we serialize the event's `capabilities` payload into
/// a single `"config_changed: {...}"` line so a scenario can assert both the
/// event's PRESENCE (`InfoContains("config_changed")`) and a field within it
/// (e.g. `InfoContains("\"current_mode\":\"force\"")`).
fn capture_config_changed(ev: &Value, info_events: &mut Vec<String>) {
    let caps = ev
        .get("capabilities")
        .map(ToString::to_string)
        .unwrap_or_default();
    info_events.push(format!("config_changed: {caps}"));
}

async fn drive_session(
    mut stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    scenario: &crate::scenario::Scenario,
) -> anyhow::Result<DriveOutput> {
    let mut reader = BufReader::new(stdout);
    // Cold-boot latency clock: from here (≈ just after spawn) to the `ready`
    // event = the engine's bootstrap time (a usability/perf metric).
    let drive_start = Instant::now();

    // Consume engine bootstrap output up to AND INCLUDING the `ready` event
    // before sending the first user message, so we don't race bootstrap. We
    // loop until we actually see `type == "ready"` rather than blindly reading
    // one line (finding #1): the engine can emit other JSON events around
    // `ready` (`mcp_ready`, `info`, …), and consuming one of those as the
    // "ready" would desync the whole session into a spurious Hung/RunnerError.
    let boot_time = {
        let mut saw_ready = false;
        for _ in 0..256 {
            match read_one_event(&mut reader).await? {
                Some(ev) => {
                    if ev.get("type").and_then(Value::as_str) == Some("ready") {
                        saw_ready = true;
                        break;
                    }
                    // Pre-/around-ready event (mcp_ready/info/…) — skip it.
                }
                None => anyhow::bail!("child stdout closed before the `ready` event"),
            }
        }
        if !saw_ready {
            anyhow::bail!("did not observe a `ready` event within 256 stdout lines");
        }
        drive_start.elapsed()
    };

    let mut trace = ToolTrace::default();
    let mut final_text = String::new();
    let mut turn_results: Vec<TurnResult> = Vec::new();
    let mut runner_error: Option<String> = None;
    let mut info_events: Vec<String> = Vec::new();
    // D3: how to answer the engine's approval gate (only fires when the
    // scenario spawned WITHOUT `--yolo`, i.e. policy != Yolo).
    let approval = scenario.approval;
    // Lane B: capture tool INPUT args. The `tool_request` event carries the
    // model-supplied arguments (under `tool.args`) and arrives BEFORE the
    // matching `tool_result` (which carries only output). We stash the input
    // JSON keyed by `call_id` here, then attach it to the `TraceEntry` when the
    // result lands. Best-effort: a result with no pending request falls back to
    // empty input (the prior behaviour).
    let mut pending_inputs: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // #278 — `session_cost` is emitted by the engine BEFORE `stream_end`
    // (engine.rs `fire_on_session_end` runs inside `engine.run()`; the
    // json-stream loop emits `stream_end` only after `engine.run()` returns).
    // The per-turn loop below therefore MUST capture cost events as they fly
    // by — the post-stop drain is too late for the common one-turn case.
    let mut cost: Option<CostReport> = None;

    for (turn_idx, turn) in scenario.turns.iter().enumerate() {
        let turn_start = Instant::now();

        // D2: send this turn's pre-commands (`set_config` / `set_mode`) BEFORE
        // the user message. These are between-turn protocol commands; the
        // engine applies them synchronously (the standalone-command arms in
        // wcore-cli/src/main.rs), emitting `info` + (on a real change)
        // `config_changed`. We drain that response inline (below) into
        // `info_events` so it doesn't bleed into the turn's message stream.
        // Sending pre-commands first means the model swap / mode change is in
        // effect for this turn.
        for pre in &turn.pre_commands {
            let pre_cmd = turn_command_to_json(pre);
            let mut pline = serde_json::to_vec(&pre_cmd)?;
            pline.push(b'\n');
            stdin.write_all(&pline).await?;
            stdin.flush().await?;
            // Drain the standalone-command response so its events land in
            // `info_events` and don't bleed into the turn's message stream.
            //
            // Event order for a standalone set_config/set_mode (the arms in
            // wcore-cli/src/main.rs): on a REAL change the engine emits `info`
            // (the "config updated: …" / "mode updated: …" ack) FOLLOWED by
            // `config_changed`; on a NO-OP set_config it emits only `info`
            // ("set_config: no changes") with NO `config_changed`. So we treat
            // `config_changed` as the terminal event when a change happened,
            // and a no-op `info` as terminal otherwise. Bounded so neither
            // case can hang us.
            for _ in 0..16 {
                match read_turn_event(&mut reader, turn_idx, turn_start, turn.max_time).await? {
                    Some(ev) => {
                        let ty = ev.get("type").and_then(Value::as_str).unwrap_or("");
                        if ty == "config_changed" {
                            capture_config_changed(&ev, &mut info_events);
                            // Terminal for a successful change.
                            break;
                        } else if ty == "info" {
                            let m = ev
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let is_noop = m.contains("no changes");
                            info_events.push(m);
                            // A no-op set_config emits no `config_changed`;
                            // stop here. Otherwise keep draining for the
                            // trailing `config_changed`.
                            if is_noop {
                                break;
                            }
                        }
                        // Skip anything else (stray events) within the bound.
                    }
                    None => anyhow::bail!(
                        "child stdout closed while applying pre-command for turn {turn_idx}"
                    ),
                }
            }
        }

        // Wire format per crates/wcore-protocol/src/commands.rs:9
        // `ProtocolCommand::Message { msg_id, content }`. The plan
        // showed `{"type":"user_message","text":...}` — that is
        // wrong; the actual command variant is `message` + `content`.
        let msg_id = format!("eval-t{turn_idx}");
        let cmd = serde_json::json!({
            "type": "message",
            "msg_id": msg_id,
            "content": turn.prompt,
        });
        let mut line = serde_json::to_vec(&cmd)?;
        line.push(b'\n');
        stdin.write_all(&line).await?;
        stdin.flush().await?;

        let mut turn_text = String::new();
        // D2: Stop-mid-turn — send `stop` once, right after the first event
        // of this turn arrives, then expect the engine to halt the run future
        // and emit `stream_end`. `stop_pending` flips false after the send so
        // it fires exactly once.
        let mut stop_pending = turn.stop_mid_turn;

        loop {
            let ev = match read_turn_event(&mut reader, turn_idx, turn_start, turn.max_time).await {
                Ok(Some(ev)) => ev,
                Ok(None) => {
                    runner_error = Some(format!(
                        "child stdout closed mid-turn {turn_idx} (no stream_end)"
                    ));
                    break;
                }
                Err(e) => {
                    if e.downcast_ref::<TurnTimeout>().is_some() {
                        return Err(e);
                    }
                    runner_error = Some(format!("stdout decode error: {e}"));
                    break;
                }
            };
            // D2: Stop-mid-turn — the first event of the turn proves the
            // engine started working; send `stop` now to cancel it. The engine
            // breaks its `engine.run()` future (the `ProtocolCommand::Stop` arm
            // in the in-turn select loop) and emits this turn's `stream_end`,
            // which ends the loop below normally.
            if stop_pending {
                stop_pending = false;
                let stop_cmd = serde_json::json!({"type": "stop"});
                let mut sline = serde_json::to_vec(&stop_cmd)?;
                sline.push(b'\n');
                stdin.write_all(&sline).await?;
                stdin.flush().await?;
            }

            // Dispatch by "type" tag — same model the W0 host decoder
            // contract uses. Unknown variants are silently dropped
            // (forward-compat).
            let ty = ev.get("type").and_then(Value::as_str).unwrap_or("");
            match ty {
                "text_delta" => {
                    if let Some(t) = ev.get("text").and_then(Value::as_str) {
                        turn_text.push_str(t);
                    }
                }
                "tool_request" => {
                    // Record the model-supplied input args keyed by call_id so
                    // the matching `tool_result` can attach them. Per
                    // wcore_protocol::events::ProtocolEvent::ToolRequest the
                    // args live at `tool.args` (a JSON Value); fall back to a
                    // top-level `input`/`arguments` field for forward-compat.
                    let call_id = ev
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let input_val = ev
                        .get("tool")
                        .and_then(|t| t.get("args"))
                        .or_else(|| ev.get("input"))
                        .or_else(|| ev.get("arguments"));
                    if !call_id.is_empty()
                        && let Some(v) = input_val
                    {
                        pending_inputs.insert(call_id.clone(), v.to_string());
                    }
                    // D3: in non-`Yolo` mode the engine emits `tool_request`
                    // and then BLOCKS on `request_approval(call_id)` (no
                    // separate `approval_required` event for normal tools — the
                    // `tool_request` IS the approval prompt, same as the TUI).
                    // Answer per policy. `approve`/`resolve` are no-ops on a
                    // call_id with no pending approval (auto-approved tools like
                    // Read/Glob), so responding to every request is safe.
                    if let Some(cmd) = approval_command(approval, &call_id) {
                        let mut line = serde_json::to_vec(&cmd)?;
                        line.push(b'\n');
                        stdin.write_all(&line).await?;
                        stdin.flush().await?;
                    }
                }
                "tool_result" => {
                    let call_id = ev
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let tool_name = ev
                        .get("tool_name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let output = ev
                        .get("output")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let is_error = ev.get("status").and_then(Value::as_str) == Some("error");
                    // Attach the pending input captured from `tool_request`;
                    // empty (prior behaviour) when no matching request was seen.
                    let input = pending_inputs.remove(&call_id).unwrap_or_default();
                    trace.entries.push(TraceEntry {
                        call_id,
                        tool_name,
                        input,
                        output,
                        is_error,
                        duration: None,
                        turn: turn_idx,
                    });
                }
                "stream_end" => {
                    // Only THIS turn's stream_end ends the turn. The engine
                    // echoes the msg_id we sent (`set_current_msg_id` +
                    // `emit_stream_end(&msg_id)` in the json-stream loop), so a
                    // stream_end carrying a different id is stray and must not
                    // cut the turn short (finding #2 — multi-turn boundary
                    // desync). Absent msg_id → accept (forward-compat).
                    match ev.get("msg_id").and_then(Value::as_str) {
                        Some(id) if id == msg_id.as_str() => break,
                        None => break,
                        Some(_) => { /* stray stream_end for another msg_id; keep reading */ }
                    }
                }
                "error" => {
                    let err = ev.get("error").map(ToString::to_string).unwrap_or_default();
                    runner_error = Some(format!("engine emitted error: {err}"));
                    // Don't break — wait for stream_end if the engine
                    // still emits one. If it doesn't, the outer
                    // timeout will catch us.
                }
                "session_cost" => {
                    // #278 — capture in-band; this event arrives BEFORE
                    // stream_end on the wire and would otherwise fall into
                    // `_ => {}` and be dropped, leaving `cost_usd == 0.0`.
                    if let Some(c) = crate::cost::parse(&ev) {
                        cost = Some(c);
                    }
                }
                "info" => {
                    // D1/D2: capture engine notices + slash-command acks
                    // ("style updated", "mode updated: …", "conversation cleared").
                    if let Some(m) = ev.get("message").and_then(Value::as_str) {
                        info_events.push(m.to_string());
                    }
                }
                "config_changed" => {
                    // D2: a SetConfig/SetMode that lands DURING a turn (the
                    // engine queues set_config and applies it after the current
                    // response, emitting config_changed inline). Capture its
                    // capabilities into info_events as a synthetic line so
                    // `Assertion::InfoContains("config_changed")` /
                    // `InfoContains("current_mode\":\"force")` can assert it.
                    capture_config_changed(&ev, &mut info_events);
                }
                "approval_required" => {
                    // D3: the Script-tool approval gate emits this dedicated
                    // event (normal tools use `tool_request` above). Answer it
                    // with the same policy.
                    let call_id = ev
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if let Some(cmd) = approval_command(approval, &call_id) {
                        let mut line = serde_json::to_vec(&cmd)?;
                        line.push(b'\n');
                        stdin.write_all(&line).await?;
                        stdin.flush().await?;
                    }
                }
                _ => {}
            }
        }

        let elapsed = turn_start.elapsed();
        // `final_text` reflects the MOST RECENT turn's assistant text, even if
        // empty (finding #7) — a final turn that produced only tool calls must
        // not leave `final_text` showing a stale earlier turn's prose.
        final_text = turn_text.clone();
        turn_results.push(TurnResult {
            turn: turn_idx,
            prompt: turn.prompt.clone(),
            assistant_text: turn_text,
            wall_time: elapsed,
        });

        if runner_error.is_some() {
            break;
        }
    }

    // End-of-session: send `stop` and drain remaining events. The
    // primary capture for `session_cost` is inside the per-turn loop
    // above (per #278 — it arrives BEFORE `stream_end`). This drain
    // exists to (a) read the pipe to EOF so the child can exit cleanly
    // and (b) catch a cost event that the engine might emit late under
    // a future schema change.
    let stop_cmd = serde_json::json!({"type": "stop"});
    let mut stop_line = serde_json::to_vec(&stop_cmd)?;
    stop_line.push(b'\n');
    let _ = stdin.write_all(&stop_line).await;
    let _ = stdin.flush().await;
    drop(stdin); // close stdin so the engine's command reader sees EOF

    loop {
        match read_one_event(&mut reader).await {
            Ok(Some(ev)) => {
                if let Some(c) = crate::cost::parse(&ev) {
                    cost = Some(c);
                }
                // Drain to EOF either way — leaving bytes in the pipe
                // can stall the child's exit.
            }
            Ok(None) => break,
            Err(_e) => break,
        }
    }

    Ok(DriveOutput {
        turn_results,
        trace,
        final_text,
        cost,
        runner_error,
        boot_time,
        info_events,
    })
}

/// Read one newline-delimited JSON event from the engine's stdout as a
/// `serde_json::Value`. Returns `Ok(None)` on EOF. Blank lines are
/// skipped. Lines that don't parse are silently dropped (W0 host
/// decoder contract — tolerate unknown / forward-additive shapes).
async fn read_one_event<R>(reader: &mut BufReader<R>) -> anyhow::Result<Option<Value>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    const MAX_STDOUT_EVENT_BYTES: usize = 64 * 1024;
    loop {
        let mut line = Vec::new();
        let complete = loop {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                break !line.is_empty();
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(available.len(), |index| index + 1);
            if line.len().saturating_add(consumed) > MAX_STDOUT_EVENT_BYTES {
                anyhow::bail!("stdout event exceeded {MAX_STDOUT_EVENT_BYTES} bytes");
            }
            line.extend_from_slice(&available[..consumed]);
            reader.consume(consumed);
            if newline.is_some() {
                break true;
            }
        };
        match complete {
            false => return Ok(None),
            true => {
                if line.last() == Some(&b'\n') {
                    line.pop();
                }
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                let line = std::str::from_utf8(&line)
                    .map_err(|_| anyhow::anyhow!("stdout event was not valid UTF-8"))?;
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(line) {
                    Ok(value) => return Ok(Some(value)),
                    Err(_) => continue,
                }
            }
        }
    }
}
