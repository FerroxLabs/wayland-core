//! Supervised lifecycle for the `signal-cli` subprocess.
//!
//! CRITICAL-4 fix: previously the child process + its reader task died
//! permanently on EOF / IO error / process death, so the channel went
//! silent forever after any disconnect. The supervisor wraps the
//! launch → reader-loop cycle in a respawn loop:
//!
//! 1. Launch `signal-cli`, wire stdin/stdout, start the reader loop.
//! 2. Wait for the reader loop to end OR for shutdown.
//! 3. If the reader ended for a NON-shutdown reason (EOF, IO error,
//!    process death), emit `ConnectionState::Reconnecting`, wait a
//!    backoff interval, and respawn — rebuilding the stdin writer and
//!    starting a fresh pending map / reader.
//! 4. If shutdown was signalled, exit without respawning.
//!
//! The supervisor observes the same `watch::Receiver<bool>` shutdown
//! channel the reader does, so `stop()` tears the whole thing down with
//! no respawn and no task leak.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncBufRead;
use tokio::sync::{Mutex, watch};

use wcore_channels::event::{ChannelEvent, ConnectionState};

use crate::config::SignalConfig;
use crate::subprocess::{
    PendingResponses, ReaderArgs, SharedStdin, SignalProcessHandle, SignalProcessLauncher,
    reader_loop,
};

/// The stdout half + owned child for an already-launched process —
/// handed to the supervisor as its first iteration's seed so `start()`
/// can validate the launch synchronously while the supervisor owns the
/// reader + every respawn thereafter.
pub struct SeedHandle {
    pub stdout: Box<dyn AsyncBufRead + Unpin + Send>,
    pub child: Option<tokio::process::Child>,
}

/// Exponential backoff schedule for respawn attempts.
///
/// Deterministic (no jitter) so it is unit-testable: the delay doubles
/// from `base` each failed attempt, capped at `cap`. A successful
/// reconnect that stays up past [`Backoff::STABLE_AFTER`] resets the
/// schedule back to `base`.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    cap: Duration,
    current: Duration,
}

impl Backoff {
    /// A reconnect that stays up at least this long is considered
    /// "stable" and resets the backoff schedule to the base.
    pub const STABLE_AFTER: Duration = Duration::from_secs(30);

    /// Default schedule: 1s base, doubling, capped at 30s.
    pub fn new() -> Self {
        Self::with_params(Duration::from_secs(1), Duration::from_secs(30))
    }

