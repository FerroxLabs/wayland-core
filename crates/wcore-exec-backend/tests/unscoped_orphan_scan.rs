//! Issue #366 d5: the unscoped orphan scan must FIND a leftover from a run
//! this process never made.
//!
//! The defect this covers is not "the filter is wrong" — the filter was never
//! issued. `scan_orphans(nonce)` is key-EQUALITY, every caller supplies a
//! nonce that is either fresh or read out of the live registry, and a nonce is
//! fresh per run, so no call the product could make was structurally capable
//! of returning a PREVIOUS run's container. The two leftovers in #365 were
//! found by a human running `docker ps -a` by hand.
//!
//! EVERY test here CREATES the condition it tests and removes it again,
//! following `container_wedge.rs` and #365 c5. A test that waits for a dirty
//! host has exactly the blind spot of the thing it replaces: CI's runners are
//! always fresh, so a leftover never exists there by accident.
//!
//! SHARED-DAEMON DISCIPLINE. The daemon these run against may be shared. Every
//! container created here carries a name prefixed `orphan366-` plus this
//! process's pid, and the cleanup removes those names and only those names.
//! Nothing here runs `docker system prune` or a wildcard removal, and no
//! assertion requires the daemon to hold nothing but this test's containers —
//! a co-tenant's labelled container is a legitimate extra row, so every
//! assertion is stated over the rows this test planted rather than over the
//! total.

use wcore_exec_backend::backends::container::{ContainerBackend, NONCE_LABEL};
use wcore_exec_backend::conformance::reference_budget;
use wcore_exec_backend::contract::ExecutionBackend;

fn docker(args: &[&str]) -> std::process::Output {
    std::process::Command::new("docker")
        .args(args)
        .output()
        .expect("the docker client is launchable")
}

