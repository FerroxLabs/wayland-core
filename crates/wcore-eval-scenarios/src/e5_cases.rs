//! The nine F28-01 dimensions as BLACK-BOX probes against the shipped binary.
//!
//! # Why the definitions live here and the execution does not
//!
//! Black-box is not a preference in Phase 28, it is what makes the macOS leg
//! reachable at all. The certification Mac may run the CI-produced arm64
//! `wayland-core` binary; it may **not** run cargo beyond `cargo fmt --all -- --check`.
//! A probe implemented as a cargo-built test harness therefore cannot run there, and a
//! dimension whose only coverage is such a probe silently loses a whole OS family.
//!
//! So this module is the **canonical definition** of every probe — dimension, families,
//! the exact invocation, the observable that separates pass from fail, and the
//! condition under which the probe goes red — and `scripts/f28-native-matrix.mjs` is
//! the **executor and verifier**, which needs only a Node runtime and the shipped
//! binary. `tests/e5_native_matrix.rs` asserts the two agree entry for entry, so the
//! executor cannot drift from the definition it claims to implement.
//!
//! That split is asserted, not assumed: [`Harness::BlackBox`] means the probe runs
//! without a cargo-built harness on the platform it runs on, and
//! `black_box_probes_require_no_cargo_harness` proves the claim against the executor.
//!
//! # The wrong-OS anti-drift guard
//!
//! A cell mapped to something gated for a different OS passes or skips without ever
//! proving the platform property, and that defect survived undetected on this program
//! until a guard was written over a canonical map. Here every probe declares the
//! [`SurfaceBinding`] it actually exercises, and its family list is DERIVED from that
//! binding rather than typed beside it — a probe cannot be listed on a family where
//! the surface it claims to exercise does not exist.
//!
//! # Sandbox activeness
//!
//! A sandbox-dimension probe emits its activeness observation as its own field. The
//! verifier rejects a passing sandbox cell whose activeness field is absent, and
//! [`crate::e5_matrix::ActivenessEvidence`] has no variant expressing "no violation
//! observed". Absence of an observed violation is what a silently disabled sandbox
//! produces, so it must not be expressible as a green.
//!
//! # Every probe must be able to go red
//!
//! A hostile-input or Unicode probe authored so permissively that it cannot fail adds
//! a green cell that proves nothing. Each spec therefore names a
//! [`ProbeSpec::failing_counterpart`] — a concrete mutation of the product or the
//! fixture under which that probe MUST report red — and the executor's self-test
//! drives each one. A probe with no failing counterpart is rejected at test time.

use crate::Platform;
use crate::e5_matrix::Dimension;

// ---------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------

/// Whether a probe needs a cargo-built harness on the platform it runs on.
///
/// `HarnessBound` is legal, but it is a declaration with a cost: a harness-bound probe
/// that is the ONLY coverage of a CRITICAL cell on a family where no harness can be
/// built fails the suite rather than narrowing coverage silently. That failure is the
/// honest outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    /// Runs against the shipped binary with no compiler on the host.
    BlackBox,
    /// Requires a cargo-built harness, with the reason stated.
    HarnessBound { reason: &'static str },
}

impl Harness {
    pub const fn is_black_box(self) -> bool {
        matches!(self, Self::BlackBox)
    }
}

// ---------------------------------------------------------------------------------------
// Surface binding — the wrong-OS anti-drift guard
// ---------------------------------------------------------------------------------------

/// The platform surface a probe actually exercises.
///
/// A probe's family list is derived from this rather than declared next to it, so a
/// cell cannot be mapped to something gated for a different OS and then pass or skip
/// without ever proving the platform property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceBinding {
    /// Exists on every family: process spawn, argv handling, stdio, exit status.
    UniversalProcess,
    /// The OS sandbox backend: bubblewrap, sandbox-exec, AppContainer.
    OsSandboxBackend,
    /// Reparse points and symbolic links. Present on all three families, by different
    /// mechanisms — NTFS junctions and symlinks on Windows, POSIX symlinks elsewhere.
    ReparseAndSymlink,
    /// UNC paths and the `\\?\` verbatim prefix. Windows only, by construction.
    UncVerbatimPath,
    /// Process suspend and resume: `SIGSTOP`/`SIGCONT` on Unix, thread suspension on
    /// Windows.
    ProcessSuspension,
}

