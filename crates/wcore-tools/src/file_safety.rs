//! T3-3.2.2 — Shared file-safety rules ported from the prior Wayland
//! Python engine.
//!
//! This is a *helper* module, not a tool. It exposes:
//!
//!   * [`is_write_denied`] — would-be writes to known-sensitive paths
//!     (SSH/AWS/GnuPG/k8s creds, `~/.bashrc`, `/etc/sudoers`, etc.) and
//!     paths outside the optional `WAYLAND_WRITE_SAFE_ROOT` are refused.
//!   * [`get_read_block_error`] — reads targeting the internal Wayland
//!     skill-hub cache return a refusal string instead of leaking the
//!     prompt-injection-prone contents.
//!   * [`sensitive_patterns`] — flat view of the deny list (exact paths
//!     followed by directory prefixes ending in the platform separator)
//!     for callers that want to inspect or render it.
//!
//! Relationship to the existing [`crate::path_validation`] module:
//! `path_validation` enforces *shape* invariants (absolute, no NUL, no
//! `..` traversal) plus a small built-in deny-list for `/etc/shadow` and
//! a handful of SSH/AWS suffixes. `file_safety` is broader and
//! configurable — it adds shell-rc files, sudoers.d, the safe-root
//! env-gate, and the read-block list for the skill cache. Tools that
//! need both should call `validate_user_path` first (shape) and
//! `is_write_denied` / `get_read_block_error` second (policy).
//!
//! ## Design notes that differ from the Python original
//!
//! * Symlink resolution: the Python original uses
//!   `Path(...).resolve(strict=False)` plus `os.path.realpath`. The
//!   Rust port uses `std::fs::canonicalize` when the path exists and a
//!   lexical normalization (collapsing `.` / `..`) otherwise — this
//!   matches `wcore_tools::path_validation::lex_normalize` and lets us
//!   refuse a write-then-create on a sensitive path *before* the file
//!   ever lands on disk.
//! * `WAYLAND_WRITE_SAFE_ROOT` is resolved with `shellexpand`-style
//!   `~` expansion and then canonicalized once per call (cheap — env
//!   vars are read fresh so tests can flip them).
//! * `_wayland_home_path` is replaced by an injected `wayland_home`
//!   argument on the lower-level builders, defaulting to
//!   `wcore_config::config::app_config_dir()` in the top-level entry
//!   points. Tests pass a synthetic path; production callers don't need
//!   to.

use std::path::{Component, Path, PathBuf};

use wcore_config::config::app_config_dir;

/// Env var that, when set, restricts all writes to paths underneath the
/// expanded value. An unset or empty value disables the restriction.
pub const WRITE_SAFE_ROOT_ENV: &str = "WAYLAND_WRITE_SAFE_ROOT";

/// Build the set of *exact* paths that must never be written. Paths are
/// canonicalized (when they exist) so symlink aliasing can't bypass the
/// check.
///
/// `home` should be the canonicalized user home directory.
/// `wayland_home` should be the canonicalized application config dir
/// (typically `~/.config/wayland-core` on Linux, the platform analog
/// elsewhere).
pub fn build_write_denied_paths(home: &Path, wayland_home: &Path) -> Vec<PathBuf> {
    let mut raw: Vec<PathBuf> = vec![
        // Cross-platform: SSH keys, shell rcs, package-manager credentials
        // and the wayland-core dotenv. These appear under the user's home
        // directory regardless of OS — Git-for-Windows places `.ssh` under
        // `%USERPROFILE%\.ssh`, dotfiles under `%USERPROFILE%`, etc.
        home.join(".ssh").join("authorized_keys"),
        home.join(".ssh").join("id_rsa"),
        home.join(".ssh").join("id_ed25519"),
        home.join(".ssh").join("config"),
        wayland_home.join(".env"),
        home.join(".bashrc"),
        home.join(".zshrc"),
        home.join(".profile"),
        home.join(".bash_profile"),
        home.join(".zprofile"),
        home.join(".netrc"),
        home.join(".pgpass"),
        home.join(".npmrc"),
        home.join(".pypirc"),
    ];
    #[cfg(unix)]
    {
        raw.push(PathBuf::from("/etc/sudoers"));
        raw.push(PathBuf::from("/etc/passwd"));
        raw.push(PathBuf::from("/etc/shadow"));
    }
    #[cfg(windows)]
    {
        // Windows-specific sensitive files. Paths follow standard install
        // locations; checks use lex_normalize so the deny list still
        // matches when the host has a different SystemDrive or expanded
        // environment.
        //
        // If WINDIR or APPDATA is unset or empty, we MUST NOT silently
        // drop SAM / AWS credential paths from the deny list — that's a
        // load-bearing security gap. Fall back to canonical defaults and
        // emit a warn so the operator can fix the environment.
        let windir = windows_dir_with_fallback();
        raw.push(windir.join("System32").join("config").join("SAM"));
        raw.push(windir.join("System32").join("config").join("SECURITY"));
        raw.push(windir.join("System32").join("config").join("SYSTEM"));

        let appdata = appdata_dir_with_fallback();
        raw.push(appdata.join(".aws").join("credentials"));
        // Git-for-Windows .gitconfig / DPAPI master key directory are
        // covered by the prefix list (`.aws`, `.gnupg`); the explicit
        // paths above are the leaf files an attacker would target.
    }
    raw.into_iter()
        .map(|p| canonicalize_for_check(&p))
        .collect()
}

