//! gh#900 and gh#826 — "the product's own remediation text is not followable".
//!
//! Both reports are the same failure with different endings.
//!
//! **gh#900.** The reporter edited a `config.toml` in four successive
//! throwaway `wcore-temp-*` workspaces, confirmed the browser block was
//! present each time, and the tool kept returning the same fail-closed
//! denial. They concluded the browser policy had its own broken config
//! resolver. It does not. The message said "add allowed domains to your
//! config.toml" and named no location, and in a desktop session the working
//! directory IS a throwaway workspace — so `<cwd>/config.toml` is where that
//! sentence leads. A bare `config.toml` in the working directory is not a
//! config source in any layer (the project layer is `.wayland-core.toml`; a
//! plain `config.toml` is read only under the app config dir), so the edits
//! landed where the loader never looks, silently.
//!
//! **gh#826.** A loopback denial carried no remediation at all. With nothing
//! true to say, the denial got relayed with an invented fix — "enable
//! allowLoopbackHostnames in sandbox settings" — and a user went looking for
//! a setting that has never existed in this product. The cure is not better
//! prose; it is a real setting plus a message that names it.
//!
//! **What makes these guards rather than string assertions.** No test here
//! writes to a path it computed itself. Each one reads the path back OUT of
//! the message a human sees, writes the printed snippet THERE, and then drives
//! the REAL resolver (`Config::resolve_with_provenance`) plus the real
//! bootstrap policy copy and the real mirror→core conversion through to
//! `BrowserPolicy::check_url`. If a message ever again names a location the
//! loader does not read, or a setting the gate does not honour, the write
//! lands nowhere and the decision is Deny.
//!
//! [`control_a_bare_config_toml_in_the_workspace_is_not_read`] is the negative
//! control: the SAME journey with the file placed where the pre-fix wording
//! led. Without it, a fix that defaulted the browser policy to allow-all would
//! pass everything above while destroying the fail-closed posture.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serial_test::serial;
use wcore_agent::plugins::adapters::browser_adapter::{apply_config_policy, spec_to_core};
use wcore_browser::config_hint::{
    ALLOWLIST_ADMITTED_URL, ENABLE_BY_ALLOWLIST_TOML, ENABLE_LOOPBACK_TOML, LOOPBACK_ADMITTED_URL,
    LOOPBACK_REFUSED_PORT_URL, LOOPBACK_REFUSED_PRIVATE_URL, disabled_by_default_hint,
    loopback_blocked_hint,
};
use wcore_browser::policy::BrowserPolicy;
use wcore_config::config::{CliArgs, Config};
use wcore_plugin_api::browser_spec::{
    BrowserPolicySpec, BrowserProviderHint, BrowserToolSpec as MirrorSpec,
};

// ---------------------------------------------------------------------------
// Environment scaffolding
// ---------------------------------------------------------------------------

struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn set(values: &[(&'static str, Option<&OsStr>)]) -> Self {
        let saved = values
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in values {
            // SAFETY: every test in this binary is in the same `serial` group
            // and restores the environment through this guard on drop.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            // SAFETY: see `EnvGuard::set`.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// The reporter's topology: a long-lived config home plus a per-chat throwaway
/// working directory.
fn desktop_session() -> (tempfile::TempDir, tempfile::TempDir, EnvGuard) {
    let home = tempfile::tempdir().expect("temp home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let guard = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        // Observed-but-ignored by the resolver; unset so it cannot confuse the
        // reading of this test's result either way.
        ("WAYLAND_CONFIG_PATH", None),
        ("XDG_DATA_HOME", None),
    ]);
    (home, workspace, guard)
}

/// Sentinel proving the resolver really read OUR temp global file. hetzner and
/// CI hosts can carry a real `~/.wayland`; without this, a passing decision
/// could be coming from some other machine-wide config entirely.
const GLOBAL_SENTINEL_MAX_TOKENS: u32 = 4321;

/// Pull a config file path back out of the text a human reads.
///
/// Deliberately naive — someone scanning the message for "the file to edit"
/// does the same thing. A message that names no path yields `None`, which is
/// itself the gh#900 defect.
///
/// Known limit: whitespace-delimited, so it would truncate a path containing a
/// space. `WAYLAND_HOME` is pinned to a temp dir here and every platform's temp
/// root is space-free, so it cannot bite. If it ever does, the result is the
/// loud symptom panic below quoting the truncated path, not a silent pass.
fn config_path_named_in(hint: &str) -> Option<PathBuf> {
    hint.split_whitespace()
        .map(|token| token.trim_matches(|c| c == '`' || c == ',' || c == '.'))
        .find(|token| {
            token.ends_with("config.toml")
                && Path::new(token)
                    .parent()
                    .is_some_and(|parent| parent != Path::new(""))
        })
        .map(PathBuf::from)
}

/// The journey: resolve the real config with the working directory set to a
/// throwaway workspace, then drive the merged `[browser]` block through the
/// real bootstrap copy and the real mirror→core conversion.
fn policy_after_resolving(home: &Path, workspace: &Path) -> BrowserPolicy {
    let cli = CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("test-key-not-a-real-credential".to_string()),
        project_dir: Some(workspace.to_path_buf()),
        ..CliArgs::default()
    };
    let resolved = Config::resolve_with_provenance(&cli).expect("resolving config");
    assert_eq!(
        resolved.value.max_tokens,
        GLOBAL_SENTINEL_MAX_TOKENS,
        "the resolver did not read the temp global config at {}; every other \
         measurement in this test would be about some other machine's config",
        home.display()
    );

    // What the `wayland-browser` plugin shell registers before the operator's
    // config is applied: deny-all. Anything the edit fails to reach leaves this
    // untouched.
    let mut specs = vec![MirrorSpec {
        tool_namespace: "Browser".into(),
        preferred_provider: BrowserProviderHint::Auto,
        policy: BrowserPolicySpec::default(),
        allow_cloud: false,
    }];
    apply_config_policy(&resolved.value.browser.policy, &mut specs);
    spec_to_core(&specs[0]).policy
}

/// Seed the operator's pre-existing config. Written to the file the RESOLVER
/// reads, never to the file the MESSAGE names, so the liveness assertion above
/// stays honest even when those two disagree — which is the failure under test.
fn seed_existing_global_config(home: &Path) {
    std::fs::write(
        home.path_join_config(),
        format!("[default]\nmax_tokens = {GLOBAL_SENTINEL_MAX_TOKENS}\n"),
    )
    .unwrap();
}

trait HomeConfig {
    fn path_join_config(&self) -> PathBuf;
}
impl HomeConfig for Path {
    fn path_join_config(&self) -> PathBuf {
        self.join("config.toml")
    }
}

/// Append the snippet the message printed to the file the message named.
/// Nothing here is a path the test chose.
fn follow_the_message(hint: &str, snippet: &str) -> PathBuf {
    let named = config_path_named_in(hint).unwrap_or_else(|| {
        panic!(
            "the message names no config file location. Someone in a throwaway workspace \
             has nowhere to put the snippet, which is exactly gh#900:\n{hint}"
        )
    });
    std::fs::create_dir_all(named.parent().expect("named path has a parent")).unwrap();
    let mut existing = std::fs::read_to_string(&named).unwrap_or_default();
    existing.push('\n');
    existing.push_str(snippet);
    std::fs::write(&named, existing).unwrap();
    named
}

// ---------------------------------------------------------------------------
// gh#900 — following the disabled-by-default message works
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn following_the_disabled_message_from_a_temp_workspace_enables_the_tool() {
    let (home, workspace, _env) = desktop_session();
    seed_existing_global_config(home.path());

    let hint = disabled_by_default_hint();
    let named = follow_the_message(&hint, ENABLE_BY_ALLOWLIST_TOML);

    let policy = policy_after_resolving(home.path(), workspace.path());
    policy
        .check_url(ALLOWLIST_ADMITTED_URL)
        .unwrap_or_else(|e| {
            panic!(
                "the operator followed the product's own denial message exactly — wrote its \
                 snippet to the file it named, {} — and the browser tool is STILL disabled: \
                 {ALLOWLIST_ADMITTED_URL} refused: {e}\nmessage was:\n{hint}",
                named.display()
            )
        });
}

/// Negative control — the pre-fix journey, pinned. "your config.toml" with no
/// location, read from inside a `wcore-temp-*` workspace, means
/// `<cwd>/config.toml`. That file must still be inert, or the tests above
/// would pass for a reason that has nothing to do with the message.
#[test]
#[serial]
fn control_a_bare_config_toml_in_the_workspace_is_not_read() {
    let (home, workspace, _env) = desktop_session();
    seed_existing_global_config(home.path());

    std::fs::write(
        workspace.path().join("config.toml"),
        ENABLE_BY_ALLOWLIST_TOML,
    )
    .unwrap();

    let policy = policy_after_resolving(home.path(), workspace.path());
    assert!(
        policy.check_url(ALLOWLIST_ADMITTED_URL).is_err(),
        "a bare config.toml in the working directory is now a config source. If that is \
         intended, the gh#900 remediation text may name it — but this control, and the \
         reasoning in every test above, must be rewritten first."
    );
}

// ---------------------------------------------------------------------------
// gh#826 — the loopback message names a setting that exists and works
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn following_the_loopback_message_actually_reaches_loopback() {
    let (home, workspace, _env) = desktop_session();
    seed_existing_global_config(home.path());

    // The message a user sees when they try to open something on localhost.
    let hint = loopback_blocked_hint("loopback hostname blocked: localhost");
    let named = follow_the_message(&hint, ENABLE_LOOPBACK_TOML);

    let policy = policy_after_resolving(home.path(), workspace.path());
    policy.check_url(LOOPBACK_ADMITTED_URL).unwrap_or_else(|e| {
        panic!(
            "the loopback denial named a setting and a file — {} — and following it verbatim \
             STILL cannot reach {LOOPBACK_ADMITTED_URL}: {e}\n\
             This is gh#826: the product describing a control that does not deliver.\n\
             message was:\n{hint}",
            named.display()
        )
    });

    // The grant must not have widened anything else on the way through. These
    // two are the promises the same message makes in prose.
    assert!(
        policy.check_url(LOOPBACK_REFUSED_PORT_URL).is_err(),
        "the loopback grant opened an ungranted port ({LOOPBACK_REFUSED_PORT_URL}); the \
         message promises only the listed ports become reachable"
    );
    assert!(
        policy.check_url(LOOPBACK_REFUSED_PRIVATE_URL).is_err(),
        "the loopback grant reached a private RFC 1918 address \
         ({LOOPBACK_REFUSED_PRIVATE_URL}); the message promises those stay blocked"
    );
}

/// gh#826's specific ending: the invented setting. Nothing the product prints
/// may name it, and — more usefully — the settings the product DOES print must
/// be reachable in the config surface, which the test above proves by driving
/// them. This one pins the fabrication itself so it cannot come back as text.
#[test]
#[serial]
fn no_message_names_the_setting_that_never_existed() {
    let messages = [
        disabled_by_default_hint(),
        loopback_blocked_hint("loopback hostname blocked: localhost"),
        loopback_blocked_hint("loopback IP blocked: 127.0.0.1"),
    ];
    for hint in messages {
        let lowered = hint.to_lowercase();
        for invented in ["allowloopbackhostnames", "sandbox settings"] {
            assert!(
                !lowered.contains(invented),
                "a remediation message names {invented:?}, which is not a setting this \
                 product has. That fabrication is gh#826:\n{hint}"
            );
        }
    }
}

/// The loopback message must carry the SPECIFIC gate that refused. "Loopback
/// is blocked" alone is what left the reporter guessing; "port 9377 is not in
/// the granted ports [3000]" is actionable.
#[test]
#[serial]
fn the_loopback_message_keeps_the_reason_the_gate_gave() {
    let (home, workspace, _env) = desktop_session();
    seed_existing_global_config(home.path());
    let hint = loopback_blocked_hint("loopback hostname blocked: localhost");
    follow_the_message(&hint, ENABLE_LOOPBACK_TOML);

    let policy = policy_after_resolving(home.path(), workspace.path());
    let err = policy
        .check_url(LOOPBACK_REFUSED_PORT_URL)
        .expect_err("ungranted port must be refused");
    let reason = err.to_string();
    assert!(
        reason.contains("9377") && reason.contains("3000"),
        "the refusal names neither the port asked for nor the ports granted, so a reader \
         cannot tell what to change: {reason}"
    );
}
