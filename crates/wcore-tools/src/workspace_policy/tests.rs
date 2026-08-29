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
/// #1182: stated in ENTRIES VISITED, by direct observation through
/// [`super::walk_entries`], not in wall-clock time.
///
/// The two properties this test needs are "the walk really enumerates the tree"
/// (otherwise "construction did not walk" passes for free against a walk that
/// is unreachable) and "construction does not". Both used to be stated as
/// timings, and both were therefore decided by whatever else the host was
/// doing. The liveness half was the worse of the two because it could declare
/// ITSELF dead — recorded verbatim on the 0.13.10 integration branch under
/// concurrent load on a 96-core box:
///
/// ```text
/// instrument is dead: the walk must be reachable and tree-driven;
///   walk=20.476985ms  walk_empty=8.247227ms  construct_empty=772.466µs
/// ```
///
/// The walk in that run visited the whole 3000-directory tree. Nothing was
/// wrong with it; the EMPTY-tree baseline had stalled, the ratio compressed,
/// and the control reported a healthy instrument as a dead one. A ratio of two
/// wall clocks cannot tell "the walk did not happen" from "the machine is
/// busy", and only the first of those is a defect.
///
/// The counter tells them apart, and it keeps the property that made the
/// control worth having: a walk that becomes unreachable visits ZERO entries
/// and this test still fails. It also states the SECOND half directly, which
/// removes the last timing — an eager walk at construction is now a count, not
/// a duration comparison against a per-platform constant that earlier revisions
/// of this test had to model (on Windows, construction's fixed path probes cost
/// the same order as the walk itself at this tree size).
#[test]
fn contained_construction_does_not_walk_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Enough directories that a walk is unmistakably an enumeration of THIS
    // tree rather than of a handful of fixed probe paths, but small enough to
    // stay quick in CI. Above `SERIAL_WALK_BUDGET`, so the parallel arm is the
    // one graded — the arm whose entries are counted on a worker pool and
    // folded back onto this thread.
    for i in 0..3000 {
        let sub = root.join(format!("d{i}"));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("a.rs"), b"fn main() {}").unwrap();
    }
    // A real committed secret, so the walk has a positive result to report and
    // "visited the tree" cannot mean "enumerated it and understood nothing".
    std::fs::write(root.join(".env"), b"TOKEN=hunter2\n").unwrap();

    let before_construct = super::walk_entries();
    let p = WorkspacePolicy::contained(root);
    let during_construct = super::walk_entries() - before_construct;

    let before_walk = super::walk_entries();
    let dynamic = p.secret_deny_paths_dynamic();
    let during_walk = super::walk_entries() - before_walk;

    // KNOWN-POSITIVE CONTROL, stated first: the walk this construction must NOT
    // be doing is reachable and really did enumerate this tree. Without it the
    // assertion below passes for free against a walk that never runs.
    assert!(
        during_walk >= 3000,
        "instrument is dead: the walk must be reachable and must enumerate the \
         workspace; it visited {during_walk} entries over a 3000-directory tree"
    );
    // The second half of the same control: enumerating is not classifying. A
    // walk that visited every entry and stopped recognising secrets would leave
    // the deny list empty and every claim built on it vacuous.
    assert!(
        dynamic.iter().any(|path| path.ends_with(".env")),
        "instrument is dead: the walk must still find the planted .env; got {dynamic:?}"
    );

    // The property. An eager walk at construction would put the whole
    // enumeration above inside `contained()`.
    assert_eq!(
        during_construct, 0,
        "construction must not walk the workspace: it visited {during_construct} \
         entries before anything asked for the deny list (the walk itself visits \
         {during_walk})"
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
        // #243/#245, the Mercurial analogue of `.git/config`: `[paths]
        // default = https://user:TOKEN@host/repo` is the same
        // embedded-credential shape.
        ".hg/hgrc",
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
        // NOT `docs/...`: `remedy_advertisements::advertised_doc_paths_exist`
        // scans the tree for `docs/*.md` citations and asserts each file is
        // real, so a fictional doc path here reads as a broken advertisement.
        // The fixture only needs a name CONTAINING "env" that must not be
        // classified secret; the directory is incidental.
        "notes/ENVOY.md",
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

// ---------------------------------------------------------------------------
// FerroxLabs/wayland#1097 — a writable-but-unreadable path is refused at WRITE
// time, with the reason named, instead of failing at read time.
// ---------------------------------------------------------------------------

/// The ordinary case the invariant exists to keep true: a target inside the
/// workspace, in a directory that does not exist yet (which is what every
/// first spill and every first artifact write looks like).
#[test]
fn a_write_target_inside_the_workspace_is_readable_back() {
    let dir = tempfile::tempdir().expect("workspace");
    let policy = WorkspacePolicy::contained(dir.path());
    let target = dir
        .path()
        .join(".wayland-out")
        .join("results")
        .join("toolu_01.txt");
    assert!(
        !target.parent().unwrap().exists(),
        "the point of this case is that the directories are NOT created yet"
    );
    policy
        .ensure_write_target_readable(&target)
        .expect("a path under the workspace root must be readable back");
}

/// The shipped defect, stated as a property: the host temp tree is granted to
/// nothing, so a spill file written there is one the session can never open.
#[test]
fn a_write_target_in_the_host_temp_tree_is_refused_by_name() {
    let dir = tempfile::tempdir().expect("workspace");
    let policy = WorkspacePolicy::contained(dir.path());
    let target = std::env::temp_dir()
        .join("wayland-results")
        .join("toolu_01.txt");
    let refusal = policy
        .ensure_write_target_readable(&target)
        .expect_err("the host temp tree is outside every readable root");
    assert_eq!(refusal.path, target);
    let rendered = refusal.to_string();
    assert!(
        rendered.ends_with("is outside this session's readable roots"),
        "the refusal has to name the reason, not just fail: {rendered}"
    );
    assert!(
        rendered.contains("toolu_01.txt"),
        "the refusal has to name the path: {rendered}"
    );
}

/// The check resolves `..` and symlinks BEFORE the prefix match, so a target
/// that merely starts with the workspace root textually does not pass.
#[test]
fn a_traversal_out_of_the_workspace_is_refused() {
    let dir = tempfile::tempdir().expect("workspace");
    let workspace = dir.path().join("ws");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let policy = WorkspacePolicy::contained(&workspace);

    let traversal = workspace.join("..").join("outside").join("loot.txt");
    policy
        .ensure_write_target_readable(&traversal)
        .expect_err("a `..` segment must be resolved before the prefix match");

    let link = workspace.join("escape");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    #[cfg(windows)]
    let _ = std::os::windows::fs::symlink_dir(&outside, &link);
    if link.exists() {
        policy
            .ensure_write_target_readable(&link.join("loot.txt"))
            .expect_err("a symlinked parent must be resolved before the prefix match");
    }
}

/// A standing read grant is part of `readable_roots()`, so it moves this
/// answer too — the grant and the write-time check must not disagree about
/// what the session can read.
#[test]
fn a_granted_read_root_makes_a_write_target_acceptable() {
    let dir = tempfile::tempdir().expect("workspace");
    let workspace = dir.path().join("ws");
    let granted = dir.path().join("granted");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&granted).unwrap();
    // A standing grant is only issuable to a local-operator session; the
    // property under test is what the grant does to `readable_roots()`, so the
    // principal is set directly rather than reconstructed through bootstrap.
    let policy = WorkspacePolicy::contained(&workspace).with_local_operator_principal();

    let target = granted.join("report.html");
    policy
        .ensure_write_target_readable(&target)
        .expect_err("ungranted to start with");

    policy
        .grant_session_read_root(&granted, false)
        .expect("grant");
    policy
        .ensure_write_target_readable(&target)
        .expect("the grant is in readable_roots(), so the write target is covered now");
}

