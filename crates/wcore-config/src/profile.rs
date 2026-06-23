//! Isolated-profile control plane (Phase 1, Task 1.1).
//!
//! An *isolated profile* is a self-contained `WAYLAND_HOME`-rooted home (its own
//! config, credentials, OAuth, memory, skills). This module owns the
//! control-plane resolvers that LIST and LOCATE profiles — distinct from a
//! single profile's home (`config::profile_home()`), which resolves state
//! *inside* one profile.
//!
//! Load-bearing invariant (C2): [`profiles_root`] must NEVER read `WAYLAND_HOME`.
//! A profile home is a *child* of the profiles root, so reading `WAYLAND_HOME`
//! here would make the root resolve inside one of the very homes it enumerates.
//! Activation (Task 1.2) reads the `active` pointer ONCE at process entry and
//! materializes it into `WAYLAND_HOME`; nothing here is consulted again at
//! runtime.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Maximum profile-name length. Generous for human-chosen names while staying
/// well under filesystem component limits (255 bytes on ext4/APFS/NTFS).
pub const MAX_PROFILE_NAME_LEN: usize = 64;

/// Errors from profile-name validation and path resolution.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProfileError {
    /// The supplied name failed validation (C6). `reason` is a short,
    /// human-readable explanation suitable for surfacing to the CLI user.
    #[error("invalid profile name {name:?}: {reason}")]
    InvalidName { name: String, reason: &'static str },
}

/// Windows reserved device names (case-insensitive, with or without an
/// extension, e.g. `CON` and `con.txt` are both reserved). Rejected on every
/// platform so a profile created on Linux cannot become unusable when the same
/// home is opened on Windows.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Names reserved for the profiles control plane itself — flat entries that live
/// directly under [`profiles_root`] alongside the per-profile directories. A
/// profile may not take one of these, or its home directory would collide with
/// a control file. `active` is the [`active_pointer_path`] file; grow this list
/// whenever a new well-known control-plane entry is added (e.g. a future
/// `lock`). Compared case-folded, since [`profile_dir`] lowercases.
const RESERVED_PROFILE_NAMES: &[&str] = &["active"];

/// Validate a profile name (C6). The grammar is intentionally strict — these
/// names become filesystem directory components, so anything ambiguous across
/// platforms is rejected up front rather than sanitized:
///
/// * non-empty, at most [`MAX_PROFILE_NAME_LEN`] bytes;
/// * only ASCII letters, digits, `.`, `_`, `-` (this alone rejects every path
///   separator — `/`, `\` — plus `:`, spaces, NUL, and all control chars);
/// * not composed solely of dots (rejects `.`, `..`, `...` → traversal / cwd);
/// * no trailing `.` (Windows silently strips it → collides with the dotless
///   name);
/// * not a Windows reserved device name (`CON`, `NUL`, `COM1`…, with or without
///   an extension).
///
/// Case is NOT rejected here — `Work` and `work` are both valid names that map
/// to the SAME on-disk profile (see [`profile_dir`], which case-folds), matching
/// case-insensitive-filesystem semantics. A leading `.` is rejected (it would
/// create a hidden directory and invites dotfile confusion). The control-plane
/// names in [`RESERVED_PROFILE_NAMES`] (e.g. `active`) are rejected so a profile
/// home cannot collide with a control file under [`profiles_root`].
#[must_use = "validation result must be checked before using the name"]
pub fn validate_profile_name(name: &str) -> Result<(), ProfileError> {
    let invalid = |reason: &'static str| ProfileError::InvalidName {
        name: name.to_string(),
        reason,
    };

    if name.is_empty() {
        return Err(invalid("name must not be empty"));
    }
    if name.len() > MAX_PROFILE_NAME_LEN {
        return Err(invalid("name too long (max 64 bytes)"));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(invalid(
            "only ASCII letters, digits, '.', '_', '-' are allowed",
        ));
    }
    if name.bytes().all(|b| b == b'.') {
        return Err(invalid("name must not be all dots ('.', '..')"));
    }
    if name.starts_with('.') {
        return Err(invalid("name must not start with '.'"));
    }
    if name.starts_with('-') {
        // A leading '-' makes the name look like a CLI flag (e.g. `--profile
        // --help` would otherwise resolve the name "--help"); reject it.
        return Err(invalid("name must not start with '-'"));
    }
    if name.ends_with('.') {
        return Err(invalid("name must not end with '.'"));
    }
    // Reserved-device check is on the stem (portion before the first '.'),
    // because Windows reserves `CON` AND `CON.anything`.
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if WINDOWS_RESERVED.contains(&stem.as_str()) {
        return Err(invalid("name is a reserved device name on Windows"));
    }
    if RESERVED_PROFILE_NAMES.contains(&name.to_ascii_lowercase().as_str()) {
        return Err(invalid("name is reserved for the profiles control plane"));
    }
    Ok(())
}

/// The control-plane root that LISTS profiles.
///
/// Resolution order:
///   1. `WAYLAND_PROFILES_ROOT` env var (explicit escape hatch / sandbox);
///   2. `<os-native config dir>/wayland-core-profiles/` — a SIBLING of the
///      legacy home, so the existing single home stays untouched as the implicit
///      `default` profile.
///
/// NEVER reads `WAYLAND_HOME` (C2). The override must be an ABSOLUTE,
/// control-char-free path — a relative override would make the profiles root
/// (and thus every profile home) depend on the process CWD, so it is ignored
/// and resolution falls through to the default. The last-resort fallback
/// anchors to the current dir (absolute) only if the OS config dir cannot be
/// resolved at all.
#[must_use]
pub fn profiles_root() -> PathBuf {
    if let Ok(custom) = std::env::var("WAYLAND_PROFILES_ROOT")
        && !custom.is_empty()
        && !custom.chars().any(|c| c.is_control())
        && Path::new(&custom).is_absolute()
    {
        return PathBuf::from(custom);
    }
    crate::config::os_native_config_root()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wayland-core-profiles")
}

