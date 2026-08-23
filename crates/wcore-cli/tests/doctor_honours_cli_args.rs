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

// ---------------------------------------------------------------------------
// #1079's HEADLINE flag, and the three that share its mechanism.
//
// The threading fix above is graded by `--project-dir` alone. `--api-key`,
// `--provider`, `--model` and `--base-url` reach `Config::resolve` through the
// same `CliArgs` literal at `main.rs:1781`, but until the provider section
// existed they changed nothing a test could read: doctor printed no provider
// row at all. Measured on this tree before the section landed — with
// `api_key: cli.api_key.clone()` mutated to `api_key: None`, which restores the
// exact reported symptom, `cargo nextest -E 'binary(doctor_honours_cli_args) +
// binary(doctor_smoke)'` reported 7 tests run, 7 passed. Each test below
// reddens under the loss of its OWN field.
// ---------------------------------------------------------------------------

/// The built-in default provider is `anthropic` (`wcore-config` `config.rs`
/// `default_provider`), so asking for `openai` is observable: if `--provider`
/// is dropped the row falls back to `anthropic`.
const FLAG_PROVIDER: &str = "openai";
/// Strings no config on any host could produce, so their presence in doctor's
/// output is attributable to these flags alone.
const MODEL_MARKER: &str = "issue1079-model-marker";
const BASE_URL_MARKER: &str = "https://issue1079-base-url.invalid/v1";
/// Never a real credential — nothing is ever sent anywhere with it. Doctor
/// makes no provider call; the value exists only so a key RESOLVES.
const API_KEY_VALUE: &str = "sk-issue1079-not-a-real-key";

/// Positive control for every assertion in this block, the same role
/// [`MCP_SECTION_HEADER`] plays above.
const PROVIDER_SECTION_HEADER: &str = "Provider (this invocation):";

/// Every env var `resolve_api_key_from_env` consults for the two providers in
/// play here, plus the provider-agnostic `API_KEY` — which is honoured as a
/// credential for ANY provider, so a CI runner that sets it for an unrelated
/// service would otherwise resolve a config in the no-flag control and make it
/// vacuous.
const CREDENTIAL_ENV: &[&str] = &["API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"];

/// Like [`run`], but additionally strips every ambient credential so the only
/// key in play is the one the flags carry.
fn run_without_ambient_credentials(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(bin());
    cmd.args(args).env("WAYLAND_HOME", home).current_dir(cwd);
    for var in CREDENTIAL_ENV {
        cmd.env_remove(var);
    }
    cmd.output().expect("spawn wayland-core")
}

