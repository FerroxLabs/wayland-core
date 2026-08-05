//! Regression guard for ledger row `27-C2(a)` — "the product's own remediation
//! text sends the operator in a circle".
//!
//! The browser tool, when it denies solely because of its fail-closed default,
//! prints a `config.toml` snippet. That snippet used to name `[browser]` while
//! the loader reads `browser.policy.*`. Because `BrowserConfig` and
//! `BrowserPolicyConfig` are `#[serde(default)]` and neither denies unknown
//! fields, the misplaced key parsed cleanly and was **silently discarded** — an
//! operator following the product's own instructions got no error and a
//! permanently disabled tool.
//!
//! **What makes this a guard rather than a string assertion.** Nothing here
//! hardcodes the expected section name. The test takes the exact TOML the hint
//! emits (`wcore_browser::config_hint::*`, the same constants
//! `disabled_by_default_hint()` interpolates) and drives it through the real
//! production chain:
//!
//! 1. `toml::from_str::<ConfigFile>` — the real serde types the config loader
//!    deserialises `config.toml` into. `Config::resolve` assigns
//!    `browser: merged.browser` verbatim, so `ConfigFile.browser` IS the
//!    runtime value.
//! 2. `browser_adapter::apply_config_policy` — the real copy `AgentBootstrap`
//!    performs onto every captured `BrowserToolSpec`.
//! 3. `browser_adapter::spec_to_core` — the real mirror→core conversion,
//!    including the `"allow"`/`"ask"`/else string match.
//! 4. `BrowserPolicy::check_url` — the real gate the tool consults.
//!
//! If the hint ever again advertises a path the loader does not read, step 1
//! yields an empty policy and step 4 denies, so this file goes red. The
//! negative control below pins the failure mode itself: a bare `[browser]`
//! section parses successfully and produces a policy that denies.

use wcore_agent::plugins::adapters::browser_adapter::{apply_config_policy, spec_to_core};
use wcore_browser::config_hint::{
    ALLOWLIST_ADMITTED_URL, ALLOWLIST_REFUSED_URL, DEFAULT_ACTION_ADMITTED_URL,
    ENABLE_BY_ALLOWLIST_TOML, ENABLE_BY_DEFAULT_ACTION_TOML, disabled_by_default_hint,
};
use wcore_browser::policy::BrowserPolicy;
use wcore_config::config::ConfigFile;
use wcore_plugin_api::browser_spec::{
    BrowserPolicySpec, BrowserProviderHint, BrowserToolSpec as MirrorSpec,
};

/// Run `toml_src` through every production hop between `config.toml` and the
/// live `BrowserPolicy` the tool gates on.
fn policy_from_config_toml(toml_src: &str) -> BrowserPolicy {
    let file: ConfigFile = toml::from_str(toml_src)
        .unwrap_or_else(|e| panic!("hint TOML must parse as a real config file: {e}\n{toml_src}"));

    // What the plugin shell registers before the operator's config is applied:
    // a deny-all default. Anything the snippet fails to reach leaves this as-is.
    let mut specs = vec![MirrorSpec {
        tool_namespace: "Browser".into(),
        preferred_provider: BrowserProviderHint::Auto,
        policy: BrowserPolicySpec::default(),
        allow_cloud: false,
    }];

    apply_config_policy(&file.browser.policy, &mut specs);
    spec_to_core(&specs[0]).policy
}

#[test]
fn allowlist_snippet_actually_enables_the_tool() {
    let policy = policy_from_config_toml(ENABLE_BY_ALLOWLIST_TOML);

    policy
        .check_url(ALLOWLIST_ADMITTED_URL)
        .unwrap_or_else(|e| {
            panic!(
                "the remediation snippet the product prints does NOT enable the browser tool.\n\
             {ALLOWLIST_ADMITTED_URL} was refused: {e}\n\
             snippet:\n{ENABLE_BY_ALLOWLIST_TOML}"
            )
        });

    // An allow-list is an allow-list: the snippet must not accidentally open
    // everything (which would be a worse defect than the one it fixes).
    assert!(
        policy.check_url(ALLOWLIST_REFUSED_URL).is_err(),
        "allow-list snippet admitted an origin it never listed: {ALLOWLIST_REFUSED_URL}"
    );
}

#[test]
fn default_action_snippet_actually_enables_the_tool() {
    let policy = policy_from_config_toml(ENABLE_BY_DEFAULT_ACTION_TOML);

    policy
        .check_url(DEFAULT_ACTION_ADMITTED_URL)
        .unwrap_or_else(|e| {
            panic!(
                "the `default_action` alternative the product prints does NOT enable the \
                 browser tool.\n{DEFAULT_ACTION_ADMITTED_URL} was refused: {e}\n\
                 snippet:\n{ENABLE_BY_DEFAULT_ACTION_TOML}"
            )
        });
}

#[test]
fn the_text_the_operator_reads_contains_the_snippets_that_were_proven() {
    // The two tests above prove the CONSTANTS work. This links the constants
    // to the message a human actually sees, so proving one proves the other.
    let hint = disabled_by_default_hint();
    assert!(
        hint.contains(ENABLE_BY_ALLOWLIST_TOML),
        "the proven allow-list snippet is not in the printed hint:\n{hint}"
    );
    assert!(
        hint.contains(ENABLE_BY_DEFAULT_ACTION_TOML),
        "the proven default_action snippet is not in the printed hint:\n{hint}"
    );
}

/// Negative control — this is the defect, pinned.
///
/// The pre-fix hint's section header parses **without error** and yields a
/// policy that denies. That silence is the whole reason `27-C2(a)` cost a user
/// their afternoon rather than producing a config error, and it is why the
/// tests above assert an `Allow` decision instead of comparing strings: a
/// string comparison would have passed against this input too.
#[test]
fn the_wrong_section_parses_silently_and_leaves_the_tool_disabled() {
    const WRONG: &str = "\
[browser]
allowed_origins = [\"example.com\", \"*.mysite.com\"]
default_action = \"allow\"
";

    let file: ConfigFile =
        toml::from_str(WRONG).expect("a key at the wrong level is silently accepted, not rejected");
    assert!(
        file.browser.policy.allowed_origins.is_empty(),
        "loader unexpectedly read [browser].allowed_origins — if this now works, \
         the hint may name either section and this guard needs rewriting"
    );

    let policy = policy_from_config_toml(WRONG);
    assert!(
        policy.check_url(ALLOWLIST_ADMITTED_URL).is_err(),
        "the [browser] section is supposed to be a no-op; if it enables the tool, \
         the 27-C2(a) defect no longer exists and this control is stale"
    );
}