/// The on-disk directory for a named profile, under [`profiles_root`].
///
/// The name is validated and **case-folded to lowercase** so `Work` and `work`
/// resolve to the same directory on every platform (deterministic identity on
/// case-sensitive *and* case-insensitive filesystems — C6). Returns
/// [`ProfileError`] for an invalid name rather than silently joining a hostile
/// component.
#[must_use = "the resolved profile directory should be used"]
pub fn profile_dir(name: &str) -> Result<PathBuf, ProfileError> {
    validate_profile_name(name)?;
    Ok(profiles_root().join(name.to_ascii_lowercase()))
}

/// Path of the `active` pointer file — a tiny file under [`profiles_root`]
/// holding the name of the profile to activate when neither `WAYLAND_HOME` nor
/// `--profile` is supplied. Read ONCE at process entry by activation (Task 1.2)
/// and never again (C2). Living at the control-plane root (not inside any home)
/// keeps it outside every profile's isolation boundary.
pub fn active_pointer_path() -> PathBuf {
    profiles_root().join("active")
}

/// Read the `active` pointer file. This is the SOLE place the pointer is read
/// for the launch decision (C2 / D2) — the `active_pointer_path` single-reader
/// CI lint (`tests/active_pointer_single_reader_test.rs`) keeps it that way.
/// Returns the trimmed profile name, or `None` if the file is absent, empty, or
/// unreadable (a corrupt pointer must never abort launch — it falls through to
/// the default home).
fn read_active_pointer() -> Option<String> {
    let raw = std::fs::read_to_string(active_pointer_path()).ok()?;
    let name = raw.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Extract the value of `--profile <name>` or `--profile=<name>` from a raw
/// argv iterator, WITHOUT a full clap parse (which cannot run before the home
/// is resolved). Returns the first occurrence. Scanning stops at `--`
/// (end-of-options), so a literal `--profile` inside a user prompt after `--`
/// is not mistaken for the flag.
fn profile_flag_from_args(args: impl Iterator<Item = String>) -> Option<String> {
    let mut args = args;
    // Skip argv[0] (the program name).
    let _ = args.next();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if let Some(val) = arg.strip_prefix("--profile=") {
            return Some(val.to_string());
        }
        if arg == "--profile" {
            return args.next();
        }
    }
    None
}

/// THE single source-of-truth resolver (C2). Resolve the active profile ONCE, at
/// process entry, and materialize it into `WAYLAND_HOME`. After this returns,
/// `WAYLAND_HOME` is the only thing any code (or child process) consults; the
/// `active` pointer is never read again — the precise failure that corrupts
/// that corruption bug, where a sticky pointer and the env var can disagree deep in
/// the stack.
///
/// Resolution order:
///   1. `WAYLAND_HOME` already set → an explicit override always wins; return.
///   2. `--profile <name>` / `--profile=<name>` on argv.
///   3. else the `active` pointer file.
///
/// A resolved name whose [`profile_dir`] exists is set as `WAYLAND_HOME`. An
/// invalid name, or a name whose directory does not exist, warns to stderr and
/// falls through to the legacy default home (NEVER aborts — `--help` must run).
///
/// # Panics / threading
/// Calls [`std::env::set_var`], which is sound ONLY while the process is
/// single-threaded. The sole caller is `wcore-cli`'s `main()`, immediately
/// before `load_wayland_env_file()` and before any thread (or the Tokio
/// runtime) is spawned.
pub fn activate_for_launch() {
    activate_for_launch_impl(std::env::args());
}

/// Testable core of [`activate_for_launch`] — argv is injected so tests can
/// exercise the `--profile` path without mutating the real process arguments.
fn activate_for_launch_impl(args: impl Iterator<Item = String>) {
    // 1. Explicit WAYLAND_HOME wins — never override it.
    if std::env::var_os("WAYLAND_HOME").is_some() {
        return;
    }

    // 2. --profile on argv, else 3. the active pointer.
    let Some(name) = profile_flag_from_args(args).or_else(read_active_pointer) else {
        return;
    };

    match profile_dir(&name) {
        Ok(dir) if dir.is_dir() => {
            // SAFETY: single-threaded at process entry (see the doc comment and
            // the `main()` call site). No other thread can observe the env race.
            unsafe { std::env::set_var("WAYLAND_HOME", &dir) };
        }
        Ok(dir) => {
            eprintln!(
                "warning: profile {name:?} not found at {} — using the default home",
                dir.display()
            );
        }
        Err(e) => {
            eprintln!("warning: ignoring invalid profile selection: {e}");
        }
    }
}

// ===========================================================================
// Phase 2: profile management control plane (CRUD + active-pointer writers +
// export/import). EVERYTHING that reads OR writes the `active` pointer lives in
// this file (D2 single-reader/writer lint); the CLI calls these helpers and
// never touches the pointer file directly.
// ===========================================================================

