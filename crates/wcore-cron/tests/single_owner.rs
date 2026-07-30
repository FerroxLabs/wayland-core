//! Single-owner scheduling: exactly one process fires, every other observes.
//!
//! Phase 24 plan 24-02, Task 1.
//!
//! Every case here drives an INJECTED instant rather than sleeping to a
//! boundary, so the whole file is deterministic. The one thing that is
//! deliberately real is the exclusion primitive: both runners take the lease
//! through the actual OS lock, inside one test process. That is what makes
//! the first case capable of going red — a primitive owned by the PROCESS
//! (`fcntl`) rather than by the open file description would admit the second
//! claim and the assertion would silently pass on a broken lease.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, TimeZone, Utc};
use wcore_cron::job::{CronFireOutcome, CronFireRecord, Target};
use wcore_cron::lease::{LeaseAttempt, LeaseHandle, LeaseRole, ScheduleLease};
use wcore_cron::runner::JobHandler;
use wcore_cron::store::{CronStore, FileCronStore};
use wcore_cron::{CronError, CronJob, Result, tick_once_at};

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Counts dispatches. The count is the fire tally every case asserts on.
#[derive(Default, Clone)]
struct Counting {
    seen: Arc<tokio::sync::Mutex<Vec<Target>>>,
}

impl Counting {
    async fn count(&self) -> usize {
        self.seen.lock().await.len()
    }
}

#[async_trait]
impl JobHandler for Counting {
    async fn dispatch(&self, target: &Target) -> Result<()> {
        self.seen.lock().await.push(target.clone());
        Ok(())
    }
}

/// A dispatcher that is not live — the shape a slash target hits in a process
/// with no cross-session dispatcher wired.
struct NoLiveDispatcher;

#[async_trait]
impl JobHandler for NoLiveDispatcher {
    async fn dispatch(&self, _t: &Target) -> Result<()> {
        Err(CronError::NoDispatcher)
    }
}

/// Surrenders the schedule from INSIDE a dispatch, which is the only way to
/// reach the between-selection-and-dispatch window deterministically.
#[derive(Clone)]
struct RevokesAfterFirst {
    lease: LeaseHandle,
    seen: Arc<tokio::sync::Mutex<Vec<Target>>>,
}

#[async_trait]
impl JobHandler for RevokesAfterFirst {
    async fn dispatch(&self, target: &Target) -> Result<()> {
        self.seen.lock().await.push(target.clone());
        // The gateway has entered drain. Everything selected but not yet
        // dispatched in this same tick now belongs to nobody.
        self.lease.revoke();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn store_in(dir: &std::path::Path) -> Arc<dyn CronStore> {
    Arc::new(FileCronStore::new(dir.join("jobs.json")))
}

/// A job that is unambiguously due at `now`: the anchor sits two days back and
/// the expression fires daily, so a next-fire strictly after the anchor is
/// always in the past relative to `now`.
fn due_job(cmd: &str, now: chrono::DateTime<Utc>) -> CronJob {
    let mut j = CronJob::new(
        "0 9 * * *",
        Target::Slash {
            command: cmd.to_string(),
        },
    )
    .expect("fixture expression must parse");
    j.created_at = now - ChronoDuration::days(2);
    j
}

fn history_records(path: &PathBuf) -> Vec<CronFireRecord> {
    match std::fs::read_to_string(path) {
        Ok(s) => s
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("every history line must parse"))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn fixed_now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 27, 12, 0, 0)
        .single()
        .expect("fixed instant is unambiguous")
}

// ---------------------------------------------------------------------------
// 1. Exactly one fire, two runners, one store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_runners_against_one_store_fire_a_due_job_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let history = dir.path().join("history.jsonl");
    let now = fixed_now();

    store.insert(due_job("/morning", now)).await.unwrap();

    // Runner A claims the schedule. Runner B attempts the SAME directory and
    // is demoted. Both attempts go through the real OS lock, from inside one
    // process — the case a process-owned lock primitive would wrongly admit.
    let a = ScheduleLease::attempt(dir.path(), "runner-a").unwrap();
    assert!(a.is_owner(), "the first claim must win the schedule");
    let b = ScheduleLease::attempt(dir.path(), "runner-b").unwrap();
    assert_eq!(
        b.role(),
        LeaseRole::Observer,
        "the second claim must be demoted, not admitted"
    );
    match &b {
        LeaseAttempt::Observer { holder_pid } => assert_eq!(
            *holder_pid,
            Some(std::process::id()),
            "an observer must be able to name the owner"
        ),
        LeaseAttempt::Owner(_) => unreachable!(),
    }

