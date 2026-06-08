//! Off-thread statusLine command executor (SPEC §6 — audit BLOCKER).
//!
//! The user's `statusLine.command` is run HERE, on a dedicated background
//! tokio task, NEVER on the synchronous render path. The task publishes its
//! sanitized result into the shared [`StatusLineCache`]; `widgets::status_bar`
//! only reads that cache (see the module docs in `statusline/mod.rs`).
//!
//! ## Defenses (every one is enforced below)
//!
//! - **Debounce ≥1 s** — `tokio::time::interval(DEBOUNCE)`, so the command
//!   is forked at most once per second, never per-frame.
//! - **Hard 500 ms timeout** — a SINGLE `tokio::time::timeout` wraps the
//!   whole run (stdout read + process wait), so a command that writes fast
//!   then sleeps before exiting still completes within one 500ms budget, not
//!   two sequential ones. `std::process::Command` has NO built-in timeout,
//!   which is exactly why this must be async. On expiry the child is killed.
//! - **4 KB stdout cap** — at most [`STDOUT_CAP`] bytes are read, then the
//!   child is killed (defends against a megabyte-spewing command).
//! - **One-line + ANSI/OSC sanitize** — [`super::sanitize_status_output`]
//!   strips escapes/control chars and keeps the first line, so a command
//!   can never inject escape sequences or move the cursor.
//! - **Last-good on failure** — a non-zero exit, timeout, or spawn error
//!   leaves the cache untouched (never blanks the bar, never blocks render).
//!
//! ## SECURITY / trust boundary (SPEC §6)
//!
//! The command runs with the user's full environment via the user's shell.
//! It is SETTINGS-FILE-ONLY: the model CANNOT set `statusLine.command` —
//! there is no protocol command, no slash command, and no tool that writes
//! it. The trust boundary is "the user trusts their own status command."

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::StatusLineCache;

/// Minimum wall-clock gap between two command runs. The status command is a
/// chrome decoration, not a live feed — once per second is plenty and keeps
/// us from forking a process every frame.
const DEBOUNCE: Duration = Duration::from_secs(1);

/// Hard ceiling on a single command run. Past this the child is killed and
/// the run counts as a failure (last-good cache is kept). Async-only — this
/// is why the executor cannot live on the synchronous render path.
const RUN_TIMEOUT: Duration = Duration::from_millis(500);

/// Maximum bytes read from the command's stdout before the child is killed.
/// Defends the renderer against a command that spews megabytes.
const STDOUT_CAP: usize = 4096;

/// Spawn the background statusLine sampler. Runs `command` at most once per
/// [`DEBOUNCE`] on a dedicated tokio task and publishes each sanitized
/// result into `cache`. Returns immediately; the task runs until the
/// process exits. Call from inside the tokio runtime (the TUI run-loop).
pub fn spawn_statusline_sampler(command: String, cache: Arc<Mutex<StatusLineCache>>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(DEBOUNCE);
        loop {
            ticker.tick().await;
            // The contract JSON fed on stdin. Per-tick session data (cost,
            // ctx) would require an App snapshot the background task does
            // not hold; a minimal process-derived contract is fed for now
            // (still valid + versioned). Richer fields are a follow-up that
            // needs an App→sampler snapshot channel.
            let contract = minimal_contract_json();
            let result = run_once(&command, &contract).await;
            publish(&cache, result);
        }
    });
}

/// Build a minimal, valid, versioned contract from process-available data.
/// The background task has no `App` handle, so cost/context fields are
/// zero-valued placeholders; the schema (incl. `contract_version`) is
/// stable so a command can still parse it.
fn minimal_contract_json() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    super::build_contract_json(
        "wayland",
        "",
        "",
        &cwd,
        &cwd,
        env!("CARGO_PKG_VERSION"),
        0.0,
        0,
        0.0,
        0,
    )
}

