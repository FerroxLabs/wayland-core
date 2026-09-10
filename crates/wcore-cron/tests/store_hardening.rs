//! Grading tests for the `store.rs` mutants that survived the 2026-09-04
//! `mutants-nightly` run on `wcore-cron` (wayland-core#449).
//!
//! Every test here exists to fail against a NAMED surviving mutant, and each
//! one says which. The cluster is the file-permission and tamper-detection
//! half of the store: the survivors include replacing
//! `FileCronStore::check_ownership_and_perms` outright with `Ok(())` and
//! returning an EMPTY integrity key, i.e. exactly the two checks that decide
//! whether an unattended runner will execute a job somebody else authored.
//!
//! Mutants targeted from this file (line numbers as reported by the run, in
//! `crates/wcore-cron/src/`):
//!
//! | mutant | killed by |
//! |---|---|
//! | `store.rs:246:9` replace `check_ownership_and_perms` with `Ok(())` | `a_group_readable_jobs_file_is_tightened_on_load` |
//! | `store.rs:145:5` replace `set_owner_only_perms` with `()` | `a_group_readable_jobs_file_is_tightened_on_load` |
//! | `store.rs:265:25` replace `!=` with `==` | both perm tests |
//! | `store.rs:265:17` replace `&` with `\|` / `^` | `an_already_tight_jobs_file_is_left_exactly_as_it_is` |
//! | `store.rs:264:46` replace `&` with `\|` / `^` | `an_already_tight_jobs_file_is_left_exactly_as_it_is` |
//! | `store.rs:219:9` replace `integrity_key` with `Some(vec![])` / `Some(vec![0])` / `Some(vec![1])` | `the_integrity_key_is_per_directory_and_persisted_owner_only` |
//! | `store.rs:35:5` replace `default_store_path` with `None` / `Some(Default::default())` | `the_default_paths_are_real_paths_under_the_wayland_home` |
//! | `store.rs:44:5` replace `default_history_path` with `None` / `Some(Default::default())` | `the_default_paths_are_real_paths_under_the_wayland_home` |
//! | `store.rs:68:9` replace `CronStore::list_for_run` with `Ok(vec![])` | `a_store_without_a_file_runs_everything_it_lists` |
//! | `store.rs:79:9` replace `CronStore::cron_dir` with `Some(Default::default())` | `a_store_without_a_file_runs_everything_it_lists` |
//! | `events.rs:202:5` replace `restrict_permissions` with `()` | `a_published_event_queue_is_tightened_to_owner_only` |
//! | `retry.rs:71:25` replace `*` with `+` | `a_backoff_ceiling_clamps_to_a_full_day` |
//!
//! Three `check_ownership_and_perms` survivors are NOT graded here and are not
//! claimed as killed: `store.rs:274:49` (`&` -> `|`, `^`) and `store.rs:274:57`
//! (`!=` -> `==`) sit inside the `still_loose` computation, whose only effect
//! is whether a `tracing::warn!` fires. Nothing in this crate captures tracing
//! output, so an in-tree test cannot observe them without a subscriber
//! harness. `store.rs:430:20` (`delete !`) is the same class. The uid branch
//! (`meta.uid() != uid`) needs a file owned by a second user, which a
//! single-uid test process cannot create.

use std::path::Path;

use async_trait::async_trait;
use wcore_cron::job::Target;
use wcore_cron::store::{CronStore, FileCronStore, default_history_path, default_store_path};
use wcore_cron::{CronJob, Result, RetryPolicy};

fn mk_job(expr: &str, cmd: &str) -> CronJob {
    CronJob::new(
        expr,
        Target::Slash {
            command: cmd.into(),
        },
    )
    .unwrap()
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

// ----- the ownership / permission gate -----

/// Kills `store.rs:246:9` (whole gate -> `Ok(())`), `store.rs:145:5`
/// (`set_owner_only_perms` -> `()`) and `store.rs:265:25` (`!=` -> `==`).
///
/// The gate's one observable effect on a same-uid file is the self-heal: a
/// jobs.json left group/other-accessible by a legacy or Desktop-app write is
/// tightened to 0600 in place on the next load. A gate that returns `Ok(())`
/// without looking, a `set_owner_only_perms` that does nothing, and an
/// inverted loose-mode test all leave the file exactly as loose as it was.
#[cfg(unix)]
#[tokio::test]
async fn a_group_readable_jobs_file_is_tightened_on_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.json");
    let store = FileCronStore::new(path.clone());
    store.insert(mk_job("0 9 * * *", "/a")).await.unwrap();

    // Loosen it the way a pre-hardening writer would have left it.
    chmod(&path, 0o644);
    assert_eq!(
        mode_of(&path),
        0o644,
        "control: this filesystem must honour the chmod, or the assertion below \
         proves nothing"
    );

    // A load runs the gate.
    store.list().await.unwrap();

    assert_eq!(
        mode_of(&path),
        0o600,
        "a group/other-accessible jobs.json must be tightened to 0600 on load; \
         it can carry secrets-in-prompts and anyone who can write it can make \
         the unattended runner execute their job"
    );
}

