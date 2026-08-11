//! Does the journal-lease race exist on the PRODUCTION path?
//!
//! The lib-suite failures all land on `lease::lock_data_file` — the flock on
//! the journal DATA file — never on the `.writer.lock` sentinel. The sentinel
//! is explicitly `LOCK_UN`'d in `WriterLease::drop`; the data file is not, so
//! its lock is released only by `close(2)` on the last fd referring to that
//! open file description. `fork(2)` duplicates the fd table, so any subprocess
//! spawned while a journal is open keeps that description alive until it
//! `exec`s (O_CLOEXEC) or exits.
//!
//! Production spawns subprocesses constantly while a session journal is open
//! (Bash tool, `git status`, spawner, forge). This probe uses ONLY production
//! APIs: `SessionJournal::open` and `wcore_config::shell::shell_command_argv`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wcore_agent::session_journal::SessionJournal;

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn spawner_count() -> usize {
    env_usize("PROBE_SPAWNERS", 1)
}

fn burner_count() -> usize {
    env_usize("PROBE_BURNERS", 0)
}

fn iterations() -> usize {
    env_usize("PROBE_ITERS", 400)
}

#[test]
fn journal_reopen_races_subprocess_spawn_on_the_production_path() {
    let dir = tempfile::tempdir().unwrap();
    let stop = Arc::new(AtomicBool::new(false));

    // A background spawner standing in for the Bash/git tool surface.
    let spawners: Vec<_> = (0..spawner_count())
        .map(|_| {
            let spawner_stop = stop.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    while !spawner_stop.load(Ordering::Relaxed) {
                        let mut cmd = wcore_config::shell::shell_command_argv("true", &[]);
                        if let Ok(mut child) = cmd.spawn() {
                            let _ = child.wait().await;
                        }
                    }
                });
            })
        })
        .collect();

    // Scheduling pressure: the fork->exec window only matters when the box is
    // busy, which is exactly the condition the whole-suite run creates.
    let burners: Vec<_> = (0..burner_count())
        .map(|_| {
            let burn_stop = stop.clone();
            std::thread::spawn(move || {
                let mut x = 0u64;
                while !burn_stop.load(Ordering::Relaxed) {
                    x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                    std::hint::black_box(x);
                }
            })
        })
        .collect();

    let mut attempts = 0u32;
    let mut refusals = Vec::new();
    for i in 0..iterations() {
        let path = dir.path().join(format!("s{i}.journal"));
        {
            let journal = SessionJournal::open(&path, format!("probe-{i}")).unwrap();
            drop(journal);
        }
        attempts += 1;
        // The drop above is the ONLY owner. A refusal here is a ghost lock.
        if let Err(error) = SessionJournal::open(&path, format!("probe-{i}")) {
            refusals.push(format!("iteration {i}: {error}"));
        }
    }

    stop.store(true, Ordering::Relaxed);
    for handle in spawners {
        handle.join().unwrap();
    }
    for handle in burners {
        handle.join().unwrap();
    }

    println!("PROBE attempts={attempts} refusals={}", refusals.len());
    for refusal in refusals.iter().take(10) {
        println!("PROBE refusal: {refusal}");
    }
    assert!(
        refusals.is_empty(),
        "closing a session journal must release its data-file lock synchronously; \
         {} of {attempts} reopens were refused by a lock with no live owner: {:?}",
        refusals.len(),
        refusals.iter().take(5).collect::<Vec<_>>()
    );
}
