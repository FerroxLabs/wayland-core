//! #1173 — a keyless self-hosted endpoint must not be refused at startup.
//!
//! `wayland-core --provider openai --model qwen3:8b \
//!  --base-url http://127.0.0.1:11434/v1 "…"` exited with
//! `Error: No API key found`, even though the OpenAI provider already carries a
//! `SELF_HOSTED_PLACEHOLDER_KEY` path for exactly this case — a stock Ollama /
//! llama.cpp / LM Studio / vLLM server ignores the bearer entirely. The
//! capability existed; the startup gate returned before anything could reach
//! it, so every local-model user's first run died and the discovered workaround
//! was to invent a dummy `--api-key ollama` that is never meaningfully used.
//!
//! **The negative controls are the point of this file.** The exemption is only
//! defensible if it CANNOT be read as "the credential requirement was relaxed":
//!
//! - [`remote_endpoint_without_credential_still_refuses`] — a public host with
//!   no key is still refused, so nothing is ever sent to a remote endpoint
//!   without a real credential.
//! - [`default_endpoint_without_credential_still_refuses`] — the provider's own
//!   default endpoint never qualifies; only an endpoint the USER declared can.
//! - [`a_provider_without_a_keyless_wire_still_requires_a_key`] — locality alone
//!   is not enough. The provider must declare that its wire has a keyless path
//!   (`ProviderCompat::keyless_self_hosted`), or the clear startup refusal is
//!   kept rather than traded for an opaque 401 mid-turn.
//! - [`an_explicit_compat_optout_restores_the_key_requirement`] — the operator
//!   can switch the exemption back off.
//!
//! Delete any of them and the positive test would also pass against a build
//! that had simply stopped requiring credentials at all.

use std::collections::HashMap;

use serial_test::serial;
use tempfile::TempDir;
use wcore_config::config::{
    CliArgs, Config, CouncilProviderError, ProviderConfig, resolve_council_provider,
};

/// A stock Ollama endpoint over its OpenAI-compatible surface — the exact URL
/// from the issue's reproduction.
const LOCAL_ENDPOINT: &str = "http://127.0.0.1:11434/v1";

/// #1211 — a PUBLIC host whose query string merely CONTAINS `@` followed by a
/// loopback literal. Every byte after the `?` is query, so the request goes to
/// `api.openai.com`; a gate that reads the authority past the `?` calls it
/// loopback and waives the credential on the open internet.
const PUBLIC_HOST_WITH_USERINFO_QUERY: &str = "https://api.openai.com?x=@127.0.0.1";

/// Credential-bearing variables that would otherwise satisfy the key chain from
/// the developer's own shell and mask the behaviour under test.
const CREDENTIAL_VARS: &[&str] = &[
    "API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
];

/// RAII guard: removes the credential vars for the duration of a test and
/// restores whatever was there on drop. Paired with `#[serial]` because
/// `set_var` is process-global.
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

/// CliArgs carrying NO api key, against an isolated empty project dir so the
/// host's own `.wayland-core.toml` cannot supply one.
fn args_without_key(provider: &str, base_url: Option<&str>, project_dir: &TempDir) -> CliArgs {
    CliArgs {
        provider: Some(provider.into()),
        api_key: None,
        base_url: base_url.map(Into::into),
        model: Some("qwen3:8b".into()),
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: Some(project_dir.path().to_path_buf()),
    }
}

/// One isolated resolution: fresh project dir, fresh WAYLAND_HOME (optionally
/// seeded with a global `config.toml`), no credentials in the environment.
fn resolve_without_credentials(
    provider: &str,
    base_url: Option<&str>,
    global_config: Option<&str>,
) -> anyhow::Result<Config> {
    let _env = NoCredentialEnv::enter();
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    if let Some(body) = global_config {
        std::fs::write(home.path().join("config.toml"), body).expect("write global config");
    }
    let _home_guard = HomeGuard::enter(home.path());
    Config::resolve(&args_without_key(provider, base_url, &project))
}

