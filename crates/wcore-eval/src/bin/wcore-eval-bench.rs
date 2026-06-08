//! `wcore-eval-bench` — M4.2 per-PR regression gate for the 30-case
//! mini-bench corpus.
//!
//! The binary loads the frozen [`BenchCorpus`], runs every case through
//! the deterministic [`CannedBenchRunner`] (no LLM keys), and reports a
//! JSON summary plus per-category breakdown to stdout. Exit code is 0
//! iff `pass_ratio >= floor`, 1 otherwise — that's what the GitHub
//! Actions workflow keys off.
//!
//! ## CLI
//!
//! ```text
//! wcore-eval-bench [--floor <0.0..=1.0>] [--report-json <path>]
//! ```
//!
//! Defaults: `--floor 0.7`. With the canned runner every case passes,
//! so a green run reports `pass_ratio = 1.0`. The floor exists so the
//! gate stays meaningful after M4.3 wires in a stochastic
//! `wcore-agent::Session`-backed runner.
//!
//! ## Why not clap
//!
//! The sibling `wcore-eval` binary uses `std::env::args` for the same
//! reason: this binary's surface is tiny (two flags) and stays
//! dependency-free so cold-build time in CI is dominated by the
//! workspace rebuild, not arg-parser codegen.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use wcore_eval::bench::{BenchCategory, BenchCorpus, CannedBenchRunner};

/// Default regression floor. Bench runs whose `pass_ratio` falls below
/// this value fail the gate. Chosen to be loose enough that the
/// deterministic canned runner clears it by a wide margin (1.0 vs
/// 0.7), while leaving headroom for the M4.3 stochastic runner.
const DEFAULT_FLOOR: f64 = 0.7;

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().collect();
    let args = match Args::parse(&argv) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("error: {msg}");
            eprintln!("usage: wcore-eval-bench [--floor <0.0..=1.0>] [--report-json <path>]");
            return ExitCode::from(2);
        }
    };

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let corpus = match BenchCorpus::load(&root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: bench corpus load failed: {e}");
            return ExitCode::from(1);
        }
    };

    let runner = Arc::new(CannedBenchRunner::new());
    let scorer = wcore_eval::BenchScorer::new(corpus, runner);
    let outcomes = scorer.outcomes();

    let report = Report::build(&outcomes, args.floor);

    // Always print JSON to stdout so a CI step can `tee` and feed it
    // into a comment-on-PR step without parsing logs.
    let json = match serde_json::to_string_pretty(&report) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: report serialization failed: {e}");
            return ExitCode::from(1);
        }
    };
    println!("{json}");

    if let Some(path) = args.report_json.as_deref()
        && let Err(e) = write_report(path, &json)
    {
        eprintln!("error: failed to write {}: {e}", path.display());
        return ExitCode::from(1);
    }

    // Human-readable summary to stderr — keeps stdout machine-parseable
    // while CI logs stay scannable at a glance.
    eprintln!(
        "bench: {}/{} passed (ratio {:.4}, floor {:.4}) → {}",
        report.passed,
        report.total,
        report.pass_ratio,
        report.floor,
        if report.regressed { "REGRESSION" } else { "OK" }
    );
    for cat in &report.by_category {
        eprintln!(
            "  {:>12}: {}/{} ({:.4})",
            cat.category, cat.passed, cat.total, cat.pass_ratio
        );
    }

    if report.regressed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Parsed CLI arguments.
struct Args {
    floor: f64,
    report_json: Option<PathBuf>,
}

