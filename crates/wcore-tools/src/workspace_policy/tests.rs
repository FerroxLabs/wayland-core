use super::*;
use std::path::Path;

/// REGRESSION PIN for the boot-path walk.
///
/// `WorkspacePolicy::contained()` used to run `project_committed_secrets` — a
/// full recursive NO-PRUNE walk of the workspace — at construction, to fill a
/// deny list that (after #234) nothing in production read. Construction happens
/// inside the bootstrap future that blocks the TUI's first paint, so on a large
/// tree it cost seconds of dead startup time.
///
/// Stated against baselines taken over an EMPTY tree rather than as a bare
/// `walk > construct * 10` ratio, because construction carries a per-platform
/// CONSTANT — canonicalization plus a handful of well-known-path probes — that
/// the old form silently assumed was zero. MEASURED on Windows, where that
/// constant is the same order as the walk itself at this tree size:
///
/// ```text
///   dirs   construct     walk
///    300    12.4 ms    11.6 ms
///   3000    10.4 ms   112.7 ms
///  12000    25.7 ms  1011.9 ms
/// ```
///
/// Construction is FLAT while the walk is linear, so the property holds — but
/// the ratio at 3000 sat on the 10x line and flapped (a failing CI run
/// measured 6.5x). Comparing construction against construction removes the
/// constant instead of pretending it is absent, and an eager walk still cannot
/// pass: it would add a whole walk to the big-tree construction.
#[test]
fn contained_construction_does_not_walk_the_workspace() {
    // Baselines over an empty tree. Taken FIRST so they absorb the cold cost
    // of the fixed path probes, which would otherwise land on the measurement
    // below and flatter it.
    let empty = tempfile::tempdir().unwrap();
    let t = std::time::Instant::now();
    let baseline = WorkspacePolicy::contained(empty.path());
    let construct_empty = t.elapsed();
    let t = std::time::Instant::now();
    let _ = baseline.secret_deny_paths_dynamic();
    let walk_empty = t.elapsed();

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Enough directories that a walk is unmistakably more expensive than not
    // walking, but small enough to stay quick in CI.
    for i in 0..3000 {
        let sub = root.join(format!("d{i}"));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("a.rs"), b"fn main() {}").unwrap();
    }

    let t0 = std::time::Instant::now();
    let p = WorkspacePolicy::contained(root);
    let construct = t0.elapsed();

    // Known-positive control in the same test: the walk this construction must
    // NOT be doing is still reachable, still happens, and its cost really is
    // driven by the tree. Without this the assertion below could pass on a
    // machine where BOTH are instant — i.e. where the instrument is dead.
    let t1 = std::time::Instant::now();
    let dynamic = p.secret_deny_paths_dynamic();
    let walk = t1.elapsed();
    let _ = dynamic;

    assert!(
        walk > walk_empty * 10 && walk > construct_empty,
        "instrument is dead: the walk must be reachable and tree-driven; \
         walk={walk:?} walk_empty={walk_empty:?} construct_empty={construct_empty:?}"
    );

    // An eager walk would put a whole `walk` inside `construct`. Half of one is
    // far below that and far above the noise on the constant.
    assert!(
        construct < construct_empty + walk / 2,
        "construction must not walk the workspace: construct={construct:?} \
         construct_empty={construct_empty:?} walk={walk:?} \
         (an eager walk adds a whole walk to construction)"
    );
}

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
    for (var, _) in CACHE_ENV_DIRS {
        assert!(
            p.cache_env().iter().all(|(k, _)| k != var),
            "Trusted must not redirect {var}"
        );
    }
    // ...but TMPDIR/TMP/TEMP MUST be redirected into the writable scratch
    // grant, because the sandbox backends remount the ungranted host `/tmp`
    // read-only. Leaving them alone is what made `sort` print nothing and
    // still report success (see `temp_env`).
    let scratch = p
        .writable_roots()
        .into_iter()
        .find(|w| w != p.root())
        .expect("Trusted has a writable scratch grant");
    for var in ["TMPDIR", "TMP", "TEMP"] {
        let value = p
            .cache_env()
            .iter()
            .find(|(k, _)| k == var)
            .unwrap_or_else(|| panic!("Trusted must redirect {var}"));
        assert_eq!(
            Path::new(&value.1),
            scratch,
            "{var} must point at the writable scratch grant"
        );
    }
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
            .secret_deny_paths_dynamic()
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

    // TMPDIR/TMP/TEMP go to the Contained scratch grant, NOT into the
    // workspace and NOT left on the read-only host `/tmp` (see `temp_env`).
    let scratch = p
        .writable_roots()
        .into_iter()
        .find(|w| w != p.root())
        .expect("Contained has a writable scratch grant");
    for var in ["TMPDIR", "TMP", "TEMP"] {
        let value = p
            .cache_env()
            .iter()
            .find(|(k, _)| k == var)
            .unwrap_or_else(|| panic!("Contained must redirect {var}"));
        assert_eq!(
            Path::new(&value.1),
            scratch,
            "{var} must point at the writable scratch grant"
        );
    }
}

