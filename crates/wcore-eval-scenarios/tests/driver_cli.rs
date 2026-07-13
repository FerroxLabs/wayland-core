use std::process::{Command, Output};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn run(args: &[&str], with_deepseek_key: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wayland-eval"));
    command.args(args);
    command.env_remove("WCORE_EVAL_BIN");
    command.env_remove("WCORE_EVAL_PROVIDER");
    command.env_remove("ANTHROPIC_API_KEY");
    command.env_remove("OPENAI_API_KEY");
    if with_deepseek_key {
        command.env("DEEPSEEK_API_KEY", "fixture-key");
    } else {
        command.env_remove("DEEPSEEK_API_KEY");
    }
    command.output().expect("run wayland-eval")
}

fn output_context(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_wcore-eval-fixture")
}

#[test]
fn fixture_scenario_runs_through_exact_artifact_and_json_stream() {
    let output = run(
        &[
            "--scenario",
            "canary",
            "--provider",
            "deepseek",
            "--binary",
            fixture(),
            "--expected-source-commit",
            COMMIT,
        ],
        true,
    );

    assert!(output.status.success(), "{}", output_context(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("PASS canary deepseek"),
        "{}",
        output_context(&output)
    );
    assert!(
        stdout.contains("SUMMARY pass=1 fail=0 skip=0"),
        "{}",
        output_context(&output)
    );
}

#[test]
fn strict_mode_fails_before_spawn_when_required_key_is_absent() {
    let output = run(
        &[
            "--scenario",
            "canary",
            "--provider",
            "deepseek",
            "--strict",
            "--binary",
            fixture(),
            "--expected-source-commit",
            COMMIT,
        ],
        false,
    );

    assert!(!output.status.success(), "{}", output_context(&output));
    assert!(output.stdout.is_empty(), "{}", output_context(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing required provider credentials"),
        "{}",
        output_context(&output)
    );
}

#[test]
fn stale_expected_source_is_rejected_before_scenario_execution() {
    let output = run(
        &[
            "--scenario",
            "canary",
            "--provider",
            "deepseek",
            "--binary",
            fixture(),
            "--expected-source-commit",
            "ffffffffffffffffffffffffffffffffffffffff",
        ],
        true,
    );

    assert!(!output.status.success(), "{}", output_context(&output));
    assert!(output.stdout.is_empty(), "{}", output_context(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("provenance mismatch"),
        "{}",
        output_context(&output)
    );
}

#[test]
fn verify_binary_proves_identity_without_provider_credentials() {
    let output = run(
        &[
            "--verify-binary",
            "--binary",
            fixture(),
            "--expected-source-commit",
            COMMIT,
        ],
        false,
    );

    assert!(output.status.success(), "{}", output_context(&output));
    assert!(output.stderr.is_empty(), "{}", output_context(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("VERIFIED sha256="),
        "{}",
        output_context(&output)
    );
    assert!(
        stdout.contains("version=0.12.25") && stdout.contains(&format!("source={COMMIT}")),
        "{}",
        output_context(&output)
    );
}

#[test]
fn dry_matrix_renders_the_offline_plan_without_binary_discovery() {
    let output = run(
        &["--scenario", "canary", "--provider", "matrix", "--dry"],
        true,
    );

    assert!(output.status.success(), "{}", output_context(&output));
    assert!(output.stderr.is_empty(), "{}", output_context(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("PLAN canary deepseek")
            && stdout.contains("SKIP canary anthropic")
            && stdout.contains("SKIP canary openai")
            && stdout.contains("ESTIMATE upper_bound_usd="),
        "{}",
        output_context(&output)
    );
}

#[test]
fn non_finite_budgets_are_usage_errors() {
    for value in ["NaN", "inf"] {
        let output = run(&["--scenario", "canary", "--budget", value], true);
        assert!(!output.status.success(), "{}", output_context(&output));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("finite"),
            "{}",
            output_context(&output)
        );
    }
}

#[test]
fn failed_canary_aborts_every_remaining_run_cell() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wayland-eval"));
    command.args([
        "--scenario",
        "canary",
        "--scenario",
        "qa_slash_style",
        "--provider",
        "deepseek",
        "--binary",
        fixture(),
        "--expected-source-commit",
        COMMIT,
    ]);
    command.env("DEEPSEEK_API_KEY", "fixture-key");
    command.env("WCORE_EVAL_FIXTURE_FAIL_CANARY", "1");
    command.env_remove("ANTHROPIC_API_KEY");
    command.env_remove("OPENAI_API_KEY");
    let output = command.output().expect("run failing canary");

    assert!(!output.status.success(), "{}", output_context(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FAIL canary deepseek"), "{stdout}");
    assert!(
        stdout.contains("ABORTED qa_slash_style deepseek reason=canary"),
        "{stdout}"
    );
    assert!(
        stdout.contains("SUMMARY pass=0 fail=1 skip=0 aborted=1"),
        "{stdout}"
    );
}

#[test]
fn budget_reservation_aborts_before_spending() {
    let output = run(
        &[
            "--scenario",
            "canary",
            "--provider",
            "deepseek",
            "--budget",
            "0.01",
            "--binary",
            fixture(),
            "--expected-source-commit",
            COMMIT,
        ],
        true,
    );

    assert!(!output.status.success(), "{}", output_context(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("ABORTED canary deepseek reason=budget"),
        "{stdout}"
    );
    assert!(
        stdout.contains("SUMMARY pass=0 fail=0 skip=0 aborted=1"),
        "{stdout}"
    );
}

#[test]
fn fixture_run_publishes_one_redacted_receipt_bundle() {
    let temp = tempfile::tempdir().expect("temp report root");
    let report_root = temp.path().join("reports");
    let mut command = Command::new(env!("CARGO_BIN_EXE_wayland-eval"));
    command.args([
        "--scenario",
        "canary",
        "--provider",
        "deepseek",
        "--binary",
        fixture(),
        "--expected-source-commit",
        COMMIT,
        "--report-dir",
    ]);
    command.arg(&report_root);
    command.env("DEEPSEEK_API_KEY", "fixture-key");
    command.env_remove("ANTHROPIC_API_KEY");
    command.env_remove("OPENAI_API_KEY");
    let output = command.output().expect("run receipt fixture");
    assert!(output.status.success(), "{}", output_context(&output));

    let cells = std::fs::read_dir(&report_root)
        .expect("report root")
        .collect::<Result<Vec<_>, _>>()
        .expect("report entries");
    assert_eq!(cells.len(), 1);
    let cell = cells[0].path();
    for name in [
        "receipt.json",
        "events.jsonl",
        "junit.xml",
        "report.txt",
        "report.md",
    ] {
        let bytes = std::fs::read(cell.join(name)).expect("report projection");
        assert!(
            !bytes
                .windows(b"fixture-key".len())
                .any(|window| window == b"fixture-key")
        );
    }

    let receipt: wcore_eval_scenarios::receipt::EvidenceReceiptV1 =
        serde_json::from_slice(&std::fs::read(cell.join("receipt.json")).expect("receipt JSON"))
            .expect("versioned receipt");
    assert_eq!(receipt.schema, "wayland.eval.receipt");
    assert_eq!(receipt.body.identity.source_commit, COMMIT);
    assert_eq!(receipt.body.identity.binary_sha256.len(), 64);
    assert!(matches!(
        wcore_eval_scenarios::receipt::ReceiptVerifier::new()
            .verify(
                &receipt,
                &wcore_eval_scenarios::receipt::VerificationPolicy::default()
            )
            .expect("local receipt integrity")
            .authority,
        wcore_eval_scenarios::receipt::VerifiedAuthority::LocalNonAuthoritative
    ));
}
