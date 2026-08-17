//! gh#900 — "Browser tool ignores edited browser policy config".
//!
//! The reporter edited a `config.toml` in four different throwaway
//! `wcore-temp-*` workspaces, confirmed `[browser]`/`default_action = "allow"`
//! was present each time, and the Browser tool kept returning the same
//! fail-closed denial. They concluded the browser policy had its own broken
//! config-resolution path.
//!
//! It does not. Two separate pieces of the product's own remediation text sent
//! them there:
//!
//! 1. The SECTION. Fixed under ledger row `27-C2(a)`; guarded by
//!    `browser_config_hint_roundtrip.rs`.
//! 2. The PATH — still open when this file was written. The message said
//!    "Add allowed domains to your config.toml" and named no location. In a
//!    desktop session the working directory is a throwaway `wcore-temp-*`
//!    workspace, so an operator following that sentence literally creates
//!    `<cwd>/config.toml`. That file is not a config source in ANY layer: the
//!    workspace layer is `.wayland-core.toml` (or `.wayland-core/config.toml`),
//!    and a plain `config.toml` is read only under the app config dir.
//!    The edit lands somewhere the loader never looks, no diagnostic is
//!    emitted, and the tool stays disabled — exactly the reported loop.
//!
//! **What makes this a guard rather than a string assertion.** The test never
//! writes to a path it computed itself. It reads the path back OUT of the
//! message an operator sees, writes the snippet THERE, and then drives the real
//! resolver (`Config::resolve_with_provenance` over a global file and an
//! unrelated project workspace) and the real bootstrap policy copy through to
//! `BrowserPolicy::check_url`. If the message ever again names a path the
//! loader does not read, the write lands nowhere and the decision is Deny.
//!
//! [`control_writing_config_toml_into_the_workspace_leaves_the_tool_disabled`]
//! is the negative control: it performs the SAME journey with the file placed
//! where the pre-fix wording led, and requires a Deny. Without it, a fix that
//! defaulted the browser policy to allow-all would pass everything above while
//! destroying the fail-closed posture.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serial_test::serial;
use wcore_agent::plugins::adapters::browser_adapter::{apply_config_policy, spec_to_core};
use wcore_browser::config_hint::{
    ALLOWLIST_ADMITTED_URL, ENABLE_BY_ALLOWLIST_TOML, PROJECT_CONFIG_FILENAME,
    disabled_by_default_hint,
};
use wcore_browser::policy::BrowserPolicy;
use wcore_config::config::{CliArgs, Config};
use wcore_config::workspace_trust::WorkspaceTrustStore;
use wcore_plugin_api::browser_spec::{
    BrowserPolicySpec, BrowserProviderHint, BrowserToolSpec as MirrorSpec,
};

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
            // and restores the environment through this guard.
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

/// Pull the config path back out of the text the operator reads.
///
/// Deliberately naive — an operator scanning the message for "the file to
/// edit" does the same thing. A message that names no path at all yields
/// `None`, which is itself the gh#900 defect.
///
/// Known limit: whitespace-delimited, so it would truncate a config path
/// containing a space. Every platform's temp root is space-free
/// (`/tmp`, `/var/folders/**/T`, `…\AppData\Local\Temp`) and `WAYLAND_HOME`
/// is pinned to a temp dir here, so it cannot bite in this test. If it ever
/// does, the failure is the loud symptom panic below quoting the truncated
/// path, not a silent pass.
fn config_path_named_in(hint: &str) -> Option<PathBuf> {
    hint.split_whitespace()
        .map(|token| token.trim_matches('`'))
        .find(|token| {
            token.ends_with("config.toml")
                && Path::new(token)
                    .parent()
                    .is_some_and(|parent| parent != Path::new(""))
        })
        .map(PathBuf::from)
}

/// Sentinel proving the resolver really read OUR temp global file. hetzner and
/// CI hosts can carry a real `~/.wayland`; without this a passing decision
/// could come from some other machine-wide config entirely.
const GLOBAL_SENTINEL_MAX_TOKENS: u32 = 4321;

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
    // config is applied: deny-all. Anything the operator's edit fails to reach
    // leaves this untouched.
    let mut specs = vec![MirrorSpec {
        tool_namespace: "Browser".into(),
        preferred_provider: BrowserProviderHint::Auto,
        policy: BrowserPolicySpec::default(),
        allow_cloud: false,
    }];
    apply_config_policy(&resolved.value.browser.policy, &mut specs);
    spec_to_core(&specs[0]).policy
}

/// `(temp home, temp workspace, env guard)` — the reporter's topology: a
/// long-lived config home plus a per-chat throwaway working directory. The
/// workspace starts UNTRUSTED, which is the state of every freshly created
/// `wcore-temp-*` directory.
fn desktop_session() -> (tempfile::TempDir, tempfile::TempDir, EnvGuard) {
    let home = tempfile::tempdir().expect("temp home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let guard = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        ("WAYLAND_CONFIG_PATH", None),
        ("XDG_DATA_HOME", None),
    ]);
    (home, workspace, guard)
}

/// Grant the workspace fingerprint. Must be called AFTER the project file is
/// written: `fingerprint_workspace` hashes the CONTENT of `.wayland-core.toml`
/// into the digest and `WorkspaceTrustStore::resolve` re-derives it on every
/// resolve, so granting first and writing after silently revokes the grant.
/// (Measured: doing it in the other order made the trusted arm below fail with
/// a Deny, which reads exactly like a product defect and is not one.)
fn grant_trust(workspace: &Path) {
    // The trust store lives under WAYLAND_HOME, so the env guard must already
    // be installed when this runs.
    WorkspaceTrustStore::for_current_home()
        .grant(workspace)
        .expect("granting workspace trust");
}

