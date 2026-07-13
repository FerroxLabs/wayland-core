use std::path::PathBuf;
use std::time::Duration;

use wcore_eval_scenarios::providers::ProviderId;
use wcore_eval_scenarios::report::{render_console, render_json, render_markdown};
use wcore_eval_scenarios::runner::{Failure, ScenarioResult};
use wcore_eval_scenarios::scenario::{ApprovalPolicy, Platform};
use wcore_eval_scenarios::{Report, ToolTrace};

const SECRET_CANARY: &str = "wcore-secret-canary-report-contract";

fn failed_report() -> Report {
    let mut report = Report::new();
    report.push(ScenarioResult {
        name: "deterministic-edit".to_string(),
        provider: ProviderId::OpenAI,
        platform: Platform::Linux,
        approval: ApprovalPolicy::ApproveAll,
        passed: false,
        failures: vec![Failure::SecretDetected {
            sink: "stderr".to_string(),
        }],
        wall_time: Duration::from_millis(1250),
        cost_usd: 0.0025,
        trace: ToolTrace::default(),
        final_text: format!("unsafe {SECRET_CANARY}"),
        stderr_tail: format!("leaked {SECRET_CANARY}"),
        turn_results: Vec::new(),
        workdir: PathBuf::from("/private/host/path"),
        boot_time: Duration::from_millis(250),
        info_events: Vec::new(),
    });
    report
}

#[test]
fn renderers_identify_the_exact_failed_cell() {
    let report = failed_report();
    let console = render_console(&report);
    let markdown = render_markdown(&report);
    let json = render_json(&report);
    let json_text = serde_json::to_string(&json).expect("report JSON must serialize");

    for rendered in [&console, &markdown, &json_text] {
        assert!(rendered.contains("deterministic-edit"));
        assert!(rendered.contains("openai"));
        assert!(rendered.contains("secret_detected"));
        assert!(!rendered.contains("not implemented"));
        assert!(!rendered.contains("not_implemented"));
    }

    assert_eq!(json["schema"], "wayland.eval.report");
    assert_eq!(json["schema_version"], 1);
}

#[test]
fn renderers_never_emit_raw_result_payloads() {
    let report = failed_report();
    let rendered = [
        render_console(&report),
        render_markdown(&report),
        serde_json::to_string(&render_json(&report)).expect("report JSON must serialize"),
    ];

    for output in rendered {
        assert!(!output.contains(SECRET_CANARY));
        assert!(!output.contains("/private/host/path"));
    }
}
