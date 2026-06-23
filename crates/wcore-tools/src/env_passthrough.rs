//! T3-3.4 (sub-wave 4): environment variable passthrough registry
//! ported from the prior Wayland Python engine.
//!
//! Skills that declare `required_environment_variables` in their
//! frontmatter need those vars available in sandboxed execution
//! environments (`script` / `bash`). By default the sandboxes strip
//! secrets from the child process environment for security. This
//! module provides a session-scoped allowlist so skill-declared vars
//! (and user-configured overrides) pass through.
//!
//! Two sources feed the allowlist:
//!
//! 1. **Skill declarations** — when a skill is loaded, its
//!    `required_environment_variables` are registered here via
//!    [`register_env_passthrough`].
//! 2. **User config** — the host wires `tools.env_passthrough` from
//!    `config.yaml` into [`set_config_passthrough`] once at startup.
//!
//! Callers (BashTool / ScriptTool / sandbox builders) consult
//! [`is_env_passthrough`] before stripping a variable from the child
//! process environment.
//!
//! ## Differences from the Python original
//!
//! The Python port used a `ContextVar` so each gateway request had its
//! own allowlist. Wayland runs as a single-tenant CLI / library, so we
//! use a process-global `RwLock<HashSet<String>>` instead. Tests use
//! [`clear_env_passthrough`] to reset state between cases.
//!
//! Config loading is also lifted out of this module — the helper
//! exposes an explicit [`set_config_passthrough`] setter so it stays
//! free of `wcore-config` dependency cycles. Hosts call it once at
//! boot with whatever they parsed.

use std::collections::HashSet;
use std::sync::OnceLock;

use parking_lot::RwLock;

/// Base set of environment variable names that are always safe to pass
/// into a sandboxed child: locale / terminal / toolchain-discovery vars
/// that carry no secret material. A sandboxed `bash` / CLI wrapper needs
/// these to actually function (`PATH` to find binaries, `HOME` for
/// per-user config lookups, etc.).
///
/// Deliberately excludes anything that can carry credentials —
/// `*_API_KEY`, `*_TOKEN`, `*_SECRET`, `WAYLAND_VAULT_*`, etc. are never
/// in this list and are filtered out by [`is_sensitive_env_var`].
const BASE_SANDBOX_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SHELL",
    "PWD",
    "COLUMNS",
    "LINES",
    "XDG_RUNTIME_DIR",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    // C3: the isolated-profile home, so a sandboxed command that itself invokes
    // `wayland-core` (or reads its config) resolves the SAME profile as the
    // parent rather than the default ~/.wayland. A non-secret path — exactly
    // like HOME / XDG_*_HOME already forwarded above, from which the default
    // home path is already inferable, so this exposes nothing new. The vault
    // passphrase (`WAYLAND_VAULT_*`) is still dropped by `is_sensitive_env_var`.
    "WAYLAND_HOME",
    // SSL trust-store discovery — needed by curl / git / CLIs to verify
    // TLS; the file *paths*, never a secret.
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
    "SYSTEMROOT", // Windows: required for most native binaries to start.
];

/// Substring / suffix patterns that mark an environment variable name as
/// secret-bearing. A var matching any of these is NEVER passed into a
/// sandboxed child even if it would otherwise be on an allowlist —
/// secrets win over convenience.
fn is_sensitive_env_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    // Wayland's own vault unlock secret — the single most dangerous var
    // to leak into a tool child.
    if upper.starts_with("WAYLAND_VAULT") {
        return true;
    }
    const SECRET_MARKERS: &[&str] = &[
        "API_KEY",
        "APIKEY",
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "PASSPHRASE",
        "PRIVATE_KEY",
        "ACCESS_KEY",
        "CREDENTIAL",
        "SESSION_KEY",
        "AUTH",
    ];
    SECRET_MARKERS.iter().any(|m| upper.contains(m))
}

