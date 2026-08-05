//! Whether an archive's payload paths can actually be materialized on the
//! platform doing the restore (F26-03-D).
//!
//! # Why this is separate from the long-path fix
//!
//! `wcore_config::atomic_io` now resolves a long destination to its
//! extended-length (`\\?\`) form, which lifts Windows' 260-character
//! `MAX_PATH` at the Win32 layer. That fixes the measured defect — an archive
//! created on Windows that its own machine could not restore.
//!
//! It does NOT fix a second, wider class, and the extended-length prefix makes
//! part of that class *stricter* rather than looser, because a verbatim path
//! receives no Win32 normalization at all:
//!
//! * a component that is a reserved DOS device name (`CON`, `NUL`, `COM1`…);
//! * a component containing a character Windows forbids (`< > : " | ? *`);
//! * a component ending in a dot or a space;
//! * a component longer than 255 bytes, which no common filesystem accepts.
//!
//! Every one of those is a legal filename on Linux and macOS. Archives are
//! portable by design — created on one platform, restored on another — so a
//! Linux home containing `aux.txt` or `report:final.md` produces an archive
//! that is perfectly restorable on Linux and cannot be restored on Windows.
//!
//! # Where each check fires, and why the two differ
//!
//! * **Restore refuses.** The target root is known, so every reconstructed
//!   destination is known exactly, and a refusal is a statement of fact. It
//!   happens before the first byte is written, so a refusal costs nothing.
//! * **Create only warns.** The creating machine does not know which platform
//!   will restore, nor into what directory. Refusing a valid Linux-to-Linux
//!   archive because it *might* one day meet Windows would break correct use,
//!   so create names the payloads that will not survive a Windows restore and
//!   proceeds. That is the earliest point the fact can be known, and it is
//!   knowable only for the root-independent half.

use std::path::Path;

/// Longest single path component any common filesystem accepts.
const MAX_COMPONENT_BYTES: usize = 255;

/// The practical ceiling on a full path.
///
/// On Windows an extended-length path reaches ~32767 UTF-16 units; on unix
/// `PATH_MAX` is 4096. These are the limits that remain AFTER the
/// extended-length fix, so a refusal here means genuinely impossible rather
/// than merely long.
#[cfg(windows)]
const MAX_TOTAL_PATH: usize = 32_000;
#[cfg(not(windows))]
const MAX_TOTAL_PATH: usize = 4_096;

/// Reserved DOS device names. Reserved with any extension and in any case, so
/// `aux`, `AUX`, `Aux.txt` are all unusable as a component on Windows.
const DOS_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Why a payload path cannot be materialized. Carries the offending payload and
/// the specific component, because "some path was too long" is not actionable
/// and an operator needs to know which file to rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathObjection {
    pub payload: String,
    pub reason: String,
}

impl std::fmt::Display for PathObjection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.payload, self.reason)
    }
}

/// Root-independent objections: true of the payload path itself, whatever
/// directory it is restored into. Safe to evaluate at create time.
///
/// `for_windows` selects whether the Windows-only naming rules apply, so the
/// same function serves the create-time warning (which asks "would Windows
/// take this?") and the restore-time refusal on a Windows host.
pub fn intrinsic_objections(payload: &str, for_windows: bool) -> Vec<PathObjection> {
    let mut out = Vec::new();
    for component in payload.split('/').filter(|c| !c.is_empty()) {
        if component.len() > MAX_COMPONENT_BYTES {
            out.push(PathObjection {
                payload: payload.to_string(),
                reason: format!(
                    "path component is {} bytes, past the {MAX_COMPONENT_BYTES}-byte filesystem limit",
                    component.len()
                ),
            });
        }
        if !for_windows {
            continue;
        }
        let stem = component.split('.').next().unwrap_or(component);
        if DOS_DEVICE_NAMES
            .iter()
            .any(|d| stem.eq_ignore_ascii_case(d))
        {
            out.push(PathObjection {
                payload: payload.to_string(),
                reason: format!(
                    "component '{component}' is a reserved Windows device name and cannot exist as a file there"
                ),
            });
        }
        if let Some(bad) = component
            .chars()
            .find(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (*c as u32) < 0x20)
        {
            out.push(PathObjection {
                payload: payload.to_string(),
                reason: format!(
                    "component '{component}' contains {bad:?}, which Windows forbids in a filename"
                ),
            });
        }
        if component.ends_with('.') || component.ends_with(' ') {
            out.push(PathObjection {
                payload: payload.to_string(),
                reason: format!(
                    "component '{component}' ends in a dot or space, which Windows cannot represent"
                ),
            });
        }
    }
    out
}

