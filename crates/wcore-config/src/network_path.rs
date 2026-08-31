//! The one implementation of "does this path *name* a Windows network share",
//! and the signpost to the different question people keep asking it.
//!
//! # The two questions, which are not the same question
//!
//! 1. **Is this path spelled as a UNC share?** — `\\server\share`,
//!    `\\?\UNC\server\share`. Pure syntax. It is what you want before *opening*
//!    an attacker-supplied path, because opening a UNC name makes Windows dial
//!    out to a host of the attacker's choosing and leak a NetNTLM hash on the
//!    way. That hazard is created by the spelling, so a syntactic answer is the
//!    correct and complete answer. This module answers it.
//!
//! 2. **Is this path stored on a network filesystem?** — an NFS-mounted `$HOME`
//!    at `/home/alice`, an SMB share mapped to `Z:\`. Nothing about those
//!    strings says "network"; only asking the kernel does. That question is
//!    answered by [`crate::sqlite_journal::classify_path`], re-exported below.
//!
//! # Why this module exists
//!
//! Question 1 had **five** implementations in this workspace, disagreeing with
//! each other, and two of them were named `is_network_path` — which reads like
//! an answer to question 2 and is not one. `wcore-config`'s copy returned
//! `false` unconditionally off Windows, so on Linux and macOS it answered "not
//! a network path" for every path in existence, including every NFS mount.
//! Prior art that looks like protection and is not is worse than no prior art,
//! because the next reader stops looking.
//!
//! The disagreements were real, not cosmetic:
//!
//! | input | `executable_readiness` | `vision_tools` | `media_intake` | here |
//! |---|---|---|---|---|
//! | `\\?\C:\Users\x` (verbatim **local** disk) | network | not network | network | not network |
//! | `\\.\pipe\x` (**device** namespace) | network | not network | network | not network |
//! | `//server/share` on Unix | not network | not network | network | network |
//!
//! Calling a local disk a network share is not harmless: it is how a correct
//! path gets refused with a misleading reason.
//!
//! # What this deliberately does NOT do
//!
//! It does not consult the filesystem, so it cannot see a mapped drive: `Z:\`
//! backed by `\\nas\home` is indistinguishable from a local `Z:\` by
//! inspection. Callers who need that must use [`classify_path`], which asks
//! `GetDriveTypeW`. Callers guarding against the SMB-dial-out hazard do **not**
//! need it — a drive letter is already-established, admin-configured access, so
//! refusing it would break legitimate use for no security gain.

use std::path::Path;

/// The kernel-backed answer to "is this path *stored on* a network
/// filesystem", as opposed to the syntactic question this module answers.
/// Re-exported so there is exactly one place to look for either.
pub use crate::sqlite_journal::{FsClass, classify_path, is_network_backed};

/// Normalise separators so one matcher covers both spellings. Windows accepts
/// `/` and `\` interchangeably in UNC names, and a check that only looks for
/// backslashes is bypassed by the forward-slash form.
fn normalised_lower(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c == '/' { '\\' } else { c })
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Does `path` name a UNC network share — `\\server\share` or
/// `\\?\UNC\server\share`?
///
/// Deliberately excludes the verbatim-disk (`\\?\C:\…`) and device (`\\.\…`)
/// namespaces: they start with the same two separators but are local. Use
/// [`has_device_or_verbatim_prefix`] for those.
///
/// Answers the same on every platform. The check must hold on Linux and macOS
/// too, both because a UNC string can reach a Windows host through config or a
/// synced file and because a guard that silently evaporates off Windows cannot
/// be tested there — which is precisely how the previous copy stayed broken.
pub fn has_unc_prefix(path: &Path) -> bool {
    // Authoritative on Windows: the parsed prefix already understands every
    // spelling the OS accepts.
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if let Some(Component::Prefix(p)) = path.components().next()
            && matches!(p.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
        {
            return true;
        }
    }

    let lower = normalised_lower(path);
    if lower.starts_with(r"\\?\unc\") {
        return true;
    }
    match lower.strip_prefix(r"\\") {
        // Two separators then a host character. `?` and `.` introduce the
        // verbatim and device namespaces; a bare `\\` names nothing.
        Some(rest) => !matches!(rest.chars().next(), Some('?') | Some('.') | None),
        None => false,
    }
}

/// Does `path` use the Windows device (`\\.\`) or verbatim (`\\?\`) namespace?
///
/// Verbatim paths bypass Win32 path normalisation, so a confinement check that
/// reasoned about the normalised form can be walked straight past; device paths
/// are raw handles rather than files. Neither is a network path — `\\?\UNC\…`
/// is UNC and is reported by [`has_unc_prefix`], not here.
pub fn has_device_or_verbatim_prefix(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if let Some(Component::Prefix(p)) = path.components().next()
            && matches!(
                p.kind(),
                Prefix::VerbatimDisk(..) | Prefix::DeviceNS(..) | Prefix::Verbatim(..)
            )
        {
            return true;
        }
    }

    let lower = normalised_lower(path);
    if lower.starts_with(r"\\?\unc\") {
        return false;
    }
    lower.starts_with(r"\\?\") || lower.starts_with(r"\\.\")
}