/// The escape the FIRST form of this check let through, kept as a standing
/// case: a `..` that follows a component which does not exist yet.
///
/// This is not a hypothetical shape — a spill target's own directories
/// (`<root>/.wayland-out/results/`) do not exist before the first spill, so
/// "part of the path is missing" is the ordinary state, not the edge case.
/// Resolving only the longest existing ancestor and appending the rest
/// verbatim leaves the `..` in the string: the result still `starts_with` the
/// workspace root while the real target is outside it.
#[test]
fn a_traversal_through_a_directory_that_does_not_exist_yet_is_refused() {
    let dir = tempfile::tempdir().expect("workspace");
    let workspace = dir.path().join("ws");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let policy = WorkspacePolicy::contained(&workspace);

    let missing = workspace.join("nope");
    assert!(
        !missing.exists(),
        "the point of this case is that this component is absent"
    );

    let traversal = missing
        .join("..")
        .join("..")
        .join("outside")
        .join("loot.txt");
    policy
        .ensure_write_target_readable(&traversal)
        .expect_err("a `..` after a missing component must still be applied");

    // Known-positive control: the identical shape that lands back INSIDE the
    // workspace is still accepted, so the case is not passing merely because
    // anything containing a `..` is refused.
    let inside = missing.join("..").join(".wayland-out").join("results.txt");
    policy
        .ensure_write_target_readable(&inside)
        .expect("a `..` that lands back inside the workspace is fine");
}