#[test]
fn network_is_gated_on_trust_posture() {
    // #657 (Overwatch ruling, Sean-confirmed): the bare `trusted_local`
    // constructor is fail-safe — it seeds network from the shared helper
    // (an unconditional Deny), NOT unconditional Inherit.
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

/// #1099 follow-up: the credential denylist must fold ASCII case on EVERY
/// rule, not just the extension.
///
/// The predicate lower-cased only `path.extension()`, so `server.KEY` was
/// refused while `.ENV`, `ID_RSA`, `.SSH/known_hosts` and
/// `SERVICE-ACCOUNT.JSON` sailed through. On macOS and Windows — the two hosts
/// the desktop app ships on — the filesystem is case-INSENSITIVE, so `.ENV`
/// and `.env` are the SAME FILE and the alternate spelling is a plain read of
/// the real secret.
///
/// Every case variant is paired with its lowercase twin as a KNOWN-POSITIVE
/// CONTROL in the same loop: if a lowercase assertion ever fails, the query is
/// broken rather than the world, and the test says so instead of passing.
#[test]
fn is_secret_path_folds_case_on_every_rule() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let p = WorkspacePolicy::contained(root);

    for (lower, variant) in [
        // SECRET_SUFFIXES
        (".env", ".ENV"),
        (".env.local", ".Env.local"),
        (".git-credentials", ".Git-Credentials"),
        (".npmrc", ".NPMRC"),
        (".netrc", ".NetRC"),
        (".git/config", ".GIT/config"),
        (".aws/credentials", ".AWS/Credentials"),
        ("gradle.properties", "Gradle.Properties"),
        // SECRET_BASENAMES
        ("id_rsa", "ID_RSA"),
        ("keys/id_ed25519", "keys/Id_Ed25519"),
        // SECRET_DIR_SEGMENTS
        (".ssh/known_hosts", ".SSH/known_hosts"),
        (".gnupg/notes.txt", ".GnuPG/notes.txt"),
        // the bounded *.json credential shapes
        ("service-account.json", "SERVICE-ACCOUNT.JSON"),
        ("ci-key.json", "CI-KEY.Json"),
        // compound .tfstate extension — the plain `.tfstate` is caught by the
        // already-folded SECRET_EXTENSIONS arm, but `.tfstate.backup` parses
        // as extension `backup` and falls through to the name rule.
        ("terraform.tfstate.backup", "terraform.TFSTATE.backup"),
    ] {
        assert!(
            p.is_secret_path(&root.join(lower)),
            "CONTROL FAILED: {lower} must be secret — the test is broken, \
             not the product"
        );
        assert!(
            p.is_secret_path(&root.join(variant)),
            "{variant} must be secret: on a case-insensitive filesystem it \
             names the same file as {lower}"
        );
    }
}