    let a_lease = a.into_lease().unwrap();
    let owner_handle = a_lease.handle();
    let observer_handle = LeaseHandle::observer();

    let handler_a = Counting::default();
    let handler_b = Counting::default();
    let arc_a: Arc<dyn JobHandler> = Arc::new(handler_a.clone());
    let arc_b: Arc<dyn JobHandler> = Arc::new(handler_b.clone());

    // Both tick against the same store at the same instant, and the OBSERVER
    // ticks FIRST. The order is load-bearing and was chosen by measurement:
    // with the owner ticking first, its fire advances `last_fired`, the job
    // stops being due, and the observer then fires nothing whether or not the
    // lease is consulted at all — the assertion would pass against a runtime
    // with the whole ownership check deleted. Ticking the observer against a
    // still-due job is what makes this gate capable of going red.
    tick_once_at(&store, &arc_b, Some(&history), &observer_handle, now)
        .await
        .unwrap();
    tick_once_at(&store, &arc_a, Some(&history), &owner_handle, now)
        .await
        .unwrap();

    assert_eq!(handler_a.count().await, 1, "the owner fires the due job");
    assert_eq!(handler_b.count().await, 0, "an observer fires nothing");

    let recs = history_records(&history);
    assert_eq!(
        recs.len(),
        1,
        "exactly one history record for one due job, got {recs:?}"
    );
    assert!(matches!(recs[0].outcome, CronFireOutcome::Success { .. }));
}

#[tokio::test]
async fn an_observer_leaves_the_store_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let history = dir.path().join("history.jsonl");
    let now = fixed_now();

    let job = due_job("/observed", now);
    store.insert(job.clone()).await.unwrap();

    let handler = Counting::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());
    tick_once_at(&store, &arc, Some(&history), &LeaseHandle::observer(), now)
        .await
        .unwrap();

    assert_eq!(handler.count().await, 0);
    let listed = store.list().await.unwrap();
    let after = listed.iter().find(|j| j.id == job.id).unwrap();
    assert!(
        after.last_fired.is_none(),
        "an observer must not advance last_fired"
    );
    assert!(
        after.last_result.is_none(),
        "an observer must not write a fire result"
    );
    assert!(
        history_records(&history).is_empty(),
        "an observer must not write history"
    );
}

// ---------------------------------------------------------------------------
// 2. Reclamation requires proof of death, not a timestamp
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_live_holders_schedule_is_not_reclaimable_however_old_the_record_looks() {
    let dir = tempfile::tempdir().unwrap();
    let held = ScheduleLease::attempt(dir.path(), "live-owner")
        .unwrap()
        .into_lease()
        .unwrap();

    // Backdate the readable record by a year. A reclamation rule built on a
    // timestamp comparison would hand the schedule to the challenger here and
    // produce the exact double-fire the lease exists to prevent.
    let record_path = ScheduleLease::record_path(dir.path());
    let mut rec: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    rec["acquired_at"] = serde_json::json!("2025-01-01T00:00:00+00:00");
    std::fs::write(&record_path, serde_json::to_vec_pretty(&rec).unwrap()).unwrap();

    let challenger = ScheduleLease::attempt(dir.path(), "challenger").unwrap();
    assert_eq!(
        challenger.role(),
        LeaseRole::Observer,
        "a live owner keeps its schedule no matter how stale its record looks"
    );

    drop(held);
}

#[tokio::test]
async fn a_dead_holders_schedule_is_reclaimable_because_the_os_released_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    {
        let _dead = ScheduleLease::attempt(dir.path(), "about-to-die")
            .unwrap()
            .into_lease()
            .unwrap();
        // Scope exit stands in for process death: the OS releases the claim
        // when the holding descriptor closes, which is the same thing that
        // happens on SIGKILL, on a panic, and after a power loss.
    }

    let next = ScheduleLease::attempt(dir.path(), "successor").unwrap();
    assert!(
        next.is_owner(),
        "a schedule whose owner is gone must be reclaimable without a timeout"
    );
}