    /// Construct with explicit base / cap — used by tests to keep the
    /// schedule small and assertable.
    pub fn with_params(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            current: base,
        }
    }

    /// Return the delay to wait before the next respawn attempt, then
    /// advance the schedule (double, capped at `cap`). The first call
    /// after construction (or after [`Backoff::reset`]) returns `base`.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        // Double for the next call, saturating at the cap.
        self.current = (self.current * 2).min(self.cap);
        delay
    }

    /// Reset the schedule back to the base delay. Called after a
    /// reconnect that stayed up past [`Backoff::STABLE_AFTER`].
    pub fn reset(&mut self) {
        self.current = self.base;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything the supervisor task needs to own the respawn loop.
pub struct SupervisorArgs {
    pub config: SignalConfig,
    pub launcher: Arc<dyn SignalProcessLauncher>,
    /// Shared stdin slot — the supervisor swaps the inner writer on each
    /// (re)spawn so `send_message` always sees the current process.
    pub stdin: SharedStdin,
    pub inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    pub pending: PendingResponses,
    /// Shutdown watch shared with each reader loop. The supervisor
    /// exits (without respawning) when this flips to `true`.
    pub shutdown: watch::Receiver<bool>,
    /// The first, already-launched process's stdout + child. `start()`
    /// performs the initial launch synchronously so launcher errors
    /// surface as a `start()` failure; it then hands the live stdout +
    /// child here so the supervisor runs its reader before entering the
    /// respawn loop. The stdin writer for this seed is already installed
    /// in `stdin`.
    pub seed: SeedHandle,
}

/// The supervisor task body. Runs until shutdown is signalled.
///
/// Returns once shutdown is observed. Each respawn rebuilds the stdin
/// writer and starts a fresh reader loop; stale pending requests are
/// already failed by the reader on exit (EOF → `SubprocessClosed`, IO
/// error → `Io`), and we clear the pending map before each respawn so
/// the new process starts clean.
pub async fn supervisor_loop(args: SupervisorArgs) {
    let SupervisorArgs {
        config,
        launcher,
        stdin,
        inbox,
        pending,
        shutdown,
        seed,
    } = args;

    let mut backoff = Backoff::new();
    // The first iteration consumes the seed handle that `start()`
    // already launched + whose stdin is already installed. Later
    // iterations launch fresh inside the loop.
    let mut seed = Some(seed);

    loop {
        // Bail before launching if shutdown is already set.
        if *shutdown.borrow() {
            tracing::debug!(target: "wcore_channel_signal", "supervisor: shutdown before launch");
            break;
        }

        // Acquire the handle for this iteration: the seed on the first
        // pass, or a fresh launch (after Reconnecting + backoff) on a
        // respawn. Either way we end up with the process's stdout +
        // owned child, and the live stdin writer installed in `stdin`.
        let (stdout, child) = match seed.take() {
            // First pass: reuse the handle `start()` launched. Its stdin
            // is already installed and Connected already announced.
            Some(SeedHandle { stdout, child }) => (stdout, child),

            // Respawn pass: announce Reconnecting, back off, relaunch.
            None => {
                inbox
                    .lock()
                    .await
                    .push_back(ChannelEvent::ConnectionStateChanged {
                        state: ConnectionState::Reconnecting,
                    });

                let delay = backoff.next_delay();
                tracing::warn!(
                    target: "wcore_channel_signal",
                    delay_ms = delay.as_millis() as u64,
                    "supervisor: signal-cli connection lost; backing off before respawn"
                );

                // Sleep, but wake early if shutdown fires during the wait.
                let mut sd = shutdown.clone();
                tokio::select! {
                    biased;
                    _ = sd.changed() => {
                        if *shutdown.borrow() {
                            tracing::debug!(target: "wcore_channel_signal", "supervisor: shutdown during backoff");
                            break;
                        }
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
                if *shutdown.borrow() {
                    break;
                }

                // Clear any residual pending requests so the new process
                // starts with a clean map. The reader already failed them
                // on exit; this is belt-and-suspenders for the respawn.
                clear_pending(&pending).await;

                match launcher.launch(&config.signal_cli_path, &config.account) {
                    Ok(SignalProcessHandle {
                        stdin: new_stdin,
                        stdout,
                        child,
                    }) => {
                        // Swap the live stdin writer in so `send_message`
                        // targets the new process. Previous writer dropped.
                        *stdin.lock().await = Some(new_stdin);
                        // Announce Connected on a successful respawn.
                        inbox
                            .lock()
                            .await
                            .push_back(ChannelEvent::ConnectionStateChanged {
                                state: ConnectionState::Connected,
                            });
                        (stdout, child)
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "wcore_channel_signal",
                            error = %e,
                            "supervisor: relaunch failed; will retry after backoff"
                        );
                        // Treat a failed launch like a lost connection:
                        // loop back so the backoff branch fires again.
                        continue;
                    }
                }
            }
        };

        let reader_args = ReaderArgs {
            stdout,
            inbox: Arc::clone(&inbox),
            pending: Arc::clone(&pending),
            shutdown: shutdown.clone(),
        };

        // Run the reader loop inline (this task IS the supervisor; the
        // reader returning means the connection ended one way or
        // another). Time how long it stayed up to decide on backoff
        // reset.
        let started = std::time::Instant::now();
        reader_loop(reader_args).await;
        let uptime = started.elapsed();

        // Kill the child we own so a dead-stdout process doesn't linger.
        if let Some(mut child) = child {
            let _ = child.start_kill();
        }
        // Drop the stale writer; the next launch installs a fresh one.
        *stdin.lock().await = None;

        // If shutdown drove the reader's exit, stop cleanly — no respawn.
        if *shutdown.borrow() {
            tracing::debug!(target: "wcore_channel_signal", "supervisor: reader exited on shutdown; stopping");
            break;
        }

        // Non-shutdown exit (EOF / IO / process death). If the
        // connection had stayed up long enough, reset the backoff so a
        // healthy-then-flaky process doesn't inherit a huge delay.
        if uptime >= Backoff::STABLE_AFTER {
            backoff.reset();
        }
        // Loop back around: the top of the loop emits Reconnecting and
        // backs off before relaunching.
    }
}

/// Fail + clear every in-flight request so a respawn starts clean and no
/// caller hangs on a oneshot whose process is gone.
async fn clear_pending(pending: &PendingResponses) {
    let mut guard = pending.lock().await;
    for (_, tx) in guard.drain() {
        let _ = tx.send(Err(crate::error::SignalError::SubprocessClosed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_sequence_doubles_from_base() {
        let mut b = Backoff::with_params(Duration::from_secs(1), Duration::from_secs(30));
        assert_eq!(b.next_delay(), Duration::from_secs(1));
        assert_eq!(b.next_delay(), Duration::from_secs(2));
        assert_eq!(b.next_delay(), Duration::from_secs(4));
        assert_eq!(b.next_delay(), Duration::from_secs(8));
        assert_eq!(b.next_delay(), Duration::from_secs(16));
    }

    #[test]
    fn backoff_caps_at_ceiling() {
        let mut b = Backoff::with_params(Duration::from_secs(1), Duration::from_secs(30));
        // 1, 2, 4, 8, 16 → next would be 32 but caps at 30, and stays.
        for _ in 0..5 {
            b.next_delay();
        }
        assert_eq!(b.next_delay(), Duration::from_secs(30));
        assert_eq!(b.next_delay(), Duration::from_secs(30));
        assert_eq!(b.next_delay(), Duration::from_secs(30));
    }

    #[test]
    fn backoff_reset_returns_to_base() {
        let mut b = Backoff::with_params(Duration::from_secs(1), Duration::from_secs(30));
        b.next_delay(); // 1
        b.next_delay(); // 2
        b.next_delay(); // 4
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_secs(1));
        assert_eq!(b.next_delay(), Duration::from_secs(2));
    }

    #[test]
    fn backoff_default_matches_one_second_base_thirty_cap() {
        let mut b = Backoff::new();
        assert_eq!(b.next_delay(), Duration::from_secs(1));
        // Walk to the ceiling and confirm it's 30s.
        for _ in 0..10 {
            b.next_delay();
        }
        assert_eq!(b.next_delay(), Duration::from_secs(30));
    }
}
