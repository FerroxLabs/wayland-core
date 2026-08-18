//! `[browser]` merge — trust polarity and cross-field clobbering.
//!
//! Two separate properties, both about the same block, both driven through the
//! **real** load path (`Config::resolve_with_provenance` over a global
//! `config.toml` and a project `.wayland-core.toml` on disk) rather than by
//! calling the merge or the trust filter with a hand-built struct. A fixture
//! that cannot carry the defect proves nothing, so every arm here writes the
//! attacker-shaped TOML a cloned repository would actually ship.
//!
//! ## 1. Enabling a download must not drop the operator's policy
//!
//! `[browser]` used to merge as one all-or-nothing block, and
//! `[browser.camoufox_download]` — a network-fetch-and-execute surface — was
//! added as a trigger for that choice. A project configuring only a download
//! therefore replaced the operator's `[browser.policy]` with
//! `BrowserPolicyConfig::default()`, silently deleting the origin allowlist
//! that bounds where the browser may go. The merge is now field-wise.
//!
//! ## 2. An untrusted project cannot reach `[browser]` at all
//!
//! `restrict_untrusted_project_config` is an ALLOWLIST — it starts from
//! `ConfigFile::default()` and forwards a named handful of narrowing fields —
//! so `[browser]` from an untrusted workspace is dropped whole. That is a load-
//! bearing property of the download surface (an untrusted repo naming its own
//! URL and its own digest would otherwise be fetched, verified against the
//! attacker's digest, chmod'd and spawned), and nothing in the function names
//! `browser`, so nothing in the function would break if a future edit started
//! forwarding it. These arms pin the property from the outside.
//!
//! Both untrusted arms are paired with a TRUSTED control using the identical
//! project body. Without the control, "the untrusted config did not take
//! effect" is satisfiable by a fixture that never took effect anywhere.

use std::ffi::{OsStr, OsString};

use serial_test::serial;
use wcore_config::browser::{CamoufoxDownloadConfig, platform_key};
use wcore_config::config::{CliArgs, Config};
use wcore_config::resolution_provenance::{ConfigSourceDisposition, ConfigSourceRole};
use wcore_config::workspace_trust::WorkspaceTrustStore;

/// Written into every temp GLOBAL config and read back out of the merged
/// result. This host injects provider credentials into a process regardless of
/// the shell environment, and a stale real `~/.wayland` would silently supply a
/// different `[browser]` block — so each arm proves it read *its own* global
/// file before it trusts anything else the merge produced.
const GLOBAL_SENTINEL_MAX_TOKENS: u32 = 4321;

/// The operator's allowlisted origin. Its survival (or disappearance) is the
/// symptom under test in the clobbering arms.
const OPERATOR_ORIGIN: &str = "https://ops.example.com";

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
            // SAFETY: every test in this binary that mutates the environment is
            // in the same `serial` group and restores through this guard.
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

/// Whether the workspace fingerprint is granted before the config is resolved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Trust {
    Granted,
    /// The DEFAULT state of any freshly cloned or freshly created project.
    Untrusted,
}

/// Drive the real load + merge and hand back the resolved config.
fn load(global_browser: &str, project_body: &str, trust: Trust) -> Config {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    std::fs::write(
        home.path().join("config.toml"),
        format!("[default]\nmax_tokens = {GLOBAL_SENTINEL_MAX_TOKENS}\n\n{global_browser}"),
    )
    .unwrap();
    std::fs::write(project.path().join(".wayland-core.toml"), project_body).unwrap();

    let _env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        ("WAYLAND_CONFIG_PATH", None),
        ("XDG_DATA_HOME", None),
    ]);

    if trust == Trust::Granted {
        // The trust store lives under WAYLAND_HOME, so the grant must happen
        // with the guard already installed.
        WorkspaceTrustStore::for_current_home()
            .grant(project.path())
            .expect("granting workspace trust");
    }

    let cli = CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("test-key-not-a-real-credential".to_string()),
        project_dir: Some(project.path().to_path_buf()),
        ..CliArgs::default()
    };
    let resolved = Config::resolve_with_provenance(&cli).expect("resolving config");

    let project_sources: Vec<_> = resolved
        .provenance
        .sources
        .iter()
        .filter(|source| source.role == ConfigSourceRole::Project)
        .collect();
    assert!(
        !project_sources.is_empty(),
        "a Project config source must appear in the provenance; the project \
         .wayland-core.toml was never read and every arm here is vacuous"
    );
    let project_restricted = project_sources.iter().any(|source| {
        source
            .dispositions
            .contains(&ConfigSourceDisposition::Restricted)
    });

    // Instrument-alive check: the merge really read the temp global file.
    assert_eq!(
        resolved.value.max_tokens, GLOBAL_SENTINEL_MAX_TOKENS,
        "the merged config did not carry the temp global config's sentinel \
         max_tokens — this test read some OTHER global config, so nothing it \
         measures about [browser] can be trusted"
    );
    // And the trust arm actually under test was the one exercised.
    assert_eq!(
        project_restricted,
        trust == Trust::Untrusted,
        "workspace trust arm mismatch: wanted {trust:?}, but the resolver \
         reported project_restricted={project_restricted}"
    );

    resolved.value
}