/// Run the command once, feeding `contract_json` on stdin, with all the
/// §6 defenses. Returns `Some(sanitized_one_line)` on a clean (exit-zero)
/// run, or `None` on timeout / non-zero exit / spawn error — the caller
/// keeps the last-good cache on `None`.
async fn run_once(command: &str, contract_json: &str) -> Option<String> {
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("/bin/sh", "-c")
    };

    let mut child = Command::new(shell)
        .arg(flag)
        .arg(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Feed the contract on stdin, then drop the handle to send EOF. A
    // command that ignores stdin is unaffected.
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(contract_json.as_bytes()).await;
        // Dropping `stdin` here closes the pipe (EOF) so a reader unblocks.
    }

    // Read at most STDOUT_CAP bytes, then wait for a clean exit — the WHOLE
    // thing bounded by a SINGLE hard timeout. A command that writes quickly
    // but then sleeps before exiting (the M1 case) must not get a fresh
    // budget for the wait: read + wait share one `RUN_TIMEOUT`, so the total
    // wall budget is the documented hard 500ms, not ~1s.
    let run_fut = async {
        // Phase 1: read the (capped) stdout.
        let mut buf = Vec::with_capacity(256);
        if let Some(mut out) = child.stdout.take() {
            let mut chunk = [0u8; 1024];
            loop {
                match out.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.len() >= STDOUT_CAP {
                            buf.truncate(STDOUT_CAP);
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        // Phase 2: confirm a clean exit within the same budget.
        let status = child.wait().await;
        (buf, status)
    };

    let (buf, status) = match tokio::time::timeout(RUN_TIMEOUT, run_fut).await {
        Ok(pair) => pair,
        Err(_) => {
            // The combined read+wait blew the single hard timeout — kill the
            // child and keep last-good.
            let _ = child.start_kill();
            return None;
        }
    };

    // Non-zero exit / wait error → failure (caller keeps last-good).
    match status {
        Ok(status) if status.success() => {}
        _ => return None,
    }

    let raw = String::from_utf8_lossy(&buf);
    let line = super::sanitize_status_output(&raw);
    if line.is_empty() { None } else { Some(line) }
}

/// Publish a run result into the cache. `Some` updates the line +
/// timestamp; `None` (a failed run) leaves the last-good cache untouched.
/// A poisoned lock is ignored (the renderer keeps whatever it last read).
fn publish(cache: &Arc<Mutex<StatusLineCache>>, result: Option<String>) {
    // `None` (a failed run) keeps last-good — do nothing.
    let Some(line) = result else { return };
    if let Ok(mut guard) = cache.lock() {
        guard.line = Some(line);
        guard.updated_at = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_cache() -> Arc<Mutex<StatusLineCache>> {
        Arc::new(Mutex::new(StatusLineCache::default()))
    }

    #[test]
    fn publish_some_updates_line_and_timestamp() {
        let cache = fresh_cache();
        publish(&cache, Some("ok".into()));
        let g = cache.lock().unwrap();
        assert_eq!(g.line.as_deref(), Some("ok"));
        assert!(g.updated_at.is_some());
    }

    #[test]
    fn publish_none_keeps_last_good() {
        // A failed run (None) must NEVER blank an existing good line.
        let cache = fresh_cache();
        publish(&cache, Some("last-good".into()));
        publish(&cache, None);
        let g = cache.lock().unwrap();
        assert_eq!(
            g.line.as_deref(),
            Some("last-good"),
            "None result clobbered last-good"
        );
    }

    #[test]
    fn minimal_contract_is_valid_versioned_json() {
        let s = minimal_contract_json();
        assert!(s.contains("\"contract_version\":1"));
        // Parseable JSON.
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        assert_eq!(v["contract_version"], 1);
    }

    #[tokio::test]
    async fn run_once_returns_echo_output() {
        // A trivial command paints its stdout (sanitized, one line).
        let out = run_once("echo hello-status", "{}").await;
        assert_eq!(out.as_deref(), Some("hello-status"));
    }

    #[tokio::test]
    async fn run_once_strips_escapes_from_output() {
        // Even if the command emits the ESC control byte, it is stripped
        // (the printable SGR args that follow survive — see the sanitizer
        // unit tests in `mod.rs`). The key property: no raw ESC reaches the
        // chrome, so the command cannot move the cursor or recolor the bar.
        let out = run_once("printf 'a\\033[31mb'", "{}").await.unwrap();
        assert!(!out.contains('\u{1b}'), "ESC leaked into output: {out:?}");
        assert!(out.starts_with('a') && out.ends_with('b'));
    }

    #[tokio::test]
    async fn run_once_non_zero_exit_is_a_failure() {
        // A non-zero exit yields None so the caller keeps last-good.
        // `;`/`exit` are POSIX-shell syntax; `run_once` uses `cmd /C` on
        // Windows (where `;` is literal), so use a cmd-native non-zero exit.
        let cmd = if cfg!(windows) {
            "exit 7"
        } else {
            "echo nope; exit 7"
        };
        let out = run_once(cmd, "{}").await;
        assert_eq!(out, None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_once_times_out_a_slow_command() {
        // A command slower than RUN_TIMEOUT (500ms) is killed and counts
        // as a failure (None) — the render path is NEVER blocked on it. The
        // SINGLE hard timeout (M1 fix) means the whole run is killed within
        // ~500ms (+ scheduling slack), not the ~1s two sequential timeouts
        // used to allow.
        let start = std::time::Instant::now();
        let out = run_once("sleep 2", "{}").await;
        let elapsed = start.elapsed();
        assert_eq!(out, None, "slow command must fail, not hang");
        assert!(
            elapsed < Duration::from_millis(600),
            "single hard timeout did not fire within budget: {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_once_single_timeout_no_stacking_on_slow_exit() {
        // M1 regression guard: a command that writes its output FAST but then
        // sleeps before exiting must NOT get a fresh wait budget. With the old
        // two-sequential-timeouts the read completed early then `wait()` got a
        // fresh 500ms → ~1s total. With the single timeout wrapping read+wait,
        // the whole run is bounded by one RUN_TIMEOUT (~500ms), so this is
        // killed well under 600ms.
        // Write output fast, then block ~2s before exit. On Windows `cmd /C`
        // has no `sleep`/`;`; `ping -n 3 127.0.0.1` blocks ~2s and `&` chains
        // after the fast echo. Either way the run is killed at ~500ms → None.
        let start = std::time::Instant::now();
        let cmd = if cfg!(windows) {
            "echo fast-out & ping -n 3 127.0.0.1"
        } else {
            "echo fast-out; sleep 2"
        };
        let out = run_once(cmd, "{}").await;
        let elapsed = start.elapsed();
        assert_eq!(
            out, None,
            "slow-exit command must fail (killed), not return"
        );
        assert!(
            elapsed < Duration::from_millis(600),
            "read+wait stacked past one hard 500ms budget: {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_command_keeps_prior_last_good_via_publish() {
        // End-to-end of the defense: a good run, then a timing-out run,
        // leaves the good line in the cache.
        let cache = fresh_cache();
        publish(&cache, run_once("echo good", "{}").await);
        publish(&cache, run_once("sleep 2", "{}").await);
        let g = cache.lock().unwrap();
        assert_eq!(g.line.as_deref(), Some("good"));
    }
}