/// Win32 strips trailing spaces and dots from the final path component before
/// opening it, so `.env `, `.env.` and `.env. ` all open `.env`. A denylist
/// matching the literal name is bypassable by typing a space — the same class
/// of alias as the case bypass above, and reachable the same way: the model
/// names the spelling that escapes the guard.
///
/// Each variant is paired with its plain twin as a KNOWN-POSITIVE CONTROL, so
/// a broken query fails loudly instead of passing.
#[test]
fn is_secret_path_strips_win32_trailing_space_and_dot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let p = WorkspacePolicy::contained(root);

    for (plain, alias) in [
        (".env", ".env "),
        (".env", ".env."),
        (".env", ".env. "),
        ("id_rsa", "id_rsa "),
        ("id_rsa", "id_rsa."),
        (".npmrc", ".npmrc "),
        ("service-account.json", "service-account.json "),
        // extension rule: `foo.key ` parses as extension `key ` and would
        // otherwise match nothing in SECRET_EXTENSIONS.
        ("server.key", "server.key "),
        ("cert.pem", "cert.pem."),
        // the alias and the case bypass compose — closing one must not leave
        // the pair open.
        (".env", ".ENV "),
        ("id_rsa", "ID_RSA."),
    ] {
        assert!(
            p.is_secret_path(&root.join(plain)),
            "CONTROL FAILED: {plain} must be secret — the test is broken, \
             not the product"
        );
        assert!(
            p.is_secret_path(&root.join(alias)),
            "{alias} must be secret: Win32 strips trailing spaces and dots, \
             so it opens the same file as {plain}"
        );
    }
}