/// The same missing-component path, but the escape is a SYMLINK reached only
/// after the `..`. It has to be resolved before the prefix compare, which is
/// only true if resolution re-runs as the path is walked down.
#[test]
fn a_symlink_reached_after_a_missing_component_is_refused() {
    let dir = tempfile::tempdir().expect("workspace");
    let workspace = dir.path().join("ws");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let policy = WorkspacePolicy::contained(&workspace);

    let link = workspace.join("escape");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    #[cfg(windows)]
    let _ = std::os::windows::fs::symlink_dir(&outside, &link);
    if !link.exists() {
        return;
    }

    let traversal = workspace
        .join("nope")
        .join("..")
        .join("escape")
        .join("loot.txt");
    policy
        .ensure_write_target_readable(&traversal)
        .expect_err("a symlink reached after a missing component must be resolved");
}

/// The escape the shipped check let through, MEASURED before this test existed:
/// a symlink whose target does not exist yet.
///
/// `std::fs::canonicalize` fails on a DANGLING link, so the component stayed in
/// the comparison verbatim, the prefix compare judged where the LINK sits
/// rather than where the write lands, and `std::fs::write` followed it out of
/// the workspace. Observed with its controls in the same run:
///
/// ```text
///   CONTROL plain-outside    : Err(WriteTargetNotReadable)
///   CONTROL plain-inside     : Ok(())
///   CONTROL live-symlink-out : Err(WriteTargetNotReadable)   <-- target EXISTS
///   PROBE   dangling-symlink : Ok(())                        <-- accepted
///   ESCAPE: outside target exists = true, content = "escaped"
/// ```
///
/// The two writers this check is the ONLY containment for — the tool-result
/// spill and `text_to_speech` — both write with bare `std::fs::write`, so the
/// gap was reachable by anything that could plant a link in the workspace.
///
/// The controls are carried IN this test so a change that refused everything
/// (which would also make the probe pass) fails here instead of looking green.
#[test]
#[cfg(unix)]
fn a_dangling_symlink_out_of_the_workspace_is_refused() {
    let dir = tempfile::tempdir().expect("workspace");
    let workspace = dir.path().join("ws");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let policy = WorkspacePolicy::contained(&workspace);

    // CONTROL: an ordinary in-workspace target is still accepted, so a check
    // that refused everything cannot pass this test.
    policy
        .ensure_write_target_readable(&workspace.join("fine.txt"))
        .expect("an ordinary in-workspace target must still be accepted");

    // CONTROL: the same link shape with an EXISTING target was already refused
    // before this fix, so a green here has to come from the dangling case.
    let live_target = outside.join("live.txt");
    std::fs::write(&live_target, b"x").unwrap();
    let live_link = workspace.join("live_link.txt");
    std::os::unix::fs::symlink(&live_target, &live_link).unwrap();
    policy
        .ensure_write_target_readable(&live_link)
        .expect_err("CONTROL: a symlink with an existing target outside must be refused");

    // THE DEFECT: the target does not exist yet.
    let dangling_target = outside.join("loot.txt");
    let dangling = workspace.join("out.txt");
    std::os::unix::fs::symlink(&dangling_target, &dangling).unwrap();
    policy
        .ensure_write_target_readable(&dangling)
        .expect_err("a DANGLING symlink pointing out of the workspace must be refused");

    // CONTROL: a dangling link that stays INSIDE the workspace is a legitimate
    // write target and must still be accepted — the fix resolves the link, it
    // does not blanket-refuse unresolvable ones.
    let inside_link = workspace.join("inside_link.txt");
    std::os::unix::fs::symlink(workspace.join("not_yet.txt"), &inside_link).unwrap();
    policy
        .ensure_write_target_readable(&inside_link)
        .expect("CONTROL: a dangling link landing back inside the workspace stays writable");

    // A symlink CYCLE must terminate rather than spin.
    let a = workspace.join("cycle_a");
    let b = workspace.join("cycle_b");
    std::os::unix::fs::symlink(&b, &a).unwrap();
    std::os::unix::fs::symlink(&a, &b).unwrap();
    let _ = policy.ensure_write_target_readable(&a);
}

