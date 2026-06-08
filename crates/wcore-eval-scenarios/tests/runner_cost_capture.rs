//! Regression test for #278 — R-009 vs S4 cost-event divergence.
//!
//! The bug: `drive_session` in `runner.rs` only captured `session_cost`
//! in the post-stop drain, but the engine emits `session_cost` BEFORE
//! `stream_end` on the wire (per `engine.rs::fire_on_session_end`, which
//! runs inside `engine.run()`; the json-stream loop emits `stream_end`
//! only after `engine.run()` resolves). The per-turn event loop broke on
//! `stream_end` and never looked at `session_cost`, so `result.cost_usd`
//! stayed `0.0` and `Assertion::CostWithinTolerance` silently FAILed
//! despite the engine having emitted a perfectly correct cost number.
//!
//! Wave 1.1 R-009 caught this because it asserts cost via the harness;
//! the parallel hand-driven cross-cut S4 (which scans every event for
//! `type=session_cost` without making any order assumption) reported the
//! same engine emitting the right cost. The bug was on the harness side.
//!
//! This test reproduces the exact wire ordering against a fake binary
//! (a small shell script playing back canned events), driven through
//! the real `runner::run` pipeline. No API key needed; no network call;
//! deterministic in <1s.

// All imports are only used by the `#[cfg(unix)]` test body below — the
// fake binary is a POSIX shell script and the regression repro requires
// it. Gate the imports to match so Windows clippy (`-D warnings`) doesn't
// flag them as unused.
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
#[cfg(unix)]
use wcore_eval_scenarios::runner::run;
#[cfg(unix)]
use wcore_eval_scenarios::scenario::{Category, Scenario, Turn};

/// Write a POSIX shell script to `path` that mimics a `wayland-core
/// --json-stream` child for one turn — emitting events in the SAME
/// order the real engine does (#278 — `session_cost` BEFORE
/// `stream_end`).
///
/// The script accepts (and ignores) any CLI args, reads & discards one
/// `message` JSON line from stdin, then prints the canned event
/// sequence and exits 0. The runner sends a `stop` after the message
/// loop completes; we drain stdin to EOF so the child exits cleanly.
#[cfg(unix)]
fn write_fake_binary(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    // The runner expects:
    //   1) first line = `ready` event (consumed before the first send)
    //   2) per-turn loop reads events until `stream_end`
    //   3) post-stop drain reads remaining events to EOF
    //
    // Wire order matches engine.rs: text_delta* → session_cost → stream_end.
    // The `session_cost` payload mirrors `wcore-protocol::events::SessionCost`.
    let script = r#"#!/bin/sh
# Mock wayland-core --json-stream for runner cost-capture regression (#278).
# Emit ready, consume one user message, emit one turn's events in the
# real engine's order, then drain stdin to EOF and exit 0.
printf '{"type":"ready","capabilities":{"cost_attribution":true}}\n'
# Read & discard one user-message line.
IFS= read -r _msg
printf '{"type":"text_delta","msg_id":"eval-t0","text":"ok"}\n'
printf '{"type":"session_cost","session_id":"fake","total_cost_usd":0.00185985,"per_turn":[{"turn":0,"model":"gpt-4o-mini","provider":"openai","cost_usd":0.00185985}]}\n'
printf '{"type":"stream_end","msg_id":"eval-t0","finish_reason":"stop","turns":1,"usage":{"input_tokens":12,"output_tokens":1,"cache_creation_tokens":0,"cache_read_tokens":0}}\n'
# Drain remaining stdin lines (the runner's `stop` arrives here) so the
# parent's `drop(stdin)` is observed as EOF.
while IFS= read -r _line; do :; done
exit 0
"#;

    let mut f = std::fs::File::create(path).expect("create fake bin");
    f.write_all(script.as_bytes()).expect("write fake bin");
    drop(f);
    let mut perm = std::fs::metadata(path).expect("metadata").permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(path, perm).expect("chmod +x");
}

/// Regression for #278. Drive the real `runner::run` against a fake
/// binary that emits `session_cost` BEFORE `stream_end` (mirroring the
/// real engine's wire order) and assert `result.cost_usd > 0`.
///
/// Pre-fix, the cost event fell into the per-turn loop's `_ => {}` arm
/// and was silently dropped — `cost_usd` ended up `0.0` and
/// `Assertion::CostWithinTolerance` FAILed despite a valid emission.
#[cfg(unix)]
#[tokio::test]
async fn captures_session_cost_emitted_before_stream_end() {
    let tmp = tempfile::TempDir::new().expect("tempdir for fake bin");
    let fake = tmp.path().join("wayland-core");
    write_fake_binary(&fake);

    // Point the runner at the fake.
    // SAFETY: tests in this crate run single-threaded (eval profile);
    // no sibling reads WCORE_EVAL_BIN concurrently.
    let prev = std::env::var_os("WCORE_EVAL_BIN");
    unsafe { std::env::set_var("WCORE_EVAL_BIN", &fake) };

    let scenario = Scenario::new(
        "captures_session_cost_emitted_before_stream_end",
        Category::Coverage,
    )
    .max_total_time(Duration::from_secs(10))
    .turn(
        Turn::new("anything")
            .max_time(Duration::from_secs(5))
            .max_steps(1),
    );

    let provider = ProviderConfig::new(ProviderId::OpenAI, "gpt-4o-mini")
        .with_api_key("test-key-never-used".to_string());

    let result = run(&scenario, &provider).await.expect("run completes");

    // Restore env before any panic so sibling tests aren't poisoned.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WCORE_EVAL_BIN", v),
            None => std::env::remove_var("WCORE_EVAL_BIN"),
        }
    }

    assert!(
        result.cost_usd > 0.0,
        "#278 regression: runner failed to capture `session_cost` event \
         emitted before `stream_end`. Got cost_usd={}; failures={:?}",
        result.cost_usd,
        result.failures
    );
    // The fake emits exactly $0.00185985 — make sure it round-trips
    // through cost::parse → ScenarioResult.cost_usd unchanged.
    assert!(
        (result.cost_usd - 0.00185985).abs() < 1e-9,
        "expected $0.00185985, got ${}",
        result.cost_usd
    );
}
