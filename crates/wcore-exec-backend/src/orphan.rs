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

use crate::contract::{BackendKind, OrphanScan, ResourceBudget};
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
    pub nonce: String,
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

/// Scan every reference backend this build carries for surfaces still carrying
/// `nonce`.
pub async fn scan_all(nonce: &str, limits: ResourceBudget) -> Result<Vec<OrphanEvidence>> {
    let mut out = Vec::new();
    for reference in crate::reference_backends(limits)? {
        let scan = reference.backend.scan_orphans(nonce).await?;
        out.push(OrphanEvidence::from_scan(&scan));
    }
    out.sort_by(|a, b| a.backend_id.cmp(&b.backend_id));
    Ok(out)
}

/// Scan one named backend.
pub async fn scan_one(
    backend_id: &str,
    nonce: &str,
    limits: ResourceBudget,
) -> Result<Option<OrphanEvidence>> {
    let Some(reference) = crate::reference_backend_named(backend_id, limits)? else {
        return Ok(None);
    };
    let scan = reference.backend.scan_orphans(nonce).await?;
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
    // F25-05 FINDING (HIGH), fixed here.
    //
    // This used `tasklist /V /FO CSV` on Windows. **`tasklist` does not print
    // command lines at all** — its columns are image name, pid, session,
    // memory, status, user, CPU and window title. The nonce lives in the
    // command line, so it was never in the output, and the scanner returned a
    // *MEASURED* zero while a process carrying the nonce was running.
    //
    // A measured zero that is wrong is strictly worse than an unmeasured one,
    // because it is the value this module exists to make trustworthy. Measured
    // on SeanDesktop: Win32_Process found 1 row, the scanner reported 0.
    //
    // `Get-CimInstance Win32_Process` is the enumeration that carries
    // `CommandLine`. The PowerShell argument is a FIXED literal — the nonce is
    // never interpolated into it, and the filtering happens in Rust — so this
    // is not a shell-injection surface even though it is a shell string.
    let program = if cfg!(windows) { "powershell" } else { "ps" };
    let args: Vec<&str> = if cfg!(windows) {
        vec![
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.ParentProcessId) $($_.CommandLine)\" }",
        ]
    } else {
        vec!["-eo", "pid,ppid,pgid,etimes,args"]
    };
    let mut command = wcore_config::shell::shell_command_argv(program, &args);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = command
        .output()
        .await
        .map_err(|e| crate::error::ExecError::Exec(format!("enumerating processes: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let me = std::process::id();
    Ok(text
        .lines()
        .filter(|line| line.contains(nonce))
        // EXCLUDE OURSELVES. `wayland-core backend scan --task-id <nonce>`
        // carries the nonce on its own argv, so the scanner matches itself and
        // reports one orphan that does not exist. This is the SAME defect plan
        // 25-01 hit on the remote scanner ("the remote orphan scanner found
        // itself"), and it recurred here the moment the local scan started
        // reading the real process table. Measured on hetzner-dsm: scanner=1,
        // independent enumeration=0, and the one row was the scanner.
        .filter(|line| row_pid(line) != Some(me))
        .map(|line| line.trim().to_string())
        .collect())
}

/// The pid a process-listing row describes.
///
/// `ps -eo pid,...` puts it first; `tasklist /FO CSV` puts it in the second
/// quoted field. A row whose pid cannot be parsed is KEPT — dropping a row we
/// failed to understand would be a filter that silently loses orphans, which
/// is the worst failure available to this module.
fn row_pid(row: &str) -> Option<u32> {
    // Both platforms now emit `<pid> <ppid> <command line>`, so there is ONE
    // parse rather than two that can drift apart.
    row.split_whitespace().next().and_then(|f| f.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn only_the_three_proven_mechanism_names_are_ever_spelled() {
        // If a fourth name appears here, someone has invented a mechanism that
        // `wcore-sandbox` does not implement.
        let allowed = [
            "LinuxPidNamespaceReap",
            "DockerContainerReap",
            "WindowsJobObject",
        ];
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
                    allowed.contains(&process_tree_mechanism.as_str()),
                    "{kind:?} claims an unrecognised mechanism: {process_tree_mechanism}"
                );
            }
        }
    }

    fn scan(enumerated: bool, found: Vec<String>) -> OrphanScan {
        OrphanScan {
            backend_id: "local".into(),
            kind: BackendKind::Local,
            nonce: "n-1".into(),
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