/// The same guard, with the workspace reached THROUGH a symlinked directory.
///
/// `ensure_write_target_readable` compares a resolved write target against a
/// readable root that has been through `canonicalize`. When the target is a
/// dangling symlink the resolver follows it by hand, and it used to return
/// that target verbatim -- so if the workspace itself sat under a symlink, the
/// two sides were spelled differently and a legitimate in-workspace write was
/// REFUSED.
///
/// macOS hits this on every run without arranging anything, because `TMPDIR`
/// is under `/var/folders` and `/var` is a symlink to `/private/var`; CI run
/// 32700730900 failed exactly here. Nothing about the defect is macOS-specific
/// though -- any workspace reached through a symlink has it -- so this builds
/// the topology explicitly and grades it on every platform that has symlinks.
#[cfg(unix)]
#[test]
fn a_workspace_reached_through_a_symlink_still_accepts_its_own_dangling_writes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let real = tmp.path().join("real");
    std::fs::create_dir_all(real.join("ws")).unwrap();
    std::fs::create_dir_all(real.join("outside")).unwrap();

    // The workspace is addressed through `link`, never through `real`.
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let workspace = link.join("ws");
    let outside = link.join("outside");

    let policy = WorkspacePolicy::contained(&workspace);

    // CONTROL: an ordinary target under the symlinked workspace is accepted,
    // so a green below cannot come from the whole policy being permissive.
    policy
        .ensure_write_target_readable(&workspace.join("plain.txt"))
        .expect("CONTROL: an ordinary in-workspace target must be accepted");

    // THE DEFECT: a dangling link landing back inside the workspace. Before the
    // fix the resolver returned the raw `<tmp>/link/ws/not_yet.txt` while the
    // readable root had canonicalized to `<tmp>/real/ws`, so `starts_with`
    // failed and this was refused.
    let inside_link = workspace.join("inside_link.txt");
    std::os::unix::fs::symlink(workspace.join("not_yet.txt"), &inside_link).unwrap();
    policy
        .ensure_write_target_readable(&inside_link)
        .expect("a dangling link landing back inside the workspace stays writable");

    // CONTROL, and the one that must never regress: the containment itself
    // still holds through the symlinked spelling.
    let escaping = workspace.join("escape.txt");
    std::os::unix::fs::symlink(outside.join("loot.txt"), &escaping).unwrap();
    policy
        .ensure_write_target_readable(&escaping)
        .expect_err("CONTROL: a dangling link pointing OUT of the workspace is still refused");
}

// ── #242 / #243: VCS content stores beyond the repo-root `.git` ───────────

/// Build a workspace holding a checkout of every VCS the deny set covers, plus
/// the working-STATE files of each that must stay readable.
fn vcs_workspace(root: &Path) {
    for (rel, body) in [
        (".git/objects/ab/cd1234", "zlib"),
        (".git/config", "url = https://u:TOK@h/r.git"),
        (".git/HEAD", "ref: refs/heads/main"),
        (".git/refs/heads/main", "deadbeef"),
        (".hg/store/data/_env.i", "revlog"),
        (".hg/hgrc", "default = https://u:TOK@h/r"),
        (".hg/dirstate", "state"),
        (".svn/pristine/ab/abcd.svn-base", "base"),
        (".svn/wc.db", "db"),
        (".bzr/repository/packs/x.pack", "pack"),
        ("src/store/mod.rs", "fn main() {}"),
    ] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
}