/// Kills `store.rs:264:46` (`&` -> `|` and `&` -> `^`) and `store.rs:265:17`
/// (`&` -> `|` and `&` -> `^`), and independently `store.rs:265:25`
/// (`!=` -> `==`).
///
/// 0400 rather than 0600 is the whole point: every one of those five mutants
/// makes the loose-mode test true for a file that is already tight, and the
/// branch it then wrongly enters calls `set_owner_only_perms`, which writes
/// 0600. So the mode is a witness for "the gate touched a file it had no
/// business touching" — which a 0600 fixture could never show, because there
/// the spurious tighten is a no-op.
#[cfg(unix)]
#[tokio::test]
async fn an_already_tight_jobs_file_is_left_exactly_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.json");
    let store = FileCronStore::new(path.clone());
    store.insert(mk_job("0 9 * * *", "/a")).await.unwrap();

    chmod(&path, 0o400);
    assert_eq!(
        mode_of(&path),
        0o400,
        "control: this filesystem must honour the chmod"
    );

    store.list().await.unwrap();

    assert_eq!(
        mode_of(&path),
        0o400,
        "a file with no group/other bits is already at or below the posture the \
         gate enforces; re-writing its mode means the loose-mode test fired on \
         a tight file"
    );
}

// ----- the integrity key -----

/// Kills all three `store.rs:219:9` survivors: `integrity_key` replaced with
/// `Some(vec![])`, `Some(vec![0])` and `Some(vec![1])`.
///
/// A constant key is a forgeable tag: the MAC exists so that a writer must
/// also possess the host's 0600 key, and a key baked into the binary is a key
/// every tamperer already has. Two arms make that observable without reaching
/// into private state:
///
/// 1. the key file must actually be created, be 32 bytes, and be 0600 — a
///    constant-returning `integrity_key` never writes one;
/// 2. the same job content stamped in two different directories must carry
///    DIFFERENT tags, because the keys differ. A constant key makes them
///    equal.
///
/// The stability assertion is the control for (2): without it, per-write
/// random tags would satisfy `assert_ne!` for the wrong reason.
#[tokio::test]
async fn the_integrity_key_is_per_directory_and_persisted_owner_only() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let path_a = dir_a.path().join("jobs.json");
    let path_b = dir_b.path().join("jobs.json");
    let store_a = FileCronStore::new(path_a.clone());
    let store_b = FileCronStore::new(path_b.clone());

    // The SAME job value in both stores, so the canonical payload the tag is
    // taken over is byte-identical and only the key can differ.
    let job = mk_job("0 9 * * *", "/a");
    store_a.insert(job.clone()).await.unwrap();
    store_b.insert(job.clone()).await.unwrap();

    let key_a_path = dir_a.path().join(".integrity.key");
    let key_b_path = dir_b.path().join(".integrity.key");
    let key_a = std::fs::read(&key_a_path).expect("an engine write must create the host key file");
    let key_b = std::fs::read(&key_b_path).expect("an engine write must create the host key file");
    assert_eq!(key_a.len(), 32, "the host key must be 32 bytes of entropy");
    assert_ne!(
        key_a, key_b,
        "two installations must not share a key; a shared key is a forgeable tag"
    );
    #[cfg(unix)]
    assert_eq!(
        mode_of(&key_a_path),
        0o600,
        "the host key must not be readable by anyone but its owner"
    );

    let tag_of = |p: &Path| -> String {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        v["integrity"]
            .as_str()
            .expect("an engine write must stamp an integrity tag")
            .to_string()
    };
    let tag_a = tag_of(&path_a);
    let tag_b = tag_of(&path_b);

    // Control: the tag is a function of (key, content), not of the moment of
    // writing. Without this, `assert_ne!` below would pass for a tag that was
    // simply random each time.
    store_a.insert(job.clone()).await.unwrap();
    assert_eq!(
        tag_a,
        tag_of(&path_a),
        "re-stamping identical content under the same key must reproduce the tag"
    );

    assert_ne!(
        tag_a, tag_b,
        "identical job content under two different host keys must not produce \
         the same tag; if it does the key is a constant and the tag is forgeable"
    );
}

// ----- the default paths -----