/// The case fold must not turn ordinary files into secrets. Guards the other
/// direction of the same change: over-denying is the SAFE failure, but only
/// while it stays confined to the denylist's own shapes.
#[test]
fn case_fold_does_not_overmatch_ordinary_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let p = WorkspacePolicy::contained(root);
    for rel in [
        "SRC/MAIN.RS",
        "README.MD",
        "Cargo.TOML",
        ".GITIGNORE",
        "Environment.rs",
        "PACKAGE.JSON",
        "Config.Json",
        "MONKEY.JSON",
        "docs/ENVOY.md",
    ] {
        assert!(
            !p.is_secret_path(&root.join(rel)),
            "{rel} must NOT be secret"
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
    let deny = p.secret_deny_paths_dynamic();

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
    let deny = p.secret_deny_paths_dynamic();

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
    // The deny list is resolved on demand (it reads `WAYLAND_HOME` at call
    // time), so it must be materialized BEFORE the env var is restored.
    let deny = p.secret_deny_paths_dynamic();
    // SAFETY: serial test; restore prior value.
    match &prev {
        Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
        None => unsafe { std::env::remove_var("WAYLAND_HOME") },
    }

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
    for path in p.secret_deny_paths_dynamic() {
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
    let deny = p.secret_deny_paths_dynamic();

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
        !local.secret_deny_paths_dynamic().contains(&env_canon),
        "local keyboard session must stay EXEMPT (may read own .env)"
    );

    let remote = WorkspacePolicy::trusted_local(root).with_project_secret_deny();
    assert!(
        remote.secret_deny_paths_dynamic().contains(&env_canon),
        "Full/remote session must deny project .env; deny={:?}",
        remote.secret_deny_paths_dynamic()
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
        .secret_deny_paths_dynamic();
    let twice = WorkspacePolicy::trusted_local(root)
        .with_project_secret_deny()
        .with_project_secret_deny()
        .secret_deny_paths_dynamic();
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

/// #234 core: a Full/remote policy denies a secret CREATED AFTER construction
/// via `secret_deny_paths_dynamic()` — the `Bash cat terraform.tfstate` gap.
///
/// This test used to also assert that the frozen construction-time
/// `secret_deny_paths()` MISSED these files, as the contrast that motivated
/// #234. That half is gone because the frozen list itself is gone: it had no
/// production reader left (only this accessor's own tests), and keeping a
/// stale `pub` deny-list next to the live one invited a future caller to pick
/// the weaker of the two. Deleting it also took a full recursive no-prune walk
/// of the workspace off the TUI boot path. The surviving half is the one that
/// describes enforcement.
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
    // No project walk for this posture: nothing under the workspace root may
    // appear in the deny list at all. (This used to be phrased as "equals the
    // frozen list"; the frozen list is gone, so it is now stated directly.)
    let under_root: Vec<_> = p
        .secret_deny_paths_dynamic()
        .into_iter()
        .filter(|path| path.starts_with(root))
        .collect();
    assert!(
        under_root.is_empty(),
        "local keyboard session stays exempt — no project walk; got {under_root:?}"
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

// ---------- scratch grant: bounded, and not shared across trust ----------
//
// Re-authored from the narrowing frankforges proposed in wayland-core #254.
// Before this, `scratch_dirs()` returned `vec![canon(temp_dir())]` -- the
// ENTIRE host temp tree, handed to trusted and untrusted sessions alike.

#[test]
fn scratch_grant_is_bounded_not_the_whole_temp_tree() {
    let host_temp = canon(std::env::temp_dir());
    let dir = tempfile::tempdir().unwrap();
    let policy = WorkspacePolicy::trusted_local(dir.path());

    // THE REGRESSION: granting the host temp root gives a sandboxed child
    // write access to every other process's temp state.
    assert!(
        !policy.writable_roots().contains(&host_temp),
        "the whole host temp tree {host_temp:?} is granted writable"
    );

    let scratch =
        scratch_dir(WorkspaceTrust::Trusted).expect("a scratch dir must be establishable");
    assert_ne!(
        scratch, host_temp,
        "scratch collapsed back to the temp root"
    );
    assert!(
        scratch.starts_with(&host_temp),
        "scratch {scratch:?} escaped the temp tree {host_temp:?}"
    );
    // Bounded is only useful if it is also actually granted -- otherwise this
    // would pass equally against a version that granted no scratch at all.
    assert!(
        policy.writable_roots().contains(&scratch),
        "the bounded scratch dir {scratch:?} is not in {:?}",
        policy.writable_roots()
    );
}

#[test]
fn trusted_and_contained_do_not_share_a_scratch_directory() {
    let trusted = scratch_dir(WorkspaceTrust::Trusted).expect("trusted scratch");
    let contained = scratch_dir(WorkspaceTrust::Contained).expect("contained scratch");
    assert_ne!(
        trusted, contained,
        "one shared scratch dir lets an untrusted session write where a \
         trusted session reads"
    );
    assert!(
        !trusted.starts_with(&contained) && !contained.starts_with(&trusted),
        "scratch dirs must be siblings, not nested: {trusted:?} vs {contained:?}"
    );

    // Same property at the public surface, which is what actually reaches the
    // sandbox backend as a write ACE.
    let t = tempfile::tempdir().unwrap();
    let c = tempfile::tempdir().unwrap();
    let trusted_roots = WorkspacePolicy::trusted_local(t.path()).writable_roots();
    for root in WorkspacePolicy::contained(c.path()).writable_roots() {
        assert!(
            !trusted_roots.contains(&root),
            "a Contained session shares writable root {root:?} with a Trusted session"
        );
    }
}

#[test]
fn scratch_dir_is_a_real_directory_we_own() {
    let scratch = scratch_dir(WorkspaceTrust::Trusted).expect("trusted scratch");
    let meta = std::fs::symlink_metadata(&scratch).expect("scratch must exist once granted");
    assert!(
        meta.is_dir(),
        "scratch {scratch:?} is not a directory -- a squatted symlink would be \
         granted a write ACE"
    );
    assert!(
        scratch
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with(SCRATCH_ROOT)),
        "scratch {scratch:?} is not under the {SCRATCH_ROOT} tree"
    );
}

/// Measured under the macOS seatbelt sandbox: with only the executable's own
/// directory granted, `npm --version` dies with
/// `Error: Cannot find module '../lib/cli.js'` — the entry shim lives in
/// `<pkg>/bin` while the code it requires lives in `<pkg>/lib`.
#[test]
fn capability_roots_grants_the_node_package_root_not_just_its_bin_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let package = tmp.path().join("lib").join("node_modules").join("npm");
    std::fs::create_dir_all(package.join("bin")).expect("bin");
    std::fs::create_dir_all(package.join("lib")).expect("lib");
    let executable = package.join("bin").join("npm-cli.js");
    std::fs::write(&executable, b"#!/usr/bin/env node\n").expect("write shim");
    let executable = std::fs::canonicalize(&executable).expect("canonicalize");

    let roots = capability_roots(&executable);
    let want = std::fs::canonicalize(&package).expect("canonicalize package");
    assert!(
        roots.contains(&want),
        "package root {want:?} must be granted so the shim can require ../lib; got {roots:?}"
    );

    // Narrowness: the shared node_modules tree must NOT become readable.
    let node_modules =
        std::fs::canonicalize(tmp.path().join("lib").join("node_modules")).expect("canonicalize");
    assert!(
        !roots.contains(&node_modules),
        "grant widened to the whole node_modules tree {node_modules:?}: {roots:?}"
    );
}

#[test]
fn capability_roots_keeps_the_scope_segment_for_a_scoped_node_package() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let package = tmp
        .path()
        .join("lib")
        .join("node_modules")
        .join("@acme")
        .join("cli");
    std::fs::create_dir_all(package.join("bin")).expect("bin");
    let executable = package.join("bin").join("cli.js");
    std::fs::write(&executable, b"#!/usr/bin/env node\n").expect("write shim");
    let executable = std::fs::canonicalize(&executable).expect("canonicalize");

    let roots = capability_roots(&executable);
    let want = std::fs::canonicalize(&package).expect("canonicalize package");
    assert!(
        roots.contains(&want),
        "scoped package root {want:?} must be granted; got {roots:?}"
    );
    let scope =
        std::fs::canonicalize(package.parent().expect("scope dir")).expect("canonicalize scope");
    assert!(
        !roots.contains(&scope),
        "grant widened to the whole @acme scope {scope:?}: {roots:?}"
    );
}

/// The `contained` profile must stop git reading the operator's global config
/// instead of being granted it.
///
/// This profile is what a workspace gets BY DEFAULT — `EffectiveWorkspaceTrust`
/// starts untrusted — and `$HOME/.gitconfig` is deliberately not in its
/// readable roots. git opens that file unconditionally, so before this redirect
/// every git invocation under macOS seatbelt died with
/// `fatal: unable to access '<home>/.gitconfig': Operation not permitted`.
///
/// Both values must be ABSOLUTE paths inside the workspace cache root: git
/// requires an absolute path for these variables, and the cache root is already
/// a writable root, so `git config --global` inside the sandbox lands in a real
/// scoped file rather than being silently discarded.
#[test]
fn contained_redirects_git_global_config_into_the_workspace_cache() {
    let dir = tempfile::tempdir().expect("workspace");
    let policy = WorkspacePolicy::contained(dir.path());
    let cache_root = policy.root().join(".wcache");

    for var in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
        let value = policy
            .cache_env()
            .iter()
            .find(|(k, _)| k == var)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| {
                panic!("{var} must be redirected; git reads the host file without it")
            });
        let value = Path::new(&value);
        assert!(
            value.is_absolute(),
            "git rejects a relative {var}, so the redirect would be silently ignored: {value:?}"
        );
        assert!(
            value.starts_with(&cache_root),
            "{var} must point inside the workspace cache root {cache_root:?}, got {value:?}"
        );
    }
}

