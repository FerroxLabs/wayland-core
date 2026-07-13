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