fn denies(deny: &[std::path::PathBuf], p: &Path) -> bool {
    let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    deny.contains(&canon)
}

/// #243 — the #241 content-store deny was git-only, so a committed secret in a
/// Mercurial / Subversion / Bazaar checkout was still reconstructable from the
/// respective store via `Bash` (`hg cat -r`, `svn cat -r`, `bzr cat -r`) in a
/// posture whose whole point is that committed secrets are unreadable.
///
/// The CONTROLS are the point of the second half: the deny must not swallow the
/// working-STATE files each VCS needs for an ordinary metadata question, which
/// is the same carve-out `.git/refs` + `HEAD` already get so `git rev-parse`
/// keeps working. A deny that refuses those trades a low-severity read hazard
/// for a broken checkout.
#[test]
fn secret_deny_covers_non_git_vcs_content_stores() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    vcs_workspace(root);

    let deny = WorkspacePolicy::trusted_local(root)
        .with_project_secret_deny()
        .secret_deny_paths_dynamic();

    // CONTROL that the query works at all: the git store #241 already denied.
    assert!(
        denies(&deny, &root.join(".git/objects")),
        "CONTROL: the pre-existing git object-store deny must still fire; deny={deny:?}"
    );
    for rel in [".hg/store", ".svn/pristine", ".bzr/repository", ".hg/hgrc"] {
        assert!(
            denies(&deny, &root.join(rel)),
            "{rel} is a VCS content/credential store and must be denied; deny={deny:?}"
        );
    }
    // WRONG-REFUSAL: metadata-only state stays readable in every VCS.
    for rel in [
        ".git/HEAD",
        ".git/refs/heads/main",
        ".hg/dirstate",
        ".svn/wc.db",
        "src/store/mod.rs",
    ] {
        assert!(
            !denies(&deny, &root.join(rel)),
            "{rel} carries no committed content and must stay readable; deny={deny:?}"
        );
    }
}

/// #242 — a workspace that IS a linked worktree (`git worktree add`) has a `.git`
/// FILE, not a directory, so `root.join(".git/objects")` matches nothing while
/// `git show HEAD:.env` inside it reads the MAIN repository's store perfectly
/// well. Every spelling of "the store is not at `<root>/.git/objects`" is graded
/// here: an absolute gitdir behind a `commondir` hop, a relative gitdir (the
/// submodule shape), and an `objects/info/alternates` borrow.
#[test]
fn secret_deny_follows_gitfile_and_alternate_object_stores() {
    let outer = tempfile::tempdir().unwrap();
    let mk = |rel: &str, body: &str| {
        let p = outer.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    };
    let deny_for = |root: &Path| {
        WorkspacePolicy::trusted_local(root)
            .with_project_secret_deny()
            .secret_deny_paths_dynamic()
    };

    // (a) linked worktree: absolute gitdir, objects reached via `commondir`.
    mk("mainrepo/.git/objects/ff/eeee", "blob");
    mk("mainrepo/.git/worktrees/wt/commondir", "../..\n");
    let linked = outer.path().join("linked");
    std::fs::create_dir_all(&linked).unwrap();
    std::fs::write(
        linked.join(".git"),
        format!(
            "gitdir: {}\n",
            outer.path().join("mainrepo/.git/worktrees/wt").display()
        ),
    )
    .unwrap();
    assert!(
        denies(
            &deny_for(&linked),
            &outer.path().join("mainrepo/.git/objects")
        ),
        "a linked worktree's external object store must be denied"
    );

    // (b) submodule shape: a RELATIVE gitdir, resolved against the worktree.
    mk("realgit/objects/aa/bb", "blob");
    let sub = outer.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join(".git"), "gitdir: ../realgit\n").unwrap();
    assert!(
        denies(&deny_for(&sub), &outer.path().join("realgit/objects")),
        "a relative gitdir must resolve against the worktree, not the cwd"
    );

    // (c) `git clone --shared`: the store is BORROWED through alternates.
    mk("shared/objects/cc/dd", "blob");
    mk(
        "altrepo/.git/objects/info/alternates",
        &format!("{}\n", outer.path().join("shared/objects").display()),
    );
    let alt = outer.path().join("altrepo");
    let alt_deny = deny_for(&alt);
    assert!(
        denies(&alt_deny, &alt.join(".git/objects")),
        "CONTROL: the repo's own store is denied even when it borrows"
    );
    assert!(
        denies(&alt_deny, &outer.path().join("shared/objects")),
        "a borrowed alternate object store must be denied too"
    );

    // WRONG-REFUSAL / no-op CONTROL: a workspace under no VCS at all must gain
    // nothing from any of this. Compared against the SAME policy without the
    // project-secret opt-in so the always-on system entries cancel out.
    mk("plain/src/main.rs", "fn main() {}");
    let plain = outer.path().join("plain");
    assert_eq!(
        deny_for(&plain),
        WorkspacePolicy::trusted_local(&plain).secret_deny_paths_dynamic(),
        "a workspace with no VCS metadata must gain no deny entries"
    );
}

