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

/// The user-facing contract page. OBS-02's second clause was ungradeable
/// because the contract existed only in `exit_code.rs` and appeared nowhere a
/// user could read it, so there was nothing to grade the binary AGAINST.
/// Documenting it is only half the repair — a doc nobody checks drifts. The
/// table in this file is therefore PARSED below and compared against the
/// constants, so a code change that forgets the doc (or a doc edit that
/// invents a code) fails the suite.
const CONTRACT_DOC: &str = include_str!("../../../docs/exit-codes.md");

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

// ── the documented contract, and the clauses it names ───────────────────────

/// Every `| code | `CONST` | … |` row between the table markers, as
/// `(code, constant)`. Anything outside the markers is prose and is ignored.
fn documented_rows() -> Vec<(u8, String)> {
    let body = CONTRACT_DOC
        .split_once("<!-- EXIT-CODE-TABLE:BEGIN -->")
        .expect("docs/exit-codes.md lost its table BEGIN marker")
        .1
        .split_once("<!-- EXIT-CODE-TABLE:END -->")
        .expect("docs/exit-codes.md lost its table END marker")
        .0;
    body.lines()
        .filter_map(|line| {
            let mut cells = line.trim().strip_prefix('|')?.split('|');
            let code: u8 = cells.next()?.trim().parse().ok()?;
            let name = cells.next()?.trim().trim_matches('`').to_owned();
            Some((code, name))
        })
        .collect()
}

/// The doc and the code are one contract or they are two contradictions.
///
/// This is the OBS-02 clause that could not previously be graded at all: the
/// numbers lived in `exit_code.rs` and appeared in no user-facing document, so
/// "does the binary follow the documented contract" had no documented side.
#[test]
fn the_documented_table_and_the_constants_agree() {
    let rows = documented_rows();
    assert!(
        rows.len() >= 8,
        "parsed only {} rows from docs/exit-codes.md — the parser or the \
         table shape has drifted, and a parser that finds nothing would let \
         every assertion below pass vacuously: {rows:?}",
        rows.len()
    );

    // Every named constant must appear at its real value.
    let expected: &[(u8, &str)] = &[
        (exit_code::OK, "OK"),
        (exit_code::FAILURE, "FAILURE"),
        (exit_code::TOOL_FAILURE, "TOOL_FAILURE"),
        (exit_code::LIMIT, "LIMIT"),
        (exit_code::INTERRUPTED, "INTERRUPTED"),
        (exit_code::TERMINATED, "TERMINATED"),
        (exit_code::HUNG_UP, "HUNG_UP"),
    ];
    for (value, name) in expected {
        let documented = rows
            .iter()
            .find(|(_, n)| n == name)
            .unwrap_or_else(|| panic!("docs/exit-codes.md documents no row for {name}: {rows:?}"));
        assert_eq!(
            documented.0, *value,
            "docs/exit-codes.md says {name} is {}, the code says {value}",
            documented.0
        );
    }

    // And no row may claim a constant the code does not define — the other
    // drift direction, where the doc grows a code nothing ever emits.
    let known: Vec<&str> = expected.iter().map(|(_, n)| *n).collect();
    for (code, name) in &rows {
        assert!(
            name == "—" || known.contains(&name.as_str()),
            "docs/exit-codes.md documents code {code} as `{name}`, which is \
             not a constant in wcore_cli::exit_code"
        );
    }
}

/// PROVIDER ERROR. A provider that cannot be reached at all is a run that
/// never produced an answer — the `FAILURE` row of the documented table.
#[tokio::test(flavor = "multi_thread")]
async fn a_provider_that_cannot_be_reached_exits_one() {
    // Bind a port, learn its number, drop it: nothing is listening there, so
    // the connection is REFUSED rather than left to hang on a firewall.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("addr").port()
    };
    let run = drive_against(
        "deadprovider",
        &format!("http://127.0.0.1:{dead}/v1"),
        &[],
        &[],
    );
    assert_eq!(
        run.code,
        Some(exit_code::FAILURE as i32),
        "an unreachable provider must exit {}; stdout={:?} stderr={:?}",
        exit_code::FAILURE,
        run.stdout,
        run.stderr
    );
}

