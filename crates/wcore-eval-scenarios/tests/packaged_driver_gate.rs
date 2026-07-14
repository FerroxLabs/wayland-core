#![cfg(feature = "packaged-driver-gate")]

use std::path::{Path, PathBuf};
use std::process::Output;

use sha2::{Digest, Sha256};
use tokio::process::Command;
use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};
use wcore_eval_scenarios::runner::discover_binary;

fn expected_source_commit() -> String {
    let source = std::env::var("WAYLAND_BUILD_SOURCE_SHA")
        .expect("packaged-driver gate requires externally pinned WAYLAND_BUILD_SOURCE_SHA");
    assert!(
        source.len() == 40
            && source
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "WAYLAND_BUILD_SOURCE_SHA must be exactly 40 lowercase hexadecimal characters"
    );
    source
}

fn packaged_core() -> PathBuf {
    discover_binary().unwrap_or_else(|error| {
        panic!(
            "packaged-driver gate requires a packaged wayland-core binary; \
             build wcore-cli in this target directory first: {error}"
        )
    })
}

fn sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read packaged wayland-core bytes");
    format!("{:x}", Sha256::digest(bytes))
}

async fn driver(core: &Path, source: &str, extra_args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wayland-eval"));
    command
        .args(extra_args)
        .arg("--binary")
        .arg(core)
        .arg("--expected-source-commit")
        .arg(source)
        .env("OPENAI_API_KEY", "packaged-driver-fixture-key")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("WCORE_EVAL_BIN")
        .env_remove("WCORE_EVAL_PROVIDER");
    command.output().await.expect("execute wayland-eval driver")
}

fn context(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[tokio::test]
async fn packaged_core_identity_and_driver_gates_are_enforced() {
    let source = expected_source_commit();
    let core = packaged_core();
    let digest = sha256(&core);

    let verified = driver(&core, &source, &["--verify-binary"]).await;
    assert!(verified.status.success(), "{}", context(&verified));
    let verified_stdout = String::from_utf8_lossy(&verified.stdout);
    assert!(
        verified_stdout.contains(&format!("sha256={digest}"))
            && verified_stdout.contains(&format!("source={source}")),
        "driver did not bind the expected source and exact packaged bytes: {}",
        context(&verified)
    );

    let passing_fixture = OpenAiFixtureScript::new([OpenAiStep::text("READY")])
        .start()
        .await
        .expect("start passing OpenAI fixture");
    let passing_base_url = passing_fixture.base_url().to_string();
    let passed = driver(
        &core,
        &source,
        &[
            "--scenario",
            "canary",
            "--provider",
            "openai",
            "--base-url",
            &passing_base_url,
        ],
    )
    .await;
    let passing_observation = passing_fixture
        .shutdown()
        .await
        .expect("stop passing OpenAI fixture");
    assert!(passed.status.success(), "{}", context(&passed));
    let passed_stdout = String::from_utf8_lossy(&passed.stdout);
    assert!(
        passed_stdout.contains("PASS canary openai")
            && passed_stdout.contains("SUMMARY pass=1 fail=0 skip=0 aborted=0"),
        "{}",
        context(&passed)
    );
    assert!(
        passing_observation.complete(),
        "real packaged Core did not consume the passing fixture"
    );

    let failing_fixture = OpenAiFixtureScript::new([OpenAiStep::text("WRONG")])
        .start()
        .await
        .expect("start failing OpenAI fixture");
    let failing_base_url = failing_fixture.base_url().to_string();
    let failed = driver(
        &core,
        &source,
        &[
            "--scenario",
            "canary",
            "--provider",
            "openai",
            "--base-url",
            &failing_base_url,
        ],
    )
    .await;
    let failing_observation = failing_fixture
        .shutdown()
        .await
        .expect("stop failing OpenAI fixture");
    assert!(!failed.status.success(), "{}", context(&failed));
    let failed_stdout = String::from_utf8_lossy(&failed.stdout);
    assert!(
        failed_stdout.contains("FAIL canary openai")
            && failed_stdout.contains("SUMMARY pass=0 fail=1 skip=0 aborted=0"),
        "{}",
        context(&failed)
    );
    assert!(
        failing_observation.complete(),
        "real packaged Core did not consume the hard-gate fixture"
    );
}