/// Build the list of directory *prefixes* under which no write may
/// land. Each returned path has a trailing path separator so a startswith
/// check can't false-match a longer name (e.g. `~/.sshrc` vs `~/.ssh`).
pub fn build_write_denied_prefixes(home: &Path) -> Vec<PathBuf> {
    let mut raw: Vec<PathBuf> = vec![
        home.join(".ssh"),
        home.join(".aws"),
        home.join(".gnupg"),
        home.join(".kube"),
        home.join(".docker"),
        home.join(".azure"),
        home.join(".config").join("gh"),
    ];
    #[cfg(unix)]
    {
        raw.push(PathBuf::from("/etc/sudoers.d"));
        raw.push(PathBuf::from("/etc/systemd"));
    }
    #[cfg(windows)]
    {
        let windir = windows_dir_with_fallback();
        raw.push(windir.join("System32").join("config"));

        let appdata = appdata_dir_with_fallback();
        // DPAPI master key store + Crypto keys live under
        // %APPDATA%\Microsoft\Crypto / %APPDATA%\Microsoft\Protect.
        raw.push(appdata.join("Microsoft").join("Crypto"));
        raw.push(appdata.join("Microsoft").join("Protect"));
    }
    raw.into_iter()
        .map(|p| {
            let mut canon = canonicalize_for_check(&p).into_os_string();
            canon.push(std::path::MAIN_SEPARATOR_STR);
            PathBuf::from(canon)
        })
        .collect()
}

/// Read `WAYLAND_WRITE_SAFE_ROOT` from the environment, expand `~`, and
/// canonicalize. Returns `None` when unset, empty, or unresolvable.
pub fn get_safe_write_root() -> Option<PathBuf> {
    let raw = std::env::var(WRITE_SAFE_ROOT_ENV).ok()?;
    if raw.is_empty() {
        return None;
    }
    Some(canonicalize_for_check(&expand_tilde(&raw)))
}

/// Resolve `WINDIR` or fall back to `r"C:\Windows"` after warning.
/// Critical-path: the SAM/SECURITY/SYSTEM hive deny entries MUST exist
/// even when the host env is stripped (CI runner, service account,
/// post-`env -i` shells). Silently dropping them would unprotect the
/// registry hives.
#[cfg(windows)]
fn windows_dir_with_fallback() -> PathBuf {
    static WARNED: std::sync::Once = std::sync::Once::new();
    match std::env::var("WINDIR") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            WARNED.call_once(|| {
                tracing::warn!(
                    target: "wcore_tools::file_safety",
                    "WINDIR env var unset or empty; falling back to C:\\Windows for the \
                     Windows-specific write-deny list (SAM/SECURITY/SYSTEM hives + \
                     System32\\config prefix). Set WINDIR explicitly to silence this."
                );
            });
            PathBuf::from(r"C:\Windows")
        }
    }
}