/// Assert a resolution failed BECAUSE the credential was missing — not because
/// something else went wrong on the way. An assertion that merely reads "it
/// errored" would pass on a build that refused every configuration.
fn assert_refused_for_missing_key(result: anyhow::Result<Config>, case: &str) {
    let err = match result {
        Ok(cfg) => panic!(
            "{case}: resolution must still refuse; it succeeded with \
             model={} base_url={} api_key_empty={}",
            cfg.model,
            cfg.base_url,
            cfg.api_key.is_empty()
        ),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(
        msg.contains("No API key found"),
        "{case}: the refusal must name the missing credential, got: {msg}"
    );
}

/// THE DEFECT. A keyless local endpoint the user pointed us at must start.
#[test]
#[serial]
fn keyless_self_hosted_endpoint_resolves_without_any_credential() {
    let resolved = resolve_without_credentials("openai", Some(LOCAL_ENDPOINT), None);

    let cfg = resolved.unwrap_or_else(|e| {
        panic!(
            "a self-hosted endpoint the user explicitly pointed us at needs no \
             remote credential -- the OpenAI wire sends a placeholder bearer that \
             local inference servers ignore. Resolution still failed: {e:#}"
        )
    });
    assert_eq!(
        cfg.base_url, LOCAL_ENDPOINT,
        "the declared endpoint must survive resolution"
    );
    assert!(
        cfg.api_key.is_empty(),
        "a keyless local endpoint must not acquire a credential here; the \
         provider decides what (if anything) goes on the wire. Got a non-empty key"
    );
    assert!(
        cfg.compat.keyless_self_hosted(),
        "the exemption must be the provider's declared capability, not an \
         address-only inference"
    );
}

/// The declaration may equally live in config — `[providers.openai] base_url`
/// is the same statement as `--base-url`, and a profile folds into it too.
#[test]
#[serial]
fn a_config_declared_local_endpoint_also_resolves_without_a_credential() {
    let resolved = resolve_without_credentials(
        "openai",
        None,
        Some("[providers.openai]\nbase_url = \"http://127.0.0.1:11434\"\n"),
    );

    let cfg = resolved.unwrap_or_else(|e| {
        panic!("a config-declared self-hosted endpoint must resolve too: {e:#}")
    });
    assert_eq!(cfg.base_url, "http://127.0.0.1:11434");
    assert!(
        cfg.api_key.is_empty(),
        "a keyless local endpoint must not acquire a credential here"
    );
}

/// NEGATIVE CONTROL — the security boundary. A public host with no credential
/// anywhere must still be refused at startup, so no placeholder or empty
/// credential is ever sent to a remote endpoint.
#[test]
#[serial]
fn remote_endpoint_without_credential_still_refuses() {
    let resolved = resolve_without_credentials("openai", Some("https://api.openai.com/v1"), None);
    assert_refused_for_missing_key(resolved, "explicit remote endpoint");
}

/// NEGATIVE CONTROL — only an endpoint the USER declared can qualify. A
/// provider's own default endpoint is untouched.
#[test]
#[serial]
fn default_endpoint_without_credential_still_refuses() {
    let resolved = resolve_without_credentials("openai", None, None);
    assert_refused_for_missing_key(resolved, "provider default endpoint");
}

/// NEGATIVE CONTROL — locality is not sufficient on its own. Anthropic's wire
/// has no keyless path, so pointing it at loopback must keep the clear startup
/// refusal instead of deferring to a 401 several seconds into the first turn.
#[test]
#[serial]
fn a_provider_without_a_keyless_wire_still_requires_a_key() {
    let resolved = resolve_without_credentials("anthropic", Some("http://127.0.0.1:8080"), None);
    assert_refused_for_missing_key(resolved, "loopback endpoint on a keyless-less wire");
}

/// NEGATIVE CONTROL — the operator's explicit opt-out wins. Without this the
/// exemption would be unconditional for the whole OpenAI-wire family.
#[test]
#[serial]
fn an_explicit_compat_optout_restores_the_key_requirement() {
    let resolved = resolve_without_credentials(
        "openai",
        Some(LOCAL_ENDPOINT),
        Some("[providers.openai.compat]\nkeyless_self_hosted = false\n"),
    );
    assert_refused_for_missing_key(resolved, "explicit `keyless_self_hosted = false`");
}

/// The address predicate must not be talked into calling a public host local.
/// `is_self_hosted_base_url` is what stands between the exemption and the open
/// internet, so its polarity is pinned here as well as in its own crate.
#[test]
fn the_locality_predicate_rejects_public_hosts() {
    use wcore_config::self_hosted::is_self_hosted_base_url;

    for url in [
        "http://127.0.0.1:11434/v1",
        "http://localhost:11434/v1",
        "http://[::1]:8080/v1",
        "http://192.168.1.9:8000/v1",
        "http://host.docker.internal:11434/v1",
    ] {
        assert!(is_self_hosted_base_url(url), "expected self-hosted: {url}");
    }
    for url in [
        "https://api.openai.com/v1",
        "https://api.anthropic.com",
        // A public host whose NAME merely mentions loopback.
        "https://127.0.0.1.attacker.example/v1",
        "https://localhost.attacker.example/v1",
        "https://8.8.8.8/v1",
        // #1211 — the `@` is in the QUERY, not the authority. Reading the
        // authority as "everything before the first '/'" and then taking the
        // last `@`-separated part makes both of these read as loopback.
        PUBLIC_HOST_WITH_USERINFO_QUERY,
        "https://h?a=@10.0.0.1",
        // The same confusion one delimiter over: for a special scheme the URL
        // grammar ends the authority at '#' too, so this is a fragment.
        "https://api.openai.com#@127.0.0.1",
        // And why cutting at the first of '/', '?', '#' is not enough on its
        // own: a special scheme treats '\\' as a path separator, so this is a
        // request to api.openai.com with the path `/@127.0.0.1/v1`.
        "https://api.openai.com\\@127.0.0.1/v1",
        // Real userinfo, pointed the other way: the HOST is public and the
        // loopback literal is the user name.
        "https://127.0.0.1@api.openai.com/v1",
    ] {
        assert!(!is_self_hosted_base_url(url), "expected public: {url}");
    }
}

/// #1212 — THE SECOND CREDENTIAL GATE. `resolve_council_provider` runs its own
/// credential chain for council members. It reached the opposite verdict to
/// `Config::resolve` on identical config: a keyless self-hosted member was
/// classified `CouncilProviderError::Keyless` and dropped before spawn, so a
/// user who pointed a council member at a local Ollama silently lost it.
#[test]
#[serial]
fn a_keyless_self_hosted_council_member_is_not_skipped() {
    let _env = NoCredentialEnv::enter();
    let home = TempDir::new().unwrap();
    let _home_guard = HomeGuard::enter(home.path());

    let mut providers = HashMap::new();
    providers.insert(
        "openai".to_string(),
        ProviderConfig {
            base_url: Some(LOCAL_ENDPOINT.to_string()),
            ..Default::default()
        },
    );

    let (cfg, _model) = resolve_council_provider(&providers, &Config::default(), "openai")
        .unwrap_or_else(|e| {
            panic!(
                "a council member at a keyless self-hosted endpoint must be built, \
                 not skipped -- the same config starts fine on the main CLI path. \
                 Got: {e:?}"
            )
        });

    assert_eq!(
        cfg.base_url, LOCAL_ENDPOINT,
        "the declared endpoint must survive council resolution"
    );
    assert!(
        cfg.api_key.is_empty(),
        "a keyless local member must not acquire a credential here; the provider \
         decides what (if anything) goes on the wire. Got a non-empty key"
    );
}

/// NEGATIVE CONTROL for the council gate — the exemption must not have been
/// traded for "the council stopped checking credentials". A public member with
/// no key anywhere is still `Keyless`, and so is the #1211 spelling of a public
/// host dressed up to look like loopback.
#[test]
#[serial]
fn a_keyless_public_council_member_is_still_skipped() {
    let _env = NoCredentialEnv::enter();
    let home = TempDir::new().unwrap();
    let _home_guard = HomeGuard::enter(home.path());

    for endpoint in ["https://api.openai.com", PUBLIC_HOST_WITH_USERINFO_QUERY] {
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                base_url: Some(endpoint.to_string()),
                ..Default::default()
            },
        );
        let err = resolve_council_provider(&providers, &Config::default(), "openai")
            .err()
            .unwrap_or_else(|| {
                panic!("{endpoint}: a public council member with no credential must be skipped")
            });
        assert!(
            matches!(err, CouncilProviderError::Keyless(_)),
            "{endpoint}: expected Keyless, got {err:?}"
        );
    }
}

