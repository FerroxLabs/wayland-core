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
use wcore_config::config::{CliArgs, Config, store_provider_api_key};

/// Invented literals. Distinct per source so an assertion can name WHICH rung
/// answered, not merely that some rung did.
const BARE_ENV_KEY: &str = "f685-fixture-bare-api-key-not-a-credential";
const ANTHROPIC_ENV_KEY: &str = "f685-fixture-anthropic-env-not-a-credential";
const CLI_KEY: &str = "f685-fixture-cli-flag-not-a-credential";
const STORE_KEY: &str = "f685-fixture-store-not-a-credential";
const ENV_FILE_KEY: &str = "f685-fixture-dotenv-not-a-credential";
const VAULT_PASS: &str = "f685-test-vault-passphrase";

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

/// The opt-in must fail CLOSED for every non-affirmative value, not merely when
/// it is unset.
///
/// [`bare_api_key_is_not_adopted_as_a_provider_credential`] above only ever
/// exercises the UNSET case, so it survives the two most likely wrong rewrites
/// of the gate: `std::env::var(ALLOW_BARE_API_KEY_ENV).is_ok()` and
/// `!value.is_empty()`. Under either of those, an operator who writes
/// `WAYLAND_ALLOW_BARE_API_KEY=0` in order to turn the bare key OFF turns it
/// ON, and a generic `API_KEY` exported for an unrelated service is adopted as
/// this provider's credential and sent to the configured endpoint. That is the
/// disclosure path #685 closed, so the refusal is asserted value by value
/// rather than only for the absent variable.
///
/// Asserted in BOTH directions: a gate that refused everything would pass a
/// one-directional test while silently deleting a documented input
/// (`docs/getting-started.md`, "API Key Resolution Order").
#[test]
#[serial(f685_credential_off_state)]
fn the_bare_api_key_opt_in_fails_closed_on_non_affirmative_values() {
    /// Not an explicit yes. Every one of these must leave the bare `API_KEY`
    /// ignored, including the two an operator reaches for to disable it.
    const REFUSED: &[&str] = &[
        "0", "false", "FALSE", "no", "off", "", "   ", "2", "onn", "true-ish", "yesno", "null",
        "disabled",
    ];
    /// An explicit yes. The gate trims and lowercases, so the accepted set is
    /// asserted in the spellings a real shell and a real CI file produce.
    const ACCEPTED: &[&str] = &["1", "true", "TRUE", "True", "yes", "YES", "on", " on "];

    for value in REFUSED {
        let (_home, project, mut env) = fixture("");
        env.set("API_KEY", BARE_ENV_KEY);
        env.set("WAYLAND_ALLOW_BARE_API_KEY", value);

        match Config::resolve(&cli(project.path())) {
            Ok(config) => panic!(
                "WAYLAND_ALLOW_BARE_API_KEY={value:?} is not an affirmative opt-in, yet the \
                 bare API_KEY was adopted as the anthropic credential (resolved a key of \
                 len {}). The gate must fail closed on anything that is not an explicit yes.",
                config.api_key.len()
            ),
            Err(err) => assert!(
                err.to_string().contains("No API key found"),
                "WAYLAND_ALLOW_BARE_API_KEY={value:?}: expected the missing-credential \
                 refusal, got: {err}"
            ),
        }
    }

    for value in ACCEPTED {
        let (_home, project, mut env) = fixture("");
        env.set("API_KEY", BARE_ENV_KEY);
        env.set("WAYLAND_ALLOW_BARE_API_KEY", value);

        let config = Config::resolve(&cli(project.path())).unwrap_or_else(|err| {
            panic!(
                "WAYLAND_ALLOW_BARE_API_KEY={value:?} IS an affirmative opt-in, yet the \
                 documented bare API_KEY was refused: {err}"
            )
        });
        assert_eq!(
            config.api_key, BARE_ENV_KEY,
            "WAYLAND_ALLOW_BARE_API_KEY={value:?} resolved a credential that is not the \
             bare API_KEY"
        );
    }
}

// ── 2. The OFF state ─────────────────────────────────────────────────────────

