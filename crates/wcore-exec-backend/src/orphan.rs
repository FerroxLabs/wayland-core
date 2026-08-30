//! F25-05 — the orphan scanner, and an honest account of what reaps what.
//!
//! ## Why a count of zero is the dangerous number
//!
//! "No orphaned execution" is not provable by a test suite. A green suite
//! cannot see a process it did not spawn; that is precisely why orphans
//! survive. So every count here is either **measured by a real enumeration**
//! or explicitly **not measured**, and the two are different values —
//! [`OrphanEvidence::observed`] versus [`OrphanEvidence::unobserved`]. An
//! orphan count of zero written without a scan launders an unmeasured hope
//! into evidence, and it is the single worst line this module could contain.
//!
//! ## What actually reaps a process tree in this codebase
//!
//! `wcore_sandbox::backends::process_tree::ProcessTreeMechanism` enumerates
//! EXACTLY three proven mechanisms:
//!
//! * `LinuxPidNamespaceReap` — the bwrap PID-namespace init, reaped via `/proc`
//!   descendant discovery,
//! * `DockerContainerReap` — the daemon force-removing the container and its
//!   whole tree,
//! * `WindowsJobObject` — a kill-on-close Job.
//!
//! **There is no variant for an SSH far end and none for a cloud machine.**
//! The local and container backends inherit a proven mechanism. SSH and cloud
//! inherit nothing, and this module says so rather than implying otherwise.
//!
//! The same module's doc comment is explicit that an ordinary Unix process
//! group is a *reliability backstop* an adversarial child leaves with `setsid`
//! or `setpgid`, and must NEVER by itself qualify as the hard containment
//! boundary. An SSH backend whose far-end cleanup rests on a remote process
//! group therefore has a **best-effort** reap, and [`ReapingMechanism`] records
//! it as best-effort. Relabelling it as kernel-backed would be the exact
//! dishonesty this phase exists to remove.

use serde::{Deserialize, Serialize};

use crate::contract::{BackendKind, OrphanScan, OrphanScope, ResourceBudget};
use crate::error::Result;

/// What a backend relies on to take a process tree down with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mechanism", rename_all = "snake_case")]
pub enum ReapingMechanism {
    /// One of `wcore-sandbox`'s three PROVEN mechanisms. The name is carried as
    /// a string rather than the foreign enum so this crate does not re-export
    /// `wcore-sandbox`'s type surface, but it is the same three names and
    /// nothing else may be spelled here.
    KernelBacked {
        /// `LinuxPidNamespaceReap` | `DockerContainerReap` | `WindowsJobObject`
        process_tree_mechanism: String,
        detail: String,
    },
    /// A remote process group, or equivalent. Takes the common case down and is
    /// escapable by an adversarial child via `setsid` / `setpgid`. Recorded as
    /// best-effort because that is what it is.
    BestEffort { detail: String },
    /// Nothing takes the tree down automatically.
    None { detail: String },
}

impl ReapingMechanism {
    pub fn is_kernel_backed(&self) -> bool {
        matches!(self, ReapingMechanism::KernelBacked { .. })
    }

    pub fn label(&self) -> String {
        match self {
            ReapingMechanism::KernelBacked {
                process_tree_mechanism,
                detail,
            } => {
                format!("kernel-backed: ProcessTreeMechanism::{process_tree_mechanism} — {detail}")
            }
            ReapingMechanism::BestEffort { detail } => format!("BEST-EFFORT: {detail}"),
            ReapingMechanism::None { detail } => format!("NONE: {detail}"),
        }
    }
}

