//! When the Discord gateway is entitled to report `Connected`, driven end to
//! end against a real local WebSocket server.
//!
//! # Why this file exists
//!
//! `gateway.rs` used to publish `ConnectionState::Connected` the instant it had
//! WRITTEN the IDENTIFY (or RESUME) frame, before reading a single byte back.
//! `HealthState::from_connection_state` maps `Connected` straight to `Healthy`,
//! so **every failed handshake reported `Healthy` on its way down** — with a
//! rejected token the sequence was `Connected` → close 4004 → `AuthExpired`, and
//! a health poll landing in that window saw a dead channel claim to be live.
//!
//! The fix moves the `Connected` push to READY (fresh IDENTIFY) and RESUMED
//! (resume). The previous lane that found this defect declined to move it,
//! because moving it risks stranding a RESUMED session in `Degraded` forever and
//! it could not exercise the resume path. **So the resume path is exercised
//! here** — `resume_path_reaches_connected_only_after_resumed` drives a full
//! READY → op-7 → RESUME → RESUMED cycle against the fake gateway and asserts
//! the second `Connected`.
//!
//! Both directions of the gate are proven, per LANE-BRIEF §3b-iii:
//! - it can FAIL: `..._never_publishes_connected` (a refused handshake);
//! - it can PASS: `..._publishes_connected_after_ready` and the resume test
//!   (accepted handshakes), which would go red if the push had merely been
//!   deleted rather than moved.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use wcore_channel_discord::{DiscordChannel, DiscordConfig};
use wcore_channels::Channel;
use wcore_channels::event::{ChannelEvent, ConnectionState};
use wcore_config::credentials::{CredentialsError, CredentialsStore};

/// Minimal in-memory credentials store.
struct MapStore(String);
impl CredentialsStore for MapStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        Ok(if key == "discord.test.bot_token" {
            Some(self.0.clone())
        } else {
            None
        })
    }
    fn put(&self, _k: &str, _v: &str) -> Result<(), CredentialsError> {
        Ok(())
    }
    fn delete(&self, _k: &str) -> Result<(), CredentialsError> {
        Ok(())
    }
}

/// What the fake gateway does once it has read the client's handshake frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Script {
    /// Refuse the handshake with a close code. Models 4004/4013/4014.
    RefuseWith(u16),
    /// Accept: send READY and then hold the socket open forever.
    AcceptAndHold,
    /// Accept, then ask for a reconnect (op 7) so the client comes back with a
    /// RESUME; the second connection is answered with RESUMED.
    AcceptThenReconnectThenResume,
}

/// Counters the assertions read back. Every one of these exists so that a test
/// cannot pass because the fixture never ran (LANE-BRIEF §6a-i): an actor that
/// never launched is a dead instrument.
#[derive(Default)]
struct Seen {
    connections: AtomicUsize,
    identifies: AtomicUsize,
    resumes: AtomicUsize,
}

