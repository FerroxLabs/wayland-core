//! What a headless run reports when it had nobody to approve its tool calls.
//!
//! The audited failure: in the default posture a piped `-p` run has every
//! mutating tool refused (correctly — there is no approver), the refusal used
//! to be labelled "Tool execution denied by user" although no user existed,
//! and the process then exited **0**. A script, a CI job or a Desktop host
//! could not tell "did the work" from "did nothing".
//!
//! Two legs, and neither means anything without the other:
//!
//! * **A — the default posture.** The gate holds, the run says why, and the
//!   exit status is non-zero and distinct.
//! * **B — the positive control.** Same fixture, same piped stdin, one flag
//!   different (`--auto-approve`). The canary must appear on disk and the run
//!   must exit 0. Without B, leg A's "the canary is absent" is an absence of
//!   effect from an actor that may never have acted — the exact class the
//!   child-authority corpus recorded twelve times as F-V2.
//!
//! `QUOKKA-7F3A9C` appears nowhere else in the workspace, so the byte-exact
//! content check cannot be satisfied by a stale file or by anything the
//! harness itself wrote.

use std::process::Stdio;

use tokio::process::Command;
use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::tempenv::{self, TempEnv};

const FIXTURE_MODEL: &str = "fixture-chat-v1";
const FIXTURE_KEY: &str = "fixture-local-token";
const CANARY: &str = "QUOKKA-7F3A9C";

/// The distinctive clause of the CLI's operator advisory. The unit test
/// `headless_no_approver_advice_names_only_flags_that_work` owns the flag it
/// names; this only has to recognise it on stderr.
const ADVISORY_CLAUSE: &str = "no interactive approver (stdin is not a terminal)";

/// "The run completed nothing" — see `EXIT_RUN_COMPLETED_NOTHING` in the CLI.
const EXPECTED_BLOCKED_EXIT: i32 = 9;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

struct Outcome {
    status: Option<i32>,
    stdout: String,
    stderr: String,
    canary_exists: bool,
    canary_contents: String,
}

/// Serve four identical `Write` tool calls and then a final answer. Four is
/// above the retry burn observed in the audit, so a build that still treats
/// the refusal as retryable cannot exhaust the script and look calm.
async fn run(extra_args: &[&str]) -> Outcome {
    // The workspace is built FIRST: `Write` requires an absolute path, and the
    // fixture has to name it before it can be served. The base_url in the
    // written config is a placeholder — the real one is passed on the command
    // line below, which is what the provider actually uses.
    let provider = ProviderConfig::new(ProviderId::OpenAI, FIXTURE_MODEL)
        .with_api_key(FIXTURE_KEY)
        .with_known_free_cost()
        .with_base_url("http://127.0.0.1:1");
    let env: TempEnv = tempenv::build(&provider).expect("build hermetic Core environment");
    let canary = env.path().join("canary.txt");

    let steps = (0..4)
        .map(|i| {
            OpenAiStep::tool_call(
                format!("call-{i}"),
                "Write",
                serde_json::json!({
                    "file_path": canary.to_string_lossy(),
                    "content": CANARY,
                }),
            )
        })
        .chain(std::iter::once(OpenAiStep::text("done")));
    let fixture = OpenAiFixtureScript::new(steps)
        .start()
        .await
        .expect("start loopback fixture");

    let mut cmd = Command::new(binary());
    cmd.arg("--provider")
        .arg("openai")
        .arg("--model")
        .arg(FIXTURE_MODEL)
        .arg("--base-url")
        .arg(fixture.base_url());
    for arg in extra_args {
        cmd.arg(arg);
    }
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(180),
        cmd.arg("write the canary file")
            .current_dir(env.path())
            .env("HOME", env.path())
            .env("WAYLAND_HOME", env.home())
            .env("OPENAI_API_KEY", FIXTURE_KEY)
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            // stdin is a pipe that is closed immediately: the audited
            // invocation, and the one with no approver behind it.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("the headless run must terminate")
    .expect("collect Core output");

    Outcome {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        canary_exists: canary.exists(),
        canary_contents: std::fs::read_to_string(&canary).unwrap_or_default(),
    }
}

#[tokio::test]
async fn a_run_with_no_approver_refuses_says_why_and_does_not_exit_zero() {
    // LEG B FIRST — the positive control. If this fails, leg A proves nothing
    // and the failure should be reported against the control, not against the
    // gate.
    let allowed = run(&["--auto-approve"]).await;
    assert!(
        allowed.canary_exists && allowed.canary_contents.contains(CANARY),
        "POSITIVE CONTROL FAILED: with --auto-approve the fixture must write \
         the canary, so leg A can mean something.\nstatus={:?}\nstderr:\n{}",
        allowed.status,
        allowed.stderr
    );
    assert_eq!(
        allowed.status,
        Some(0),
        "a run that did the work must still exit 0"
    );
    assert!(
        !allowed.stderr.contains(ADVISORY_CLAUSE),
        "the advisory must not fire on a run that bypasses approvals"
    );

    // LEG A — the default posture.
    let blocked = run(&[]).await;
    let combined = format!("{}{}", blocked.stdout, blocked.stderr);

    // The gate still holds. This is the assertion a "just auto-approve when
    // headless" shortcut fails.
    assert!(
        !blocked.canary_exists,
        "the refused tool executed: canary.txt exists"
    );

    // The operator is told, before any token is spent, and told the remedy.
    assert!(
        blocked.stderr.contains(ADVISORY_CLAUSE),
        "stderr never named the missing approver:\n{}",
        blocked.stderr
    );
    assert!(blocked.stderr.contains("--auto-approve"));

    // The model is told the truth, not blamed on a user who was never asked.
    assert!(
        !combined.contains("denied by user"),
        "the run still claims a user refused:\n{combined}"
    );
    assert!(
        combined.contains(wcore_agent::orchestration::TOOL_BLOCKED_NO_APPROVER),
        "the refusal did not carry its cause:\n{combined}"
    );

    // And the exit status says the run could not be completed.
    assert_eq!(
        blocked.status,
        Some(EXPECTED_BLOCKED_EXIT),
        "a run whose tool calls were all refused for want of an approver must \
         not report success"
    );
}