impl SurfaceBinding {
    /// Whether the bound surface exists on a family. This is the anti-drift predicate.
    pub const fn applies_on(self, os: Platform) -> bool {
        match self {
            Self::UniversalProcess
            | Self::OsSandboxBackend
            | Self::ReparseAndSymlink
            | Self::ProcessSuspension => true,
            Self::UncVerbatimPath => matches!(os, Platform::Windows),
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::UniversalProcess => "universal-process",
            Self::OsSandboxBackend => "os-sandbox-backend",
            Self::ReparseAndSymlink => "reparse-and-symlink",
            Self::UncVerbatimPath => "unc-verbatim-path",
            Self::ProcessSuspension => "process-suspension",
        }
    }
}

// ---------------------------------------------------------------------------------------
// ProbeSpec
// ---------------------------------------------------------------------------------------

/// One probe.
///
/// A live run is specified by three things and is not accepted without all three: the
/// EXACT invocation, an OBSERVABLE OUTCOME that distinguishes pass from fail, and the
/// NAMED PLATFORM. `invocation` and `observable` carry the first two; `families`
/// carries the third. "Verify it works" is not a verification step and cannot be
/// expressed in this struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeSpec {
    /// Stable wire id, emitted in every marker.
    pub id: &'static str,
    pub dimension: Dimension,
    /// Derived from `binding`; asserted, never trusted. See
    /// `families_are_derived_from_the_surface_binding`.
    pub families: &'static [Platform],
    /// `Some` for a probe that covers exactly one named cell (the three mandatory
    /// cells), `None` for a dimension probe that covers every cell of its dimension on
    /// its families.
    pub cell_id: Option<&'static str>,
    pub harness: Harness,
    pub emits_activeness: bool,
    pub binding: SurfaceBinding,
    /// The exact invocation. `{bin}` is the shipped binary, `{verb}` the surface's
    /// depth-1 verb, `{tmp}` a hermetic scratch root created per cell.
    pub invocation: &'static str,
    /// What separates pass from fail, in observable terms.
    pub observable: &'static str,
    /// The condition that makes this probe report red.
    pub red_when: &'static str,
    /// A concrete mutation under which this probe MUST report red. A probe that cannot
    /// go red adds a green cell that proves nothing, so the executor's self-test drives
    /// every one of these.
    pub failing_counterpart: &'static str,
}