// ---------------------------------------------------------------------------
// FerroxLabs/wayland#1104 — the WRITE-only grant predicates.
//
// Unit-graded here, in the crate, because two of them are pure and one takes an
// injected budget: an integration test would have to reach them through a real
// `$HOME` and a twenty-thousand-file fixture, and both of those grade the
// environment as much as the code.
// ---------------------------------------------------------------------------

/// Overlap is symmetric, and each direction fails differently.
///
/// `dir` inside `known` hands over the auto-run directory itself; `dir`
/// containing `known` hands over a directory that CONTAINS it. A containment
/// check written as one `starts_with` catches only the first, and the second is
/// the likelier request ("grant me ~/.config").
#[test]
fn auto_run_overlap_is_symmetric_and_component_wise() {
    let autostart = Path::new("/home/u/.config/autostart");

    assert!(paths_overlap(autostart, autostart), "identity overlaps");
    assert!(
        paths_overlap(Path::new("/home/u/.config/autostart/deep"), autostart),
        "a directory INSIDE an auto-run location overlaps it"
    );
    assert!(
        paths_overlap(Path::new("/home/u/.config"), autostart),
        "a directory that CONTAINS an auto-run location overlaps it"
    );

    // WRONG-REFUSAL CONTROL. Both of these are the ordinary user folder the
    // whole feature exists to grant, and a prefix compared byte-wise rather
    // than component-wise would refuse the second one.
    assert!(
        !paths_overlap(Path::new("/home/u/Downloads"), autostart),
        "an unrelated sibling does not overlap"
    );
    assert!(
        !paths_overlap(Path::new("/home/u/.configuration"), autostart),
        "a longer name that merely SHARES a prefix is a different directory"
    );
}

/// A path inside a `.git` is the hook surface reached from below, and
/// `is_repo_control_path` cannot see it — that predicate is deliberately
/// scoped to THIS workspace, and a granted folder is by definition elsewhere.
#[test]
fn a_path_inside_a_git_directory_is_an_auto_run_location() {
    assert!(auto_run_overlap(Path::new("/srv/proj/.git/hooks")).is_some());
    assert!(auto_run_overlap(Path::new("/srv/proj/.git")).is_some());

    // WRONG-REFUSAL CONTROL: component-wise, so a directory whose NAME merely
    // contains the string is ordinary user data.
    assert!(auto_run_overlap(Path::new("/srv/proj/my.git-notes")).is_none());
    assert!(auto_run_overlap(Path::new("/srv/proj/src")).is_none());
}

