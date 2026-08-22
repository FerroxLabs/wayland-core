//! FerroxLabs/wayland#1079 — `--doctor` discards the invocation's config-
//! selecting arguments.
//!
//! Before the fix, `main.rs` called `doctor::run(cli.probe_mcp)` and nothing
//! else, and every `Config::resolve` inside `doctor/mod.rs` passed
//! `CliArgs::default()`. So `--profile` and `--project-dir` were dropped on
//! the floor and the two config-derived sections — the declared MCP server
//! list and the durable-sessions verdict — were computed against a DIFFERENT
//! config than the one the same flags would select for a real run. `run` now
//! takes the invocation's own `CliArgs` and threads it to all three sites
//! (`doctor/mod.rs:440`, `:501`, `:650`).
//!
//! The ticket describes a missing provider/model/api-key row. That is not the
//! defect: doctor prints no such row at all, so there is nothing to be wrong.
//! The damage measured here is the wrong-config computation above.
//!
//! Every run is hermetic: `WAYLAND_HOME` points at a per-test temp dir so the
//! host's real config, trust store and plugin root cannot contaminate the
//! result, and the CWD is a third empty dir so an ambient `.wayland-core.toml`
//! cannot be picked up by the no-flag control.

use std::path::Path;
use std::process::Command;

/// A name no other config on any host could plausibly declare, so its presence
/// in doctor's output is attributable to THIS test's project config alone.
const MARKER_SERVER: &str = "issue1079-project-scoped-marker";

/// The section header doctor always prints. Used as the POSITIVE CONTROL for
/// every absence assertion below: if this is missing, the run did not produce
/// real doctor output and a "marker not found" result would be meaningless.
const MCP_SECTION_HEADER: &str = "MCP servers (declared):";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Write a project config declaring `MARKER_SERVER`, plus a provider/key so
/// `Config::resolve` succeeds on a host with no ambient credentials.
///
/// The key belongs under `[providers.<name>]`, not `[default]` —
/// `DefaultConfig` has no `api_key` field, so a key placed there is silently
/// dropped and the resolve fails `MissingApiKey`, which degrades the very
/// sections this test reads. No command ever runs with it: doctor without
/// `--probe-mcp` only lists what the config declares.
fn write_project_config(dir: &Path) {
    std::fs::write(
        dir.join(".wayland-core.toml"),
        format!(
            r#"
[default]
provider = "anthropic"

[providers.anthropic]
api_key = "test-key-not-used"

[mcp.servers.{MARKER_SERVER}]
transport = "stdio"
command = "echo"
"#
        ),
    )
    .expect("write project .wayland-core.toml");
}

/// Run the real binary with an isolated `WAYLAND_HOME` and an explicit CWD.
fn run(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .env("WAYLAND_HOME", home)
        .current_dir(cwd)
        .output()
        .expect("spawn wayland-core")
}

/// `[mcp.servers.*]` is authority-expanding, so an UNTRUSTED workspace has it
/// stripped by the trust gate before doctor could ever see it. Grant trust
/// first, or this test would measure the trust gate rather than #1079.
fn trust(home: &Path, cwd: &Path, project: &Path) {
    let out = run(
        home,
        cwd,
        &[
            "--trust-workspace",
            "--project-dir",
            &project.to_string_lossy(),
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Trusted workspace executable fingerprint"),
        "the workspace was not trusted, so this test would measure the trust \
         gate instead of #1079. stderr:\n{stderr}"
    );
}

/// RED ARM. `--project-dir` selects the config a real run would use, so
/// `--doctor --project-dir X` must report the MCP servers declared by X.
#[test]
fn doctor_reports_mcp_servers_from_the_requested_project_dir() {
    let home = tempfile::tempdir().expect("home tempdir");
    let project = tempfile::tempdir().expect("project tempdir");
    let neutral = tempfile::tempdir().expect("neutral cwd tempdir");
    write_project_config(project.path());
    trust(home.path(), neutral.path(), project.path());

    let out = run(
        home.path(),
        neutral.path(),
        &[
            "--doctor",
            "--project-dir",
            &project.path().to_string_lossy(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    // POSITIVE CONTROL: prove we captured real doctor output before asserting
    // anything about what is missing from it.
    assert!(
        stdout.contains(MCP_SECTION_HEADER),
        "no MCP section in doctor output — the absence assertion below would \
         be vacuous. stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        stdout.contains(MARKER_SERVER),
        "#1079: --doctor discarded --project-dir, so the MCP section was \
         computed against the ambient config instead of the requested \
         workspace. Expected to find {MARKER_SERVER:?}. stdout:\n{stdout}"
    );
}

/// NEGATIVE CONTROL for the test above. With no `--project-dir` and a CWD that
/// declares nothing, the marker must NOT appear. Without this, a passing red
/// arm could be explained by the marker leaking in from the ambient
/// environment rather than from the flag being honoured.
#[test]
fn doctor_without_project_dir_does_not_report_the_project_scoped_server() {
    let home = tempfile::tempdir().expect("home tempdir");
    let project = tempfile::tempdir().expect("project tempdir");
    let neutral = tempfile::tempdir().expect("neutral cwd tempdir");
    write_project_config(project.path());
    trust(home.path(), neutral.path(), project.path());

    let out = run(home.path(), neutral.path(), &["--doctor"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains(MCP_SECTION_HEADER),
        "no MCP section in doctor output — this control cannot certify \
         anything. stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains(MARKER_SERVER),
        "the marker leaked in without --project-dir, so the red arm above \
         cannot attribute a pass to the flag. stdout:\n{stdout}"
    );
}