/// The mechanism each backend kind actually relies on.
///
/// Derived from `ProcessTreeMechanism`'s three variants and from what plan
/// 25-01's backends actually do — not from what would be convenient to claim.
pub fn mechanism_for(kind: BackendKind) -> ReapingMechanism {
    match kind {
        BackendKind::Local => {
            if cfg!(target_os = "linux") {
                ReapingMechanism::KernelBacked {
                    process_tree_mechanism: "LinuxPidNamespaceReap".into(),
                    detail: "the platform sandbox backend's PID namespace, with /proc \
                             descendant discovery"
                        .into(),
                }
            } else if cfg!(windows) {
                ReapingMechanism::KernelBacked {
                    process_tree_mechanism: "WindowsJobObject".into(),
                    detail: "a kill-on-close Job Object owning the descendant tree".into(),
                }
            } else {
                // macOS has no PID namespace and no Job Object. `process_tree`
                // carries a `MacProcessGroupAuthority`, which is a process
                // group — a backstop, not a boundary.
                ReapingMechanism::BestEffort {
                    detail: "a Unix process group; this platform has neither a PID namespace \
                             nor a Job Object, and a child can leave a group with setsid"
                        .into(),
                }
            }
        }
        BackendKind::Container => ReapingMechanism::KernelBacked {
            process_tree_mechanism: "DockerContainerReap".into(),
            detail: "the container daemon force-removing the container and its whole tree".into(),
        },
        BackendKind::Ssh => ReapingMechanism::BestEffort {
            detail: "a remote process group killed over the same ssh transport. \
                     ProcessTreeMechanism has NO variant that crosses an ssh connection, so \
                     nothing kernel-backed on the controller reaps the far end; a remote child \
                     that calls setsid survives"
                .into(),
        },
        BackendKind::Cloud => ReapingMechanism::None {
            detail: "stopping a cloud machine is a vendor API call, not a kernel mechanism. \
                     ProcessTreeMechanism has no variant for it, and if the API call does not \
                     land the machine keeps running"
                .into(),
        },
    }
}

/// A measured — or explicitly unmeasured — orphan count.
///
/// The two constructors are the whole point. There is deliberately no
/// `Default`, and no way to produce a count of zero without saying whether it
/// was observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanEvidence {
    pub backend_id: String,
    pub kind: BackendKind,
    /// `None` when the scan was not restricted to one run. See
    /// [`OrphanScan::nonce`].
    pub nonce: Option<String>,
    /// The enumeration that was run, in words an operator can re-run.
    pub method: String,
    pub mechanism: ReapingMechanism,
    /// `Some` ONLY when an enumeration actually ran.
    pub orphan_count: Option<u64>,
    /// The raw rows the enumeration returned. Empty with `orphan_count:
    /// Some(0)` means "looked, found nothing"; empty with `None` means
    /// "did not look".
    pub rows: Vec<String>,
    /// Present when the count is `None`.
    pub unobserved_reason: Option<String>,
}

impl OrphanEvidence {
    /// A count that was MEASURED.
    pub fn observed(scan: &OrphanScan) -> Self {
        Self {
            backend_id: scan.backend_id.clone(),
            kind: scan.kind,
            nonce: scan.nonce.clone(),
            method: scan.method.clone(),
            mechanism: mechanism_for(scan.kind),
            orphan_count: Some(scan.found.len() as u64),
            rows: scan.found.clone(),
            unobserved_reason: None,
        }
    }

    /// A count that was NOT measured. Carries no number at all.
    pub fn unobserved(scan: &OrphanScan, reason: impl Into<String>) -> Self {
        Self {
            backend_id: scan.backend_id.clone(),
            kind: scan.kind,
            nonce: scan.nonce.clone(),
            method: scan.method.clone(),
            mechanism: mechanism_for(scan.kind),
            orphan_count: None,
            rows: Vec::new(),
            unobserved_reason: Some(reason.into()),
        }
    }

    /// Build from a scan, honouring the scan's own `enumerated` flag.
    pub fn from_scan(scan: &OrphanScan) -> Self {
        if scan.enumerated {
            Self::observed(scan)
        } else {
            Self::unobserved(
                scan,
                format!(
                    "the surface could not be enumerated ({}), so no count exists — this is \
                     NOT zero orphans",
                    scan.method
                ),
            )
        }
    }

    pub fn is_observed(&self) -> bool {
        self.orphan_count.is_some()
    }

    /// One operator-facing line.
    pub fn label(&self) -> String {
        match self.orphan_count {
            Some(0) => format!(
                "{}: 0 orphans, MEASURED via {}",
                self.backend_id, self.method
            ),
            Some(n) => format!(
                "{}: {n} ORPHAN(S) FOUND via {}",
                self.backend_id, self.method
            ),
            None => format!(
                "{}: NOT MEASURED — {}",
                self.backend_id,
                self.unobserved_reason
                    .as_deref()
                    .unwrap_or("no reason recorded")
            ),
        }
    }
}

