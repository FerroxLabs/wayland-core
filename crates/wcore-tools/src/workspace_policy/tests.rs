use super::*;
use std::path::Path;

#[test]
fn trusted_local_sets_cwd_and_does_not_redirect_caches() {
    let dir = tempfile::tempdir().unwrap();
    let p = WorkspacePolicy::trusted_local(dir.path());
    assert_eq!(p.trust(), WorkspaceTrust::Trusted);
    assert!(p.writable_roots().iter().any(|w| w == p.root()));
    // Root identity: writable_roots()[0] must equal the canonicalized tmpdir.
    assert_eq!(
        p.root(),
        std::fs::canonicalize(dir.path())
            .unwrap_or_else(|_| dir.path().to_path_buf())
            .as_path()
    );
    // Trusted reuses the user's global caches — no redirect.
    assert!(p.cache_env().is_empty());
}

#[test]
fn trusted_local_never_grants_the_entire_home_directory() {
    let dir = tempfile::tempdir().unwrap();
    let policy = WorkspacePolicy::trusted_local(dir.path());
    let Some(home) = dirs::home_dir().and_then(|path| std::fs::canonicalize(path).ok()) else {
        return;
    };

    assert!(policy.writable_roots().iter().all(|path| path != &home));
    assert!(policy.readable_roots().iter().all(|path| path != &home));
}

#[test]
fn detected_developer_capability_paths_are_absolute_and_canonical() {
    let dir = tempfile::tempdir().unwrap();
    let policy = WorkspacePolicy::trusted_local(dir.path());

    for capability in policy.developer_capabilities() {
        if !capability.executable.is_empty() {
            let executable = Path::new(&capability.executable);
            assert!(executable.is_absolute());
            assert_eq!(std::fs::canonicalize(executable).unwrap(), executable);
        }
        for root in &capability.read_only_roots {
            let root = Path::new(root);
            assert!(root.is_absolute());
            assert_eq!(std::fs::canonicalize(root).unwrap(), root);
        }
    }
}

#[test]
fn session_capability_grant_is_read_only_and_local_trusted_only() {
    let workspace = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let executable = runtime.path().join("custom-tool");
    std::fs::write(&executable, b"tool").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let trusted = WorkspacePolicy::trusted_local(workspace.path());
    let before_writes = trusted.writable_roots();
    let capability = trusted.grant_session_capability(&executable).unwrap();
    let runtime_root = std::fs::canonicalize(runtime.path()).unwrap();
    assert!(trusted.readable_roots().contains(&runtime_root));
    assert_eq!(trusted.writable_roots(), before_writes);
    assert_eq!(
        capability.executable,
        std::fs::canonicalize(&executable)
            .unwrap()
            .to_string_lossy()
    );

    let contained = WorkspacePolicy::contained(workspace.path());
    assert!(matches!(
        contained.grant_session_capability(&executable),
        Err(WorkspaceCapabilityGrantError::RequiresTrustedLocal)
    ));
}

#[test]
#[serial_test::serial]
fn session_capability_grant_refreshes_secret_deny_for_new_read_root() {
    let workspace = tempfile::tempdir().unwrap();
    // Put the new runtime under HOME rather than the system temp root:
    // scratch_dirs() intentionally mounts the latter before any grant.
    let home = dirs::home_dir().expect("test requires a writable home directory");
    let runtime = tempfile::Builder::new()
        .prefix("wcore-capability-")
        .tempdir_in(home)
        .unwrap();
    let executable = runtime.path().join("custom-tool");
    let credential = runtime.path().join("credentials.toml");
    std::fs::write(&executable, b"tool").unwrap();
    std::fs::write(&credential, b"secret = true").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let previous = std::env::var_os("WAYLAND_HOME");
    // SAFETY: this test is serialized with the other environment-mutating
    // workspace-policy tests.
    unsafe { std::env::set_var("WAYLAND_HOME", runtime.path()) };
    let policy = WorkspacePolicy::trusted_local(workspace.path());
    assert!(
        !policy
            .secret_deny_paths()
            .contains(&std::fs::canonicalize(&credential).unwrap())
    );
    policy.grant_session_capability(&executable).unwrap();
    let dynamic = policy.secret_deny_paths_dynamic();
    match previous {
        Some(value) => unsafe { std::env::set_var("WAYLAND_HOME", value) },
        None => unsafe { std::env::remove_var("WAYLAND_HOME") },
    }

    assert!(
        dynamic.contains(&std::fs::canonicalize(&credential).unwrap()),
        "a post-bootstrap read grant must refresh credential denials: {dynamic:?}"
    );
}