/// An operator global that allows exactly one origin. Anything that replaces
/// this block with `BrowserPolicyConfig::default()` produces a deny-all with an
/// empty allowlist, which is a visibly different value.
fn operator_global_policy() -> String {
    format!(
        "[browser.policy]\ndefault_action = \"allow\"\nallowed_origins = [\"{OPERATOR_ORIGIN}\"]\n"
    )
}

/// A project block that configures ONLY a Camoufox download — it says nothing
/// whatsoever about `[browser.policy]`.
fn download_only_project_body() -> String {
    let key = platform_key();
    format!(
        "[browser.camoufox_download]\nenabled = true\n\n\
         [browser.camoufox_download.artifacts.\"{key}\"]\n\
         url = \"https://repo-supplied.example.com/camoufox.tar.gz\"\n\
         sha256 = \"{}\"\n\
         archive_exe_path = \"camoufox/camoufox\"\n",
        "a".repeat(64)
    )
}

/// An operator global that allows one origin AND holds a loopback grant.
fn operator_global_policy_with_loopback() -> String {
    format!(
        "[browser.policy]\ndefault_action = \"allow\"\nallowed_origins = [\"{OPERATOR_ORIGIN}\"]\n\n\
         [browser.policy.loopback]\nenabled = true\nschema_version = 1\n\
         session_scope = \"operator-scope\"\nports = [3000]\n"
    )
}

/// A project block that configures ONLY a loopback grant — it says nothing
/// about `default_action` or either origin list.
fn loopback_only_project_body() -> String {
    "[browser.policy.loopback]\nenabled = true\nschema_version = 1\n\
     session_scope = \"project-scope\"\nports = [4000]\n"
        .to_owned()
}

// ── 1. Cross-field clobbering (trusted path) ─────────────────────────────────

/// A trusted project that configures only `[browser.camoufox_download]` must
/// keep the operator's `[browser.policy]`. Both values must survive: the
/// download the project asked for AND the allowlist it never mentioned.
///
/// Red arm: with the whole-block merge, `project.browser` wins outright and the
/// operator's `allowed_origins` comes back empty with `default_action = "deny"`.
#[test]
#[serial(browser_merge_trust_env)]
fn enabling_a_download_does_not_drop_the_operator_browser_policy() {
    let config = load(
        &operator_global_policy(),
        &download_only_project_body(),
        Trust::Granted,
    );

    assert_eq!(
        config.browser.policy.allowed_origins,
        vec![OPERATOR_ORIGIN.to_string()],
        "the operator's browser origin allowlist was dropped by a project \
         config that configured only [browser.camoufox_download] and never \
         mentioned [browser.policy]"
    );
    assert_eq!(
        config.browser.policy.default_action, "allow",
        "the operator's browser default_action was dropped by a project config \
         that never mentioned [browser.policy]"
    );
    // The other half of field-wise: the project's own block still applies.
    assert!(
        config.browser.camoufox_download.enabled,
        "the trusted project's [browser.camoufox_download] was itself dropped; \
         this arm would then pass for the wrong reason"
    );
}

/// Opposite control — the merge must still be able to reach the project's
/// policy. Without this, a "fix" that hard-wired `global.browser` would pass
/// the arm above while deleting the trusted-project override entirely.
#[test]
#[serial(browser_merge_trust_env)]
fn a_trusted_project_can_still_replace_the_browser_policy() {
    let config = load(
        &operator_global_policy(),
        "[browser.policy]\ndefault_action = \"deny\"\n\
         denied_origins = [\"https://blocked.example.com\"]\n",
        Trust::Granted,
    );

    assert_eq!(config.browser.policy.default_action, "deny");
    assert_eq!(
        config.browser.policy.denied_origins,
        vec!["https://blocked.example.com".to_string()]
    );
    assert!(
        config.browser.policy.allowed_origins.is_empty(),
        "policy merges as a unit: a project that wrote a policy owns the whole \
         policy, so the operator's allowed_origins must not be spliced into it"
    );
}

// ── 2. Untrusted project cannot reach [browser] ──────────────────────────────

/// An untrusted workspace — the default state of any freshly cloned repo — must
/// not be able to configure the download surface. The fixture names its own URL
/// and its own digest, which is exactly the shape that would otherwise be
/// fetched, verified against the attacker's digest, chmod'd and spawned.
#[test]
#[serial(browser_merge_trust_env)]
fn an_untrusted_project_cannot_configure_a_camoufox_download() {
    let config = load(
        &operator_global_policy(),
        &download_only_project_body(),
        Trust::Untrusted,
    );

    assert_eq!(
        config.browser.camoufox_download,
        CamoufoxDownloadConfig::default(),
        "an untrusted project config reached [browser.camoufox_download]: \
         enabled={} artifacts={:?} — a cloned repository can now name the URL \
         and the digest of an executable Core will fetch and run",
        config.browser.camoufox_download.enabled,
        config
            .browser
            .camoufox_download
            .artifacts
            .keys()
            .collect::<Vec<_>>(),
    );
}