/// Resolve `APPDATA` or fall back to `%USERPROFILE%\AppData\Roaming`
/// (or, if USERPROFILE is also missing, the conventional path under
/// `C:\Users\Default`). Same security rationale as `windows_dir_with_fallback`.
#[cfg(windows)]
fn appdata_dir_with_fallback() -> PathBuf {
    static WARNED: std::sync::Once = std::sync::Once::new();
    if let Ok(v) = std::env::var("APPDATA")
        && !v.is_empty()
    {
        return PathBuf::from(v);
    }
    WARNED.call_once(|| {
        tracing::warn!(
            target: "wcore_tools::file_safety",
            "APPDATA env var unset or empty; falling back to %USERPROFILE%\\AppData\\Roaming \
             for the Windows-specific write-deny list (AWS credentials + DPAPI key directories). \
             Set APPDATA explicitly to silence this."
        );
    });
    if let Some(home) = dirs::home_dir() {
        return home.join("AppData").join("Roaming");
    }
    PathBuf::from(r"C:\Users\Default\AppData\Roaming")
}

/// Resolve `path` for deny-list comparison.
///
/// Strategy: if the path exists, defer to `std::fs::canonicalize`
/// (which follows symlinks). For the typical "write then create" case
/// where the leaf doesn't exist yet, walk up to the deepest existing
/// ancestor, canonicalize that, and re-attach the remaining segments
/// after lexical normalization. Failing closed is *not* required here
/// because the caller still applies its own deny-list check on the
/// resulting path.
fn canonicalize_for_check(path: &Path) -> PathBuf {
    let expanded = if let Some(s) = path.to_str() {
        expand_tilde(s)
    } else {
        path.to_path_buf()
    };

    if let Ok(canon) = std::fs::canonicalize(&expanded) {
        return canon;
    }

    // Walk up to the deepest existing ancestor.
    let mut ancestor = expanded.as_path();
    let mut tail = PathBuf::new();
    loop {
        if ancestor.exists() {
            break;
        }
        match (ancestor.parent(), ancestor.file_name()) {
            (Some(parent), Some(name)) => {
                tail = {
                    let mut acc = PathBuf::from(name);
                    acc.push(&tail);
                    acc
                };
                ancestor = parent;
            }
            _ => break,
        }
    }

    let canon = std::fs::canonicalize(ancestor).unwrap_or_else(|_| expanded.clone());
    if tail.as_os_str().is_empty() {
        lex_normalize(&canon)
    } else {
        lex_normalize(&canon.join(tail))
    }
}

/// Expand a leading `~` (and `~/`) to the user's home directory. Other
/// tilde forms (e.g. `~user`) are left untouched because the deny
/// checks operate on the calling user's home only.
fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if raw == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(raw)
}

/// Collapse `.` / `..` segments lexically without touching the
/// filesystem. Matches the helper in `path_validation`.
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

/// Resolve the active Wayland config dir (`app_config_dir`) and fall
/// back to a benign placeholder so deny-list construction never panics
/// on a host without `dirs::config_dir`.
fn resolved_wayland_home() -> PathBuf {
    app_config_dir().unwrap_or_else(|| PathBuf::from("/nonexistent/wayland-core"))
}

/// Resolve the user home dir; fall back to a benign placeholder so
/// deny-list construction never panics in unusual environments.
fn resolved_user_home() -> PathBuf {
    dirs::home_dir()
        .map(|p| canonicalize_for_check(&p))
        .unwrap_or_else(|| PathBuf::from("/nonexistent/home"))
}

/// Return `true` if `path` is blocked by the write deny list or the
/// optional `WAYLAND_WRITE_SAFE_ROOT` clamp.
///
/// Symlinks and `..` segments are resolved before comparison so
/// attackers can't bypass via `/tmp/safe_link -> ~/.ssh`.
pub fn is_write_denied(path: &Path) -> bool {
    is_write_denied_with(path, &resolved_user_home(), &resolved_wayland_home())
}