/// The same redirect must NOT be applied to `trusted_local`.
///
/// That profile grants `$HOME/.gitconfig` on purpose — a local operator working
/// in their own trusted checkout should keep their identity, aliases and
/// includes. Redirecting there would be a silent behaviour change for the
/// common case, dressed up as a security fix.
#[test]
fn trusted_local_keeps_the_operators_own_git_config() {
    let dir = tempfile::tempdir().expect("workspace");
    let policy = WorkspacePolicy::trusted_local(dir.path());
    for var in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
        assert!(
            policy.cache_env().iter().all(|(k, _)| k != var),
            "the trusted profile grants the operator's own ~/.gitconfig and must \
             not redirect {var} away from it"
        );
    }
}

/// The `contained` profile must let a child STAT both paths libgit2 probes for
/// a global git configuration.
///
/// libgit2 derives them from `$HOME` / `$XDG_CONFIG_HOME` and ignores
/// `GIT_CONFIG_GLOBAL`, so the redirect in `git_config_env` — which does fix
/// `git(1)` — leaves `cargo new` dead: measured on Darwin 25.3.0, exit 101,
/// `failed to stat '<home>/.gitconfig'; class=Config (7)`. Under seatbelt an
/// ungranted path is EPERM and libgit2 treats that as fatal; ENOENT it would
/// have tolerated, which is why the identical code works on Linux bwrap.
///
/// BOTH paths, not just the first: with only `~/.gitconfig` granted, a host
/// that has an XDG git config failed identically one path along (measured).
#[test]
fn contained_grants_stat_on_both_libgit2_global_config_probes() {
    let dir = tempfile::tempdir().expect("workspace");
    let policy = WorkspacePolicy::contained(dir.path());
    let home = dirs::home_dir().expect("home directory");
    let granted = policy.metadata_readable_roots();
    for expected in [
        home.join(".gitconfig"),
        home.join(".config").join("git").join("config"),
    ] {
        assert!(
            granted.contains(&expected),
            "libgit2 probes {expected:?} and dies on EPERM; granted={granted:?}"
        );
    }
}

