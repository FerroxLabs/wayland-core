//! Usability / Krug scanner — strategy doc D10.
//!
//! Scans a completed [`ScenarioResult`] for usability + performance findings
//! that a functional PASS/FAIL will never catch: optional-feature *nagging*
//! (the "honcho rule" — an unconfigured feature must degrade silently, never
//! warn), panics, broken-subsystem errors (`no such table: kg_nodes`), auth
//! noise (401s), cold-boot/turn latency over budget, and unrecovered tool
//! errors.
//!
//! Findings are ADVISORY — they form a punch list to fix afterward, they do
//! NOT flip a scenario's PASS/FAIL (which stays about functional assertions).
//! This is the engine behind the "Degradation-QA" specialist and the Krug
//! usability axis.

use std::time::Duration;

use crate::runner::ScenarioResult;

/// Cold-boot latency budget. Today boot is ~3s (MCP connect attempts + plugin
/// load); the usability target is well under this. Tunable as the engine speeds up.
const BOOT_BUDGET: Duration = Duration::from_secs(2);

/// A single turn taking longer than this is flagged as a possible stall (the
/// model itself rarely needs this long for the persona prompts; a 60s wall was
/// the DeepSeek burst-drop signature).
const TURN_STALL_BUDGET: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
}

impl Severity {
    fn tag(self) -> &'static str {
        match self {
            Severity::High => "HIGH",
            Severity::Medium => "MED",
            Severity::Low => "LOW",
        }
    }
}

/// One usability/perf observation about a scenario run.
#[derive(Debug, Clone)]
pub struct UsabilityFinding {
    pub severity: Severity,
    /// Stable kebab category, e.g. `nag-optional-feature`, `broken-subsystem`,
    /// `panic`, `latency-boot`, `latency-turn`, `tool-error`, `auth-noise`,
    /// `shutdown-stall`.
    pub category: &'static str,
    pub scenario: String,
    /// Cleaned (ANSI-stripped, timestamp-trimmed) evidence line.
    pub evidence: String,
}

/// Scan one scenario result for usability/perf findings.
pub fn scan(result: &ScenarioResult) -> Vec<UsabilityFinding> {
    let mut out = Vec::new();
    let name = result.name.clone();
    let push = |out: &mut Vec<UsabilityFinding>, severity, category, evidence: String| {
        out.push(UsabilityFinding {
            severity,
            category,
            scenario: name.clone(),
            evidence,
        });
    };

    // 1. Cold-boot latency.
    if result.boot_time > BOOT_BUDGET {
        push(
            &mut out,
            Severity::Medium,
            "latency-boot",
            format!(
                "cold boot {:.1}s exceeds {:.0}s budget",
                result.boot_time.as_secs_f64(),
                BOOT_BUDGET.as_secs_f64()
            ),
        );
    }

    // 2. Per-turn stalls.
    for t in &result.turn_results {
        if t.wall_time > TURN_STALL_BUDGET {
            push(
                &mut out,
                Severity::Medium,
                "latency-turn",
                format!(
                    "turn {} took {:.1}s (possible stall / over budget)",
                    t.turn,
                    t.wall_time.as_secs_f64()
                ),
            );
        }
    }

    // 3. Tool errors — the agent hit a tool failure during the journey.
    for e in &result.trace.entries {
        if e.is_error {
            push(
                &mut out,
                Severity::Medium,
                "tool-error",
                format!(
                    "tool '{}' returned an error in turn {}",
                    e.tool_name, e.turn
                ),
            );
        }
    }

    // 4. stderr line scan — nag / panic / broken-subsystem / auth / shutdown.
    for raw in result.stderr_tail.lines() {
        let line = clean_line(raw);
        if line.is_empty() {
            continue;
        }
        let l = line.to_lowercase();
        if l.contains("panic") {
            push(&mut out, Severity::High, "panic", line);
        } else if l.contains("no such table")
            || (l.contains("failed")
                && (l.contains("infer_kg")
                    || l.contains("kg fact")
                    || l.contains("ingest")
                    || l.contains("dream cycle")))
        {
            push(&mut out, Severity::High, "broken-subsystem", line);
        } else if (l.contains("missing") && l.contains("api_key")) || l.contains("for live mode") {
            // The honcho rule: an unconfigured optional feature must NOT nag.
            push(&mut out, Severity::Medium, "nag-optional-feature", line);
        } else if l.contains("unauthorized") || l.contains("status: 401") || l.contains(" 401 ") {
            push(&mut out, Severity::Low, "auth-noise", line);
        } else if l.contains("did not finish within") {
            push(&mut out, Severity::Low, "shutdown-stall", line);
        }
    }

    out
}