/// Every reason the given payloads could not be written under `target` on THIS
/// platform. Root-dependent, so this is the restore-time check and it is exact.
pub fn objections_for_target(target: &Path, payloads: &[String]) -> Vec<PathObjection> {
    let mut out = Vec::new();
    let root_len = target.as_os_str().len();
    for payload in payloads {
        out.extend(intrinsic_objections(payload, cfg!(windows)));
        // +1 for the separator joining root and relative path.
        let total = root_len + 1 + payload.len();
        if total > MAX_TOTAL_PATH {
            out.push(PathObjection {
                payload: payload.clone(),
                reason: format!(
                    "restored path would be {total} bytes, past this platform's {MAX_TOTAL_PATH}-byte limit"
                ),
            });
        }
    }
    out
}

/// Render objections into one operator-facing message. Names the count and
/// every offending payload — a refusal an operator cannot act on is only
/// marginally better than the silent corruption it replaced.
pub fn render(objections: &[PathObjection], target: &Path) -> String {
    let mut s = format!(
        "refusing to restore into {}: {} archived path(s) cannot be written on this platform.\n",
        target.display(),
        objections.len()
    );
    for o in objections {
        s.push_str(&format!("  - {o}\n"));
    }
    s.push_str(
        "These paths are legal on the platform that produced the archive and are not legal here. \
         Restore on the originating platform, or rename the offending entries at the source and \
         take a fresh backup. Nothing has been written.",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_payload_path_raises_no_objection() {
        // The control. Without this, a check that objected to EVERYTHING would
        // look identical to a correct one in the tests below.
        assert!(intrinsic_objections("skills/demo/SKILL.md", true).is_empty());
        assert!(intrinsic_objections("config.toml", true).is_empty());
        assert!(intrinsic_objections("memory/notes.2026.md", true).is_empty());
    }

    #[test]
    fn windows_reserved_device_names_are_named_as_unrestorable() {
        // Every one of these is a perfectly ordinary Linux filename, which is
        // exactly why a portable archive can carry them.
        for payload in ["aux.txt", "skills/CON/SKILL.md", "nul", "logs/COM1.log"] {
            let objs = intrinsic_objections(payload, true);
            assert_eq!(objs.len(), 1, "{payload} -> {objs:?}");
            assert!(objs[0].reason.contains("reserved Windows device name"));
            // ...and the same path is fine where it came from.
            assert!(
                intrinsic_objections(payload, false).is_empty(),
                "{payload} must be acceptable off Windows"
            );
        }
    }

    #[test]
    fn characters_windows_forbids_are_named_with_the_offending_character() {
        let objs = intrinsic_objections("reports/report:final.md", true);
        assert_eq!(objs.len(), 1, "{objs:?}");
        assert!(objs[0].reason.contains("forbids"));
        assert!(objs[0].reason.contains(':'));
        assert!(intrinsic_objections("reports/report:final.md", false).is_empty());
    }

    #[test]
    fn a_trailing_dot_or_space_is_named() {
        assert_eq!(intrinsic_objections("skills/demo./SKILL.md", true).len(), 1);
        assert_eq!(intrinsic_objections("skills/demo /SKILL.md", true).len(), 1);
        assert!(intrinsic_objections("skills/demo./SKILL.md", false).is_empty());
    }

    #[test]
    fn an_overlong_component_is_rejected_on_every_platform() {
        let long = "x".repeat(300);
        let payload = format!("skills/{long}/SKILL.md");
        // This one is NOT Windows-specific: no common filesystem accepts it.
        assert_eq!(intrinsic_objections(&payload, false).len(), 1);
        assert_eq!(intrinsic_objections(&payload, true).len(), 1);
    }

    #[test]
    fn deep_but_legal_paths_raise_no_objection_after_the_long_path_fix() {
        // The regression guard for the fix itself: a path far past MAX_PATH but
        // made of legal components must NOT be refused. Refusing it would
        // "solve" F26-03-D by declining to do the work, which is precisely the
        // outcome the finding calls worse than the defect.
        let deep: String = (0..12)
            .map(|i| format!("deeply-nested-directory-segment-{i}/"))
            .collect::<String>()
            + "deep-canary.md";
        assert!(deep.len() > 260, "fixture too shallow: {}", deep.len());
        assert!(
            intrinsic_objections(&deep, true).is_empty(),
            "a long-but-legal path must not be refused"
        );
        assert!(
            objections_for_target(Path::new("/tmp/target"), &[deep]).is_empty(),
            "a long-but-legal path must not be refused at restore either"
        );
    }

    #[test]
    fn the_rendered_refusal_names_every_offending_payload() {
        let objs = objections_for_target(
            Path::new("/tmp/t"),
            &["aux.txt".to_string(), "b/report:x.md".to_string()],
        );
        let msg = render(&objs, Path::new("/tmp/t"));
        if cfg!(windows) {
            assert_eq!(objs.len(), 2, "{objs:?}");
            assert!(msg.contains("aux.txt"));
            assert!(msg.contains("report:x.md"));
            assert!(msg.contains("Nothing has been written"));
        } else {
            // Off Windows these paths are legal, so the restore must proceed.
            assert!(objs.is_empty(), "{objs:?}");
        }
    }
}