/// The grant must NOT be gated on the file existing.
///
/// A bwrap-shaped intuition ("no file, no problem") is wrong here: seatbelt
/// answers EPERM for an ungranted path whether or not it exists. Measured — a
/// profile granting metadata on a decoy path instead left `cargo new` at exit
/// 101 with the same `failed to stat` error, on the same host.
#[test]
fn contained_metadata_grant_is_not_gated_on_existence() {
    let dir = tempfile::tempdir().expect("workspace");
    let policy = WorkspacePolicy::contained(dir.path());
    let granted = policy.metadata_readable_roots();
    assert_eq!(
        granted.len(),
        2,
        "both probes must be granted unconditionally, not filtered by exists(); \
         granted={granted:?}"
    );
    for path in &granted {
        assert!(
            path.is_absolute(),
            "seatbelt needs a literal path: {path:?}"
        );
    }
}

/// A metadata grant must never become a READ grant. `~/.gitconfig` carries the
/// operator's identity and any `[url … insteadOf]` rewrite, which can embed a
/// credential — the exact thing `git_config_env` was written to keep away from
/// untrusted workspace content.
#[test]
fn contained_never_makes_the_global_git_config_readable() {
    let dir = tempfile::tempdir().expect("workspace");
    let policy = WorkspacePolicy::contained(dir.path());
    let readable = policy.readable_roots();
    let metadata = policy.metadata_readable_roots();
    // Without this the assertion below is satisfied by an EMPTY grant list —
    // confirmed by mutation: reverting the policy to `Vec::new()` left this
    // test green while the other three went red.
    assert!(
        !metadata.is_empty(),
        "nothing was granted, so the no-widening assertion would be vacuous"
    );
    for metadata_only in metadata {
        assert!(
            !readable.iter().any(|root| metadata_only.starts_with(root)),
            "{metadata_only:?} became readable via {readable:?}"
        );
    }
}

