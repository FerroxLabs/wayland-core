//! C4-F3 — a local (`ollama:`) turn must be attributed to the route that
//! served it, not to the configured compatibility profile.
//!
//! `compat.provider_type()` is the SOLE key every cost surface reads: the
//! cache/cost ledger (`engine.rs` `record_cache_ledger_sample`), both
//! `TurnTrace.provider` emission sites, the budget reservation's
//! `reservation_provider`, and the journalled provider-attempt identity.
//! `make_plugin_provider_router` (wcore-cli) claims every `ollama:`-prefixed
//! model and serves it from `wayland-ollama`, but `ProviderType` has no Ollama
//! variant, so `compat_defaults_for` used to hand that local turn the
//! configured REMOTE provider's profile. Measured consequence, live:
//! `ollama:smollm2:135m` ran on a local machine for nothing and was recorded
//! as `provider=anthropic`, billed **$0.0756** at Anthropic's family rate.
//!
//! **Why this file exists next to the tests that already passed.** Two
//! pre-existing suites covered the pieces and could not see the defect:
//!
//! * `wcore-observability/tests/cost_estimate.rs::cost_ollama_preset_is_zero`
//!   constructs `ProviderCompat::ollama_defaults()` BY HAND and proves it
//!   prices to zero. True, and useless here — until this fix that preset had
//!   no production construction site at all, so the test guarded a preset
//!   nothing selected.
//! * `local_model_no_credential_test.rs::local_model_resolves_without_any_credential`
//!   resolves the exact `Config` that carried the wrong label, and asserts on
//!   `cfg.model` and `cfg.api_key` — never on `cfg.compat`.
//!
//! So the assertion that catches this has to be made against a `Config` that
//! `Config::resolve` actually produced, on the compat field. That is what the
//! two positive tests below do.
//!
//! The two controls are not decoration. Without them a build that stamped
//! `ollama_defaults()` onto EVERY model — zero cost rows for real cloud spend,
//! a far more expensive bug in the other direction — would pass the positives.

use serial_test::serial;
use tempfile::TempDir;
use wcore_config::config::{CliArgs, Config};

/// Explicit inline key + isolated project dir + throwaway home: resolution is
/// hermetic, so neither the developer's credential store nor a stray
/// `.wayland-core.toml` can supply a `[provider.compat]` override that would
/// mask what the defaults selected.
fn args(provider: &str, model: &str, project_dir: &TempDir) -> CliArgs {
    CliArgs {
        provider: Some(provider.into()),
        api_key: Some("test-key-not-a-real-credential".into()),
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

fn resolve(provider: &str, model: &str) -> Config {
    let tmp = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let _home_guard = HomeGuard::enter(home.path());
    Config::resolve(&args(provider, model, &tmp))
        .unwrap_or_else(|e| panic!("resolve({provider}, {model}) failed: {e:#}"))
}

#[test]
#[serial]
fn local_model_is_attributed_to_the_local_route_not_the_configured_profile() {
    let cfg = resolve("anthropic", "ollama:smollm2:135m");

    assert_eq!(
        cfg.compat.provider_type(),
        "ollama",
        "the `ollama:` route serves this turn, so every cost surface keyed on \
         compat.provider_type() must say so. Got `{}` — that is the configured \
         compatibility profile, not the provider that ran the turn.",
        cfg.compat.provider_type()
    );
    assert_eq!(
        cfg.model, "ollama:smollm2:135m",
        "the prefix must survive resolution — the plugin router matches on it"
    );
}

#[test]
#[serial]
fn local_model_carries_the_free_cost_rows_not_the_cloud_family_rate() {
    let cfg = resolve("anthropic", "ollama:smollm2:135m");

    // This is the money half. `anthropic_defaults()` carries $15/Mtok input
    // and $75/Mtok output; charging a local model those rows is what produced
    // the $0.0756 bill on hardware that spent nothing.
    assert_eq!(
        cfg.compat.cost_per_input_token,
        Some(0.0),
        "a local model must not inherit the cloud provider's input rate"
    );
    assert_eq!(
        cfg.compat.cost_per_output_token,
        Some(0.0),
        "a local model must not inherit the cloud provider's output rate"
    );
    assert_eq!(
        cfg.compat.cost_is_known_free,
        Some(true),
        "the local route is free and must be labelled as KNOWN free, so a \
         zero is read as proved-free rather than as unpriced"
    );
}

/// CONTROL. A remote model under the same provider must be unchanged — same
/// id on the cost key, and real (non-zero) rates. Without this, a build that
/// applied `ollama_defaults()` unconditionally would pass both positives while
/// silently pricing all cloud spend at $0.
#[test]
#[serial]
fn remote_model_still_carries_its_own_provider_and_real_rates() {
    let cfg = resolve("anthropic", "claude-sonnet-4-6");

    assert_eq!(cfg.compat.provider_type(), "anthropic");
    assert!(
        cfg.compat.cost_per_input_token.unwrap_or(0.0) > 0.0,
        "a remote Anthropic model must keep a real input rate; got {:?}",
        cfg.compat.cost_per_input_token
    );
    assert_ne!(
        cfg.compat.cost_is_known_free,
        Some(true),
        "remote spend is never known-free"
    );
}

/// CONTROL. The local route is claimed on an anchored `ollama:` prefix. A
/// near-miss must NOT be diverted onto the free rows — that would under-charge
/// a real remote provider whose id merely starts with the same letters.
#[test]
#[serial]
fn a_near_miss_prefix_is_not_treated_as_local() {
    let cfg = resolve("anthropic", "ollamacloud:llama3");

    assert_eq!(
        cfg.compat.provider_type(),
        "anthropic",
        "`ollamacloud:` is not the local route"
    );
    assert!(cfg.compat.cost_per_input_token.unwrap_or(0.0) > 0.0);
}

/// Confines config resolution to a throwaway home so the developer's real
/// config and credential store cannot influence the resolved compat.
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
