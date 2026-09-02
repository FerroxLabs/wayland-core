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
    #[error(
        "path uses a Windows device namespace (\\\\.\\) or a non-disk \\\\?\\ verbatim root: {0:?}"
    )]
    DeviceOrVerbatimPath(PathBuf),
    #[error("path is not a regular file: {0:?}")]
    NonRegularFile(PathBuf),
    #[error("path names the Windows NUL device: {0:?}")]
    WindowsNullDevice(PathBuf),
    #[error("path could not be inspected ({1:?}): {0:?}")]
    Unstattable(PathBuf, std::io::ErrorKind),
}

/// #238 -- is this final path component the Windows `NUL` device?
///
/// **Deliberately just `NUL`.** The filed fix was the textbook one: refuse any
/// component whose extension-stripped name is a reserved DOS device
/// (`NUL`/`CON`/`AUX`/`PRN`/`COM1`-`COM9`/`LPT1`-`LPT9`). That fix was written,
/// unit-tested green, and then REFUTED by measurement -- the tests were green
/// because they encoded the same wrong assumption the guard did.
///
/// Measured 2026-08-18 on Windows 11 build 10.0.26200.0 / NTFS, fresh
/// directory, .NET write + read-back + on-disk byte count + `cmd type`:
///
/// | name | write | read back | bytes on disk | verdict |
/// |---|---|---|---|---|
/// | `NUL` | throws | fail | 0 | device |
/// | `CON` `AUX` `PRN` `COM1` `LPT1` | ok | ok | 1 | **ordinary file** |
/// | `NUL.txt` `AUX.log` | ok | ok | 1 | **ordinary file** |
/// | `ordinary.txt` (control) | ok | ok | 1 | ordinary file |
///
/// Both controls were present: `ordinary.txt` proves the probe reads normal
/// behaviour and `NUL` proves it can still detect device behaviour, so the
/// middle rows are real. The broad blocklist would therefore REFUSE
/// `aux.txt`, `NUL.txt`, `COM1` and `con.json` -- addressable files holding
/// real user data on this build. Refusing real data to close a LOW-severity
/// read of an empty device stream is a worse trade than the gap.
///
/// `NUL` alone is free: no ordinary file can bear that name on Windows, so
/// refusing it cannot refuse anything real. Trailing dots and spaces are
/// stripped because Win32 strips them before resolving the name; the extension
/// is NOT stripped, because the measurement says `NUL.txt` is a file.
///
/// On an older kernel (Server 2019/2022, build 20348) the other names are
/// likely still devices -- the same build split as the DELETE-mask work. That
/// residual is knowingly left open: it reads an empty stream, while the fix
/// for it destroys data on current Windows. Do not re-file the broad guard
/// without a measurement table for the build you are targeting.
///
/// Pure and string-only, so it is unit-testable on every platform; the
/// ENFORCEMENT is Windows-gated, because `NUL` is an ordinary, legal file name
/// on Unix and refusing it there would be the same data-refusing mistake.
pub fn is_windows_null_device_name(component: &str) -> bool {
    component
        .trim_end_matches(['.', ' '])
        .eq_ignore_ascii_case("nul")
}

