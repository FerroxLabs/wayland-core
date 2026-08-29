//! #1069 — unknown / misnested config keys must be VISIBLE to the user.
//!
//! The detection pass (#326) already existed and already worked; it reported
//! through `tracing::warn!`. With `RUST_LOG` unset — the normal case — WARN is
//! routed to `$WAYLAND_HOME/logs/wayland-core.log` and only ERROR reaches
//! stderr, so a user who misnested `base_url` was told nothing at all and their
//! prompt plus their real API key went to the vendor's default endpoint.
//!
//! Unit tests in `wcore-config` cover the collector and the notice text. This
//! file covers the only thing they cannot: that the notice actually comes out
//! of the real binary, on the channel a user reads.
//!
//! Hermetic: `WAYLAND_HOME`/`HOME` point at a throwaway tempdir and the full
//! provider-credential env set is stripped, so both arms abort on a missing
//! credential before any network call.

use std::path::Path;
use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

/// The line every ignored-key notice carries. Both arms key on this exact
/// phrase so the control cannot pass by matching some other output.
const NOTICE_MARKER: &str = "were not recognised and are IGNORED";

/// Run the real binary headless against a hermetic home and return stderr.
fn stderr_of_headless_run(home: &Path) -> String {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-core"));
    cmd.args(["--no-tui", "hello"]).current_dir(home);
    support::pty::harden_child_env(&mut cmd, home);
    let vault = support::vault::configure_process(&mut cmd);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = OwnedTree::new(cmd.spawn().expect("spawn wayland-core headless"));
    drop(vault);
    let out = child
        .wait_with_output()
        .expect("wait for wayland-core headless");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// RED ARM — the issue's verbatim config. A user who meant "do not talk to the
/// vendor, talk to 127.0.0.1:8899" must be told, on stderr, that the setting
/// was dropped and where it actually belongs.
#[test]
fn misnested_base_url_is_named_on_stderr() {
    let home = TempDir::new().expect("tempdir");
    std::fs::write(
        home.path().join("config.toml"),
        "base_url = \"http://127.0.0.1:8899\"\n\
         provider = \"anthropic\"\n\
         modle = \"typo-model\"\n\
         [defaults]\n\
         [browser.polcy]\n",
    )
    .expect("write config.toml");

    let stderr = stderr_of_headless_run(home.path());

    assert!(
        stderr.contains(NOTICE_MARKER),
        "a config full of ignored keys must warn on stderr, got:\n{stderr}"
    );
    for key in ["base_url", "modle", "defaults", "browser.polcy"] {
        assert!(
            stderr.contains(key),
            "the notice must name the ignored key `{key}`, got:\n{stderr}"
        );
    }
    assert!(
        stderr.contains("[providers.anthropic]"),
        "a top-level base_url must be told the correct [providers.<name>] spelling, got:\n{stderr}"
    );
    // The notice is emitted from the config loader, and config resolution runs
    // several times per launch. Once per file, not once per resolve.
    assert_eq!(
        stderr.matches(NOTICE_MARKER).count(),
        1,
        "the notice must be printed exactly once per config file, got:\n{stderr}"
    );
}

/// CONTROL — the SAME settings, spelled correctly. Identical run, identical
/// abort path; the only difference is where the keys sit. Nothing may be
/// warned about, which proves the red arm above is reacting to the misnesting
/// and not to any config file at all (and that an upgrading user with a valid
/// file gets silence).
#[test]
fn correctly_nested_config_warns_about_nothing() {
    let home = TempDir::new().expect("tempdir");
    std::fs::write(
        home.path().join("config.toml"),
        "[default]\n\
         provider = \"anthropic\"\n\
         model = \"typo-model\"\n\
         \n\
         [providers.anthropic]\n\
         base_url = \"http://127.0.0.1:8899\"\n",
    )
    .expect("write config.toml");

    let stderr = stderr_of_headless_run(home.path());

    assert!(
        !stderr.contains(NOTICE_MARKER),
        "a correctly-nested config must produce no ignored-key notice, got:\n{stderr}"
    );
}