/// Does `path` name a Windows **verbatim DISK** path — `\\?\C:\…`?
///
/// The one member of the `\\?\` family that names an ordinary local file, and
/// the reason [`has_device_or_verbatim_prefix`] is too coarse to refuse on by
/// itself. `std::fs::canonicalize` RETURNS this form on Windows, so it is the
/// spelling this workspace's own canonical paths carry — refusing it refuses
/// the product's own output (FerroxLabs/wayland-core#409 c2).
///
/// Everything else under `\\?\` stays outside this predicate:
/// `\\?\GLOBALROOT\Device\…` and `\\?\Volume{…}` reach raw devices and
/// volumes, `\\.\…` is the device namespace, and `\\?\UNC\…` is UNC (see
/// [`has_unc_prefix`]). Answers the same on every platform, for the reason
/// given on [`has_unc_prefix`].
pub fn has_verbatim_disk_prefix(path: &Path) -> bool {
    // Authoritative on Windows, exactly as in the two predicates above.
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if let Some(Component::Prefix(p)) = path.components().next()
            && matches!(p.kind(), Prefix::VerbatimDisk(..))
        {
            return true;
        }
    }

    let lower = normalised_lower(path);
    let Some(rest) = lower.strip_prefix(r"\\?\") else {
        return false;
    };
    // A single drive letter, a colon, then a separator or the end of the path.
    // `\\?\GLOBALROOT\…` fails on the first character, `\\?\UNC\…` on the
    // second, and `\\?\Volume{…}` on both.
    let mut chars = rest.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.next() == Some(':')
        && matches!(chars.next(), Some('\\') | None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    // ---- The three implementations this module replaces, kept executable ---
    //
    // Transcribed from the pre-consolidation sources so the divergence is a
    // measurement a future reader can re-run, not a claim in a comment. If one
    // of these is ever revived, these tests say what it would get wrong.

    /// `wcore-config/src/shell/executable_readiness.rs:540`, Windows arm.
    /// Off-Windows it returned `false` before reaching this line at all.
    fn legacy_executable_readiness(path: &Path) -> bool {
        path.to_str()
            .is_some_and(|p| p.starts_with(r"\\") || p.starts_with("//"))
    }

    /// `wcore-tools/src/media_intake.rs:185`.
    fn legacy_media_intake(path: &Path) -> bool {
        let s = path.to_string_lossy();
        s.starts_with("\\\\") || s.starts_with("//")
    }

    /// `wcore-tools/src/vision_tools.rs:167`. `Component::Prefix` never
    /// matches on a Unix target, so off-Windows this is a constant `false`.
    fn legacy_vision_tools(path: &Path) -> bool {
        use std::path::{Component, Prefix};
        matches!(
            path.components().next(),
            Some(Component::Prefix(pre)) if matches!(pre.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
        )
    }

    #[test]
    fn unc_forms_are_recognised() {
        assert!(has_unc_prefix(&p(r"\\server\share\file.png")));
        assert!(has_unc_prefix(&p(r"\\?\UNC\server\share\file.png")));
        assert!(has_unc_prefix(&p(r"\\?\unc\server\share")));
        // Forward-slash spelling: Windows accepts it, so the guard must too.
        assert!(has_unc_prefix(&p("//server/share/file.png")));
        assert!(has_unc_prefix(&p("//?/UNC/server/share")));
    }

    #[test]
    fn local_and_device_forms_are_not_unc() {
        assert!(!has_unc_prefix(&p(r"C:\Users\x\file.png")));
        assert!(!has_unc_prefix(&p("/home/alice/file.png")));
        assert!(!has_unc_prefix(&p("relative/file.png")));
        // A verbatim path to a LOCAL disk is not a network share.
        assert!(!has_unc_prefix(&p(r"\\?\C:\Users\x")));
        // The device namespace is not a network share either.
        assert!(!has_unc_prefix(&p(r"\\.\pipe\wayland")));
        assert!(!has_unc_prefix(&p(r"\\")));
    }

    #[test]
    fn device_and_verbatim_are_separated_from_unc() {
        assert!(has_device_or_verbatim_prefix(&p(r"\\?\C:\Users\x")));
        assert!(has_device_or_verbatim_prefix(&p(r"\\.\PhysicalDrive0")));
        assert!(has_device_or_verbatim_prefix(&p("//./PhysicalDrive0")));
        // `\\?\UNC\...` is UNC and must be reported by exactly one of the two.
        assert!(!has_device_or_verbatim_prefix(&p(r"\\?\UNC\server\share")));
        assert!(has_unc_prefix(&p(r"\\?\UNC\server\share")));
        // Ordinary paths are neither.
        assert!(!has_device_or_verbatim_prefix(&p(r"C:\Users\x")));
        assert!(!has_device_or_verbatim_prefix(&p("/home/alice")));
    }

    /// The three-assertion self-test (LANE-BRIEF §6b-ii): known-positive
    /// passes, known-negative fails, **and the implementations being replaced
    /// would have got it wrong**. Without the third assertion this file would
    /// pass unchanged against the code it is supposed to be fixing.
    #[test]
    fn consolidation_changes_answers_the_old_copies_got_wrong() {
        // (1) KNOWN-POSITIVE — a real UNC share is still refused, and every
        //     old copy agreed. So this case alone proves nothing.
        let unc = p(r"\\evil-host\share\payload.png");
        assert!(has_unc_prefix(&unc));
        assert!(legacy_executable_readiness(&unc));
        assert!(legacy_media_intake(&unc));

        // (2) KNOWN-NEGATIVE — an ordinary local path is not a UNC share, and
        //     every old copy agreed there too.
        let local = p(r"C:\Users\alice\photo.png");
        assert!(!has_unc_prefix(&local));
        assert!(!legacy_executable_readiness(&local));
        assert!(!legacy_media_intake(&local));

        // (3) THE OLD COPIES WERE WRONG HERE, IN BOTH DIRECTIONS.
        //
        //     Over-blocking: a verbatim path to a LOCAL disk. Two of the three
        //     called it a network path, which is how a correct local path got
        //     refused with a reason that named the wrong hazard.
        let verbatim_local = p(r"\\?\C:\Users\alice\photo.png");
        assert!(
            !has_unc_prefix(&verbatim_local),
            "a verbatim path to a local disk is not a network share"
        );
        assert!(
            legacy_executable_readiness(&verbatim_local),
            "executable_readiness called a local disk a network path — this \
             difference is the repair"
        );
        assert!(
            legacy_media_intake(&verbatim_local),
            "media_intake called a local disk a network path — this difference \
             is the repair"
        );

        //     Under-blocking: on a Unix build `vision_tools` could not see a
        //     UNC string at all, because `Component::Prefix` never matches
        //     there. The guard evaporated on the platform its tests ran on.
        #[cfg(not(windows))]
        {
            assert!(
                !legacy_vision_tools(&unc),
                "on Unix the Component::Prefix check returns false for a UNC \
                 string, so the guard was untestable on the platform CI used"
            );
            assert!(
                has_unc_prefix(&unc),
                "the consolidated check answers the same on every platform"
            );
        }
        #[cfg(windows)]
        let _ = legacy_vision_tools(&unc);
    }

    /// core#409 c2 — the `\\?\` family splits in two and only one half is
    /// refusable. Pins BOTH directions: the ordinary half must be recognised
    /// as ordinary, and the device half must stay caught.
    #[test]
    fn verbatim_disk_is_distinguished_from_the_device_namespace() {
        // ORDINARY. `std::fs::canonicalize` returns exactly this shape on
        // Windows, so a guard that refuses it refuses the product's own paths.
        for ordinary in [
            r"\\?\C:\Users\me\notes.txt",
            r"\\?\c:\x",
            r"\\?\F:\ws\.wayland-out",
            r"\\?\C:",
            "//?/C:/x",
        ] {
            assert!(
                has_verbatim_disk_prefix(&p(ordinary)),
                "{ordinary:?} names an ordinary local file"
            );
            assert!(!has_unc_prefix(&p(ordinary)));
        }

        // DEVICE / NON-DISK VERBATIM. None of these is a disk path, and the
        // coarse predicate must keep claiming every one that is not UNC — this
        // is the half callers still refuse.
        for device in [
            r"\\.\PhysicalDrive0",
            r"\\.\pipe\wayland",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\secret",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x",
            r"\\?\CC:\x",
        ] {
            assert!(
                !has_verbatim_disk_prefix(&p(device)),
                "{device:?} is not an ordinary local disk path"
            );
            assert!(
                has_device_or_verbatim_prefix(&p(device)),
                "{device:?} must stay claimed by the coarse predicate"
            );
        }

        // `\\?\UNC\…` belongs to neither: it is UNC and nothing else.
        let unc = p(r"\\?\UNC\server\share");
        assert!(!has_verbatim_disk_prefix(&unc));
        assert!(!has_device_or_verbatim_prefix(&unc));
        assert!(has_unc_prefix(&unc));

        // Ordinary non-namespace paths are not verbatim disks either.
        assert!(!has_verbatim_disk_prefix(&p(r"C:\Users\me")));
        assert!(!has_verbatim_disk_prefix(&p("/home/alice")));
    }

    #[test]
    fn syntax_check_and_filesystem_check_answer_different_questions() {
        // The heart of the naming defect: a path on a *real* network
        // filesystem is not spelled differently, so no amount of string
        // inspection can find it. This is why the old `is_network_path` name
        // was actively misleading, and why the two functions must stay
        // distinguishable at the call site.
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            !has_unc_prefix(tmp.path()),
            "a local temp dir is not spelled as a UNC share"
        );
        // And the kernel-backed check is the one that would notice if this
        // directory were network-backed. On CI it is not.
        assert_eq!(classify_path(tmp.path()), FsClass::Local);
        assert!(!is_network_backed(tmp.path()));
    }
}
