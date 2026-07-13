//! Redacted console and machine-readable report projections.

use serde_json::{Value, json};

use crate::runner::{Failure, ScenarioResult};

/// One run's in-memory summary. Receipt construction copies only the safe,
/// stable fields required by the public report schema.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub results: Vec<ScenarioResult>,
    pub total_cost_usd: f64,
    pub wall_time_secs: f64,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, r: ScenarioResult) {
        self.total_cost_usd += r.cost_usd;
        self.wall_time_secs += r.wall_time.as_secs_f64();
        self.results.push(r);
    }

    pub fn passed(&self) -> usize {
        self.results.iter().filter(|r| r.passed).count()
    }
    pub fn failed(&self) -> usize {
        self.results.iter().filter(|r| !r.passed).count()
    }
}

pub fn render_console(report: &Report) -> String {
    let mut lines = Vec::new();
    for result in &report.results {
        let verdict = if result_passed(result) {
            "PASS"
        } else {
            "FAIL"
        };
        let failures = failure_codes(result).join(",");
        lines.push(format!(
            "{verdict} {} {} failures={failures}",
            result.name, result.provider
        ));
    }
    lines.push(format!(
        "SUMMARY pass={} fail={} cost_usd={:.6} wall_time_ms={}",
        report.results.iter().filter(|r| result_passed(r)).count(),
        report.results.iter().filter(|r| !result_passed(r)).count(),
        report.total_cost_usd,
        (report.wall_time_secs * 1000.0).round() as u64
    ));
    lines.join("\n")
}

pub fn render_markdown(report: &Report) -> String {
    let mut output = String::from(
        "# Wayland evaluation report\n\n| Task | Provider | Verdict | Failures |\n|---|---|---|---|\n",
    );
    for result in &report.results {
        let verdict = if result_passed(result) {
            "pass"
        } else {
            "fail"
        };
        output.push_str(&format!(
            "| {} | {} | {verdict} | {} |\n",
            escape_markdown(&result.name),
            result.provider,
            failure_codes(result).join(", ")
        ));
    }
    output
}

pub fn render_json(report: &Report) -> Value {
    let results = report
        .results
        .iter()
        .map(|result| {
            json!({
                "task": result.name,
                "provider": result.provider.cli_name(),
                "platform": result.platform,
                "approval": result.approval,
                "verdict": if result_passed(result) { "pass" } else { "fail" },
                "failure_codes": failure_codes(result),
                "wall_time_ms": result.wall_time.as_millis(),
                "boot_time_ms": result.boot_time.as_millis(),
                "cost_usd": result.cost_usd,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema": "wayland.eval.report",
        "schema_version": 1,
        "summary": {
            "passed": report.results.iter().filter(|r| result_passed(r)).count(),
            "failed": report.results.iter().filter(|r| !result_passed(r)).count(),
            "total_cost_usd": report.total_cost_usd,
            "wall_time_ms": (report.wall_time_secs * 1000.0).round() as u64,
        },
        "results": results,
    })
}

fn result_passed(result: &ScenarioResult) -> bool {
    result.passed && result.failures.is_empty()
}

fn failure_codes(result: &ScenarioResult) -> Vec<&'static str> {
    result.failures.iter().map(failure_code).collect()
}

fn failure_code(failure: &Failure) -> &'static str {
    match failure {
        Failure::OverTime { .. } => "over_time",
        Failure::OverCost { .. } => "over_cost",
        Failure::CostMissing => "cost_missing",
        Failure::Crashed { .. } => "crashed",
        Failure::Hung { .. } => "hung",
        Failure::ExpectedToolMissing(_) => "expected_tool_missing",
        Failure::ForbiddenToolUsed(_) => "forbidden_tool_used",
        Failure::AssertionFailed { .. } => "assertion_failed",
        Failure::TraceFailed { .. } => "trace_failed",
        Failure::StepsExceeded { .. } => "steps_exceeded",
        Failure::SessionBrick { .. } => "session_brick",
        Failure::SkippedInStrict { .. } => "skipped_in_strict",
        Failure::RunnerError(_) => "runner_error",
        Failure::SecretDetected { .. } => "secret_detected",
    }
}

fn escape_markdown(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}