/// Aggregate findings across ALL scenarios into a deduped markdown punch list.
/// The same boot noise (e.g. honcho) appears in every scenario's stderr, so we
/// group by `(category, normalized evidence)` and list how many scenarios /
/// which ones hit it — turning N copies of one bug into a single actionable row.
pub fn render_punch_list(findings: &[UsabilityFinding]) -> String {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    if findings.is_empty() {
        return "## Usability Punch List\n\n(no usability findings)\n".to_string();
    }

    // key: (severity, category, normalized-evidence) -> set of scenarios
    let mut groups: BTreeMap<(Severity, &'static str, String), Vec<String>> = BTreeMap::new();
    for f in findings {
        let key = (f.severity, f.category, normalize(&f.evidence));
        let entry = groups.entry(key).or_default();
        if !entry.contains(&f.scenario) {
            entry.push(f.scenario.clone());
        }
    }

    // Order: High → Medium → Low, then by category.
    let mut rows: Vec<_> = groups.into_iter().collect();
    rows.sort_by(|a, b| {
        b.0.0
            .cmp(&a.0.0) // severity desc
            .then(a.0.1.cmp(b.0.1)) // category asc
    });

    let mut out = String::new();
    let _ = writeln!(out, "## Usability Punch List\n");
    let _ = writeln!(
        out,
        "{} distinct finding(s) across {} scenario-hit(s). Advisory — these are \
         the Krug/QA punch list, separate from functional PASS/FAIL.\n",
        rows.len(),
        findings.len()
    );
    let _ = writeln!(out, "| sev | category | scenarios | evidence |");
    let _ = writeln!(out, "|-----|----------|-----------|----------|");
    for ((sev, cat, ev), scenarios) in rows {
        let scen = if scenarios.len() > 3 {
            format!("{}+{} more", scenarios[..3].join(", "), scenarios.len() - 3)
        } else {
            scenarios.join(", ")
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            sev.tag(),
            cat,
            scen,
            ev.replace('|', "\\|")
        );
    }
    out.push('\n');
    out
}

/// Strip ANSI escape sequences and the leading tracing timestamp/level so
/// evidence lines are clean and dedupe-able.
fn clean_line(raw: &str) -> String {
    let no_ansi = strip_ansi(raw);
    no_ansi.trim().to_string()
}

/// Remove `\x1b[...m` style ANSI sequences.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip until a letter (the final byte of the CSI sequence).
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Normalize an evidence line for dedupe: drop leading ISO timestamps, collapse
/// whitespace, and elide volatile temp paths/numbers so the same logical line
/// from different runs collapses to one.
fn normalize(s: &str) -> String {
    let mut t = s.to_string();
    // Drop a leading ISO-8601 timestamp if present.
    if let Some(idx) = t.find(char::is_whitespace) {
        let head = &t[..idx];
        if head.contains('T') && head.contains(':') {
            t = t[idx..].trim_start().to_string();
        }
    }
    // Elide /tmp/... and /private/... paths (per-run temp dirs).
    let mut out = String::with_capacity(t.len());
    for word in t.split_whitespace() {
        if word.contains("/tmp") || word.contains("/private/") || word.contains(".tmp") {
            out.push_str("<path>");
        } else {
            out.push_str(word);
        }
        out.push(' ');
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderId;
    use crate::runner::ScenarioResult;
    use crate::trace::ToolTrace;

    fn result_with_stderr(stderr: &str) -> ScenarioResult {
        ScenarioResult {
            name: "t".into(),
            provider: ProviderId::DeepSeek,
            passed: true,
            failures: vec![],
            wall_time: Duration::from_secs(1),
            cost_usd: 0.0,
            trace: ToolTrace::default(),
            final_text: String::new(),
            stderr_tail: stderr.into(),
            turn_results: vec![],
            workdir: std::path::PathBuf::from("/tmp/x"),
            boot_time: Duration::from_millis(500),
            info_events: vec![],
        }
    }

    #[test]
    fn flags_honcho_nag() {
        let r = result_with_stderr(
            "WARN honcho user-model reify failed; error=missing HONCHO_API_KEY for live mode",
        );
        let f = scan(&r);
        assert!(
            f.iter().any(|x| x.category == "nag-optional-feature"),
            "honcho missing-key warning must be flagged as a nag: {f:?}"
        );
    }

    #[test]
    fn flags_broken_subsystem_kg() {
        let r = result_with_stderr(
            "WARN dream cycle: infer_kg failed error=memory DB: no such table: kg_edges",
        );
        let f = scan(&r);
        assert!(
            f.iter()
                .any(|x| x.category == "broken-subsystem" && x.severity == Severity::High)
        );
    }

    #[test]
    fn flags_boot_latency_over_budget() {
        let mut r = result_with_stderr("");
        r.boot_time = Duration::from_secs(3);
        let f = scan(&r);
        assert!(f.iter().any(|x| x.category == "latency-boot"));
    }

    #[test]
    fn clean_run_has_no_findings() {
        let r = result_with_stderr("INFO web search: using DuckDuckGo (free default)");
        assert!(
            scan(&r).is_empty(),
            "a benign info line must not be flagged"
        );
    }

    #[test]
    fn dedupes_same_finding_across_scenarios() {
        let f = vec![
            UsabilityFinding {
                severity: Severity::Medium,
                category: "nag-optional-feature",
                scenario: "a".into(),
                evidence: "2026-01-01T00:00:00Z missing HONCHO_API_KEY for live mode".into(),
            },
            UsabilityFinding {
                severity: Severity::Medium,
                category: "nag-optional-feature",
                scenario: "b".into(),
                evidence: "2026-01-02T11:11:11Z missing HONCHO_API_KEY for live mode".into(),
            },
        ];
        let md = render_punch_list(&f);
        // Two scenarios, one logical finding (timestamps normalized away).
        assert!(md.contains("1 distinct finding"), "got: {md}");
        assert!(md.contains("a, b"));
    }
}
