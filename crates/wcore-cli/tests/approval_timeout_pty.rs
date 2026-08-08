//! What a REAL terminal can still do after an approval prompt times out.
//!
//! The bound on the approval wait (`WAYLAND_APPROVAL_TIMEOUT_SECS`) closed a
//! hang: a pty that stays open and never delivers another line used to pin the
//! turn, the run and any lease it held, forever. The first cut of that bound
//! then latched `interactive_approver` off on expiry, on the theory that the
//! stdin reader it had to abandon made the terminal unusable. That is not what
//! a timeout means. The operator walked away for five minutes; the terminal is
//! still there. Latching left them told — falsely — that the run had no
//! interactive approver, with no recovery short of restarting the process.
//!
//! This is that claim, at the product level, on a real PTY:
//!
//! * **The control** — a prompt answered `y` at a real terminal runs the tool.
//!   Without it, the leg below could pass with an approval path that never
//!   works at all.
//! * **The leg** — the first prompt is left unanswered until the bound expires
//!   and is refused; the SECOND prompt, at the same terminal, is answered and
//!   its tool runs. One line clears the read the expired prompt left
//!   outstanding, which is exactly what the timeout notice tells the operator
//!   to do.
//!
//! Unix-only for the same reason as every other PTY test here: `portable_pty`
//! ConPTY cannot surface a headless child's stdout on a Windows runner.
#![cfg(unix)]

use std::time::Duration;

use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::tempenv::{self, TempEnv};

#[path = "support/mod.rs"]
mod support;

const FIXTURE_MODEL: &str = "fixture-chat-v1";
const FIXTURE_KEY: &str = "fixture-local-token";
/// Short enough to leave a test waiting for it, long enough that a loaded
/// runner cannot cross it between printing the prompt and our first answer.
const BOUND_SECS: &str = "5";
const SCREEN_BUDGET: Duration = Duration::from_secs(120);

/// One run of the headless surface on a real terminal, serving two `Write`
/// calls and then a final answer. Two calls is the whole point: the second is
/// the one a latched gate can never reach.
struct Run {
    pty: support::pty::Pty,
    first: std::path::PathBuf,
    second: std::path::PathBuf,
    _fixture: wcore_eval_scenarios::fixtures::openai::RunningOpenAiFixture,
    _env: TempEnv,
}

async fn start() -> Run {
    let provider = ProviderConfig::new(ProviderId::OpenAI, FIXTURE_MODEL)
        .with_api_key(FIXTURE_KEY)
        .with_known_free_cost()
        .with_base_url("http://127.0.0.1:1");
    let env: TempEnv = tempenv::build(&provider).expect("build hermetic Core environment");
    let first = env.path().join("canary-first.txt");
    let second = env.path().join("canary-second.txt");

    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "call-first",
            "Write",
            serde_json::json!({ "file_path": first.to_string_lossy(), "content": "FIRST" }),
        ),
        OpenAiStep::tool_call(
            "call-second",
            "Write",
            serde_json::json!({ "file_path": second.to_string_lossy(), "content": "SECOND" }),
        ),
        OpenAiStep::text("done"),
    ])
    .start()
    .await
    .expect("start loopback fixture");

    let pty = support::pty::Pty::spawn_with_args_env(
        env.home(),
        env.path(),
        40,
        160,
        &[
            "--no-tui",
            "--provider",
            "openai",
            "--model",
            FIXTURE_MODEL,
            "--base-url",
            fixture.base_url(),
            "write both canaries",
        ],
        &[
            ("OPENAI_API_KEY", FIXTURE_KEY),
            ("WAYLAND_APPROVAL_TIMEOUT_SECS", BOUND_SECS),
            ("NO_COLOR", "1"),
        ],
    );

    Run {
        pty,
        first,
        second,
        _fixture: fixture,
        _env: env,
    }
}

/// THE CONTROL. A prompt answered at a real terminal must run the tool — this
/// is the path the single stdin reader has to keep working.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approval_answered_at_a_real_terminal_runs_the_tool() {
    let mut run = start().await;
    run.pty.wait_for(
        |s| s.contains("canary-first"),
        SCREEN_BUDGET,
        "the approval prompt for the first Write",
    );
    run.pty.send(b"y\n");
    run.pty.wait_for(
        |s| s.contains("canary-second"),
        SCREEN_BUDGET,
        "the approval prompt for the second Write",
    );
    run.pty.send(b"y\n");
    run.pty.wait_for_exit(SCREEN_BUDGET);

    assert_eq!(
        std::fs::read_to_string(&run.first).unwrap_or_default(),
        "FIRST",
        "an approval typed at a real terminal did not run the tool\n--- screen ---\n{}",
        run.pty.screen_text()
    );
    assert_eq!(
        std::fs::read_to_string(&run.second).unwrap_or_default(),
        "SECOND",
        "the second approval at the same terminal did not run its tool\n--- screen ---\n{}",
        run.pty.screen_text()
    );
}

/// THE LEG. A prompt nobody answers in time is refused — and the terminal is
/// still an approval channel afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_terminal_that_missed_one_prompt_can_still_answer_the_next() {
    let mut run = start().await;
    run.pty.wait_for(
        |s| s.contains("canary-first"),
        SCREEN_BUDGET,
        "the approval prompt for the first Write",
    );

    // Answer nothing. The bound expires and the operator is told what happened
    // and what their terminal will do next.
    run.pty.wait_for(
        |s| s.contains("no answer in"),
        SCREEN_BUDGET,
        "the timeout notice",
    );
    let notice = run.pty.screen_text();
    assert!(
        !notice.contains("no interactive approver"),
        "a live terminal was told the run has no approver:\n{notice}"
    );

    // The next prompt still arrives — a latched gate refuses it silently.
    run.pty.wait_for(
        |s| s.contains("canary-second"),
        SCREEN_BUDGET,
        "the approval prompt for the second Write, which a latched gate never asks",
    );
    // One line clears the read the expired prompt left outstanding (the notice
    // says so), then the real answer.
    run.pty.send(b"\n");
    run.pty.send(b"y\n");
    run.pty.wait_for_exit(SCREEN_BUDGET);

    assert!(
        !run.first.exists(),
        "the unanswered call ran anyway: {}",
        run.first.display()
    );
    assert_eq!(
        std::fs::read_to_string(&run.second).unwrap_or_default(),
        "SECOND",
        "the operator came back, answered the next prompt, and the gate ignored \
         them\n--- screen ---\n{}",
        run.pty.screen_text()
    );
}
