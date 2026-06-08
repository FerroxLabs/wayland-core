//! Wave SD — path validation for the legacy (non-`_with_ctx`) entry
//! points on `ReadTool` / `WriteTool` / `EditTool`.
//!
//! Closes SECURITY MAJOR #14 and INFORMATIONAL #25:
//!
//! * #14 — `Read/Write/Edit::execute()` (the non-ctx legacy path)
//!   accepted arbitrary absolute paths. `Read { file_path: "/etc/shadow" }`
//!   returned the file's bytes if the OS let the user read them.
//! * #25 — `validate_memory_path` exists but was never invoked by the
//!   file tools. We replicate its safety checks here (absolute,
//!   non-traversal, no null bytes) because `wcore-tools` doesn't depend
//!   on `wcore-memory` (and shouldn't — wcore-memory depends on wcore-
//!   config which depends on no other internal crates).
//!
//! Strategy:
//!
//! The legacy entries don't have a `ToolContext` and therefore no
//! sandbox-rooted `VirtualFs` to clamp against. So we apply the same
//! shape check `validate_memory_path` would:
//!
//!   1. Path must be absolute. The schema documents this; we enforce.
//!   2. Path must not contain null bytes.
//!   3. Path must not contain `..` traversal segments (after lexical
//!      normalization).
//!   4. Path must canonicalize to a real prefix that does not point at
//!      an obvious OS-secret target (we maintain a small deny-list of
//!      sensitive system paths — `/etc/shadow`, `/etc/sudoers`,
//!      `~/.ssh`, `~/.aws/credentials`, etc.). This is defence-in-depth;
//!      the absolute-path discipline is the primary boundary.
//!
//! Callers route both `execute()` and `execute_with_ctx()` through
//! `validate_user_path()`; the ctx path additionally clamps via the
//! `SandboxedFs` containment check.

use std::fs;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathValidationError {
    #[error("path must be absolute: {0:?}")]
    NotAbsolute(PathBuf),
    #[error("path contains null byte: {0:?}")]
    NullByte(PathBuf),
    #[error("path contains traversal (..): {0:?}")]
    Traversal(PathBuf),
    #[error("path targets a denied system location: {0:?}")]
    SystemPath(PathBuf),
}

/// Validate an LLM-supplied path before any filesystem touch.
///
/// Returns the lex-normalized `PathBuf` on success. On failure the
/// error carries the offending input so the calling tool can surface
/// a clear refusal back to the model.
pub fn validate_user_path(path: &Path) -> Result<PathBuf, PathValidationError> {
    let raw = path.to_path_buf();

    let path_str = path.to_string_lossy();
    if path_str.contains('\0') {
        return Err(PathValidationError::NullByte(raw));
    }

    if !path.is_absolute() {
        return Err(PathValidationError::NotAbsolute(raw));
    }

    // Traversal segments — string-form check matches `validate_memory_path`'s
    // approach: any literal `..` component is refused before we even
    // try to canonicalize. Avoids the "normalize first, then check"
    // class of bypass.
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(PathValidationError::Traversal(raw));
    }

    let normalized = lex_normalize(path);

    // First-pass lexical deny check on the literal (lex-normalized) path.
    if is_denied_system_path(&normalized) {
        return Err(PathValidationError::SystemPath(normalized));
    }

    // M-8 / tools-io-17: the lexical check above is bypassable by an
    // innocuously-named symlink — `ln -s ~/.ssh/id_rsa /tmp/work/notes.txt`
    // then `Read {file_path:"/tmp/work/notes.txt"}` passes the string
    // denylist while RealFs follows the link straight to the key. Resolve
    // the longest EXISTING prefix (which follows symlinks) and re-run the
    // deny check against the canonical target, mirroring
    // `SandboxedFs::canonicalize_existing_prefix` in `vfs.rs`. Write/Edit
    // targets whose leaf does not yet exist canonicalize their parent dir,
    // so a symlinked parent is still caught.
    if let Some(resolved) = canonicalize_existing_prefix(&normalized)
        && resolved != normalized
        && is_denied_system_path(&resolved)
    {
        return Err(PathValidationError::SystemPath(resolved));
    }

    // Defense-in-depth: a symlink whose target does NOT yet exist makes
    // `fs::canonicalize` fail, so `canonicalize_existing_prefix` falls back to
    // the link's own name and the deny check never sees the target — e.g.
    // `notes.txt -> $WAYLAND_HOME/cron/jobs.json` (or `~/.ssh/id_rsa`) before
    // the target exists. Resolve a symlink leaf explicitly (bounded hops, even
    // through a dangling final target) and re-run the deny check, so the
    // guarantee lives here rather than relying on a calling tool's write
    // mechanics (atomic rename-over-symlink).
    if let Some(link_target) = resolve_symlink_target(&normalized)
        && is_denied_system_path(&link_target)
    {
        return Err(PathValidationError::SystemPath(link_target));
    }

    Ok(normalized)
}

