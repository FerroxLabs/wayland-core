//! Redacted console and machine-readable report projections.

use serde_json::{Value, json};
use thiserror::Error;

use crate::receipt::{CellResultV1, EvidenceReceiptV1};
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

/// All human and machine projections from one canonical evidence receipt.
/// Only `json` is the authoritative envelope; every other projection carries
/// its content digest so consumers can join it back to that envelope.
#[derive(Debug, Clone)]
pub struct ReceiptReports {
    pub json: String,
    pub jsonl: String,
    pub junit: String,
    pub console: String,
    pub markdown: String,
}

#[derive(Debug, Error)]
pub enum ReportRenderError {
    #[error("could not serialize {format} report: {source}")]
    Serialize {
        format: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("secret material detected in rendered {0} report")]
    SecretDetected(&'static str),
}

pub fn render_receipt_reports(
    receipt: &EvidenceReceiptV1,
    forbidden_secrets: &[String],
) -> Result<ReceiptReports, ReportRenderError> {
    let reports = ReceiptReports {
        json: serde_json::to_string_pretty(receipt).map_err(|source| {
            ReportRenderError::Serialize {
                format: "JSON",
                source,
            }
        })?,
        jsonl: render_receipt_jsonl(receipt)?,
        junit: render_receipt_junit(receipt),
        console: render_receipt_console(receipt),
        markdown: render_receipt_markdown(receipt),
    };
    for (format, output) in [
        ("JSON", &reports.json),
        ("JSONL", &reports.jsonl),
        ("JUnit", &reports.junit),
        ("console", &reports.console),
        ("Markdown", &reports.markdown),
    ] {
        if forbidden_secrets
            .iter()
            .any(|secret| !secret.is_empty() && output.contains(secret))
        {
            return Err(ReportRenderError::SecretDetected(format));
        }
    }
    Ok(reports)
}

fn render_receipt_jsonl(receipt: &EvidenceReceiptV1) -> Result<String, ReportRenderError> {
    let mut lines = Vec::with_capacity(receipt.body.results.len() + 2);
    lines.push(json!({
        "type": "receipt_header",
        "schema": receipt.schema,
        "schema_version": receipt.schema_version,
        "body_sha256": receipt.body_sha256,
        "run_id": receipt.body.run_id,
    }));
    for result in &receipt.body.results {
        lines.push(json!({
            "type": "cell_result",
            "body_sha256": receipt.body_sha256,
            "result": result,
        }));
    }
    lines.push(json!({
        "type": "receipt_trailer",
        "body_sha256": receipt.body_sha256,
        "summary": receipt.body.summary,
    }));
    let encoded = lines
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| ReportRenderError::Serialize {
            format: "JSONL",
            source,
        })?;
    Ok(format!("{}\n", encoded.join("\n")))
}

fn render_receipt_console(receipt: &EvidenceReceiptV1) -> String {
    let mut lines = vec![format!(
        "RECEIPT {} authority={}",
        receipt.body_sha256,
        authority_label(receipt)
    )];
    for result in &receipt.body.results {
        lines.push(format_cell(result));
    }
    lines.push(format!(
        "SUMMARY pass={} fail={} cost_microusd={} wall_time_ms={}",
        receipt.body.summary.passed,
        receipt.body.summary.failed,
        receipt.body.summary.total_cost_microusd,
        receipt.body.summary.wall_time_ms
    ));
    lines.join("\n")
}

fn render_receipt_markdown(receipt: &EvidenceReceiptV1) -> String {
    let mut output = format!(
        "# Wayland evaluation receipt\n\n- Receipt: `{}`\n- Authority claim: `{}`\n\n| Cell | Task | Provider | Platform | Verdict | Failures |\n|---|---|---|---|---|---|\n",
        receipt.body_sha256,
        authority_label(receipt)
    );
    for result in &receipt.body.results {
        let failures = result
            .failures
            .iter()
            .map(|failure| failure.code.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            escape_markdown(&result.cell_id),
            escape_markdown(&result.task),
            escape_markdown(&result.provider),
            escape_markdown(&result.platform),
            if result.passed { "pass" } else { "fail" },
            escape_markdown(&failures),
        ));
    }
    output
}

fn render_receipt_junit(receipt: &EvidenceReceiptV1) -> String {
    let mut output = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"wayland-eval\" tests=\"{}\" failures=\"{}\" time=\"{:.3}\" receipt_sha256=\"{}\">\n",
        receipt.body.results.len(),
        receipt.body.summary.failed,
        receipt.body.summary.wall_time_ms as f64 / 1000.0,
        escape_xml(&receipt.body_sha256),
    );
    for result in &receipt.body.results {
        output.push_str(&format!(
            "  <testcase name=\"{}\" classname=\"{}.{}\" time=\"{:.3}\">",
            escape_xml(&result.task),
            escape_xml(&result.provider),
            escape_xml(&result.platform),
            result.wall_time_ms as f64 / 1000.0,
        ));
        if !result.passed {
            let codes = result
                .failures
                .iter()
                .map(|failure| failure.code.as_str())
                .collect::<Vec<_>>()
                .join(",");
            output.push_str(&format!(
                "<failure type=\"evaluation\" message=\"{}\" />",
                escape_xml(&codes)
            ));
        }
        output.push_str("</testcase>\n");
    }
    output.push_str("</testsuite>\n");
    output
}

fn format_cell(result: &CellResultV1) -> String {
    let failures = result
        .failures
        .iter()
        .map(|failure| failure.code.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{} {} provider={} platform={} failures={}",
        if result.passed { "PASS" } else { "FAIL" },
        result.task,
        result.provider,
        result.platform,
        failures
    )
}

fn authority_label(receipt: &EvidenceReceiptV1) -> &'static str {
    match &receipt.authority {
        crate::receipt::AuthorityClaimV1::Local => "local_non_authoritative",
        crate::receipt::AuthorityClaimV1::Ci { .. } => "ci_claim_unverified",
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