#[test]
fn contained_redirects_caches_into_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let p = WorkspacePolicy::contained(dir.path());
    assert_eq!(p.trust(), WorkspaceTrust::Contained);
    let cargo = p
        .cache_env()
        .iter()
        .find(|(k, _)| k == "CARGO_HOME")
        .expect("Contained redirects CARGO_HOME");
    assert!(Path::new(&cargo.1).starts_with(p.root()));
    assert!(p.cache_env().iter().any(|(k, _)| k == "npm_config_cache"));
    assert!(p.cache_env().iter().any(|(k, _)| k == "PIP_CACHE_DIR"));
}

#[test]
fn network_is_gated_on_trust_posture() {
    // #657 (Overwatch ruling, Sean-confirmed): the bare `trusted_local`
    // constructor is fail-safe — it seeds network from the shared helper
    // (Deny unless `WAYLAND_BASH_ALLOW_NETWORK`), NOT unconditional Inherit.
    // Egress is granted only at bootstrap for a genuinely-local session; see
    // `local_bash_network` + `with_network`. Contained stays denied too.
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        WorkspacePolicy::trusted_local(dir.path()).network(),
        crate::bash::default_bash_network_policy(),
        "bare trusted_local must be fail-safe (Deny default), not network-on"
    );
    assert_eq!(
        WorkspacePolicy::contained(dir.path()).network(),
        crate::bash::default_bash_network_policy(),
        "a contained workspace stays denied (env opt-in via the helper)"
    );
    // `with_network` is the explicit local grant applied at bootstrap.
    assert_eq!(
        WorkspacePolicy::trusted_local(dir.path())
            .with_network(NetworkPolicy::Inherit)
            .network(),
        NetworkPolicy::Inherit,
        "with_network must override the fail-safe default"
    );
}

#[test]
fn local_bash_network_grants_inherit_only_without_channel_posture() {
    // The gate: a genuinely-local session (no channel posture) gets network
    // egress; any channel-attached session — including Full — stays on the
    // pre-#657 lockdown (default_bash_network_policy = Deny + env hatch).
    assert_eq!(
        local_bash_network(false),
        NetworkPolicy::Inherit,
        "genuinely-local session must get network egress"
    );
    assert_eq!(
        local_bash_network(true),
        crate::bash::default_bash_network_policy(),
        "a channel-attached session (incl Full) must stay on the Deny default"
    );
}

#[test]
fn is_secret_path_flags_project_and_key_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let p = WorkspacePolicy::contained(root);
    for rel in [
        ".env",
        ".env.local",
        ".env.production",
        ".git/config",
        ".git/hooks/pre-commit",
        ".git-credentials",
        "deploy/key.pem",
        "server.key",
        "cert.p12",
        "cert.pfx",
        ".npmrc",
        ".netrc",
        ".aws/credentials",
        "terraform.tfstate",
        "terraform.tfstate.backup",
        "gradle.properties",
        "service-account.json",
        "ci-key.json",
        "keys/id_rsa",
        "id_ed25519",
        ".ssh/id_ecdsa",
    ] {
        assert!(p.is_secret_path(&root.join(rel)), "{rel} must be secret");
    }
}

#[test]
fn is_secret_path_allows_ordinary_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let p = WorkspacePolicy::contained(root);
    for rel in [
        "src/main.rs",
        "README.md",
        "Cargo.toml",
        ".gitignore",
        "environment.rs",
        "package.json",
        "config.json",
    ] {
        assert!(
            !p.is_secret_path(&root.join(rel)),
            "{rel} must NOT be secret"
        );
    }
}

#[test]
fn is_secret_path_does_not_overmatch_json() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let p = WorkspacePolicy::contained(root);

    // These must NOT be flagged — they share a suffix but are not credentials.
    for not_secret in ["monkey.json", "package.json", "config.json"] {
        assert!(
            !p.is_secret_path(&root.join(not_secret)),
            "{not_secret} must NOT be secret"
        );
    }

    // These MUST be flagged — bounded credential patterns.
    for secret in [
        "service-account.json",
        "service-account-prod.json",
        "ci-key.json",
        "app_key.json",
        "key.json",
    ] {
        assert!(
            p.is_secret_path(&root.join(secret)),
            "{secret} must be secret"
        );
    }
}

