//! Issue #685 — the credential ladder has no OFF state, and it adopts a bare
//! `API_KEY`.
//!
//! Two distinct defects, both driven here through the REAL resolution entry
//! point ([`Config::resolve`]) rather than through the private rung functions,
//! because the reported symptoms are properties of the whole ladder:
//!
//! 1. **A bare, unnamespaced `API_KEY` is honoured as a named provider's
//!    credential.** A generic `API_KEY` exported for an entirely unrelated
//!    service is picked up and sent to the configured provider endpoint. It
//!    must require an explicit opt-in (`WAYLAND_ALLOW_BARE_API_KEY`), because
//!    the variable IS a documented input (`docs/getting-started.md`) and
//!    silently dropping it would break installs that rely on it.
//!
//! 2. **There is no disabled state.** Four independent sources feed the same
//!    ladder — CLI flag, `config.toml`, the credentials store, and the
//!    environment (which `~/.wayland/.env` re-injects at every startup) — so a
//!    host UI that clears one config field leaves three live. A provider marked
//!    `enabled = false` must stay off no matter which source holds a key.
//!
//! HOST INDEPENDENCE: every case sets `WAYLAND_HOME`, which makes `open_store`
//! skip the process-global OS keyring by construction, so nothing here can read
//! or write the developer's real Keychain. Identical on Linux, macOS, Windows.
//!
//! NO REAL SECRET APPEARS HERE. Every key below is an invented literal that is
//! a credential for nothing.

use std::ffi::OsString;
use std::path::Path;

use serial_test::serial;
use tempfile::{TempDir, tempdir};
use wcore_config::config::{CliArgs, Config};

/// Invented literals. Distinct per source so an assertion can name WHICH rung
/// answered, not merely that some rung did.
const BARE_ENV_KEY: &str = "f685-fixture-bare-api-key-not-a-credential";

/// Every variable that can satisfy the ladder from the developer's own shell.
/// Cleared by default in each fixture — an ambient key would make every
/// assertion below vacuous in the direction that hides the bug.
const AMBIENT: &[&str] = &[
    "API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "WAYLAND_ALLOW_BARE_API_KEY",
    "WAYLAND_VAULT_PASSPHRASE",
    "WAYLAND_VAULT_PASSPHRASE_FD",
    "WAYLAND_CONFIG_PATH",
];

/// RAII env guard. Restores prior values on drop so one case cannot leak its
/// fixture into the next in the same process.
struct EnvGuard {
    saved: Vec<(String, Option<OsString>)>,
}

impl EnvGuard {
    fn new() -> Self {
        let mut saved = Vec::new();
        for key in AMBIENT {
            saved.push(((*key).to_string(), std::env::var_os(key)));
            // SAFETY: every test in this binary is in the same serial group.
            unsafe { std::env::remove_var(key) };
        }
        Self { saved }
    }

    fn set(&mut self, key: &str, value: &str) {
        if !self.saved.iter().any(|(k, _)| k == key) {
            self.saved.push((key.to_string(), std::env::var_os(key)));
        }
        // SAFETY: see `new`.
        unsafe { std::env::set_var(key, value) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prior) in self.saved.drain(..).rev() {
            // SAFETY: see `EnvGuard::new`.
            unsafe {
                match prior {
                    Some(value) => std::env::set_var(&key, value),
                    None => std::env::remove_var(&key),
                }
            }
        }
    }
}

/// An isolated profile home plus a project dir, with every ambient credential
/// stripped. `config` is written verbatim to `$WAYLAND_HOME/config.toml`.
fn fixture(config: &str) -> (TempDir, TempDir, EnvGuard) {
    let home = tempdir().unwrap();
    let project = tempdir().unwrap();
    if !config.is_empty() {
        std::fs::write(home.path().join("config.toml"), config).unwrap();
    }
    let mut env = EnvGuard::new();
    env.set("WAYLAND_HOME", home.path().to_str().unwrap());
    env.set("HOME", home.path().to_str().unwrap());
    (home, project, env)
}

fn cli(project: &Path) -> CliArgs {
    CliArgs {
        provider: Some("anthropic".to_string()),
        project_dir: Some(project.to_path_buf()),
        ..CliArgs::default()
    }
}

// ── 1. The bare `API_KEY` ────────────────────────────────────────────────────

#[test]
#[serial(f685_credential_off_state)]
fn bare_api_key_is_not_adopted_as_a_provider_credential() {
    let (_home, project, mut env) = fixture("");
    // A generic API_KEY exported for some OTHER service. Nothing here names
    // Anthropic, and no opt-in is set.
    env.set("API_KEY", BARE_ENV_KEY);

    let resolved = Config::resolve(&cli(project.path()));

    match resolved {
        Ok(config) => panic!(
            "a bare, unnamespaced API_KEY was adopted as the anthropic credential \
             (resolved key came from the environment); it must require an explicit \
             opt-in. len={}",
            config.api_key.len()
        ),
        Err(err) => {
            let text = err.to_string();
            assert!(
                text.contains("No API key found"),
                "expected the missing-credential refusal, got: {text}"
            );
        }
    }
}

#[test]
#[serial(f685_credential_off_state)]
fn bare_api_key_is_honoured_when_explicitly_opted_in() {
    let (_home, project, mut env) = fixture("");
    env.set("API_KEY", BARE_ENV_KEY);
    env.set("WAYLAND_ALLOW_BARE_API_KEY", "1");

    let config = Config::resolve(&cli(project.path()))
        .expect("the documented bare API_KEY must still work behind its opt-in");
    assert_eq!(
        config.api_key, BARE_ENV_KEY,
        "the opt-in did not route the bare API_KEY into the ladder"
    );
}