#[test]
#[serial]
fn following_the_denial_message_from_a_temp_workspace_enables_the_tool() {
    let (home, workspace, _env) = desktop_session();

    // The operator hits the denial and reads it.
    let hint = disabled_by_default_hint();
    let named = config_path_named_in(&hint).unwrap_or_else(|| {
        panic!(
            "the denial message names no config file location. An operator in a throwaway \
             workspace has nowhere to put the snippet, which is gh#900:\n{hint}"
        )
    });

    // The operator already has a config; the sentinel stands in for its
    // existing contents. It is written to the file the RESOLVER reads, not to
    // the file the MESSAGE names, so the liveness check below stays honest even
    // when those two disagree — which is the whole failure mode under test.
    std::fs::write(
        home.path().join("config.toml"),
        format!("[default]\nmax_tokens = {GLOBAL_SENTINEL_MAX_TOKENS}\n"),
    )
    .unwrap();

    // They append the printed snippet to exactly the file the message named.
    // Nothing here is a path the test chose.
    std::fs::create_dir_all(named.parent().expect("named path has a parent")).unwrap();
    let mut existing = std::fs::read_to_string(&named).unwrap_or_default();
    existing.push('\n');
    existing.push_str(ENABLE_BY_ALLOWLIST_TOML);
    std::fs::write(&named, existing).unwrap();

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

/// Negative control — the pre-fix journey, pinned.
///
/// "your config.toml" with no location, read inside a `wcore-temp-*` workspace,
/// means `<cwd>/config.toml`. Trust is GRANTED here so the Deny below is about
/// the FILENAME alone and not about the workspace layer being restricted; the
/// trusted counterpart with the real filename is the next test.
#[test]
#[serial]
fn control_writing_config_toml_into_the_workspace_leaves_the_tool_disabled() {
    let (home, workspace, _env) = desktop_session();

    std::fs::write(
        home.path().join("config.toml"),
        format!("[default]\nmax_tokens = {GLOBAL_SENTINEL_MAX_TOKENS}\n"),
    )
    .unwrap();
    // Where the unanchored wording sent the reporter, four times.
    std::fs::write(
        workspace.path().join("config.toml"),
        ENABLE_BY_ALLOWLIST_TOML,
    )
    .unwrap();
    grant_trust(workspace.path());

    let policy = policy_after_resolving(home.path(), workspace.path());
    assert!(
        policy.check_url(ALLOWLIST_ADMITTED_URL).is_err(),
        "a bare config.toml inside the working directory is supposed to be invisible to \
         the loader. If it now enables the browser tool, gh#900's mechanism no longer \
         exists and this control is stale"
    );
}

/// Positive control for the test above: the workspace layer is not inert, it
/// just uses a different filename. Same journey, same snippet, same trust —
/// only the filename changes, and the decision flips to Allow. Without this,
/// the Deny above could be measuring "workspace configs never reach the browser
/// policy" and would pass no matter what filename the hint advertised.
#[test]
#[serial]
fn control_the_real_workspace_filename_does_reach_the_browser_policy() {
    let (home, workspace, _env) = desktop_session();

    std::fs::write(
        home.path().join("config.toml"),
        format!("[default]\nmax_tokens = {GLOBAL_SENTINEL_MAX_TOKENS}\n"),
    )
    .unwrap();
    std::fs::write(
        workspace.path().join(PROJECT_CONFIG_FILENAME),
        ENABLE_BY_ALLOWLIST_TOML,
    )
    .unwrap();
    grant_trust(workspace.path());

    let policy = policy_after_resolving(home.path(), workspace.path());
    policy
        .check_url(ALLOWLIST_ADMITTED_URL)
        .unwrap_or_else(|e| {
            panic!(
                "`{PROJECT_CONFIG_FILENAME}` in a TRUSTED working directory did not reach the \
             browser policy — the workspace layer is inert for browser config, which is a \
             bigger defect than gh#900: {e}"
            )
        });
}

/// Why the hint must name the GLOBAL file rather than a workspace-local one.
///
/// A freshly created `wcore-temp-*` workspace is untrusted, and
/// `restrict_untrusted_project_config` drops the project `[browser]` block
/// entirely — browser access is authority-granting, so a directory that
/// travels with a clone may not mint it. For the reporter's topology there is
/// therefore exactly ONE file that can enable the tool, and a message that does
/// not name it cannot be followed.
#[test]
#[serial]
fn an_untrusted_workspace_cannot_enable_the_browser_even_with_the_right_filename() {
    let (home, workspace, _env) = desktop_session();

    std::fs::write(
        home.path().join("config.toml"),
        format!("[default]\nmax_tokens = {GLOBAL_SENTINEL_MAX_TOKENS}\n"),
    )
    .unwrap();
    std::fs::write(
        workspace.path().join(PROJECT_CONFIG_FILENAME),
        ENABLE_BY_ALLOWLIST_TOML,
    )
    .unwrap();

    let policy = policy_after_resolving(home.path(), workspace.path());
    assert!(
        policy.check_url(ALLOWLIST_ADMITTED_URL).is_err(),
        "an UNTRUSTED workspace granted itself browser access; that is a privilege \
         escalation, not a fix for gh#900"
    );
}
