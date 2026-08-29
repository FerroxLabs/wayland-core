//! F23-04 acceptance: `wayland-core cache` drives the shipped BINARY, not the
//! library function.
//!
//! Every assertion here runs `CARGO_BIN_EXE_wayland-core` as a child process
//! with a hermetic `--dir`. That is deliberate. Success Criterion 4 turns on
//! the four families being reachable by an operator; calling `cache_cmd::run`
//! in-process would prove the arithmetic and prove nothing about reachability —
//! a subcommand that is never registered in `TopCmd` would still pass such a
//! test. Going through the binary means the clap wiring, the dispatch arm and
//! the exit-code map are all in the assertion's path.
//!
//! Fixture ledgers are written as JSON rather than produced by a live session,
//! because a session that reaches a *cache miss for a stated reason* on demand
//! needs a provider. The live leg is a separate, recorded, real-provider run;
//! this suite covers the shapes that run cannot reproduce on command
//! (unpriced models, negative savings, failed compactions, an empty store).

use std::path::Path;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_wayland-core");

fn run(dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(BIN);
    cmd.arg("cache");
    cmd.args(args);
    cmd.arg("--dir").arg(dir);
    // Keep the child out of the developer's real Wayland home no matter what
    // the surrounding environment holds.
    cmd.env("WAYLAND_HOME", dir);
    cmd.output().expect("failed to run wayland-core cache")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

/// Pull `key=value` out of an `F23_CACHE=<kind> ...` line.
fn field(out: &str, kind: &str, key: &str) -> String {
    let line = out
        .lines()
        .find(|l| l.starts_with(&format!("F23_CACHE={kind} ")))
        .unwrap_or_else(|| panic!("no `F23_CACHE={kind}` line in:\n{out}"));
    for tok in line.split_whitespace().skip(1) {
        if let Some(v) = tok.strip_prefix(&format!("{key}=")) {
            return v.to_string();
        }
    }
    panic!("no `{key}=` on the F23_CACHE={kind} line:\n{line}");
}

fn f64_field(out: &str, kind: &str, key: &str) -> f64 {
    field(out, kind, key)
        .parse()
        .unwrap_or_else(|e| panic!("{kind}.{key} is not a number: {e}"))
}

// ── Fixture builders ────────────────────────────────────────────────────────

/// A turn as raw JSON so the fixture does not depend on the struct's field
/// order and a schema change shows up here as a decode failure.
#[allow(clippy::too_many_arguments)]
fn turn_json(
    round_trip: u64,
    uncached: u64,
    read: u64,
    write: u64,
    cost: f64,
    uncached_equiv: f64,
    cost_source: &str,
    cause: Option<&str>,
    watermark: u64,
) -> serde_json::Value {
    let mut v = serde_json::json!({
        "turn": round_trip - 1,
        "round_trip": round_trip,
        "ts": format!("2026-07-29T10:00:{:02}.000Z", round_trip),
        "provider": "anthropic",
        "model": "claude-opus-4-7",
        "retention": "ephemeral5m",
        "uncached_input_tokens": uncached,
        "cache_read_tokens": read,
        "cache_write_tokens": write,
        "output_tokens": 250,
        "cost_usd": cost,
        "cost_source": cost_source,
        "uncached_equivalent_usd": uncached_equiv,
        "watermark_tokens": watermark,
        "conservative_watermark_tokens": watermark + 1_000,
        "autocompact_threshold_tokens": 150_000u64,
        "emergency_limit_tokens": 197_000u64,
    });
    if let Some(c) = cause {
        v["invalidation_cause"] = serde_json::json!(c);
    }
    v
}

fn write_ledger(
    dir: &Path,
    session: &str,
    turns: Vec<serde_json::Value>,
    compactions: Vec<serde_json::Value>,
) {
    std::fs::create_dir_all(dir).unwrap();
    let ledger = serde_json::json!({
        // The schema this build WRITES, not a literal. Pinned at `1`, every
        // fixture here quietly became a legacy file the moment #1163 c4 bumped
        // the version, so the whole suite would have been grading the
        // migration path while claiming to grade the current one.
        "schema": wcore_agent::cache_ledger::LEDGER_SCHEMA,
        "session_id": session,
        "started_at": "2026-07-29T10:00:00.000Z",
        "updated_at": format!("2026-07-29T10:{:02}:00.000Z", turns.len().max(1)),
        "session_complete": true,
        "turns": turns,
        "compactions": compactions,
    });
    std::fs::write(
        dir.join(format!("{session}.json")),
        serde_json::to_vec_pretty(&ledger).unwrap(),
    )
    .unwrap();
}

/// A ledger in the shape v0.13.9 wrote: schema 1, and the counterfactual a
/// bare `0.0` because the model had no catalog rate. v1 had no way to say
/// "unknown", so `0.0` WAS how it said it — which is the whole of #1163 and,
/// once the field became `Option<f64>`, of wayland#1205.
fn write_v1_ledger(dir: &Path, session: &str, turns: Vec<serde_json::Value>) {
    std::fs::create_dir_all(dir).unwrap();
    let ledger = serde_json::json!({
        "schema": 1,
        "session_id": session,
        "started_at": "2026-07-29T10:00:00.000Z",
        "updated_at": "2026-07-29T10:04:00.000Z",
        "session_complete": true,
        "turns": turns,
        "compactions": [],
    });
    std::fs::write(
        dir.join(format!("{session}.json")),
        serde_json::to_vec_pretty(&ledger).unwrap(),
    )
    .unwrap();
}

/// A healthy, fully-priced session: cold open, then two warm hits.
fn healthy(dir: &Path, session: &str) {
    write_ledger(
        dir,
        session,
        vec![
            turn_json(
                1,
                20_000,
                0,
                20_000,
                0.0900,
                0.0750,
                "catalog",
                Some("no_marker"),
                20_000,
            ),
            turn_json(
                2,
                500,
                0,
                500,
                0.0030,
                0.0030,
                "catalog",
                Some("system_prompt_drift"),
                21_000,
            ),
            turn_json(3, 500, 20_000, 0, 0.0090, 0.0615, "catalog", None, 41_000),
            turn_json(4, 500, 20_500, 0, 0.0092, 0.0630, "catalog", None, 62_000),
        ],
        vec![],
    );
}

// ── Reachability ────────────────────────────────────────────────────────────

#[test]
fn cache_subcommand_is_reachable_from_the_shipped_binary() {
    // The single most important assertion in this file: `--help` for a
    // subcommand that was never registered exits non-zero. This is what makes
    // every other test in the file evidence of EXPOSURE rather than of
    // arithmetic.
    let out = Command::new(BIN)
        .args(["cache", "--help"])
        .output()
        .expect("failed to run binary");
    assert_eq!(
        code(&out),
        0,
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let help = stdout(&out);
    for verb in ["report", "list", "show", "verify"] {
        assert!(help.contains(verb), "`{verb}` missing from help:\n{help}");
    }

    // Known-negative for this instrument: a subcommand that genuinely does not
    // exist must fail. Without this, `--help` succeeding could mean clap is
    // accepting anything.
    let bogus = Command::new(BIN)
        .args(["cache-that-does-not-exist", "--help"])
        .output()
        .expect("failed to run binary");
    assert_ne!(
        code(&bogus),
        0,
        "an unknown subcommand exited 0 — the reachability check above is vacuous"
    );
}

// ── Quality ─────────────────────────────────────────────────────────────────

#[test]
fn report_exposes_cache_quality_and_separates_warm_from_cold() {
    let tmp = tempfile::tempdir().unwrap();
    healthy(tmp.path(), "sess-quality");
    let out = run(tmp.path(), &["report"]);
    assert_eq!(code(&out), 0);
    let o = stdout(&out);

    assert_eq!(field(&o, "quality", "hit_round_trips"), "2");
    assert_eq!(field(&o, "quality", "miss_round_trips"), "2");
    assert_eq!(field(&o, "quality", "cache_read"), "40500");
    assert_eq!(field(&o, "quality", "cache_write"), "20500");
    assert_eq!(field(&o, "quality", "uncached_input"), "21500");
    assert_eq!(field(&o, "quality", "total_input"), "82500");

    // Warm ratio must be strictly better than the cold-inclusive one, and the
    // warm window must exclude exactly the first two round-trips.
    assert_eq!(field(&o, "quality", "warm_round_trips"), "2");
    let all = f64_field(&o, "quality", "hit_ratio");
    let warm = f64_field(&o, "quality", "warm_hit_ratio");
    assert!(warm > all, "warm={warm} should exceed cold-inclusive={all}");
    assert!((warm - 40_500.0 / 41_500.0).abs() < 1e-4, "warm={warm}");
}

// ── Invalidation ────────────────────────────────────────────────────────────

#[test]
fn report_names_every_invalidation_cause_with_its_count() {
    let tmp = tempfile::tempdir().unwrap();
    write_ledger(
        tmp.path(),
        "sess-inval",
        vec![
            turn_json(
                1,
                1_000,
                0,
                0,
                0.01,
                0.01,
                "catalog",
                Some("no_marker"),
                1_000,
            ),
            turn_json(
                2,
                1_000,
                0,
                0,
                0.01,
                0.01,
                "catalog",
                Some("expired"),
                2_000,
            ),
            turn_json(
                3,
                1_000,
                0,
                0,
                0.01,
                0.01,
                "catalog",
                Some("expired"),
                3_000,
            ),
            turn_json(
                4,
                1_000,
                0,
                0,
                0.01,
                0.01,
                "catalog",
                Some("history_rewritten"),
                4_000,
            ),
        ],
        vec![],
    );
    let out = run(tmp.path(), &["report"]);
    let o = stdout(&out);

    assert_eq!(field(&o, "invalidation", "distinct_causes"), "3");
    let causes = field(&o, "invalidation", "causes");
    assert!(causes.contains("expired:2"), "{causes}");
    assert!(causes.contains("no_marker:1"), "{causes}");
    assert!(causes.contains("history_rewritten:1"), "{causes}");

    // Per-cause lines exist too, so a shell can loop rather than parse a CSV.
    let per_cause: Vec<&str> = o
        .lines()
        .filter(|l| l.starts_with("F23_CACHE=invalidation_cause "))
        .collect();
    assert_eq!(per_cause.len(), 3, "{o}");

    // Known-negative: a session with no misses must report no causes, so the
    // assertions above cannot be passing on a hardcoded list.
    let tmp2 = tempfile::tempdir().unwrap();
    write_ledger(
        tmp2.path(),
        "sess-clean",
        vec![turn_json(
            1, 100, 9_000, 0, 0.01, 0.05, "catalog", None, 9_100,
        )],
        vec![],
    );
    let clean = stdout(&run(tmp2.path(), &["report"]));
    assert_eq!(field(&clean, "invalidation", "distinct_causes"), "0");
    assert_eq!(field(&clean, "invalidation", "causes"), "-");
}

// ── Token pressure ──────────────────────────────────────────────────────────

#[test]
fn report_exposes_token_pressure_against_the_real_thresholds() {
    let tmp = tempfile::tempdir().unwrap();
    write_ledger(
        tmp.path(),
        "sess-pressure",
        vec![
            turn_json(
                1,
                10_000,
                0,
                0,
                0.01,
                0.01,
                "catalog",
                Some("no_marker"),
                10_000,
            ),
            turn_json(2, 10_000, 0, 0, 0.01, 0.01, "catalog", None, 120_000),
        ],
        vec![
            serde_json::json!({
                "after_round_trip": 2,
                "ts": "2026-07-29T10:03:00.000Z",
                "kind": "auto",
                "trigger": "watermark",
                "watermark_tokens": 120_000u64,
                "threshold_tokens": 150_000u64,
                "pre_tokens": 120_000u64,
                "tokens_freed": 90_000u64,
                "items_collapsed": 40u64,
            }),
            serde_json::json!({
                "after_round_trip": 2,
                "ts": "2026-07-29T10:04:00.000Z",
                "kind": "auto_failed",
                "trigger": "watermark",
                "watermark_tokens": 140_000u64,
                "threshold_tokens": 150_000u64,
                "pre_tokens": 140_000u64,
                "tokens_freed": 0u64,
                "items_collapsed": 0u64,
                "error": "circuit breaker tripped after 3 consecutive failures",
            }),
        ],
    );
    let o = stdout(&run(tmp.path(), &["report"]));

    assert_eq!(field(&o, "pressure", "peak_watermark"), "120000");
    assert_eq!(field(&o, "pressure", "autocompact_threshold"), "150000");
    assert_eq!(field(&o, "pressure", "emergency_limit"), "197000");
    assert!((f64_field(&o, "pressure", "peak_pressure") - 0.8).abs() < 1e-4);
    assert_eq!(field(&o, "pressure", "compactions"), "2");
    assert_eq!(field(&o, "pressure", "auto"), "1");
    // A compaction that FAILED must be counted apart from one that worked, and
    // must not contribute reclaimed tokens.
    assert_eq!(field(&o, "pressure", "failed"), "1");
    assert_eq!(field(&o, "pressure", "tokens_reclaimed"), "90000");

    // `show` carries the failure's reason all the way out to the operator.
    let shown = stdout(&run(tmp.path(), &["show"]));
    assert!(
        shown.contains("kind=auto_failed") && shown.contains("circuit"),
        "the failed compaction's reason did not survive to `show`:\n{shown}"
    );
}

// ── Cost truth ──────────────────────────────────────────────────────────────

#[test]
fn cost_varies_with_the_session_it_reports_on() {
    // Guards the defect this criterion was warned about: a cost observable
    // that reports the same number regardless of what happened. Two ledgers
    // that differ ONLY in their token counts must produce different costs,
    // savings and ratios.
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    write_ledger(
        a.path(),
        "cheap",
        vec![turn_json(
            1,
            1_000,
            0,
            0,
            0.0150,
            0.0150,
            "catalog",
            Some("no_marker"),
            1_000,
        )],
        vec![],
    );
    write_ledger(
        b.path(),
        "dear",
        vec![turn_json(
            1,
            900_000,
            0,
            0,
            13.5000,
            13.5000,
            "catalog",
            Some("no_marker"),
            900_000,
        )],
        vec![],
    );

    let oa = stdout(&run(a.path(), &["report"]));
    let ob = stdout(&run(b.path(), &["report"]));
    let ca = f64_field(&oa, "cost", "usd");
    let cb = f64_field(&ob, "cost", "usd");
    assert!(ca > 0.0 && cb > 0.0, "a={ca} b={cb}");
    assert!(
        (cb - ca).abs() > 1.0,
        "cost is INVARIANT across two very different sessions: {ca} vs {cb}"
    );
    assert!(
        (cb / ca - 900.0).abs() < 1.0,
        "cost did not scale with the work: {ca} -> {cb}"
    );
}

#[test]
fn cache_saving_is_signed_and_a_write_heavy_session_reports_a_loss() {
    let tmp = tempfile::tempdir().unwrap();
    // One round-trip that paid the cache-write premium and never read back.
    write_ledger(
        tmp.path(),
        "sess-loss",
        vec![turn_json(
            1,
            1_000,
            0,
            40_000,
            0.0765,
            0.0615,
            "catalog",
            Some("no_marker"),
            41_000,
        )],
        vec![],
    );
    let o = stdout(&run(tmp.path(), &["report"]));
    let saving = f64_field(&o, "cost", "saving_usd");
    assert!(
        saving < 0.0,
        "a write-only session must report a NEGATIVE saving, got {saving}\n{o}"
    );
    assert!(f64_field(&o, "cost", "saving_ratio") < 0.0);

    // Known-negative: the healthy fixture must report a POSITIVE saving, so
    // the assertion above is not passing on a sign bug that always returns
    // negative.
    let tmp2 = tempfile::tempdir().unwrap();
    healthy(tmp2.path(), "sess-gain");
    let good = stdout(&run(tmp2.path(), &["report"]));
    assert!(
        f64_field(&good, "cost", "saving_usd") > 0.0,
        "the sign check is vacuous — a cache-reading session reported no gain:\n{good}"
    );
}

#[test]
fn an_unpriced_model_is_reported_as_unpriced_not_as_free() {
    let tmp = tempfile::tempdir().unwrap();
    write_ledger(
        tmp.path(),
        "sess-unpriced",
        vec![
            turn_json(
                1,
                50_000,
                0,
                0,
                0.0,
                0.0,
                "unpriced",
                Some("no_marker"),
                50_000,
            ),
            turn_json(2, 50_000, 0, 0, 0.0, 0.0, "unpriced", None, 100_000),
        ],
        vec![],
    );
    let o = stdout(&run(tmp.path(), &["report"]));
    assert_eq!(field(&o, "cost", "cost_truth"), "unpriced");
    assert_eq!(field(&o, "cost", "unpriced_round_trips"), "2");
    assert_eq!(field(&o, "cost", "catalog_priced_round_trips"), "0");
    assert!(
        o.contains("F23_CACHE=cost_warning"),
        "a $0.00 that means `we cannot price this` must carry a warning:\n{o}"
    );

    // And the machine-readable gate refuses it.
    let v = run(tmp.path(), &["verify"]);
    assert_eq!(
        code(&v),
        7,
        "verify must exit 7 on an unpriceable session; stdout:\n{}",
        stdout(&v)
    );
    assert_eq!(field(&stdout(&v), "verify", "trustworthy"), "false");
}

#[test]
fn verify_passes_only_when_every_round_trip_is_priced() {
    let tmp = tempfile::tempdir().unwrap();
    healthy(tmp.path(), "sess-ok");
    let ok = run(tmp.path(), &["verify"]);
    assert_eq!(code(&ok), 0, "stdout:\n{}", stdout(&ok));
    assert_eq!(field(&stdout(&ok), "verify", "cost_truth"), "priced");

    // One unpriced round-trip out of four is enough to fail it. This is the
    // assertion that proves the gate can fail on partially-good input, not
    // only on all-bad input.
    let mixed = tempfile::tempdir().unwrap();
    write_ledger(
        mixed.path(),
        "sess-mixed",
        vec![
            turn_json(
                1,
                1_000,
                0,
                0,
                0.01,
                0.01,
                "catalog",
                Some("no_marker"),
                1_000,
            ),
            turn_json(2, 1_000, 0, 0, 0.01, 0.01, "catalog", None, 2_000),
            turn_json(3, 1_000, 0, 0, 0.00, 0.00, "unpriced", None, 3_000),
        ],
        vec![],
    );
    let bad = run(mixed.path(), &["verify"]);
    assert_eq!(code(&bad), 7, "stdout:\n{}", stdout(&bad));
    assert_eq!(field(&stdout(&bad), "verify", "cost_truth"), "partial");
}

#[test]
fn a_family_rate_estimate_does_not_pass_verify() {
    // The finding that produced `CostSource`: `resolve_turn_cost` reports
    // `priced = true` for a model the catalog has never heard of, using the
    // provider FAMILY's rate. That number looks exactly like spend and is not.
    // It must not clear the gate, and it must be labelled distinctly from an
    // outright-unpriced session.
    let tmp = tempfile::tempdir().unwrap();
    write_ledger(
        tmp.path(),
        "sess-estimated",
        vec![
            turn_json(
                1,
                1_000,
                0,
                0,
                0.015,
                0.015,
                "provider_defaults",
                Some("no_marker"),
                1_000,
            ),
            turn_json(
                2,
                1_000,
                0,
                0,
                0.015,
                0.015,
                "provider_defaults",
                None,
                2_000,
            ),
        ],
        vec![],
    );
    let v = run(tmp.path(), &["verify"]);
    assert_eq!(code(&v), 7, "stdout:\n{}", stdout(&v));
    let o = stdout(&v);
    assert_eq!(field(&o, "verify", "cost_truth"), "estimated");
    assert_eq!(field(&o, "verify", "trustworthy"), "false");
    assert_eq!(field(&o, "verify", "estimated_round_trips"), "2");
    assert_eq!(field(&o, "verify", "unpriced_round_trips"), "0");

    // And `report` says so in words, distinctly from the unpriced warning.
    let r = stdout(&run(tmp.path(), &["report"]));
    assert_eq!(
        field(&r, "cost_warning", "text"),
        "usd_is_a_family_rate_estimate_not_spend"
    );

    // Known-negative: the fully-catalogued fixture must NOT produce a warning
    // line at all, so the assertion above is not matching a line that is
    // always emitted.
    let good = tempfile::tempdir().unwrap();
    healthy(good.path(), "sess-catalogued");
    let gr = stdout(&run(good.path(), &["report"]));
    assert!(
        !gr.contains("F23_CACHE=cost_warning"),
        "a fully catalogued session must not carry a cost warning:\n{gr}"
    );
}

#[test]
fn verify_on_an_empty_store_is_a_distinct_failure_not_a_pass() {
    // "There is nothing to check" must never read as "the check passed".
    let tmp = tempfile::tempdir().unwrap();
    let out = run(tmp.path(), &["verify"]);
    assert_eq!(code(&out), 8, "stdout:\n{}", stdout(&out));
    let o = stdout(&out);
    assert_eq!(field(&o, "verify", "trustworthy"), "false");
    assert_eq!(field(&o, "verify", "reason"), "no_ledger");
}

// ── Listing / selection / JSON ──────────────────────────────────────────────

#[test]
fn list_reports_every_session_and_report_selects_by_id() {
    let tmp = tempfile::tempdir().unwrap();
    healthy(tmp.path(), "sess-a");
    write_ledger(
        tmp.path(),
        "sess-b",
        vec![turn_json(
            1,
            7_777,
            0,
            0,
            0.1234,
            0.1234,
            "catalog",
            Some("no_marker"),
            7_777,
        )],
        vec![],
    );
    let l = stdout(&run(tmp.path(), &["list"]));
    assert_eq!(field(&l, "list", "sessions"), "2");

    // Selecting explicitly must reach THAT session, not the newest.
    let b = stdout(&run(tmp.path(), &["report", "--session", "sess-b"]));
    assert_eq!(field(&b, "session", "id"), "sess-b");
    assert_eq!(field(&b, "quality", "uncached_input"), "7777");

    // Known-negative: an id that does not exist must fail, so the selection
    // above is not silently falling back to `latest`.
    let missing = run(tmp.path(), &["report", "--session", "no-such-session"]);
    assert_ne!(
        code(&missing),
        0,
        "an unknown --session exited 0; selection may be falling through to latest"
    );
}

#[test]
fn json_output_carries_the_derived_figures_not_just_the_raw_counters() {
    let tmp = tempfile::tempdir().unwrap();
    healthy(tmp.path(), "sess-json");
    let out = run(tmp.path(), &["report", "--json"]);
    assert_eq!(code(&out), 0);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("report --json is JSON");
    for key in [
        "hit_ratio",
        "warm_hit_ratio",
        "cache_saving_usd",
        "cache_saving_ratio",
        "peak_pressure_ratio",
        "cost_truth",
        "cost_trustworthy",
        "total_input_tokens",
    ] {
        assert!(
            v.get(key).is_some(),
            "`{key}` missing from report --json:\n{v}"
        );
    }
    assert_eq!(v["cost_truth"], "priced");
    assert_eq!(v["priced_round_trips"], 4);
    assert_eq!(v["cost_trustworthy"], true);
}

#[test]
fn a_malformed_ledger_does_not_hide_the_good_ones() {
    let tmp = tempfile::tempdir().unwrap();
    healthy(tmp.path(), "sess-good");
    std::fs::write(tmp.path().join("broken.json"), b"{not json").unwrap();
    // And one with a schema this build does not understand.
    std::fs::write(
        tmp.path().join("future.json"),
        br#"{"schema":999,"session_id":"future","started_at":"x","updated_at":"x","session_complete":true,"turns":[],"compactions":[]}"#,
    )
    .unwrap();

    let l = stdout(&run(tmp.path(), &["list"]));
    assert_eq!(
        field(&l, "list", "sessions"),
        "1",
        "list should skip the two unreadable files and still show the good one:\n{l}"
    );
    assert_eq!(code(&run(tmp.path(), &["verify"])), 0);
}

// ── #1162: the session id the user set must resolve ─────────────────────────

/// Write a persisted session snapshot that points at a ledger id.
///
/// Only the fields `SessionManager::load` needs; the rest carry `serde(default)`.
fn write_session(session_dir: &Path, id: &str, conversation_id: Option<&str>) {
    std::fs::create_dir_all(session_dir).unwrap();
    let mut session = serde_json::json!({
        "schema_version": 1,
        "id": id,
        "created_at": "2026-07-29T10:00:00Z",
        "updated_at": "2026-07-29T10:05:00Z",
        "provider": "anthropic",
        "model": "claude-opus-4-7",
        "cwd": "/tmp",
        "messages": [],
    });
    if let Some(conv) = conversation_id {
        session["conversation_id"] = serde_json::json!(conv);
    }
    std::fs::write(
        session_dir.join(format!("{id}.json")),
        serde_json::to_vec_pretty(&session).unwrap(),
    )
    .unwrap();
}

/// #1162 — `cache report --session <the id the user set>` must find the record.
///
/// The ledger is keyed by the engine-internal `conversation_id`; the flag says
/// "Session id to report on", which is the id the user controls. Nothing
/// bridged the two, so the only reachable outcome was a bare io error naming a
/// path that never existed.
///
/// HOW THIS FAILS IF THE DEFECT RETURNS: drop the session-store fallback in
/// `cache_cmd::resolve` — the run exits non-zero with
/// `ledger io error at .../aa55aa55-0002.json`.
#[test]
fn a_user_chosen_session_id_resolves_to_the_ledger_keyed_by_its_conversation_id() {
    let tmp = tempfile::tempdir().unwrap();
    let ledgers = tmp.path().join("cache-ledger");
    let sessions = tmp.path().join("sessions");
    healthy(&ledgers, "2fc54759-conv-uuid");
    write_session(&sessions, "aa55aa55-0002", Some("2fc54759-conv-uuid"));

    let out = Command::new(BIN)
        .args(["cache", "report", "--session", "aa55aa55-0002"])
        .arg("--dir")
        .arg(&ledgers)
        .arg("--session-dir")
        .arg(&sessions)
        .env("WAYLAND_HOME", tmp.path())
        .output()
        .expect("failed to run wayland-core cache");
    assert_eq!(
        code(&out),
        0,
        "stdout:\n{}\nstderr:\n{}",
        stdout(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    let report = stdout(&out);
    assert_eq!(
        field(&report, "session", "id"),
        "2fc54759-conv-uuid",
        "the report must name the real ledger key it resolved to:\n{report}"
    );
    assert_eq!(field(&report, "session", "round_trips"), "4");
}

/// The known-negative: an id that is neither a ledger nor a session must still
/// fail, and the failure must name the real key and point somewhere useful.
/// Without this, a fallback that silently returned the newest ledger would
/// satisfy the test above.
#[test]
fn an_unknown_session_id_still_fails_and_says_where_to_look() {
    let tmp = tempfile::tempdir().unwrap();
    let ledgers = tmp.path().join("cache-ledger");
    let sessions = tmp.path().join("sessions");
    healthy(&ledgers, "2fc54759-conv-uuid");
    std::fs::create_dir_all(&sessions).unwrap();

    let out = Command::new(BIN)
        .args(["cache", "report", "--session", "no-such-id"])
        .arg("--dir")
        .arg(&ledgers)
        .arg("--session-dir")
        .arg(&sessions)
        .env("WAYLAND_HOME", tmp.path())
        .output()
        .expect("failed to run wayland-core cache");
    assert_ne!(code(&out), 0, "an unknown id must not succeed");
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        err.contains("cache list"),
        "the error must point at the verb that lists the real keys:\n{err}"
    );
}

// ── #1163: an unpriceable counterfactual must render as unknown ─────────────

/// #1163 — a counterfactual the catalog cannot price must render as `unknown`,
/// never as a saving subtracted against a fabricated zero.
///
/// The fixture writes `uncached_equivalent_usd: null` — the honest encoding of
/// "no rate exists for this model". Before the fix the field was a bare `f64`,
/// so this ledger did not decode at all: the product had no way to SAY
/// unknown, which is why it said `0.000000` and reported `saving_usd=-cost`.
///
/// The assertion is on the RENDERED verdict, not on the stored number. A test
/// asserting `uncached_equivalent_usd == 0.0` would restate the defaulted
/// literal and pass on the broken build.
#[test]
fn an_unpriced_counterfactual_renders_unknown_instead_of_a_negative_saving() {
    let tmp = tempfile::tempdir().unwrap();
    let mut turn = turn_json(
        1,
        7_000,
        0,
        0,
        0.061389,
        0.0,
        "provider_reported",
        None,
        7_000,
    );
    turn["uncached_equivalent_usd"] = serde_json::Value::Null;
    write_ledger(tmp.path(), "flux-sess", vec![turn], vec![]);

    let out = run(tmp.path(), &["report"]);
    assert_eq!(
        code(&out),
        0,
        "stdout:\n{}\nstderr:\n{}",
        stdout(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    let report = stdout(&out);
    assert_eq!(
        field(&report, "cost", "uncached_equivalent_usd"),
        "unknown",
        "a counterfactual with no catalog rate is unknown, not zero:\n{report}"
    );
    assert_eq!(
        field(&report, "cost", "saving_usd"),
        "unknown",
        "there is nothing to subtract from, so there is no saving to report:\n{report}"
    );
    assert_eq!(field(&report, "cost", "saving_ratio"), "unknown");
    // The billed figure is still spend — the provider reported it — so the
    // cost grade must NOT be dragged down. Only the saving is unknown.
    assert_eq!(field(&report, "cost", "cost_truth"), "priced");
    assert_eq!(field(&report, "cost", "saving_truth"), "unpriced");
    assert!(
        !report.contains("saving_usd=-"),
        "the report must never render a negative saving against a zero it \
         invented:\n{report}"
    );
}

/// wayland#1205 — reading back a ledger an OLDER build wrote must not
/// reproduce #1163 on the fixed build.
///
/// `uncached_equivalent_usd` changed from `f64` to `Option<f64>` in place, and
/// `#[serde(default)]` cannot tell a v1 `0.0` (which meant "nothing could
/// price this") from a v2 `Some(0.0)` (a genuine priced zero). Only the schema
/// version can, which is why it was bumped. Measured through the BINARY, on
/// all three verbs, because the operator in #1163 filed it off a `cache report`
/// over a ledger directory they already had — a library-level assertion would
/// not have been that surface.
#[test]
fn a_v0_13_9_ledger_does_not_render_a_negative_saving_or_certify_it() {
    let tmp = tempfile::tempdir().unwrap();
    write_v1_ledger(
        tmp.path(),
        "legacy-sess",
        vec![turn_json(
            1,
            6_620,
            7_232,
            0,
            0.061389,
            0.0,
            "provider_reported",
            None,
            14_752,
        )],
    );

    let report = stdout(&run(tmp.path(), &["report"]));
    assert_eq!(
        field(&report, "cost", "uncached_equivalent_usd"),
        "unknown",
        "a v1 zero meant `nothing could price this`; rendering it as a priced \
         zero is #1163 coming back on the fixed build:\n{report}"
    );
    assert_eq!(field(&report, "cost", "saving_usd"), "unknown");
    assert_ne!(
        field(&report, "cost", "saving_truth"),
        "priced",
        "grading the fabricated saving `priced` is a confidence the pre-fix \
         build never even claimed:\n{report}"
    );
    assert!(
        !report.contains("saving_usd=-"),
        "the ticket`s own report line is `saving_usd=-0.061389`; it must not \
         come back:\n{report}"
    );

    // The store total sums the same field, so one legacy session poisons
    // `cache list` for the whole directory if the migration is skipped.
    let list = stdout(&run(tmp.path(), &["list"]));
    assert_eq!(
        field(&list, "total", "uncached_equivalent_usd"),
        "unknown",
        "one legacy session must not put a fabricated zero into the store \
         total:\n{list}"
    );

    // `verify` may still call the SPEND trustworthy — the provider reported it
    // — but it must not call the SAVING priced.
    let verified = run(tmp.path(), &["verify"]);
    let v = stdout(&verified);
    assert_ne!(
        field(&v, "verify", "saving_truth"),
        "priced",
        "verify is the certification surface; certifying a saving computed \
         against a baseline nobody wrote is the worst place to say it:\n{v}"
    );
    assert_eq!(field(&v, "verify", "cost_truth"), "priced");
}

/// Known-negative for the test above: a REAL negative saving — a session that
/// writes cache and never reads it back — must still print as a negative
/// number. Rendering every saving as `unknown` would satisfy the assertions
/// above and destroy the signal the ledger exists to carry.
#[test]
fn a_genuinely_negative_saving_is_still_reported_as_a_negative_number() {
    let tmp = tempfile::tempdir().unwrap();
    write_ledger(
        tmp.path(),
        "write-heavy",
        vec![turn_json(
            1, 1_000, 0, 100_000, 0.4650, 0.3030, "catalog", None, 101_000,
        )],
        vec![],
    );

    let report = stdout(&run(tmp.path(), &["report"]));
    let saving = f64_field(&report, "cost", "saving_usd");
    assert!(
        saving < 0.0,
        "a write-dominated session really does cost more than an uncached one; \
         that must stay reportable: {saving}\n{report}"
    );
    assert_eq!(field(&report, "cost", "saving_truth"), "priced");
}
