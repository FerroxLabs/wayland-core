//! B3 — the process exit code must track the run's outcome.
//!
//! Observed on the shipped binary (conformance probes `xc_task_failed`,
//! `ag13_truth_edit`, `ag15_truth_command`): a run whose last tool call failed
//! and was never retried, and a run stopped by the turn cap, BOTH exited 0 —
//! the same code as a clean success. Startup failures (bad config, unknown
//! flag, unknown profile) were faithful, so a non-zero exit was reachable;
//! task outcome simply never reached it. Any CI step, `just` recipe or cron
//! job that shells out to `wayland-core` and checks `$?` therefore could not
//! tell a completed run from a failed one.
//!
//! Graded from OUTSIDE the process: the exit status of a real
//! `wayland-core -p …` child, driven by a scripted loopback provider. The
//! agent's own stdout is read only to show what it CLAIMED.

use std::path::PathBuf;
use std::process::Command;

use wcore_cli::exit_code;
use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Drive the real binary through one scripted turn sequence in an isolated
/// `WAYLAND_HOME`, and report only what the OS reports.
///
/// `seed` files are written into the agent's cwd before it starts. `{WORK}` in
/// any scripted tool argument is replaced with that cwd, so a probe can name
/// absolute paths INSIDE the sandbox root (a path outside it is refused by the
/// workspace policy and would measure the policy, not the exit code).
async fn drive(
    name: &str,
    seed: &[(&str, &str)],
    extra_args: &[&str],
    steps: Vec<OpenAiStep>,
) -> Run {
    let root = std::env::temp_dir().join(format!("wlc-exit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let work = root.join("work");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&work).expect("work");
    for (rel, body) in seed {
        std::fs::write(work.join(rel), body).expect("seed file");
    }

    let script_json = serde_json::to_string(&steps)
        .expect("serialize script")
        .replace(
            "{WORK}",
            work.to_string_lossy().replace('\\', "\\\\").as_str(),
        );
    let steps: Vec<OpenAiStep> = serde_json::from_str(&script_json).expect("re-parse script");

    let fixture = OpenAiFixtureScript::new(steps)
        .start()
        .await
        .expect("start scripted provider");

    std::fs::write(
        home.join("config.toml"),
        format!(
            "[session]\nenabled = false\n\n[memory]\nenabled = false\n\n\
             [providers.rec]\nprovider = \"openai\"\n\
             base_url = \"{}\"\nmodel = \"fake\"\napi_key = \"unused\"\n",
            fixture.base_url()
        ),
    )
    .expect("config");

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_wayland-core"));
    let mut cmd = Command::new(bin);
    cmd.current_dir(&work)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("WAYLAND_HOME", &home)
        .env("NO_COLOR", "1");
    // Windows only: these are OS prerequisites, not host configuration.
    // Winsock loads its layered service providers from `%SystemRoot%`, so a
    // child launched without it fails EVERY send with WSAEPROVIDERFAILEDINIT
    // (os error 10106). The engine reads that as a transient connect failure
    // and spends its full 900 s provider-outage budget, so the whole binary
    // hung past the harness cap instead of grading an exit code. Measured on
    // Windows; the same allowlist already exists in
    // `wcore_cli::profile_router::ENV_PASSTHROUGH` and in the evaluator's
    // `ChildEnvironment::build`.
    #[cfg(windows)]
    for name in ["SystemRoot", "SystemDrive", "windir", "ComSpec", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            cmd.env(name, value);
        }
    }
    cmd.args([
        "-p",
        "rec",
        "-m",
        "fake",
        "--no-color",
        "--dangerously-skip-permissions",
    ])
    .args(extra_args)
    .arg("do the thing");
    let out = cmd.output().expect("spawn wayland-core");

    let _ = std::fs::remove_dir_all(&root);
    Run {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn read(path: &str) -> OpenAiStep {
    OpenAiStep::tool_call(
        format!("call_{path}"),
        "Read",
        serde_json::json!({ "file_path": path }),
    )
}

// ─────────────────────────────────────────────────────────────────────────
// T3 — the model is cut off by the provider's OUTPUT token cap while it is
// streaming a tool call. Wire signature from the field (A-4 run):
// `max_tokens: 8192`, `finish_reason: "length"`, buffer cut at
// `Write{"file_path": ".../review.json"`. The partial call used to be dropped
// with no event and no error, after which the engine saw an empty tool-call
// list and took the natural-completion path — ending the run early while
// telling the user the endpoint "may be incompatible".
// ─────────────────────────────────────────────────────────────────────────

/// The exact field cut point: a `Write` of the deliverable, severed after the
/// `file_path` value with the JSON object still open.
fn truncated_write() -> OpenAiStep {
    OpenAiStep::truncated_tool_call(
        "call_truncated_write",
        "Write",
        "{\"file_path\": \"{WORK}/review.json\"",
    )
}

/// The run must not end as though the model finished. A truncated deliverable
/// is a limit stop, and it must be told apart from the turn cap.
#[tokio::test(flavor = "multi_thread")]
async fn an_output_cap_truncation_is_not_reported_as_a_finished_run() {
    let run = drive(
        "outputcap",
        &[],
        &[],
        // Both attempts truncate: the bounded single retry is spent and the
        // run must terminate with the truth rather than a clean finish.
        vec![truncated_write(), truncated_write()],
    )
    .await;
    let said = format!("{}{}", run.stdout, run.stderr);

    assert_eq!(
        run.code,
        Some(exit_code::OUTPUT_TRUNCATED as i32),
        "a run cut off mid-deliverable by the output cap must report the \
         output-cap code, not success and not the turn cap; \
         stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
    assert_ne!(
        run.code,
        Some(exit_code::OK as i32),
        "a truncated write must never read as a completed run"
    );
    assert!(
        said.contains("output token limit"),
        "the user must be told the response hit the output token limit; \
         got stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
    assert!(
        said.contains("Write"),
        "the user must be told WHICH tool call was cut off; \
         got stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
    assert!(
        !said.contains("unexpected_request"),
        "the retry must be BOUNDED: the fixture scripts exactly two attempts \
         and answers a third with HTTP 409, so a loop here would surface as \
         `unexpected_request`; stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
    assert!(
        !said.contains("may be incompatible"),
        "the endpoint is not incompatible — that diagnosis sends the user \
         down the wrong path; got stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
}

/// The retry must actually fire, and must be enough for a model that gets it
/// right the second time. The fixture answers a fourth request with HTTP 409,
/// so this also bounds the retry: an unbounded loop could not pass.
#[tokio::test(flavor = "multi_thread")]
async fn an_output_cap_truncation_is_retried_once_and_can_recover() {
    let run = drive(
        "outputcap-recover",
        &[("present.txt", "SEEDED-CONTENT\n")],
        &[],
        vec![
            truncated_write(),
            read("{WORK}/present.txt"),
            OpenAiStep::text("recovered"),
        ],
    )
    .await;

    assert_eq!(
        run.code,
        Some(exit_code::OK as i32),
        "one retry after an output-cap truncation must be able to recover the \
         run; stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
    assert!(
        run.stdout.contains("recovered"),
        "the recovered answer must reach the user; stdout={:?}",
        run.stdout
    );
}

/// NEGATIVE CONTROL. A run that never truncates must be untouched: no extra
/// message and no extra request. The fixture returns HTTP 409 once its script
/// is exhausted, so a spurious retry would fail this run outright — this
/// asserts the absence of a retry, it does not merely hope for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_normal_tool_run_gains_no_truncation_message_and_no_extra_request() {
    let run = drive(
        "outputcap-negative",
        &[("present.txt", "SEEDED-CONTENT\n")],
        &[],
        vec![read("{WORK}/present.txt"), OpenAiStep::text("all done")],
    )
    .await;
    let said = format!("{}{}", run.stdout, run.stderr);

    assert_eq!(
        run.code,
        Some(exit_code::OK as i32),
        "an ordinary tool run must still exit 0; stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
    assert!(
        !said.contains("output token limit"),
        "an untruncated run must not be told anything about truncation; \
         stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
    assert!(
        !said.contains("unexpected_request"),
        "an untruncated run must issue no extra provider request; \
         stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
}

/// The turn cap and the output-token cap are unrelated events. Before this
/// they shared exit code 4, so no caller or harness could tell "the agent ran
/// out of turns" from "the model was cut off mid-answer".
#[tokio::test(flavor = "multi_thread")]
async fn the_output_cap_and_the_turn_cap_do_not_share_an_exit_code() {
    assert_ne!(
        exit_code::OUTPUT_TRUNCATED,
        exit_code::LIMIT,
        "max_tokens and max_turns must not be the same code"
    );
    let turn_cap = drive(
        "split-turncap",
        &[("present.txt", "SEEDED-CONTENT\n")],
        &["--max-turns", "1"],
        vec![
            read("{WORK}/present.txt"),
            read("{WORK}/present.txt"),
            OpenAiStep::text("never reached"),
        ],
    )
    .await;
    let output_cap = drive(
        "split-outputcap",
        &[],
        &[],
        vec![truncated_write(), truncated_write()],
    )
    .await;

    assert_eq!(turn_cap.code, Some(exit_code::LIMIT as i32));
    assert_eq!(output_cap.code, Some(exit_code::OUTPUT_TRUNCATED as i32));
    assert_ne!(
        turn_cap.code, output_cap.code,
        "the two limits must be distinguishable from outside the process"
    );
}

/// POSITIVE CONTROL. Without this, "always 0" and "0 means nothing" are
/// indistinguishable, and the failure assertions below could not be
/// attributed.
#[tokio::test(flavor = "multi_thread")]
async fn a_clean_run_exits_zero() {
    let run = drive("ok", &[], &[], vec![OpenAiStep::text("all done")]).await;
    assert_eq!(
        run.code,
        Some(exit_code::OK as i32),
        "a plainly successful run must exit 0; stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
}

/// The run's last action was a tool the tool itself flagged as failed, and the
/// model answered without retrying. Whatever the model then SAYS, the process
/// must not report success.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_ends_on_an_unrecovered_tool_failure_does_not_exit_zero() {
    let run = drive(
        "toolfail",
        &[],
        &[],
        vec![
            read("{WORK}/absent.txt"),
            // The model claims success anyway — the AG-13 / AG-15 shape.
            OpenAiStep::text("Done. Everything succeeded."),
        ],
    )
    .await;

    assert_eq!(
        run.code,
        Some(exit_code::TOOL_FAILURE as i32),
        "the run ended on a failed tool call and was never retried, yet the \
         process reported {:?}. The agent's own claim was {:?} (stderr={:?})",
        run.code,
        run.stdout,
        run.stderr
    );
}

/// FALSE-POSITIVE CONTROL. A tool that fails and is then RECOVERED must not
/// poison the exit code — otherwise every agent that probes for a missing file
/// and moves on looks broken.
///
/// The second read must genuinely SUCCEED, or this test would pass for the
/// wrong reason. The seeded file and the assertion on the agent's own
/// transcript below both guard that.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_tool_failure_still_exits_zero() {
    let run = drive(
        "recovered",
        &[("present.txt", "SEEDED-CONTENT\n")],
        &[],
        vec![
            read("{WORK}/absent.txt"),
            read("{WORK}/present.txt"),
            OpenAiStep::text("Found it on the second path."),
        ],
    )
    .await;

    assert!(
        run.stderr.contains("SEEDED-CONTENT"),
        "precondition: the recovery read must actually have succeeded, \
         otherwise this control is vacuous. stderr={:?}",
        run.stderr
    );
    assert_eq!(
        run.code,
        Some(exit_code::OK as i32),
        "a tool failure the model recovered from must still exit 0; \
         stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
}

/// The engine stopped the run at the turn cap: the model never got to finish.
/// That is neither a success nor a tool failure and needs its own code.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_stopped_at_the_turn_cap_reports_the_limit() {
    let run = drive(
        "maxturns",
        &[("present.txt", "SEEDED-CONTENT\n")],
        &["--max-turns", "1"],
        vec![
            read("{WORK}/present.txt"),
            read("{WORK}/present.txt"),
            OpenAiStep::text("never reached"),
        ],
    )
    .await;

    assert_eq!(
        run.code,
        Some(exit_code::LIMIT as i32),
        "a run truncated by the turn cap must be distinguishable from one \
         that finished; stdout={:?} stderr={:?}",
        run.stdout,
        run.stderr
    );
}