/// Kills `store.rs:35:5` and `store.rs:44:5` — both `-> None` and both
/// `-> Some(Default::default())` (an empty `PathBuf`).
#[test]
fn the_default_paths_are_real_paths_under_the_wayland_home() {
    // Control: this environment can resolve a home at all. Without it a `None`
    // return would be indistinguishable from a machine with no `$HOME`.
    assert!(
        std::env::var_os("WAYLAND_HOME").is_some() || dirs::home_dir().is_some(),
        "control: the test environment must resolve WAYLAND_HOME or $HOME"
    );

    let store = default_store_path().expect("the store path must resolve when a home exists");
    let history = default_history_path().expect("the history path must resolve when a home exists");

    assert!(
        store.ends_with("cron/jobs.json"),
        "expected a cron/jobs.json path, got {}",
        store.display()
    );
    assert!(
        history.ends_with("cron/history.jsonl"),
        "expected a cron/history.jsonl path, got {}",
        history.display()
    );
    assert_eq!(
        store.parent(),
        history.parent(),
        "the job set and its history must share one cron directory"
    );
}

// ----- the trait defaults, which only a non-file store reaches -----

#[derive(Default)]
struct MemStore {
    jobs: std::sync::Mutex<Vec<CronJob>>,
}

#[async_trait]
impl CronStore for MemStore {
    async fn list(&self) -> Result<Vec<CronJob>> {
        Ok(self.jobs.lock().unwrap().clone())
    }
    async fn insert(&self, job: CronJob) -> Result<()> {
        self.jobs.lock().unwrap().push(job);
        Ok(())
    }
    async fn update(&self, _job: CronJob) -> Result<()> {
        Ok(())
    }
    async fn remove(&self, _id: &str) -> Result<()> {
        Ok(())
    }
    async fn set_enabled(&self, _id: &str, _enabled: bool) -> Result<()> {
        Ok(())
    }
}

/// Kills `store.rs:68:9` (`CronStore::list_for_run` default -> `Ok(vec![])`)
/// and `store.rs:79:9` (`CronStore::cron_dir` default ->
/// `Some(Default::default())`).
///
/// `FileCronStore` overrides both, so only a store with no filesystem home
/// reaches the defaults — and the defaults are load-bearing in opposite
/// directions. `list_for_run` must delegate to `list`: a store with no file
/// has nothing to tamper with, and silently returning nothing would stop an
/// in-memory or future store from ever firing a job. `cron_dir` must stay
/// `None`: an empty `PathBuf` would name the process's working directory as
/// the event queue's home.
#[tokio::test]
async fn a_store_without_a_file_runs_everything_it_lists() {
    let store = MemStore::default();
    let job = mk_job("0 9 * * *", "/a");
    store.insert(job.clone()).await.unwrap();

    let listed = store.list().await.unwrap();
    assert_eq!(listed.len(), 1, "control: the job was stored");

    let runnable = store.list_for_run().await.unwrap();
    assert_eq!(
        runnable.len(),
        1,
        "a store with no file to tamper with must run what it lists"
    );
    assert_eq!(runnable[0].id, job.id);

    assert_eq!(
        store.cron_dir(),
        None,
        "a store with no filesystem home must not name one; an empty path is \
         the working directory"
    );
}

// ----- neighbours of the same class -----

/// Kills `events.rs:202:5` (`restrict_permissions` -> `()`).
///
/// Same authority as `jobs.json`: anyone who can write the queue can fire any
/// event-triggered job. Pre-loosening the directory makes the assertion
/// independent of the test process's umask.
#[cfg(unix)]
#[test]
fn a_published_event_queue_is_tightened_to_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let queue = wcore_cron::events_dir(dir.path());
    std::fs::create_dir_all(&queue).unwrap();
    chmod(&queue, 0o777);
    assert_eq!(
        mode_of(&queue),
        0o777,
        "control: this filesystem must honour the chmod"
    );

    wcore_cron::publish_event(dir.path(), "deploy.finished", chrono::Utc::now()).unwrap();

    assert_eq!(
        mode_of(&queue),
        0o700,
        "the event queue confers the same authority as jobs.json and must not \
         be left group/other-writable"
    );
}

/// Kills `retry.rs:71:25` (`*` -> `+` in `.min(24 * 3600)`), which turns a
/// one-day backoff ceiling into 3624 seconds.
#[test]
fn a_backoff_ceiling_clamps_to_a_full_day() {
    let wide = RetryPolicy {
        max_attempts: 1_000,
        base_backoff_secs: 1,
        max_backoff_secs: 100_000,
    }
    .clamped();
    assert_eq!(
        wide.max_backoff_secs, 86_400,
        "the backoff ceiling is one day (24 * 3600)"
    );
    assert_eq!(
        wide.max_attempts, 10,
        "control: the attempt cap is narrowed by the same call"
    );

    // Control: the clamp narrows, it does not overwrite. A policy already
    // inside both ceilings comes back unchanged.
    let narrow = RetryPolicy {
        max_attempts: 2,
        base_backoff_secs: 30,
        max_backoff_secs: 1_800,
    }
    .clamped();
    assert_eq!(narrow.max_backoff_secs, 1_800);
    assert_eq!(narrow.max_attempts, 2);
}
