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
//! ## Naming the FILE, not just the section (gh#900)
//!
//! Naming the section correctly is only half of a followable instruction. The
//! reporter of gh#900 read "add allowed domains to your config.toml", and in a
//! desktop session the working directory is a throwaway `wcore-temp-*`
//! workspace — so they created `<cwd>/config.toml` in four different
//! workspaces in turn. A bare `config.toml` in the working directory is not a
//! config source in ANY layer: the project layer is `.wayland-core.toml` (or
//! `.wayland-core/config.toml`) and a plain `config.toml` is read only under
//! the app config dir. Their edits landed where the loader never looks, no
//! diagnostic was emitted, and they reasonably concluded the browser tool had
//! its own broken config resolver. [`config_targets`] resolves the real paths
//! from the same functions the loader calls, so the message cannot name a
//! location the loader does not read.
//!
//! ## Naming a setting that EXISTS (gh#826)
//!
//! A loopback denial used to carry no remediation at all — just "loopback
//! hostname blocked: localhost". With nothing true to say, an assistant
//! relaying that denial invented a plausible-sounding one, and a user went
//! looking for "allowLoopbackHostnames in sandbox settings", which has never
//! existed anywhere in this product. [`loopback_blocked_hint`] closes that gap
//! by naming the real `[browser.policy.loopback]` grant, and
//! [`ENABLE_LOOPBACK_TOML`] is round-tripped through the real loader so the
//! named setting is provably the one that works.
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

/// Project-layer config filename. The documented layout; `.wayland-core/config.toml`
/// is also accepted by the loader but only one form should ever be advertised.
pub const PROJECT_CONFIG_FILENAME: &str = ".wayland-core.toml";

/// TOML that grants the gh#911 loopback capability for one local port. Must
/// parse into a policy that admits [`LOOPBACK_ADMITTED_URL`] and still refuses
/// [`LOOPBACK_REFUSED_PORT_URL`] and [`LOOPBACK_REFUSED_PRIVATE_URL`].
pub const ENABLE_LOOPBACK_TOML: &str = "\
[browser.policy.loopback]
enabled = true
schema_version = 1
# Any label identifying who/what this grant is for; it is reported back in
# the allow decision. Must not be empty.
session_scope = \"local-dev\"
# Only these ports are reachable on loopback. There is no all-ports value.
ports = [3000]
";

/// A URL [`ENABLE_LOOPBACK_TOML`] promises to admit.
pub const LOOPBACK_ADMITTED_URL: &str = "http://localhost:3000/";

/// Same host, ungranted port — the grant is a port allow-list, not a
/// "localhost is open now" switch. (9377 is the Camoufox sidecar.)
pub const LOOPBACK_REFUSED_PORT_URL: &str = "http://localhost:9377/";

/// A loopback grant must not become an SSRF hole into the local network.
pub const LOOPBACK_REFUSED_PRIVATE_URL: &str = "http://192.168.0.1:3000/";

/// The config files the loader actually reads, most-specific last, resolved
/// through the very functions `resolve_config_files` calls.
///
/// Returned rather than formatted so the round-trip guard can write to these
/// paths and prove the advice works, instead of asserting on prose.
pub fn config_targets() -> Vec<String> {
    vec![
        wcore_config::config::global_config_path()
            .display()
            .to_string(),
        format!("<project dir>/{PROJECT_CONFIG_FILENAME}"),
    ]
}

/// The `Edit one of:` block shared by every remediation message. Naming the
/// real resolved global path is the gh#900 fix.
fn config_target_block() -> String {
    let mut out = String::from("Edit one of these files (the only ones the loader reads):\n");
    for target in config_targets() {
        out.push_str("  - ");
        out.push_str(&target);
        out.push('\n');
    }
    out.push_str(
        "A plain `config.toml` in the current working directory is NOT read \
         (in a desktop session that is a throwaway workspace).\n",
    );
    out
}

/// The full remediation message shown when the browser tool denies solely
/// because it is in its fail-closed default posture.
pub fn disabled_by_default_hint() -> String {
    format!(
        "Browser tool is disabled by default. \
         Add allowed domains to enable it.\n\n\
         {}\n\
         {ENABLE_BY_ALLOWLIST_TOML}\n\
         Alternatively, permit all origins — not recommended, exposes SSRF risk:\n\n\
         {ENABLE_BY_DEFAULT_ACTION_TOML}",
        config_target_block()
    )
}