const DISABLED: &str = "[providers.anthropic]\nenabled = false\n";

/// Non-vacuity control for every `enabled = false` case below: the SAME
/// fixture, with the flag absent, must resolve. Without this a refusal could
/// mean "the fixture supplied no credential at all".
#[test]
#[serial(f685_credential_off_state)]
fn control_enabled_provider_resolves_the_env_credential() {
    let (_home, project, mut env) = fixture("");
    env.set("ANTHROPIC_API_KEY", ANTHROPIC_ENV_KEY);

    let config = Config::resolve(&cli(project.path()))
        .expect("control: an enabled provider must resolve its env credential");
    assert_eq!(config.api_key, ANTHROPIC_ENV_KEY);
}

#[test]
#[serial(f685_credential_off_state)]
fn disabled_provider_refuses_an_environment_credential() {
    let (_home, project, mut env) = fixture(DISABLED);
    env.set("ANTHROPIC_API_KEY", ANTHROPIC_ENV_KEY);

    let err = Config::resolve(&cli(project.path()))
        .err()
        .unwrap_or_else(|| {
            panic!(
                "provider anthropic is `enabled = false` yet resolution succeeded from \
             ANTHROPIC_API_KEY — there is no OFF state"
            )
        });
    assert!(
        err.to_string().contains("disabled"),
        "expected a disabled-provider refusal, got: {err}"
    );
}

#[test]
#[serial(f685_credential_off_state)]
fn disabled_provider_refuses_a_config_file_credential() {
    let config_toml =
        format!("[providers.anthropic]\nenabled = false\napi_key = \"{ANTHROPIC_ENV_KEY}\"\n");
    let (_home, project, _env) = fixture(&config_toml);

    let err = Config::resolve(&cli(project.path()))
        .err()
        .unwrap_or_else(|| panic!("a disabled provider resolved its inline config api_key"));
    assert!(
        err.to_string().contains("disabled"),
        "expected a disabled-provider refusal, got: {err}"
    );
}

#[test]
#[serial(f685_credential_off_state)]
fn disabled_provider_refuses_a_cli_flag_credential() {
    let (_home, project, _env) = fixture(DISABLED);

    let mut args = cli(project.path());
    args.api_key = Some(CLI_KEY.to_string());

    let err = Config::resolve(&args)
        .err()
        .unwrap_or_else(|| panic!("a disabled provider resolved a --api-key flag"));
    assert!(
        err.to_string().contains("disabled"),
        "expected a disabled-provider refusal, got: {err}"
    );
}

#[test]
#[serial(f685_credential_off_state)]
fn disabled_provider_refuses_a_credentials_store_credential() {
    let (_home, project, mut env) = fixture(DISABLED);
    env.set("WAYLAND_VAULT_PASSPHRASE", VAULT_PASS);
    store_provider_api_key(wcore_config::config::ProviderType::Anthropic, STORE_KEY)
        .expect("store write");

    let err = Config::resolve(&cli(project.path()))
        .err()
        .unwrap_or_else(|| panic!("a disabled provider resolved its credentials-store slot"));
    assert!(
        err.to_string().contains("disabled"),
        "expected a disabled-provider refusal, got: {err}"
    );
}

#[test]
#[serial(f685_credential_off_state)]
fn disabled_provider_refuses_a_wayland_env_file_credential() {
    let (home, project, _env) = fixture(DISABLED);
    std::fs::write(
        home.path().join(".env"),
        format!("ANTHROPIC_API_KEY={ENV_FILE_KEY}\n"),
    )
    .unwrap();
    // The startup re-injection that makes `~/.wayland/.env` a live fourth
    // source no host toggle can clear.
    wcore_config::env_file::load_wayland_env_file();

    let resolved = Config::resolve(&cli(project.path()));

    // Restore: `load_wayland_env_file` mutates the real process environment.
    // SAFETY: single-threaded inside the serial group.
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };

    let err = resolved
        .err()
        .unwrap_or_else(|| panic!("a disabled provider resolved a ~/.wayland/.env credential"));
    assert!(
        err.to_string().contains("disabled"),
        "expected a disabled-provider refusal, got: {err}"
    );
}