/// Errors from profile management operations (CRUD, pointer writes,
/// export/import). Distinct from [`ProfileError`] (pure name validation) because
/// these wrap I/O — so this type is intentionally NOT `PartialEq`/`Eq`. A name
/// problem surfaces through the `#[from] ProfileError` variant.
#[derive(Debug, Error)]
pub enum ProfileOpError {
    /// The supplied name failed validation (C6); forwarded from
    /// [`validate_profile_name`].
    #[error(transparent)]
    Name(#[from] ProfileError),

    /// A profile with this (case-folded) name already exists on disk.
    #[error("profile {0:?} already exists")]
    AlreadyExists(String),

    /// No profile with this (case-folded) name exists on disk.
    #[error("profile {0:?} does not exist")]
    NotFound(String),

    /// An import/adopt source path escaped its expected root (path traversal,
    /// absolute escape, or zip-slip-style `..`). C6 / SECURITY.
    #[error("refusing unsafe path {path:?}: {reason}")]
    UnsafePath { path: String, reason: &'static str },

    /// Underlying filesystem error, tagged with the operation for context.
    #[error("{op} failed: {source}")]
    Io {
        op: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl ProfileOpError {
    fn io(op: &'static str) -> impl FnOnce(std::io::Error) -> ProfileOpError {
        move |source| ProfileOpError::Io { op, source }
    }
}

/// Secret-bearing entries inside a profile home that must NEVER be exported
/// (without an explicit opt-in) nor copied by `create --base`. Matched against
/// each top-level entry's file name: an exact match for a directory (`oauth`)
/// or a `credentials.*` / exact `credentials` prefix match for files. Keep this
/// the single source of truth for "what is a secret in a profile home".
const SECRET_DIR_NAMES: &[&str] = &["oauth"];
const SECRET_FILE_PREFIX: &str = "credentials";

/// Decide whether a top-level entry name within a profile home is a secret that
/// the default (secret-excluding) export/copy path must skip. Directories named
/// `oauth/` and any `credentials*` file (`credentials.toml`, `credentials.enc`,
/// `credentials.kdf.json`) are secrets.
///
/// Assumption: secrets live at the profile-home ROOT (true today —
/// `credentials*` resolves under `wayland_config_dir()` == `WAYLAND_HOME` and
/// `oauth/` under `profile_home()` when a profile is active). The export filter
/// only special-cases the top level; if the credential/oauth layout ever nests
/// (e.g. `providers/<x>/credentials.toml`), update this set AND the copy depth
/// handling together.
fn is_secret_entry(file_name: &str) -> bool {
    if SECRET_DIR_NAMES.contains(&file_name) {
        return true;
    }
    // `credentials` exact, or `credentials.<ext>` — but NOT e.g.
    // `credentials-notes` (defensive: only the dotted family is a secret).
    file_name == SECRET_FILE_PREFIX
        || file_name
            .strip_prefix(SECRET_FILE_PREFIX)
            .is_some_and(|rest| rest.starts_with('.'))
}

// --- active-pointer writers / display reader (D2: must live HERE) ----------

/// Public, display-only reader of the active-profile name (e.g. for `profile
/// list` / `profile show` to mark the active row). This is the ONLY sanctioned
/// way for the CLI to learn the active profile WITHOUT touching the pointer file
/// — the single-reader lint (`tests/active_pointer_single_reader_test.rs`) bans
/// pointer access outside this module, so list/show call this instead. Returns
/// the trimmed, case-folded name, or `None` if unset/empty/unreadable. Does NOT
/// verify the profile still exists on disk (callers can cross-check against
/// [`list_profiles`]); a dangling pointer is reported verbatim so the user can
/// see and repair it.
#[must_use]
pub fn active_profile_name() -> Option<String> {
    read_active_pointer().map(|n| n.to_ascii_lowercase())
}

/// Set the active profile by atomically writing its (validated, case-folded)
/// name to [`active_pointer_path`]. The profile MUST already exist on disk
/// (`profile set` does not create). Uses temp-file + rename ([`atomic_write`]),
/// so a crash mid-write can never leave a half-written pointer that would
/// mis-route the next launch (C2 / SECURITY: atomic pointer writes).
///
/// Errors: [`ProfileOpError::Name`] for an invalid name, [`NotFound`] if the
/// profile directory is absent, [`Io`] on write failure.
///
/// [`NotFound`]: ProfileOpError::NotFound
pub fn set_active_profile(name: &str) -> Result<(), ProfileOpError> {
    let dir = profile_dir(name)?; // validates + case-folds
    if !dir.is_dir() {
        return Err(ProfileOpError::NotFound(name.to_ascii_lowercase()));
    }
    let lower = name.to_ascii_lowercase();
    let ptr = active_pointer_path();
    if let Some(parent) = ptr.parent() {
        std::fs::create_dir_all(parent).map_err(ProfileOpError::io("create profiles root"))?;
    }
    // Newline-terminated to match what activation tolerates (read_active_pointer
    // trims) and what a human `cat` expects.
    crate::atomic_write(&ptr, format!("{lower}\n").as_bytes())
        .map_err(ProfileOpError::io("write active pointer"))
}

/// Clear the active pointer (remove the file). Used when the active profile is
/// deleted or renamed away, so the next launch falls through to the default
/// home rather than chasing a dangling name. Removing a non-existent pointer is
/// a no-op success (idempotent). Re-pointing on rename uses
/// [`set_active_profile`] instead.
pub fn clear_active_profile() -> Result<(), ProfileOpError> {
    let ptr = active_pointer_path();
    match std::fs::remove_file(&ptr) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ProfileOpError::Io {
            op: "clear active pointer",
            source: e,
        }),
    }
}

/// True iff `name` is currently the active profile (case-folded compare). Reads
/// the pointer via [`active_profile_name`] (the sanctioned display reader), so
/// the CLI can refuse to delete/rename the active profile without `--force`
/// without itself touching the pointer file.
#[must_use]
pub fn is_active(name: &str) -> bool {
    match validate_profile_name(name) {
        Ok(()) => active_profile_name().as_deref() == Some(name.to_ascii_lowercase().as_str()),
        Err(_) => false,
    }
}

// --- enumeration / existence ----------------------------------------------

/// Enumerate the profiles under [`profiles_root`], sorted ascending. Returns the
/// on-disk (already-lowercase) directory names that pass [`validate_profile_name`].
/// Skips: the `active` pointer file, any non-directory entry, and any entry whose
/// name fails validation (defensive — a manually-created junk dir never surfaces
/// as a usable profile). A missing root yields an empty list, not an error.
#[must_use]
pub fn list_profiles() -> Vec<String> {
    let root = profiles_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| validate_profile_name(name).is_ok())
        .collect();
    names.sort();
    names
}

/// True iff a profile directory for `name` exists (validated + case-folded). A
/// validation failure returns `false` (an invalid name can have no profile).
#[must_use]
pub fn profile_exists(name: &str) -> bool {
    match profile_dir(name) {
        Ok(dir) => dir.is_dir(),
        Err(_) => false,
    }
}

// --- create / rename / delete ---------------------------------------------

/// Create a new, empty profile directory under [`profiles_root`] and return its
/// path. Errors with [`ProfileOpError::AlreadyExists`] if one is already there.
///
/// If `base` is supplied, it must name an existing profile; a `profile.toml`
/// recording `base = "<base>"` is written inside the new dir. Inheritance is
/// RESOLVED LATER (Phase 3) — create NEVER copies any state, and in particular
/// never copies credentials or `oauth/` (SECURITY). The marker file is the only
/// content written.
pub fn create_profile(name: &str, base: Option<&str>) -> Result<PathBuf, ProfileOpError> {
    let dir = profile_dir(name)?; // validates + case-folds
    if dir.exists() {
        return Err(ProfileOpError::AlreadyExists(name.to_ascii_lowercase()));
    }
    // Validate the base name (and its existence) BEFORE creating anything, so a
    // bad --base leaves no half-made profile behind.
    let base_lower = match base {
        Some(b) => {
            let base_dir = profile_dir(b)?;
            if !base_dir.is_dir() {
                return Err(ProfileOpError::NotFound(b.to_ascii_lowercase()));
            }
            Some(b.to_ascii_lowercase())
        }
        None => None,
    };

    std::fs::create_dir_all(&dir).map_err(ProfileOpError::io("create profile dir"))?;

    if let Some(base_lower) = base_lower {
        // Minimal, hand-written TOML — no serde round-trip needed for one key,
        // and the base name is already validated (ASCII [A-Za-z0-9._-]) so it
        // needs no TOML string escaping.
        let marker = format!("base = \"{base_lower}\"\n");
        crate::atomic_write(dir.join("profile.toml"), marker.as_bytes())
            .map_err(ProfileOpError::io("write profile.toml"))?;
    }
    Ok(dir)
}

/// Rename a profile directory `old` → `new`. Both names are validated and
/// case-folded. Errors: [`NotFound`] if `old` is absent, [`AlreadyExists`] if
/// `new` is taken. On a same-name case-only rename (`work` → `Work`, identical
/// on-disk path) this is a no-op success. If the active pointer currently names
/// `old`, it is re-pointed to `new` (so the active selection follows the rename).
///
/// Non-atomicity: the directory move and the pointer re-point are two steps. If
/// the pointer write fails AFTER the move succeeded, this returns `Err` even
/// though the rename already happened, leaving the pointer naming the old (now
/// absent) profile. That dangling pointer is harmless — activation warns and
/// falls through to the default home — but the error does not convey that the
/// rename itself succeeded.
///
/// [`NotFound`]: ProfileOpError::NotFound
/// [`AlreadyExists`]: ProfileOpError::AlreadyExists
pub fn rename_profile(old: &str, new: &str) -> Result<(), ProfileOpError> {
    let old_dir = profile_dir(old)?;
    let new_dir = profile_dir(new)?;
    let old_lower = old.to_ascii_lowercase();
    let new_lower = new.to_ascii_lowercase();

    if old_dir == new_dir {
        // Case-only "rename" maps to the same directory — nothing to move. The
        // pointer (if it named old) already equals new case-folded.
        return if old_dir.is_dir() {
            Ok(())
        } else {
            Err(ProfileOpError::NotFound(old_lower))
        };
    }
    if !old_dir.is_dir() {
        return Err(ProfileOpError::NotFound(old_lower));
    }
    if new_dir.exists() {
        return Err(ProfileOpError::AlreadyExists(new_lower));
    }

    // Capture active-status BEFORE the move (read via the sanctioned reader).
    let was_active = is_active(&old_lower);

    std::fs::rename(&old_dir, &new_dir).map_err(ProfileOpError::io("rename profile dir"))?;

    if was_active {
        // Re-point so the active selection follows the rename. The new dir now
        // exists, so set_active_profile's existence check passes.
        set_active_profile(&new_lower)?;
    }
    Ok(())
}

/// Remove a profile's directory tree. This is the low-level removal — it does
/// NOT consult or clear the active pointer and does NOT refuse the active
/// profile; the CLI layer decides that policy (via [`is_active`] + a `--force`
/// flag) and is responsible for calling [`clear_active_profile`] afterward when
/// it deletes the active one. Errors [`NotFound`] if the profile is absent.
///
/// [`NotFound`]: ProfileOpError::NotFound
pub fn delete_profile_dir(name: &str) -> Result<(), ProfileOpError> {
    let dir = profile_dir(name)?;
    if !dir.is_dir() {
        return Err(ProfileOpError::NotFound(name.to_ascii_lowercase()));
    }
    std::fs::remove_dir_all(&dir).map_err(ProfileOpError::io("remove profile dir"))
}

/// Delete a profile, clearing the active pointer first if this profile is the
/// active one. Convenience wrapper over [`is_active`], [`clear_active_profile`],
/// and [`delete_profile_dir`] for the common CLI path AFTER the handler has
/// already confirmed intent (e.g. behind `--force`/`--yes` when active). The
/// handler still owns the refuse-without-force decision — this function does not
/// second-guess it, it just keeps the pointer consistent with the deletion.
pub fn delete_profile(name: &str) -> Result<(), ProfileOpError> {
    let lower = name.to_ascii_lowercase();
    if is_active(&lower) {
        clear_active_profile()?;
    }
    delete_profile_dir(&lower)
}

// --- export / import (recursive copy; secret-excluding by default) ----------

/// Recursively copy `src` into `dst`, creating `dst` and parents. When
/// `skip_secrets` is true, TOP-LEVEL secret entries ([`is_secret_entry`]) are
/// skipped (we only special-case the top level, since `credentials*` / `oauth/`
/// live directly in a profile home). Symlinks are NOT followed and NOT recreated
/// (C6: no symlink/junction sharing) — a symlink encountered in the tree is
/// skipped with no error, so a hostile symlink can never redirect the copy out
/// of the tree.
fn copy_tree_filtered(src: &Path, dst: &Path, skip_secrets: bool) -> Result<(), ProfileOpError> {
    std::fs::create_dir_all(dst).map_err(ProfileOpError::io("create export dir"))?;
    copy_tree_inner(src, dst, skip_secrets, true)
}

fn copy_tree_inner(
    src: &Path,
    dst: &Path,
    skip_secrets: bool,
    is_top_level: bool,
) -> Result<(), ProfileOpError> {
    for entry in std::fs::read_dir(src).map_err(ProfileOpError::io("read source dir"))? {
        let entry = entry.map_err(ProfileOpError::io("read source entry"))?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if is_top_level && skip_secrets && is_secret_entry(&name_str) {
            continue;
        }
        let from = entry.path();
        // Use symlink_metadata so a symlink is detected as a symlink (not its
        // target) and skipped — never followed (C6).
        let meta = from
            .symlink_metadata()
            .map_err(ProfileOpError::io("stat source entry"))?;
        let to = dst.join(&name);
        if meta.file_type().is_symlink() {
            continue;
        }
        // On Windows, an NTFS directory JUNCTION (and any other reparse point) is
        // NOT classified as a symlink by `is_symlink()`, yet it redirects just
        // like one — and a junction needs no special privilege to create. Skip
        // any reparse point by its file attribute so a hostile/import tree cannot
        // redirect the copy out of the source via a junction (C6 / zip-slip).
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                continue;
            }
        }
        if meta.is_dir() {
            std::fs::create_dir_all(&to).map_err(ProfileOpError::io("create dir"))?;
            copy_tree_inner(&from, &to, skip_secrets, false)?;
        } else {
            std::fs::copy(&from, &to).map_err(ProfileOpError::io("copy file"))?;
        }
    }
    Ok(())
}