#[tokio::test]
async fn a_stale_record_alone_never_grants_ownership() {
    // A record naming a plausible pid, with NO lock behind it, is exactly what
    // an attacker (or a crashed process) leaves. The successor must win on the
    // lock, and the presence of the record must not change the outcome either
    // way.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(
        ScheduleLease::record_path(dir.path()),
        serde_json::to_vec_pretty(&serde_json::json!({
            "pid": 999_999u32,
            "acquired_at": "2026-07-27T00:00:00+00:00",
            "holder": "ghost",
        }))
        .unwrap(),
    )
    .unwrap();

    let attempt = ScheduleLease::attempt(dir.path(), "successor").unwrap();
    assert!(
        attempt.is_owner(),
        "an unbacked record must not block a real claim"
    );
    let rec = ScheduleLease::read_record(dir.path()).unwrap();
    assert_eq!(
        rec.pid,
        std::process::id(),
        "the winner must overwrite the ghost record"
    );
}

// ---------------------------------------------------------------------------
// 3. A lease lost mid-tick abandons the selected fire, with a record
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_lease_lost_mid_tick_abandons_the_selected_fire_and_records_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let history = dir.path().join("history.jsonl");
    let now = fixed_now();

    let first = due_job("/first", now);
    let second = due_job("/second", now);
    store.insert(first.clone()).await.unwrap();
    store.insert(second.clone()).await.unwrap();

    let lease = ScheduleLease::attempt(dir.path(), "draining-gateway")
        .unwrap()
        .into_lease()
        .unwrap();
    let handle = lease.handle();

    let handler = RevokesAfterFirst {
        lease: handle.clone(),
        seen: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    tick_once_at(&store, &arc, Some(&history), &handle, now)
        .await
        .unwrap();

    assert_eq!(
        handler.seen.lock().await.len(),
        1,
        "only the fire already in flight completes; the next must not dispatch"
    );

    let recs = history_records(&history);
    assert_eq!(
        recs.len(),
        2,
        "both the fire and the abandonment are recorded"
    );
    assert!(matches!(recs[0].outcome, CronFireOutcome::Success { .. }));
    match &recs[1].outcome {
        CronFireOutcome::Abandoned { reason } => {
            assert!(
                reason.contains("lease"),
                "the abandonment must name why, got {reason:?}"
            );
        }
        other => panic!("expected an Abandoned record, got {other:?}"),
    }
    assert_eq!(recs[1].job_id, second.id);

    // The abandoned job did NOT run, so the incoming owner must still see it
    // as due: last_fired stays unset.
    let listed = store.list().await.unwrap();
    let after = listed.iter().find(|j| j.id == second.id).unwrap();
    assert!(
        after.last_fired.is_none(),
        "an abandoned fire must not advance last_fired — the new owner still owes it"
    );
    assert!(matches!(
        after.last_result,
        Some(CronFireOutcome::Abandoned { .. })
    ));
}

// ---------------------------------------------------------------------------
// 4. The staged-fire hole: honest without a live dispatcher, closed with one
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_slash_target_still_stages_honestly_when_no_dispatcher_is_live() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let history = dir.path().join("history.jsonl");
    let now = fixed_now();

    let job = due_job("/brief", now);
    store.insert(job.clone()).await.unwrap();

    let arc: Arc<dyn JobHandler> = Arc::new(NoLiveDispatcher);
    tick_once_at(&store, &arc, Some(&history), &LeaseHandle::unleased(), now)
        .await
        .unwrap();

    let recs = history_records(&history);
    assert_eq!(recs.len(), 1);
    assert_eq!(
        recs[0].outcome,
        CronFireOutcome::Staged,
        "with nothing live to dispatch to, Staged is the honest outcome and must be preserved"
    );
}

#[tokio::test]
async fn a_slash_target_stops_staging_once_a_live_dispatcher_is_wired() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let history = dir.path().join("history.jsonl");
    let now = fixed_now();

    let job = due_job("/brief", now);
    store.insert(job.clone()).await.unwrap();

    // The gateway supplies a live cross-session dispatcher. Same target, same
    // schedule, different sink — and the staged outcome disappears.
    let handler = Counting::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());
    let lease = ScheduleLease::attempt(dir.path(), "gateway")
        .unwrap()
        .into_lease()
        .unwrap();

    tick_once_at(&store, &arc, Some(&history), &lease.handle(), now)
        .await
        .unwrap();

    assert_eq!(handler.count().await, 1);
    let recs = history_records(&history);
    assert_eq!(recs.len(), 1);
    assert!(
        matches!(recs[0].outcome, CronFireOutcome::Success { .. }),
        "a live dispatcher turns the staged hole into a real fire, got {:?}",
        recs[0].outcome
    );
}