/// Scan every reference backend this build carries, in `scope`.
///
/// # The scope is a parameter, and there is no default (core#366 d2)
///
/// This function used to hardcode the nonce-scoped question. Everything built
/// on it therefore inherited a scope its author never chose, and one of those
/// things is an operator-facing gate — `wayland-core backend scan`, whose
/// non-zero exit a human wires into CI. It printed `count 0 (MEASURED)` with a
/// labelled leftover sitting in `docker ps -a`, because the only question it
/// could ask was "is there residue under the nonce I am holding", and a fresh
/// nonce can never match a previous run's container.
///
/// Adding a second, unscoped function beside this one would have fixed the one
/// caller that was named in the ticket and left every other caller — and every
/// future caller — on the silent default. It was tried in this lane's first
/// pass and shipped with ZERO callers while the surface it existed for stayed
/// broken. Taking [`OrphanScope`] instead makes the choice non-optional: the
/// caller set and what each one asks is now whatever `cargo check` accepts,
/// not whatever a note claims.
pub async fn scan_all(
    scope: OrphanScope<'_>,
    limits: ResourceBudget,
) -> Result<Vec<OrphanEvidence>> {
    let mut out = Vec::new();
    for reference in crate::reference_backends(limits)? {
        let scan = reference.backend.scan_orphans_in_scope(scope).await?;
        out.push(OrphanEvidence::from_scan(&scan));
    }
    out.sort_by(|a, b| a.backend_id.cmp(&b.backend_id));
    Ok(out)
}

/// Scan one named backend, in `scope`. Same rule as [`scan_all`]: the caller
/// states the scope, because the caller is the only one who knows which
/// question it means.
pub async fn scan_one(
    backend_id: &str,
    scope: OrphanScope<'_>,
    limits: ResourceBudget,
) -> Result<Option<OrphanEvidence>> {
    let Some(reference) = crate::reference_backend_named(backend_id, limits)? else {
        return Ok(None);
    };
    let scan = reference.backend.scan_orphans_in_scope(scope).await?;
    Ok(Some(OrphanEvidence::from_scan(&scan)))
}

/// Enumerate the LOCAL process table for `nonce`, in argv mode.
///
/// Separate from the backends' own `scan_orphans` so the live exercise has an
/// implementation to check them AGAINST. A scanner checked only by itself has
/// not been checked.
///
/// Every value is a separate argv entry and the filtering happens in Rust, not
/// in a shell — the nonce is task-derived data and interpolating it into a
/// shell string is the shape AGENTS.md forbids. Filtering in Rust also avoids
/// the whole class of "the filter silently dropped lines", which here would
/// report zero orphans while orphans exist.
pub async fn local_process_rows(nonce: &str) -> Result<Vec<String>> {
    match enumerate_process_table(nonce).await {
        ProcessTableScan::Enumerated { rows } => Ok(rows),
        ProcessTableScan::CannotDetermine { reason } => Err(crate::error::ExecError::Exec(reason)),
    }
}

/// The result of trying to read the host process table.
///
/// **Three states collapse to two everywhere else in this codebase and that is
/// the bug.** "Found nothing" and "could not look" are different facts, and a
/// scanner that returns `0` for the second is reporting proof of correctness it
/// does not have. This type makes the second one unrepresentable as a number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessTableScan {
    /// The table was read AND the instrument proved it can see command lines.
    Enumerated { rows: Vec<String> },
    /// The table could not be read, or could be read but not usefully. Carries
    /// no count at all — deliberately.
    CannotDetermine { reason: String },
}

impl ProcessTableScan {
    pub fn rows(&self) -> &[String] {
        match self {
            ProcessTableScan::Enumerated { rows } => rows,
            ProcessTableScan::CannotDetermine { .. } => &[],
        }
    }
    pub fn is_determinate(&self) -> bool {
        matches!(self, ProcessTableScan::Enumerated { .. })
    }
}

/// Read the host process table and filter it for `nonce`.
///
/// # The two findings this function exists to hold closed
///
/// **1. `tasklist` cannot see command lines at all.** The Windows arm used
/// `tasklist /V /FO CSV`, whose columns are image name, pid, session, memory,
/// status, user, CPU and window title — no command line anywhere. The nonce
/// lives in the command line, so it was never in the output and the scanner
/// returned a *MEASURED zero while a process carrying the nonce was running*.
/// Measured on SeanDesktop: `Win32_Process` found 1 row, the scanner said 0.
///
/// **2. `Win32_Process.CommandLine` can come back NULL.** It is not readable
/// for processes owned by another user without the right privilege, and under
/// some conditions it is empty across the board. An enumeration that "succeeds"
/// with every command line blank looks *exactly* like a clean host, and would
/// reproduce finding 1 with a different instrument.
///
/// So the instrument SELF-TESTS: this process's own row must be present AND
/// carry a non-empty command line. We know our own pid, and we know we have a
/// command line, so if we cannot see our own we cannot see anyone's — and that
/// is reported as [`ProcessTableScan::CannotDetermine`], never as zero.
/// The `ps -eo` field list for this Unix.
///
/// Exactly four leading whitespace-free columns before `args`, on every arm —
/// see [`row_command_line`], which skips a fixed count.
#[cfg(all(unix, target_os = "linux"))]
pub(crate) const UNIX_PS_FIELDS: &str = "pid,ppid,pgid,etimes,args";
/// BSD `ps` (macOS, the *BSDs) has no `etimes`; the elapsed column is `etime`,
/// formatted `[[dd-]hh:]mm:ss`.
#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) const UNIX_PS_FIELDS: &str = "pid,ppid,pgid,etime,args";
/// Never used on Windows (that arm builds a PowerShell command instead), but
/// defined so the constant exists on every target and the test below can
/// assert its shape without a platform gate.
#[cfg(not(unix))]
pub(crate) const UNIX_PS_FIELDS: &str = "pid,ppid,pgid,etime,args";

