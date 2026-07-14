use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const MAX_QUEUED_BYTES: usize = 8 * 1024 * 1024;
const MAX_QUEUED_FRAMES: usize = 4096;
const OUTPUT_DRAIN_BUDGET: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

struct Frame {
    stream: OutputStream,
    bytes: Vec<u8>,
    sequence: u64,
}

#[derive(Default)]
struct Queue {
    frames: VecDeque<Frame>,
    queued_bytes: usize,
    accepted_sequence: u64,
    completed_sequence: u64,
    /// Accepted frame whose asynchronous delivery failed. A snapshot is
    /// poisoned only when its target includes this sequence.
    delivery_failure_sequence: Option<u64>,
    /// Admission/control failure observed before a snapshot was taken. A
    /// rejection after a snapshot cannot retroactively poison that snapshot.
    sticky_failure: bool,
    closed: bool,
    exited: bool,
}

struct State {
    queue: Mutex<Queue>,
    ready: Condvar,
    started: AtomicBool,
    failed: AtomicBool,
    failure_handler: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl State {
    fn record_failure(&self) -> (bool, Option<Arc<dyn Fn() + Send + Sync>>) {
        let first = !self.failed.swap(true, Ordering::AcqRel);
        let handler = first.then(|| self.failure_handler.clone()).flatten();
        (first, handler)
    }

    fn invoke_failure(handler: Option<Arc<dyn Fn() + Send + Sync>>) {
        if let Some(handler) = handler {
            handler();
        }
    }
}

/// Byte-bounded, non-blocking process-output queue.
///
/// Callers only serialize and enqueue. A detached writer thread owns the
/// potentially blocking OS stdout/stderr writes, so a stalled host pipe cannot
/// pin an agent runtime or prevent signal-driven teardown. Once the byte/frame
/// budget is full, `write` fails with `WouldBlock` instead of blocking or
/// growing memory without bound.
pub struct OutputPump {
    state: Arc<State>,
}

#[derive(Clone)]
pub struct OutputPumpWriter {
    output: Arc<OutputPump>,
    stream: OutputStream,
}

impl Write for OutputPumpWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.output.write(self.stream, bytes.to_vec())?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush_bounded()
    }
}

impl Default for OutputPump {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputPump {
    pub fn new() -> Self {
        Self::with_writer(write_process_output, None)
    }

    pub fn new_with_failure_handler(handler: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::with_writer(write_process_output, Some(handler))
    }

    pub fn writer(self: &Arc<Self>, stream: OutputStream) -> OutputPumpWriter {
        OutputPumpWriter {
            output: Arc::clone(self),
            stream,
        }
    }

    /// Wait at most 100 ms for every frame accepted before this call to finish
    /// writing. The pump remains open on success. A timeout or asynchronous
    /// writer failure is returned to the caller and makes failure sticky.
    pub fn flush_bounded(&self) -> io::Result<()> {
        self.drain_bounded(OUTPUT_DRAIN_BUDGET, false)
    }

    /// Stop accepting frames and wait at most 100 ms for accepted output to be
    /// written in FIFO order. Success means the worker has exited after
    /// delivering the complete queue. A blocked OS write returns `TimedOut`;
    /// a late write failure returns `BrokenPipe`. The worker is never joined.
    pub fn close_and_drain_bounded(&self) -> io::Result<()> {
        self.drain_bounded(OUTPUT_DRAIN_BUDGET, true)
    }

    fn drain_bounded(&self, budget: Duration, close: bool) -> io::Result<()> {
        self.drain_bounded_after_snapshot(budget, close, || {})
    }

    fn drain_bounded_after_snapshot<F>(
        &self,
        budget: Duration,
        close: bool,
        after_snapshot: F,
    ) -> io::Result<()>
    where
        F: FnOnce(),
    {
        let deadline = Instant::now() + budget;
        let mut queue = match self.state.queue.lock() {
            Ok(queue) => queue,
            Err(error) => {
                drop(error);
                let (_, handler) = self.state.record_failure();
                State::invoke_failure(handler);
                return Err(io::Error::other("output pump lock poisoned"));
            }
        };
        let target_sequence = queue.accepted_sequence;
        let failed_at_snapshot = queue.sticky_failure;
        if close {
            queue.closed = true;
            self.state.ready.notify_one();
        }
        after_snapshot();

        loop {
            let target_failed = queue
                .delivery_failure_sequence
                .is_some_and(|sequence| sequence <= target_sequence);
            if failed_at_snapshot || target_failed {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "output pump stopped before delivery completed",
                ));
            }
            let target_completed = queue.completed_sequence >= target_sequence;
            if target_completed && (!close || queue.exited) {
                return Ok(());
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match self.state.ready.wait_timeout(queue, remaining) {
                Ok((next, timeout)) => {
                    queue = next;
                    if timeout.timed_out() {
                        continue;
                    }
                }
                Err(error) => {
                    drop(error);
                    let (_, handler) = self.state.record_failure();
                    State::invoke_failure(handler);
                    return Err(io::Error::other("output pump lock poisoned"));
                }
            }
        }

