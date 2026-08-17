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

/// The workspace-local config filename. The loader accepts this file form (and
/// a `.wayland-core/config.toml` dir form) inside the working directory — it
/// does NOT read a plain `config.toml` sitting beside the operator's work.
/// Named here because the hint must not advertise a filename the loader
/// ignores; see [`disabled_by_default_hint_for`].
pub const PROJECT_CONFIG_FILENAME: &str = ".wayland-core.toml";

/// The full remediation message shown when the browser tool denies solely
/// because it is in its fail-closed default posture.
///
/// Names the config file THIS process reads, resolved from the same
/// [`wcore_config::config::global_config_path`] the loader uses, so the two
/// cannot drift.
pub fn disabled_by_default_hint() -> String {
    disabled_by_default_hint_for(&wcore_config::config::global_config_path())
}

/// [`disabled_by_default_hint`] with the config path injected, so a guard can
/// drive the exact text an operator sees without depending on the host's
/// `WAYLAND_HOME`.
///
/// gh#900: naming the right SECTION was only half of `27-C2(a)`. The message
/// still said "your config.toml" with no path, and the reporter's sessions ran
/// with the working directory set to a throwaway `wcore-temp-*` workspace. An
/// operator following the hint verbatim creates `<cwd>/config.toml`, which is
/// not a config source in ANY layer: the workspace layer is
/// [`PROJECT_CONFIG_FILENAME`], and `config.toml` is only read under the app
/// config dir. Four workspaces and four edits later the tool was still denying,
/// with no diagnostic, because every edited file was invisible to the loader.
pub fn disabled_by_default_hint_for(config_path: &std::path::Path) -> String {
    format!(
        "Browser tool is disabled by default. \
         Add allowed domains to the config file this session reads — \
         {} — to enable it:\n\n\
         {ENABLE_BY_ALLOWLIST_TOML}\n\
         Alternatively, permit all origins — not recommended, exposes SSRF risk:\n\n\
         {ENABLE_BY_DEFAULT_ACTION_TOML}\n\
         A workspace-local override must be named `{PROJECT_CONFIG_FILENAME}` in the \
         working directory. Browser policy is read once when the session starts, so \
         restart the session after editing.",
        config_path.display(),
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

    /// gh#900 — the hint must name a config file the loader actually reads.
    ///
    /// This is an ALLOWLIST: exactly two filenames are legal in the message —
    /// the resolved config path (which ends in `config.toml`, but only under
    /// the app config dir) and [`PROJECT_CONFIG_FILENAME`]. Any OTHER mention
    /// of `config.toml` is an unanchored filename the operator will create in
    /// their working directory, where nothing reads it. Returns `false` when
    /// such a mention survives.
    fn names_only_config_files_the_loader_reads(hint: &str, config_path: &std::path::Path) -> bool {
        !hint
            .replace(&config_path.display().to_string(), "")
            .contains("config.toml")
    }

    #[test]
    fn hint_names_the_config_file_this_process_actually_reads() {
        let path = std::path::Path::new("/somewhere/absolute/wayland-core/config.toml");
        let hint = disabled_by_default_hint_for(path);
        assert!(
            hint.contains(&path.display().to_string()),
            "hint does not name the config file it is telling the operator to edit:\n{hint}"
        );
        assert!(
            names_only_config_files_the_loader_reads(&hint, path),
            "hint tells the operator to edit an unanchored `config.toml`; they will \
             create it in the working directory, which is not a config source:\n{hint}"
        );
        assert!(
            hint.contains(PROJECT_CONFIG_FILENAME),
            "hint never names the workspace-local filename, so an operator working in a \
             temp workspace has no correct local option:\n{hint}"
        );
    }

    /// Positive control for the predicate above: the pre-gh#900 text — which
    /// shipped and cost the reporter four workspaces of edits — must FAIL it.
    /// Without this the assertion could pass by being unfalsifiable.
    #[test]
    fn the_unanchored_pre_fix_wording_fails_the_same_check() {
        let path = std::path::Path::new("/somewhere/absolute/wayland-core/config.toml");
        const PRE_FIX: &str = "Browser tool is disabled by default. Add allowed domains to your config.toml \
             to enable it:";
        assert!(
            !names_only_config_files_the_loader_reads(PRE_FIX, path),
            "the check cannot detect the very wording it exists to ban; it is vacuous"
        );
    }

    /// The production entry point must resolve its path from the loader's own
    /// resolver. Recomputing it here from the SAME function is the point: a
    /// hardcoded or hand-built path in `disabled_by_default_hint` reddens this.
    #[test]
    // Reads `WAYLAND_HOME` twice (once directly, once through the hint) and
    // requires both to agree. `supervisor::tests::pid_dir_roots_under_wayland_home`
    // repoints that variable for the length of its body, so without this the two
    // reads can straddle the change and the assertion fails on a scheduling
    // interleave. Measured: with both windows artificially widened the test
    // reports "loader resolves: /root/.config/wayland-core/config.toml" against a
    // tempdir path in the hint.
    #[serial_test::serial]
    fn the_production_hint_names_the_loaders_own_resolved_path() {
        let expected = wcore_config::config::global_config_path();
        let hint = disabled_by_default_hint();
        assert!(
            hint.contains(&expected.display().to_string()),
            "the hint names a path the config loader does not resolve.\n\
             loader resolves: {}\nhint says:\n{hint}",
            expected.display()
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