pub async fn enumerate_process_table(nonce: &str) -> ProcessTableScan {
    // The PowerShell argument is a FIXED literal — the nonce is never
    // interpolated into it and the filtering happens in Rust — so this is not
    // an injection surface even though it is a shell string. `-Property
    // ProcessId,ParentProcessId,CommandLine` keeps the CIM query narrow.
    let program = if cfg!(windows) { "powershell" } else { "ps" };
    let args: Vec<&str> = if cfg!(windows) {
        vec![
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId,CommandLine \
             | ForEach-Object { \"$($_.ProcessId) $($_.ParentProcessId) $($_.CommandLine)\" }",
        ]
    } else {
        // THREE platforms, so three arms — not two. `cfg!(windows)` alone left
        // macOS silently inheriting the Linux arm, and `etimes` is a procps
        // extension: BSD `ps` answers `ps: etimes: keyword not found`, exits 1
        // and prints a header with the column simply MISSING. That is a scan
        // this module can only ever report as `CannotDetermine` — honest, but
        // permanently so, which makes the whole Unix orphan scanner unable to
        // pass on macOS. BSD spells the same column `etime`.
        //
        // The column count must stay at 4 across both spellings, because
        // `row_command_line` skips a fixed number of leading fields. Neither
        // `etimes` (`3600`) nor `etime` (`33-15:30:15`) contains whitespace,
        // so it does.
        vec!["-eo", UNIX_PS_FIELDS]
    };

    let mut command = wcore_config::shell::shell_command_argv(program, &args);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = match command.output().await {
        Ok(o) => o,
        Err(e) => {
            return ProcessTableScan::CannotDetermine {
                reason: format!("could not run `{program}` to enumerate processes: {e}"),
            };
        }
    };
    if !output.status.success() {
        return ProcessTableScan::CannotDetermine {
            reason: format!(
                "`{program}` exited {}: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        };
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let all: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if all.is_empty() {
        return ProcessTableScan::CannotDetermine {
            reason: format!("`{program}` returned no rows at all"),
        };
    }

    let me = std::process::id();
    if let Some(reason) = command_line_visibility_failure(&all, me, program) {
        return ProcessTableScan::CannotDetermine { reason };
    }

    // EXCLUDE THE SCANNER'S WHOLE LINEAGE, not merely its own pid.
    //
    // `wayland-core backend scan --task-id <nonce>` carries the nonce on its
    // own argv, so the scanner matches itself and reports an orphan that does
    // not exist. Plan 25-01 hit that on the remote scanner and it recurred
    // here the moment the local scan started reading the real process table;
    // excluding `me` closed it, and only it.
    //
    // The nonce is on the argv of everything that INVOKED the scanner too --
    // the shell, the ssh session, the CI step. A reviewer running this scan
    // over ssh got their own ssh command line back as a MEASURED orphan,
    // which on the scriptable F25-05 gate is a non-zero exit over nothing at
    // all. "Which wrappers should I skip" is undecidable over an open
    // alphabet of shells and runners; "is this row an ancestor of the process
    // asking the question" is decidable, and the answer is already in the
    // table, because the enumeration carries ppid. An ancestor of a LIVE
    // scanner is by construction not a leaked task: it is what started the
    // scanner and is waiting on it.
    let lineage = self_and_ancestors(&all, me);
    ProcessTableScan::Enumerated {
        rows: all
            .into_iter()
            .filter(|line| line.contains(nonce))
            .filter(|line| !row_pid(line).is_some_and(|pid| lineage.contains(&pid)))
            .map(|line| line.trim().to_string())
            .collect(),
    }
}

/// The pid asking the question, plus every ancestor of it the enumeration can
/// see.
///
/// Walks the `ppid` column upward and stops on a pid it has already seen, so a
/// table captured mid-reparent -- or a malformed one -- cannot loop. This
/// NARROWS the exclusion to one chain; it never widens it to "anything that
/// looks like a wrapper", and a row the walk does not reach is untouched.
fn self_and_ancestors(rows: &[&str], me: u32) -> Vec<u32> {
    let mut lineage = vec![me];
    let mut current = me;
    while let Some(parent) = rows
        .iter()
        .find(|row| row_pid(row) == Some(current))
        .and_then(|row| row_ppid(row))
    {
        if parent == 0 || lineage.contains(&parent) {
            break;
        }
        lineage.push(parent);
        current = parent;
    }
    lineage
}

/// The instrument's self-test. `None` means command lines are genuinely visible.
///
/// Split out so it is unit-testable against captured output, including the
/// NULL-`CommandLine` shape that cannot be induced on a Linux CI box.
fn command_line_visibility_failure(rows: &[&str], me: u32, program: &str) -> Option<String> {
    let own = rows.iter().find(|row| row_pid(row) == Some(me));
    match own {
        None => Some(format!(
            "`{program}` listed {} process(es) but not this one (pid {me}); the enumeration \
             cannot see the process asking the question, so it cannot be trusted to see an \
             orphan either",
            rows.len()
        )),
        Some(row) if row_command_line(row).is_empty() => Some(format!(
            "`{program}` listed this process (pid {me}) with an EMPTY command line. The nonce \
             lives in the command line, so a filter over this output can only ever return \
             zero. On Windows this is the documented NULL-CommandLine case — Win32_Process \
             does not expose it without sufficient privilege. Reporting zero here would be a \
             false negative dressed as a measurement"
        )),
        Some(_) => None,
    }
}

/// The command-line portion of a row: everything after the fixed leading
/// numeric columns.
fn row_command_line(row: &str) -> &str {
    // Unix: `pid ppid pgid <elapsed> args…` (4 leading columns; the elapsed
    // one is `etimes` on Linux and `etime` on BSD — see `UNIX_PS_FIELDS`).
    // Windows: `pid ppid CommandLine` (2 numeric columns).
    let skip = if cfg!(windows) { 2 } else { 4 };
    let mut rest = row.trim();
    for _ in 0..skip {
        match rest.split_once(char::is_whitespace) {
            Some((_, tail)) => rest = tail.trim_start(),
            None => return "",
        }
    }
    rest
}

/// The pid a process-listing row describes.
///
/// Both platforms emit `<pid> <ppid> …`, so there is ONE parse rather than two
/// that can drift apart. A row whose pid cannot be parsed is KEPT by callers —
/// dropping a row we failed to understand would be a filter that silently loses
/// orphans, which is the worst failure available to this module.
fn row_pid(row: &str) -> Option<u32> {
    row.split_whitespace().next().and_then(|f| f.parse().ok())
}

/// The parent pid column. Second on every arm: Unix is
/// `pid ppid pgid <elapsed> args`, Windows is `pid ppid CommandLine`.
fn row_ppid(row: &str) -> Option<u32> {
    row.split_whitespace().nth(1).and_then(|f| f.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scan must not report the processes that STARTED it.
    ///
    /// A reviewer ran the scoped scan over ssh and got their own ssh command
    /// line back as a MEASURED orphan: the nonce is on the scanner's argv, and
    /// therefore on the argv of every wrapper that invoked it. On the
    /// scriptable `backend scan` gate that is a non-zero exit over nothing.
    #[test]
    fn the_scanners_whole_lineage_is_excluded_not_only_its_own_pid() {
        // `pid ppid pgid etimes args`, the Linux shape.
        let rows = vec![
            "1 0 1 999 /sbin/init",
            "100 1 100 500 sshd: user [priv] -- backend scan --task-id NONCE-X",
            "200 100 200 400 bash -lc wayland-core backend scan --task-id NONCE-X",
            "300 200 300 300 wayland-core backend scan --task-id NONCE-X",
            "400 1 400 100 /usr/bin/leaked-task --nonce NONCE-X",
        ];
        let lineage = self_and_ancestors(&rows, 300);
        assert_eq!(
            lineage,
            vec![300, 200, 100, 1],
            "the walk must reach every ancestor, not stop at the immediate parent"
        );
        // CONTROL, the load-bearing half: a genuine leftover that is NOT in
        // the lineage must stay visible. An exclusion that hid it would turn
        // this scanner into the silent zero the whole module exists to refuse.
        assert!(
            !lineage.contains(&400),
            "a leftover outside the scanner's ancestry must remain reportable"
        );
    }

    /// A malformed or mid-reparent table must not spin.
    #[test]
    fn a_cyclic_parent_chain_terminates() {
        let rows = vec!["10 20 10 5 a", "20 10 20 5 b"];
        assert_eq!(self_and_ancestors(&rows, 10), vec![10, 20]);
    }

    #[test]
    fn ssh_and_cloud_are_not_claimed_kernel_backed() {
        assert!(!mechanism_for(BackendKind::Ssh).is_kernel_backed());
        assert!(!mechanism_for(BackendKind::Cloud).is_kernel_backed());
        assert!(
            mechanism_for(BackendKind::Ssh)
                .label()
                .contains("BEST-EFFORT")
        );
        assert!(mechanism_for(BackendKind::Cloud).label().contains("NONE"));
    }

    #[test]
    fn container_inherits_the_proven_docker_reap() {
        let m = mechanism_for(BackendKind::Container);
        assert!(m.is_kernel_backed());
        assert!(m.label().contains("DockerContainerReap"));
    }

    /// Every variant of the upstream authority, DERIVED rather than restated.
    ///
    /// The previous version of the test below compared one hand-written list
    /// against another: an `allowed` array typed here, and the string literals
    /// typed into `mechanism_for`. Neither could notice `wcore-sandbox`
    /// renaming a variant or adding a fourth — and this crate DEPENDS on
    /// `wcore-sandbox`, so the authority was reachable the whole time. A
    /// stale name here is not cosmetic: it is printed to an operator by
    /// `wayland-core backend scan` as the mechanism a backend relies on.
    ///
    /// Both rot modes are now COMPILE errors rather than green tests. The
    /// match has no wildcard, so a fourth variant upstream fails to compile
    /// here; the names come from the derived `Debug`, so a rename fails to
    /// compile too.
    fn every_upstream_mechanism() -> Vec<String> {
        use wcore_sandbox::backends::process_tree::ProcessTreeMechanism;
        let all = [
            ProcessTreeMechanism::LinuxPidNamespaceReap,
            ProcessTreeMechanism::DockerContainerReap,
            ProcessTreeMechanism::WindowsJobObject,
        ];
        for mechanism in all {
            match mechanism {
                ProcessTreeMechanism::LinuxPidNamespaceReap
                | ProcessTreeMechanism::DockerContainerReap
                | ProcessTreeMechanism::WindowsJobObject => {}
            }
        }
        all.iter().map(|m| format!("{m:?}")).collect()
    }

    #[test]
    fn only_the_three_proven_mechanism_names_are_ever_spelled() {
        let allowed = every_upstream_mechanism();
        // CONTROL: an empty or truncated derivation would make the assertion
        // below vacuously true, which is the failure this rewrite exists to
        // remove rather than reproduce.
        assert_eq!(
            allowed.len(),
            3,
            "CONTROL: wcore-sandbox's ProcessTreeMechanism must still carry exactly the three \
             proven mechanisms; derived {allowed:?}"
        );
        for kind in [
            BackendKind::Local,
            BackendKind::Container,
            BackendKind::Ssh,
            BackendKind::Cloud,
        ] {
            if let ReapingMechanism::KernelBacked {
                process_tree_mechanism,
                ..
            } = mechanism_for(kind)
            {
                assert!(
                    allowed.contains(&process_tree_mechanism),
                    "{kind:?} claims a mechanism wcore-sandbox does not implement: \
                     {process_tree_mechanism}. The authority is \
                     wcore_sandbox::backends::process_tree::ProcessTreeMechanism, which spells \
                     {allowed:?}, and this string is printed to an operator by `backend scan`."
                );
            }
        }
    }

    fn scan(enumerated: bool, found: Vec<String>) -> OrphanScan {
        OrphanScan {
            backend_id: "local".into(),
            kind: BackendKind::Local,
            nonce: Some("n-1".into()),
            method: "test".into(),
            found,
            enumerated,
        }
    }

    /// The central distinction: "looked and found nothing" is a number;
    /// "did not look" is not.
    #[test]
    fn an_unenumerable_surface_yields_no_count_at_all_rather_than_zero() {
        let looked = OrphanEvidence::from_scan(&scan(true, vec![]));
        assert_eq!(looked.orphan_count, Some(0));
        assert!(looked.is_observed());
        assert!(looked.label().contains("MEASURED"));

        let did_not_look = OrphanEvidence::from_scan(&scan(false, vec![]));
        assert_eq!(did_not_look.orphan_count, None);
        assert!(!did_not_look.is_observed());
        assert!(did_not_look.label().contains("NOT MEASURED"));
        assert!(
            did_not_look
                .unobserved_reason
                .as_deref()
                .unwrap()
                .contains("NOT zero orphans")
        );
    }

    #[test]
    fn a_found_orphan_is_carried_with_its_raw_row() {
        let e =
            OrphanEvidence::from_scan(&scan(true, vec!["12345 1 12345 30 sleep 120 n-1".into()]));
        assert_eq!(e.orphan_count, Some(1));
        assert_eq!(e.rows.len(), 1);
        assert!(e.label().contains("1 ORPHAN(S) FOUND"));
    }

    // ---- the instrument's self-test -------------------------------------
    //
    // These pin the exact defect that made the Windows scanner report a
    // MEASURED zero while an orphan was running. They are unit tests over
    // captured output shapes because the NULL-CommandLine condition cannot be
    // induced on a Linux CI box — and "we could not reproduce it so we did not
    // test it" is how it survived the first time.

    #[test]
    fn windows_null_command_lines_are_cannot_determine_not_zero() {
        // The shape Win32_Process returns when CommandLine is not readable:
        // pids and parents are present, the command line is blank.
        let rows = ["4321 1 ", "9999 4321 "];
        let rows: Vec<&str> = rows.to_vec();
        let failure = command_line_visibility_failure(&rows, 4321, "powershell")
            .expect("blank command lines must be CannotDetermine");
        assert!(failure.contains("EMPTY command line"), "{failure}");
        assert!(
            failure.contains("false negative"),
            "the reason must say what the consequence is: {failure}"
        );
    }

    #[test]
    fn an_enumeration_that_cannot_see_its_own_process_is_cannot_determine() {
        let rows = vec!["1 0 1 99 /sbin/init", "2 0 2 99 [kthreadd]"];
        let failure = command_line_visibility_failure(&rows, 424242, "ps")
            .expect("not seeing our own process must be CannotDetermine");
        assert!(
            failure.contains("cannot see the process asking"),
            "{failure}"
        );
    }

    #[test]
    fn a_healthy_enumeration_passes_the_self_test() {
        let rows = vec![
            "1 0 1 99 /sbin/init",
            "4321 1 4321 9 wayland-core backend scan",
        ];
        assert_eq!(
            command_line_visibility_failure(&rows, 4321, "ps"),
            None,
            "an enumeration that shows our own command line must be trusted"
        );
    }

    /// The self-test itself has to be able to pass AND fail, or it is decoration.
    #[tokio::test]
    async fn the_real_process_table_passes_its_own_self_test_on_this_host() {
        let scan = enumerate_process_table("f25-nonce-that-matches-nothing").await;
        assert!(
            scan.is_determinate(),
            "the process table could not be enumerated on this host: {scan:?}"
        );
        assert!(
            scan.rows().is_empty(),
            "a nonce matching nothing must yield no rows"
        );
    }

    #[test]
    fn the_unix_field_list_keeps_exactly_four_columns_before_args() {
        // `row_command_line` skips a FIXED four leading fields on Unix. If a
        // future edit adds or removes a column from `UNIX_PS_FIELDS` without
        // updating that count, every orphan row silently loses (or gains) a
        // word of its command line — and the nonce lives in that command line.
        let fields: Vec<&str> = UNIX_PS_FIELDS.split(',').collect();
        assert_eq!(
            fields.len(),
            5,
            "UNIX_PS_FIELDS must be exactly 4 columns plus `args`, got {UNIX_PS_FIELDS}"
        );
        assert_eq!(*fields.last().unwrap(), "args");
        assert_eq!(&fields[..3], &["pid", "ppid", "pgid"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn this_hosts_ps_accepts_our_field_list_and_keeps_the_columns_aligned() {
        // The test that would have caught the macOS defect. `etimes` is a
        // procps extension: BSD `ps` rejects the keyword, exits 1, and emits a
        // header with the column MISSING. A four-field skip over three actual
        // columns eats the first word of every command line.
        //
        // This drives the REAL `ps` on the REAL host, so it fails on any Unix
        // whose `ps` does not accept what `UNIX_PS_FIELDS` asks for.
        let output = wcore_config::shell::shell_command_argv("ps", &["-eo", UNIX_PS_FIELDS])
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .expect("could not run ps");
        assert!(
            output.status.success(),
            "`ps -eo {UNIX_PS_FIELDS}` failed on this host ({}): {}",
            std::env::consts::OS,
            String::from_utf8_lossy(&output.stderr).trim()
        );

        let text = String::from_utf8_lossy(&output.stdout);
        let mut lines = text.lines();
        let header = lines.next().expect("ps produced no header");
        assert_eq!(
            header.split_whitespace().count(),
            5,
            "ps emitted {header:?} — that is not 4 columns plus ARGS, so the fixed skip in \
             row_command_line is misaligned"
        );

        // And the elapsed column must be whitespace-free, or the skip drifts
        // per row rather than uniformly. Our own row is the one we can check:
        // we know its pid.
        let me = std::process::id();
        let own = lines
            .find(|line| row_pid(line) == Some(me))
            .unwrap_or_else(|| panic!("ps did not list this process (pid {me})"));
        let command_line = row_command_line(own);
        assert!(
            !command_line.is_empty(),
            "our own row {own:?} yielded an EMPTY command line after skipping 4 fields; the \
             columns do not line up"
        );
        assert!(
            own.ends_with(command_line),
            "the skipped prefix of {own:?} did not stop at the command line"
        );
    }

    #[test]
    fn cannot_determine_carries_no_count_at_all() {
        let scan = ProcessTableScan::CannotDetermine {
            reason: "blank command lines".into(),
        };
        assert!(!scan.is_determinate());
        assert!(scan.rows().is_empty());
        // And there is deliberately no `count()` on this type: the ONLY way to
        // get a number out of a scan is through the Enumerated arm.
    }

    #[test]
    fn the_command_line_column_is_found_on_each_platform_shape() {
        if cfg!(windows) {
            assert_eq!(
                row_command_line("4321 1 wayland-core backend scan"),
                "wayland-core backend scan"
            );
            assert_eq!(row_command_line("4321 1 "), "");
        } else {
            assert_eq!(
                row_command_line("4321 1 4321 9 sh -c while :; do sleep 1; done"),
                "sh -c while :; do sleep 1; done"
            );
            assert_eq!(row_command_line("4321 1 4321 9"), "");
        }
    }

    #[test]
    fn a_row_whose_pid_cannot_be_parsed_is_kept_rather_than_dropped() {
        // Dropping an unparseable row would be a filter that silently loses
        // orphans — the single worst failure this module can have.
        assert_eq!(row_pid("  1234 1 1234 9 sh -c ..."), Some(1234));
        assert_eq!(row_pid("not-a-pid something"), None);
    }

    /// The scanner must be able to return NONZERO, or a zero proves nothing.
    /// This deliberately leaves a descendant behind and requires it be found.
    #[tokio::test]
    #[cfg(unix)]
    async fn the_local_scanner_finds_a_descendant_that_was_deliberately_left_behind() {
        let nonce = format!("f25-orphan-probe-{}", std::process::id());
        // A process that outlives this test body unless something reaps it.
        // NO `exec`. `exec sleep 30 # nonce` replaces the shell, and the
        // replacement's argv is just `sleep 30` — the nonce is GONE, so the
        // scan finds nothing and the test passes for the wrong reason. That is
        // exactly the defect plan 25-01 hit on its remote scanner: the task's
        // own argv carried no nonce, so a genuine orphan was invisible. The
        // shell here keeps its full argv, comment included.
        let mut child = wcore_config::shell::shell_command_argv(
            "sh",
            &["-c", &format!("while :; do sleep 1; done # {nonce}")],
        )
        .spawn()
        .expect("spawn the deliberate orphan");
        // Give it a moment to appear in the process table.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        let rows = local_process_rows(&nonce).await.unwrap();
        let _ = child.kill().await;
        let _ = child.wait().await;

        assert!(
            !rows.is_empty(),
            "the scanner returned zero for a process that was definitely running — \
             a scanner that has never returned nonzero has not been shown to work"
        );

        // And after the reap it must go back to zero, measured.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let after = local_process_rows(&nonce).await.unwrap();
        assert!(
            after.is_empty(),
            "the deliberate orphan survived its own reap: {after:?}"
        );
    }

    /// The scanner must not count ITSELF. This process's own argv carries the
    /// nonce whenever a caller passes one on the command line, and plan 25-01
    /// already hit this exact defect on the remote scanner.
    #[tokio::test]
    #[cfg(unix)]
    async fn the_scanner_does_not_count_its_own_process() {
        // A nonce this process itself carries: our own pid, which appears in
        // no other process's argv but is trivially findable if we scanned
        // ourselves by pid.
        let me = std::process::id();
        let rows = local_process_rows(&me.to_string()).await.unwrap();
        for row in &rows {
            assert_ne!(
                row_pid(row),
                Some(me),
                "the scanner counted its own process as an orphan: {row}"
            );
        }
    }
}