/// The auto-run LISTS, which the `.git` case above never reaches.
///
/// `auto_run_overlap` refuses through two branches, and only the first was
/// graded: a path carrying a `.git` component returns before the
/// `AUTO_RUN_HOME_DIRS` / `AUTO_RUN_SYSTEM_DIRS` lookup is ever consulted.
/// MEASURED — emptying both arrays outright, or breaking the
/// `dirs::home_dir()` join that turns the relative entries into absolute ones,
/// left the entire crate suite green, while `~/.config` became write-grantable
/// and the agent could drop a `.desktop` file into `~/.config/autostart`.
///
/// Rule 2 of #1104 names four auto-run locations. Three of them —
/// `~/Library/LaunchAgents`, `~/.config/autostart`, and the Windows Startup
/// folder — exist ONLY as entries in these arrays, so this is the only place
/// they can be graded at all. They are asserted BY NAME and not merely swept
/// by the loop below, because a loop over an emptied array runs zero
/// iterations and passes: a sweep grades additions, never deletions.
#[test]
fn every_auto_run_list_entry_is_refused_in_both_directions() {
    let home = dirs::home_dir().expect(
        "the relative half of this list is only as good as `dirs::home_dir()`; \
         with no home it silently stops applying altogether, so a host without \
         one must FAIL here rather than skip",
    );
    let home = canon_for_scope(&home);

    // Both directions, and they fail differently: the location itself hands
    // over the auto-run directory, a parent hands over a directory that
    // CONTAINS it. #1104 calls out the ancestor case specifically because it
    // is the likelier user request ("grant me ~/.config").
    let both_ways = |dir: PathBuf, parent: PathBuf, what: &str| {
        assert!(
            auto_run_overlap(&dir).is_some(),
            "{what}: the auto-run location itself must be refused ({})",
            dir.display()
        );
        assert!(
            auto_run_overlap(&parent).is_some(),
            "{what}: a directory that CONTAINS it must be refused too ({})",
            parent.display()
        );
    };

    // The rule-2 locations that live ONLY in the arrays, by name.
    both_ways(
        home.join("Library/LaunchAgents"),
        home.join("Library"),
        "macOS LaunchAgents",
    );
    both_ways(
        home.join(".config/autostart"),
        home.join(".config"),
        "freedesktop autostart",
    );
    both_ways(
        home.join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Startup"),
        home.join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs"),
        "Windows Startup folder",
    );
    // At least one absolute entry, which needs no home at all and so is still
    // graded on a host where `home_dir()` is a lie.
    both_ways(
        PathBuf::from("/etc/cron.d"),
        PathBuf::from("/etc"),
        "system cron.d",
    );

    // ...and then the whole of both lists, so an entry added later is graded
    // on the day it is added rather than the day it is exploited.
    for relative in AUTO_RUN_HOME_DIRS {
        let dir = home.join(relative);
        let parent = dir
            .parent()
            .expect("a joined home-relative entry always has a parent")
            .to_path_buf();
        both_ways(dir, parent, relative);
    }
    for absolute in AUTO_RUN_SYSTEM_DIRS {
        let dir = PathBuf::from(absolute);
        let parent = dir
            .parent()
            .expect("every system entry has at least two components")
            .to_path_buf();
        both_ways(dir, parent, absolute);
    }

    // WRONG-REFUSAL CONTROL. `~/Downloads` is the ticket's own worked example
    // of the folder a user grants, and it must stay grantable — without this,
    // the test above passes just as well against a predicate that refuses
    // everything, which is the other way to fail #1104.
    assert!(
        auto_run_overlap(&home.join("Downloads")).is_none(),
        "the folder the whole feature exists to grant is not an auto-run location"
    );
    assert!(
        auto_run_overlap(&home.join("Documents/reports")).is_none(),
        "nor is an ordinary nested project directory under home"
    );
}

