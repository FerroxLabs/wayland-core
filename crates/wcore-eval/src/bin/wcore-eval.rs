//! `wcore-eval` — CLI front-end to the W10A eval harness.
//!
//! Subcommands:
//!   score             — run the harness; print one JSON line per case to stdout.
//!   gate              — run the harness; exit 0 iff P >= 0.80 AND R >= 0.80,
//!                       else 1.
//!   gate --json       — same gate, but also print a JSON summary
//!                       (`{ "precision": ..., "recall": ..., ... }`) to stdout
//!                       and write it to `target/eval/agreement.json`.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use wcore_eval::{EvalReport, Harness};

const P_MIN: f64 = 0.80;
const R_MIN: f64 = 0.80;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let sub = args.get(1).map(String::as_str).unwrap_or("");
    let json_flag = args.iter().any(|a| a == "--json");
    match sub {
        "score" => run_score(),
        "gate" => run_gate(json_flag),
        _ => {
            eprintln!("usage: wcore-eval <score|gate> [--json]");
            ExitCode::from(2)
        }
    }
}

fn load() -> Result<EvalReport, String> {
    let h = Harness::from_manifest_dir().map_err(|e| format!("harness load: {e}"))?;
    h.run().map_err(|e| format!("harness run: {e}"))
}

fn run_score() -> ExitCode {
    let report = match load() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    for case in &report.by_case {
        match serde_json::to_string(case) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("json serialization failed for {}: {e}", case.case_id);
                return ExitCode::from(1);
            }
        }
    }
    ExitCode::SUCCESS
}

fn run_gate(json: bool) -> ExitCode {
    let report = match load() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    eprintln!(
        "W10A gate: precision={:.3} (>={:.2}), recall={:.3} (>={:.2})",
        report.precision, P_MIN, report.recall, R_MIN
    );

    if json {
        let summary = serde_json::json!({
            "precision": report.precision,
            "recall":    report.recall,
            "f1":        report.f1,
            "agreement_rate": report.agreement_rate,
            "tp": report.true_positive,
            "tn": report.true_negative,
            "fp": report.false_positive,
            "fn": report.false_negative,
            "total": report.total,
        });
        let serialized = match serde_json::to_string(&summary) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("json summary failed: {e}");
                return ExitCode::from(1);
            }
        };
        println!("{serialized}");
        // Best-effort sidecar write for W10B Pre-2 consumption.
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("eval")
            .join("agreement.json");
        if let Some(parent) = out.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&out, &serialized);
    }

    if report.meets_threshold(P_MIN, R_MIN) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