/// All four config-selecting flags at once, on an isolated home and a neutral
/// CWD. One invocation serves every test below; each asserts on its own row.
fn doctor_with_all_overrides() -> (String, String) {
    let home = tempfile::tempdir().expect("home tempdir");
    let neutral = tempfile::tempdir().expect("neutral cwd tempdir");
    let out = run_without_ambient_credentials(
        home.path(),
        neutral.path(),
        &[
            "--doctor",
            "--provider",
            FLAG_PROVIDER,
            "--api-key",
            API_KEY_VALUE,
            "--model",
            MODEL_MARKER,
            "--base-url",
            BASE_URL_MARKER,
        ],
    );
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The row labelled `label` inside the provider section, or a panic naming what
/// was actually printed. Asserting on the whole row rather than a bare
/// `contains` keeps a value from matching somewhere else in doctor's output.
///
/// Panicking when the section header is missing is the POSITIVE CONTROL: a run
/// that produced no provider section must fail loudly rather than report a
/// missing value, which would be indistinguishable from a regression.
fn provider_row(stdout: &str, label: &str) -> String {
    let section = stdout
        .split_once(PROVIDER_SECTION_HEADER)
        .unwrap_or_else(|| {
            panic!("no {PROVIDER_SECTION_HEADER:?} in doctor output — every assertion against it would be vacuous. stdout:\n{stdout}")
        })
        .1;
    section
        .lines()
        // `split_once` leaves the remainder of the header LINE as the first
        // element (empty); the rows start after it.
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .find(|l| l.trim_start().starts_with(label))
        .unwrap_or_else(|| panic!("no {label:?} row in the provider section. section:\n{section}"))
        .to_string()
}

/// RED ARM for `--provider`.
#[test]
fn doctor_reports_the_provider_the_invocation_selected() {
    let (stdout, _) = doctor_with_all_overrides();
    let row = provider_row(&stdout, "provider");
    assert!(
        row.contains(FLAG_PROVIDER) && row.contains("(from --provider)"),
        "#1079: --doctor did not resolve against the invocation's --provider. \
         row: {row:?}\nstdout:\n{stdout}"
    );
}

/// RED ARM for `--model`.
#[test]
fn doctor_reports_the_model_the_invocation_selected() {
    let (stdout, _) = doctor_with_all_overrides();
    let row = provider_row(&stdout, "model");
    assert!(
        row.contains(MODEL_MARKER) && row.contains("(from --model)"),
        "#1079: --doctor did not resolve against the invocation's --model. \
         row: {row:?}\nstdout:\n{stdout}"
    );
}

/// RED ARM for `--base-url`.
#[test]
fn doctor_reports_the_base_url_the_invocation_selected() {
    let (stdout, _) = doctor_with_all_overrides();
    let row = provider_row(&stdout, "base url");
    assert!(
        row.contains(BASE_URL_MARKER) && row.contains("(from --base-url)"),
        "#1079: --doctor did not resolve against the invocation's --base-url. \
         row: {row:?}\nstdout:\n{stdout}"
    );
}

/// RED ARM for `--api-key` — the flag in the ticket's title. Reddens under the
/// one-word regression the fix cannot prevent by construction: changing
/// `api_key: cli.api_key.clone()` to `api_key: None` in `main.rs` compiles.
#[test]
fn doctor_honours_the_api_key_the_invocation_supplied() {
    let (stdout, _) = doctor_with_all_overrides();
    let row = provider_row(&stdout, "api key");
    assert!(
        row.contains("present") && row.contains("(from --api-key)"),
        "#1079: --doctor ignored --api-key; no credential from the command \
         line reached Config::resolve. row: {row:?}\nstdout:\n{stdout}"
    );
}

/// The section must report the credential's PRESENCE, never its value —
/// doctor output is what users paste into bug reports.
///
/// The `present` assertion is the positive control: it proves a key really did
/// resolve on this run, so the absence assertion is about redaction rather than
/// about there being no key at all.
#[test]
fn doctor_never_prints_the_api_key_value() {
    let (stdout, stderr) = doctor_with_all_overrides();
    let row = provider_row(&stdout, "api key");
    assert!(
        row.contains("present"),
        "no credential resolved, so this test cannot certify redaction. \
         row: {row:?}\nstdout:\n{stdout}"
    );
    assert!(
        !stdout.contains(API_KEY_VALUE) && !stderr.contains(API_KEY_VALUE),
        "the doctor printed the API key itself.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// NEGATIVE CONTROL for all four tests above. With no override flags and no
/// ambient credential, none of the flagged values may appear — otherwise a
/// passing red arm could be explained by ambient config rather than by the
/// flags being honoured.
#[test]
fn doctor_without_overrides_reports_none_of_the_flagged_values() {
    let home = tempfile::tempdir().expect("home tempdir");
    let neutral = tempfile::tempdir().expect("neutral cwd tempdir");
    let out = run_without_ambient_credentials(home.path(), neutral.path(), &["--doctor"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains(PROVIDER_SECTION_HEADER),
        "no provider section at all — this control cannot certify anything. \
         stdout:\n{stdout}"
    );
    for marker in [MODEL_MARKER, BASE_URL_MARKER, API_KEY_VALUE] {
        assert!(
            !stdout.contains(marker),
            "{marker:?} appeared without the flag that carries it, so the red \
             arms above cannot attribute a pass to their flags. stdout:\n{stdout}"
        );
    }
    for attribution in [
        "(from --provider)",
        "(from --model)",
        "(from --base-url)",
        "(from --api-key)",
    ] {
        assert!(
            !stdout.contains(attribution),
            "{attribution:?} appeared with no flag passed. stdout:\n{stdout}"
        );
    }
}