/// Build the curated environment for a sandboxed tool child.
///
/// Starts from the host process environment and keeps a variable only if
/// **both**:
///
/// 1. it is on the [`BASE_SANDBOX_ENV_ALLOWLIST`], a caller-supplied
///    `extra_allow` list (e.g. `KUBECONFIG` for kubectl, `AWS_*`
///    discovery vars for the AWS CLI), or the session passthrough
///    allowlist ([`is_env_passthrough`] — skill / config declared); and
/// 2. it does NOT match [`is_sensitive_env_var`] — secret-bearing names
///    are dropped unconditionally, so a misconfigured passthrough entry
///    cannot leak `*_API_KEY` / `WAYLAND_VAULT_PASSPHRASE` into a tool
///    child (and thence into the model context).
///
/// This replaces the historical blanket `std::env::vars().collect()`
/// copy that broadcast every host secret into every sandboxed command.
pub fn build_sandboxed_env(extra_allow: &[&str]) -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(name, _)| {
            if is_sensitive_env_var(name) {
                return false;
            }
            allowlist_contains(BASE_SANDBOX_ENV_ALLOWLIST, name)
                || allowlist_contains(extra_allow, name)
                || is_env_passthrough(name)
        })
        .collect()
}

/// Allowlist membership check that respects platform env-var semantics.
/// Windows env-var names are case-insensitive — `std::env::vars()` may
/// yield `"Path"` even though the allowlist entry is spelled `"PATH"`,
/// and dropping `Path` from a sandboxed env would prevent the child from
/// finding binaries. Unix env names ARE case-sensitive, so the check
/// stays exact there.
fn allowlist_contains(allow: &[&str], name: &str) -> bool {
    #[cfg(windows)]
    {
        allow.iter().any(|a| a.eq_ignore_ascii_case(name))
    }
    #[cfg(not(windows))]
    {
        allow.contains(&name)
    }
}

/// Like [`build_sandboxed_env`] but additionally keeps any host variable
/// whose name starts with one of `extra_prefixes` (and is not sensitive).
///
/// The CLI wrappers need this: AWS credential *discovery* relies on a
/// family of `AWS_*` vars (`AWS_REGION`, `AWS_PROFILE`,
/// `AWS_CONFIG_FILE`, …) and gcloud on `CLOUDSDK_*` — too many to
/// enumerate, and new ones appear across CLI versions. The
/// [`is_sensitive_env_var`] filter still runs first, so
/// `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN` are dropped even though
/// they match the `AWS_` prefix.
pub fn build_sandboxed_env_with_prefixes(
    extra_allow: &[&str],
    extra_prefixes: &[&str],
) -> Vec<(String, String)> {
    build_sandboxed_env_full(extra_allow, extra_prefixes, &[])
}

/// Like [`build_sandboxed_env_with_prefixes`] but additionally passes
/// through an explicit `force_allow` list of *exact* variable names that
/// **bypass the [`is_sensitive_env_var`] secret filter**.
///
/// This is the escape hatch for a credential-carrying CLI tool that
/// legitimately needs a secret-shaped variable — the canonical case is
/// the `aws_cli` tool, which must receive `AWS_ACCESS_KEY_ID` /
/// `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN` to authenticate against
/// AWS when the host's only credential source is environment variables.
///
/// The R1 hardening principle still holds for every *other* path: the
/// secret filter is bypassed only for the exact names a specific tool
/// passes here, never broadcast to arbitrary commands. `force_allow` is
/// matched by exact, case-sensitive name — a prefix or substring is not
/// enough — so it cannot accidentally widen to the whole `AWS_*` family.
///
/// `force_allow` wins over `is_sensitive_env_var`; a name on
/// `force_allow` is kept even if it also matches a secret marker.
pub fn build_sandboxed_env_with_force_allow(
    extra_allow: &[&str],
    extra_prefixes: &[&str],
    force_allow: &[&str],
) -> Vec<(String, String)> {
    build_sandboxed_env_full(extra_allow, extra_prefixes, force_allow)
}

/// Shared implementation behind the public env builders.
///
/// A variable is kept if it is on `force_allow` (exact name — this wins
/// over the secret filter), OR it is non-sensitive AND on the base
/// allowlist / `extra_allow` / an `extra_prefixes` prefix / the session
/// passthrough allowlist.
fn build_sandboxed_env_full(
    extra_allow: &[&str],
    extra_prefixes: &[&str],
    force_allow: &[&str],
) -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(name, _)| {
            // force_allow is an explicit per-tool credential escape hatch
            // and wins over the secret filter — checked first.
            if allowlist_contains(force_allow, name) {
                return true;
            }
            if is_sensitive_env_var(name) {
                return false;
            }
            allowlist_contains(BASE_SANDBOX_ENV_ALLOWLIST, name)
                || allowlist_contains(extra_allow, name)
                || prefix_matches(extra_prefixes, name)
                || is_env_passthrough(name)
        })
        .collect()
}

