//! The survivors whose only observable effect is a `tracing` record
//! (wayland-core#449).
//!
//! Four mutants in `store.rs` and one in `runner.rs` change nothing a return
//! value can show: they decide whether an operator WARNING fires, and with what
//! count. The previous grading pass recorded them as unobservable "without a
//! subscriber harness and a dev-dependency". The dev-dependency turns out not
//! to be needed — `tracing` itself exposes the `Subscriber` trait and a
//! thread-local `set_default`, which is all a capture needs — so they are
//! graded here rather than excused.
//!
//! They are worth grading. Both warnings are the only signal an operator gets
//! for a security-relevant decision the runtime has already made silently: "I
//! could not tighten your jobs.json" and "I am withholding every job in this
//! file from unattended execution". A warning that fires on a clean boot trains
//! the operator to ignore it; one that never fires leaves the withholding
//! invisible.
//!
//! | mutant (`crates/wcore-cron/src/`) | killed by |
//! |---|---|
//! | `store.rs:274:49` replace `&` with `\|` / `^` | `a_jobs_file_that_was_successfully_tightened_warns_about_nothing` |
//! | `store.rs:274:57` replace `!=` with `==` | `a_jobs_file_that_was_successfully_tightened_warns_about_nothing` |
//! | `store.rs:430:20` delete `!` | `withholding_untagged_jobs_from_auto_fire_is_announced_once` |
//! | `runner.rs:1118:19` replace `+=` with `*=` | `the_drain_reports_how_many_subscribers_it_actually_fired` |

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use wcore_cron::job::Target;
use wcore_cron::lease::LeaseHandle;
use wcore_cron::runner::{JobHandler, tick_once_at};
use wcore_cron::store::{CronStore, FileCronStore};
use wcore_cron::{CronJob, Trigger};

// ---------------------------------------------------------------------------
// A minimal in-process `tracing` capture.
//
// `tracing::subscriber::set_default` installs per-THREAD, not globally, so two
// of these running concurrently under the test harness cannot see each other's
// records. Every test below is `#[tokio::test]`, which is a current-thread
// runtime, so the code under test runs on the thread that installed the guard.
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct Records {
    warnings: Arc<Mutex<Vec<String>>>,
    fired_counts: Arc<Mutex<Vec<u64>>>,
}

impl Records {
    fn warnings(&self) -> Vec<String> {
        self.warnings.lock().unwrap().clone()
    }
    fn fired_counts(&self) -> Vec<u64> {
        self.fired_counts.lock().unwrap().clone()
    }
}

struct Collector(Records);

impl tracing::Subscriber for Collector {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = Fields::default();
        event.record(&mut visitor);
        if *event.metadata().level() == tracing::Level::WARN {
            self.0
                .warnings
                .lock()
                .unwrap()
                .push(visitor.message.unwrap_or_default());
        }
        if let Some(fired) = visitor.fired {
            self.0.fired_counts.lock().unwrap().push(fired);
        }
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

#[derive(Default)]
struct Fields {
    message: Option<String>,
    fired: Option<u64>,
}

impl tracing::field::Visit for Fields {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "fired" {
            self.fired = Some(value);
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        }
    }
}

/// Installs the capture for the rest of the current scope.
fn capture() -> (Records, tracing::subscriber::DefaultGuard) {
    let records = Records::default();
    let guard = tracing::subscriber::set_default(Collector(records.clone()));
    (records, guard)
}

// ---------------------------------------------------------------------------

fn mk_job(cmd: &str) -> CronJob {
    CronJob::new(
        "0 9 * * *",
        Target::Slash {
            command: cmd.into(),
        },
    )
    .unwrap()
}

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Kills `store.rs:274:49` (`&` -> `|` and `&` -> `^`) and `store.rs:274:57`
/// (`!=` -> `==`).
///
/// `still_loose` is re-read AFTER the self-heal, and its only consumer is the
/// warning. On a file the runtime successfully tightened, the honest answer is
/// "not loose any more" and the operator hears nothing. All three mutants make
/// it answer "still loose" for a file that is now 0600, so a perfectly clean
/// boot emits "could not auto-tighten" about a file it just tightened.
///
/// Asserting the MODE cannot see any of this — the mode is 0600 either way.
/// The record is the only witness, which is precisely why these three outlived
/// the permission tests that kill their neighbours on lines 264 and 265.
#[cfg(unix)]
#[tokio::test]
async fn a_jobs_file_that_was_successfully_tightened_warns_about_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.json");
    let store = FileCronStore::new(path.clone());
    store.insert(mk_job("/a")).await.unwrap();

    chmod(&path, 0o644);
    assert_eq!(
        mode_of(&path),
        0o644,
        "control: this filesystem must honour the chmod"
    );

    let (records, guard) = capture();
    store.list().await.unwrap();
    drop(guard);

    // Control: the self-heal really did run and really did succeed, so
    // "no warning" below means "nothing to warn about" and not "the branch
    // was never entered".
    assert_eq!(
        mode_of(&path),
        0o600,
        "control: the loose file must have been tightened"
    );
    assert_eq!(
        records.warnings(),
        Vec::<String>::new(),
        "a file the runtime tightened successfully must not be reported as one \
         it could not tighten; a warning on every clean boot is a warning the \
         operator learns to ignore"
    );
}