        queue.sticky_failure = true;
        let (first, handler) = self.state.record_failure();
        self.state.ready.notify_all();
        drop(queue);
        State::invoke_failure(handler);
        if first {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "output pump drain timed out",
            ))
        } else {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "output pump stopped before delivery completed",
            ))
        }
    }

    pub(crate) fn with_writer<F>(
        writer: F,
        failure_handler: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self
    where
        F: Fn(OutputStream, &[u8]) -> io::Result<()> + Send + 'static,
    {
        let state = Arc::new(State {
            queue: Mutex::new(Queue::default()),
            ready: Condvar::new(),
            started: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failure_handler,
        });
        let worker_state = Arc::clone(&state);
        let spawned = std::thread::Builder::new()
            .name("wcore-output-pump".to_string())
            .spawn(move || run_writer(worker_state, writer))
            .is_ok();
        state.started.store(spawned, Ordering::Release);
        if !spawned {
            let (_, handler) = state.record_failure();
            if let Ok(mut queue) = state.queue.lock() {
                queue.sticky_failure = true;
                state.ready.notify_all();
            }
            State::invoke_failure(handler);
        }
        Self { state }
    }

    pub fn write(&self, stream: OutputStream, bytes: Vec<u8>) -> io::Result<()> {
        if !self.state.started.load(Ordering::Acquire) {
            return Err(io::Error::other("output pump failed to start"));
        }
        if self.state.failed.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "output pump stopped",
            ));
        }
        let mut queue = match self.state.queue.lock() {
            Ok(queue) => queue,
            Err(error) => {
                drop(error);
                let (_, handler) = self.state.record_failure();
                State::invoke_failure(handler);
                return Err(io::Error::other("output pump lock poisoned"));
            }
        };
        if queue.closed || self.state.failed.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "output pump is closed",
            ));
        }
        if bytes.len() > MAX_QUEUED_BYTES {
            queue.sticky_failure = true;
            let (_, handler) = self.state.record_failure();
            self.state.ready.notify_all();
            drop(queue);
            State::invoke_failure(handler);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output frame exceeds bounded queue capacity",
            ));
        }
        if queue.frames.len() >= MAX_QUEUED_FRAMES
            || queue.queued_bytes.saturating_add(bytes.len()) > MAX_QUEUED_BYTES
        {
            queue.sticky_failure = true;
            let (_, handler) = self.state.record_failure();
            self.state.ready.notify_all();
            drop(queue);
            State::invoke_failure(handler);
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "output pump queue is full",
            ));
        }
        // A wrapped sequence would make an old completion indistinguishable
        // from a new snapshot. Exhaustion is therefore a sticky rejection.
        let Some(sequence) = queue.accepted_sequence.checked_add(1) else {
            queue.sticky_failure = true;
            let (_, handler) = self.state.record_failure();
            self.state.ready.notify_all();
            drop(queue);
            State::invoke_failure(handler);
            return Err(io::Error::other("output pump sequence exhausted"));
        };
        queue.queued_bytes += bytes.len();
        queue.frames.push_back(Frame {
            stream,
            bytes,
            sequence,
        });
        queue.accepted_sequence = sequence;
        self.state.ready.notify_one();
        Ok(())
    }
}

impl Drop for OutputPump {
    fn drop(&mut self) {
        // Preserve normal final output when the host is draining, while still
        // making a blocked host a strictly bounded destructor path.
        let _ = self.close_and_drain_bounded();
    }
}