/// If `path` is a symlink, follow it (up to 8 hops) to an absolute,
/// lex-normalized target — even when the final target does not exist, which
/// defeats `fs::canonicalize`. Returns `None` when `path` is not a symlink (or
/// its parent does not exist). Used as a deny-check backstop for dangling
/// symlink leaves.
fn resolve_symlink_target(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    let mut followed = false;
    for _ in 0..8 {
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = fs::read_link(&current).ok()?;
                current = if target.is_absolute() {
                    target
                } else {
                    current.parent().map(|p| p.join(&target)).unwrap_or(target)
                };
                current = lex_normalize(&current);
                followed = true;
            }
            // Not a symlink, or the target does not exist yet — stop.
            _ => break,
        }
    }
    followed.then_some(current)
}

/// Resolve the longest existing ancestor of `path` (following symlinks via
/// `fs::canonicalize`) and re-attach the trailing not-yet-existing suffix.
/// Returns `None` when no ancestor resolves. Replicates the minimal logic
/// of `vfs::canonicalize_existing_prefix` locally so the file-tool deny
/// check can resolve symlink targets without depending on the sandbox VFS.
fn canonicalize_existing_prefix(path: &Path) -> Option<PathBuf> {
    let mut p: &Path = path;
    loop {
        if let Ok(canon) = fs::canonicalize(p) {
            let suffix = path.strip_prefix(p).unwrap_or(Path::new(""));
            return Some(if suffix.as_os_str().is_empty() {
                canon
            } else {
                canon.join(suffix)
            });
        }
        p = p.parent()?;
    }
}