/// Lower-level entry point with injected `home` / `wayland_home` —
/// public for tests and for callers that already hold resolved paths.
pub fn is_write_denied_with(path: &Path, home: &Path, wayland_home: &Path) -> bool {
    let resolved = canonicalize_for_check(path);
    if resolved.as_os_str().is_empty() {
        // Couldn't canonicalize at all — fail closed.
        return true;
    }

    for denied in build_write_denied_paths(home, wayland_home) {
        if resolved == denied {
            return true;
        }
    }
    let resolved_str = resolved.as_os_str().to_string_lossy().to_string();
    for prefix in build_write_denied_prefixes(home) {
        let prefix_str = prefix.as_os_str().to_string_lossy().to_string();
        if resolved_str.starts_with(&prefix_str) {
            return true;
        }
    }

    if let Some(safe_root) = get_safe_write_root() {
        let safe_str = safe_root.as_os_str().to_string_lossy().to_string();
        let mut safe_prefix = safe_str.clone();
        safe_prefix.push(std::path::MAIN_SEPARATOR);
        if resolved_str != safe_str && !resolved_str.starts_with(&safe_prefix) {
            return true;
        }
    }

    false
}

/// Public alias requested by the uplift slot contract — the human-
/// friendly name for the write deny check.
pub fn is_sensitive_path(path: &Path) -> bool {
    is_write_denied(path)
}

/// Return an error message when a read targets internal Wayland cache
/// files (the skill hub's index cache, which is prompt-injection-prone
/// if surfaced to the model directly). Returns `None` for safe reads.
pub fn get_read_block_error(path: &Path) -> Option<String> {
    get_read_block_error_with(path, &resolved_wayland_home())
}

/// Lower-level entry point with injected `wayland_home`.
pub fn get_read_block_error_with(path: &Path, wayland_home: &Path) -> Option<String> {
    let resolved = canonicalize_for_check(path);
    let resolved_str = resolved.as_os_str().to_string_lossy().to_string();

    let hub = wayland_home.join("skills").join(".hub");
    let cache = hub.join("index-cache");

    // Most-specific path first so the message stays accurate even when
    // both prefixes match (cache lives under .hub).
    let hub_canon = canonicalize_for_check(&hub);
    let cache_canon = canonicalize_for_check(&cache);
    let hub_str = hub_canon.as_os_str().to_string_lossy().to_string();
    let cache_str = cache_canon.as_os_str().to_string_lossy().to_string();
    let mut hub_prefix = hub_str.clone();
    hub_prefix.push(std::path::MAIN_SEPARATOR);
    let mut cache_prefix = cache_str.clone();
    cache_prefix.push(std::path::MAIN_SEPARATOR);

    let in_cache = resolved_str == cache_str || resolved_str.starts_with(&cache_prefix);
    let in_hub = resolved_str == hub_str || resolved_str.starts_with(&hub_prefix);

    if in_cache || in_hub {
        return Some(format!(
            "Access denied: {} is an internal Wayland cache file \
             and cannot be read directly to prevent prompt injection. \
             Use the skills_list or skill_view tools instead.",
            path.display()
        ));
    }
    None
}