fn run_writer<F>(state: Arc<State>, writer: F)
where
    F: Fn(OutputStream, &[u8]) -> io::Result<()>,
{
    loop {
        let frame = {
            let mut queue = match state.queue.lock() {
                Ok(queue) => queue,
                Err(error) => {
                    drop(error);
                    let (_, handler) = state.record_failure();
                    State::invoke_failure(handler);
                    return;
                }
            };
            while queue.frames.is_empty() && !queue.closed {
                queue = match state.ready.wait(queue) {
                    Ok(queue) => queue,
                    Err(error) => {
                        drop(error);
                        let (_, handler) = state.record_failure();
                        State::invoke_failure(handler);
                        return;
                    }
                };
            }
            let Some(frame) = queue.frames.pop_front() else {
                queue.exited = true;
                state.ready.notify_all();
                return;
            };
            queue.queued_bytes -= frame.bytes.len();
            frame
        };

        if writer(frame.stream, &frame.bytes).is_err() {
            let (_, handler) = state.record_failure();
            if let Ok(mut queue) = state.queue.lock() {
                queue.delivery_failure_sequence = Some(frame.sequence);
                queue.frames.clear();
                queue.queued_bytes = 0;
                queue.closed = true;
                queue.exited = true;
                state.ready.notify_all();
            }
            State::invoke_failure(handler);
            return;
        }

        match state.queue.lock() {
            Ok(mut queue) => {
                queue.completed_sequence = frame.sequence;
                state.ready.notify_all();
            }
            Err(error) => {
                drop(error);
                let (_, handler) = state.record_failure();
                State::invoke_failure(handler);
                return;
            }
        }
    }
}