/// Remediation for a loopback denial (gh#826 / gh#911).
///
/// `refusal` is the reason `BrowserPolicy` produced, which already names the
/// specific gate that refused (no grant / wrong `schema_version` / empty
/// `session_scope` / ungranted port). This wraps it with the real setting to
/// change, so the message a user reads always names a control that exists.
///
/// gh#1053 changed what the last paragraph is allowed to say. The host IS now
/// re-checked after DNS resolution on the executed path, so the old "NOT
/// re-checked" sentence became false — but the gate is still not total, and
/// saying it were would be gh#826 all over again (a message naming a
/// protection the reader does not have). Both halves are stated: what the gate
/// closes, and the residual sidecar/TTL gap it cannot.
pub fn loopback_blocked_hint(refusal: &str) -> String {
    format!(
        "{refusal}\n\n\
         Loopback is blocked by default. It is reachable only through an explicit, \
         port-scoped grant — there is no sandbox-wide switch and no setting named \
         anything else.\n\n\
         {}\n\
         {ENABLE_LOOPBACK_TOML}\n\
         Only the ports listed above become reachable. Private (RFC 1918), \
         link-local and cloud-metadata addresses stay blocked when they appear \
         literally in the URL, and any other host is re-checked after DNS \
         resolution — a name that resolves inward is refused, and one that \
         resolves to nothing at all is refused too.\n\n\
         The browser runs as a separate sidecar process and does its own DNS, \
         so a record served with TTL=0 could be re-answered inside a single \
         navigation. Core closes that by dialling for it: a sidecar Core \
         launches is pointed at a loopback proxy Core runs, which screens and \
         resolves every connection the browser opens and connects to the \
         address it screened. A sidecar Core did not launch is refused, \
         because that guarantee does not hold for it.\n\n\
         Loopback is included. Firefox would otherwise dial 127.0.0.1 and \
         localhost around any configured proxy, so Core sets \
         network.proxy.allow_hijacking_localhost in the Camoufox install \
         before starting the sidecar, and refuses to start one when it \
         cannot. A loaded page's own requests to this machine reach the same \
         gate, and are refused unless a grant above authorises the port.",
        config_target_block()
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
        for snippet in [
            ENABLE_BY_ALLOWLIST_TOML,
            ENABLE_BY_DEFAULT_ACTION_TOML,
            ENABLE_LOOPBACK_TOML,
        ] {
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

    /// gh#900 — every remediation message must name a real file. A message
    /// that says only "your config.toml" sent the reporter to
    /// `<cwd>/config.toml` four times, which no layer reads.
    #[test]
    fn both_hints_name_an_absolute_config_file() {
        let global = wcore_config::config::global_config_path();
        for hint in [
            disabled_by_default_hint(),
            loopback_blocked_hint("loopback hostname blocked: localhost"),
        ] {
            assert!(
                hint.contains(&global.display().to_string()),
                "hint names no resolved global config path, so a reader in a throwaway \
                 workspace has nowhere to put the snippet:\n{hint}"
            );
            assert!(
                hint.contains(PROJECT_CONFIG_FILENAME),
                "hint never names the project config filename:\n{hint}"
            );
        }
    }

    /// gh#826 — the loopback message must name the grant that exists and must
    /// carry the specific gate that refused, so a reader never has to guess a
    /// setting name.
    #[test]
    fn loopback_hint_names_the_real_grant_and_keeps_the_refusal() {
        let refusal = "loopback hostname blocked: localhost; \
                       loopback capability refused: port 9377 is not in the granted ports [3000]";
        let hint = loopback_blocked_hint(refusal);
        assert!(
            hint.contains(refusal),
            "hint dropped the specific refusal:\n{hint}"
        );
        assert!(
            hint.contains(ENABLE_LOOPBACK_TOML),
            "hint dropped the loopback grant snippet:\n{hint}"
        );
        assert!(
            hint.contains("[browser.policy.loopback]"),
            "hint never names the loopback setting:\n{hint}"
        );
    }

    /// INVERTED for gh#1053 (this guard was written to be inverted -- see the
    /// last line of its previous doc comment, "If the guard is ever wired into
    /// the live navigation path, update the hint AND this test together").
    ///
    /// Before: the hint had to promise NOTHING, because `check_resolved_host`
    /// had zero production callers and every claim would have been false.
    /// After: the resolution gate exists on the executed path, so the hint has
    /// to say so -- and, just as importantly, has to say what it still does
    /// NOT cover. Camoufox is a SIDECAR: Firefox resolves DNS in its own
    /// process and we cannot pin the addresses it dials. The gate closes
    /// static DNS SSRF; it does not close TTL=0 intra-navigation rebinding.
    ///
    /// An overclaiming hint is the exact failure gh#826 was: a message naming
    /// a protection the reader does not actually have.
    #[test]
    fn loopback_hint_claims_no_protection_the_live_path_does_not_have() {
        let hint = loopback_blocked_hint("loopback hostname blocked: localhost");
        let lowered = hint.to_ascii_lowercase();
        assert!(
            !lowered.contains("not re-checked after dns resolution"),
            "the resolution gate now runs on the executed path; the hint still \
             tells the operator it does not:\n{hint}"
        );
        assert!(
            lowered.contains("re-checked after dns resolution"),
            "the hint must state that the host IS re-checked after resolution, \
             or an operator cannot tell this build from one without the gate:\n{hint}"
        );
        assert!(
            lowered.contains("ttl"),
            "the hint must name the residual gap it does NOT close -- a sidecar \
             resolving with TTL=0 can still rebind inside a single navigation. \
             Claiming the gate is total is gh#826 all over again:\n{hint}"
        );
        assert!(
            lowered.contains("sidecar"),
            "the hint must say WHY the residual gap exists (the browser is a \
             separate process doing its own resolution), not just that it \
             exists:\n{hint}"
        );
    }

    /// The loopback snippet must drive the local policy type to exactly the
    /// three decisions it advertises. Anchoring on the text alone would let
    /// the snippet and the gate drift apart — which is how gh#826 happened.
    #[test]
    fn loopback_snippet_drives_the_local_policy_type() {
        let granted = crate::BrowserPolicy::new(crate::PolicyAction::Deny, vec![], vec![])
            .with_loopback(crate::policy::LoopbackCapability {
                enabled: true,
                schema_version: crate::policy::LOOPBACK_CAPABILITY_VERSION,
                session_scope: "local-dev".into(),
                ports: vec![3000],
            });
        assert!(granted.check_url(LOOPBACK_ADMITTED_URL).is_ok());
        assert!(granted.check_url(LOOPBACK_REFUSED_PORT_URL).is_err());
        assert!(granted.check_url(LOOPBACK_REFUSED_PRIVATE_URL).is_err());
    }
}