/// Defence-in-depth deny-list of paths the LLM should never read or
/// write through the top-level legacy execute() entry. The sandbox
/// containment check handles sub-agent confinement; this list catches
/// the obvious "I've been prompt-injected to read your secrets"
/// pattern at the root agent layer.
fn is_denied_system_path(path: &Path) -> bool {
    let s = path.to_string_lossy();

    // Universal: anything under /etc that smells like creds.
    const DENIED_PREFIXES: &[&str] = &[
        "/etc/shadow",
        "/etc/sudoers",
        "/etc/sudoers.d",
        "/etc/ssh/ssh_host_",
        "/private/etc/shadow",
        "/private/etc/sudoers",
        "/private/var/db/sudo",
    ];
    if DENIED_PREFIXES.iter().any(|p| s.starts_with(p)) {
        return true;
    }

    // User-home secret stashes — normalize any HOME-relative form to the
    // raw absolute path, then check suffix.
    //
    // v0.6.2 cross-audit Round 1: added authorized_keys + known_hosts + id_dsa
    // to close the read-path gap surfaced by the Tier 3 audit. file_safety.rs
    // already blocks writes to these, but path_validation.rs is the read-path
    // guard and was missing them.
    const DENIED_SUFFIXES: &[&str] = &[
        "/.ssh/id_rsa",
        "/.ssh/id_ed25519",
        "/.ssh/id_ecdsa",
        "/.ssh/id_dsa",
        "/.ssh/authorized_keys",
        "/.ssh/known_hosts",
        "/.aws/credentials",
        "/.gnupg/private-keys-v1.d",
        "/.kube/config",
        // F-054: Wayland-Core own credential files — a prompt-injected agent
        // must not be able to Read the engine's stored secrets back to the model.
        "/.config/wayland-core/credentials.toml",
        "/.wayland/credentials.toml",
        "/wayland-core/auth.json",
        "/wayland-core/credentials.enc",
        "/wayland-core/credentials.key.json",
        // M-19: cron state directory (`~/.wayland/cron/` — `jobs.json` +
        // `.integrity.key`). store.rs gates loading on ownership/0600 + a keyed
        // integrity tag, but a same-uid prompt-injected agent with Write/Edit
        // could still author this file directly. Deny the whole dir so the
        // agent-facing file tools refuse to touch it.
        "/.wayland/cron/",
        // Broad per-app credential files used by common developer tooling.
        "/.netrc",
        "/.npmrc",
        "/.pypirc",
        "/.docker/config.json",
        "/.gcloud/credentials.db",
        "/.azure/",
    ];
    if DENIED_SUFFIXES.iter().any(|sfx| s.contains(sfx)) {
        return true;
    }

    // M-19 (residual bypass): the `/.wayland/cron/` suffix above only matches
    // the DEFAULT cron dir. The cron store resolves `$WAYLAND_HOME` first
    // (`wcore_cron::store::default_store_path`), so a relocated home puts
    // `jobs.json` + `.integrity.key` somewhere the substring never matches —
    // letting a same-uid prompt-injected agent author a Trusted cron job
    // directly. Derive the cron dir from the SAME env resolution the store
    // uses and deny anything within it (component-wise, no sibling-prefix bug).
    if resolved_cron_dirs()
        .iter()
        .any(|cron_dir| path.starts_with(cron_dir))
    {
        return true;
    }

    false
}

/// The cron state directory(ies), resolved exactly as the cron store resolves
/// it: `$WAYLAND_HOME/cron` when set, else `~/.wayland/cron`. Mirrors
/// `wcore_cron::store::default_store_path`; `wcore-tools` must not depend on
/// `wcore-cron`, so the resolution is duplicated rather than imported.
///
/// Returns BOTH the raw (as-configured) dir and, when it differs, the
/// canonical (symlink-resolved) dir. `validate_user_path` deny-checks the
/// request path in both its lexical and canonicalized forms, so a symlinked
/// `WAYLAND_HOME` is caught whichever way the agent spells the target: a
/// write via the canonical real path matches the canonical entry, while a
/// write via the symlink path matches the raw entry. Without the canonical
/// entry, a symlinked home let a write to the real inode slip past the
/// lexical compare.
fn resolved_cron_dirs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("WAYLAND_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".wayland")))
    else {
        return Vec::new();
    };
    let raw = home.join("cron");
    let mut dirs = vec![raw.clone()];
    // Canonicalize the home (more likely to exist than the cron subdir on
    // first run) and re-derive; fall back silently when it does not resolve.
    if let Ok(canon_home) = fs::canonicalize(&home) {
        let canon = canon_home.join("cron");
        if canon != raw {
            dirs.push(canon);
        }
    }
    dirs
}