// ── Task 6 tests ──────────────────────────────────────────────────────────

/// Contained mode: a project `.env` under the workspace root is in the
/// deny list; `src/main.rs` is NOT.
#[test]
fn contained_includes_project_env_excludes_main_rs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Create the file so canonicalize succeeds.
    std::fs::write(root.join(".env"), b"SECRET=x").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();

    let p = WorkspacePolicy::contained(root);
    let deny = p.secret_deny_paths();

    let env_canon = std::fs::canonicalize(root.join(".env")).unwrap();
    assert!(
        deny.contains(&env_canon),
        ".env must be in deny list; deny={deny:?}"
    );

    let main_canon = std::fs::canonicalize(root.join("src/main.rs")).unwrap();
    assert!(
        !deny.contains(&main_canon),
        "src/main.rs must NOT be in deny list"
    );
}

/// Trusted mode: the project `.env` under the workspace root is NOT in
/// the deny list (Trusted only denies credential stores, not project files).
#[test]
fn trusted_excludes_project_env() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join(".env"), b"SECRET=x").unwrap();

    let p = WorkspacePolicy::trusted_local(root);
    let deny = p.secret_deny_paths();

    let env_canon = std::fs::canonicalize(root.join(".env")).unwrap();
    assert!(
        !deny.contains(&env_canon),
        "Trusted must NOT deny project .env; deny={deny:?}"
    );
}

/// The active profile's OWN credential + OAuth stores (the Task 0.1 vault /
/// plaintext fallback / OAuth tokens) are denied so an LLM-driven bash
/// command cannot read them out of a Trusted sandbox, even though the
/// profile home sits inside a mounted root.
#[test]
#[serial_test::serial]
fn trusted_denies_wayland_profile_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Profile home inside the (readable) workspace root → mounted.
    let wh = root.join("profile-home");
    std::fs::create_dir_all(wh.join("oauth")).unwrap();
    std::fs::write(wh.join("credentials.toml"), b"secrets = {}").unwrap();
    std::fs::write(wh.join("oauth/chatgpt.json"), b"{}").unwrap();

    let prev = std::env::var_os("WAYLAND_HOME");
    // SAFETY: `#[serial_test::serial]` serializes every env-mutating test
    // in this binary, so this mutation cannot race another.
    unsafe { std::env::set_var("WAYLAND_HOME", &wh) };
    let p = WorkspacePolicy::trusted_local(root);
    // SAFETY: serial test; restore prior value (deny is already computed).
    match &prev {
        Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
        None => unsafe { std::env::remove_var("WAYLAND_HOME") },
    }
    let deny = p.secret_deny_paths();

    let cred = std::fs::canonicalize(wh.join("credentials.toml")).unwrap();
    assert!(
        deny.contains(&cred),
        "profile credentials.toml must be denied; deny={deny:?}"
    );
    let oauth = std::fs::canonicalize(wh.join("oauth")).unwrap();
    assert!(
        deny.contains(&oauth),
        "profile oauth dir must be denied; deny={deny:?}"
    );
}

/// Every emitted path is absolute.
#[test]
fn every_deny_path_is_absolute() {
    let dir = tempfile::tempdir().unwrap();
    let p = WorkspacePolicy::contained(dir.path());
    for path in p.secret_deny_paths() {
        assert!(path.is_absolute(), "deny path must be absolute: {path:?}");
    }
}

/// Symlink `notes.txt -> .env` (both inside the workspace) causes
/// `notes.txt`'s canonicalized path (= `.env`) to be denied in Contained
/// mode. Because `fs::canonicalize` resolves the symlink, the canonical
/// path equals `.env`'s canonical path and both end up in the deny list
/// (deduped to one entry).
#[cfg(unix)]
#[test]
fn contained_symlink_to_env_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join(".env"), b"SECRET=x").unwrap();
    std::os::unix::fs::symlink(".env", root.join("notes.txt")).unwrap();

    let p = WorkspacePolicy::contained(root);
    let deny = p.secret_deny_paths();

    // canonicalize(notes.txt) resolves to canonicalize(.env)
    let env_canon = std::fs::canonicalize(root.join(".env")).unwrap();
    assert!(
        deny.contains(&env_canon),
        "symlink target (.env canonical) must be in deny list; deny={deny:?}"
    );
}

