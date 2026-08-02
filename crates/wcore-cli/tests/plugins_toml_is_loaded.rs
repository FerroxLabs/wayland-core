//! `plugins.toml` must actually be read from disk at engine boot.
//!
//! Four documents and three runtime error strings tell operators to edit
//! `plugins.toml`, but the engine used to boot from
//! `PluginsConfig::default()` — the file was never opened, so `enabled =
//! false`, `plugin_signature_verification` and `trusted_plugin_keys` were
//! all inert. This pins the wiring at the product surface: the real binary,
//! a real `plugins.toml`, and a behaviour difference an operator can see.
//!
//! The observable is the local-inference route. `wayland-ollama` is a
//! statically linked inventory plugin that claims `--model ollama:*`. With
//! the plugin disabled in `plugins.toml`, nothing claims the route and
//! `bootstrap.rs` refuses by name rather than silently falling back to a
//! remote provider. With no `plugins.toml` at all, the plugin loads and that
//! refusal must NOT appear — otherwise this test would pass against an
//! engine that simply refuses everything.
//!
//! Hermetic: `WAYLAND_HOME` + `HOME` point at a throwaway tempdir and every
//! provider credential is stripped, so no developer key or config is read.
//! The assertions key on the presence or absence of the boot-time refusal, so
//! they hold whether or not a local Ollama daemon happens to answer — an
//! unreachable daemon and a 404 both fail *after* the route was claimed, which
//! is exactly what the negative leg needs to observe.

use std::path::Path;
use std::process::Command;

/// Substring of the refusal `bootstrap.rs` emits when the local route is
/// requested and no plugin claimed it.
const REFUSAL: &str = "requests the local inference route, but no provider";

const STRIPPED_PROVIDER_ENV: &[&str] = &[
    "API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "VERTEX_PROJECT",
    "VERTEX_LOCATION",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// Run the real binary against `home` with `--model ollama:*` and return the
/// combined stdout+stderr.
fn run_local_route(home: &Path) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wayland-core"));
    cmd.arg("--model")
        .arg("ollama:plugins-toml-probe")
        .arg("say hi")
        .env("WAYLAND_HOME", home)
        .env("HOME", home)
        .env("TERM", "dumb")
        .current_dir(home);
    for key in STRIPPED_PROVIDER_ENV {
        cmd.env_remove(key);
    }
    let out = cmd.output().expect("spawn wayland-core");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn plugins_toml_enabled_false_is_honoured_at_engine_boot() {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("plugins.toml"),
        "[[plugin]]\nname = \"wayland-ollama\"\nenabled = false\n",
    )
    .expect("write plugins.toml");

    let output = run_local_route(home.path());
    assert!(
        output.contains(REFUSAL),
        "`enabled = false` in plugins.toml was ignored — the wayland-ollama \
         plugin still claimed the local route. Output was:\n{output}"
    );
}

#[test]
fn without_plugins_toml_the_plugin_still_claims_the_local_route() {
    // No plugins.toml written: the defaults must keep the plugin enabled, so
    // the refusal above must NOT be reachable for every configuration.
    let home = tempfile::tempdir().expect("tempdir");
    assert!(!home.path().join("plugins.toml").exists());

    let output = run_local_route(home.path());
    assert!(
        !output.contains(REFUSAL),
        "with no plugins.toml the wayland-ollama plugin must load and claim \
         the local route; the engine refused instead. Output was:\n{output}"
    );
}

#[test]
fn malformed_plugins_toml_refuses_rather_than_booting_on_default_policy() {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(home.path().join("plugins.toml"), "this is not = = toml\n")
        .expect("write plugins.toml");

    let output = run_local_route(home.path());
    assert!(
        output.contains("plugins.toml"),
        "a plugins.toml the engine cannot parse must be named in the failure, \
         not silently replaced by default policy. Output was:\n{output}"
    );
}