fn write_process_output(stream: OutputStream, bytes: &[u8]) -> io::Result<()> {
    match stream {
        OutputStream::Stdout => {
            let stdout = io::stdout();
            let mut writer = stdout.lock();
            writer.write_all(bytes).and_then(|()| writer.flush())
        }
        OutputStream::Stderr => {
            let stderr = io::stderr();
            let mut writer = stderr.lock();
            writer.write_all(bytes).and_then(|()| writer.flush())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_writer_saturates_without_blocking_producer() {
        let failures = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failures_for_handler = Arc::clone(&failures);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let pump = Arc::new(OutputPump::with_writer(
            move |_, _| {
                let _ = entered_tx.send(());
                let _ = release_rx.lock().unwrap().recv();
                let _ = finished_tx.send(());
                Ok(())
            },
            Some(Arc::new(move || {
                failures_for_handler.fetch_add(1, Ordering::SeqCst);
            })),
        ));

        pump.write(OutputStream::Stdout, vec![b'x']).unwrap();
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("writer fixture did not block");
        pump.write(OutputStream::Stdout, vec![b'x'; MAX_QUEUED_BYTES])
            .unwrap();
        let pump_for_producer = Arc::clone(&pump);
        let (producer_tx, producer_rx) = std::sync::mpsc::channel();
        let producer = std::thread::spawn(move || {
            let kind = pump_for_producer
                .write(OutputStream::Stdout, vec![b'x'])
                .expect_err("full queue must reject output")
                .kind();
            producer_tx.send(kind).unwrap();
        });

        assert_eq!(
            producer_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("producer remained blocked on a full queue"),
            io::ErrorKind::WouldBlock
        );
        assert!(matches!(
            finished_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        producer.join().unwrap();
        {
            let queue = pump.state.queue.lock().unwrap();
            assert_eq!(queue.accepted_sequence, 2);
            assert_eq!(queue.completed_sequence, 0);
        }
        assert_eq!(failures.load(Ordering::SeqCst), 1);
        assert_eq!(
            pump.write(OutputStream::Stdout, vec![b'x'])
                .expect_err("overflow is sticky")
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(failures.load(Ordering::SeqCst), 1);

        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();
        finished_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first accepted frame did not finish");
        finished_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second accepted frame did not finish");
    }

    #[test]
    fn flush_ignores_frames_accepted_after_its_snapshot() {
        let failures = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failures_for_handler = Arc::clone(&failures);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_for_writer = Arc::clone(&calls);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let pump = Arc::new(OutputPump::with_writer(
            move |_, _| {
                let call = calls_for_writer.fetch_add(1, Ordering::SeqCst);
                entered_tx.send(call).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Ok(())
            },
            Some(Arc::new(move || {
                failures_for_handler.fetch_add(1, Ordering::SeqCst);
            })),
        ));

        pump.write(OutputStream::Stdout, b"A".to_vec()).unwrap();
        assert_eq!(
            entered_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("frame A did not enter the writer"),
            0
        );

        let pump_for_flush = Arc::clone(&pump);
        let (snapshot_tx, snapshot_rx) = std::sync::mpsc::channel();
        let (flush_tx, flush_rx) = std::sync::mpsc::channel();
        let flush = std::thread::spawn(move || {
            let result = pump_for_flush
                .drain_bounded_after_snapshot(Duration::from_secs(1), false, move || {
                    snapshot_tx.send(()).unwrap();
                })
                .map_err(|error| error.kind());
            flush_tx.send(result).unwrap();
        });
        snapshot_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("flush did not capture its sequence snapshot");

        pump.write(OutputStream::Stdout, b"B".to_vec()).unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(
            entered_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("frame B did not enter the writer"),
            1
        );
        assert_eq!(
            flush_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("flush waited for post-snapshot frame B"),
            Ok(())
        );
        assert_eq!(failures.load(Ordering::SeqCst), 0);

        release_tx.send(()).unwrap();
        flush.join().unwrap();
        pump.close_and_drain_bounded().unwrap();
    }

    #[test]
    fn flush_completes_snapshot_despite_later_rejected_write() {
        let failures = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failures_for_handler = Arc::clone(&failures);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_for_writer = Arc::clone(&calls);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let pump = Arc::new(OutputPump::with_writer(
            move |_, _| {
                let call = calls_for_writer.fetch_add(1, Ordering::SeqCst);
                entered_tx.send(call).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Ok(())
            },
            Some(Arc::new(move || {
                failures_for_handler.fetch_add(1, Ordering::SeqCst);
            })),
        ));

        pump.write(OutputStream::Stdout, b"A".to_vec()).unwrap();
        assert_eq!(
            entered_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("frame A did not enter the writer"),
            0
        );
        pump.write(OutputStream::Stdout, vec![b'B'; MAX_QUEUED_BYTES])
            .unwrap();

        let pump_for_flush = Arc::clone(&pump);
        let (snapshot_tx, snapshot_rx) = std::sync::mpsc::channel();
        let (flush_tx, flush_rx) = std::sync::mpsc::channel();
        let flush = std::thread::spawn(move || {
            let result = pump_for_flush
                .drain_bounded_after_snapshot(Duration::from_secs(1), false, move || {
                    snapshot_tx.send(()).unwrap();
                })
                .map_err(|error| error.kind());
            flush_tx.send(result).unwrap();
        });
        snapshot_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("flush did not capture both accepted frames");

        assert_eq!(
            pump.write(OutputStream::Stdout, b"rejected".to_vec())
                .expect_err("full snapshot queue must reject later output")
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(failures.load(Ordering::SeqCst), 1);

        release_tx.send(()).unwrap();
        assert_eq!(
            entered_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("frame B did not enter the writer after frame A completed"),
            1
        );
        assert!(matches!(
            flush_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        release_tx.send(()).unwrap();
        assert_eq!(
            flush_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("flush did not finish after its snapshot completed"),
            Ok(())
        );
        flush.join().unwrap();
    }

    #[test]
    fn sequence_exhaustion_rejects_without_wrapping_or_admission() {
        let failures = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failures_for_handler = Arc::clone(&failures);
        let pump = OutputPump::with_writer(
            |_, _| Ok(()),
            Some(Arc::new(move || {
                failures_for_handler.fetch_add(1, Ordering::SeqCst);
            })),
        );
        {
            let mut queue = pump.state.queue.lock().unwrap();
            queue.accepted_sequence = u64::MAX;
            queue.completed_sequence = u64::MAX;
        }

        let error = pump
            .write(OutputStream::Stdout, b"never admitted".to_vec())
            .expect_err("sequence exhaustion must reject output");
        assert_eq!(error.kind(), io::ErrorKind::Other);
        let queue = pump.state.queue.lock().unwrap();
        assert_eq!(queue.accepted_sequence, u64::MAX);
        assert_eq!(queue.completed_sequence, u64::MAX);
        assert!(queue.frames.is_empty());
        assert_eq!(failures.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failure_callback_can_reenter_without_queue_deadlock() {
        let pump_slot = Arc::new(Mutex::new(None::<std::sync::Weak<OutputPump>>));
        let pump_slot_for_handler = Arc::clone(&pump_slot);
        let (callback_tx, callback_rx) = std::sync::mpsc::channel();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_for_writer = Arc::clone(&calls);
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();

        let pump = Arc::new(OutputPump::with_writer(
            move |_, _| {
                if calls_for_writer.fetch_add(1, Ordering::SeqCst) == 0 {
                    entered_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                } else {
                    finished_tx.send(()).unwrap();
                }
                Ok(())
            },
            Some(Arc::new(move || {
                let pump = pump_slot_for_handler
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(std::sync::Weak::upgrade)
                    .unwrap();
                let kind = pump.close_and_drain_bounded().unwrap_err().kind();
                callback_tx.send(kind).unwrap();
            })),
        ));
        *pump_slot.lock().unwrap() = Some(Arc::downgrade(&pump));

        pump.write(OutputStream::Stdout, vec![b'x']).unwrap();
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("writer fixture did not block");
        pump.write(OutputStream::Stdout, vec![b'x'; MAX_QUEUED_BYTES])
            .unwrap();

        let pump_for_overflow = Arc::clone(&pump);
        let (overflow_tx, overflow_rx) = std::sync::mpsc::channel();
        let overflow_thread = std::thread::spawn(move || {
            let kind = pump_for_overflow
                .write(OutputStream::Stdout, vec![b'x'])
                .unwrap_err()
                .kind();
            overflow_tx.send(kind).unwrap();
        });

        assert_eq!(
            callback_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("reentrant failure callback deadlocked on the queue mutex"),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            overflow_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("overflow write did not return after callback"),
            io::ErrorKind::WouldBlock
        );
        overflow_thread.join().unwrap();

        release_tx.send(()).unwrap();
        finished_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("accepted queued frame was not released");
    }

    #[test]
    fn close_and_drain_delivers_all_accepted_frames_in_order() {
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let delivered_for_writer = Arc::clone(&delivered);
        let pump = OutputPump::with_writer(
            move |stream, bytes| {
                delivered_for_writer
                    .lock()
                    .unwrap()
                    .push((stream, bytes.to_vec()));
                Ok(())
            },
            None,
        );

        pump.write(OutputStream::Stdout, b"first\n".to_vec())
            .unwrap();
        pump.write(OutputStream::Stderr, b"second\n".to_vec())
            .unwrap();
        pump.close_and_drain_bounded().unwrap();

        let queue = pump.state.queue.lock().unwrap();
        assert_eq!(queue.accepted_sequence, 2);
        assert_eq!(queue.completed_sequence, 2);

        assert_eq!(
            delivered.lock().unwrap().as_slice(),
            &[
                (OutputStream::Stdout, b"first\n".to_vec()),
                (OutputStream::Stderr, b"second\n".to_vec()),
            ]
        );
    }

    #[test]
    fn close_and_drain_times_out_when_write_is_blocked() {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let pump = OutputPump::with_writer(
            move |_, _| {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Ok(())
            },
            None,
        );

        pump.write(OutputStream::Stdout, b"blocked\n".to_vec())
            .unwrap();
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("writer fixture did not block");
        let error = pump
            .drain_bounded(Duration::from_millis(20), true)
            .expect_err("blocked writer must exceed the drain budget");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);

        release_tx.send(()).unwrap();
    }

    #[test]
    fn close_and_drain_reports_late_write_failure() {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let pump = OutputPump::with_writer(
            move |_, _| {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture failure"))
            },
            None,
        );

        pump.write(OutputStream::Stdout, b"accepted\n".to_vec())
            .expect("queue admission succeeds before the asynchronous write");
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("writer fixture did not start");
        release_tx.send(()).unwrap();

        let error = pump
            .close_and_drain_bounded()
            .expect_err("late writer failure must be observable at drain");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        let queue = pump.state.queue.lock().unwrap();
        assert_eq!(queue.accepted_sequence, 1);
        assert_eq!(queue.completed_sequence, 0);
    }

    #[test]
    fn writer_failure_callback_runs_once() {
        let failures = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failures_for_handler = Arc::clone(&failures);
        let (failed_tx, failed_rx) = std::sync::mpsc::channel();
        let pump = OutputPump::with_writer(
            |_, _| Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture failure")),
            Some(Arc::new(move || {
                failures_for_handler.fetch_add(1, Ordering::SeqCst);
                let _ = failed_tx.send(());
            })),
        );

        pump.write(OutputStream::Stdout, vec![b'x']).unwrap();
        failed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("writer failure callback was not invoked");
        assert_eq!(failures.load(Ordering::SeqCst), 1);
        assert_eq!(
            pump.write(OutputStream::Stderr, vec![b'x'])
                .expect_err("writer failure must be sticky")
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(failures.load(Ordering::SeqCst), 1);
    }
}