// ── #667: project-secret deny for Full-posture channel/remote ─────────────

/// #667 Bash vector: `with_project_secret_deny` adds the project `.env` to
/// `secret_deny_paths()` (which `bash.rs` feeds to the OS sandbox's
/// `fs_read_deny`), matching Contained — while a bare `trusted_local` (the
/// genuinely-local keyboard session) still does NOT (see
/// `trusted_excludes_project_env`).
#[test]
fn with_project_secret_deny_denies_project_env_for_bash() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join(".env"), b"SECRET=x").unwrap();

    let env_canon = std::fs::canonicalize(root.join(".env")).unwrap();

    let local = WorkspacePolicy::trusted_local(root);
    assert!(
        !local.secret_deny_paths().contains(&env_canon),
        "local keyboard session must stay EXEMPT (may read own .env)"
    );

    let remote = WorkspacePolicy::trusted_local(root).with_project_secret_deny();
    assert!(
        remote.secret_deny_paths().contains(&env_canon),
        "Full/remote session must deny project .env; deny={:?}",
        remote.secret_deny_paths()
    );
}

/// #667 read-path predicate: `is_project_secret` is TRUE for a secret-named
/// file UNDER the workspace root and FALSE for both an ordinary in-root file
/// and a secret-named file OUTSIDE the root (host secrets stay readable — a
/// `Full` session is the trusted-remote-operator escape hatch).
#[test]
fn is_project_secret_is_scoped_to_workspace_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join(".env"), b"SECRET=x").unwrap();
    std::fs::write(root.join("main.rs"), b"fn main() {}").unwrap();

    // A secret sibling OUTSIDE the workspace root.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join(".env"), b"HOST=y").unwrap();

    let p = WorkspacePolicy::trusted_local(root);

    assert!(
        p.is_project_secret(&root.join(".env")),
        "in-root .env is a project secret"
    );
    assert!(
        !p.is_project_secret(&root.join("main.rs")),
        "ordinary in-root file is not a secret"
    );
    assert!(
        !p.is_project_secret(&outside.path().join(".env")),
        "a secret OUTSIDE the workspace root is out of scope (host secret)"
    );
}

/// #667: `is_project_secret` catches a project `.env` even when it did not
/// exist at construction time (lexical name-match, no TOCTOU gap), and the
/// under-root scope still resolves for a not-yet-created target (the
/// `canon_for_scope` parent fallback normalizes `/var`→`/private/var`).
#[test]
fn is_project_secret_has_no_toctou_gap() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Policy built BEFORE the .env exists.
    let p = WorkspacePolicy::trusted_local(root);
    let late_env = root.join("config").join(".env");
    std::fs::create_dir_all(root.join("config")).unwrap();
    assert!(
        p.is_project_secret(&late_env),
        "a project secret created after construction must still be denied"
    );
}

/// #667: `with_project_secret_deny` is idempotent — applying it twice does
/// not duplicate entries.
#[test]
fn with_project_secret_deny_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join(".env"), b"SECRET=x").unwrap();

    let once = WorkspacePolicy::trusted_local(root)
        .with_project_secret_deny()
        .secret_deny_paths()
        .to_vec();
    let twice = WorkspacePolicy::trusted_local(root)
        .with_project_secret_deny()
        .with_project_secret_deny()
        .secret_deny_paths()
        .to_vec();
    assert_eq!(once, twice, "double-apply must not duplicate deny entries");
}

/// #667 F2: the `secret_read_deny_required` flag (which gates whether
/// `bash.rs` refuses the shell on a non-enforcing backend) is set for
/// Contained AND for a Full/remote `with_project_secret_deny` policy, but
/// NOT for a bare local `trusted_local` — so a genuinely-local session keeps
/// its shell while a remote one is fenced.
#[test]
fn secret_read_deny_required_tracks_project_secret_denial() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(
        !WorkspacePolicy::trusted_local(root).secret_read_deny_required(),
        "local Trusted must NOT require read-deny enforcement (keeps shell)"
    );
    assert!(
        WorkspacePolicy::trusted_local(root)
            .with_project_secret_deny()
            .secret_read_deny_required(),
        "Full/remote Trusted must require read-deny enforcement (#667 F2)"
    );
    assert!(
        WorkspacePolicy::contained(root).secret_read_deny_required(),
        "Contained must require read-deny enforcement"
    );
}