/// The interpreter grant gives the contained shell a package manager's PROGRAM
/// FILES and never its configuration or its state.
///
/// Before this the contained profile knew only about Rust, so `node` and `npm`
/// were exit 127 on a Homebrew host — not because the tool was missing but
/// because `ls -l /opt/homebrew/bin/node` was `Operation not permitted`. The
/// repair must not swing to the other extreme: `<prefix>/var` holds live
/// service state (`postgresql@16`, `postgresql@17` on the host this was
/// measured on) and `<prefix>/etc` holds service configuration.
#[test]
fn contained_interpreter_grant_excludes_package_manager_config_and_state() {
    let dir = tempfile::tempdir().expect("workspace");
    let readable = WorkspacePolicy::contained(dir.path()).readable_roots();
    let mut saw_a_prefix = false;
    for prefix in ["/opt/homebrew", "/opt/local", "/usr/local"] {
        let prefix = Path::new(prefix);
        if !prefix.exists() {
            continue;
        }
        for root in &readable {
            if !root.starts_with(prefix) {
                continue;
            }
            saw_a_prefix = true;
            assert_ne!(
                root.as_path(),
                prefix,
                "the whole package prefix was granted to untrusted content"
            );
            for forbidden in ["var", "Caskroom", "Library"] {
                assert!(
                    !root.starts_with(prefix.join(forbidden)),
                    "{root:?} reaches {forbidden} — configuration/state, not program files"
                );
            }
            // `etc` yields exactly one thing: the OpenSSL config FILE that
            // every keg-linked binary opens at init. Never the directory —
            // an OpenSSL etc directory has a `private/` sibling.
            if root.starts_with(prefix.join("etc")) {
                assert!(
                    root.is_file() && root.file_name() == Some(std::ffi::OsStr::new("openssl.cnf")),
                    "{root:?} is an etc grant that is not the openssl config file"
                );
            }
        }
    }
    if !saw_a_prefix {
        eprintln!("note: no package-manager prefix on this host; prefix assertions vacuous");
    }
}

// ── #922 R1 acceptance tests (A1, A3, A4) ────────────────────────────────────

/// Build a fixture tree carrying every entry class the deny list must cover.
///
/// Returns the tempdir (kept alive by the caller) and the canonical root.
fn secret_fixture_tree() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join(".env"), b"TOKEN=1").unwrap();
    // gitignored, and still denied — the #234 anti-bypass property.
    std::fs::write(root.join(".gitignore"), b"id.pem\nnode_modules/\ntarget/\n").unwrap();
    std::fs::write(root.join("id.pem"), b"key").unwrap();
    std::fs::create_dir_all(root.join("node_modules/vendor")).unwrap();
    std::fs::write(root.join("node_modules/vendor/x.pem"), b"key").unwrap();
    std::fs::create_dir_all(root.join("target/debug/build")).unwrap();
    std::fs::write(root.join("target/debug/build/secret.pem"), b"key").unwrap();
    std::fs::create_dir_all(root.join(".git/objects")).unwrap();
    std::fs::write(root.join(".git/objects/keep"), b"x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join(".env"), root.join("notes.txt")).unwrap();
    (dir, root)
}