/// The #238 decision with the platform as a PARAMETER.
///
/// There is no local Windows gate in this project, so a `#[cfg(windows)]`
/// enforcement arm would be graded by nothing until CI. Threading the platform
/// through means both arms -- refuse on Windows, allow on Unix -- are exercised
/// on every host, the way the UNC guard is tested by its string form.
fn is_windows_null_device_path(path: &Path, on_windows: bool) -> bool {
    on_windows
        && path
            .file_name()
            .is_some_and(|name| is_windows_null_device_name(&name.to_string_lossy()))
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

    // #644 (CI(Array) fix): reject the Windows device namespace (`\\.\...`,
    // e.g. `\\.\PhysicalDrive0`) — a raw handle with the same unbounded-read /
    // non-regular hazard #644 targets — and any NON-DISK verbatim root
    // (`\\?\GLOBALROOT\Device\...`, `\\?\Volume{...}\...`), which reach the
    // same devices by the other spelling. Neither is a legitimate input to the
    // legacy file tools, and neither is UNC (`\\?\UNC\...` is already consumed
    // as UncPath above), so both are rejected explicitly and portably here.
    //
    // core#409 c2: the verbatim DISK form (`\\?\C:\...`) is deliberately NOT
    // rejected — see `looks_like_device_or_nondisk_verbatim`.
    if looks_like_device_or_nondisk_verbatim(path, &path_str) {
        return Err(PathValidationError::DeviceOrVerbatimPath(raw));
    }

    // #238: `C:\\Users\\me\\NUL` has an ordinary Disk prefix, is absolute, and
    // is neither UNC nor a device/verbatim namespace, so every guard above
    // passes it straight through to `CreateFileW`, which resolves it to the
    // null device. Windows-only: see `is_windows_null_device_name` for why
    // this is `NUL` and nothing else, and why Unix must not enforce it.
    if is_windows_null_device_path(path, cfg!(windows)) {
        return Err(PathValidationError::WindowsNullDevice(raw));
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
    //
    // #238: this used to be `if let Ok(meta)`, so ANY stat failure silently
    // SKIPPED the check and the path returned `Ok`. A guard that disappears
    // exactly when the OS refuses to describe the target is the wrong way
    // round. `NotFound` is the one benign failure -- it is the normal
    // Write/Edit "leaf does not exist yet" case -- so it alone still passes;
    // every other kind now fails closed and says which kind it was.
    match fs::metadata(&normalized) {
        Ok(meta) => {
            let ft = meta.file_type();
            if !ft.is_file() && !ft.is_dir() {
                return Err(PathValidationError::NonRegularFile(normalized));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(PathValidationError::Unstattable(normalized, err.kind())),
    }

    Ok(normalized)
}

/// Validate an LLM-supplied **search root** before any filesystem touch.
///
/// `Grep` and `Glob` are the read-path siblings of `Read`, but they could not
/// use [`validate_user_path`]: a search root is legitimately relative (`.` is
/// the schema default) and legitimately a directory, so the absolute-path and
/// regular-file rules do not apply. The consequence was that the entire
/// credential deny-list was enforced on one read tool and walked around with
/// another — `Grep {pattern:"root", path:"/etc/shadow"}` returned the hash
/// line, and `Grep` returns matched line CONTENT, not just names.
///
/// This applies the deny-list half of `validate_user_path` to a resolved
/// search root:
///
///   1. No null bytes.
///   2. No UNC, device or non-disk verbatim namespace (#644's NetNTLM-leak
///      reasoning applies unchanged when the path is handed to `rg` instead of
///      `fs`). A verbatim DISK root IS accepted — see
///      `looks_like_device_or_nondisk_verbatim` and core#409 c2.
///   3. Resolve relative input against `base` (the sandbox jail root when the
///      caller has one, else the process cwd) and lex-normalize, so
///      `../../../etc/shadow` is graded as the absolute path it denotes rather
///      than passed through as an innocuous-looking relative string.
///   4. Deny-list the result, including through a symlinked leaf or prefix.
///
/// **Known residual (deliberate, not an oversight).** The deny-list matches
/// specific credential FILES, so this refuses a denied path as the *direct*
/// target but does not stop a recursive search of a parent directory that
/// happens to contain one (`Grep {path:"/etc"}`). Closing that needs
/// per-entry filtering inside the walk, which the subprocess backends
/// (`rg`, `grep`, `findstr`) do not expose uniformly — doing it for `rg`
/// only would make the boundary depend on which binary is installed, which
/// is worse than a documented gap. Tracked separately.
pub fn validate_search_root(
    path: &Path,
    base: Option<&Path>,
) -> Result<PathBuf, PathValidationError> {
    let raw = path.to_path_buf();

    let path_str = path.to_string_lossy();
    if path_str.contains('\0') {
        return Err(PathValidationError::NullByte(raw));
    }
    if looks_like_unc(path, &path_str) {
        return Err(PathValidationError::UncPath(raw));
    }
    if looks_like_device_or_nondisk_verbatim(path, &path_str) {
        return Err(PathValidationError::DeviceOrVerbatimPath(raw));
    }

    // An absolute `path` wins over `base`, matching `Path::join` semantics and
    // the pre-existing search-root resolution in `run_grep`.
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match base {
            Some(root) => root.join(path),
            None => match std::env::current_dir() {
                Ok(cwd) => cwd.join(path),
                // No cwd (deleted working directory) means we cannot say what
                // the relative path denotes, so we cannot clear it. Fail closed.
                Err(_) => return Err(PathValidationError::NotAbsolute(raw)),
            },
        }
    };

    let normalized = lex_normalize(&absolute);

    if is_denied_system_path(&normalized) {
        return Err(PathValidationError::SystemPath(normalized));
    }
    if let Some(resolved) = canonicalize_existing_prefix(&normalized)
        && resolved != normalized
        && is_denied_system_path(&resolved)
    {
        return Err(PathValidationError::SystemPath(resolved));
    }
    if let Some(link_target) = resolve_symlink_target(&normalized)
        && is_denied_system_path(&link_target)
    {
        return Err(PathValidationError::SystemPath(link_target));
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
    // This logic now lives in `wcore_config::network_path`, which is where the
    // other four copies in this workspace were consolidated onto. It was
    // promoted from here because this was the best of the five: it normalizes
    // separators, is authoritative via the parsed prefix on Windows, and is
    // the only one that kept UNC distinct from the verbatim/device namespaces.
    // Two of the others called `\\?\C:\Users\x` — a local disk — a network
    // path because they lacked that distinction.
    let _ = s;
    wcore_config::network_path::has_unc_prefix(path)
}

/// #644 / core#409 c2: does `path`/`s` name a Windows path the file tools must
/// refuse on its NAMESPACE alone — the device namespace (`\\.\PhysicalDrive0`,
/// a raw device handle) or a NON-DISK verbatim root
/// (`\\?\GLOBALROOT\Device\...`, `\\?\Volume{...}\...`), which reach the same
/// devices by the other spelling? Callers invoke this AFTER `looks_like_unc`,
/// so `\\?\UNC\...` is already classified as UNC.
///
/// **`\\?\C:\...` — the verbatim DISK form — is deliberately NOT refused.**
/// It names an ordinary local file, and it is the form `std::fs::canonicalize`
/// RETURNS on Windows, so it is the spelling this product's own canonical paths
/// carry: `WorkspacePolicy::root()` is `canonicalize`d at construction, and
/// every absolute path built by joining onto it is verbatim. Refusing it
/// refused the product's own output. MEASURED on Windows three times before the
/// guard itself was narrowed, each time worked around at a CALLER:
///
///   * `Refused to read \\?\F:\...\.wayland-out\results\toolu_01.txt` on
///     Windows 11 26200 — `workspace_policy::session_output_root`, worked
///     around there with `dunce::simplified`;
///   * all three tests in `wcore-agent/tests/full_posture_secret_jail_test.rs`
///     on the hosted Windows runner, worked around there with `simplified_root`;
///   * `Refused to search \\?\C:\...\Temp\.tmpcEhD2n` — the CONTROL arm of
///     `grep_vcs_content_store_deny`, i.e. an ORDINARY search of the session's
///     own workspace. That is FerroxLabs/wayland-core#409 c2, and it is a
///     user-facing wrong refusal, not a test artefact.
///
/// Admitting it opens nothing, because nothing downstream keys on the prefix:
/// the Windows credential deny-list matches with `contains`, so a prefix cannot
/// evade it; `..` is refused outright (`validate_user_path`) or lex-normalized
/// away (`validate_search_root`) before any open; and every containment compare
/// in `workspace_policy` canonicalizes BOTH sides, which on Windows means both
/// are verbatim. The `\\?\` normalization-bypass hazard #644 named is real for
/// the DEVICE spellings, which this still refuses.
///
/// Mirrors `looks_like_unc`'s dual strategy via the consolidated predicates in
/// `wcore_config::network_path`: authoritative parsed prefix on Windows,
/// portable normalized-string match everywhere, same answer on every platform.
fn looks_like_device_or_nondisk_verbatim(path: &Path, s: &str) -> bool {
    let _ = s;
    wcore_config::network_path::has_device_or_verbatim_prefix(path)
        && !wcore_config::network_path::has_verbatim_disk_prefix(path)
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

    // Linux procfs re-exposes process-private state as ordinary regular files,
    // so none of the guards above (or the non-regular-file guard in
    // `validate_user_path`) sees it: `Read {file_path:"/proc/self/environ"}`
    // returns the AGENT'S OWN environment — `ANTHROPIC_API_KEY`,
    // `OPENAI_API_KEY` and every other provider credential — straight into
    // model context. `BashTool` already refuses `env` / `printenv` via its
    // credential denylist (`bash/policy.rs`), so without this the boundary was
    // enforced on one tool and trivially walked around with another.
    if is_denied_proc_path(path) {
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
        //
        // #644 part 3 named `~/.git-credentials` as readable, and it stayed
        // readable while its whole class — `.netrc`, `.npmrc`, `.pypirc`,
        // `.docker/config.json` — was denied. It is the most direct of them:
        // git's `store` helper writes bare `https://user:token@host` lines in
        // cleartext, so one Read returns a usable push credential for every
        // remote the user has authenticated to. Denied nowhere before this —
        // not here, not in `bash/policy.rs`, not in `file_safety.rs`.
        "/.git-credentials",
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
            // Mirror of the POSIX `/.git-credentials` entry above. The first
            // cut of this fix added only the forward-slash form, which gives
            // ZERO protection on Windows — `%USERPROFILE%\.git-credentials`
            // matched nothing and stayed readable. The tests that would have
            // caught it were `#[cfg(unix)]`, so the guard was enforced on the
            // one platform it worked on and the gap was invisible. They are
            // no longer gated.
            r"\.git-credentials",
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

/// Does `path` name a Linux procfs location that exposes process-private
/// state (or all of physical memory)?
///
/// Denies the whole PER-PROCESS subtree — `/proc/self/...`,
/// `/proc/thread-self/...` and `/proc/<pid>/...` — rather than enumerating leaf
/// names, because the leaf list is unwinnable: `environ` is one spelling of the
/// hole, but `cmdline` carries secrets in argv (`mysql -p<pw>`,
/// `--api-key=...`), `mem` is a direct read of the process's memory,
/// `fd/<n>`/`fdinfo` re-open whatever the process already has open, `maps` is
/// an ASLR map for a follow-up `mem` read, and `root` / `cwd` are symlinks that
/// rebuild ANY path — including every entry denied above — through /proc.
/// `/proc/<pid>/task/<tid>/environ` needs no special case: `<pid>` is still the
/// second component. Both spellings matter even though `validate_user_path`
/// re-runs this check on the canonicalized form, because canonicalizing
/// `/proc/self/environ` yields `/proc/<pid>/environ`.
///
/// `/proc/kcore` is denied too. It is not per-process, but it is a regular file
/// mapping all of physical memory — the same "read memory, recover the keys"
/// defect as `/proc/self/mem`, and an unbounded `fs::read`.
///
/// Deliberately NOT denied, so the boundary is a decision rather than an
/// accident:
///   * The system-wide informational entries (`/proc/cpuinfo`, `/proc/meminfo`,
///     `/proc/version`, `/proc/mounts`, …). They carry no process-private data
///     and are legitimate agent reads.
///   * `/proc/sys/...`. Its hazard is privileged kernel-tunable WRITES, which
///     is a different finding from this credential-disclosure one and would
///     need its own analysis of what the agent legitimately reads there.
///   * `/proc/<pid>/auxv`, `/proc/kallsyms`. Address-layout / canary infoleaks,
///     not credentials — out of scope for this fix.
///
/// Matching is COMPONENT-wise on the parsed path, never a substring, so it
/// cannot over-match: a workspace file at `/tmp/proc/self/environ`, or
/// `/procfs/self/environ`, or `/proc-notes.txt`, is not under `/proc` and stays
/// readable. Compiled on every platform (like the POSIX entries above rather
/// than the `cfg(windows)` block) — a `/proc/...` string is not absolute on
/// Windows, so `validate_user_path` rejects it as `NotAbsolute` before this
/// runs, and macOS simply has no `/proc`.
fn is_denied_proc_path(path: &Path) -> bool {
    let mut components = path.components();

    // Must be rooted directly at `/proc` — not merely contain a `proc` segment.
    if !matches!(components.next(), Some(Component::RootDir)) {
        return false;
    }
    match components.next() {
        Some(Component::Normal(c)) if c.to_string_lossy() == "proc" => {}
        _ => return false,
    }

    let Some(Component::Normal(second)) = components.next() else {
        // Bare `/proc` — the directory listing itself discloses nothing.
        return false;
    };
    let second = second.to_string_lossy();

    second == "self"
        || second == "thread-self"
        || second == "kcore"
        // `/proc/<pid>/...` for any pid.
        || (!second.is_empty() && second.bytes().all(|b| b.is_ascii_digit()))
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

/// Lexical (no-syscall) path normalization. Shared with `grep_policy`, which
/// must key a backend's emitted path and the walker's entry on the SAME string
/// or the ignore/secret policy silently admits everything.
pub(crate) fn lex_normalize(path: &Path) -> PathBuf {
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

    // --- #238: the Windows NUL device ------------------------------------
    //
    // Every path literal below uses FORWARD slashes on purpose. `Path` treats
    // `\` as an ordinary character on Unix, so a backslash literal would give
    // `file_name() == "C:\\Users\\me\\NUL"` here and the test would pass for
    // the wrong reason. `/` is a separator on BOTH platforms.

    /// The name predicate. `NUL` and its trailing-dot/space forms only --
    /// every other reserved DOS name is an ordinary file on the build this
    /// was measured on, and refusing them would refuse real user data.
    #[test]
    fn only_nul_is_treated_as_a_windows_device_name() {
        for yes in ["NUL", "nul", "Nul", "NUL.", "NUL ", "nul. . ."] {
            assert!(
                is_windows_null_device_name(yes),
                "{yes:?} must be recognised as the NUL device"
            );
        }
        // MEASURED ordinary files on Windows 11 26200 -- see the predicate's
        // doc comment. Refusing any of these is the refuted fix.
        for no in [
            "CON", "AUX", "PRN", "COM1", "LPT1", "con", "aux.txt", "NUL.txt", "nul.log",
            "con.json", "NULL", "nulls", "annul", "", "n u l",
        ] {
            assert!(
                !is_windows_null_device_name(no),
                "{no:?} is an ordinary file name and must NOT be refused"
            );
        }
    }

    /// The enforcement decision, with the platform threaded in so BOTH arms
    /// run on every host. There is no local Windows gate; a `#[cfg(windows)]`
    /// test would be graded by nothing until CI.
    #[test]
    fn the_null_device_guard_fires_on_windows_and_only_on_windows() {
        let device = Path::new("C:/Users/me/NUL");
        assert!(
            is_windows_null_device_path(device, true),
            "on Windows this reaches CreateFileW and resolves to the null device"
        );
        // NEGATIVE CONTROL: `NUL` is a legal, ordinary file name on Unix.
        // Enforcing there would be the same data-refusing mistake as the
        // blocklist, in the other direction.
        assert!(
            !is_windows_null_device_path(Path::new("/home/me/NUL"), false),
            "Unix must not refuse an ordinary file named NUL"
        );
        // Only the FINAL component names a device: `C:\NUL\x` is not one.
        assert!(!is_windows_null_device_path(
            Path::new("C:/NUL/notes.txt"),
            true
        ));
        // NEGATIVE CONTROL: the measured-ordinary names, through the real
        // path decision rather than the string predicate.
        for ordinary in ["C:/p/CON", "C:/p/COM1", "C:/p/NUL.txt", "C:/p/aux.log"] {
            assert!(
                !is_windows_null_device_path(Path::new(ordinary), true),
                "{ordinary} holds real user data on Windows 11 26200"
            );
        }
    }

    /// End-to-end on this host: a file literally named `NUL` must still be
    /// readable on Unix. This is the guard-does-not-leak control, and it runs
    /// in plain CI on Linux and macOS.
    #[cfg(not(windows))]
    #[test]
    fn a_unix_file_named_nul_is_still_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nul = dir.path().join("NUL");
        std::fs::write(&nul, b"real user data").expect("write");
        let validated = validate_user_path(&nul).expect("NUL is an ordinary Unix file name");
        assert_eq!(validated.file_name().unwrap(), "NUL");
    }

    /// End-to-end on Windows. Unverified locally -- there is no Windows host
    /// in this loop -- so it is graded by CI's windows job alone.
    #[cfg(windows)]
    #[test]
    fn a_windows_nul_path_is_refused_end_to_end() {
        let dir = tempfile::tempdir().expect("tempdir");
        let device = dir.path().join("NUL");
        assert!(matches!(
            validate_user_path(&device),
            Err(PathValidationError::WindowsNullDevice(_))
        ));
        // NEGATIVE CONTROL: the measured-ordinary reserved names still work.
        for ordinary in ["CON", "COM1", "NUL.txt", "aux.log"] {
            let target = dir.path().join(ordinary);
            std::fs::write(&target, b"real user data").expect("write");
            validate_user_path(&target)
                .unwrap_or_else(|e| panic!("{ordinary} must stay readable: {e}"));
        }
    }

    /// A path whose `fs::metadata` fails for a NON-absence reason, in this
    /// platform's own spelling. Returned rather than inlined so the premise
    /// assertion below is identical on both.
    ///
    /// UNIX -- the original #238 red arm: a component that is a FILE rather
    /// than a directory makes `metadata` fail with `ENOTDIR`.
    ///
    /// WINDOWS (FerroxLabs/wayland-core#374): that same provocation maps to
    /// `ERROR_PATH_NOT_FOUND` (3), which Rust reports as `NotFound` -- the one
    /// kind the guard deliberately lets through -- so the premise could not be
    /// established and the test HARD-FAILED the nightly soak, 3 of 3 tries.
    /// A component longer than `NAME_MAX` is used instead. MEASURED on Windows
    /// 11 build 26200 rather than assumed, because two of the three
    /// provocations #374 suggested do not work here:
    ///
    /// ```text
    /// file-as-a-component      ERR kind=NotFound        raw=Some(3)
    /// component of 300 chars   ERR kind=InvalidFilename raw=Some(123)
    /// illegal character `<`    ERR kind=InvalidFilename raw=Some(123)
    /// 392-char path, MAX_PATH  ERR kind=NotFound        raw=Some(3)
    /// ```
    ///
    /// So the over-long *path* #374 proposed is `NotFound` on this build and
    /// would not have established the premise either; the over-long
    /// *component* does. It is preferred over the illegal-character spelling
    /// because every character in it is a legal filename character, so no
    /// earlier guard in `validate_user_path` can plausibly claim it first.
    fn a_non_absence_stat_failure(dir: &Path) -> PathBuf {
        #[cfg(not(windows))]
        {
            let file = dir.join("not-a-dir.txt");
            std::fs::write(&file, b"x").expect("write");
            file.join("child.txt")
        }
        #[cfg(windows)]
        {
            dir.join("L".repeat(300))
        }
    }

    /// #238 RED ARM. The `NonRegularFile` guard was written `if let
    /// Ok(meta)`, so any stat failure SKIPPED it and the path returned `Ok`,
    /// and the path sailed through.
    #[test]
    fn a_path_whose_metadata_fails_for_a_reason_other_than_absence_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let through_a_file = a_non_absence_stat_failure(dir.path());

        let err = std::fs::metadata(&through_a_file).unwrap_err();
        assert_ne!(
            err.kind(),
            std::io::ErrorKind::NotFound,
            "premise: this must be a NON-absence stat failure, got {err:?}"
        );

        let refusal = validate_user_path(&through_a_file)
            .expect_err("a target the OS refuses to describe must not be waved through");
        assert!(
            matches!(refusal, PathValidationError::Unstattable(_, _)),
            "expected Unstattable, got {refusal:?}"
        );
    }

    /// NEGATIVE CONTROL -- must pass in BOTH arms. A Write/Edit target whose
    /// leaf does not exist yet is the normal case and must stay allowed.
    #[test]
    fn a_not_yet_created_write_target_is_still_allowed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("new-file.txt");
        assert_eq!(
            std::fs::metadata(&target).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        validate_user_path(&target).expect("a nonexistent leaf under a real dir must be allowed");
    }

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
        // UNC guard must not claim either of them, whatever else happens to
        // them afterwards.
        for p in [r"\\?\C:\Users\me\notes.txt", r"\\.\PhysicalDrive0"] {
            if let Err(err) = validate_user_path(Path::new(p)) {
                assert!(
                    !matches!(err, PathValidationError::UncPath(_)),
                    "{p:?} must not be classified as UNC, got {err:?}"
                );
            }
        }
        // The DEVICE half is still refused on its namespace, on every platform.
        let err = validate_user_path(Path::new(r"\\.\PhysicalDrive0")).unwrap_err();
        assert!(
            matches!(err, PathValidationError::DeviceOrVerbatimPath(_)),
            "the device namespace must stay refused, got {err:?}"
        );
    }

    /// core#409 c2 — the namespace guard, pinned in BOTH directions.
    ///
    /// Graded on the classifier rather than through `validate_user_path`,
    /// because a `\\?\...` string is not `is_absolute()` on Unix: the admit arm
    /// would come back `NotAbsolute` there and prove nothing about the guard
    /// under test. The classifier is pure and answers the same on every
    /// platform, so both arms are exercised on every host.
    #[test]
    fn the_namespace_guard_refuses_devices_and_admits_verbatim_disks() {
        // REFUSE — raw devices, by either spelling.
        for refused in [
            r"\\.\PhysicalDrive0",
            r"\\.\pipe\wayland",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\secret",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x",
        ] {
            let path = Path::new(refused);
            assert!(
                looks_like_device_or_nondisk_verbatim(path, &path.to_string_lossy()),
                "{refused:?} reaches a raw device and must stay refused"
            );
        }

        // ADMIT — ordinary local files. `\\?\C:\...` is what
        // `std::fs::canonicalize` returns on Windows and what
        // `WorkspacePolicy::root()` stores, so refusing it refuses the
        // product's own canonical paths (core#409 c2).
        for admitted in [
            r"\\?\C:\Users\me\notes.txt",
            r"\\?\F:\ws\.wayland-out\results\toolu_01.txt",
            r"\\?\c:\Windows\ServiceProfiles\NetworkService\AppData\Local\Temp\.tmp",
        ] {
            let path = Path::new(admitted);
            assert!(
                !looks_like_device_or_nondisk_verbatim(path, &path.to_string_lossy()),
                "{admitted:?} is an ordinary local file"
            );
        }
    }

    /// core#409 c2, through the production entry point: the search root a
    /// session actually carries must be a legal search root.
    ///
    /// On Windows `std::fs::canonicalize` returns `\\?\C:\...`, which is what
    /// `WorkspacePolicy::root()` holds and therefore what Grep is handed; the
    /// guard refused it, and the failure surfaced as the CONTROL arm of
    /// `grep_vcs_content_store_deny` — an ordinary search of the workspace —
    /// dying with `path uses a Windows device / verbatim namespace`. On Unix
    /// `canonicalize` returns the plain form, so this arm is a control there
    /// and the regression itself is graded on the Windows host.
    #[test]
    fn a_canonicalized_workspace_root_is_a_valid_search_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let resolved = validate_search_root(&root, None).unwrap_or_else(|e| {
            panic!("canonicalize's own output was refused as a search root: {e}")
        });
        assert_eq!(resolved, lex_normalize(&root));

        // Both directions in one test: the device namespace is still refused
        // by the SAME entry point, so this cannot pass by the guard having
        // been deleted.
        let err = validate_search_root(Path::new(r"\\.\PhysicalDrive0"), None).unwrap_err();
        assert!(
            matches!(err, PathValidationError::DeviceOrVerbatimPath(_)),
            "the device namespace must stay refused as a search root, got {err:?}"
        );
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

    // ----- Linux procfs (credential disclosure via /proc) -----
    //
    // Unix-gated for the same reason as `ssh_private_key_rejected` above: a
    // `/proc/...` string is not an absolute path on Windows, so there it would
    // be refused as `NotAbsolute` and the assertion would pass without ever
    // reaching the procfs rule. On Linux `/proc/self/environ` is a REAL file,
    // so this is genuinely end-to-end there; on macOS it simply does not exist,
    // which the lexical rule does not care about.

    /// The core defect: `/proc/self/environ` is the agent's OWN environment,
    /// including every provider API key, and it is a regular file so no other
    /// guard in `validate_user_path` stops it.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: delete the
    /// `if is_denied_proc_path(path) { return true; }` block in
    /// `is_denied_system_path` and this returns `Ok` instead of `SystemPath`.
    #[cfg(unix)]
    #[test]
    fn proc_self_environ_rejected() {
        let err = validate_user_path(Path::new("/proc/self/environ")).unwrap_err();
        assert!(
            matches!(err, PathValidationError::SystemPath(_)),
            "/proc/self/environ leaks the agent's own API keys, got {err:?}"
        );
    }

    /// `/proc/self/environ` is one spelling. The whole per-process subtree is
    /// denied, so the pid / thread / task / memory / symlink-re-entry variants
    /// are refused too — including `/proc/self/root/...` and `/proc/self/cwd/...`,
    /// which otherwise let an attacker rebuild ANY denied path through procfs.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: delete the
    /// `if is_denied_proc_path(path) { return true; }` block in
    /// `is_denied_system_path`, or narrow the second-component match arm in
    /// `is_denied_proc_path` (dropping `"thread-self"`, `"kcore"`, or the
    /// `is_ascii_digit` pid arm), and the corresponding rows return `Ok`.
    #[cfg(unix)]
    #[test]
    fn proc_per_process_subtree_rejected() {
        for p in [
            // Any pid, not just self.
            "/proc/1/environ",
            "/proc/12345/environ",
            // Per-thread spellings.
            "/proc/thread-self/environ",
            "/proc/self/task/1/environ",
            "/proc/self/task/1/mem",
            // argv frequently carries secrets (`--api-key=`, `mysql -p<pw>`).
            "/proc/self/cmdline",
            "/proc/1/cmdline",
            // Direct memory reads — arguably worse than environ.
            "/proc/self/mem",
            "/proc/12345/mem",
            "/proc/kcore",
            // Symlink re-entry: rebuilds any path, including ones denied above.
            "/proc/self/root/etc/shadow",
            "/proc/self/root/proc/self/environ",
            "/proc/self/cwd/notes.txt",
            "/proc/1/root/home/alice/.ssh/id_rsa",
            // Open file descriptors of a running process.
            "/proc/self/fd/3",
        ] {
            let res = validate_user_path(Path::new(p));
            assert!(
                matches!(res, Err(PathValidationError::SystemPath(_))),
                "{p:?} must be denied as a system path, got {res:?}"
            );
        }
    }

    /// The boundary is a decision, not an accident: the system-wide procfs
    /// entries carry no process-private data and stay READABLE. This pins the
    /// current behaviour so widening the rule to all of `/proc` is a conscious,
    /// test-breaking change rather than a silent one.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: replace the second-component match
    /// arm in `is_denied_proc_path` with an unconditional `true` (i.e. deny all
    /// of `/proc`) and these become `SystemPath` errors.
    #[cfg(unix)]
    #[test]
    fn proc_system_wide_entries_still_allowed() {
        for p in [
            "/proc/cpuinfo",
            "/proc/meminfo",
            "/proc/version",
            // `/proc/sys/...` is left readable: its hazard is privileged
            // kernel-tunable WRITES, a separate finding from this one.
            "/proc/sys/kernel/hostname",
        ] {
            let res = validate_user_path(Path::new(p));
            assert!(
                res.is_ok(),
                "{p:?} carries no process-private data and must stay readable, got {res:?}"
            );
        }
    }

    /// Over-match guard. The rule matches path COMPONENTS rooted at `/proc`, so
    /// an innocuous workspace file that merely contains the same text — a real
    /// file at `<tmp>/proc/self/environ`, or one named `proc-notes.txt` — is
    /// still readable. A sloppy `s.contains("/proc/self/environ")` or
    /// `s.contains("proc")` implementation would pass every test above while
    /// silently breaking legitimate reads; this is the test that catches it.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: swap the component walk in
    /// `is_denied_proc_path` for a substring test against `s` and the
    /// `<tmp>/proc/self/environ` row starts failing.
    #[cfg(unix)]
    #[test]
    fn proc_lookalike_paths_outside_procfs_allowed() {
        let dir = tempfile::tempdir().unwrap();

        // A real workspace file literally named `proc/self/environ`.
        let nested = dir.path().join("proc/self/environ");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, b"workspace notes").unwrap();
        assert!(
            validate_user_path(&nested).is_ok(),
            "a workspace file named proc/self/environ must stay readable"
        );

        // A file whose name merely starts with `proc`.
        let notes = dir.path().join("proc-notes.txt");
        std::fs::write(&notes, b"notes").unwrap();
        assert!(
            validate_user_path(&notes).is_ok(),
            "proc-notes.txt must stay readable"
        );

        // Root-level lookalikes: `/procfs/...` and `/proc-notes.txt` are not
        // `/proc`. Neither exists, which is fine — the rule is lexical.
        for p in ["/procfs/self/environ", "/proc-notes.txt", "/proconly"] {
            let res = validate_user_path(Path::new(p));
            assert!(res.is_ok(), "{p:?} is not under /proc, got {res:?}");
        }
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