/// Spawn a fake Discord gateway running `script`.
async fn fake_gateway(script: Script) -> (String, Arc<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Seen::default());
    let counter = Arc::clone(&seen);

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let nth = counter.connections.fetch_add(1, Ordering::SeqCst);
            let seen = Arc::clone(&counter);
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let (mut sink, mut source) = ws.split();
                // HELLO with a long interval so the heartbeat timer never fires
                // and cannot be mistaken for any of the paths under test.
                if sink
                    .send(WsMessage::Text(
                        r#"{"op":10,"d":{"heartbeat_interval":600000}}"#.to_string(),
                    ))
                    .await
                    .is_err()
                {
                    return;
                }

                // Read the client's handshake frame and record WHICH it was.
                // Distinguishing op 2 from op 6 is what makes the resume test a
                // resume test rather than a second fresh login.
                let Some(Ok(WsMessage::Text(handshake))) = source.next().await else {
                    return;
                };
                match serde_json::from_str::<serde_json::Value>(&handshake)
                    .ok()
                    .and_then(|v| v["op"].as_u64())
                {
                    Some(2) => seen.identifies.fetch_add(1, Ordering::SeqCst),
                    Some(6) => seen.resumes.fetch_add(1, Ordering::SeqCst),
                    _ => 0,
                };

                match script {
                    Script::RefuseWith(code) => {
                        let _ = sink
                            .send(WsMessage::Close(Some(CloseFrame {
                                code: CloseCode::Library(code),
                                reason: "test".into(),
                            })))
                            .await;
                        let _ = sink.close().await;
                    }
                    Script::AcceptAndHold => {
                        let _ = sink.send(WsMessage::Text(ready_frame())).await;
                        // Hold. Dropping here would look like a transport fault
                        // and add a Reconnecting the assertions would have to
                        // tolerate.
                        std::future::pending::<()>().await;
                    }
                    Script::AcceptThenReconnectThenResume => {
                        if nth == 0 {
                            // First connection: accept the IDENTIFY, then ask
                            // for a reconnect. op 7 keeps the session VALID, so
                            // the client must come back with RESUME, not a
                            // fresh IDENTIFY.
                            let _ = sink.send(WsMessage::Text(ready_frame())).await;
                            tokio::time::sleep(Duration::from_millis(150)).await;
                            let _ = sink.send(WsMessage::Text(r#"{"op":7}"#.to_string())).await;
                            let _ = sink.close().await;
                        } else {
                            // Second connection: answer the RESUME with RESUMED.
                            let _ = sink
                                .send(WsMessage::Text(
                                    r#"{"op":0,"t":"RESUMED","s":7,"d":{}}"#.to_string(),
                                ))
                                .await;
                            std::future::pending::<()>().await;
                        }
                    }
                }
            });
        }
    });

    (format!("ws://127.0.0.1:{port}"), seen)
}

/// READY with no `resume_gateway_url`, so the client resumes against the same
/// fake server instead of a host that does not exist.
fn ready_frame() -> String {
    r#"{"op":0,"t":"READY","s":1,"d":{"session_id":"sess-under-test","user":{"id":"7"}}}"#
        .to_string()
}

fn channel(gateway_base: String) -> DiscordChannel {
    let cfg = DiscordConfig {
        credential_handle: "discord.test.bot_token".to_string(),
        allowed_channel_ids: Vec::new(),
        intents: 0,
        heartbeat_grace_ms: 600_000,
        api_base_url: "http://127.0.0.1:1".to_string(),
        gateway_url: gateway_base.clone(),
    };
    DiscordChannel::with_bases(
        "discord",
        cfg,
        Arc::new(MapStore("bot-token-value".to_string())),
        "http://127.0.0.1:1".to_string(),
        gateway_base,
    )
}

/// Drain `poll_events` for up to `secs`, collecting everything seen in order.
async fn collect_events(ch: &mut DiscordChannel, secs: u64) -> Vec<ChannelEvent> {
    let mut all = Vec::new();
    for _ in 0..(secs * 10) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(evs) = ch.poll_events().await {
            all.extend(evs);
        }
    }
    all
}

/// The ordered connection states the adapter published, which is the whole
/// signal these tests are about.
fn states(events: &[ChannelEvent]) -> Vec<ConnectionState> {
    events
        .iter()
        .filter_map(|e| match e {
            ChannelEvent::ConnectionStateChanged { state } => Some(*state),
            _ => None,
        })
        .collect()
}