fn lex_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                out.push(c.as_os_str());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_rejected() {
        let err = validate_user_path(Path::new("relative/file.txt")).unwrap_err();
        assert!(matches!(err, PathValidationError::NotAbsolute(_)));
    }

    #[cfg(unix)]
    #[test]
    fn traversal_rejected() {
        // Absolute path with `..` inside still flagged before lex-normalize
        // collapses it.
        let err = validate_user_path(Path::new("/tmp/../etc/passwd")).unwrap_err();
        assert!(matches!(err, PathValidationError::Traversal(_)));
    }

    #[cfg(unix)]
    #[test]
    fn null_byte_rejected() {
        let s = "/tmp/before\0after.txt";
        let err = validate_user_path(Path::new(s)).unwrap_err();
        assert!(matches!(err, PathValidationError::NullByte(_)));
    }

    #[cfg(unix)]
    #[test]
    fn system_etc_shadow_rejected() {
        let err = validate_user_path(Path::new("/etc/shadow")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    // These tests exercise unix path semantics — `/home/alice/.ssh/id_rsa`
    // isn't classified as a system path on Windows (where SSH lives under
    // `%USERPROFILE%\.ssh\`), and `/tmp/wcore/...` isn't an absolute path
    // on Windows at all (which wants `C:\...`). Gate to cfg(unix). The
    // Windows-equivalent test would need entirely different fixtures
    // (`C:\Users\...`, `C:\Windows\System32\config\SAM`) — out of scope
    // for Wave A CI unblock.
    #[cfg(unix)]
    #[test]
    fn ssh_private_key_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.ssh/id_rsa")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn ssh_authorized_keys_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.ssh/authorized_keys")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn ssh_known_hosts_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.ssh/known_hosts")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn ssh_id_dsa_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.ssh/id_dsa")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_absolute_path_allowed() {
        let p = validate_user_path(Path::new("/tmp/wcore/work.txt")).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/wcore/work.txt"));
    }

    // F-054: Wayland-Core own credential files must be blocked.
    #[cfg(unix)]
    #[test]
    fn wayland_core_credentials_toml_rejected() {
        let err = validate_user_path(Path::new(
            "/home/alice/.config/wayland-core/credentials.toml",
        ))
        .unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn wayland_credentials_toml_rejected() {
        let err =
            validate_user_path(Path::new("/home/alice/.wayland/credentials.toml")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    // M-19: cron state dir must be refused on the read/write path so a
    // same-uid prompt-injected agent cannot author jobs.json directly.
    #[cfg(unix)]
    #[test]
    fn wayland_cron_jobs_json_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.wayland/cron/jobs.json")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn wayland_cron_integrity_key_rejected() {
        let err =
            validate_user_path(Path::new("/home/alice/.wayland/cron/.integrity.key")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    // M-19 (residual bypass): with WAYLAND_HOME relocated, the cron store no
    // longer lives under `~/.wayland/cron`, so the hardcoded substring missed
    // it. The deny-list must derive the cron dir from the same env the store
    // reads. The literal `/home/alice/.wayland/...` tests above prove the
    // default path stays denied regardless of this env var.
    #[cfg(unix)]
    #[test]
    fn wayland_cron_relocated_home_jobs_and_key_rejected() {
        // SAFETY: single-threaded test setup; no other test mutates this var.
        unsafe { std::env::set_var("WAYLAND_HOME", "/srv/wl-relocated-test") };
        let jobs = validate_user_path(Path::new("/srv/wl-relocated-test/cron/jobs.json"));
        let key = validate_user_path(Path::new("/srv/wl-relocated-test/cron/.integrity.key"));
        unsafe { std::env::remove_var("WAYLAND_HOME") };
        assert!(
            matches!(jobs, Err(PathValidationError::SystemPath(_))),
            "relocated cron jobs.json must be denied, got {jobs:?}"
        );
        assert!(
            matches!(key, Err(PathValidationError::SystemPath(_))),
            "relocated cron .integrity.key must be denied, got {key:?}"
        );
    }

    // M-19 (residual of the residual): a symlinked WAYLAND_HOME let a write to
    // the canonical cron inode slip past the raw-string compare. The
    // comparator now also canonicalizes, so the canonical write path is denied.
    #[cfg(unix)]
    #[test]
    fn wayland_cron_symlinked_home_canonical_write_rejected() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("wl-cron-symlink-{}", std::process::id()));
        let realhome = base.join("realhome");
        let cron = realhome.join("cron");
        fs::create_dir_all(&cron).expect("create cron dir");
        let link = base.join("link");
        let _ = fs::remove_file(&link);
        symlink(&realhome, &link).expect("symlink link -> realhome");

        // SAFETY: single-threaded test setup; restored below.
        unsafe { std::env::set_var("WAYLAND_HOME", &link) };
        // The agent writes via the CANONICAL real path, which under the raw
        // (symlink) comparator did not match `link/cron`.
        let res = validate_user_path(&cron.join("jobs.json"));
        unsafe { std::env::remove_var("WAYLAND_HOME") };
        let _ = fs::remove_dir_all(&base);

        assert!(
            matches!(res, Err(PathValidationError::SystemPath(_))),
            "write to the canonical cron dir under a symlinked WAYLAND_HOME must be denied, got {res:?}"
        );
    }

    // Defense-in-depth: a symlink leaf pointing at a DENIED target whose target
    // does not yet exist (dangling) used to slip past — canonicalize fails and
    // the fallback keeps the link's own name. resolve_symlink_target now
    // follows the dangling link and the deny check catches it.
    #[cfg(unix)]
    #[test]
    fn dangling_symlink_leaf_to_denied_target_rejected() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("wl-dangling-{}", std::process::id()));
        let work = base.join("work");
        fs::create_dir_all(&work).expect("create work dir");
        // Target does NOT exist (dangling) but matches a denied suffix.
        let denied_target = base.join("victim/.ssh/id_rsa");
        let link = work.join("notes.txt");
        let _ = fs::remove_file(&link);
        symlink(&denied_target, &link).expect("create dangling symlink");

        let res = validate_user_path(&link);
        let _ = fs::remove_dir_all(&base);

        assert!(
            matches!(res, Err(PathValidationError::SystemPath(_))),
            "dangling symlink leaf pointing at a denied target must be rejected, got {res:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn wayland_auth_json_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.config/wayland-core/auth.json"))
            .unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn netrc_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.netrc")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn npmrc_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.npmrc")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn pypirc_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.pypirc")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn docker_config_json_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.docker/config.json")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn gcloud_credentials_db_rejected() {
        let err = validate_user_path(Path::new("/home/alice/.gcloud/credentials.db")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn azure_credentials_rejected() {
        let err =
            validate_user_path(Path::new("/home/alice/.azure/accessTokens.json")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    // M-8 / tools-io-17: an innocuously-named symlink whose canonical target
    // is a denied credential file must be refused. The lexical denylist
    // passes (the link name is benign), so this asserts the symlink-resolving
    // prefix canonicalization closes the hole.
    #[cfg(unix)]
    #[test]
    fn symlink_named_path_to_ssh_key_rejected() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "wcore_pathval_symlink_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let ssh_dir = base.join(".ssh");
        std::fs::create_dir_all(&ssh_dir).unwrap();
        let real_key = ssh_dir.join("id_rsa");
        std::fs::write(&real_key, b"PRIVATE KEY").unwrap();

        let work = base.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let innocuous = work.join("notes.txt");
        symlink(&real_key, &innocuous).unwrap();

        // The link name `notes.txt` is not on the lexical denylist, but its
        // canonical target ends in `/.ssh/id_rsa` and MUST be refused.
        let err = validate_user_path(&innocuous).unwrap_err();
        assert!(
            matches!(err, PathValidationError::SystemPath(_)),
            "symlink to ssh key must be denied, got {err:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // Companion: a symlink to a benign file is still allowed (no
    // false-positive from the canonicalization pass).
    #[cfg(unix)]
    #[test]
    fn symlink_to_benign_file_allowed() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "wcore_pathval_benign_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let real = base.join("data.txt");
        std::fs::write(&real, b"hello").unwrap();
        let link = base.join("alias.txt");
        symlink(&real, &link).unwrap();

        assert!(
            validate_user_path(&link).is_ok(),
            "symlink to benign file must be allowed"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