/// The canonical probe table.
///
/// Nine dimension probes — one per F28-01 dimension, verbatim and fixed — plus one
/// probe per mandatory cell. The nine may not be added to, merged, or renamed: the
/// requirement text is the authority and a renamed dimension is an unprovable one.
pub const PROBES: [ProbeSpec; 12] = [
    ProbeSpec {
        id: "sandbox-probes",
        dimension: Dimension::SandboxProbes,
        families: &[Platform::Linux, Platform::Macos, Platform::Windows],
        cell_id: None,
        harness: Harness::BlackBox,
        emits_activeness: true,
        binding: SurfaceBinding::OsSandboxBackend,
        invocation: "{bin} {verb} --help  (baseline) then {bin} {verb} under \
                     WAYLAND_SANDBOX=none with WAYLAND_ALLOW_NO_SANDBOX unset",
        observable: "the surface must not perform sandboxed execution while the sandbox \
                     is unavailable; a PASS additionally requires a positive activeness \
                     observation captured from the child token (an AppContainer SID and \
                     Low mandatory level on Windows, a bwrap/sandbox-exec containment \
                     marker elsewhere)",
        red_when: "the surface executes sandboxed work with the backend unavailable, or \
                   no activeness observation can be captured for a cell that otherwise \
                   passed",
        failing_counterpart: "force WAYLAND_ALLOW_NO_SANDBOX=1 so the product runs \
                              unsandboxed: the activeness capture finds no containment \
                              marker and the probe must report red",
    },
    ProbeSpec {
        id: "unicode",
        dimension: Dimension::Unicode,
        families: &[Platform::Linux, Platform::Macos, Platform::Windows],
        cell_id: None,
        harness: Harness::BlackBox,
        emits_activeness: false,
        binding: SurfaceBinding::UniversalProcess,
        invocation: "{bin} {verb} --help with HOME and CWD under {tmp}/ünïcode-\u{e9}\u{301}-\u{6f22}\u{5b57}-\u{1f600}",
        observable: "exit status is the surface's own, stdout and stderr are valid UTF-8, \
                     and no panic, replacement character or mojibake appears",
        red_when: "the process panics, emits invalid UTF-8, or emits U+FFFD where the \
                   input was well-formed",
        failing_counterpart: "replace the fixture path with one the product echoes \
                              through a lossy conversion: the U+FFFD assertion fires",
    },
    ProbeSpec {
        id: "long-paths",
        dimension: Dimension::LongPaths,
        families: &[Platform::Linux, Platform::Macos, Platform::Windows],
        cell_id: None,
        harness: Harness::BlackBox,
        emits_activeness: false,
        binding: SurfaceBinding::UniversalProcess,
        invocation: "{bin} {verb} --help with HOME and CWD under a {tmp} path exceeding \
                     260 bytes, built from nested 40-byte components",
        observable: "the surface runs to its own exit status with no path-length error",
        red_when: "the process reports a path-length failure (Windows os error 206, \
                   ENAMETOOLONG elsewhere) or panics on the long path",
        failing_counterpart: "assert against a path deliberately built past the host \
                              limit with long-path support disabled: the os error 206 / \
                              ENAMETOOLONG assertion fires",
    },
    ProbeSpec {
        id: "unc-reparse-symlink",
        dimension: Dimension::UncReparseSymlink,
        families: &[Platform::Linux, Platform::Macos, Platform::Windows],
        cell_id: None,
        harness: Harness::BlackBox,
        emits_activeness: false,
        binding: SurfaceBinding::ReparseAndSymlink,
        invocation: "{bin} {verb} --help with HOME and CWD reached through a reparse \
                     point ({tmp}/link -> {tmp}/real; a directory junction on Windows, a \
                     symbolic link elsewhere), and on Windows additionally through the \
                     UNC spelling \\\\localhost\\<drive>$\\<tmp-relative-path>",
        observable: "the surface runs to its own exit status and does not escape, \
                     duplicate or refuse the linked path",
        red_when: "the process fails on the link where it succeeds on the target, or \
                   resolves the link to a different identity than the target",
        failing_counterpart: "point the link at a nonexistent target: the resolution \
                              assertion fires",
    },
    ProbeSpec {
        id: "process-cleanup",
        dimension: Dimension::ProcessCleanup,
        families: &[Platform::Linux, Platform::Macos, Platform::Windows],
        cell_id: None,
        harness: Harness::BlackBox,
        emits_activeness: false,
        binding: SurfaceBinding::UniversalProcess,
        invocation: "{bin} {verb} --help, with the full descendant set enumerated before \
                     and after by the host's own process table",
        observable: "no process descended from the invocation survives its exit",
        red_when: "any descendant of the invocation is still alive after the parent exits",
        failing_counterpart: "run a fixture that deliberately leaks a detached child: \
                              the survivor assertion fires",
    },
    ProbeSpec {
        id: "suspend-resume",
        dimension: Dimension::SuspendResume,
        families: &[Platform::Linux, Platform::Macos, Platform::Windows],
        cell_id: None,
        harness: Harness::BlackBox,
        emits_activeness: false,
        binding: SurfaceBinding::ProcessSuspension,
        invocation: "{bin} {verb} --help, suspended mid-run and resumed (SIGSTOP/SIGCONT \
                     on Unix, thread suspension on Windows)",
        observable: "the surface completes with the same exit status and output it \
                     produces without the suspension",
        red_when: "the process dies, hangs past its budget, or produces different output \
                   across the suspension",
        failing_counterpart: "suspend without resuming: the budget assertion fires rather \
                              than the run reporting a pass",
    },
    ProbeSpec {
        id: "offline",
        dimension: Dimension::Offline,
        families: &[Platform::Linux, Platform::Macos, Platform::Windows],
        cell_id: None,
        harness: Harness::BlackBox,
        emits_activeness: false,
        binding: SurfaceBinding::UniversalProcess,
        invocation: "{bin} {verb} --help with egress pointed at a closed local port via \
                     HTTP_PROXY, HTTPS_PROXY and ALL_PROXY, and NO_PROXY cleared",
        observable: "the surface completes within its budget and reports an actionable \
                     error rather than hanging or panicking",
        red_when: "the process hangs past its budget, panics, or reports success for work \
                   that required the network",
        failing_counterpart: "point the proxy at a black-hole address that never resets: \
                              the budget assertion fires",
    },
    ProbeSpec {
        id: "disk-full-read-only",
        dimension: Dimension::DiskFullReadOnly,
        families: &[Platform::Linux, Platform::Macos, Platform::Windows],
        cell_id: None,
        harness: Harness::BlackBox,
        emits_activeness: false,
        binding: SurfaceBinding::UniversalProcess,
        invocation: "{bin} {verb} --help with HOME pointed at a read-only directory under \
                     {tmp}",
        observable: "the surface reports a clean, actionable error or completes without \
                     writing; it does not panic and does not report success for a write \
                     it could not perform",
        red_when: "the process panics, or reports success while its write was refused",
        failing_counterpart: "make the read-only root writable and assert the refusal \
                              anyway: the assertion fires, proving it reads the real state",
    },
    ProbeSpec {
        id: "hostile-inputs",
        dimension: Dimension::HostileInputs,
        families: &[Platform::Linux, Platform::Macos, Platform::Windows],
        cell_id: None,
        harness: Harness::BlackBox,
        emits_activeness: false,
        binding: SurfaceBinding::UniversalProcess,
        invocation: "{bin} {verb} with a hostile argv vector: shell metacharacters, \
                     command substitution, ANSI control sequences, a 64 KiB argument, \
                     and an argument that is exactly `--`",
        observable: "no metacharacter is interpreted (the canary file the substitution \
                     would create does not exist), no ANSI sequence reaches the terminal \
                     unescaped, and the process exits with a handled status rather than a \
                     panic",
        red_when: "the canary file exists, an unescaped control sequence is emitted, or \
                   the process panics",
        failing_counterpart: "run the same vector through a deliberately shell-string \
                              invocation: the canary file appears and the probe reports red",
    },
    // ---- the three mandatory cells ----
    ProbeSpec {
        id: "w-sandbox-silent-disable",
        dimension: Dimension::SandboxProbes,
        families: &[Platform::Windows],
        cell_id: Some("w-sandbox-silent-disable"),
        harness: Harness::BlackBox,
        emits_activeness: true,
        binding: SurfaceBinding::OsSandboxBackend,
        invocation: "{bin} swarm --workers 1 --worker-command \"<token-capture>\" with a \
                     deliberately unreconcilable AppContainer ACL lease present",
        observable: "with the availability probe reporting unavailable, the product must \
                     refuse to execute; the token-capture sentinel must NOT exist. A PASS \
                     additionally requires the activeness observation from the clean-lease \
                     leg of the same run",
        red_when: "the sentinel exists while the probe reported the sandbox unavailable — \
                   the product ran the work with no sandbox at all",
        failing_counterpart: "set WAYLAND_ALLOW_NO_SANDBOX=1 so the product is permitted \
                              to proceed unsandboxed: the sentinel appears and the probe \
                              reports red",
    },
    ProbeSpec {
        id: "w-process-cleanup-descendant-tree",
        dimension: Dimension::ProcessCleanup,
        families: &[Platform::Windows],
        cell_id: Some("w-process-cleanup-descendant-tree"),
        harness: Harness::BlackBox,
        emits_activeness: false,
        binding: SurfaceBinding::UniversalProcess,
        invocation: "{bin} swarm --workers 1 --worker-command \"<spawns a detached \
                     grandchild>\", with the descendant set enumerated before and after",
        observable: "no descendant of the invocation, at any depth, survives its exit",
        red_when: "a descendant survives its owner",
        failing_counterpart: "run the same fixture against a worker command that detaches \
                              via a breakaway job: the survivor assertion fires",
    },
    ProbeSpec {
        id: "w-sandbox-observability-control",
        dimension: Dimension::SandboxProbes,
        families: &[Platform::Windows],
        cell_id: Some("w-sandbox-observability-control"),
        harness: Harness::BlackBox,
        emits_activeness: true,
        binding: SurfaceBinding::OsSandboxBackend,
        invocation: "the observability control itself: six availability observations \
                     across two session types and three lease states, plus both \
                     directional controls, recorded in evidence/28-02/controls.json",
        observable: "the control produces one of the three named verdicts with both \
                     directional controls behaving directionally, checked by \
                     f28-check-matrix-results.py --check-controls and \
                     --check-control-directions",
        red_when: "the control is inconclusive, either directional control fails to \
                   behave directionally, or no control ran at all",
        failing_counterpart: "record a negative control that reports `observable`: \
                              --check-control-directions rejects it with F28R-035",
    },
];