impl Args {
    fn parse(argv: &[String]) -> Result<Self, String> {
        let mut floor = DEFAULT_FLOOR;
        let mut report_json: Option<PathBuf> = None;

        let mut i = 1; // skip program name
        while i < argv.len() {
            match argv[i].as_str() {
                "--floor" => {
                    let v = argv
                        .get(i + 1)
                        .ok_or_else(|| "--floor requires a value".to_string())?;
                    let parsed: f64 = v
                        .parse()
                        .map_err(|e| format!("--floor: not a number: {e}"))?;
                    if !(0.0..=1.0).contains(&parsed) || parsed.is_nan() {
                        return Err(format!("--floor must be in 0.0..=1.0, got {parsed}"));
                    }
                    floor = parsed;
                    i += 2;
                }
                "--report-json" => {
                    let v = argv
                        .get(i + 1)
                        .ok_or_else(|| "--report-json requires a path".to_string())?;
                    report_json = Some(PathBuf::from(v));
                    i += 2;
                }
                "-h" | "--help" => {
                    return Err("help requested".to_string());
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        Ok(Self { floor, report_json })
    }
}

/// JSON report shape consumed by CI (and humans scanning artifacts).
///
/// Stable across M4.2 → M4.3. New fields go at the end; renames require
/// a coordinated bump in the workflow consumer.
#[derive(Debug, serde::Serialize)]
struct Report {
    /// Total number of cases run.
    total: usize,
    /// How many passed.
    passed: usize,
    /// `passed / total`.
    pass_ratio: f64,
    /// The `--floor` the gate compared against.
    floor: f64,
    /// True iff `pass_ratio < floor`. The CI step keys off this and
    /// off the exit code (which mirrors it).
    regressed: bool,
    /// Per-category breakdown, in a stable order.
    by_category: Vec<CategoryRow>,
    /// Per-case failure reasons. Empty when all cases pass.
    failures: Vec<FailureRow>,
}

#[derive(Debug, serde::Serialize)]
struct CategoryRow {
    /// snake_case, matches the YAML `category:` field.
    category: &'static str,
    total: usize,
    passed: usize,
    pass_ratio: f64,
}

#[derive(Debug, serde::Serialize)]
struct FailureRow {
    case_id: String,
    category: &'static str,
    reason: String,
}

impl Report {
    fn build(outcomes: &[wcore_eval::BenchOutcome], floor: f64) -> Self {
        let total = outcomes.len();
        let passed = outcomes.iter().filter(|o| o.passed).count();
        let pass_ratio = if total == 0 {
            0.0
        } else {
            passed as f64 / total as f64
        };
        let regressed = pass_ratio < floor;

        // Categories in a stable presentation order: routing → arith →
        // recall → fileops. Matches the table in `bench.rs` doc-header.
        let order = [
            BenchCategory::ToolRouting,
            BenchCategory::Arithmetic,
            BenchCategory::Recall,
            BenchCategory::FileOps,
        ];
        let by_category = order
            .iter()
            .map(|c| {
                let total = outcomes.iter().filter(|o| o.category == *c).count();
                let passed = outcomes
                    .iter()
                    .filter(|o| o.category == *c && o.passed)
                    .count();
                let ratio = if total == 0 {
                    0.0
                } else {
                    passed as f64 / total as f64
                };
                CategoryRow {
                    category: category_name(*c),
                    total,
                    passed,
                    pass_ratio: ratio,
                }
            })
            .collect();

        let failures = outcomes
            .iter()
            .filter(|o| !o.passed)
            .map(|o| FailureRow {
                case_id: o.case_id.clone(),
                category: category_name(o.category),
                reason: o.reason.clone(),
            })
            .collect();

        Self {
            total,
            passed,
            pass_ratio,
            floor,
            regressed,
            by_category,
            failures,
        }
    }
}

fn category_name(c: BenchCategory) -> &'static str {
    match c {
        BenchCategory::ToolRouting => "tool_routing",
        BenchCategory::Arithmetic => "arithmetic",
        BenchCategory::Recall => "recall",
        BenchCategory::FileOps => "file_ops",
    }
}

fn write_report(path: &Path, json: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("wcore-eval-bench".to_string())
            .chain(args.iter().map(|s| s.to_string()))
            .collect()
    }

    #[test]
    fn args_default_floor() {
        let a = Args::parse(&argv(&[])).unwrap();
        assert_eq!(a.floor, DEFAULT_FLOOR);
        assert!(a.report_json.is_none());
    }

    #[test]
    fn args_parse_floor_and_path() {
        let a = Args::parse(&argv(&["--floor", "0.85", "--report-json", "out.json"])).unwrap();
        assert!((a.floor - 0.85).abs() < 1e-12);
        assert_eq!(a.report_json.as_deref(), Some(Path::new("out.json")));
    }

    #[test]
    fn args_rejects_floor_out_of_range() {
        assert!(Args::parse(&argv(&["--floor", "1.5"])).is_err());
        assert!(Args::parse(&argv(&["--floor", "-0.1"])).is_err());
        assert!(Args::parse(&argv(&["--floor", "nope"])).is_err());
    }

    #[test]
    fn args_rejects_unknown_flag() {
        assert!(Args::parse(&argv(&["--bogus"])).is_err());
    }

    #[test]
    fn args_floor_requires_value() {
        assert!(Args::parse(&argv(&["--floor"])).is_err());
    }
}