/// Export a profile's home tree to `dst_dir` (a plain directory the caller
/// chooses). By default secrets are EXCLUDED ([`is_secret_entry`]); pass
/// `include_secrets = true` to copy them too — the CLI MUST warn on stderr when
/// it does. Returns the destination path. Errors [`NotFound`] if the profile is
/// absent. `dst_dir` must not already exist as a non-empty target the caller
/// cares about — we create it and copy into it (existing files with the same
/// name are overwritten by `std::fs::copy`).
///
/// [`NotFound`]: ProfileOpError::NotFound
pub fn export_profile(
    name: &str,
    dst_dir: &Path,
    include_secrets: bool,
) -> Result<PathBuf, ProfileOpError> {
    let src = profile_dir(name)?;
    if !src.is_dir() {
        return Err(ProfileOpError::NotFound(name.to_ascii_lowercase()));
    }
    copy_tree_filtered(&src, dst_dir, !include_secrets)?;
    Ok(dst_dir.to_path_buf())
}

/// Import (adopt) a directory tree `src_dir` as a NEW profile `name`. The new
/// profile must not already exist ([`AlreadyExists`]). `src_dir` is validated
/// against path-escape: it must be an existing directory, and after
/// canonicalization every entry copied stays within it (the recursive copy never
/// follows symlinks, which is the zip-slip / path-escape defense — C6 /
/// SECURITY). Secrets in the source are NOT filtered on import (the user is
/// adopting a tree they supplied); the caller decides whether the source was an
/// exclude-secrets export. Returns the created profile dir.
///
/// [`AlreadyExists`]: ProfileOpError::AlreadyExists
pub fn import_profile(name: &str, src_dir: &Path) -> Result<PathBuf, ProfileOpError> {
    let dst = profile_dir(name)?; // validates + case-folds
    if dst.exists() {
        return Err(ProfileOpError::AlreadyExists(name.to_ascii_lowercase()));
    }
    // Reject a non-existent / non-dir / non-canonicalizable source up front.
    let canonical_src = src_dir
        .canonicalize()
        .map_err(|_| ProfileOpError::UnsafePath {
            path: src_dir.display().to_string(),
            reason: "source path does not exist or cannot be resolved",
        })?;
    if !canonical_src.is_dir() {
        return Err(ProfileOpError::UnsafePath {
            path: src_dir.display().to_string(),
            reason: "import source must be a directory",
        });
    }
    // copy_tree_filtered with skip_secrets=false performs a full recursive copy.
    // It never follows symlinks, so no entry in the source can redirect the
    // copy outside `canonical_src` (zip-slip / `..` escape defense).
    copy_tree_filtered(&canonical_src, &dst, false)?;
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::tempdir;

    /// RAII env guard — restores prior values on drop so env-mutating tests stay
    /// hermetic even under a thread-per-test `cargo test` runner.
    struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl EnvGuard {
        fn set(pairs: &[(&'static str, Option<&str>)]) -> Self {
            let saved = pairs
                .iter()
                .map(|(k, v)| {
                    let prev = std::env::var_os(k);
                    match v {
                        Some(val) => unsafe { std::env::set_var(k, val) },
                        None => unsafe { std::env::remove_var(k) },
                    }
                    (*k, prev)
                })
                .collect();
            Self(saved)
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, prev) in &self.0 {
                match prev {
                    Some(v) => unsafe { std::env::set_var(k, v) },
                    None => unsafe { std::env::remove_var(k) },
                }
            }
        }
    }

    #[test]
    fn accepts_reasonable_names() {
        for ok in [
            "work",
            "Work",
            "my-profile_2.test",
            "a",
            "client.acme",
            "x-1",
        ] {
            assert!(validate_profile_name(ok).is_ok(), "should accept {ok:?}");
        }
    }

    #[test]
    fn rejects_traversal_and_separators() {
        for bad in [
            "", "..", ".", "...", "a/b", "a\\b", "../etc", "a\0b", "a b", "a:b", "foo.", "café",
            "a/../b", ".hidden", ".", "..foo", "-foo", "--help", "-",
        ] {
            assert!(validate_profile_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_control_plane_reserved_names() {
        // A profile named "active" would collide with active_pointer_path().
        for bad in ["active", "Active", "ACTIVE"] {
            assert!(
                validate_profile_name(bad).is_err(),
                "should reject control-plane name {bad:?}"
            );
        }
        // ...but a name that merely contains it is fine.
        assert!(validate_profile_name("active-work").is_ok());
    }

    #[test]
    fn rejects_overlong_name() {
        let long = "a".repeat(MAX_PROFILE_NAME_LEN + 1);
        assert!(validate_profile_name(&long).is_err());
        let max = "a".repeat(MAX_PROFILE_NAME_LEN);
        assert!(validate_profile_name(&max).is_ok());
    }

    #[test]
    fn rejects_windows_reserved_names_case_insensitively() {
        for bad in [
            "CON", "con", "Nul", "COM1", "lpt9", "aux", "con.txt", "NUL.cfg",
        ] {
            assert!(
                validate_profile_name(bad).is_err(),
                "should reject reserved {bad:?}"
            );
        }
        // ...but a name that merely CONTAINS a reserved word is fine.
        assert!(validate_profile_name("console").is_ok());
        assert!(validate_profile_name("com10").is_ok());
    }

    #[test]
    #[serial]
    fn profiles_root_ignores_wayland_home() {
        // profiles_root() must resolve identically whether or not WAYLAND_HOME
        // is set, and must NEVER be a child of it (C2).
        let _g = EnvGuard::set(&[("WAYLAND_HOME", None), ("WAYLAND_PROFILES_ROOT", None)]);
        let without = profiles_root();

        let _g2 = EnvGuard::set(&[("WAYLAND_HOME", Some("/tmp/some-isolated-home"))]);
        let with = profiles_root();

        assert_eq!(
            without, with,
            "profiles_root must not depend on WAYLAND_HOME"
        );
        assert!(
            !with.starts_with("/tmp/some-isolated-home"),
            "profiles_root must never resolve inside a profile home"
        );
    }

    /// An absolute path valid on BOTH Unix (`/tmp/<seg>`) and Windows
    /// (`C:\<seg>`). `profiles_root()` rejects non-absolute overrides, and a
    /// Unix `/tmp/...` is NOT absolute on Windows — so hardcoding it makes these
    /// path-resolution tests fail under Windows CI (the override is rejected and
    /// resolution falls through to the real config dir).
    fn abs_root(seg: &str) -> String {
        if cfg!(windows) {
            format!("C:\\{seg}")
        } else {
            format!("/tmp/{seg}")
        }
    }

    #[test]
    #[serial]
    fn profiles_root_honors_explicit_override() {
        let root = abs_root("custom-profiles");
        let _g = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some(root.as_str()))]);
        assert_eq!(profiles_root(), PathBuf::from(&root));

        // A control-char-bearing override is ignored (falls through to default).
        let bad = format!("{}\nroot", abs_root("bad"));
        let _g2 = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some(bad.as_str()))]);
        assert_ne!(profiles_root(), PathBuf::from(&bad));

        // A RELATIVE override is ignored — would make every home CWD-dependent.
        let _g3 = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some("relative/profiles"))]);
        let r = profiles_root();
        assert_ne!(r, PathBuf::from("relative/profiles"));
        assert!(
            r.is_absolute() || r == PathBuf::from(".").join("wayland-core-profiles"),
            "default profiles_root should be absolute (or the cwd-less fallback)"
        );
    }

    #[test]
    #[serial]
    fn profile_dir_case_folds_to_same_path() {
        let root = abs_root("p");
        let _g = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some(root.as_str()))]);
        let upper = profile_dir("Work").unwrap();
        let lower = profile_dir("work").unwrap();
        assert_eq!(upper, lower, "Work and work must map to the same directory");
        assert_eq!(lower, PathBuf::from(&root).join("work"));
    }

    #[test]
    #[serial]
    fn profile_dir_rejects_invalid_name() {
        let root = abs_root("p");
        let _g = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some(root.as_str()))]);
        assert!(profile_dir("../escape").is_err());
        assert!(profile_dir("a/b").is_err());
    }

    #[test]
    #[serial]
    fn active_pointer_is_under_root_not_in_a_home() {
        let root = abs_root("p");
        let home = abs_root("some-home");
        let _g = EnvGuard::set(&[
            ("WAYLAND_PROFILES_ROOT", Some(root.as_str())),
            ("WAYLAND_HOME", Some(home.as_str())),
        ]);
        let ptr = active_pointer_path();
        assert_eq!(ptr, PathBuf::from(&root).join("active"));
        assert!(!ptr.starts_with(&home));
    }

    fn argv(parts: &[&str]) -> std::vec::IntoIter<String> {
        parts
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn profile_flag_parsing() {
        assert_eq!(
            profile_flag_from_args(argv(&["wcore", "--profile", "work"])),
            Some("work".to_string())
        );
        assert_eq!(
            profile_flag_from_args(argv(&["wcore", "--profile=work"])),
            Some("work".to_string())
        );
        assert_eq!(profile_flag_from_args(argv(&["wcore", "run"])), None);
        // First occurrence wins.
        assert_eq!(
            profile_flag_from_args(argv(&["wcore", "--profile", "a", "--profile", "b"])),
            Some("a".to_string())
        );
        // A `--profile` after `--` (end-of-options) is NOT the flag.
        assert_eq!(
            profile_flag_from_args(argv(&["wcore", "--", "--profile", "x"])),
            None
        );
        // argv[0] named --profile is skipped.
        assert_eq!(profile_flag_from_args(argv(&["--profile"])), None);
        // A trailing --profile with no value resolves to None (falls through).
        assert_eq!(profile_flag_from_args(argv(&["wcore", "--profile"])), None);
    }

    #[test]
    #[serial]
    fn activate_respects_existing_wayland_home() {
        let root = tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("work")).unwrap();
        let _g = EnvGuard::set(&[
            ("WAYLAND_PROFILES_ROOT", Some(root.path().to_str().unwrap())),
            ("WAYLAND_HOME", Some("/explicit/home")),
        ]);
        activate_for_launch_impl(argv(&["wcore", "--profile", "work"]));
        // Explicit WAYLAND_HOME must win — never overridden by --profile.
        assert_eq!(std::env::var("WAYLAND_HOME").unwrap(), "/explicit/home");
    }

    #[test]
    #[serial]
    fn activate_sets_home_from_profile_flag() {
        let root = tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let _g = EnvGuard::set(&[
            ("WAYLAND_PROFILES_ROOT", Some(root.path().to_str().unwrap())),
            ("WAYLAND_HOME", None),
        ]);
        activate_for_launch_impl(argv(&["wcore", "--profile", "Work"])); // case-folds
        assert_eq!(
            std::env::var_os("WAYLAND_HOME"),
            Some(work.into_os_string())
        );
    }

    #[test]
    #[serial]
    fn activate_reads_pointer_when_no_flag() {
        let root = tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(root.path().join("active"), "work\n").unwrap();
        let _g = EnvGuard::set(&[
            ("WAYLAND_PROFILES_ROOT", Some(root.path().to_str().unwrap())),
            ("WAYLAND_HOME", None),
        ]);
        activate_for_launch_impl(argv(&["wcore"]));
        assert_eq!(
            std::env::var_os("WAYLAND_HOME"),
            Some(work.into_os_string())
        );
    }

    #[test]
    #[serial]
    fn activate_flag_wins_over_pointer() {
        let root = tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("flagged")).unwrap();
        std::fs::create_dir_all(root.path().join("pointed")).unwrap();
        std::fs::write(root.path().join("active"), "pointed").unwrap();
        let _g = EnvGuard::set(&[
            ("WAYLAND_PROFILES_ROOT", Some(root.path().to_str().unwrap())),
            ("WAYLAND_HOME", None),
        ]);
        activate_for_launch_impl(argv(&["wcore", "--profile", "flagged"]));
        assert_eq!(
            std::env::var_os("WAYLAND_HOME"),
            Some(root.path().join("flagged").into_os_string())
        );
    }

    #[test]
    #[serial]
    fn activate_falls_through_on_missing_dir() {
        let root = tempdir().unwrap();
        let _g = EnvGuard::set(&[
            ("WAYLAND_PROFILES_ROOT", Some(root.path().to_str().unwrap())),
            ("WAYLAND_HOME", None),
        ]);
        activate_for_launch_impl(argv(&["wcore", "--profile", "ghost"]));
        // Missing profile dir → warn + fall through; WAYLAND_HOME stays unset.
        assert_eq!(std::env::var_os("WAYLAND_HOME"), None);
    }

    #[test]
    #[serial]
    fn activate_falls_through_on_invalid_name() {
        let root = tempdir().unwrap();
        let _g = EnvGuard::set(&[
            ("WAYLAND_PROFILES_ROOT", Some(root.path().to_str().unwrap())),
            ("WAYLAND_HOME", None),
        ]);
        activate_for_launch_impl(argv(&["wcore", "--profile", "../escape"]));
        assert_eq!(std::env::var_os("WAYLAND_HOME"), None);
    }

    // --- Phase 2 management helpers ----------------------------------------

    /// Set WAYLAND_PROFILES_ROOT to a fresh tempdir and return (guard, dir).
    /// All Phase-2 tests run serial because they mutate the process env.
    fn rooted() -> (EnvGuard, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let g = EnvGuard::set(&[
            ("WAYLAND_PROFILES_ROOT", Some(dir.path().to_str().unwrap())),
            ("WAYLAND_HOME", None),
        ]);
        (g, dir)
    }

    #[test]
    #[serial]
    fn create_lists_and_exists() {
        let (_g, root) = rooted();
        assert!(!profile_exists("work"));
        let dir = create_profile("work", None).unwrap();
        assert_eq!(dir, root.path().join("work"));
        assert!(dir.is_dir());
        assert!(profile_exists("Work")); // case-folds
        // No secrets / no marker for a base-less create.
        assert!(!dir.join("profile.toml").exists());
        assert!(!dir.join("credentials.toml").exists());

        create_profile("client.acme", None).unwrap();
        assert_eq!(list_profiles(), vec!["client.acme", "work"]); // sorted
    }

    #[test]
    #[serial]
    fn create_rejects_duplicate_and_invalid() {
        let (_g, _root) = rooted();
        create_profile("work", None).unwrap();
        assert!(matches!(
            create_profile("Work", None), // same on-disk dir
            Err(ProfileOpError::AlreadyExists(_))
        ));
        assert!(matches!(
            create_profile("../escape", None),
            Err(ProfileOpError::Name(_))
        ));
    }

    #[test]
    #[serial]
    fn create_with_base_writes_marker_and_never_copies_secrets() {
        let (_g, root) = rooted();
        let base = create_profile("base", None).unwrap();
        // Plant a secret in the base; create --base must NOT copy it.
        std::fs::write(base.join("credentials.toml"), "[secrets]\nx='y'\n").unwrap();
        std::fs::create_dir_all(base.join("oauth")).unwrap();
        std::fs::write(base.join("oauth/token.json"), "{}").unwrap();

        let child = create_profile("child", Some("base")).unwrap();
        let marker = std::fs::read_to_string(child.join("profile.toml")).unwrap();
        assert_eq!(marker, "base = \"base\"\n");
        // SECURITY: secrets are never inherited at create time.
        assert!(!child.join("credentials.toml").exists());
        assert!(!child.join("oauth").exists());
        let _ = root;
    }

    #[test]
    #[serial]
    fn create_with_missing_base_errors_and_leaves_nothing() {
        let (_g, root) = rooted();
        assert!(matches!(
            create_profile("child", Some("ghost")),
            Err(ProfileOpError::NotFound(_))
        ));
        // The child dir must not have been created.
        assert!(!root.path().join("child").exists());
    }

    #[test]
    #[serial]
    fn list_skips_pointer_and_nondirs() {
        let (_g, root) = rooted();
        create_profile("work", None).unwrap();
        // The pointer file and a stray loose file must be skipped.
        std::fs::write(root.path().join("active"), "work\n").unwrap();
        std::fs::write(root.path().join("README"), "hi").unwrap();
        // A junk dir with an invalid name must be skipped too.
        std::fs::create_dir_all(root.path().join("..junk")).ok();
        assert_eq!(list_profiles(), vec!["work"]);
    }

    #[test]
    #[serial]
    fn set_clear_and_active_name_roundtrip() {
        let (_g, _root) = rooted();
        create_profile("work", None).unwrap();
        assert_eq!(active_profile_name(), None);
        assert!(!is_active("work"));

        set_active_profile("Work").unwrap(); // case-folds to "work"
        assert_eq!(active_profile_name(), Some("work".to_string()));
        assert!(is_active("work"));
        assert!(is_active("WORK"));

        clear_active_profile().unwrap();
        assert_eq!(active_profile_name(), None);
        // Clearing an already-absent pointer is idempotent.
        clear_active_profile().unwrap();
    }

    #[test]
    #[serial]
    fn set_active_refuses_missing_profile() {
        let (_g, _root) = rooted();
        assert!(matches!(
            set_active_profile("ghost"),
            Err(ProfileOpError::NotFound(_))
        ));
        assert!(matches!(
            set_active_profile("../escape"),
            Err(ProfileOpError::Name(_))
        ));
    }

    #[test]
    #[serial]
    fn rename_moves_dir_and_repoints_active() {
        let (_g, root) = rooted();
        create_profile("old", None).unwrap();
        std::fs::write(root.path().join("old/marker"), "data").unwrap();
        set_active_profile("old").unwrap();

        rename_profile("old", "new").unwrap();
        assert!(!root.path().join("old").exists());
        assert!(root.path().join("new/marker").exists());
        // Active selection follows the rename.
        assert_eq!(active_profile_name(), Some("new".to_string()));
    }

    #[test]
    #[serial]
    fn rename_errors_on_missing_and_conflict() {
        let (_g, _root) = rooted();
        create_profile("a", None).unwrap();
        create_profile("b", None).unwrap();
        assert!(matches!(
            rename_profile("ghost", "z"),
            Err(ProfileOpError::NotFound(_))
        ));
        assert!(matches!(
            rename_profile("a", "b"),
            Err(ProfileOpError::AlreadyExists(_))
        ));
        // Case-only rename is a no-op success and leaves the dir intact.
        rename_profile("a", "A").unwrap();
        assert!(profile_exists("a"));
    }

    #[test]
    #[serial]
    fn delete_dir_does_not_touch_pointer() {
        let (_g, root) = rooted();
        create_profile("work", None).unwrap();
        set_active_profile("work").unwrap();
        // Low-level delete leaves the (now dangling) pointer for the CLI to handle.
        delete_profile_dir("work").unwrap();
        assert!(!root.path().join("work").exists());
        assert_eq!(active_profile_name(), Some("work".to_string()));
        assert!(matches!(
            delete_profile_dir("work"),
            Err(ProfileOpError::NotFound(_))
        ));
    }

    #[test]
    #[serial]
    fn delete_profile_clears_pointer_when_active() {
        let (_g, _root) = rooted();
        create_profile("work", None).unwrap();
        create_profile("other", None).unwrap();
        set_active_profile("work").unwrap();

        delete_profile("work").unwrap();
        assert!(!profile_exists("work"));
        assert_eq!(active_profile_name(), None, "active pointer cleared");

        // Deleting a non-active profile leaves an unrelated pointer alone.
        set_active_profile("other").unwrap();
        create_profile("temp", None).unwrap();
        delete_profile("temp").unwrap();
        assert_eq!(active_profile_name(), Some("other".to_string()));
    }

    #[test]
    #[serial]
    fn export_excludes_secrets_by_default() {
        let (_g, root) = rooted();
        let p = create_profile("work", None).unwrap();
        std::fs::write(p.join("config.toml"), "model='x'").unwrap();
        std::fs::write(p.join("credentials.toml"), "secret").unwrap();
        std::fs::write(p.join("credentials.enc"), "secret").unwrap();
        std::fs::create_dir_all(p.join("oauth")).unwrap();
        std::fs::write(p.join("oauth/t.json"), "{}").unwrap();
        std::fs::create_dir_all(p.join("memory")).unwrap();
        std::fs::write(p.join("memory/notes.md"), "keep").unwrap();

        let out = tempdir().unwrap();
        let dst = out.path().join("export");
        export_profile("work", &dst, false).unwrap();

        assert!(dst.join("config.toml").exists());
        assert!(
            dst.join("memory/notes.md").exists(),
            "non-secret subtree copied"
        );
        assert!(!dst.join("credentials.toml").exists(), "secret excluded");
        assert!(!dst.join("credentials.enc").exists(), "secret excluded");
        assert!(!dst.join("oauth").exists(), "oauth excluded");
        let _ = root;
    }

    #[test]
    #[serial]
    fn export_include_secrets_copies_them() {
        let (_g, _root) = rooted();
        let p = create_profile("work", None).unwrap();
        std::fs::write(p.join("credentials.toml"), "secret").unwrap();
        let out = tempdir().unwrap();
        let dst = out.path().join("export");
        export_profile("work", &dst, true).unwrap();
        assert!(dst.join("credentials.toml").exists());
    }

    #[test]
    #[serial]
    fn import_creates_profile_from_tree() {
        let (_g, _root) = rooted();
        let src = tempdir().unwrap();
        std::fs::write(src.path().join("config.toml"), "model='y'").unwrap();
        std::fs::create_dir_all(src.path().join("skills")).unwrap();
        std::fs::write(src.path().join("skills/s.md"), "skill").unwrap();

        let dir = import_profile("imported", src.path()).unwrap();
        assert!(dir.join("config.toml").exists());
        assert!(dir.join("skills/s.md").exists());
        assert!(profile_exists("imported"));
    }

    #[test]
    #[serial]
    fn import_rejects_existing_and_bad_source() {
        let (_g, _root) = rooted();
        create_profile("taken", None).unwrap();
        let src = tempdir().unwrap();
        assert!(matches!(
            import_profile("taken", src.path()),
            Err(ProfileOpError::AlreadyExists(_))
        ));
        // Non-existent source path is rejected as unsafe (cannot canonicalize).
        let missing = src.path().join("does-not-exist");
        assert!(matches!(
            import_profile("fresh", &missing),
            Err(ProfileOpError::UnsafePath { .. })
        ));
    }

    #[test]
    #[serial]
    fn import_does_not_follow_symlinks() {
        let (_g, _root) = rooted();
        let src = tempdir().unwrap();
        std::fs::write(src.path().join("real.txt"), "ok").unwrap();
        // A symlink pointing outside the source must NOT be followed (C6).
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "leak").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), src.path().join("escape")).unwrap();

        let dir = import_profile("safe", src.path()).unwrap();
        assert!(dir.join("real.txt").exists());
        // The symlink (and thus the outside file) was skipped, never recreated.
        assert!(!dir.join("escape").exists());
    }

    #[test]
    fn is_secret_entry_classification() {
        assert!(is_secret_entry("credentials.toml"));
        assert!(is_secret_entry("credentials.enc"));
        assert!(is_secret_entry("credentials.kdf.json"));
        assert!(is_secret_entry("credentials"));
        assert!(is_secret_entry("oauth"));
        // Not secrets:
        assert!(!is_secret_entry("config.toml"));
        assert!(!is_secret_entry("credentials-notes"));
        assert!(!is_secret_entry("oauth-helper"));
        assert!(!is_secret_entry("memory"));
    }
}
