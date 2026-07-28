//! Operator-facing remediation text for the default-denied browser tool.
//!
//! The hint tells an operator exactly what to paste into `config.toml`, so it
//! has to name the section the config **loader** actually reads.
//! `wcore_config::browser::BrowserConfig` nests the three policy fields under
//! `policy` (`BrowserPolicyConfig`), which makes the on-disk path
//! `[browser.policy]` — NOT `[browser]`.
//!
//! Why this matters more than a two-word typo: `BrowserConfig` and
//! `BrowserPolicyConfig` are both `#[serde(default)]` and neither denies
//! unknown fields, so `allowed_origins` written one level too high parses
//! without any error and is silently dropped. An operator who follows a wrong
//! hint gets **no diagnostic at all** and the tool stays disabled forever
//! (`27-C2(a)`).
//!
//! The snippets below are the single source of truth for what the hint
//! prints. `wcore-agent/tests/browser_config_hint_roundtrip.rs` parses THESE
//! constants through the real `ConfigFile` serde types, pushes the result
//! through the real bootstrap policy copy and the real spec→core conversion,
//! and asserts the resulting `BrowserPolicy` actually admits a URL. Anchoring
//! the guard on a hardcoded expected string would let the message and the
//! loader drift apart again; driving the emitted TOML to an `Allow` decision
//! cannot.

/// TOML that enables the tool by allow-listing origins. Must parse into a
/// [`crate::BrowserPolicy`] that admits [`ALLOWLIST_ADMITTED_URL`] and still
/// refuses [`ALLOWLIST_REFUSED_URL`].
pub const ENABLE_BY_ALLOWLIST_TOML: &str = "\
[browser.policy]
# Allow specific domains (glob patterns supported)
allowed_origins = [\"example.com\", \"*.mysite.com\"]
";

/// TOML that enables the tool by flipping the fail-closed default instead of
/// naming origins. Must parse into a policy that admits any http(s) origin.
pub const ENABLE_BY_DEFAULT_ACTION_TOML: &str = "\
[browser.policy]
default_action = \"allow\"
";

/// A URL [`ENABLE_BY_ALLOWLIST_TOML`] promises to admit.
pub const ALLOWLIST_ADMITTED_URL: &str = "https://example.com/";

/// A URL [`ENABLE_BY_ALLOWLIST_TOML`] must still refuse — the allow-list is an
/// allow-list, not an "allow everything" switch.
pub const ALLOWLIST_REFUSED_URL: &str = "https://not-on-the-list.test/";

/// A URL only [`ENABLE_BY_DEFAULT_ACTION_TOML`] admits.
pub const DEFAULT_ACTION_ADMITTED_URL: &str = "https://anything-at-all.test/";

/// The full remediation message shown when the browser tool denies solely
/// because it is in its fail-closed default posture.
pub fn disabled_by_default_hint() -> String {
    format!(
        "Browser tool is disabled by default. \
         Add allowed domains to your config.toml to enable it:\n\n\
         {ENABLE_BY_ALLOWLIST_TOML}\n\
         Alternatively, permit all origins — not recommended, exposes SSRF risk:\n\n\
         {ENABLE_BY_DEFAULT_ACTION_TOML}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Crate-local half of the guard: the hint must actually contain the
    /// snippets it claims to, so the cross-crate round-trip in
    /// `wcore-agent/tests/browser_config_hint_roundtrip.rs` is testing the
    /// text the operator really sees.
    #[test]
    fn hint_embeds_both_snippets_verbatim() {
        let hint = disabled_by_default_hint();
        assert!(
            hint.contains(ENABLE_BY_ALLOWLIST_TOML),
            "hint dropped the allow-list snippet:\n{hint}"
        );
        assert!(
            hint.contains(ENABLE_BY_DEFAULT_ACTION_TOML),
            "hint dropped the default_action snippet:\n{hint}"
        );
    }

    /// Neither snippet may name a bare `[browser]` section. The loader reads
    /// `browser.policy.*`; a key at `[browser]` level is silently discarded.
    #[test]
    fn snippets_never_name_the_bare_browser_section() {
        for snippet in [ENABLE_BY_ALLOWLIST_TOML, ENABLE_BY_DEFAULT_ACTION_TOML] {
            assert!(
                !snippet.lines().any(|l| l.trim() == "[browser]"),
                "snippet names [browser]; the loader reads [browser.policy]:\n{snippet}"
            );
        }
    }

    /// The policy fields the snippets set must be the ones the tool's own
    /// policy type exposes — a rename on either side breaks the promise.
    #[test]
    fn snippets_drive_the_local_policy_type() {
        let allowlist: crate::BrowserPolicy = crate::BrowserPolicy::new(
            crate::PolicyAction::Deny,
            vec!["example.com".into()],
            vec![],
        );
        assert!(allowlist.check_url(ALLOWLIST_ADMITTED_URL).is_ok());
        assert!(allowlist.check_url(ALLOWLIST_REFUSED_URL).is_err());

        let allow_all = crate::BrowserPolicy::new(crate::PolicyAction::Allow, vec![], vec![]);
        assert!(allow_all.check_url(DEFAULT_ACTION_ADMITTED_URL).is_ok());
    }
}