/// **The defect.** A handshake Discord refuses must never produce `Connected`,
/// because `Connected` is `Healthy` and the channel is not healthy — it is about
/// to die with a rejected credential.
#[tokio::test]
async fn refused_handshake_never_publishes_connected() {
    let (base, seen) = fake_gateway(Script::RefuseWith(4004)).await;
    let mut ch = channel(base);
    ch.start().await.expect("start spawns the gateway task");

    let events = collect_events(&mut ch, 5).await;
    let states = states(&events);

    // Fixture liveness: the server must actually have read an IDENTIFY, or the
    // absence below is free (LANE-BRIEF §3b-i).
    assert_eq!(
        seen.identifies.load(Ordering::SeqCst),
        1,
        "the fake gateway never read an IDENTIFY, so this run proves nothing"
    );

    assert!(
        !states.contains(&ConnectionState::Connected),
        "a refused handshake published Connected (→ HealthState::Healthy) before \
         dying: {states:?}"
    );
    // Positive control on the extractor in the same test: if `states()` were
    // returning an empty vec for any reason, the assertion above would pass for
    // the wrong reason. The in-flight handshake MUST be visible as Connecting.
    assert!(
        states.contains(&ConnectionState::Connecting),
        "the adapter published no Connecting either, so the state extractor may \
         be dead rather than the defect fixed: {states:?}"
    );
    // And the refusal still lands as a credential rejection.
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, ChannelEvent::AuthExpired { .. }))
            .count(),
        1,
        "4004 must still publish exactly one AuthExpired: {events:?}"
    );

    let _ = ch.stop().await;
}

/// **The gate can pass.** An accepted IDENTIFY must reach `Connected` — a fix
/// that merely deleted the premature push would leave the channel stuck in
/// `Degraded` forever and would fail here.
#[tokio::test]
async fn accepted_identify_publishes_connected_after_ready() {
    let (base, seen) = fake_gateway(Script::AcceptAndHold).await;
    let mut ch = channel(base);
    ch.start().await.expect("start spawns the gateway task");

    let events = collect_events(&mut ch, 4).await;
    let states = states(&events);

    assert_eq!(
        seen.identifies.load(Ordering::SeqCst),
        1,
        "the fake gateway never read an IDENTIFY, so this run proves nothing"
    );
    assert_eq!(
        states,
        vec![ConnectionState::Connecting, ConnectionState::Connected],
        "an accepted handshake must publish exactly Connecting-then-Connected, \
         in that order"
    );

    let _ = ch.stop().await;
}

/// **The path the previous lane could not test.** A RESUME accepted with
/// RESUMED must also reach `Connected`. Without the RESUMED arm, a resumed
/// session — which is fully working and delivering replayed messages — would sit
/// in `Connecting`/`Degraded` for the rest of its life, which would be a worse
/// defect than the one being fixed.
#[tokio::test]
async fn resume_path_reaches_connected_only_after_resumed() {
    let (base, seen) = fake_gateway(Script::AcceptThenReconnectThenResume).await;
    let mut ch = channel(base);
    ch.start().await.expect("start spawns the gateway task");

    let events = collect_events(&mut ch, 6).await;
    let states = states(&events);

    // The experiment only happened if BOTH participants played their part: a
    // fresh IDENTIFY on connection 1 and a real op-6 RESUME on connection 2.
    // Without this, a client that simply re-IDENTIFYed would pass the state
    // assertion below while proving nothing about the resume arm.
    assert_eq!(
        seen.identifies.load(Ordering::SeqCst),
        1,
        "expected exactly one fresh IDENTIFY; got {}",
        seen.identifies.load(Ordering::SeqCst)
    );
    assert_eq!(
        seen.resumes.load(Ordering::SeqCst),
        1,
        "the client never sent an op-6 RESUME, so the resume arm was never \
         exercised and this test is vacuous"
    );

    assert_eq!(
        states,
        vec![
            // Connection 1: handshake in flight, then READY.
            ConnectionState::Connecting,
            ConnectionState::Connected,
            // op 7 — the outer loop announces the gap.
            ConnectionState::Reconnecting,
            // Connection 2: RESUME in flight, then RESUMED.
            ConnectionState::Connecting,
            ConnectionState::Connected,
        ],
        "the full resume cycle must be Connecting/Connected/Reconnecting/\
         Connecting/Connected — a Connected missing at the end means a resumed \
         session is stranded in Degraded"
    );

    let _ = ch.stop().await;
}
