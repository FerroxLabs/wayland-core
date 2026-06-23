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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

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
            "a/../b", ".hidden", ".", "..foo",
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

    #[test]
    #[serial]
    fn profiles_root_honors_explicit_override() {
        let _g = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some("/tmp/custom-profiles"))]);
        assert_eq!(profiles_root(), PathBuf::from("/tmp/custom-profiles"));

        // A control-char-bearing override is ignored (falls through to default).
        let _g2 = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some("/tmp/bad\nroot"))]);
        assert_ne!(profiles_root(), PathBuf::from("/tmp/bad\nroot"));

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
        let _g = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some("/tmp/p"))]);
        let upper = profile_dir("Work").unwrap();
        let lower = profile_dir("work").unwrap();
        assert_eq!(upper, lower, "Work and work must map to the same directory");
        assert_eq!(lower, PathBuf::from("/tmp/p/work"));
    }

    #[test]
    #[serial]
    fn profile_dir_rejects_invalid_name() {
        let _g = EnvGuard::set(&[("WAYLAND_PROFILES_ROOT", Some("/tmp/p"))]);
        assert!(profile_dir("../escape").is_err());
        assert!(profile_dir("a/b").is_err());
    }

    #[test]
    #[serial]
    fn active_pointer_is_under_root_not_in_a_home() {
        let _g = EnvGuard::set(&[
            ("WAYLAND_PROFILES_ROOT", Some("/tmp/p")),
            ("WAYLAND_HOME", Some("/tmp/some-home")),
        ]);
        let ptr = active_pointer_path();
        assert_eq!(ptr, PathBuf::from("/tmp/p/active"));
        assert!(!ptr.starts_with("/tmp/some-home"));
    }
}
