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
    #[error("path is a UNC / network path (SMB NTLM-leak risk): {0:?}")]
    UncPath(PathBuf),
    #[error("path uses a Windows device / verbatim namespace (\\\\.\\ or \\\\?\\): {0:?}")]
    DeviceOrVerbatimPath(PathBuf),
    #[error("path is not a regular file: {0:?}")]
    NonRegularFile(PathBuf),
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

    // #644: reject Windows UNC / network paths (`\\server\share`,
    // `\\?\UNC\server\share`). On Windows such a path is absolute and would
    // pass the check below; opening it triggers an outbound SMB connect that
    // leaks a NetNTLM hash to an attacker-chosen host. On Unix the string is
    // not absolute (the `is_absolute` guard would catch it as NotAbsolute),
    // but we reject it explicitly and portably here so the guard is enforced —
    // and unit-tested — on every platform, and returns the precise reason.
    if looks_like_unc(path, &path_str) {
        return Err(PathValidationError::UncPath(raw));
    }

    // #644 (CI(Array) fix): reject the Windows device (`\\.\`) and verbatim
    // (`\\?\...`) namespaces. On Windows a verbatim-disk path (`\\?\C:\...`) is
    // absolute, so it would sail past the `is_absolute` guard below and be
    // ACCEPTED — bypassing Win32 path normalization and confinement — while a
    // device path (`\\.\PhysicalDrive0`) is a raw handle with the same
    // unbounded-read / non-regular hazard #644 targets. Neither is a legitimate
    // input to the legacy file tools. Reject both explicitly and portably (they
    // are NOT UNC — `\\?\UNC\...` is already consumed as UncPath above), so the
    // guard is enforced and unit-tested on every platform.
    if looks_like_device_or_verbatim(path, &path_str) {
        return Err(PathValidationError::DeviceOrVerbatimPath(raw));
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

    // #644: reject an EXISTING non-regular file (FIFO, char/block device,
    // socket). `/dev/zero` reports a metadata length of 0 then streams
    // unbounded into `fs::read` (OOM); a FIFO with no writer blocks the read
    // forever (DoS). `fs::metadata` follows symlinks, so a symlink to such a
    // target is caught too. Only enforced when the path already exists, so a
    // not-yet-created Write/Edit target (and ordinary directories) still pass.
    if let Ok(meta) = fs::metadata(&normalized) {
        let ft = meta.file_type();
        if !ft.is_file() && !ft.is_dir() {
            return Err(PathValidationError::NonRegularFile(normalized));
        }
    }

    Ok(normalized)
}

