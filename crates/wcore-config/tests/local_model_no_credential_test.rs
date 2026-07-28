//! 27-C2 — the engine's own no-credential remediation must be true.
//!
//! On `MissingApiKey` the CLI prints, verbatim:
//!
//! > Provider 'anthropic' requires an API key. To use a LOCAL model with
//! > Ollama, select a model id prefixed with `ollama:`
//! > (e.g. `ollama:qwen3-coder:30b`) — no API key is needed.
//!
//! Before this fix, following that instruction verbatim reproduced the exact
//! same `MissingApiKey`, because `Config::resolve` returned it before the
//! model string was consulted at all. Measured on the shipped v0.12.25
//! artifact natively on macOS, Linux and Windows.
//!
//! **These tests are paired on purpose.** The negative control
//! (`remote_model_without_credential_still_refuses`) is what makes the
//! positive test meaningful: it proves the credential requirement is still
//! enforced for every model that is NOT local, so the exemption cannot be
//! read as "the key check was removed". Delete the control and the remaining
//! test could pass against a build with no credential enforcement at all.

use serial_test::serial;
use tempfile::TempDir;
use wcore_config::config::{CliArgs, Config};

/// Credential-bearing variables that would otherwise satisfy the key chain
/// from the developer's own shell and mask the behaviour under test.
const CREDENTIAL_VARS: &[&str] = &[
    "API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
];

/// RAII guard: removes the credential vars for the duration of a test and
/// restores whatever was there on drop, so the tests stay hermetic on a
/// thread-per-test runner. Paired with `#[serial]` because `set_var` is
/// process-global.
struct NoCredentialEnv {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl NoCredentialEnv {
    fn enter() -> Self {
        let saved = CREDENTIAL_VARS
            .iter()
            .map(|k| {
                let prev = std::env::var_os(k);
                unsafe { std::env::remove_var(k) };
                (*k, prev)
            })
            .collect();
        Self { saved }
    }
}

impl Drop for NoCredentialEnv {
    fn drop(&mut self) {
        for (k, prev) in self.saved.drain(..) {
            match prev {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

/// CliArgs carrying NO api key, against an isolated empty project dir so the
/// host's own `.wayland-core.toml` cannot supply one.
fn args_without_key(model: &str, project_dir: &TempDir) -> CliArgs {
    CliArgs {
        provider: None,
        api_key: None,
        base_url: None,
        model: Some(model.into()),
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: Some(project_dir.path().to_path_buf()),
    }
}

#[test]
#[serial]
fn local_model_resolves_without_any_credential() {
    let _env = NoCredentialEnv::enter();
    let tmp = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let _home_guard = HomeGuard::enter(home.path());

    let resolved = Config::resolve(&args_without_key("ollama:qwen3-coder:30b", &tmp));

    let cfg = resolved.unwrap_or_else(|e| {
        panic!(
            "the engine instructs users to pass an `ollama:`-prefixed model \
             when no API key is available, and says no API key is needed. \
             Resolution still failed: {e:#}"
        )
    });
    assert_eq!(
        cfg.model, "ollama:qwen3-coder:30b",
        "the `ollama:` prefix must survive resolution -- the plugin router \
         matches on it and strips it itself"
    );
    assert!(
        cfg.api_key.is_empty(),
        "a local model must not acquire a remote credential; got a non-empty key"
    );
}

/// NEGATIVE CONTROL. Without this, the test above would also pass on a build
/// that had simply stopped requiring credentials.
#[test]
#[serial]
fn remote_model_without_credential_still_refuses() {
    let _env = NoCredentialEnv::enter();
    let tmp = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let _home_guard = HomeGuard::enter(home.path());

    let resolved = Config::resolve(&args_without_key("claude-sonnet-4-6", &tmp));

    let err = match resolved {
        Ok(cfg) => panic!(
            "a REMOTE model with no credential anywhere must still refuse; \
             resolution succeeded with model={} api_key_empty={}",
            cfg.model,
            cfg.api_key.is_empty()
        ),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(
        msg.contains("No API key found") || msg.contains("API key"),
        "the refusal must name the missing credential, got: {msg}"
    );
}

/// The prefix test is on the raw string, so `ollamaX:` and a bare `ollama`
/// must NOT be treated as local -- otherwise a typo silently disables the
/// credential requirement for a remote model.
#[test]
fn only_the_exact_prefix_counts_as_local() {
    use wcore_types::model_aliases::is_local_model;
    assert!(is_local_model("ollama:llama3.1"));
    assert!(is_local_model("ollama:qwen3-coder:30b"));
    assert!(!is_local_model("ollama"));
    assert!(!is_local_model("ollamacloud:llama3"));
    assert!(!is_local_model("claude-sonnet-4-6"));
    assert!(!is_local_model("gpt-4o"));
    assert!(
        !is_local_model("my-ollama:llama3"),
        "the prefix must anchor at the start, not appear anywhere"
    );
}

/// Confines config resolution to a throwaway home so the developer's real
/// credential store cannot satisfy the key chain.
struct HomeGuard {
    prev: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn enter(path: &std::path::Path) -> Self {
        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe { std::env::set_var("WAYLAND_HOME", path) };
        Self { prev }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
    }
}
