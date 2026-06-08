//! Bounded stderr-tail capture for failure dumps (cross-audit M-9).
//!
//! A panic mid-turn (49-audit bug class D1) logs to stderr. If we only
//! captured stdout (assistant text) + session JSON (tool trace), the
//! regression that re-introduced D1 would surface as "Hung" with no
//! root cause. This module drains stderr line-by-line into a fixed-size
//! ring buffer; the runner snapshot()s the last ~50 lines for any
//! failure dump.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::task::JoinHandle;

const RING_CAPACITY: usize = 50;

/// Handle to a background drain task. Drop = task is cancelled
/// (the underlying `child.stderr` is also closed when the child exits,
/// which terminates the loop naturally; explicit `stop` is for unit
/// tests).
pub struct StderrCapture {
    buf: Arc<Mutex<VecDeque<String>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl StderrCapture {
    /// Spawn a drain task on the given `stderr` byte stream. Lines are
    /// read with `BufReader`; non-UTF-8 bytes get replaced with U+FFFD
    /// via `from_utf8_lossy` so the harness never panics on a child
    /// that emits binary garbage.
    pub fn spawn<R>(stderr: R) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let buf = Arc::new(Mutex::new(VecDeque::with_capacity(RING_CAPACITY)));
        let buf_for_task = Arc::clone(&buf);

        let handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = Vec::new();
            loop {
                line.clear();
                // read_until handles partial UTF-8 cleanly — we lossy-
                // convert below.
                match reader.read_until(b'\n', &mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        // Trim the trailing newline if present so the
                        // ring buffer stores clean lines.
                        let s = String::from_utf8_lossy(&line);
                        let s = s.trim_end_matches('\n').trim_end_matches('\r').to_string();
                        if let Ok(mut q) = buf_for_task.lock() {
                            if q.len() == RING_CAPACITY {
                                q.pop_front();
                            }
                            q.push_back(s);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            buf,
            handle: Mutex::new(Some(handle)),
        }
    }

    /// Snapshot the current tail as a `\n`-joined string. Cheap; the
    /// runner calls this once per scenario at report time.
    pub fn snapshot(&self) -> String {
        let q = match self.buf.lock() {
            Ok(q) => q,
            Err(p) => p.into_inner(),
        };
        let lines: Vec<&str> = q.iter().map(String::as_str).collect();
        lines.join("\n")
    }

    /// Best-effort cancel of the drain task. Used by tests so the
    /// tokio runtime can shut down deterministically; in normal runs
    /// the task ends naturally when stderr closes on child exit.
    pub fn stop(&self) {
        if let Ok(mut h) = self.handle.lock()
            && let Some(h) = h.take()
        {
            h.abort();
        }
    }
}

impl Drop for StderrCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::AsyncRead;
    use tokio::time::{Duration, sleep};

    // tokio's `Cursor` is not natively AsyncRead-compatible; wrap a
    // `tokio::io::AsyncReadExt`-friendly version via tokio_util? We
    // don't have tokio_util as a workspace dep here. Use a small
    // adapter that wraps a sync Cursor into an AsyncRead via
    // poll_read; cleaner than adding a new dep just for the test.
    struct CursorAdapter(Cursor<Vec<u8>>);
    impl AsyncRead for CursorAdapter {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            use std::io::Read;
            let mut tmp = vec![0u8; buf.remaining()];
            let n = self.0.read(&mut tmp)?;
            buf.put_slice(&tmp[..n]);
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn captures_lines_into_ring() {
        let bytes = b"alpha\nbravo\ncharlie\n".to_vec();
        let cap = StderrCapture::spawn(CursorAdapter(Cursor::new(bytes)));
        // Give the drain task a tick to consume — the cursor reads
        // synchronously but the task is on the runtime.
        sleep(Duration::from_millis(50)).await;
        let snap = cap.snapshot();
        assert!(snap.contains("alpha"), "snap missing alpha: {snap:?}");
        assert!(snap.contains("bravo"), "snap missing bravo: {snap:?}");
        assert!(snap.contains("charlie"), "snap missing charlie: {snap:?}");
    }

    #[tokio::test]
    async fn ring_caps_at_50_lines() {
        let mut buf = Vec::new();
        for i in 0..120 {
            buf.extend_from_slice(format!("line-{i}\n").as_bytes());
        }
        let cap = StderrCapture::spawn(CursorAdapter(Cursor::new(buf)));
        sleep(Duration::from_millis(50)).await;
        let snap = cap.snapshot();
        let lines: Vec<&str> = snap.split('\n').collect();
        assert!(
            lines.len() <= 50,
            "ring should keep <= 50 lines, got {}",
            lines.len()
        );
        // The TAIL should be present; the head should be gone.
        assert!(snap.contains("line-119"), "missing last line: {snap:?}");
        assert!(!snap.contains("line-5\n"), "head should be evicted");
    }
}
