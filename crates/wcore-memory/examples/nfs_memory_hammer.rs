//! Product-level driver for the network-filesystem WAL corruption question.
//!
//! # Why this exists
//!
//! `wcore-config::sqlite_journal` was built and proven at the *SQLite* level:
//! two raw writers on a genuinely-incoherent NFS mount in WAL produced 11,826
//! write errors, 0 of 5,102 rows visible to their own writer, and a failed
//! `integrity_check` — all at `rc=0`. What was **not** shown was the *product*
//! corrupting a database through its own code paths. `wayland-core index build`
//! was tried and is too short-lived: six rounds left `integrity_check = ok`,
//! because the two processes barely overlap.
//!
//! So this driver reproduces the exposure the product actually has. It uses
//! [`wcore_memory::memory::Memory`] — the same constructor production bootstrap
//! calls — and writes through [`MemoryApi::record_episode`] /
//! [`MemoryApi::update_user_model`], the same calls the memory tool makes. It is
//! the product's storage layer under a harness `main`, not `rusqlite` in a loop
//! and not an LLM session. `memory.db` is the database at stake: it holds
//! long-term user memory, so a silent corruption there is unrecoverable user
//! data loss.
//!
//! # How to make it discriminate
//!
//! Run two of these concurrently against the SAME backing file through TWO
//! separate NFS client mounts (`nosharecache`, so the superblocks and therefore
//! the page caches are genuinely incoherent — one host sharing a page cache
//! cannot reproduce the defect, and a run that forgets this will pass for the
//! wrong reason). Point each at its own mount with `WCORE_MEMORY_DIR`.
//!
//! Both arms must be run. `WAYLAND_SQLITE_JOURNAL_MODE=wal` is the pre-fix
//! behaviour (the defect arm); leaving it unset exercises the selector (the fix
//! arm). A WAL arm that does not corrupt is a real result and must be reported
//! as one — see the lane brief: do not manufacture a reproduction.
//!
//! ```text
//! WCORE_MEMORY_DIR=/mnt/walnfs/mem  WAYLAND_SQLITE_JOURNAL_MODE=wal \
//!   HAMMER_LABEL=A HAMMER_SECONDS=120 cargo run --example nfs_memory_hammer &
//! WCORE_MEMORY_DIR=/mnt/walnfs2/mem WAYLAND_SQLITE_JOURNAL_MODE=wal \
//!   HAMMER_LABEL=B HAMMER_SECONDS=120 cargo run --example nfs_memory_hammer &
//! ```
//!
//! # What it reports, and why each field is there
//!
//! - `journal_mode` — read back **from the database**, never assumed from the
//!   environment. An arm that believes it forced WAL and did not would silently
//!   measure the wrong thing.
//! - `writes_ok` / `writes_err` — the write-error count is the loudest signal;
//!   the raw-SQLite reproduction showed 11,826 against 0 in the safe modes.
//! - `own_rows_visible` vs `writes_ok` — the sharpest signal, because it is
//!   silent. The raw reproduction had a writer commit 5,102 rows and then see
//!   **zero** of them. Any shortfall here is corruption with no error raised.
//! - `integrity_check` — SQLite's own verdict.
//! - `fatal_signal` — there is none to expect. The prior lane's measured
//!   correction to the original bug report is that the process *survives*; this
//!   field exists so a future reader can see that was checked, not assumed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use wcore_memory::MemoryApi;
use wcore_memory::memory::Memory;
use wcore_memory::v2_types::{AccessToken, Episode, EpisodeId, EpisodeStatus, Tier};

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// One episode tagged with this writer's label, so "rows I myself wrote" is
/// answerable afterwards without coordinating with the other writer.
fn episode(label: &str, n: u64) -> Episode {
    Episode {
        id: EpisodeId::new(),
        tier: Tier::Global,
        ts: now_secs(),
        episode_type: "nfs_hammer".into(),
        // Padded so each row is a few hundred bytes: single-digit rows can fit
        // a whole run inside one page and never force the multi-page frame
        // traffic that exercises the wal-index.
        summary: format!("writer {label} episode {n} {}", "x".repeat(400)),
        atomic_facts: format!("[\"writer {label} fact {n}\"]"),
        source: format!("nfs-hammer-{label}"),
        source_product: "wayland-core".into(),
        session_id: None,
        project_root: None,
        decay_score: 1.0,
        status: EpisodeStatus::Active,
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let label: String = std::env::var("HAMMER_LABEL").unwrap_or_else(|_| "A".into());
    let seconds: u64 = env_or("HAMMER_SECONDS", 60u64);
    let concurrency: usize = env_or("HAMMER_TASKS", 4usize);
    let project_root: PathBuf = std::env::var("HAMMER_PROJECT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join(format!("nfs-hammer-proj-{label}")));
    std::fs::create_dir_all(&project_root).ok();

    // The product's own bootstrap constructor. Everything below writes through
    // the same dispatcher the memory tool uses.
    let mem = match Memory::open(&project_root, &format!("nfs-hammer-{label}")).await {
        Ok(m) => m,
        Err(e) => {
            println!("HAMMER label={label} FATAL open_failed err={e}");
            std::process::exit(2);
        }
    };

    let db_path: PathBuf = mem.db.global.path.clone();

    // Read the mode back OUT OF THE DATABASE. The lane brief's §3b-ii rule:
    // never infer a selection from the environment you exported.
    let journal_mode = {
        let conn = mem.db.global.conn.lock();
        conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap_or_else(|e| format!("<unreadable: {e}>"))
    };
    println!(
        "HAMMER label={label} START db={} journal_mode={journal_mode} seconds={seconds} tasks={concurrency}",
        db_path.display()
    );

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let dispatcher = std::sync::Arc::new(mem.dispatcher.clone());

    let mut handles = Vec::new();
    for task in 0..concurrency {
        let dispatcher = dispatcher.clone();
        let label = label.clone();
        handles.push(tokio::spawn(async move {
            let (mut ok, mut err) = (0u64, 0u64);
            let mut last_err = String::new();
            let mut n = 0u64;
            while Instant::now() < deadline {
                n += 1;
                let id = n * 1_000 + task as u64;
                match dispatcher
                    .record_episode(episode(&label, id), AccessToken::System)
                    .await
                {
                    Ok(_) => ok += 1,
                    Err(e) => {
                        err += 1;
                        last_err = e.to_string();
                    }
                }
                // A second table on the same connection, so the run is not a
                // single-table insert loop.
                match dispatcher
                    .update_user_model(
                        &format!("hammer_{label}_{task}"),
                        serde_json::json!({ "n": id }),
                        AccessToken::System,
                    )
                    .await
                {
                    Ok(()) => ok += 1,
                    Err(e) => {
                        err += 1;
                        last_err = e.to_string();
                    }
                }
            }
            (ok, err, last_err)
        }));
    }

    let (mut writes_ok, mut writes_err) = (0u64, 0u64);
    let mut last_err = String::new();
    for h in handles {
        match h.await {
            Ok((ok, err, e)) => {
                writes_ok += ok;
                writes_err += err;
                if !e.is_empty() {
                    last_err = e;
                }
            }
            Err(e) => {
                writes_err += 1;
                last_err = format!("task panicked/aborted: {e}");
            }
        }
    }

    // ---- The silent-corruption probe: can this writer see its own commits? --
    let (own_rows, integrity, mode_after) = {
        let conn = mem.db.global.conn.lock();
        let own = conn
            .query_row(
                "SELECT count(*) FROM episodes WHERE source = ?1",
                [format!("nfs-hammer-{label}")],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n.to_string())
            .unwrap_or_else(|e| format!("<query failed: {e}>"));
        let integrity = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap_or_else(|e| format!("<check failed: {e}>"));
        let mode_after = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap_or_else(|e| format!("<unreadable: {e}>"));
        (own, integrity, mode_after)
    };

    // Episodes are written in pairs with a user-model upsert, so the episode
    // count this writer should see is half its successful writes.
    let expected_own = writes_ok / 2;

    println!(
        "HAMMER label={label} RESULT journal_mode={journal_mode} journal_mode_after={mode_after} \
         writes_ok={writes_ok} writes_err={writes_err} own_episodes_expected={expected_own} \
         own_episodes_visible={own_rows} integrity_check={integrity} last_err=\"{last_err}\" \
         db_bytes={}",
        std::fs::metadata(Path::new(&db_path))
            .map(|m| m.len())
            .unwrap_or(0)
    );
    println!("HAMMER label={label} DONE");
}
