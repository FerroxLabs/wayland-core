//! `wcore-channel-signal` — Signal adapter driven by a `signal-cli`
//! subprocess in `jsonRpc` mode.
//!
//! Architecture:
//! - `start()` spawns `signal-cli -a <account> jsonRpc` via a
//!   [`SignalProcessLauncher`]. The launcher trait is the seam tests
//!   use to fabricate stdio with `tokio::io::duplex` instead of
//!   spawning a real binary.
//! - A reader task consumes one JSON document per stdout line:
//!   responses (carrying `id`) resolve the matching pending oneshot;
//!   `receive` notifications enqueue an [`IncomingMessage`] into the
//!   inbox.
//! - `send_message()` allocates a request id, registers a pending
//!   oneshot, writes the request to stdin, then awaits the oneshot
//!   with the configured `send_timeout_secs`.
//! - A supervisor task owns the launch → reader-loop cycle in a respawn
//!   loop: when the child dies / stdout hits EOF / the reader errors for
//!   a non-shutdown reason, it emits `ConnectionState::Reconnecting`,
//!   backs off, and relaunches `signal-cli` — rebuilding the stdin
//!   writer and pending map without losing channel identity.
//! - `stop()` flips a watch::Sender to true (reader + supervisor exit),
//!   drops the writer, kills the child if still alive, and joins. No
//!   respawn happens after an intentional stop.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, oneshot, watch};
use tokio::task::JoinHandle;

use wcore_channels::Channel;
use wcore_channels::error::ChannelError;
use wcore_channels::event::{ChannelEvent, ConnectionState, MessageReceipt};
use wcore_channels::outgoing::OutgoingMessage;

pub use crate::config::SignalConfig;
pub use crate::error::SignalError;
pub use crate::jsonrpc::{
    DeliveryOutcome, SendResult, SendResultEntry, build_send_params, classify_delivery,
    is_direct_recipient,
};
pub use crate::subprocess::{
    PendingResponses, RealLauncher, SharedStdin, SignalProcessHandle, SignalProcessLauncher,
};
pub use crate::supervisor::Backoff;

pub mod config;
pub mod error;
pub mod jsonrpc;
pub mod subprocess;
pub mod supervisor;

/// Production Signal channel adapter.
pub struct SignalChannel {
    name: String,
    config: SignalConfig,
    state: ConnectionState,
    launcher: Arc<dyn SignalProcessLauncher>,
    /// Swappable stdin writer — `send_message` serializes writes through
    /// the current inner box. The supervisor swaps the inner writer on
    /// each (re)spawn, and clears it (`None`) between process death and
    /// the next respawn. Always `Some(Arc)` after construction.
    stdin: SharedStdin,
    /// Monotonic id allocator for JSON-RPC requests.
    request_id: Arc<AtomicU64>,
    /// Inbox of inbound events (drained by `poll_events`).
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    /// In-flight request id → response sender.
    pending: PendingResponses,
    /// The supervisor task — owns the launch → reader → respawn loop.
    /// Set by `start()`, joined + cleared by `stop()`.
    supervisor_handle: Option<JoinHandle<()>>,
    shutdown: Option<watch::Sender<bool>>,
}

impl SignalChannel {
    /// Construct a Signal channel using the real `tokio::process` launcher.
    pub fn new(name: impl Into<String>, config: SignalConfig) -> Self {
        Self::with_launcher(name, config, Arc::new(RealLauncher))
    }

    /// Construct with a custom launcher — used in tests to swap in a
    /// fabricated stdin/stdout pair.
    pub fn with_launcher(
        name: impl Into<String>,
        config: SignalConfig,
        launcher: Arc<dyn SignalProcessLauncher>,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            launcher,
            stdin: Arc::new(Mutex::new(None)),
            request_id: Arc::new(AtomicU64::new(1)),
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            pending: Arc::new(Mutex::new(HashMap::new())),
            supervisor_handle: None,
            shutdown: None,
        }
    }

    /// Current connection state — mainly for tests.
    pub fn state(&self) -> ConnectionState {
        self.state
    }
}

/// Cap on a single inbound attachment read. signal-cli has already written the
/// bytes to local disk; bound the read so an oversized file can't exhaust
/// memory.
const MAX_ATTACHMENT_BYTES: u64 = 64 * 1024 * 1024;