/// #1212 c2 — the two gates must AGREE. This drives both credential gates over
/// the same endpoints with the same declaration and asserts one verdict, so a
/// future edit that fixes one gate and not the other is red here rather than
/// discovered by a user whose council member vanished.
#[test]
#[serial]
fn both_credential_gates_agree_on_identical_config() {
    for (endpoint, exempt) in [
        ("http://127.0.0.1:11434", true),
        ("https://api.openai.com", false),
        (PUBLIC_HOST_WITH_USERINFO_QUERY, false),
    ] {
        // Gate 1 — the startup path, declaring the endpoint in config.
        let startup = resolve_without_credentials(
            "openai",
            None,
            Some(&format!("[providers.openai]\nbase_url = \"{endpoint}\"\n")),
        )
        .is_ok();

        // Gate 2 — the council path, the same declaration through the same
        // `[providers.openai]` shape.
        let council = {
            let _env = NoCredentialEnv::enter();
            let home = TempDir::new().unwrap();
            let _home_guard = HomeGuard::enter(home.path());
            let mut providers = HashMap::new();
            providers.insert(
                "openai".to_string(),
                ProviderConfig {
                    base_url: Some(endpoint.to_string()),
                    ..Default::default()
                },
            );
            resolve_council_provider(&providers, &Config::default(), "openai").is_ok()
        };

        assert_eq!(
            startup, council,
            "{endpoint}: the startup gate and the council gate must reach the same \
             verdict on identical config (startup exempt={startup}, council exempt={council})"
        );
        assert_eq!(
            startup, exempt,
            "{endpoint}: expected exempt={exempt}, both gates said {startup}"
        );
    }
}
