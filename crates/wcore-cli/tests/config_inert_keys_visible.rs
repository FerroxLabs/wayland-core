//! A config key can be perfectly spelled, parse, merge — and be read by
//! nothing. `[browser.stealth]` is exactly that: `wcore-config` parses both
//! fields and merges project over global, and no crate outside `wcore-config`
//! ever reads them. The Browser plugin shell hardcodes `ProviderHint::Auto`,
//! so an operator's `preferred_provider` never reaches selection.
//!
//! #1069's pass cannot catch this class: `serde_ignored` reports only keys the
//! struct REJECTS, and these are accepted. Measured before the fix, on the real
//! binary: a config setting both fields produced stderr that never contained
//! the string `preferred_provider`. The user was told nothing.
//!
//! Unit tests in `wcore-config` cover the detector and the exact notice text.
//! This file covers the only thing they cannot: that the notice comes out of
//! the real binary, on the channel a user reads. Deleting the one call that
//! wires the pass into the config loader leaves every unit test green and
//! fails this file — which is the whole reason it exists.
//!
//! Hermetic: `WAYLAND_HOME`/`HOME` point at a throwaway tempdir and the full
//! provider-credential env set is stripped, so every arm aborts on a missing
//! credential before any network call.

use std::path::Path;
use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;

/// The line every inert-key notice carries. Both arms key on this exact phrase
/// so the control cannot pass by matching some other output.
const NOTICE_MARKER: &str = "are read by nothing";

/// Run the real binary headless against a hermetic home and return stderr.
fn stderr_of_headless_run(home: &Path) -> String {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-core"));
    cmd.args(["--no-tui", "hello"]).current_dir(home);
    support::pty::harden_child_env(&mut cmd, home);
    let vault = support::vault::configure_process(&mut cmd);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd.spawn();
    drop(vault);
    let out = child
        .expect("spawn wayland-core headless")
        .wait_with_output()
        .expect("wait for wayland-core headless");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn write_config(home: &TempDir, body: &str) {
    std::fs::write(home.path().join("config.toml"), body).expect("write config.toml");
}

/// An operator who sets a knob we never read must be told so, on stderr, by
/// name — and told what to do instead.
#[test]
fn a_set_but_unread_knob_is_named_on_stderr() {
    let home = TempDir::new().expect("tempdir");
    write_config(
        &home,
        "[browser.stealth]\n\
         preferred_provider = \"chromium\"\n\
         allow_cloud_fallback = true\n",
    );

    let stderr = stderr_of_headless_run(home.path());

    assert!(
        stderr.contains(NOTICE_MARKER),
        "a config setting keys nothing reads must warn on stderr, got:\n{stderr}"
    );
    for key in [
        "browser.stealth.preferred_provider",
        "browser.stealth.allow_cloud_fallback",
    ] {
        assert!(
            stderr.contains(key),
            "the notice must name the inert key `{key}`, got:\n{stderr}"
        );
    }
    assert!(
        stderr.contains("WAYLAND_CAMOUFOX_BIN"),
        "the notice must say what to do INSTEAD, not just that the key is ignored, \
         got:\n{stderr}"
    );
    // Config resolution runs several times per launch. Once per file.
    assert_eq!(
        stderr.matches(NOTICE_MARKER).count(),
        1,
        "the notice must be printed exactly once per config file, got:\n{stderr}"
    );
}

/// CONTROL — the constraint that keeps this from becoming noise. A config that
/// does NOT set the knob must produce complete silence on this channel. Same
/// binary, same hermetic home, same abort path; the only difference is whether
/// the operator expressed an intent we are discarding.
#[test]
fn a_config_that_never_sets_the_knob_is_silent() {
    let home = TempDir::new().expect("tempdir");
    write_config(
        &home,
        "provider = \"anthropic\"\n\
         [browser.stealth]\n\
         preferred_provider = \"auto\"\n\
         allow_cloud_fallback = false\n",
    );

    let stderr = stderr_of_headless_run(home.path());

    assert!(
        !stderr.contains(NOTICE_MARKER),
        "a knob written at its own default discarded no intent and must stay silent, \
         got:\n{stderr}"
    );
    assert!(
        !stderr.contains("browser.stealth"),
        "nothing about [browser.stealth] may be said when nothing was overridden, \
         got:\n{stderr}"
    );
}

/// CONTROL — the inert notice must not swallow the #1069 unknown-key notice,
/// and must not be produced BY it. A misspelled table is unknown, not inert:
/// the user gets the typo notice and NOT the inert one.
#[test]
fn a_misspelled_table_gets_the_typo_notice_not_the_inert_one() {
    let home = TempDir::new().expect("tempdir");
    write_config(
        &home,
        "[browser.stealthh]\npreferred_provider = \"chromium\"\n",
    );

    let stderr = stderr_of_headless_run(home.path());

    assert!(
        stderr.contains("were not recognised and are IGNORED"),
        "a misspelled table is a typo and must get the #1069 notice, got:\n{stderr}"
    );
    assert!(
        !stderr.contains(NOTICE_MARKER),
        "a key that never parsed into the struct is not an inert key, got:\n{stderr}"
    );
}