/// Same block, WIDENING direction: an untrusted project must not be able to
/// relax the operator's origin policy either.
#[test]
#[serial(browser_merge_trust_env)]
fn an_untrusted_project_cannot_widen_the_browser_policy() {
    let config = load(
        "[browser.policy]\ndefault_action = \"deny\"\n",
        "[browser.policy]\ndefault_action = \"allow\"\n\
         allowed_origins = [\"https://collector.attacker-example.com\"]\n",
        Trust::Untrusted,
    );

    assert_eq!(
        config.browser.policy.default_action, "deny",
        "an untrusted project flipped the browser policy from deny to allow"
    );
    assert!(
        config.browser.policy.allowed_origins.is_empty(),
        "an untrusted project added a browser origin to the operator's \
         allowlist: {:?}",
        config.browser.policy.allowed_origins
    );
}

/// Control for BOTH untrusted arms: the identical project bodies DO take effect
/// once the workspace is trusted. This is what makes the two arms above
/// measurements rather than a fixture that never worked.
#[test]
#[serial(browser_merge_trust_env)]
fn the_same_project_bodies_do_take_effect_when_the_workspace_is_trusted() {
    let download = load(
        &operator_global_policy(),
        &download_only_project_body(),
        Trust::Granted,
    );
    assert!(
        download.browser.camoufox_download.enabled,
        "control failed: the download fixture does not take effect even when \
         trusted, so the untrusted arm proves nothing"
    );
    assert_eq!(
        download
            .browser
            .camoufox_download
            .artifact_for_current_platform()
            .map(|artifact| artifact.url.as_str()),
        Some("https://repo-supplied.example.com/camoufox.tar.gz"),
    );

    let widened = load(
        "[browser.policy]\ndefault_action = \"deny\"\n",
        "[browser.policy]\ndefault_action = \"allow\"\n\
         allowed_origins = [\"https://collector.attacker-example.com\"]\n",
        Trust::Granted,
    );
    assert_eq!(
        widened.browser.policy.default_action, "allow",
        "control failed: the policy-widening fixture does not take effect even \
         when trusted, so the untrusted arm proves nothing"
    );
}

/// A trusted project that configures ONLY `[browser.policy.loopback]` must
/// keep that grant AND the operator's policy it never mentioned.
///
/// Red arm: with `loopback` folded into the origin-triple predicate, a project
/// that sets only a grant fails the predicate, the whole project policy is
/// discarded, and `enabled` comes back false — the capability is inert.
#[test]
#[serial(browser_merge_trust_env)]
fn a_project_that_configures_only_loopback_keeps_that_grant() {
    let config = load(
        &operator_global_policy(),
        &loopback_only_project_body(),
        Trust::Granted,
    );

    assert!(
        config.browser.policy.loopback.enabled,
        "the trusted project's [browser.policy.loopback] grant was dropped \
         because it did not also set default_action or an origin list"
    );
    assert_eq!(
        config.browser.policy.loopback.ports,
        vec![4000],
        "the grant survived as a flag but lost its port list"
    );
    // The other half: the operator's policy must not have been clobbered.
    assert_eq!(
        config.browser.policy.allowed_origins,
        vec![OPERATOR_ORIGIN.to_string()],
        "the operator's origin allowlist was dropped by a project config that \
         mentioned only [browser.policy.loopback]"
    );
}

/// The opposite direction. A trusted project that sets an origin list wins the
/// origin triple, and that must NOT take the operator's loopback grant with it.
///
/// Red arm: with the unit merge, `project.browser.policy` replaces the global
/// wholesale and `enabled` comes back false.
#[test]
#[serial(browser_merge_trust_env)]
fn a_project_policy_override_does_not_drop_the_operator_loopback_grant() {
    let config = load(
        &operator_global_policy_with_loopback(),
        "[browser.policy]\nallowed_origins = [\"https://project.example.com\"]\n",
        Trust::Granted,
    );

    assert!(
        config.browser.policy.loopback.enabled,
        "the operator's loopback grant was dropped because the project set an \
         unrelated origin allowlist"
    );
    assert_eq!(
        config.browser.policy.loopback.ports,
        vec![3000],
        "the operator's grant survived as a flag but lost its port list"
    );
    // And the project's own override still took effect, so this cannot pass
    // by hard-wiring the global.
    assert_eq!(
        config.browser.policy.allowed_origins,
        vec!["https://project.example.com".to_string()],
        "the trusted project's origin override was itself dropped; this arm \
         would then pass for the wrong reason"
    );
}

/// Trust gate. Resolving `loopback` independently must not become a route for
/// an UNTRUSTED project to grant itself local network authority.
#[test]
#[serial(browser_merge_trust_env)]
fn an_untrusted_project_cannot_enable_a_loopback_grant() {
    let config = load(
        &operator_global_policy(),
        &loopback_only_project_body(),
        Trust::Untrusted,
    );

    assert!(
        !config.browser.policy.loopback.enabled,
        "an untrusted project granted itself a loopback capability"
    );
    assert!(
        config.browser.policy.loopback.ports.is_empty(),
        "an untrusted project supplied loopback ports"
    );
}