// ---------------------------------------------------------------------------
// 5. Cross-PROCESS single ownership — measured, not inferred
//
// Every case above drives two attempts inside ONE test process. That is the
// right shape for catching a primitive owned by the PROCESS (`fcntl`), and
// this file's own header says so. It is NOT the production shape: the thing
// this lease exists to prevent is a second `wayland-core` PROCESS.
//
// The gap matters most on Windows. There the exclusion is `LockFileEx`, whose
// byte-range locks are MANDATORY rather than advisory, and until this case
// nothing in this workspace had taken the lease across a real process
// boundary on that platform.
//
// The child is this same test binary re-executed with `--exact`, so no new
// crate and no `Cargo.lock` edit is required.
// ---------------------------------------------------------------------------

/// Env var naming the schedule directory a child worker should claim.
const CHILD_DIR_ENV: &str = "WL_CRON_LEASE_CHILD_DIR";
/// Env var naming the file the child writes its verdict to.
const CHILD_OUT_ENV: &str = "WL_CRON_LEASE_CHILD_OUT";

/// The child half of [`a_second_process_is_refused_while_the_first_holds_it`].
///
/// In CHILD mode it attempts the lease, publishes its verdict, and holds until
/// released. In ORDINARY mode it is not a no-op: it asserts the take/release
/// round trip. An env-gated early `return` that reports `ok` for zero work is a
/// measured self-passing shape in this repo (`LANE-BRIEF.md` §3.2 flavour (b)),
/// so this arm deliberately does real work instead.
#[test]
fn cross_process_lease_worker() {
    let Ok(dir) = std::env::var(CHILD_DIR_ENV) else {
        // ORDINARY mode. Real assertions, so this can never be a silent pass.
        let tmp = tempfile::tempdir().unwrap();
        let held = ScheduleLease::attempt(tmp.path(), "worker-selftest").unwrap();
        assert!(held.is_owner(), "an uncontended schedule must be claimable");
        drop(held);
        let again = ScheduleLease::attempt(tmp.path(), "worker-selftest-2").unwrap();
        assert!(again.is_owner(), "a released schedule must be reclaimable");
        return;
    };

    let out = PathBuf::from(std::env::var(CHILD_OUT_ENV).expect("child needs an output path"));
    let attempt =
        ScheduleLease::attempt(&dir, "child").expect("child lease attempt must not error");
    let verdict = match &attempt {
        LeaseAttempt::Owner(_) => format!("ROLE=OWNER\nPID={}\n", std::process::id()),
        LeaseAttempt::Observer { holder_pid } => format!(
            "ROLE=OBSERVER\nPID={}\nHOLDER={}\n",
            std::process::id(),
            holder_pid.map_or_else(|| "none".to_owned(), |p| p.to_string())
        ),
    };
    // START marker first: a participant that never launched is a dead
    // instrument, and the parent asserts on this before drawing any conclusion.
    std::fs::write(&out, format!("STARTED\n{verdict}")).unwrap();

    // Hold until the parent says stop, so the parent can contend against a
    // genuinely live holder rather than against a race with our own exit.
    let release = PathBuf::from(format!("{}.release", out.display()));
    for _ in 0..600 {
        if release.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    drop(attempt);
    std::fs::write(format!("{}.exited", out.display()), b"1").unwrap();
}

/// Spawn a child worker against `dir`, and wait for it to publish a verdict.
fn spawn_child(dir: &std::path::Path, out: &std::path::Path) -> std::process::Child {
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "cross_process_lease_worker", "--nocapture"])
        .env(CHILD_DIR_ENV, dir)
        .env(CHILD_OUT_ENV, out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("re-executing the test binary as a child must succeed");
    for _ in 0..300 {
        if let Ok(text) = std::fs::read_to_string(out) {
            if text.starts_with("STARTED") {
                return child;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!(
        "child never reached its START marker at {} — the participant did not launch, \
         so nothing was contended",
        out.display()
    );
}

fn child_field(out: &std::path::Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(out).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")).map(str::to_owned))
}

/// Two real processes, one schedule directory.
///
/// This case runs its control in BOTH directions inside one execution:
///
/// - **can it fail** — while A holds the lease, B must be refused. A primitive
///   that never actually locked (an always-`Ok(true)` stub, a lock taken on the
///   wrong handle, a path the two children did not share) makes B an OWNER and
///   reddens this assertion.
/// - **can it pass** — after A releases, C must win. A primitive that is stuck
///   locked, or a lease that leaks its lock past the holder's death, makes C an
///   OBSERVER and reddens that assertion.
///
/// A permanently-red gate proves as little as a permanently-green one
/// (`LANE-BRIEF.md` §3b-iii), so both arms are asserted, not just the refusal.
#[test]
fn a_second_process_is_refused_while_the_first_holds_it() {
    let tmp = tempfile::tempdir().unwrap();
    let sched = tmp.path().join("schedule");
    std::fs::create_dir_all(&sched).unwrap();

    let out_a = tmp.path().join("a.txt");
    let out_b = tmp.path().join("b.txt");
    let out_c = tmp.path().join("c.txt");

    // --- A takes the lease in its own process.
    let mut a = spawn_child(&sched, &out_a);
    assert_eq!(
        child_field(&out_a, "ROLE").as_deref(),
        Some("OWNER"),
        "the first process must own an uncontended schedule"
    );
    let a_pid: u32 = child_field(&out_a, "PID").unwrap().parse().unwrap();
    assert_ne!(
        a_pid,
        std::process::id(),
        "the child must be a genuinely separate process, otherwise this case \
         measures the same in-process exclusion the rest of the file already does"
    );

    // --- KNOWN-NEGATIVE DIRECTION: B contends against a live holder.
    let mut b = spawn_child(&sched, &out_b);
    assert_eq!(
        child_field(&out_b, "ROLE").as_deref(),
        Some("OBSERVER"),
        "a second PROCESS must be refused while the first holds the OS lock"
    );
    assert_eq!(
        child_field(&out_b, "HOLDER").as_deref(),
        Some(a_pid.to_string().as_str()),
        "the refusal must name the live owner, read from the unlocked record"
    );
    std::fs::write(format!("{}.release", out_b.display()), b"1").unwrap();
    b.wait().unwrap();

    // --- KNOWN-POSITIVE DIRECTION: A goes away, so C must win.
    std::fs::write(format!("{}.release", out_a.display()), b"1").unwrap();
    a.wait().unwrap();
    let mut c = spawn_child(&sched, &out_c);
    assert_eq!(
        child_field(&out_c, "ROLE").as_deref(),
        Some("OWNER"),
        "a schedule whose owning PROCESS has exited must be reclaimable — a gate \
         with no reachable pass state measures nothing"
    );
    std::fs::write(format!("{}.release", out_c.display()), b"1").unwrap();
    c.wait().unwrap();
}

/// The mandatory-locking hazard, asserted across a process boundary.
///
/// On Windows `LockFileEx` is MANDATORY: bytes covered by the lock cannot be
/// read by anybody, including this crate. That is why the lease splits the
/// one-byte `schedule.lock` sentinel from the freely readable `schedule.owner`
/// record. If the lock were ever moved onto the record, this read would fail
/// with `ERROR_LOCK_VIOLATION` (33) on Windows while continuing to pass on
/// Unix, where `flock` is advisory — a platform-only defect that no Linux run
/// can see.
#[test]
fn the_owner_record_is_readable_from_another_process_while_the_lock_is_held() {
    let tmp = tempfile::tempdir().unwrap();
    let sched = tmp.path().join("schedule");
    std::fs::create_dir_all(&sched).unwrap();
    let out_a = tmp.path().join("holder.txt");

    let mut a = spawn_child(&sched, &out_a);
    assert_eq!(child_field(&out_a, "ROLE").as_deref(), Some("OWNER"));

    // This process is NOT the holder, so the read crosses a real handle
    // boundary rather than reusing the holder's own descriptor.
    let raw = std::fs::read(ScheduleLease::record_path(&sched)).expect(
        "the owner record must be readable by another process while the lease is held; \
         a failure here is the mandatory-lock defect the split sentinel exists to avoid",
    );
    assert!(!raw.is_empty(), "the owner record must not be empty");
    let rec = ScheduleLease::read_record(&sched).expect("the owner record must parse");
    assert_eq!(rec.holder, "child");
    assert_ne!(
        rec.pid,
        std::process::id(),
        "the record must name the child, not this process"
    );

    // The sentinel stays one byte, so the mandatory range covers nothing a
    // reader wants.
    let len = std::fs::metadata(ScheduleLease::lock_path(&sched))
        .unwrap()
        .len();
    assert_eq!(len, 1, "lock sentinel must stay exactly one byte");

    std::fs::write(format!("{}.release", out_a.display()), b"1").unwrap();
    a.wait().unwrap();
}