/// A1 — #922 R1: the deny walk is not run for a backend that discards the list.
///
/// Modelled on `contained_construction_does_not_walk_the_workspace` above,
/// including its known-positive control: the `true` arm must be materially
/// more expensive on the big tree than on an empty one, so a host where both
/// arms are instant FAILS the instrument instead of passing the assertion
/// vacuously.
#[test]
fn r1_skips_the_walk_for_a_non_enforcing_backend() {
    let empty = tempfile::tempdir().unwrap();
    let baseline = WorkspacePolicy::contained(empty.path());
    // Taken first so the cold cost of the fixed path probes lands here.
    let t = std::time::Instant::now();
    let _ = baseline.secret_deny_paths_for_backend(true);
    let enforcing_empty = t.elapsed();
    let t = std::time::Instant::now();
    let _ = baseline.secret_deny_paths_for_backend(false);
    let skipped_empty = t.elapsed();

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for i in 0..3000 {
        let sub = root.join(format!("d{i}"));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("a.rs"), b"fn main() {}").unwrap();
    }
    // One real secret, so the enforcing arm's output is non-empty for a reason
    // this test controls. Without it the arm is non-empty only when the HOST
    // happens to have a Tier-0 credential store (`/etc/docker` on the Linux box,
    // nothing at all under a Windows service account with no `HOME`), and the
    // liveness assertion below then fails for an environmental reason rather
    // than a product one. Found by running this test on SeanDesktop.
    std::fs::write(root.join(".env"), b"TOKEN=1").unwrap();
    let p = WorkspacePolicy::contained(root);

    let t = std::time::Instant::now();
    let enforcing = p.secret_deny_paths_for_backend(true);
    let enforcing_big = t.elapsed();
    let t = std::time::Instant::now();
    let skipped = p.secret_deny_paths_for_backend(false);
    let skipped_big = t.elapsed();

    // KNOWN-POSITIVE CONTROL: the walk this test claims to be skipping is
    // reachable, does happen, and its cost really is driven by the tree.
    assert!(
        enforcing_big > enforcing_empty * 10,
        "instrument is dead: the enforcing arm must be tree-driven; \
         enforcing_big={enforcing_big:?} enforcing_empty={enforcing_empty:?}"
    );

    // THE CLAIM: the skipped arm is flat — the big tree costs no more than the
    // empty one. Stated against the empty-tree baseline plus half a walk, the
    // same shape the construction pin uses, so the platform constant is
    // subtracted rather than assumed to be zero.
    assert!(
        skipped_big < skipped_empty + enforcing_big / 2,
        "R1 must not walk on a non-enforcing backend: skipped_big={skipped_big:?} \
         skipped_empty={skipped_empty:?} enforcing_big={enforcing_big:?}"
    );

    // And the skipped arm produces nothing at all, while the enforcing arm does.
    assert!(
        skipped.is_empty(),
        "non-enforcing backend gets no list: {skipped:?}"
    );
    let want = std::fs::canonicalize(root.join(".env")).unwrap();
    assert!(
        enforcing.contains(&want),
        "instrument is dead: the enforcing arm must carry the tree's own secret; got {enforcing:?}"
    );
}

/// A3 — the enforcing path is byte-identical to the pre-#922 producer.
///
/// This is the "no security delta" claim. `secret_deny_paths_dynamic` is the
/// function every `#234` / `#667` test already grades; R1 must not have become
/// a second, drifting producer.
#[test]
fn r1_enforcing_arm_is_identical_to_the_dynamic_list() {
    let (_dir, root) = secret_fixture_tree();
    let p = WorkspacePolicy::contained(&root);

    let via_gate = p.secret_deny_paths_for_backend(true);
    let direct = p.secret_deny_paths_dynamic();
    assert_eq!(
        via_gate, direct,
        "the enforcing arm must be the same list, element for element"
    );

    // Not vacuous: the list actually carries the entry classes that matter.
    let has = |needle: &str| via_gate.iter().any(|p| p.ends_with(needle));
    assert!(has(".env"), "fixture .env missing from {via_gate:?}");
    assert!(has("id.pem"), "gitignored id.pem missing from {via_gate:?}");
    assert!(
        via_gate.iter().any(|p| p.ends_with(".git/objects")),
        "git object store missing from {via_gate:?}"
    );
}

/// A4 — the NO-PRUNE property survives #922.
///
/// The guard rail for the next reader who sees "436x" and reaches for a
/// `filter_entry`. Pruning `node_modules` / `target` is a PERMANENT
/// stale-negative: `is_project_secret` (the in-process file-tool predicate)
/// covers a secret anywhere under root, so the OS list must too, or
/// `Bash cat node_modules/vendor/x.pem` reads what `Read` refuses.
///
/// See the "NO directory prune" comment in `project_committed_secrets`.
#[test]
fn no_prune_survives_the_922_backend_gate() {
    let (_dir, root) = secret_fixture_tree();
    let p = WorkspacePolicy::contained(&root);
    let deny = p.secret_deny_paths_for_backend(true);

    for buried in ["node_modules/vendor/x.pem", "target/debug/build/secret.pem"] {
        let want = std::fs::canonicalize(root.join(buried)).unwrap();
        assert!(
            deny.contains(&want),
            "a secret under {buried} must still be denied — do NOT prune the walk; got {deny:?}"
        );
        // The two layers must agree: what the OS list denies, the in-process
        // predicate denies too.
        assert!(
            p.is_project_secret(&want),
            "in-process predicate must also cover {buried}"
        );
    }
}
