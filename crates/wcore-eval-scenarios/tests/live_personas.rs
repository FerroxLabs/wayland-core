//! Lane B overnight persona harness — the runnable entry point.
//!
//! Plan: `.planning/2026-06-04-e2e-and-wiring-masterplan.md`.
//!
//! This drives the REAL `wayland-core` binary against the real DeepSeek
//! provider across every persona journey in [`wcore_eval_scenarios::personas`]
//! and writes a markdown report of the outcomes. It is `#[ignore]`'d so the
//! normal `cargo test` / CI floor never spends money or needs the network — it
//! only runs when invoked explicitly, mirroring the eval-gate pattern:
//!
//! ```text
//! WAYLAND_ALLOW_NO_SANDBOX=1 DEEPSEEK_API_KEY=... \
//!   cargo nextest run -p wcore-eval-scenarios --test live_personas --run-ignored all
//! ```
//!
//! Design note: an overnight run must COMPLETE and REPORT, not abort on the
//! first failing persona. So we collect every result and emit the report
//! before asserting the aggregate acceptance result. Failed or missing scenarios
//! and report-write failures cannot certify successful behavior.

use std::fmt::Write as _;
use std::path::PathBuf;

use wcore_eval_scenarios::catalog;
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::{ScenarioResult, discover_binary};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: drives the real wayland-core binary against the real DeepSeek API (costs money, needs DEEPSEEK_API_KEY + a pre-built binary)"]
async fn overnight_personas() {
    assert!(
        std::env::var("DEEPSEEK_API_KEY").is_ok_and(|key| !key.trim().is_empty()),
        "DEEPSEEK_API_KEY is required for explicitly selected persona acceptance"
    );

    // 2. Resolve the binary. discover_binary() honours WCORE_EVAL_BIN and walks
    //    target/{release,debug}/wayland-core. Require it — a live run with no
    //    binary is operator error, so panic with an actionable message.
    let bin = match discover_binary() {
        Ok(p) => p,
        Err(e) => panic!(
            "wayland-core binary not found ({e}). \
             Pre-build it with `cargo build -p wcore-cli` (or set WCORE_EVAL_BIN)."
        ),
    };
    eprintln!("overnight_personas: using binary at {}", bin.display());

    // 3. Provider — key resolves from DEEPSEEK_API_KEY via resolved_key().
    let provider = ProviderConfig::new(ProviderId::DeepSeek, "deepseek-v4-pro");

    // 4. Run each scenario, collecting results. Don't abort on failure.
    //    Lane U (personas, user-testing) + Lane Q (qa, coverage) run together;
    //    the report's usability punch list spans both.
    let mut scenarios = catalog::standard_scenarios().expect("standard scenario catalog is valid");
    // Optional subset filter: WCORE_EVAL_ONLY=<substring> runs only scenarios
    // whose name contains the substring (e.g. `approval`, `qa_`, `coder`) — for
    // fast targeted verification without a full 12-scenario, money-spending run.
    if let Ok(filter) = std::env::var("WCORE_EVAL_ONLY") {
        scenarios.retain(|s| s.name.contains(&filter));
        eprintln!(
            "overnight_personas: WCORE_EVAL_ONLY={filter} → {} scenario(s)",
            scenarios.len()
        );
    }
    let total = scenarios.len();
    let mut results: Vec<ScenarioResult> = Vec::with_capacity(total);
    let mut runner_errors = Vec::new();

    for (idx, scenario) in scenarios.iter().enumerate() {
        eprintln!(
            "overnight_personas: [{}/{}] running '{}' ...",
            idx + 1,
            total,
            scenario.name
        );
        match scenario.run_with(&provider).await {
            Ok(result) => {
                eprintln!(
                    "overnight_personas: [{}/{}] '{}' -> {} ({:.1}s, ${:.4})",
                    idx + 1,
                    total,
                    result.name,
                    if result.passed { "PASS" } else { "FAIL" },
                    result.wall_time.as_secs_f64(),
                    result.cost_usd,
                );
                let canary_failed = result.name == "canary" && !result.passed;
                results.push(result);
                // Canary gate: if the cheapest round-trip (provider + model +
                // key + wire) can't pass, the whole run is misconfigured — abort
                // before the multi-turn journeys spend real money. The report is
                // still written below with the canary's failure as the story.
                if canary_failed {
                    eprintln!(
                        "overnight_personas: CANARY FAILED — aborting the suite \
                         (run is misconfigured: check DEEPSEEK_API_KEY, the model \
                         name, and the binary). See the canary failures in the report."
                    );
                    break;
                }
            }
            Err(e) => {
                runner_errors.push(format!("{}: {e}", scenario.name));
                // run() itself returning Err is a harness/plumbing fault (not a
                // scenario assertion failure). Record it loudly but keep going —
                // unless it's the canary, in which case the run is misconfigured.
                eprintln!(
                    "overnight_personas: [{}/{}] '{}' -> RUNNER ERROR: {e}",
                    idx + 1,
                    total,
                    scenario.name
                );
                if scenario.name == "canary" {
                    eprintln!(
                        "overnight_personas: CANARY runner-errored — aborting the \
                         suite (binary/provider misconfigured)."
                    );
                    break;
                }
            }
        }

        // Inter-scenario pacing. DeepSeek (and most providers) throttle rapid
        // sequential bursts; without a gap, a heavy persona's tail can leave the
        // next persona's first request being reset mid-stream (~60s idle → drop).
        // A short pause between personas lets the provider's rate window recover.
        // Overridable via WCORE_EVAL_PACING_SECS; skipped after the last persona.
        if idx + 1 < total {
            let pacing_secs = std::env::var("WCORE_EVAL_PACING_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(15);
            if pacing_secs > 0 {
                eprintln!("overnight_personas: pacing {pacing_secs}s before next persona…");
                tokio::time::sleep(std::time::Duration::from_secs(pacing_secs)).await;
            }
        }
    }

    // 5. Write the markdown report. `Date`/time is not available without an
    //    extra dep, so we use a fixed filename.
    let mut report = render_report(&results);
    let _ = writeln!(
        report,
        "Selected: {total}; completed: {}; runner errors: {}.",
        results.len(),
        runner_errors.len()
    );
    for error in &runner_errors {
        let _ = writeln!(report, "- RUNNER ERROR: {error}");
    }
    let report_path = report_output_path();
    // Emit all collected outcomes even if the durable report cannot be written.
    eprintln!("\n{report}");
    if let Some(parent) = report_path.parent() {
        std::fs::create_dir_all(parent).expect("create persona report directory");
    }
    std::fs::write(&report_path, &report).expect("write persona acceptance report");
    println!(
        "overnight_personas: report written to {}",
        report_path.display()
    );
    let passed = results.iter().filter(|r| r.passed).count();
    assert!(total > 0, "persona acceptance selected zero scenarios");
    assert!(
        runner_errors.is_empty(),
        "persona runner errors: {runner_errors:?}"
    );
    assert_eq!(
        results.len(),
        total,
        "persona acceptance did not execute every selected scenario"
    );
    assert_eq!(
        passed,
        total,
        "persona acceptance failed; collected report: {}",
        report_path.display()
    );
}

/// `target/persona-report.md`, resolved against the workspace target dir.
fn report_output_path() -> PathBuf {
    // Keep fixture runs and parallel live runs from overwriting one report.
    if let Some(path) = std::env::var_os("WCORE_EVAL_REPORT_PATH") {
        return PathBuf::from(path);
    }
    // CARGO_MANIFEST_DIR = crates/wcore-eval-scenarios; workspace root is two
    // levels up. Mirrors discover_binary()'s target-dir resolution.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or(manifest_dir);
    workspace_root.join("target").join("persona-report.md")
}

/// Render the collected results as a markdown report.
fn render_report(results: &[ScenarioResult]) -> String {
    let mut out = String::new();
    let passed = results.iter().filter(|r| r.passed).count();
    let _ = writeln!(out, "# Overnight Persona Report\n");
    let _ = writeln!(out, "{}/{} personas passed.\n", passed, results.len());

    // Usability/Krug punch list (D10) — advisory findings the functional
    // PASS/FAIL won't catch (optional-feature nagging, broken subsystems,
    // boot/turn latency, tool errors). Rendered up top so it's the first thing
    // we triage; deduped across scenarios.
    let mut usability_findings = Vec::new();
    for r in results {
        usability_findings.extend(wcore_eval_scenarios::usability::scan(r));
    }
    out.push_str(&wcore_eval_scenarios::usability::render_punch_list(
        &usability_findings,
    ));
    out.push('\n');

    for r in results {
        let status = if r.passed { "PASS" } else { "FAIL" };
        let _ = writeln!(out, "## {} — {}\n", r.name, status);
        let _ = writeln!(out, "- provider: {}", r.provider);
        let _ = writeln!(out, "- wall_time: {:.1}s", r.wall_time.as_secs_f64());
        let _ = writeln!(out, "- boot_time: {:.2}s", r.boot_time.as_secs_f64());
        let _ = writeln!(out, "- cost_usd: ${:.4}", r.cost_usd);
        let _ = writeln!(out, "- workdir: {}", r.workdir.display());

        // Per-turn wall time — surfaces WHERE in the journey a turn stalled
        // (e.g. a uniform ~60s before a connection error points at a provider
        // idle-reset / throttle, not a code bug).
        if !r.turn_results.is_empty() {
            let timings: Vec<String> = r
                .turn_results
                .iter()
                .map(|t| format!("t{}={:.1}s", t.turn, t.wall_time.as_secs_f64()))
                .collect();
            let _ = writeln!(out, "- turn times: {}", timings.join(", "));
        }

        // Tools used (from the live trace).
        if r.trace.entries.is_empty() {
            let _ = writeln!(out, "- tools used: (none)");
        } else {
            let tools: Vec<&str> = r
                .trace
                .entries
                .iter()
                .map(|e| e.tool_name.as_str())
                .collect();
            let _ = writeln!(out, "- tools used: {}", tools.join(", "));
        }

        // Failures (debug-printed, one per line).
        if r.failures.is_empty() {
            let _ = writeln!(out, "- failures: (none)");
        } else {
            let _ = writeln!(out, "- failures:");
            for f in &r.failures {
                let _ = writeln!(out, "    - {f:?}");
            }
        }

        // Agent's final reply (truncated) on FAIL — the triage data that tells
        // a real engine bug from a too-narrow assertion (e.g. an honest
        // "No file exists" reply that the phrasing list missed). Avoids a repro.
        if !r.passed && !r.final_text.trim().is_empty() {
            let reply: String = r.final_text.trim().chars().take(500).collect();
            let _ = writeln!(
                out,
                "- agent reply (truncated):\n  > {}",
                reply.replace('\n', "\n  > ")
            );
        }

        // Engine stderr tail — the actual provider/engine logs at failure time.
        // Without this the report can't distinguish an engine bug from a
        // provider-side connection reset. Only emitted for FAILs to keep PASS
        // entries terse.
        if !r.passed && !r.stderr_tail.trim().is_empty() {
            let _ = writeln!(out, "- stderr tail:\n```\n{}\n```", r.stderr_tail.trim());
        }
        out.push('\n');
    }

    out
}

/// Zero-execution guard — and it has to RUN to be one.
///
/// Every test in this binary is `#[ignore]`d, so `cargo test --test live_personas`
/// executes 0 of 1 and still exits 0 printing `test result: ok`. This guard is
/// deliberately NOT `#[ignore]`d: three suites in this repo carried a guard that
/// was itself ignored, which made each inert against precisely the scenario it
/// existed for — it could only fire under `--ignored`, by which point the real
/// case were running anyway.
///
/// It always runs, so this binary can never report success on zero executed
/// tests, and it FAILS when a caller sets `WAYLAND_REQUIRE_IGNORED=1` to declare a run of the
/// ignored case while passing an invocation that cannot execute any of them.
/// Skipped under nextest, whose `no-tests = "fail"` policy covers the same
/// ground at the invocation site.
#[test]
fn zero_execution_guard() {
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    if std::env::var("WAYLAND_REQUIRE_IGNORED").as_deref() != Ok("1") {
        return;
    }
    let asked_for_ignored = std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    assert!(
        asked_for_ignored,
        "declared intent to run this suite's 1 #[ignore]d case, but neither \
         --ignored nor --include-ignored was passed, so zero of them can execute. \
         Exiting 0 here would certify nothing. Re-run with: \
         cargo test -p wcore-eval-scenarios --test live_personas -- --ignored --test-threads=1"
    );
}
