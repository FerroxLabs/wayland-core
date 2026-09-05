//! W01: exercise the real live entry points against an offline process fixture.
//! Each subprocess gets its own profile and flags; no global environment edits.
#[path = "support/live_acceptance.rs"]
mod live_acceptance;
#[path = "live_personas.rs"]
mod personas;
#[path = "cross_session_live.rs"]
mod recall;

use std::process::{Output, Stdio};
use std::time::Duration;
use wcore_config::shell::shell_command_argv;

async fn invoke(
    case: &str,
    key: bool,
    filter: &str,
    report_is_directory: bool,
) -> (Output, String) {
    invoke_binary(
        case,
        key,
        filter,
        report_is_directory,
        env!("CARGO_BIN_EXE_wcore-eval-fixture"),
    )
    .await
}

async fn invoke_binary(
    case: &str,
    key: bool,
    filter: &str,
    report_is_directory: bool,
    binary: &str,
) -> (Output, String) {
    let home = tempfile::tempdir().unwrap();
    let report = home.path().join("report.md");
    if report_is_directory {
        std::fs::create_dir(&report).unwrap();
    }
    let mut child = shell_command_argv(
        std::env::current_exe().unwrap().to_str().unwrap(),
        &["--ignored", "--exact", case, "--nocapture"],
    );
    child
        .env("WAYLAND_HOME", home.path())
        .env("WAYLAND_REQUIRE_IGNORED", "1")
        .env("WCORE_EVAL_BIN", binary)
        .env("WCORE_EVAL_ONLY", filter)
        .env("WCORE_EVAL_PACING_SECS", "0")
        .env("WCORE_EVAL_REPORT_PATH", &report)
        .env_remove("WCORE_EVAL_ALLOW_SKIP")
        .env_remove("DEEPSEEK_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if key {
        child.env("DEEPSEEK_API_KEY", "fixture-not-a-real-key");
    }
    let output = tokio::time::timeout(Duration::from_secs(45), child.output())
        .await
        .expect("offline acceptance subprocess deadline")
        .expect("spawn acceptance entry point");
    let report = std::fs::read_to_string(report).unwrap_or_default();
    (output, report)
}

#[tokio::test]
async fn missing_key_cannot_pass_requested_personas() {
    let (out, _) = invoke("personas::overnight_personas", false, "canary", false).await;
    assert!(
        !out.status.success(),
        "requested persona acceptance silently skipped"
    );
}

#[tokio::test]
async fn missing_key_cannot_pass_requested_recall() {
    let (out, _) = invoke("recall::memory_recall_across_sessions", false, "", false).await;
    assert!(
        !out.status.success(),
        "requested recall acceptance silently skipped"
    );
}

#[tokio::test]
async fn empty_selection_cannot_pass_personas() {
    let (out, report) = invoke(
        "personas::overnight_personas",
        true,
        "w01-no-such-scenario",
        false,
    )
    .await;
    assert!(
        report.contains("0/0"),
        "empty-run report must still be collected"
    );
    assert!(!out.status.success(), "zero selected scenarios passed");
}

#[tokio::test]
async fn failed_persona_reports_before_failing_acceptance() {
    let (out, report) = invoke("personas::overnight_personas", true, "persona_coder", false).await;
    assert!(
        report.contains("persona_coder") && report.contains("FAIL"),
        "failure report missing: {report}"
    );
    assert!(
        !out.status.success(),
        "incorrect fixture behavior passed persona acceptance"
    );
}

#[tokio::test]
async fn failed_recall_cannot_pass_acceptance() {
    let (out, _) = invoke("recall::memory_recall_across_sessions", true, "", false).await;
    let log = String::from_utf8_lossy(&out.stderr);
    assert!(
        log.contains("xsession_recall"),
        "recall was not reached: {log}"
    );
    assert!(
        !out.status.success(),
        "fixture answered READY instead of recalling the fact but passed"
    );
}

#[tokio::test]
async fn report_write_failure_cannot_pass_acceptance() {
    let (out, _) = invoke("personas::overnight_personas", true, "canary", true).await;
    assert!(
        !out.status.success(),
        "unwritable report was reported as success"
    );
}

#[tokio::test]
async fn passing_canary_emits_report_and_passes() {
    let (out, report) = invoke("personas::overnight_personas", true, "canary", false).await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(report.contains("1/1 personas passed") && report.contains("canary"));
}

#[test]
fn extra_rounds_do_not_prove_successful_read() {
    assert!(!live_acceptance::successful_read_answer(
        "irrelevant answer",
        "unique sentinel",
        2,
        false
    ));
}

#[test]
fn guessed_text_without_a_read_is_not_tool_success() {
    assert!(!live_acceptance::successful_read_answer(
        "unique sentinel",
        "unique sentinel",
        1,
        false
    ));
    assert!(live_acceptance::successful_read_answer(
        "unique sentinel",
        "unique sentinel",
        2,
        true
    ));
}

#[test]
fn read_receipt_must_match_the_requested_file_and_successful_call() {
    use wcore_types::message::{ContentBlock, Message, Role};
    let call = Message::now(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "read-1".into(),
            name: "Read".into(),
            input: serde_json::json!({"file_path": "fixture.txt"}),
            extra: None,
        }],
    );
    for (id, failed, content, accepted) in [
        ("read-1", false, "random fixture secret", true),
        ("other-call", false, "random fixture secret", false),
        ("read-1", true, "random fixture secret", false),
        ("read-1", false, "unrelated output", false),
    ] {
        let messages = [
            call.clone(),
            Message::now(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: id.into(),
                    content: content.into(),
                    is_error: failed,
                }],
            ),
        ];
        assert_eq!(
            live_acceptance::observed_read(&messages, "fixture.txt", "random fixture secret"),
            accepted
        );
        assert!(!live_acceptance::observed_read(
            &messages,
            "different.txt",
            "random fixture secret"
        ));
    }
}

#[tokio::test]
async fn runner_start_failure_is_recorded_and_fails_acceptance() {
    // The test executable is a valid local file, but rejects Core's protocol
    // arguments immediately. No model or external executable is involved.
    let binary = std::env::current_exe().unwrap();
    let (out, report) = invoke_binary(
        "personas::overnight_personas",
        true,
        "canary",
        false,
        binary.to_str().unwrap(),
    )
    .await;
    assert!(
        report.contains("RUNNER ERROR"),
        "runner failure omitted from collected report: {report}"
    );
    assert!(!out.status.success(), "runner failure passed acceptance");
}

#[test]
fn equivalent_existing_file_spelling_counts_as_the_requested_read() {
    use wcore_types::message::{ContentBlock, Message, Role};
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("fixture.txt");
    std::fs::write(&file, "unique file contents").unwrap();
    let alias = root.path().join(".").join("fixture.txt");
    let messages = [
        Message::now(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "read-alias".into(),
                name: "Read".into(),
                input: serde_json::json!({"file_path": alias}),
                extra: None,
            }],
        ),
        Message::now(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "read-alias".into(),
                content: "unique file contents".into(),
                is_error: false,
            }],
        ),
    ];
    assert!(
        live_acceptance::observed_read(&messages, file.to_str().unwrap(), "unique file contents"),
        "equivalent spelling of the same existing file must not cause a false failure"
    );
}