/// The positive control for the capture itself, and the reason every
/// "no warning was emitted" assertion in this file is not vacuous.
///
/// A capture that silently sees nothing satisfies those assertions perfectly.
/// This drives a condition the crate is KNOWN to warn about — an untagged
/// `jobs.json` on the unattended path — through the same capture, and requires
/// the record to arrive.
#[tokio::test]
async fn the_capture_actually_sees_a_warning_from_this_crate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.json");
    let store = FileCronStore::new(path.clone());
    store.insert(mk_job("/a")).await.unwrap();
    strip_integrity(&path);

    let (records, guard) = capture();
    store.list_for_run().await.unwrap();
    drop(guard);

    assert_eq!(
        records.warnings().len(),
        1,
        "control: the capture must observe a warn! this crate is known to emit, \
         or every 'no warning' assertion in this file is vacuous; got {:?}",
        records.warnings()
    );
}

/// Rewrite `jobs.json` with its engine integrity tag removed, which is the
/// shape a legacy or Desktop-app write leaves behind.
fn strip_integrity(path: &Path) {
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let obj = v.as_object_mut().unwrap();
    assert!(
        obj.remove("integrity").is_some(),
        "control: the engine write must have stamped an integrity tag, or this \
         fixture is not testing the untagged path"
    );
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
    #[cfg(unix)]
    chmod(path, 0o600);
}

/// Kills `store.rs:430:20` (`delete !`).
///
/// An untagged `jobs.json` is withheld from unattended execution ENTIRELY —
/// `list_for_run` returns an empty vector whether the file held zero jobs or
/// fifty. The return value therefore cannot distinguish "there was nothing to
/// run" from "there were fifty jobs and none of them will ever fire again",
/// and the warning is the whole of the difference. Dropping the `!` inverts it:
/// silence for the file that is losing fifty jobs, and a warning for the empty
/// file that is losing nothing.
#[tokio::test]
async fn withholding_untagged_jobs_from_auto_fire_is_announced_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.json");
    let store = FileCronStore::new(path.clone());
    store.insert(mk_job("/a")).await.unwrap();
    store.insert(mk_job("/b")).await.unwrap();
    strip_integrity(&path);

    // Control: the jobs are still there and still listed; only the unattended
    // path withholds them.
    assert_eq!(store.list().await.unwrap().len(), 2);

    let (records, guard) = capture();
    let for_run = store.list_for_run().await.unwrap();
    drop(guard);

    assert!(
        for_run.is_empty(),
        "control: untagged jobs must be withheld from auto-fire"
    );
    let warnings = records.warnings();
    assert_eq!(
        warnings.len(),
        1,
        "withholding two real jobs from every future tick must be announced; \
         the empty return value cannot say it happened, got {warnings:?}"
    );

    // The other half of the branch: an empty untagged file loses nothing, so
    // it must stay quiet.
    let empty_dir = tempfile::tempdir().unwrap();
    let empty_path = empty_dir.path().join("jobs.json");
    std::fs::write(&empty_path, br#"{"jobs":[]}"#).unwrap();
    #[cfg(unix)]
    chmod(&empty_path, 0o600);
    let empty_store = FileCronStore::new(empty_path);

    let (records, guard) = capture();
    assert!(empty_store.list_for_run().await.unwrap().is_empty());
    drop(guard);
    assert_eq!(
        records.warnings(),
        Vec::<String>::new(),
        "an untagged file with no jobs withholds nothing and must stay quiet"
    );
}

// ---------------------------------------------------------------------------

struct Silent;

#[async_trait]
impl JobHandler for Silent {
    async fn dispatch(&self, _target: &Target) -> wcore_cron::Result<()> {
        Ok(())
    }
}

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 28, 9, 0, 0).unwrap()
}

fn cron_dir(root: &Path) -> PathBuf {
    root.join("cron")
}

/// Kills `runner.rs:1118:19` (`+=` -> `*=`).
///
/// `fired` is a `usize` starting at 0, so `*=` pins it at 0 forever while the
/// dispatches still happen. Nothing returns it — `drain_published_events`
/// returns `()` — so the count reaches the operator through exactly one path,
/// the `fired` field of the drain record, and that record is how a fan-out that
/// silently stopped reaching half its subscribers is noticed at all. A counter
/// that always reads zero reports total failure during normal operation, which
/// is worse than not reporting.
#[tokio::test]
async fn the_drain_reports_how_many_subscribers_it_actually_fired() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn CronStore> =
        Arc::new(FileCronStore::new(cron_dir(dir.path()).join("jobs.json")));
    let handler: Arc<dyn JobHandler> = Arc::new(Silent);

    for cmd in ["/a", "/b", "/c"] {
        let mut job = CronJob::with_trigger(
            Trigger::Event {
                topic: "fan.out".into(),
            },
            Target::Slash {
                command: cmd.into(),
            },
        )
        .unwrap();
        job.created_at = t0() - Duration::hours(1);
        store.insert(job).await.unwrap();
    }

    wcore_cron::publish_event(cron_dir(dir.path()), "fan.out", t0()).unwrap();

    let (records, guard) = capture();
    tick_once_at(
        &store,
        &handler,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(30),
    )
    .await
    .unwrap();
    drop(guard);

    assert_eq!(
        records.fired_counts(),
        vec![3],
        "the drain record must report the number of subscribers it actually \
         dispatched; it is the only place that number is ever observable"
    );
}