// ── #234: Bash OS-deny recomputed per-exec (post-bootstrap TOCTOU) ─────────

/// #234 core: a Full/remote policy denies a secret CREATED AFTER
/// construction via `secret_deny_paths_dynamic()`, while the frozen
/// `secret_deny_paths()` (what `bash.rs` used before) MISSES it — proving the
/// dynamic re-walk is what closes the `Bash cat terraform.tfstate` gap.
#[test]
fn dynamic_deny_catches_post_bootstrap_secret_remote() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Full/remote posture: project-secret denial active at construction,
    // but no secrets exist yet.
    let p = WorkspacePolicy::trusted_local(root).with_project_secret_deny();

    // Secrets appear AFTER bootstrap — the TOCTOU window.
    std::fs::write(root.join("deploy.pem"), b"-----BEGIN KEY-----").unwrap();
    std::fs::write(root.join("terraform.tfstate"), b"{}").unwrap();
    let pem = std::fs::canonicalize(root.join("deploy.pem")).unwrap();
    let tf = std::fs::canonicalize(root.join("terraform.tfstate")).unwrap();

    assert!(
        !p.secret_deny_paths().contains(&pem) && !p.secret_deny_paths().contains(&tf),
        "frozen list must MISS post-bootstrap secrets (that is the #234 gap)"
    );
    let dynamic = p.secret_deny_paths_dynamic();
    assert!(
        dynamic.contains(&pem),
        "dynamic deny must include the post-bootstrap *.pem; got {dynamic:?}"
    );
    assert!(
        dynamic.contains(&tf),
        "dynamic deny must include the post-bootstrap terraform.tfstate; got {dynamic:?}"
    );
}

/// #234 anti-bypass: the dynamic walk must NOT honor `.gitignore` — secrets
/// are routinely gitignored, so an ignore-respecting walk would skip exactly
/// what must be denied. (Inherited from `project_committed_secrets`, pinned
/// here for the dynamic path.)
#[test]
fn dynamic_deny_ignores_gitignore_for_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join(".gitignore"), b"*.pem\n").unwrap();
    let p = WorkspacePolicy::trusted_local(root).with_project_secret_deny();
    std::fs::write(root.join("id.pem"), b"k").unwrap();
    let pem = std::fs::canonicalize(root.join("id.pem")).unwrap();
    assert!(
        p.secret_deny_paths_dynamic().contains(&pem),
        "a gitignored secret must STILL be denied (honoring .gitignore is the bypass)"
    );
}

/// #234 exemption preserved: a bare local-keyboard session (Trusted, no
/// project-secret denial) gets NO dynamic walk — `secret_deny_paths_dynamic`
/// equals the frozen list and the operator may still read their own `.env`.
#[test]
fn dynamic_deny_local_keyboard_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let p = WorkspacePolicy::trusted_local(root);
    std::fs::write(root.join(".env"), b"SECRET=x").unwrap();
    assert_eq!(
        p.secret_deny_paths_dynamic(),
        p.secret_deny_paths().to_vec(),
        "local keyboard session stays exempt — no dynamic walk"
    );
    let env = std::fs::canonicalize(root.join(".env")).unwrap();
    assert!(
        !p.secret_deny_paths_dynamic().contains(&env),
        "local .env must remain readable for the genuinely-local operator"
    );
}

/// #234: the Contained posture also picks up a post-bootstrap secret through
/// the dynamic re-walk.
#[test]
fn dynamic_deny_catches_post_bootstrap_secret_contained() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let p = WorkspacePolicy::contained(root);
    std::fs::write(root.join("secrets.pem"), b"k").unwrap();
    let pem = std::fs::canonicalize(root.join("secrets.pem")).unwrap();
    assert!(
        p.secret_deny_paths_dynamic().contains(&pem),
        "Contained must dynamically deny a post-bootstrap secret; got {:?}",
        p.secret_deny_paths_dynamic()
    );
}

