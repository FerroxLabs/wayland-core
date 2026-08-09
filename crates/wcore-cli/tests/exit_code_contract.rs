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
        .env("NO_COLOR", "1")
        .args([
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
