//! Exit-aware wrapper around the candidate's stdout pipe.
//!
//! # Why this exists
//!
//! The session driver reads the candidate's stdout until EOF and treats EOF as
//! "the candidate is done talking". On Unix that holds: the candidate's stdout
//! pipe has exactly one writer, and `exec`ing a descendant with `Stdio::null()`
//! replaces fd 1 in that descendant, so the pipe's last write end closes when
//! the candidate exits.
//!
//! **On Windows it does not hold.** `CreateProcessW` is called with
//! `bInheritHandles = TRUE`, and the stdout pipe handle the candidate received
//! from us is itself inheritable, so EVERY descendant the candidate spawns
//! inherits a live write end — even one launched with all three stdio streams
//! set to NUL, because `STARTUPINFO` only redirects the descendant's *own*
//! stdio, it does not revoke the inherited handle. A candidate that leaves any
//! background process running (an MCP stdio server, a daemon, a watcher) is
//! therefore a candidate whose stdout pipe never reaches EOF, no matter how
//! cleanly the candidate itself exited.
//!
//! Measured on Windows (SeanDesktop, 2026-08-01): a fixture that spawned one
//! background listener and then called `ExitProcess(0)` produced **no EOF for
//! 4 s** on its stdout; killing the descendant produced EOF immediately.
//!
//! The consequence was not a containment failure — the Job Object reaped the
//! descendant correctly every time — it was a **grading** failure. The driver
//! blocked on a pipe nobody would ever write to again until the scenario's
//! outer wall clock expired, and the run was then recorded as
//! `Failure::Hung`. A clean exit, an assertion failure and an early crash all
//! came back as "hung", and the real failure was erased from the result.
//!
//! # What this does
//!
//! It makes "the direct child exited" a second, independent EOF condition. The
//! probe is [`wcore_types::process_liveness`], which reports an exited-but-not
//! yet-reaped process as `Dead` on both Unix (zombie) and Windows (pid still
//! reserved by an open handle) — exactly the state the candidate is in between
//! its exit and the runner's `wait()`.
//!
//! Two properties keep this honest:
//!
//! * **It cannot truncate output.** EOF is only synthesised while the inner
//!   pipe has no readable bytes. Everything the candidate wrote before exiting
//!   is already in the pipe buffer and reads `Ready`, which resets the probe.
//! * **It cannot recycle a pid.** The direct child is deliberately left
//!   unreaped for the whole session (see `ProcessTree`'s identity-anchor
//!   comments); its pid cannot name another process until the runner reaps it,
//!   which happens strictly after the driver has returned.
//!
//! `ProcessLiveness::Indeterminate` counts as alive, so a probe that cannot
//! answer degrades to the previous behaviour (wait for the outer deadline)
//! rather than inventing an EOF it did not observe.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::process::ChildStdout;
use wcore_types::process_liveness::process_liveness;

/// How often to re-probe the direct child while the pipe is idle.
const LIVENESS_PROBE_INTERVAL: Duration = Duration::from_millis(20);

/// How long the pipe must stay idle AFTER the child was first observed exited
/// before EOF is synthesised.
///
/// Everything the child wrote is in the pipe before it exits, so one grace
/// window is belt-and-braces against a write that has completed in the child
/// but not yet surfaced to this reader's completion port.
const POST_EXIT_DRAIN: Duration = Duration::from_millis(150);

/// The candidate's stdout, with process exit as a second EOF condition.
#[derive(Debug)]
pub(crate) struct CandidateStdout {
    inner: ChildStdout,
    /// `None` disables the probe entirely and reads exactly like the raw pipe.
    direct_child_pid: Option<u32>,
    /// When the direct child was first observed exited with the pipe idle.
    exit_observed_at: Option<Instant>,
    tick: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl CandidateStdout {
    pub(crate) fn new(inner: ChildStdout, direct_child_pid: Option<u32>) -> Self {
        Self {
            inner,
            direct_child_pid,
            exit_observed_at: None,
            tick: None,
        }
    }
}

impl AsyncRead for CandidateStdout {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            // Real data, or the pipe's own EOF. Either way the inner stream
            // answered, so any pending exit observation is stale: a child that
            // is still producing bytes must never be cut off.
            Poll::Ready(result) => {
                this.exit_observed_at = None;
                this.tick = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                let Some(pid) = this.direct_child_pid else {
                    return Poll::Pending;
                };

                // Nothing readable right now. If the candidate has exited, no
                // further bytes can ever arrive from IT — only from a
                // descendant holding an inherited write end, which is not
                // session output and must not keep the driver blocked.
                let now = Instant::now();
                match this.exit_observed_at {
                    Some(observed) if now.duration_since(observed) >= POST_EXIT_DRAIN => {
                        this.tick = None;
                        // Filling zero bytes IS EOF for `AsyncRead`.
                        return Poll::Ready(Ok(()));
                    }
                    Some(_) => {}
                    None => {
                        if process_liveness(pid).is_dead() {
                            this.exit_observed_at = Some(now);
                        }
                    }
                }

                // The inner pipe will not wake us if it is never written to
                // again, so drive the probe from a timer of our own.
                let tick = this
                    .tick
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(LIVENESS_PROBE_INTERVAL)));
                if tick.as_mut().poll(cx).is_ready() {
                    let next = tokio::time::Instant::now() + LIVENESS_PROBE_INTERVAL;
                    tick.as_mut().reset(next);
                    // Re-poll so the freshly reset timer registers this waker.
                    let _ = tick.as_mut().poll(cx);
                }
                Poll::Pending
            }
        }
    }
}