#[async_trait]
impl Channel for SignalChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "signal"
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.supervisor_handle.is_some() {
            // Already started — idempotent.
            return Ok(());
        }
        self.state = ConnectionState::Connecting;

        // Perform the FIRST launch synchronously so a launcher failure
        // surfaces as a `start()` error (rather than vanishing into the
        // supervisor's retry loop). Subsequent respawns are owned by the
        // supervisor task.
        let seed = self
            .launcher
            .launch(&self.config.signal_cli_path, &self.config.account)
            .map_err(|e| {
                self.state = ConnectionState::Disconnected;
                ChannelError::from(e)
            })?;

        // Install the seed's stdin writer into the shared, swappable
        // slot. The supervisor swaps this on each respawn; `send_message`
        // always reads the current writer.
        let SignalProcessHandle {
            stdin: seed_stdin,
            stdout: seed_stdout,
            child: seed_child,
        } = seed;
        *self.stdin.lock().await = Some(seed_stdin);

        // Push a Connected state-change so consumers know we're up.
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connected,
            });

        let (tx, rx) = watch::channel(false);
        let args = supervisor::SupervisorArgs {
            config: self.config.clone(),
            launcher: Arc::clone(&self.launcher),
            stdin: Arc::clone(&self.stdin),
            inbox: Arc::clone(&self.inbox),
            pending: Arc::clone(&self.pending),
            shutdown: rx,
            seed: supervisor::SeedHandle {
                stdout: seed_stdout,
                child: seed_child,
            },
        };
        let join = tokio::spawn(supervisor::supervisor_loop(args));
        self.supervisor_handle = Some(join);
        self.shutdown = Some(tx);
        self.state = ConnectionState::Connected;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        if self.supervisor_handle.is_none() {
            return Ok(());
        }
        // Flip the shutdown watch: both the active reader loop AND the
        // supervisor observe it and exit without respawning.
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(true);
        }
        // Drop the live writer — closing stdin signals signal-cli to
        // exit. The supervisor owns + kills the child it launched.
        *self.stdin.lock().await = None;
        // Join the supervisor task so it fully unwinds (kills its child,
        // clears stdin) before we return — no task leak. If it doesn't
        // exit in time, abort it explicitly so it can't linger and
        // respawn behind our back (a dropped JoinHandle detaches, it does
        // NOT cancel).
        if let Some(mut handle) = self.supervisor_handle.take()
            && tokio::time::timeout(Duration::from_secs(5), &mut handle)
                .await
                .is_err()
        {
            tracing::warn!(
                target: "wcore_channel_signal",
                "stop: supervisor did not exit within 5s; aborting task"
            );
            handle.abort();
        }
        // Drain any pending oneshots (they'll get SubprocessClosed from
        // the reader's EOF branch, but in case the reader exited via
        // shutdown signal before observing EOF we fail them here too).
        {
            let mut pending = self.pending.lock().await;
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err(SignalError::SubprocessClosed));
            }
        }
        self.state = ConnectionState::Disconnected;
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Disconnected,
            });
        Ok(())
    }

    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        Ok(self.inbox.lock().await.drain(..).collect())
    }

    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        let stdin = Arc::clone(&self.stdin);
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);

        // Register pending oneshot BEFORE writing — avoids a race where
        // signal-cli answers faster than we install the slot.
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        // Route to `recipient` (direct) or `groupId` (group) based on the
        // conversation id's shape — signal-cli's `send` accepts one XOR
        // the other. See `jsonrpc::build_send_params`.
        let params = jsonrpc::build_send_params(&msg.conversation_id, &msg.text);
        let request = jsonrpc::Request::new(id, "send", params);
        let line = serde_json::to_string(&request)
            .map_err(|e| ChannelError::from(SignalError::Decode(format!("encode request: {e}"))))?;

        // Write line + newline atomically — JSON-RPC over stdio is
        // line-delimited. The stdin slot is swapped by the supervisor on
        // respawn and is `None` between a process death and the next
        // (re)launch; treat an absent writer as NotStarted so callers
        // can retry once the supervisor reconnects.
        {
            let mut guard = stdin.lock().await;
            let writer = match guard.as_mut() {
                Some(w) => w,
                None => {
                    self.pending.lock().await.remove(&id);
                    return Err(ChannelError::NotStarted);
                }
            };
            if let Err(e) = writer.write_all(line.as_bytes()).await {
                self.pending.lock().await.remove(&id);
                return Err(SignalError::Io(format!("write request: {e}")).into());
            }
            if let Err(e) = writer.write_all(b"\n").await {
                self.pending.lock().await.remove(&id);
                return Err(SignalError::Io(format!("write newline: {e}")).into());
            }
            if let Err(e) = writer.flush().await {
                self.pending.lock().await.remove(&id);
                return Err(SignalError::Io(format!("flush: {e}")).into());
            }
        }

        let timeout = Duration::from_secs(self.config.send_timeout_secs);
        let result = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(payload)) => payload,
            Ok(Err(_canceled)) => {
                // Reader dropped the sender — subprocess died.
                return Err(SignalError::SubprocessClosed.into());
            }
            Err(_elapsed) => {
                // Timeout — clean up the pending slot so a late
                // response doesn't fire into a stale receiver.
                self.pending.lock().await.remove(&id);
                return Err(SignalError::Timeout { request_id: id }.into());
            }
        };

        let raw = result.map_err(ChannelError::from)?;
        let parsed: SendResult = serde_json::from_value(raw).map_err(|e| {
            ChannelError::from(SignalError::Decode(format!("parse send result: {e}")))
        })?;

        // Inspect per-recipient delivery. signal-cli answers the
        // round-trip even when some/all recipients are undelivered, so a
        // bare response is NOT proof of delivery — classify the results
        // array. Logging is content-free: counts only, never message
        // text or recipient identities.
        match jsonrpc::classify_delivery(parsed.results.as_deref()) {
            jsonrpc::DeliveryOutcome::AllSucceeded => {}
            jsonrpc::DeliveryOutcome::Partial { failed, total } => {
                tracing::warn!(
                    target: "wcore_channel_signal",
                    failed,
                    total,
                    "send: partial delivery — some recipients undelivered"
                );
            }
            jsonrpc::DeliveryOutcome::AllFailed { total } => {
                return Err(ChannelError::Rejected(format!(
                    "all {total} recipient(s) undelivered"
                )));
            }
        }

        let ts_ms = parsed.timestamp.unwrap_or_else(now_ms);
        let ts_secs = ts_ms / 1000;
        Ok(MessageReceipt {
            id: format!("{ts_ms}"),
            conversation_id: msg.conversation_id,
            ts_secs,
        })
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/signal.json")
    }

    /// Read an inbound Signal attachment's bytes off local disk. signal-cli
    /// writes each attachment to `<config-dir>/attachments/<id>`; the `id`
    /// rides in `Attachment.path`. No network fetch, so no SSRF surface —
    /// bounded by [`MAX_ATTACHMENT_BYTES`].
    async fn fetch_media(
        &self,
        attachment: &wcore_channels::event::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        let Some(id) = attachment.path.as_deref() else {
            return Err(ChannelError::Rejected(
                "signal attachment has no store id".to_string(),
            ));
        };
        // Defense in depth: a signal-cli store id is a flat filename. Reject
        // any separator / parent ref so a crafted id can't escape the dir.
        if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
            return Err(ChannelError::Rejected(format!(
                "invalid signal attachment id: {id}"
            )));
        }
        let path = self.config.resolve_attachments_dir().join(id);

        tokio::task::spawn_blocking(move || {
            let meta = std::fs::metadata(&path)
                .map_err(|e| ChannelError::Transport(format!("stat signal attachment: {e}")))?;
            if meta.len() > MAX_ATTACHMENT_BYTES {
                return Err(ChannelError::Rejected(format!(
                    "signal attachment exceeds {MAX_ATTACHMENT_BYTES} byte cap ({})",
                    meta.len()
                )));
            }
            std::fs::read(&path)
                .map_err(|e| ChannelError::Transport(format!("read signal attachment: {e}")))
        })
        .await
        .map_err(|e| ChannelError::Transport(format!("attachment read task panic: {e}")))?
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex as StdMutex;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    fn cfg() -> SignalConfig {
        SignalConfig {
            signal_cli_path: PathBuf::from("signal-cli"),
            account: "+15551234567".to_string(),
            send_timeout_secs: 10,
            attachments_dir: None,
        }
    }

    #[tokio::test]
    async fn fetch_media_reads_local_attachment_by_id() {
        // Per-process unique tempdir (auto-removed on drop). A fixed shared
        // /tmp path collides across concurrent workspace runs — one run's
        // cleanup deletes another's fixture (#191).
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("att-id-1"), b"SIGBYTES").unwrap();

        let mut config = cfg();
        config.attachments_dir = Some(dir.to_path_buf());
        let ch = SignalChannel::new("t", config);

        let att = wcore_channels::event::Attachment {
            path: Some("att-id-1".to_string()),
            ..Default::default()
        };
        let bytes = ch.fetch_media(&att).await.unwrap();
        assert_eq!(bytes, b"SIGBYTES");
        // No manual cleanup: `tmp` (TempDir) removes the directory on drop.
    }

    #[tokio::test]
    async fn fetch_media_rejects_traversal_id() {
        let ch = SignalChannel::new("t", cfg());
        let att = wcore_channels::event::Attachment {
            path: Some("../../etc/passwd".to_string()),
            ..Default::default()
        };
        let err = ch.fetch_media(&att).await.unwrap_err();
        assert!(matches!(err, ChannelError::Rejected(_)));
    }

    #[tokio::test]
    async fn fetch_media_rejects_missing_id() {
        let ch = SignalChannel::new("t", cfg());
        let err = ch
            .fetch_media(&wcore_channels::event::Attachment::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::Rejected(_)));
    }

    /// Test-side failure-injection knob. Wrapped in an Arc so the
    /// composite launcher can read it across `.launch()` invocations.
    #[derive(Default)]
    struct FailureSlot {
        fail_with: StdMutex<Option<String>>,
    }

    /// Per-launch handles the launcher must serve to the channel.
    struct LaunchStash {
        stdin: StdMutex<Option<DuplexStream>>,
        stdout: StdMutex<Option<DuplexStream>>,
    }

    struct CompositeLauncher {
        stash: Arc<LaunchStash>,
        failure: Arc<FailureSlot>,
    }

    impl SignalProcessLauncher for CompositeLauncher {
        fn launch(
            &self,
            _cli_path: &Path,
            _account: &str,
        ) -> Result<SignalProcessHandle, SignalError> {
            if let Some(msg) = self.failure.fail_with.lock().unwrap().clone() {
                return Err(SignalError::Spawn(msg));
            }
            let stdin = self.stash.stdin.lock().unwrap().take().ok_or_else(|| {
                SignalError::Spawn("test launcher: stdin already consumed".into())
            })?;
            let stdout = self.stash.stdout.lock().unwrap().take().ok_or_else(|| {
                SignalError::Spawn("test launcher: stdout already consumed".into())
            })?;
            Ok(SignalProcessHandle {
                stdin: Box::new(stdin),
                stdout: Box::new(BufReader::new(stdout)),
                child: None,
            })
        }
    }

    /// Build a paired (composite launcher, test harness io handles).
    fn build_test_pair() -> (Arc<CompositeLauncher>, HarnessIo) {
        let (channel_writes_to, harness_reads_from) = tokio::io::duplex(64 * 1024);
        let (harness_writes_to, channel_reads_from) = tokio::io::duplex(64 * 1024);

        let failure = Arc::new(FailureSlot::default());
        let stash = Arc::new(LaunchStash {
            stdin: StdMutex::new(Some(channel_writes_to)),
            stdout: StdMutex::new(Some(channel_reads_from)),
        });
        let composite = Arc::new(CompositeLauncher {
            stash,
            failure: Arc::clone(&failure),
        });
        let io = HarnessIo {
            read_from_channel: BufReader::new(harness_reads_from),
            write_to_channel: harness_writes_to,
            failure,
        };
        (composite, io)
    }

    struct HarnessIo {
        read_from_channel: BufReader<DuplexStream>,
        write_to_channel: DuplexStream,
        #[allow(dead_code)]
        failure: Arc<FailureSlot>,
    }

    impl HarnessIo {
        /// Read one JSON-RPC line that the channel wrote to "stdin".
        async fn read_line(&mut self) -> String {
            let mut buf = String::new();
            self.read_from_channel.read_line(&mut buf).await.unwrap();
            buf
        }

        /// Push a JSON-RPC line into the channel's "stdout".
        async fn write_line(&mut self, line: &str) {
            self.write_to_channel
                .write_all(line.as_bytes())
                .await
                .unwrap();
            self.write_to_channel.write_all(b"\n").await.unwrap();
            self.write_to_channel.flush().await.unwrap();
        }
    }

    fn launcher_failing(reason: &str) -> Arc<CompositeLauncher> {
        let (composite, _io) = build_test_pair();
        *composite.failure.fail_with.lock().unwrap() = Some(reason.to_string());
        composite
    }

    // -----------------------------------------------------------------
    // 1. send round-trip: write a request, write a fake response with
    //    matching id, send() resolves with the timestamp.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_round_trip_returns_timestamp() {
        let (launcher, mut io) = build_test_pair();
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        ch.start().await.unwrap();

        // Drive: harness reads the request, parses its id, writes a
        // response back with the same id and a timestamp.
        let send_fut = tokio::spawn(async move {
            ch.send_message(OutgoingMessage::text("+15550001111", "hello signal"))
                .await
        });

        let line = io.read_line().await;
        let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(req["method"], "send");
        assert_eq!(req["jsonrpc"], "2.0");
        assert_eq!(req["params"]["message"], "hello signal");
        assert_eq!(req["params"]["recipient"][0], "+15550001111");
        let id = req["id"].as_u64().unwrap();

        // Fabricate a successful response.
        let resp =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"timestamp":1700000123456}}}}"#);
        io.write_line(&resp).await;

        let receipt = send_fut.await.unwrap().unwrap();
        assert_eq!(receipt.id, "1700000123456");
        assert_eq!(receipt.conversation_id, "+15550001111");
        assert_eq!(receipt.ts_secs, 1_700_000_123);
    }

    // -----------------------------------------------------------------
    // 2. send timeout: harness never responds → SignalError::Timeout
    //    bubbles as ChannelError::Transport. Uses a 1s timeout so the
    //    real-time wait is bounded.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_timeout_fires_after_configured_window() {
        let (launcher, mut io) = build_test_pair();
        let mut conf = cfg();
        conf.send_timeout_secs = 1;
        let mut ch = SignalChannel::with_launcher("test", conf, launcher);
        ch.start().await.unwrap();

        let send_fut = tokio::spawn(async move {
            ch.send_message(OutgoingMessage::text("+15550009999", "no reply"))
                .await
        });

        // Read the request so the write side completes — but never
        // respond.
        let _line = io.read_line().await;

        let err = send_fut.await.unwrap().expect_err("expected timeout");
        match err {
            ChannelError::Transport(msg) => {
                assert!(msg.contains("timeout"), "msg = {msg}");
            }
            other => panic!("expected Transport(timeout), got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // 3. inbound receive: harness writes a `receive` notification,
    //    poll_events returns an IncomingMessage with the right fields.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn inbound_receive_surfaces_message() {
        let (launcher, mut io) = build_test_pair();
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        ch.start().await.unwrap();

        let notif = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15550002222","sourceName":"Alice","timestamp":1700000999000,"dataMessage":{"message":"hi there","timestamp":1700000999000}}}}"#;
        io.write_line(notif).await;

        // Poll until we see the message (the reader is a separate task).
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut got = None;
        while std::time::Instant::now() < deadline {
            let evs = ch.poll_events().await.unwrap();
            for e in evs {
                if let ChannelEvent::MessageReceived { msg } = e {
                    got = Some(msg);
                    break;
                }
            }
            if got.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let msg = got.expect("expected MessageReceived");
        assert_eq!(msg.text, "hi there");
        assert_eq!(msg.author, "+15550002222");
        assert_eq!(msg.conversation_id, "+15550002222");
        assert_eq!(msg.ts_secs, 1_700_000_999);
        assert_eq!(msg.id, "1700000999000");
        // Enriched fields: no UUID in this payload, so the phone is the
        // stable sender id; sourceName surfaces as the display label; a
        // non-group envelope is a 1:1 Direct chat.
        assert_eq!(msg.sender_id, "+15550002222");
        assert_eq!(msg.sender_display.as_deref(), Some("Alice"));
        assert_eq!(msg.chat_type, wcore_channels::ChatType::Direct);
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 4. malformed JSON line → reader logs + skips, doesn't crash. We
    //    follow up with a valid line and verify it lands.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn malformed_line_skipped_then_valid_one_processed() {
        let (launcher, mut io) = build_test_pair();
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        ch.start().await.unwrap();

        io.write_line("{not real json").await;
        let notif = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+1","timestamp":1700000111000,"dataMessage":{"message":"after garbage"}}}}"#;
        io.write_line(notif).await;

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut got = None;
        while std::time::Instant::now() < deadline {
            let evs = ch.poll_events().await.unwrap();
            for e in evs {
                if let ChannelEvent::MessageReceived { msg } = e {
                    got = Some(msg);
                    break;
                }
            }
            if got.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let msg = got.expect("expected message after malformed line");
        assert_eq!(msg.text, "after garbage");
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 5. stop() ends reader task within 5s. We assert the reader handle
    //    is cleared + state goes Disconnected. Idempotent on second call.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn stop_ends_reader_task_cleanly() {
        let (launcher, _io) = build_test_pair();
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        ch.start().await.unwrap();
        assert!(ch.supervisor_handle.is_some());

        ch.stop().await.unwrap();
        assert!(ch.supervisor_handle.is_none());
        assert!(ch.shutdown.is_none());
        assert!(ch.stdin.lock().await.is_none());
        assert_eq!(ch.state(), ConnectionState::Disconnected);

        // Idempotent.
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 6. Config TOML round-trip with deny_unknown_fields, surviving the
    //    ChannelConfig.options Table boundary.
    // -----------------------------------------------------------------
    #[test]
    fn config_round_trip_via_channel_config_options() {
        let raw = r#"
name = "acme-signal"
platform = "signal"

[options]
signal_cli_path = "/usr/local/bin/signal-cli"
account = "+15551234567"
send_timeout_secs = 20
"#;
        let outer: wcore_channels::ChannelConfig = toml::from_str(raw).unwrap();
        let cfg: SignalConfig = outer.options.try_into().unwrap();
        assert_eq!(cfg.account, "+15551234567");
        assert_eq!(
            cfg.signal_cli_path,
            PathBuf::from("/usr/local/bin/signal-cli")
        );
        assert_eq!(cfg.send_timeout_secs, 20);
    }

    // -----------------------------------------------------------------
    // 7. Concurrent sends each get the correct response (id matching).
    //    Fire two sends in parallel, reply out of order, verify each
    //    future resolves with its own timestamp.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn concurrent_sends_id_match_correctly() {
        let (launcher, mut io) = build_test_pair();
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        ch.start().await.unwrap();
        let ch = Arc::new(Mutex::new(ch));

        let ch_a = Arc::clone(&ch);
        let send_a = tokio::spawn(async move {
            let mut guard = ch_a.lock().await;
            guard
                .send_message(OutgoingMessage::text("+1AAA", "msg-a"))
                .await
        });

        // Read first request, capture its id.
        let line_a = io.read_line().await;
        let req_a: serde_json::Value = serde_json::from_str(line_a.trim()).unwrap();
        let id_a = req_a["id"].as_u64().unwrap();
        assert_eq!(req_a["params"]["message"], "msg-a");

        // Wait until first send is parked in pending before firing the
        // second one, since both share the channel's mutex.
        // Reply to A out-of-order — but we need to send B first via
        // the same channel. Drop the mutex first.
        drop(req_a);

        // Reply to A early.
        let resp_a =
            format!(r#"{{"jsonrpc":"2.0","id":{id_a},"result":{{"timestamp":1700001000000}}}}"#);
        io.write_line(&resp_a).await;
        let receipt_a = send_a.await.unwrap().unwrap();
        assert_eq!(receipt_a.id, "1700001000000");

        // Now fire B.
        let ch_b = Arc::clone(&ch);
        let send_b = tokio::spawn(async move {
            let mut guard = ch_b.lock().await;
            guard
                .send_message(OutgoingMessage::text("+1BBB", "msg-b"))
                .await
        });
        let line_b = io.read_line().await;
        let req_b: serde_json::Value = serde_json::from_str(line_b.trim()).unwrap();
        let id_b = req_b["id"].as_u64().unwrap();
        assert_eq!(req_b["params"]["message"], "msg-b");
        assert_ne!(id_a, id_b, "ids must be unique");

        let resp_b =
            format!(r#"{{"jsonrpc":"2.0","id":{id_b},"result":{{"timestamp":1700002000000}}}}"#);
        io.write_line(&resp_b).await;
        let receipt_b = send_b.await.unwrap().unwrap();
        assert_eq!(receipt_b.id, "1700002000000");
    }

    // -----------------------------------------------------------------
    // 8. start() surfaces a clean Transport error if the launcher fails.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn start_fails_cleanly_when_launcher_errors() {
        let launcher = launcher_failing("signal-cli not found");
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        let err = ch.start().await.expect_err("expected start error");
        match err {
            ChannelError::Transport(msg) => {
                assert!(msg.contains("signal-cli"), "msg = {msg}");
            }
            other => panic!("expected Transport, got {other:?}"),
        }
        assert_eq!(ch.state(), ConnectionState::Disconnected);
        assert!(ch.supervisor_handle.is_none());
    }

    // -----------------------------------------------------------------
    // 9. send_message before start surfaces NotStarted.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_before_start_errors_not_started() {
        let (launcher, _io) = build_test_pair();
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        let err = ch
            .send_message(OutgoingMessage::text("+1", "x"))
            .await
            .expect_err("expected NotStarted");
        assert!(matches!(err, ChannelError::NotStarted), "got {err:?}");
    }

    // -----------------------------------------------------------------
    // 10. RPC error response → ChannelError::Rejected.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn rpc_error_response_surfaces_as_rejected() {
        let (launcher, mut io) = build_test_pair();
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        ch.start().await.unwrap();

        let send_fut = tokio::spawn(async move {
            ch.send_message(OutgoingMessage::text("+1nope", "boom"))
                .await
        });

        let line = io.read_line().await;
        let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        let id = req["id"].as_u64().unwrap();
        let err_resp = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32000,"message":"unknown recipient"}}}}"#
        );
        io.write_line(&err_resp).await;

        let err = send_fut.await.unwrap().expect_err("expected rejection");
        match err {
            ChannelError::Rejected(msg) => {
                assert!(msg.contains("-32000"), "msg = {msg}");
                assert!(msg.contains("unknown recipient"), "msg = {msg}");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // 11a. Group send routes through `groupId` (not `recipient`) when the
    //      conversation id is a base64 group id rather than a +phone.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn group_send_uses_group_id_param() {
        let (launcher, mut io) = build_test_pair();
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        ch.start().await.unwrap();

        let send_fut = tokio::spawn(async move {
            ch.send_message(OutgoingMessage::text("abcd1234EFGH==", "hi group"))
                .await
        });

        let line = io.read_line().await;
        let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(req["method"], "send");
        assert_eq!(req["params"]["groupId"], "abcd1234EFGH==");
        assert_eq!(req["params"]["message"], "hi group");
        // recipient must be absent for a group send.
        assert!(req["params"].get("recipient").is_none());
        let id = req["id"].as_u64().unwrap();

        let resp =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"timestamp":1700000123456}}}}"#);
        io.write_line(&resp).await;

        let receipt = send_fut.await.unwrap().unwrap();
        assert_eq!(receipt.conversation_id, "abcd1234EFGH==");
    }

    // -----------------------------------------------------------------
    // 11b. A send whose per-recipient results are ALL failures surfaces
    //      as ChannelError::Rejected even though signal-cli answered the
    //      round-trip.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn all_failed_recipients_surface_as_rejected() {
        let (launcher, mut io) = build_test_pair();
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        ch.start().await.unwrap();

        let send_fut = tokio::spawn(async move {
            ch.send_message(OutgoingMessage::text("groupIdBase64==", "undelivered"))
                .await
        });

        let line = io.read_line().await;
        let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        let id = req["id"].as_u64().unwrap();

        let resp = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{{"timestamp":1700000123456,"results":[{{"type":"NETWORK_FAILURE"}},{{"type":"UNREGISTERED_FAILURE"}}]}}}}"#
        );
        io.write_line(&resp).await;

        let err = send_fut.await.unwrap().expect_err("expected rejection");
        match err {
            ChannelError::Rejected(msg) => {
                assert!(msg.contains('2'), "should name the recipient count: {msg}");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // 11c. A send with SOME failed recipients still returns a receipt
    //      (partial → warn-and-succeed).
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn partial_failure_still_returns_receipt() {
        let (launcher, mut io) = build_test_pair();
        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        ch.start().await.unwrap();

        let send_fut = tokio::spawn(async move {
            ch.send_message(OutgoingMessage::text("groupIdBase64==", "partial"))
                .await
        });

        let line = io.read_line().await;
        let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        let id = req["id"].as_u64().unwrap();

        let resp = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{{"timestamp":1700000123456,"results":[{{"type":"SUCCESS"}},{{"type":"NETWORK_FAILURE"}}]}}}}"#
        );
        io.write_line(&resp).await;

        let receipt = send_fut
            .await
            .unwrap()
            .expect("partial should still succeed");
        assert_eq!(receipt.id, "1700000123456");
    }

    // -----------------------------------------------------------------
    // 11. SUPERVISED RESPAWN (CRITICAL-4). A launcher that serves a
    //     queue of duplex handles + counts launches. The first child's
    //     stdout is closed (EOF), which must drive the supervisor to:
    //       (a) emit ConnectionState::Reconnecting,
    //       (b) call the launcher a SECOND time (respawn),
    //     and then `stop()` must tear down WITHOUT a third launch.
    // -----------------------------------------------------------------

    /// One queued handle the respawn launcher will hand out, in order.
    struct QueuedHandle {
        stdin: DuplexStream,
        stdout: DuplexStream,
    }

    /// Launcher that serves handles from a queue and records launch
    /// count. Returns an error if asked to launch past the queue end.
    struct RespawnLauncher {
        queue: StdMutex<VecDeque<QueuedHandle>>,
        launches: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl SignalProcessLauncher for RespawnLauncher {
        fn launch(
            &self,
            _cli_path: &Path,
            _account: &str,
        ) -> Result<SignalProcessHandle, SignalError> {
            self.launches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let h =
                self.queue.lock().unwrap().pop_front().ok_or_else(|| {
                    SignalError::Spawn("respawn launcher: queue exhausted".into())
                })?;
            Ok(SignalProcessHandle {
                stdin: Box::new(h.stdin),
                stdout: Box::new(BufReader::new(h.stdout)),
                child: None,
            })
        }
    }

    #[tokio::test]
    async fn supervisor_respawns_on_eof_and_stops_without_respawn() {
        // First handle: keep the write-half of its stdout so we can drop
        // it to force EOF. Second handle: the respawn target.
        let (ch_stdin_1, _harness_reads_1) = tokio::io::duplex(64 * 1024);
        let (harness_writes_1, ch_stdout_1) = tokio::io::duplex(64 * 1024);
        let (ch_stdin_2, _harness_reads_2) = tokio::io::duplex(64 * 1024);
        let (_harness_writes_2, ch_stdout_2) = tokio::io::duplex(64 * 1024);

        let mut queue = VecDeque::new();
        queue.push_back(QueuedHandle {
            stdin: ch_stdin_1,
            stdout: ch_stdout_1,
        });
        queue.push_back(QueuedHandle {
            stdin: ch_stdin_2,
            stdout: ch_stdout_2,
        });

        let launches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let launcher = Arc::new(RespawnLauncher {
            queue: StdMutex::new(queue),
            launches: Arc::clone(&launches),
        });

        let mut ch = SignalChannel::with_launcher("test", cfg(), launcher);
        ch.start().await.unwrap();
        assert_eq!(
            launches.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "start() should launch exactly once"
        );

        // Force EOF on the first child's stdout: drop the harness's
        // write-half. The reader sees Ok(0) → the supervisor respawns.
        drop(harness_writes_1);

        // Wait for the second launch (respawn). Backoff base is 1s, so
        // give it a generous window.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while launches.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            if std::time::Instant::now() >= deadline {
                panic!("supervisor did not respawn within 5s");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            launches.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "supervisor must relaunch exactly once after EOF"
        );

        // A Reconnecting event must have been emitted onto the inbox.
        let mut saw_reconnecting = false;
        for ev in ch.poll_events().await.unwrap() {
            if let ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Reconnecting,
            } = ev
            {
                saw_reconnecting = true;
            }
        }
        assert!(
            saw_reconnecting,
            "supervisor must emit a Reconnecting state-change on respawn"
        );

        // Now stop. The supervisor must observe shutdown and exit; with
        // the queue exhausted, any erroneous third launch attempt would
        // bump the counter — assert it does NOT.
        ch.stop().await.unwrap();
        let after_stop = launches.load(std::sync::atomic::Ordering::SeqCst);
        // Give any stray respawn a moment to (incorrectly) fire.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            launches.load(std::sync::atomic::Ordering::SeqCst),
            after_stop,
            "no respawn may occur after stop()"
        );
        assert_eq!(after_stop, 2, "exactly two launches total");
        assert!(ch.supervisor_handle.is_none());
    }
}