/// Flat view of the deny list: exact denied paths first (sorted), then
/// the directory prefixes. Useful for diagnostics and for callers that
/// want to display the policy.
pub fn sensitive_patterns() -> Vec<PathBuf> {
    let home = resolved_user_home();
    let wayland_home = resolved_wayland_home();
    let mut paths = build_write_denied_paths(&home, &wayland_home);
    paths.sort();
    let prefixes = build_write_denied_prefixes(&home);
    paths.extend(prefixes);
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- build_write_denied_paths / build_write_denied_prefixes -----

    #[cfg(unix)]
    #[test]
    fn denied_paths_include_known_sensitives() {
        let home = PathBuf::from("/home/alice");
        let wayland = PathBuf::from("/home/alice/.config/wayland-core");
        let paths = build_write_denied_paths(&home, &wayland);
        let strs: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(strs.iter().any(|s| s.ends_with(".ssh/id_rsa")));
        assert!(strs.iter().any(|s| s.ends_with(".ssh/authorized_keys")));
        assert!(strs.iter().any(|s| s.ends_with(".bashrc")));
        assert!(strs.iter().any(|s| s.ends_with(".netrc")));
        assert!(strs.iter().any(|s| s.ends_with("/etc/shadow")));
        assert!(strs.iter().any(|s| s.ends_with("/etc/passwd")));
        assert!(strs.iter().any(|s| s.ends_with("wayland-core/.env")));
    }

    #[cfg(windows)]
    #[test]
    fn denied_paths_include_known_sensitives() {
        // Use a Windows-shaped home path so the cross-platform entries
        // (which build via `home.join(".ssh").join("id_rsa")`) emit
        // backslash-separated paths under the home dir, matching what
        // Git-for-Windows + Windows installers actually create.
        let home = PathBuf::from(r"C:\Users\alice");
        let wayland = PathBuf::from(r"C:\Users\alice\AppData\Roaming\wayland-core");
        let paths = build_write_denied_paths(&home, &wayland);
        let strs: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        // Cross-platform credential files under user home.
        assert!(strs.iter().any(|s| s.ends_with(r".ssh\id_rsa")));
        assert!(strs.iter().any(|s| s.ends_with(r".ssh\authorized_keys")));
        assert!(strs.iter().any(|s| s.ends_with(".bashrc")));
        assert!(strs.iter().any(|s| s.ends_with(".netrc")));
        assert!(strs.iter().any(|s| s.ends_with(r"wayland-core\.env")));
        // Windows-specific paths come from the host's WINDIR/APPDATA at
        // runtime; spot-check only when both are present in CI.
        if std::env::var("WINDIR").is_ok() {
            assert!(
                strs.iter()
                    .any(|s| s.to_ascii_lowercase().contains("system32\\config\\sam")),
                "SAM hive missing from Windows denylist; got: {strs:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn denied_prefixes_end_in_separator() {
        let home = PathBuf::from("/home/alice");
        let prefixes = build_write_denied_prefixes(&home);
        for p in &prefixes {
            let s = p.to_string_lossy();
            assert!(
                s.ends_with(std::path::MAIN_SEPARATOR),
                "prefix missing trailing separator: {s}"
            );
        }
        let strs: Vec<String> = prefixes
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(strs.iter().any(|s| s.contains(".aws")));
        assert!(strs.iter().any(|s| s.contains(".gnupg")));
        assert!(strs.iter().any(|s| s.contains("/etc/sudoers.d")));
        assert!(
            strs.iter()
                .any(|s| s.contains(".config") && s.contains("gh"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn denied_prefixes_end_in_separator() {
        let home = PathBuf::from(r"C:\Users\alice");
        let prefixes = build_write_denied_prefixes(&home);
        for p in &prefixes {
            let s = p.to_string_lossy();
            assert!(
                s.ends_with(std::path::MAIN_SEPARATOR),
                "prefix missing trailing separator: {s}"
            );
        }
        let strs: Vec<String> = prefixes
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(strs.iter().any(|s| s.contains(".aws")));
        assert!(strs.iter().any(|s| s.contains(".gnupg")));
        assert!(
            strs.iter()
                .any(|s| s.contains(".config") && s.contains("gh"))
        );
        if std::env::var("APPDATA").is_ok() {
            assert!(
                strs.iter()
                    .any(|s| s.to_ascii_lowercase().contains(r"microsoft\crypto")),
                "DPAPI Crypto prefix missing on Windows: {strs:?}"
            );
        }
    }

    // ----- is_write_denied_with -----

    #[test]
    fn write_denied_exact_path() {
        let home = PathBuf::from("/home/alice");
        let wayland = PathBuf::from("/home/alice/.config/wayland-core");
        assert!(is_write_denied_with(
            Path::new("/home/alice/.bashrc"),
            &home,
            &wayland,
        ));
    }

    #[test]
    fn write_denied_directory_prefix() {
        let home = PathBuf::from("/home/alice");
        let wayland = PathBuf::from("/home/alice/.config/wayland-core");
        assert!(is_write_denied_with(
            Path::new("/home/alice/.ssh/some_new_file"),
            &home,
            &wayland,
        ));
        assert!(is_write_denied_with(
            Path::new("/home/alice/.aws/credentials"),
            &home,
            &wayland,
        ));
    }

    #[test]
    fn write_allowed_ordinary_path() {
        // Make sure WAYLAND_WRITE_SAFE_ROOT isn't lingering from another test.
        // SAFETY: tests run single-threaded for env mutation via #[serial] would
        // be ideal, but cargo test default is multi-thread per binary. We use
        // a unique tmpdir path here and don't touch the env var.
        let _ = std::env::var(WRITE_SAFE_ROOT_ENV); // observe only
        let home = PathBuf::from("/home/alice");
        let wayland = PathBuf::from("/home/alice/.config/wayland-core");
        // /tmp/... is benign on linux + macOS-style hosts. We don't set the
        // safe-root env so the only gate is the deny list.
        let safe_was_set = std::env::var(WRITE_SAFE_ROOT_ENV).is_ok();
        if safe_was_set {
            // Skip on hosts that pre-set the env var (CI matrix safety).
            return;
        }
        assert!(!is_write_denied_with(
            Path::new("/tmp/wcore-fs-test/output.txt"),
            &home,
            &wayland,
        ));
    }

    #[test]
    fn write_denied_when_prefix_has_lookalike() {
        // `~/.sshrc` is NOT under `~/.ssh/` and must be allowed (separator
        // discipline). This catches the classic startswith-without-separator
        // bug.
        let home = PathBuf::from("/home/alice");
        let wayland = PathBuf::from("/home/alice/.config/wayland-core");
        // Pre-clear safe root for this assertion.
        if std::env::var(WRITE_SAFE_ROOT_ENV).is_ok() {
            return;
        }
        assert!(!is_write_denied_with(
            Path::new("/home/alice/.sshrc"),
            &home,
            &wayland,
        ));
    }

    #[test]
    fn write_denied_wayland_env_file() {
        let home = PathBuf::from("/home/alice");
        let wayland = PathBuf::from("/home/alice/.config/wayland-core");
        assert!(is_write_denied_with(
            Path::new("/home/alice/.config/wayland-core/.env"),
            &home,
            &wayland,
        ));
    }

    // ----- get_read_block_error_with -----

    #[test]
    fn read_block_hits_skill_hub() {
        let wayland = PathBuf::from("/home/alice/.config/wayland-core");
        let target = wayland.join("skills").join(".hub").join("index.json");
        let msg = get_read_block_error_with(&target, &wayland).expect("should block hub index");
        assert!(msg.contains("internal Wayland cache file"));
        assert!(msg.contains("skills_list"));
    }

    #[test]
    fn read_block_hits_index_cache() {
        let wayland = PathBuf::from("/home/alice/.config/wayland-core");
        let target = wayland
            .join("skills")
            .join(".hub")
            .join("index-cache")
            .join("entry.bin");
        assert!(get_read_block_error_with(&target, &wayland).is_some());
    }

    #[test]
    fn read_block_passes_unrelated_path() {
        let wayland = PathBuf::from("/home/alice/.config/wayland-core");
        assert!(get_read_block_error_with(Path::new("/tmp/work.txt"), &wayland).is_none());
        // Skills *root* (not under .hub) is allowed.
        assert!(
            get_read_block_error_with(
                &wayland.join("skills").join("my-skill").join("SKILL.md"),
                &wayland,
            )
            .is_none()
        );
    }

    // ----- helpers -----

    #[test]
    fn lex_normalize_collapses_dot_segments() {
        let p = Path::new("/a/b/./c/../d");
        assert_eq!(lex_normalize(p), PathBuf::from("/a/b/d"));
    }

    #[test]
    fn expand_tilde_with_no_home_passes_through() {
        // We can't reliably unset HOME in a multi-thread test, but we can
        // assert the no-tilde branch returns the input verbatim.
        assert_eq!(expand_tilde("/etc/passwd"), PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn sensitive_patterns_contains_both_paths_and_prefixes() {
        let pats = sensitive_patterns();
        assert!(!pats.is_empty());
        // At least one prefix-style entry (trailing separator) and one
        // exact-path entry (no trailing separator).
        let has_prefix = pats
            .iter()
            .any(|p| p.to_string_lossy().ends_with(std::path::MAIN_SEPARATOR));
        let has_exact = pats
            .iter()
            .any(|p| !p.to_string_lossy().ends_with(std::path::MAIN_SEPARATOR));
        assert!(has_prefix, "expected at least one prefix entry");
        assert!(has_exact, "expected at least one exact-path entry");
    }
}
