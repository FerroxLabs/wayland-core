//! Issue #685 — the credential source is REPORTED, end to end.
//!
//! The unit test in `wcore-config` pins the words; this one pins the wiring.
//! A guard that is fully unit-tested and never reached is the failure mode this
//! file exists to exclude, so it runs the real binary and reads its real
//! stderr.
//!
//! `--doctor` is the vehicle: it performs the same `Config::resolve` a session
//! does — credential ladder included — and then exits without making a single
//! provider call, so nothing here can reach the network.
//!
//! NO REAL SECRET APPEARS HERE. Every key below is an invented literal.

use std::path::Path;
use std::process::Command;

/// Invented literals; credentials for nothing.
const ENV_KEY: &str = "f685-notice-fixture-env-not-a-credential";
const FLAG_KEY: &str = "f685-notice-fixture-flag-not-a-credential";
const DOTENV_KEY: &str = "f685-notice-fixture-dotenv-not-a-credential";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Run `--doctor` on an isolated home with every ambient credential stripped,
/// so the only key in play is the one the case sets.
fn doctor(home: &Path, cwd: &Path, extra_env: &[(&str, &str)], args: &[&str]) -> String {
    let mut cmd = Command::new(bin());
    cmd.arg("--doctor")
        .args(args)
        .env("WAYLAND_HOME", home)
        .env("HOME", home)
        .current_dir(cwd);
    for var in [
        "API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "WAYLAND_ALLOW_BARE_API_KEY",
    ] {
        cmd.env_remove(var);
    }
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    let out = cmd.output().expect("spawn wayland-core");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn homes() -> (tempfile::TempDir, tempfile::TempDir) {
    (
        tempfile::tempdir().expect("home"),
        tempfile::tempdir().expect("cwd"),
    )
}

#[test]
fn an_environment_credential_is_named_on_stderr() {
    let (home, cwd) = homes();
    let stderr = doctor(
        home.path(),
        cwd.path(),
        &[("ANTHROPIC_API_KEY", ENV_KEY)],
        &["--provider", "anthropic"],
    );

    assert!(
        stderr.contains("ANTHROPIC_API_KEY"),
        "the run used a credential from the environment and never said so.\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("enabled = false"),
        "the notice must name the OFF switch, since the whole point is that \
         clearing the config key does not turn the provider off.\nstderr:\n{stderr}"
    );
    // The notice must never carry the value it is describing.
    assert!(
        !stderr.contains(ENV_KEY),
        "the credential VALUE leaked into stderr:\n{stderr}"
    );
}

#[test]
fn a_wayland_env_file_credential_names_the_file() {
    let (home, cwd) = homes();
    std::fs::write(
        home.path().join(".env"),
        format!("ANTHROPIC_API_KEY={DOTENV_KEY}\n"),
    )
    .unwrap();

    let stderr = doctor(home.path(), cwd.path(), &[], &["--provider", "anthropic"]);

    assert!(
        stderr.contains("ANTHROPIC_API_KEY"),
        "a credential re-injected from ~/.wayland/.env was used silently.\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains(".env"),
        "the notice must name the FILE for a re-injected credential — that is the \
         source the user cannot see from their shell.\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(DOTENV_KEY),
        "the credential VALUE leaked into stderr:\n{stderr}"
    );
}

/// Polarity control. Without this, a notice printed unconditionally would pass
/// both assertions above while telling the user nothing.
#[test]
fn an_explicitly_supplied_credential_is_not_narrated() {
    let (home, cwd) = homes();
    let stderr = doctor(
        home.path(),
        cwd.path(),
        &[],
        &["--provider", "anthropic", "--api-key", FLAG_KEY],
    );

    assert!(
        !stderr.contains("is using the credential from"),
        "a credential the user typed on the command line must not be narrated \
         back at them.\nstderr:\n{stderr}"
    );
}