/// A real daemon round trip. Socket presence is not readiness — the backend's
/// own availability rule.
fn daemon_answers() -> bool {
    std::process::Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Removes the names it was given when it drops, however the test body ends.
///
/// A plain call at the end of the body is not enough: a failing assertion
/// unwinds past it and leaves the container behind for the next lane, which is
/// the residue this whole issue is about.
struct Planted(Vec<String>);

impl Planted {
    fn plant_labelled(&mut self, name: String, nonce: &str) {
        let _ = docker(&["rm", "-f", &name]);
        let label = format!("{NONCE_LABEL}={nonce}");
        let out = docker(&[
            "create",
            "--name",
            &name,
            "--label",
            &label,
            "--network",
            "none",
            "busybox:1.36",
            "true",
        ]);
        assert!(
            out.status.success(),
            "could not plant {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        self.0.push(name);
    }

    /// A container with NO wayland label — the WRONG-REFUSAL CONTROL. The scan
    /// must not sweep this in: on a shared daemon a key-presence filter that
    /// over-matched would report every tenant's containers as our leftovers,
    /// which is worse than reporting none.
    fn plant_unlabelled(&mut self, name: String) {
        let _ = docker(&["rm", "-f", &name]);
        let out = docker(&[
            "create",
            "--name",
            &name,
            "--network",
            "none",
            "busybox:1.36",
            "true",
        ]);
        assert!(
            out.status.success(),
            "could not plant {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        self.0.push(name);
    }
}

impl Drop for Planted {
    fn drop(&mut self) {
        for name in &self.0 {
            let _ = docker(&["rm", "-f", name]);
        }
    }
}

/// A nonce THIS process has never used and never will: it names the issue, the
/// pid, and a wall-clock stamp. Nothing wrote it to the live-task registry, so
/// `scan_orphans` could not be handed it by any product path.
fn never_used_nonce(tag: &str) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after the epoch")
        .as_nanos();
    format!("orphan366-{tag}-{}-{stamp}", std::process::id())
}

fn backend() -> ContainerBackend {
    ContainerBackend::new(reference_budget()).expect("the container backend constructs")
}

/// THE REGRESSION (#366 d1, d3, d5).
///
/// Plants a labelled container under a nonce this process has never used —
/// exactly the shape of a leftover from yesterday's run — and requires the
/// UNSCOPED scan to report it, flagged as unaccounted-for.
///
/// The control in the same body is the point of the test as much as the
/// finding is: the nonce-SCOPED scan, given a fresh nonce the way every
/// product caller gives it one, must NOT find the same container. That is the
/// defect stated as a measurement rather than as an argument, and it is what
/// makes the finding mean something — a scan that returned everything for
/// every query would satisfy the first assertion alone.
#[tokio::test(flavor = "multi_thread")]
async fn the_unscoped_scan_finds_a_leftover_from_a_nonce_this_process_never_used() {
    if !daemon_answers() {
        eprintln!("skip: no docker daemon answers a version round trip");
        return;
    }
    let backend = backend();
    let mut planted = Planted(Vec::new());

    let leftover_nonce = never_used_nonce("leftover");
    let leftover = format!("orphan366-leftover-{}", std::process::id());
    planted.plant_labelled(leftover.clone(), &leftover_nonce);

    // ── THE FINDING ──────────────────────────────────────────────────────
    let scan = backend
        .scan_all_orphans()
        .await
        .expect("the unscoped scan must answer");
    assert!(
        scan.enumerated,
        "the unscoped scan must really enumerate; reporting zero because it could not look is \
         how a leftover hides. method={}",
        scan.method
    );
    let row = scan
        .found
        .iter()
        .find(|row| row.surface == leftover)
        .unwrap_or_else(|| {
            panic!(
                "the unscoped scan did not report the planted leftover {leftover}. This is the \
                 defect: a container labelled at CREATE time, under a nonce no caller in this \
                 process holds, is invisible to every nonce-scoped path. rows={:?} via {}",
                scan.found, scan.method
            )
        });
    assert_eq!(
        row.nonce, leftover_nonce,
        "the row must carry the nonce it was labelled with, or an operator cannot tell which \
         run left it"
    );
    assert!(
        !row.in_live_registry,
        "#366 d3: a leftover this process did not create must be reported as unaccounted for. \
         A scan that can only report containers the current process already holds a nonce for \
         answers a question nobody needed asked."
    );
    assert!(
        scan.unaccounted().any(|r| r.surface == leftover),
        "the planted leftover must appear in the unaccounted-for rows, which is what the \
         operator surface prints"
    );

    // ── THE CONTROL THAT MAKES THE FINDING MEAN SOMETHING ────────────────
    // Every product caller of `scan_orphans` supplies a nonce that is either
    // fresh for this run or read out of the live registry. Neither can ever
    // be the leftover's. This is that, measured.
    let fresh = never_used_nonce("fresh-run");
    let scoped = backend
        .scan_orphans(&fresh)
        .await
        .expect("the scoped scan must answer");
    assert!(scoped.enumerated, "the scoped scan must really enumerate");
    assert!(
        !scoped.found.contains(&leftover),
        "the SCOPED scan must NOT see the leftover — if it did, this test would be measuring a \
         scan that returns everything for every query rather than the key-presence filter. \
         found={:?}",
        scoped.found
    );

    // And the scoped scan is not simply blind: given the leftover's OWN
    // nonce it finds it. This is the positive control on the query itself,
    // so the empty result above is read as "not this nonce" and never as
    // "the query is broken".
    let targeted = backend
        .scan_orphans(&leftover_nonce)
        .await
        .expect("the scoped scan must answer");
    assert!(
        targeted.found.contains(&leftover),
        "positive control: the scoped filter DOES work when handed the leftover's own nonce. \
         An empty result from a broken query reads exactly like an absent container. found={:?}",
        targeted.found
    );
}

/// WRONG-REFUSAL CONTROL (#366 d1): the key-presence filter must not
/// over-match.
///
/// The fix widens a filter, and a widened filter that sweeps in containers
/// this backend never created is worse than the blindness it replaces: on a
/// shared daemon it would name every tenant's work as our leftovers, and an
/// operator acting on that report would destroy it. Removal is out of scope
/// by decision (#366 d6), so the damage here is a false report rather than a
/// false deletion — which is exactly why the report has to be right.
#[tokio::test(flavor = "multi_thread")]
async fn the_unscoped_scan_ignores_a_container_this_backend_did_not_label() {
    if !daemon_answers() {
        eprintln!("skip: no docker daemon answers a version round trip");
        return;
    }
    let backend = backend();
    let mut planted = Planted(Vec::new());

    let foreign = format!("orphan366-foreign-{}", std::process::id());
    planted.plant_unlabelled(foreign.clone());

    // The positive control lives in the same body: without it, a scan that
    // enumerated NOTHING at all would satisfy the negative assertion below
    // and this test would certify a dead query.
    let ours_nonce = never_used_nonce("ours");
    let ours = format!("orphan366-ours-{}", std::process::id());
    planted.plant_labelled(ours.clone(), &ours_nonce);

    let scan = backend
        .scan_all_orphans()
        .await
        .expect("the unscoped scan must answer");
    assert!(scan.enumerated, "the unscoped scan must really enumerate");
    assert!(
        scan.found.iter().any(|row| row.surface == ours),
        "positive control: the labelled container must be found, or the negative assertion \
         below proves only that the scan is dead. rows={:?}",
        scan.found
    );
    assert!(
        !scan.found.iter().any(|row| row.surface == foreign),
        "a container carrying no {NONCE_LABEL} label was never created by this backend and must \
         NOT be reported as its leftover. rows={:?}",
        scan.found
    );
}