/// The Windows extension rules are live on Linux and macOS too, which is the
/// only reason they can be graded at all — CI cannot introspect the one
/// platform they describe.
#[test]
fn executable_detection_covers_both_platform_families() {
    let dir = tempfile::tempdir().unwrap();

    let by_extension = dir.path().join("installer.EXE");
    std::fs::write(&by_extension, b"MZ").unwrap();
    let metadata = std::fs::symlink_metadata(&by_extension).unwrap();
    assert!(
        entry_is_executable(&by_extension, &metadata),
        "a Windows executable has no unix mode bit and must still be caught, \
         case-folded"
    );

    let plain = dir.path().join("report.pdf");
    std::fs::write(&plain, b"%PDF").unwrap();
    let metadata = std::fs::symlink_metadata(&plain).unwrap();
    assert!(
        !entry_is_executable(&plain, &metadata),
        "WRONG-REFUSAL CONTROL: ordinary user data is not an executable"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.path().join("run");
        std::fs::write(&script, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let metadata = std::fs::symlink_metadata(&script).unwrap();
        assert!(
            entry_is_executable(&script, &metadata),
            "an ELF or a +x script has no extension at all — the mode bit is \
             what catches it"
        );

        assert!(
            !entry_is_executable(dir.path(), &std::fs::symlink_metadata(dir.path()).unwrap()),
            "WRONG-REFUSAL CONTROL: every directory carries the exec bit, and \
             calling one an executable would refuse every grant there is"
        );
    }
}

/// The scan refuses a root already holding something runnable, and passes an
/// ordinary documents folder.
#[test]
fn scan_write_root_refuses_only_what_it_should() {
    let clean = tempfile::tempdir().unwrap();
    std::fs::create_dir(clean.path().join("2026")).unwrap();
    std::fs::write(clean.path().join("2026/brief.pdf"), b"%PDF").unwrap();
    std::fs::write(clean.path().join("notes.md"), b"# hi").unwrap();
    scan_write_root(clean.path())
        .expect("WRONG-REFUSAL CONTROL: an ordinary documents folder is grantable");

    let with_exe = tempfile::tempdir().unwrap();
    std::fs::create_dir(with_exe.path().join("nested")).unwrap();
    std::fs::write(with_exe.path().join("nested/setup.msi"), b"x").unwrap();
    assert!(
        matches!(
            scan_write_root(with_exe.path()),
            Err(PathGrantError::WriteRootExecutable(_))
        ),
        "the walk descends: an executable one level down is still one the \
         operator can run"
    );

    let with_secret = tempfile::tempdir().unwrap();
    std::fs::write(with_secret.path().join(".env"), b"K=v").unwrap();
    assert!(matches!(
        scan_write_root(with_secret.path()),
        Err(PathGrantError::WriteRootSecret(_))
    ));

    let with_git = tempfile::tempdir().unwrap();
    std::fs::create_dir(with_git.path().join(".git")).unwrap();
    assert!(
        matches!(
            scan_write_root(with_git.path()),
            Err(PathGrantError::WriteRootAutoRun(_))
        ),
        "`.git/hooks` and `.git/config` both run on ordinary developer \
         commands, and this `.git` is outside the workspace so \
         `is_repo_control_path` never sees it"
    );
}

/// A symlink is not the executable the operator later runs, and following it
/// would misattribute a file that lives somewhere else entirely.
///
/// Not a hole: writing THROUGH the link is refused by both enforcement layers,
/// which resolve before they compare — `SandboxedFs` canonicalizes out of the
/// grant, and the OS sandbox never bound the target's directory writable.
#[test]
#[cfg(unix)]
fn the_scan_does_not_follow_symlinks_out_of_the_root() {
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/bin/sh", dir.path().join("shell")).unwrap();
    scan_write_root(dir.path())
        .expect("a link to an executable is a link, not an executable in this folder");

    let dangling = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/nonexistent/f1104", dangling.path().join("broken")).unwrap();
    scan_write_root(dangling.path())
        .expect("a DANGLING link must not make the scan fail closed on an io error");
}

/// Exhausting the budget is a REFUSAL. The scan cannot prove the absence of an
/// executable it never reached, and "could not check" must never read as
/// "checked and clean".
#[test]
fn an_unscannably_large_root_is_refused_not_waved_through() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..5 {
        std::fs::write(dir.path().join(format!("f{index}.txt")), b"x").unwrap();
    }
    assert!(
        matches!(
            scan_write_root_bounded(dir.path(), 2),
            Err(PathGrantError::WriteRootTooLarge(_, 2))
        ),
        "and the refusal reports the budget it actually used"
    );
    // WRONG-REFUSAL CONTROL: the same tree, under a budget that covers it.
    scan_write_root_bounded(dir.path(), 5).expect("a root inside the budget is scanned and passes");
}
