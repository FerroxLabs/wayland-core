//! D4 — cross-session keystone: live memory-recall probe.
//!
//! The masterplan headline. Two SEPARATE `wayland-core` processes share one
//! persistent `WAYLAND_HOME`: session 1 is told a fact; session 2 cold-boots
//! and is asked to recall it. If recall works, memory genuinely survives across
//! sessions. If it doesn't, that FAIL is the valuable proof of the v2
//! memory-recall gap (stored but never re-injected into the prompt).
//!
//! Like `live_personas`, this is `#[ignore]`'d (it costs money, needs the
//! network, and needs a pre-built binary) and is a REPORT, not a hard gate: a
//! recall MISS is printed as data, not asserted into a red test — the harness's
//! job is to surface the engine's real behavior, not to pretend recall works.
//!
//! ```text
//! WAYLAND_ALLOW_NO_SANDBOX=1 \
//!   DEEPSEEK_API_KEY="$(security find-generic-password -a deepseek_api_key -w)" \
//!   WCORE_EVAL_BIN="$PWD/target/release/wayland-core" \
//!   vx cargo test -p wcore-eval-scenarios --test cross_session_live \
//!     -- --ignored --exact memory_recall_across_sessions --nocapture
//! ```

use std::time::Duration;

use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::discover_binary;
use wcore_eval_scenarios::scenario::{Category, Scenario, Turn};
use wcore_eval_scenarios::{Assertion, run_cross_session};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: two real wayland-core sessions vs real DeepSeek (costs money, needs DEEPSEEK_API_KEY + a pre-built binary)"]
async fn memory_recall_across_sessions() {
    if std::env::var("DEEPSEEK_API_KEY").is_err() {
        eprintln!(
            "SKIP memory_recall_across_sessions: DEEPSEEK_API_KEY is not set. \
             Set it (and pre-build the binary) to run the cross-session keystone."
        );
        return;
    }
    match discover_binary() {
        Ok(p) => eprintln!(
            "memory_recall_across_sessions: using binary at {}",
            p.display()
        ),
        Err(e) => panic!(
            "wayland-core binary not found ({e}). \
             Pre-build it with `cargo build -p wcore-cli` (or set WCORE_EVAL_BIN)."
        ),
    }

    let provider = ProviderConfig::new(ProviderId::DeepSeek, "deepseek-v4-pro");

    // Session 1: state a memorable, unambiguous fact and ask for a plain
    // acknowledgement. The fact must land in a CROSS-session tier (project /
    // global), which the dream cycle consolidates at session-end (un-throttled
    // by the D4 config).
    let store = Scenario::new("xsession_store", Category::Multiturn)
        .max_total_time(Duration::from_secs(150))
        .turn(
            Turn::new(
                "Please remember this about me for future conversations: my \
                 favorite color is teal. Just briefly confirm you've noted it.",
            )
            .max_time(Duration::from_secs(120)),
        );

    // Session 2: a FRESH process (same home). No in-context history — the only
    // way to answer is genuine recall from persisted memory.
    let recall = Scenario::new("xsession_recall", Category::Multiturn)
        .max_total_time(Duration::from_secs(150))
        .turn(
            Turn::new("What is my favorite color? Answer in a single word.")
                .max_time(Duration::from_secs(120))
                .assert(Assertion::ContainsAny(vec!["teal", "Teal", "TEAL"])),
        );

    let results = run_cross_session(&[store, recall], &provider)
        .await
        .expect("cross-session run should complete (plumbing must not error)");

    // Report — the artifact. Both sessions' PASS/FAIL is data.
    eprintln!("\n===== D4 CROSS-SESSION KEYSTONE: memory recall =====");
    for r in &results {
        eprintln!(
            "  [{}] {} — {:.1}s, boot {:.2}s, tools: [{}]",
            r.name,
            if r.passed { "PASS" } else { "FAIL" },
            r.wall_time.as_secs_f64(),
            r.boot_time.as_secs_f64(),
            r.trace
                .entries
                .iter()
                .map(|e| e.tool_name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        if !r.failures.is_empty() {
            for f in &r.failures {
                eprintln!("      - failure: {f:?}");
            }
        }
        if !r.final_text.trim().is_empty() {
            let reply: String = r.final_text.trim().chars().take(300).collect();
            eprintln!("      reply: {}", reply.replace('\n', " "));
        }
        if !r.passed && !r.stderr_tail.trim().is_empty() {
            eprintln!("      stderr tail:\n{}", r.stderr_tail.trim());
        }
    }

    let recall_result = results
        .iter()
        .find(|r| r.name == "xsession_recall")
        .expect("recall session must be present");
    if recall_result.passed {
        eprintln!(
            "\n  ✅ RECALL WORKS: session 2 recovered 'teal' from persisted memory \
             across a cold process boundary."
        );
    } else {
        eprintln!(
            "\n  ⚠️  RECALL MISS: session 2 did NOT recall 'teal'. This is the \
             expected v2 memory-recall gap (facts are stored but not re-injected \
             into a fresh session's prompt). Captured as data — see the reply + \
             stderr above. NOT failing the test; the keystone's job is to surface \
             the gap, not pretend recall already works."
        );
    }
    eprintln!("====================================================\n");

    // The test passes as long as the cross-session run COMPLETED. Recall
    // pass/fail is the finding, reported above — not a red test.
    assert_eq!(results.len(), 2, "both sessions must have run");
}

/// Zero-execution guard — and it has to RUN to be one.
///
/// Every test in this binary is `#[ignore]`d, so `cargo test --test cross_session_live`
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
         cargo test -p wcore-eval-scenarios --test cross_session_live -- --ignored --test-threads=1"
    );
}