/// The nine F28-01 dimension probes, in requirement order. Cell-specific probes are
/// excluded, so a mandatory cell is claimed by exactly one probe rather than two.
pub fn dimension_probes() -> impl Iterator<Item = &'static ProbeSpec> {
    PROBES.iter().filter(|p| p.cell_id.is_none())
}

/// The probe covering a cell, or `None`. A cell with no probe fails the suite rather
/// than being reported absent.
pub fn probe_for(cell_id: &str, dimension: Dimension, os: Platform) -> Option<&'static ProbeSpec> {
    if let Some(specific) = PROBES.iter().find(|p| p.cell_id == Some(cell_id)) {
        return Some(specific);
    }
    let mut claiming =
        dimension_probes().filter(|p| p.dimension == dimension && p.families.contains(&os));
    let first = claiming.next()?;
    // Two probes claiming one cell is ambiguous coverage, which is a defect and not a
    // preference between them.
    if claiming.next().is_some() {
        return None;
    }
    Some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn all_nine_dimensions_have_exactly_one_dimension_probe() {
        for dimension in Dimension::ALL {
            let found: Vec<_> = dimension_probes()
                .filter(|p| p.dimension == dimension)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "{dimension} has {} dimension probes, expected exactly 1",
                found.len()
            );
        }
    }

    #[test]
    fn probe_ids_are_unique() {
        let ids: BTreeSet<&str> = PROBES.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), PROBES.len(), "probe ids must be unique");
    }

    #[test]
    fn families_are_derived_from_the_surface_binding() {
        // The wrong-OS anti-drift guard. A probe listed on a family where the surface
        // it claims to exercise does not exist would pass or skip without ever proving
        // the platform property.
        for spec in &PROBES {
            for os in Platform::ALL {
                let listed = spec.families.contains(&os);
                if listed {
                    assert!(
                        spec.binding.applies_on(os),
                        "probe `{}` is listed on {os} but its surface `{}` does not exist there",
                        spec.id,
                        spec.binding.id()
                    );
                }
            }
            // A cell-specific probe may deliberately narrow to one family; a dimension
            // probe may not, because narrowing one silently drops a whole family's cells.
            if spec.cell_id.is_none() {
                for os in Platform::ALL {
                    assert_eq!(
                        spec.families.contains(&os),
                        spec.binding.applies_on(os),
                        "dimension probe `{}` family list disagrees with its binding on {os}",
                        spec.id
                    );
                }
            }
        }
    }

    #[test]
    fn every_probe_names_a_failing_counterpart() {
        // A probe authored so permissively that it cannot fail adds a green cell that
        // proves nothing, and the only way to see that is to require the mutation that
        // makes it red to be written down.
        for spec in &PROBES {
            assert!(
                !spec.failing_counterpart.trim().is_empty(),
                "probe `{}` names no failing counterpart",
                spec.id
            );
            assert!(
                !spec.red_when.trim().is_empty(),
                "probe `{}` names no red condition",
                spec.id
            );
            assert!(
                !spec.observable.trim().is_empty() && !spec.invocation.trim().is_empty(),
                "probe `{}` is missing its invocation or its observable; a live run is \
                 specified by the exact invocation, an observable outcome and the named \
                 platform, and is not accepted without all three",
                spec.id
            );
        }
    }

    #[test]
    fn sandbox_probes_emit_activeness_and_others_do_not_claim_to() {
        for spec in &PROBES {
            if spec.dimension == Dimension::SandboxProbes {
                assert!(
                    spec.emits_activeness,
                    "sandbox probe `{}` emits no activeness observation; its green could \
                     not be distinguished from a silently disabled sandbox",
                    spec.id
                );
            } else {
                assert!(
                    !spec.emits_activeness,
                    "non-sandbox probe `{}` claims to emit activeness",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn a_mandatory_cell_is_claimed_by_exactly_one_probe() {
        for mandatory in crate::e5_matrix::MANDATORY_CELLS {
            let spec = probe_for(mandatory.id, mandatory.dimension, mandatory.os)
                .unwrap_or_else(|| panic!("mandatory cell `{}` has no probe", mandatory.id));
            assert_eq!(
                spec.cell_id,
                Some(mandatory.id),
                "mandatory cell `{}` is claimed by dimension probe `{}` rather than its own",
                mandatory.id,
                spec.id
            );
        }
    }

    #[test]
    fn unc_verbatim_paths_are_windows_only_and_the_guard_says_so() {
        // The guard has to be able to reject. If `applies_on` returned true for every
        // family the anti-drift test above would pass vacuously.
        assert!(SurfaceBinding::UncVerbatimPath.applies_on(Platform::Windows));
        assert!(!SurfaceBinding::UncVerbatimPath.applies_on(Platform::Linux));
        assert!(!SurfaceBinding::UncVerbatimPath.applies_on(Platform::Macos));
    }
}
