//! T3-3.4 (sub-wave 4): environment variable passthrough registry
//! ported from the prior Wayland Python engine.
//!
//! Skills that declare `required_environment_variables` in their
//! frontmatter need those vars available in sandboxed execution
//! environments (`script` / `bash`). By default the sandboxes strip
//! secrets from the child process environment for security. This
//! module builds curated allowlists so skill-declared vars (and
//! user-configured overrides) pass through.
//!
//! Hosted sessions carry the resolved skill and user-config allowlist on
//! their immutable `SandboxRegistry`. Direct compatibility callers receive
//! only the fixed safe base allowlist; they cannot mutate process-wide child
//! environment authority.

use std::collections::HashSet;

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
///    discovery vars for the AWS CLI), or an immutable session passthrough
///    allowlist resolved from skills and config; and
/// 2. it does NOT match [`is_sensitive_env_var`] — secret-bearing names
///    are dropped unconditionally, so a misconfigured passthrough entry
///    cannot leak `*_API_KEY` / `WAYLAND_VAULT_PASSPHRASE` into a tool
///    child (and thence into the model context).
///
/// This replaces the historical blanket `std::env::vars().collect()`
/// copy that broadcast every host secret into every sandboxed command.
pub fn build_sandboxed_env(extra_allow: &[&str]) -> Vec<(String, String)> {
    build_sandboxed_env_for(extra_allow, None)
}

/// Build a sandbox environment using an immutable session allowlist.
pub fn build_sandboxed_env_for(
    extra_allow: &[&str],
    session_allow: Option<&HashSet<String>>,
) -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(name, _)| {
            if is_sensitive_env_var(name) {
                return false;
            }
            allowlist_contains(BASE_SANDBOX_ENV_ALLOWLIST, name)
                || allowlist_contains(extra_allow, name)
                || session_or_legacy_contains(session_allow, name)
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
    build_sandboxed_env_full(extra_allow, extra_prefixes, &[], None)
}

pub fn build_sandboxed_env_with_prefixes_for(
    extra_allow: &[&str],
    extra_prefixes: &[&str],
    session_allow: Option<&HashSet<String>>,
) -> Vec<(String, String)> {
    build_sandboxed_env_full(extra_allow, extra_prefixes, &[], session_allow)
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
/// matched as one exact platform environment name (case-insensitive on
/// Windows, case-sensitive on Unix) — a prefix or substring is never enough.
///
/// `force_allow` wins over `is_sensitive_env_var`; a name on
/// `force_allow` is kept even if it also matches a secret marker.
pub fn build_sandboxed_env_with_force_allow(
    extra_allow: &[&str],
    extra_prefixes: &[&str],
    force_allow: &[&str],
) -> Vec<(String, String)> {
    build_sandboxed_env_full(extra_allow, extra_prefixes, force_allow, None)
}

pub fn build_sandboxed_env_with_force_allow_for(
    extra_allow: &[&str],
    extra_prefixes: &[&str],
    force_allow: &[&str],
    session_allow: Option<&HashSet<String>>,
) -> Vec<(String, String)> {
    build_sandboxed_env_full(extra_allow, extra_prefixes, force_allow, session_allow)
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
    session_allow: Option<&HashSet<String>>,
) -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(name, _)| {
            // force_allow is an explicit per-tool credential escape hatch
            // and wins over the secret filter — checked first.
            if force_allow
                .iter()
                .any(|allowed| env_name_matches(allowed, name))
            {
                return true;
            }
            if is_sensitive_env_var(name) {
                return false;
            }
            allowlist_contains(BASE_SANDBOX_ENV_ALLOWLIST, name)
                || allowlist_contains(extra_allow, name)
                || prefix_matches(extra_prefixes, name)
                || session_or_legacy_contains(session_allow, name)
        })
        .collect()
}

fn env_name_matches(allowed: &str, actual: &str) -> bool {
    #[cfg(windows)]
    {
        allowed.eq_ignore_ascii_case(actual)
    }
    #[cfg(not(windows))]
    {
        allowed == actual
    }
}

fn session_or_legacy_contains(session_allow: Option<&HashSet<String>>, name: &str) -> bool {
    let Some(session_allow) = session_allow else {
        return false;
    };
    #[cfg(windows)]
    {
        session_allow
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(name))
    }
    #[cfg(not(windows))]
    {
        session_allow.contains(name)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Mutex;

    /// Tests mutate process environment; serialize them.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner())
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
    fn force_allow_uses_platform_exact_name_semantics() {
        assert!(env_name_matches(
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SECRET_ACCESS_KEY"
        ));
        assert!(!env_name_matches(
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SECRET_ACCESS_KEY_EXTRA"
        ));
        #[cfg(windows)]
        assert!(env_name_matches(
            "AWS_SECRET_ACCESS_KEY",
            "Aws_Secret_Access_Key"
        ));
        #[cfg(not(windows))]
        assert!(!env_name_matches(
            "AWS_SECRET_ACCESS_KEY",
            "Aws_Secret_Access_Key"
        ));
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
    fn force_allow_follows_platform_name_casing() {
        let _g = guard();
        unsafe {
            std::env::set_var("AWS_SECRET_ACCESS_KEY", "creds");
        }
        let env = build_sandboxed_env_with_force_allow(&[], &[], &["aws_secret_access_key"]);
        let included = env
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("AWS_SECRET_ACCESS_KEY"));
        #[cfg(windows)]
        assert!(included, "Windows environment names are case-insensitive");
        #[cfg(not(windows))]
        assert!(!included, "Unix environment names are case-sensitive");
        unsafe {
            std::env::remove_var("AWS_SECRET_ACCESS_KEY");
        }
    }

    #[test]
    #[serial]
    fn config_passthrough_var_is_applied_to_sandboxed_env() {
        // #325: a var resolved from `[tools] env_passthrough` must appear in
        // the immutable session environment without process-global state.
        let _g = guard();
        unsafe {
            std::env::set_var("TEST_CFG_PASSTHROUGH_VAR", "from-config");
        }
        // Without the config allowlist the var is stripped.
        assert!(
            !build_sandboxed_env(&[])
                .iter()
                .any(|(k, _)| k == "TEST_CFG_PASSTHROUGH_VAR"),
            "var must be stripped before config passthrough installs it"
        );
        let session = HashSet::from(["TEST_CFG_PASSTHROUGH_VAR".to_string()]);
        let env = build_sandboxed_env_for(&[], Some(&session));
        assert!(
            env.iter()
                .any(|(k, v)| k == "TEST_CFG_PASSTHROUGH_VAR" && v == "from-config"),
            "a config-passthrough var must be forwarded into the sandboxed env"
        );
        unsafe {
            std::env::remove_var("TEST_CFG_PASSTHROUGH_VAR");
        }
    }

    #[test]
    #[serial]
    fn config_passthrough_cannot_leak_a_secret_shaped_var() {
        // #325 safety: even if a user lists a secret-shaped name in
        // [tools] env_passthrough, the sandbox secret filter still drops it.
        let _g = guard();
        unsafe {
            std::env::set_var("TEST_CFG_SECRET_TOKEN", "nope");
        }
        let session = HashSet::from(["TEST_CFG_SECRET_TOKEN".to_string()]);
        let env = build_sandboxed_env_for(&[], Some(&session));
        assert!(
            !env.iter().any(|(k, _)| k == "TEST_CFG_SECRET_TOKEN"),
            "a secret-shaped config passthrough var must still be dropped"
        );
        unsafe {
            std::env::remove_var("TEST_CFG_SECRET_TOKEN");
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