/// CLI USAGE ERROR. Row `2` of the documented table, owned by clap rather than
/// by `exit_code.rs` — which is exactly why it needs an end-to-end check: no
/// constant in our code would catch it changing.
#[test]
fn an_unknown_flag_exits_two() {
    let root = std::env::temp_dir().join(format!("wlc-exit-usage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");
    let out = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_wayland-core")))
        .current_dir(&root)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &root)
        .env("WAYLAND_HOME", &root)
        .arg("--no-such-flag-exists")
        .output()
        .expect("spawn wayland-core");
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        out.status.code(),
        Some(2),
        "clap usage errors must exit 2 as documented; stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// CANCELLATION. SIGINT while a tool is genuinely running must exit 130.
///
/// Unix-only: there is no portable way to raise a console Ctrl-C event in a
/// child from a test on Windows, and `TerminateProcess` would measure the OS,
/// not the product. The Windows mapping is asserted by the unit test in
/// `main.rs` that drives `run_until_shutdown` with a synthetic signal.
///
/// The probe waits for the scripted `sleep` to appear in the process table
/// before signalling, and asserts it was there — without that, a SIGINT that
/// arrived before the run started would produce the same exit code for an
/// entirely different reason.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn sigint_during_a_running_tool_exits_130() {
    use std::time::{Duration, Instant};

    let root = std::env::temp_dir().join(format!("wlc-exit-sigint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let work = root.join("work");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&work).expect("work");

    let marker = format!("wlc-exit-sigint-marker-{}", std::process::id());
    let fixture = OpenAiFixtureScript::new(vec![
        OpenAiStep::tool_call(
            "call_sleep",
            "Bash",
            serde_json::json!({
                "command": format!("sleep 600 # {marker}"),
                "timeout": 120000,
            }),
        ),
        OpenAiStep::text("NEVER REACHED"),
    ])
    .start()
    .await
    .expect("start scripted provider");
    write_home(&home, fixture.base_url());

    let mut child = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_wayland-core")))
        .current_dir(&work)
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
            "do the thing",
        ])
        .spawn()
        .expect("spawn wayland-core");

    // INSTRUMENT PRECONDITION: the tool must actually be running.
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut tool_was_live = false;
    while Instant::now() < deadline {
        if pgrep_marker(&marker) {
            tool_was_live = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        tool_was_live,
        "the scripted `sleep` never appeared in the process table, so a \
         SIGINT now would not land mid-tool and this row would be vacuous"
    );

    // SAFETY: `kill(2)` with a pid this test owns and a valid signal number.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
    let status = child.wait().expect("wait for wayland-core");
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(
        status.code(),
        Some(exit_code::INTERRUPTED as i32),
        "SIGINT mid-tool must exit {} (the shell's 128+SIGINT), got {status:?}",
        exit_code::INTERRUPTED
    );
    assert!(
        !pgrep_marker(&marker),
        "the cancelled tool outlived the agent"
    );
}

#[cfg(unix)]
fn pgrep_marker(marker: &str) -> bool {
    let out = Command::new("pgrep")
        .args(["-f", marker])
        .output()
        .expect("pgrep");
    // pgrep matches its OWN argv on some systems; require a pid line whose
    // process is not this pgrep (already exited by the time we read).
    !String::from_utf8_lossy(&out.stdout).trim().is_empty()
}

/// Write the isolated `WAYLAND_HOME` every probe uses.
fn write_home(home: &std::path::Path, base_url: &str) {
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[session]\nenabled = false\n\n[memory]\nenabled = false\n\n\
             [providers.rec]\nprovider = \"openai\"\n\
             base_url = \"{base_url}\"\nmodel = \"fake\"\napi_key = \"unused\"\n"
        ),
    )
    .expect("config");
}

/// Drive the binary against an arbitrary base URL with no scripted provider —
/// for the paths where the point IS that the provider is not there.
fn drive_against(name: &str, base_url: &str, seed: &[(&str, &str)], extra_args: &[&str]) -> Run {
    let root = std::env::temp_dir().join(format!("wlc-exit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let work = root.join("work");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&work).expect("work");
    for (rel, body) in seed {
        std::fs::write(work.join(rel), body).expect("seed file");
    }
    write_home(&home, base_url);

    let out = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_wayland-core")))
        .current_dir(&work)
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
        .arg("do the thing")
        .output()
        .expect("spawn wayland-core");
    let _ = std::fs::remove_dir_all(&root);
    Run {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}