/// Prefix-match check that respects platform env-var semantics —
/// case-insensitive on Windows, exact on Unix. Used for the discovery
/// prefix matchers like `AWS_` / `CLOUDSDK_`.
fn prefix_matches(prefixes: &[&str], name: &str) -> bool {
    #[cfg(windows)]
    {
        let name_upper = name.to_ascii_uppercase();
        prefixes
            .iter()
            .any(|p| name_upper.starts_with(&p.to_ascii_uppercase()))
    }
    #[cfg(not(windows))]
    {
        prefixes.iter().any(|p| name.starts_with(p))
    }
}

/// Skill-registered env var allowlist (mutable across the session).
fn skill_allowed() -> &'static RwLock<HashSet<String>> {
    static SKILL: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();
    SKILL.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Config-sourced allowlist (set once by the host at startup; never
/// mutated after — matches the cached `frozenset` semantics of the
/// Python original).
fn config_allowed() -> &'static RwLock<HashSet<String>> {
    static CONFIG: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();
    CONFIG.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Register environment variable names as allowed in sandboxed
/// execution environments.
///
/// Typically called when a skill declares
/// `required_environment_variables`. Empty / whitespace-only names are
/// silently skipped (matching the Python port's `name.strip()` guard).
pub fn register_env_passthrough<I, S>(var_names: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut set = skill_allowed().write();
    for name in var_names {
        let trimmed = name.as_ref().trim();
        if !trimmed.is_empty() {
            set.insert(trimmed.to_string());
        }
    }
}

/// Install the config-sourced passthrough allowlist (typically from
/// `tools.env_passthrough` in `config.yaml`).
///
/// Called once at host startup. Subsequent calls *replace* the config
/// allowlist — there's no merge, since the host owns the source of
/// truth.
pub fn set_config_passthrough<I, S>(var_names: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut set = config_allowed().write();
    set.clear();
    for name in var_names {
        let trimmed = name.as_ref().trim();
        if !trimmed.is_empty() {
            set.insert(trimmed.to_string());
        }
    }
}

/// Check whether `var_name` is allowed to pass through to sandboxed
/// environments.
///
/// Returns `true` if the variable was registered by a skill *or*
/// listed in the host-provided config allowlist.
pub fn is_env_passthrough(var_name: &str) -> bool {
    if skill_allowed().read().contains(var_name) {
        return true;
    }
    config_allowed().read().contains(var_name)
}

/// Return the union of skill-registered and config-based passthrough
/// vars. The returned set is a snapshot — subsequent mutations to the
/// registries are not reflected.
pub fn get_all_passthrough() -> HashSet<String> {
    let mut out = skill_allowed().read().clone();
    for name in config_allowed().read().iter() {
        out.insert(name.clone());
    }
    out
}

/// Reset the skill-registered allowlist (e.g. on session reset).
///
/// The config-sourced allowlist is **not** cleared — that mirrors the
/// Python port, where the config cache survived session resets.
pub fn clear_env_passthrough() {
    skill_allowed().write().clear();
}

/// Reset both the skill-registered and config-sourced allowlists.
/// Primarily for tests that need a clean slate; production code should
/// prefer [`clear_env_passthrough`] + an explicit
/// [`set_config_passthrough`] reload.
pub fn reset_all_for_test() {
    skill_allowed().write().clear();
    config_allowed().write().clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Mutex;

    /// Tests mutate process-global state; serialize them.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_all_for_test();
        g
    }

    #[test]
    fn registered_var_passes_through() {
        let _g = guard();
        register_env_passthrough(["MY_API_KEY"]);
        assert!(is_env_passthrough("MY_API_KEY"));
        assert!(!is_env_passthrough("OTHER_VAR"));
    }

    #[test]
    fn unregistered_var_is_denied() {
        let _g = guard();
        // Default allowlist is empty — nothing should pass.
        assert!(!is_env_passthrough("PATH"));
        assert!(!is_env_passthrough("HOME"));
        assert!(!is_env_passthrough(""));
    }

    #[test]
    fn config_passthrough_takes_effect() {
        let _g = guard();
        set_config_passthrough(["AWS_REGION", "AWS_PROFILE"]);
        assert!(is_env_passthrough("AWS_REGION"));
        assert!(is_env_passthrough("AWS_PROFILE"));
        assert!(!is_env_passthrough("AWS_SECRET"));
    }

    #[test]
    fn skill_and_config_union_via_get_all() {
        let _g = guard();
        register_env_passthrough(["SKILL_VAR_A", "SKILL_VAR_B"]);
        set_config_passthrough(["CFG_VAR_A", "SKILL_VAR_A"]); // overlap on A
        let all = get_all_passthrough();
        assert!(all.contains("SKILL_VAR_A"));
        assert!(all.contains("SKILL_VAR_B"));
        assert!(all.contains("CFG_VAR_A"));
        assert_eq!(all.len(), 3, "overlap should dedupe");
    }

    #[test]
    fn whitespace_and_empty_names_filtered() {
        let _g = guard();
        register_env_passthrough(["", "  ", "  GOOD  ", "\t\n"]);
        let all = get_all_passthrough();
        assert_eq!(all.len(), 1);
        assert!(all.contains("GOOD"), "leading/trailing ws should strip");
        assert!(!is_env_passthrough("  GOOD  "));
        assert!(is_env_passthrough("GOOD"));
    }

    #[test]
    fn clear_resets_skill_but_keeps_config() {
        let _g = guard();
        register_env_passthrough(["SKILL_VAR"]);
        set_config_passthrough(["CFG_VAR"]);
        clear_env_passthrough();
        assert!(!is_env_passthrough("SKILL_VAR"));
        assert!(
            is_env_passthrough("CFG_VAR"),
            "config allowlist must survive session reset (matches Python port)"
        );
    }

    #[test]
    fn set_config_replaces_previous_config() {
        let _g = guard();
        set_config_passthrough(["OLD_VAR"]);
        assert!(is_env_passthrough("OLD_VAR"));
        set_config_passthrough(["NEW_VAR"]);
        assert!(!is_env_passthrough("OLD_VAR"), "second call must replace");
        assert!(is_env_passthrough("NEW_VAR"));
    }

    #[test]
    fn register_is_additive_and_idempotent() {
        let _g = guard();
        register_env_passthrough(["VAR_A"]);
        register_env_passthrough(["VAR_A", "VAR_B"]);
        let all = get_all_passthrough();
        assert_eq!(all.len(), 2);
        assert!(all.contains("VAR_A"));
        assert!(all.contains("VAR_B"));
    }

    #[test]
    fn empty_inputs_are_noops() {
        let _g = guard();
        register_env_passthrough(Vec::<String>::new());
        set_config_passthrough(Vec::<&str>::new());
        assert!(get_all_passthrough().is_empty());
        assert!(!is_env_passthrough("ANYTHING"));
    }

    // ----------------------------------------------------------------
    // D.1 Round 1 (HIGH-2): curated sandbox env builder.
    // ----------------------------------------------------------------

    #[test]
    fn is_sensitive_env_var_flags_secret_shapes() {
        for name in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "DB_PASSWORD",
            "WAYLAND_VAULT_PASSPHRASE",
            "MY_PRIVATE_KEY",
            "SOME_CREDENTIAL",
            "service_apikey",
        ] {
            assert!(
                is_sensitive_env_var(name),
                "{name} should be flagged sensitive"
            );
        }
        for name in ["PATH", "HOME", "LANG", "KUBECONFIG", "AWS_REGION", "TERM"] {
            assert!(
                !is_sensitive_env_var(name),
                "{name} must NOT be flagged sensitive"
            );
        }
    }

    #[test]
    fn build_sandboxed_env_excludes_secrets_keeps_path() {
        let _g = guard();
        // PATH is always present in any test process; assert it survives.
        // Env-var names are case-insensitive on Windows (where iteration
        // surfaces "Path" rather than "PATH"), so compare case-insensitively.
        let env = build_sandboxed_env(&[]);
        assert!(
            env.iter().any(|(k, _)| k.eq_ignore_ascii_case("PATH")),
            "PATH must pass through so sandboxed bash can find binaries"
        );
        // No secret-shaped var may ever appear, regardless of host env.
        for (k, _) in &env {
            assert!(
                !is_sensitive_env_var(k),
                "secret-shaped var {k} leaked into sandboxed env"
            );
        }
    }

    #[test]
    #[serial]
    fn build_sandboxed_env_forwards_wayland_home_not_vault() {
        let _g = guard();
        // C3: WAYLAND_HOME must reach a sandboxed child so a nested wayland-core
        // invocation resolves the ACTIVE profile, not the default home. The
        // vault passphrase must still be dropped by the secret filter.
        unsafe {
            std::env::set_var("WAYLAND_HOME", "/tmp/isolated-profile");
            std::env::set_var("WAYLAND_VAULT_PASSPHRASE", "supersecret");
        }
        let env = build_sandboxed_env(&[]);
        assert!(
            env.iter()
                .any(|(k, v)| k == "WAYLAND_HOME" && v == "/tmp/isolated-profile"),
            "WAYLAND_HOME must be forwarded into the sandbox (C3 profile propagation)"
        );
        assert!(
            !env.iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("WAYLAND_VAULT_PASSPHRASE")),
            "the vault passphrase must never reach a sandboxed child"
        );
    }

    #[test]
    #[serial]
    fn build_sandboxed_env_secret_in_passthrough_is_still_dropped() {
        let _g = guard();
        // Even if a misconfigured allowlist names a secret var, the
        // is_sensitive_env_var filter wins.
        unsafe {
            std::env::set_var("TEST_LEAK_API_KEY", "supersecret");
        }
        register_env_passthrough(["TEST_LEAK_API_KEY"]);
        let env = build_sandboxed_env(&["TEST_LEAK_API_KEY"]);
        assert!(
            !env.iter().any(|(k, _)| k == "TEST_LEAK_API_KEY"),
            "a secret-shaped var must be dropped even when explicitly allowlisted"
        );
        unsafe {
            std::env::remove_var("TEST_LEAK_API_KEY");
        }
    }

    #[test]
    #[serial]
    fn build_sandboxed_env_extra_allow_passes_non_secret_var() {
        let _g = guard();
        unsafe {
            std::env::set_var("TEST_KUBECONFIG_PATH", "/home/u/.kube/config");
        }
        let without = build_sandboxed_env(&[]);
        assert!(!without.iter().any(|(k, _)| k == "TEST_KUBECONFIG_PATH"));
        let with = build_sandboxed_env(&["TEST_KUBECONFIG_PATH"]);
        assert!(
            with.iter().any(|(k, _)| k == "TEST_KUBECONFIG_PATH"),
            "an explicitly-allowed non-secret var must pass through"
        );
        unsafe {
            std::env::remove_var("TEST_KUBECONFIG_PATH");
        }
    }

    #[test]
    #[serial]
    fn build_sandboxed_env_with_force_allow_passes_named_secret_only() {
        let _g = guard();
        // Two secret-shaped vars sharing the AWS_ prefix: one is on the
        // force_allow list, one is not.
        unsafe {
            std::env::set_var("AWS_SECRET_ACCESS_KEY", "creds");
            std::env::set_var("AWS_OTHER_TOKEN", "leakme");
        }
        let env = build_sandboxed_env_with_force_allow(&[], &["AWS_"], &["AWS_SECRET_ACCESS_KEY"]);
        assert!(
            env.iter().any(|(k, _)| k == "AWS_SECRET_ACCESS_KEY"),
            "a force-allowed secret-shaped var must pass through"
        );
        assert!(
            !env.iter().any(|(k, _)| k == "AWS_OTHER_TOKEN"),
            "a secret-shaped var NOT on force_allow must still be dropped \
             even though its prefix matches"
        );
        unsafe {
            std::env::remove_var("AWS_SECRET_ACCESS_KEY");
            std::env::remove_var("AWS_OTHER_TOKEN");
        }
    }

    #[test]
    #[serial]
    fn build_sandboxed_env_with_prefixes_keeps_discovery_drops_secrets() {
        let _g = guard();
        unsafe {
            std::env::set_var("AWS_REGION_TEST", "us-east-1");
            std::env::set_var("AWS_SECRET_ACCESS_KEY_TEST", "leakme");
        }
        let env = build_sandboxed_env_with_prefixes(&[], &["AWS_REGION", "AWS_SECRET"]);
        assert!(
            env.iter().any(|(k, _)| k == "AWS_REGION_TEST"),
            "a non-secret prefixed discovery var must pass through"
        );
        assert!(
            !env.iter().any(|(k, _)| k == "AWS_SECRET_ACCESS_KEY_TEST"),
            "a secret-shaped var must be dropped even when its prefix matches"
        );
        unsafe {
            std::env::remove_var("AWS_REGION_TEST");
            std::env::remove_var("AWS_SECRET_ACCESS_KEY_TEST");
        }
    }
}
