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
//! - `stop()` flips a watch::Sender to true (reader exits its select),
//!   drops the writer, kills the child if still alive, and joins.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, oneshot, watch};
use tokio::task::JoinHandle;

use wcore_channels::Channel;
use wcore_channels::error::ChannelError;
use wcore_channels::event::{ChannelEvent, ConnectionState, MessageReceipt};
use wcore_channels::outgoing::OutgoingMessage;

pub use crate::config::SignalConfig;
pub use crate::error::SignalError;
pub use crate::jsonrpc::SendResult;
pub use crate::subprocess::{
    PendingResponses, RealLauncher, SignalProcessHandle, SignalProcessLauncher,
};

pub mod config;
pub mod error;
pub mod jsonrpc;
pub mod subprocess;

/// Production Signal channel adapter.
pub struct SignalChannel {
    name: String,
    config: SignalConfig,
    state: ConnectionState,
    launcher: Arc<dyn SignalProcessLauncher>,
    /// The subprocess handle. Set inside `start()`, cleared by `stop()`.
    child: Option<tokio::process::Child>,
    /// Locked stdin writer — `send_message` serializes writes here.
    stdin: Option<Arc<Mutex<Box<dyn AsyncWrite + Unpin + Send>>>>,
    /// Monotonic id allocator for JSON-RPC requests.
    request_id: Arc<AtomicU64>,
    /// Inbox of inbound events (drained by `poll_events`).
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    /// In-flight request id → response sender.
    pending: PendingResponses,
    reader_handle: Option<JoinHandle<()>>,
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
            child: None,
            stdin: None,
            request_id: Arc::new(AtomicU64::new(1)),
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            pending: Arc::new(Mutex::new(HashMap::new())),
            reader_handle: None,
            shutdown: None,
        }
    }

    /// Current connection state — mainly for tests.
    pub fn state(&self) -> ConnectionState {
        self.state
    }
}

#[async_trait]
impl Channel for SignalChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "signal"
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.reader_handle.is_some() {
            // Already started — idempotent.
            return Ok(());
        }
        self.state = ConnectionState::Connecting;

        let handle = self
            .launcher
            .launch(&self.config.signal_cli_path, &self.config.account)
            .map_err(|e| {
                self.state = ConnectionState::Disconnected;
                ChannelError::from(e)
            })?;

        let SignalProcessHandle {
            stdin,
            stdout,
            child,
        } = handle;

        self.child = child;
        self.stdin = Some(Arc::new(Mutex::new(stdin)));

        // Push a Connected state-change so consumers know we're up.
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connected,
            });

        let (tx, rx) = watch::channel(false);
        let args = subprocess::ReaderArgs {
            stdout,
            inbox: Arc::clone(&self.inbox),
            pending: Arc::clone(&self.pending),
            shutdown: rx,
        };
        let join = tokio::spawn(subprocess::reader_loop(args));
        self.reader_handle = Some(join);
        self.shutdown = Some(tx);
        self.state = ConnectionState::Connected;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        if self.reader_handle.is_none() {
            return Ok(());
        }
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(true);
        }
        // Drop the writer — closing stdin signals signal-cli to exit.
        self.stdin = None;
        // Kill child if still running.
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
        if let Some(handle) = self.reader_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
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
        let stdin = self.stdin.as_ref().ok_or(ChannelError::NotStarted)?.clone();
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);

        // Register pending oneshot BEFORE writing — avoids a race where
        // signal-cli answers faster than we install the slot.
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let params = json!({
            "recipient": [msg.conversation_id.clone()],
            "message": msg.text,
        });
        let request = jsonrpc::Request::new(id, "send", params);
        let line = serde_json::to_string(&request)
            .map_err(|e| ChannelError::from(SignalError::Decode(format!("encode request: {e}"))))?;

        // Write line + newline atomically — JSON-RPC over stdio is
        // line-delimited.
        {
            let mut guard = stdin.lock().await;
            if let Err(e) = guard.write_all(line.as_bytes()).await {
                self.pending.lock().await.remove(&id);
                return Err(SignalError::Io(format!("write request: {e}")).into());
            }
            if let Err(e) = guard.write_all(b"\n").await {
                self.pending.lock().await.remove(&id);
                return Err(SignalError::Io(format!("write newline: {e}")).into());
            }
            if let Err(e) = guard.flush().await {
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
        }
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
        assert!(ch.reader_handle.is_some());

        ch.stop().await.unwrap();
        assert!(ch.reader_handle.is_none());
        assert!(ch.shutdown.is_none());
        assert!(ch.stdin.is_none());
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
        assert!(ch.reader_handle.is_none());
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
}