/// #644: does `path`/`s` name a Windows UNC / network path?
///
/// Matches plain UNC (`\\server\share`) and verbatim UNC
/// (`\\?\UNC\server\share`), but NOT a verbatim local-disk path (`\\?\C:\...`)
/// or a device path (`\\.\...`), which are not remote SMB targets.
///
/// Two separators matter here: Windows (and Rust's path parser) treat `/` and
/// `\` as INTERCHANGEABLE in the prefix, so `//server/share` and `\/server\share`
/// parse as UNC just like the backslash spelling. A byte-literal `\\` match
/// would miss those and let the SMB connect through — so we (a) ask the parser
/// directly on Windows (authoritative), and (b) normalize `/`→`\` before the
/// portable string match (which is also what the cross-platform tests exercise).
fn looks_like_unc(path: &Path, s: &str) -> bool {
    // Authoritative on Windows: classify via the parsed prefix, which already
    // normalizes separators and recognizes every UNC spelling.
    #[cfg(windows)]
    {
        use std::path::Prefix;
        if let Some(Component::Prefix(p)) = path.components().next()
            && matches!(p.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
        {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = path;

    // Portable backstop: normalize separators, then match UNC forms.
    let norm: String = s.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
    let lower = norm.to_ascii_lowercase();
    // Verbatim UNC: \\?\UNC\server\share (case-insensitive prefix).
    if lower.starts_with(r"\\?\unc\") {
        return true;
    }
    // Plain UNC: two leading separators then a host character — exclude the
    // verbatim `\\?\` and device `\\.\` namespaces.
    if let Some(rest) = norm.strip_prefix(r"\\") {
        return !matches!(rest.chars().next(), Some('?') | Some('.') | None);
    }
    false
}

/// #644: does `path`/`s` name a Windows device (`\\.\`) or verbatim (`\\?\`)
/// namespace path (other than `\\?\UNC\...`, which `looks_like_unc` already
/// classifies as UNC)?
///
/// These share the `\\` lead-in with UNC but are not remote SMB targets:
///   * `\\?\C:\...` (verbatim disk) disables Win32 path normalization and is
///     `is_absolute() == true` on Windows, so without this guard it would be
///     ACCEPTED by the legacy file tools.
///   * `\\.\PhysicalDrive0` (device namespace) is a raw device handle.
///
/// Callers invoke this AFTER `looks_like_unc`, so verbatim-UNC is already
/// handled; the `\\?\unc\` guard below is belt-and-suspenders for the standalone
/// function. Mirrors `looks_like_unc`'s dual strategy: authoritative parsed
/// prefix on Windows, portable normalized-string match everywhere.
fn looks_like_device_or_verbatim(path: &Path, s: &str) -> bool {
    #[cfg(windows)]
    {
        use std::path::Prefix;
        if let Some(Component::Prefix(p)) = path.components().next()
            && matches!(
                p.kind(),
                Prefix::VerbatimDisk(..) | Prefix::DeviceNS(..) | Prefix::Verbatim(..)
            )
        {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = path;

    // Portable backstop: normalize separators, then match the device / verbatim
    // lead-ins. `\\?\UNC\...` is UNC (handled earlier) — never device/verbatim.
    let norm: String = s.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
    let lower = norm.to_ascii_lowercase();
    if lower.starts_with(r"\\?\unc\") {
        return false;
    }
    lower.starts_with(r"\\?\") || lower.starts_with(r"\\.\")
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

    // Windows read-path deny list. The POSIX suffixes above use forward
    // slashes and case-sensitive matching, so they give ZERO protection on
    // Windows where secrets live under `%USERPROFILE%\.ssh\`, `%APPDATA%`,
    // and the `%WINDIR%\System32\config` registry hives, and paths are
    // backslash-separated and case-insensitive. Mirror `file_safety.rs`'s
    // Windows technique here for the READ path: lowercase the path (NTFS is
    // case-insensitive) and match backslash-form denied substrings. Keep the
    // POSIX entries above intact — they still apply to `\\?\`-style mixed
    // inputs and to cross-platform test fixtures.
    #[cfg(windows)]
    {
        let lower = s.to_ascii_lowercase();
        // Backslash-form credential suffixes under the user profile / appdata.
        const WINDOWS_DENIED_SUFFIXES: &[&str] = &[
            r"\.ssh\id_rsa",
            r"\.ssh\id_ed25519",
            r"\.ssh\id_ecdsa",
            r"\.ssh\id_dsa",
            r"\.ssh\authorized_keys",
            r"\.ssh\known_hosts",
            r"\.aws\credentials",
            r"\.gnupg\private-keys-v1.d",
            r"\.kube\config",
            r"\.config\wayland-core\credentials.toml",
            r"\.wayland\credentials.toml",
            r"\wayland-core\auth.json",
            r"\wayland-core\credentials.enc",
            r"\wayland-core\credentials.key.json",
            r"\.wayland\cron\",
            r"\.netrc",
            r"\.npmrc",
            r"\.pypirc",
            r"\.docker\config.json",
            r"\.gcloud\credentials.db",
            r"\.azure\",
        ];
        if WINDOWS_DENIED_SUFFIXES
            .iter()
            .any(|sfx| lower.contains(sfx))
        {
            return true;
        }
        // `%WINDIR%\System32\config` registry hives (SAM / SYSTEM / SECURITY).
        // Match component-wise on the lowercased path so a different
        // SystemDrive (`D:\Windows\...`) is still caught.
        const WINDOWS_HIVE_SUFFIXES: &[&str] = &[
            r"\system32\config\sam",
            r"\system32\config\system",
            r"\system32\config\security",
        ];
        if WINDOWS_HIVE_SUFFIXES.iter().any(|sfx| lower.contains(sfx)) {
            return true;
        }
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

    // #644 — UNC / network-path rejection (SMB NTLM-leak). String-based, so the
    // guard is exercised on Unix CI even though a UNC path is Windows-specific.
    #[test]
    fn unc_paths_rejected() {
        for p in [
            // Backslash spellings.
            r"\\server\share\file.txt",
            r"\\?\UNC\server\share\file.txt",
            r"\\10.0.0.5\c$\secret",
            // Forward / mixed slash — Windows treats '/' == '\' in the prefix,
            // so these parse as UNC too and MUST be rejected (the SMB-leak
            // bypass). These are the security-critical spellings.
            "//server/share/secret",
            r"\/server\share\file",
            r"/\server/share/file",
            "//?/UNC/server/share/file",
        ] {
            let err = validate_user_path(Path::new(p)).unwrap_err();
            assert!(
                matches!(err, PathValidationError::UncPath(_)),
                "expected UncPath for {p:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn verbatim_disk_and_device_paths_are_not_unc() {
        // These share the `\\` lead-in but are NOT remote SMB targets, so the
        // UNC guard must not claim them. They ARE rejected (device / verbatim
        // namespace), just not as UncPath — on Windows `\\?\C:\...` is absolute
        // and would otherwise be ACCEPTED, so the reject is load-bearing there.
        for p in [r"\\?\C:\Users\me\notes.txt", r"\\.\PhysicalDrive0"] {
            let err = validate_user_path(Path::new(p)).unwrap_err();
            assert!(
                !matches!(err, PathValidationError::UncPath(_)),
                "{p:?} must not be classified as UNC, got {err:?}"
            );
            assert!(
                matches!(err, PathValidationError::DeviceOrVerbatimPath(_)),
                "{p:?} must be rejected as device/verbatim, got {err:?}"
            );
        }
    }

    // `\\?\UNC\server\share` stays classified as UNC — the device/verbatim
    // guard must not steal it from the UNC classifier (SMB-leak reject).
    #[test]
    fn verbatim_unc_still_classified_as_unc() {
        let err = validate_user_path(Path::new(r"\\?\UNC\server\share\file.txt")).unwrap_err();
        assert!(
            matches!(err, PathValidationError::UncPath(_)),
            "verbatim-UNC must stay UncPath, got {err:?}"
        );
    }

    // #644 — non-regular files (FIFO/device/socket) must be rejected: they hang
    // or stream unbounded through `fs::read`. A unix-domain socket file is a
    // pure-std way to create a non-regular file.
    #[cfg(unix)]
    #[test]
    fn non_regular_file_rejected() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("s.sock");
        let _l = UnixListener::bind(&sock).unwrap();
        let err = validate_user_path(&sock).unwrap_err();
        assert!(
            matches!(err, PathValidationError::NonRegularFile(_)),
            "a socket must be rejected as non-regular, got {err:?}"
        );
    }

    // A regular file passes, and a not-yet-existing Write/Edit target still
    // passes (the non-regular guard only fires on existing files).
    #[cfg(unix)]
    #[test]
    fn regular_file_and_nonexistent_target_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(
            validate_user_path(&f).is_ok(),
            "existing regular file allowed"
        );
        let new = dir.path().join("to-create.txt");
        assert!(
            validate_user_path(&new).is_ok(),
            "not-yet-existing write target allowed"
        );
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
    #[serial_test::serial]
    fn wayland_cron_relocated_home_jobs_and_key_rejected() {
        // WAYLAND_HOME is process-global; `#[serial]` serializes every
        // env-mutating test in this binary so this write/remove pair cannot
        // race another (a data race on the environ table is unsafe in edition
        // 2024).
        // SAFETY: `#[serial]` guarantees no other `#[serial]` test mutates the
        // environment concurrently.
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
    #[serial_test::serial]
    fn wayland_cron_symlinked_home_canonical_write_rejected() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("wl-cron-symlink-{}", std::process::id()));
        let realhome = base.join("realhome");
        let cron = realhome.join("cron");
        fs::create_dir_all(&cron).expect("create cron dir");
        let link = base.join("link");
        let _ = fs::remove_file(&link);
        symlink(&realhome, &link).expect("symlink link -> realhome");

        // SAFETY: `#[serial]` guarantees no other `#[serial]` test mutates the
        // environment concurrently; restored below.
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

    // ----- Windows read-path deny list (F2) -----
    //
    // These mirror file_safety.rs's Windows write-deny tests for the READ
    // guard. They use Windows-shaped absolute paths (`C:\Users\...`,
    // `C:\Windows\System32\config\SAM`) which only validate as absolute on
    // Windows, so they're gated to cfg(windows) and verified via CI.
    #[cfg(windows)]
    #[test]
    fn windows_ssh_private_key_rejected() {
        let err = validate_user_path(Path::new(r"C:\Users\alice\.ssh\id_rsa")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(windows)]
    #[test]
    fn windows_ssh_private_key_case_insensitive_rejected() {
        // NTFS is case-insensitive; an upper/mixed-case spelling must still
        // be denied.
        let err = validate_user_path(Path::new(r"C:\Users\Alice\.SSH\ID_RSA")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(windows)]
    #[test]
    fn windows_aws_credentials_rejected() {
        let err = validate_user_path(Path::new(
            r"C:\Users\alice\AppData\Roaming\.aws\credentials",
        ))
        .unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(windows)]
    #[test]
    fn windows_sam_hive_rejected() {
        let err = validate_user_path(Path::new(r"C:\Windows\System32\config\SAM")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(windows)]
    #[test]
    fn windows_system_hive_on_other_drive_rejected() {
        // A relocated SystemDrive must still be caught (component-wise match).
        let err = validate_user_path(Path::new(r"D:\Windows\System32\config\SYSTEM")).unwrap_err();
        assert!(matches!(err, PathValidationError::SystemPath(_)));
    }

    #[cfg(windows)]
    #[test]
    fn windows_ordinary_path_allowed() {
        let p = validate_user_path(Path::new(r"C:\work\notes.txt")).unwrap();
        assert_eq!(p, PathBuf::from(r"C:\work\notes.txt"));
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