/// MF3 (auditor) — the walk must NOT prune for detection: a secret INSIDE a
/// `node_modules`/`target`/`.wcache` dir is denied to Bash just as the file
/// tools' `is_project_secret` denies it, so `Bash cat node_modules/vendor/x.pem`
/// cannot read what `Read` refuses. The earlier prune (my #234 DoS fix) opened
/// exactly this hole; the fix is a lexical-first walk (no prune), not coverage
/// dropping. Nested ordinary-dir secret still caught (control).
#[test]
fn dynamic_deny_covers_secret_inside_machine_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let p = WorkspacePolicy::trusted_local(root).with_project_secret_deny();

    // Committed secrets that happen to live under machine-named dirs — MUST
    // be denied (the file tools' predicate denies them, so Bash must too).
    for d in ["target", ".wcache", "node_modules"] {
        std::fs::create_dir_all(root.join(d).join("vendor")).unwrap();
        std::fs::write(root.join(d).join("vendor").join("x.pem"), b"k").unwrap();
    }
    // Ordinary nested secret (control).
    std::fs::create_dir_all(root.join("deploy").join("keys")).unwrap();
    std::fs::write(root.join("deploy").join("keys").join("prod.pem"), b"k").unwrap();

    let dynamic = p.secret_deny_paths_dynamic();
    for d in ["target", ".wcache", "node_modules"] {
        let secret = std::fs::canonicalize(root.join(d).join("vendor").join("x.pem")).unwrap();
        assert!(
            dynamic.contains(&secret),
            "a committed secret under {d}/ MUST be denied (no prune); got {dynamic:?}"
        );
    }
    let real = std::fs::canonicalize(root.join("deploy").join("keys").join("prod.pem")).unwrap();
    assert!(
        dynamic.contains(&real),
        "nested ordinary secret must be denied"
    );
}

/// MF4 (auditor) — the Contained posture must not under-deny secrets under
/// machine-named dirs (the base branch denied them; the prune had regressed
/// this). Restored by dropping the prune.
#[test]
fn contained_denies_secret_inside_machine_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("node_modules").join("v")).unwrap();
    std::fs::write(root.join("node_modules").join("v").join("id.pem"), b"k").unwrap();
    let p = WorkspacePolicy::contained(root);
    let secret = std::fs::canonicalize(root.join("node_modules").join("v").join("id.pem")).unwrap();
    // Both the frozen construction-time list and the dynamic list must cover it.
    assert!(
        p.secret_deny_paths().contains(&secret),
        "Contained construction-time deny must cover node_modules secret; got {:?}",
        p.secret_deny_paths()
    );
    assert!(
        p.secret_deny_paths_dynamic().contains(&secret),
        "Contained dynamic deny must cover node_modules secret; got {:?}",
        p.secret_deny_paths_dynamic()
    );
}

/// Auditor round-2 HIGH: the git object store must be in Bash's fs_read_deny
/// for secret-deny postures so `git show HEAD:<committed secret>` cannot
/// reconstruct it from `.git/objects`. Local keyboard stays exempt.
#[test]
fn dynamic_deny_covers_git_object_store() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git").join("objects")).unwrap();
    let objects = std::fs::canonicalize(root.join(".git").join("objects")).unwrap();

    let remote = WorkspacePolicy::trusted_local(root).with_project_secret_deny();
    assert!(
        remote.secret_deny_paths_dynamic().contains(&objects),
        "Full/remote Bash deny must include .git/objects (git-show leak); got {:?}",
        remote.secret_deny_paths_dynamic()
    );

    let local = WorkspacePolicy::trusted_local(root);
    assert!(
        !local.secret_deny_paths_dynamic().contains(&objects),
        "local keyboard must NOT newly-deny .git/objects (exempt)"
    );
}

/// #667 F3: a benign-named symlink whose target is a project secret is
/// denied by `is_project_secret` even WITHOUT a `SandboxedFs` wrapper (the
/// Full deployment) — because the predicate canonicalizes first. Guards the
/// symlink read-through bypass on the Full read path.
#[cfg(unix)]
#[test]
fn is_project_secret_resolves_symlink_to_secret() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join(".env"), b"SECRET=x").unwrap();
    std::os::unix::fs::symlink(root.join(".env"), root.join("notes.txt")).unwrap();

    let p = WorkspacePolicy::trusted_local(&root);
    assert!(
        p.is_project_secret(&root.join("notes.txt")),
        "a benign-named symlink to a project secret must be denied (canon-first)"
    );
}
