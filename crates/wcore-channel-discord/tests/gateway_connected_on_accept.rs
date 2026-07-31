//! `Connected` is published when Discord ACCEPTS the handshake — never when
//! the handshake is merely SENT.
//!
//! # What this file guards, and why the numbers matter
//!
//! `HealthState::from_connection_state` maps `Connected` straight to
//! `Healthy` (wcore-channels/src/health.rs:54), so the instant the Discord
//! adapter published `Connected` it had declared the channel healthy. It used
//! to publish it the moment the IDENTIFY/RESUME frame was handed to the socket,
//! before Discord had said anything at all.
//!
//! That was measured against REAL Discord on 2026-07-31, on the unfixed binary,
//! with a valid bot token and an undefined intent bit so the server answers
//! close 4013: the shipped `wayland-core channel health` verb read **healthy in
//! 13 of 46 samples across 92 seconds**, flapping healthy<->degraded **40
//! times**, and it would have kept doing so forever, because a non-4004 close
//! sends the outer loop round again and every lap re-announced `Connected`
//! between the IDENTIFY and the close. The channel never once completed a
//! handshake.
//!
//! # Why these are fixtures and not live runs
//!
//! The live runs exist too and are the primary evidence. These are the part
//! that can go red in CI on every future commit, and they cover the two
//! branches a live run cannot force cheaply: an INVALID_SESSION that demotes a
//! RESUME back to a fresh IDENTIFY, and a socket that dies between IDENTIFY and
//! READY. The fake server below speaks only the frames Discord's documented
//! protocol defines and invents no behaviour of its own.
//!
//! # Anti-vacuity
//!
//! Every test that asserts an ABSENCE ("no `Connected`") also asserts a
//! presence that proves the fixture ran — a connection count, a handshake
//! sequence, or a heartbeat tally. An absence assertion against a server that
//! was never contacted is not a measurement.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use wcore_channel_discord::{DiscordChannel, DiscordConfig};
use wcore_channels::Channel;
use wcore_channels::event::{ChannelEvent, ConnectionState};
use wcore_config::credentials::{CredentialsError, CredentialsStore};

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

/// One scripted server action, run AFTER the client's IDENTIFY or RESUME has
/// been received.
#[derive(Clone, Debug)]
enum Act {
    /// Say nothing for this long. Used to separate "handshake sent" from
    /// "handshake accepted" by a margin far wider than the 10 ms poll.
    Silence(u64),
    /// `op:0 t:READY` carrying a session handle and a resume host pointed back
    /// at this same fixture.
    Ready(&'static str),
    /// `op:0 t:RESUMED`.
    Resumed,
    /// `op:9` — `true` = still resumable, `false` = re-IDENTIFY.
    InvalidSession(bool),
    /// A real close frame with a real code.
    Close(u16),
    /// Answer heartbeats for this long, then fall out of the script.
    Hold(u64),
    /// Return without closing: the TCP socket dies with no close frame, which
    /// is what a network drop looks like and the only thing that leaves the
    /// client's session eligible for RESUME.
    DropSocket,
}

/// What the fixture OBSERVED, so a test can prove the fixture actually ran.
#[derive(Default)]
struct Observed {
    /// `(opcode, ms since the fixture started)` for each handshake frame, in
    /// accept order: 2 = IDENTIFY, 6 = RESUME.
    ///
    /// The TIMESTAMP is load-bearing and was added after the first draft of
    /// this file shipped a resume test that could not fail. That draft asserted
    /// only that the two `Connected` events were >=900 ms apart, which the
    /// PRE-FIX build satisfies for free: it publishes on the RESUME *send*, and
    /// the reconnect backoff alone is 1000 ms. Timing the assertion from the
    /// moment the SERVER received the RESUME is what separates "published when
    /// we sent it" from "published when the server accepted it".
    handshakes: Vec<(u64, u64)>,
    /// Heartbeat (op 1) frames received, across all connections.
    heartbeats: usize,
}

struct Fixture {
    base: String,
    connections: Arc<AtomicUsize>,
    observed: Arc<Mutex<Observed>>,
    /// Shared zero point for every timestamp in `Observed`. A test's own
    /// `collect_timed` clock starts a few ms later, which only ever makes the
    /// assertions below more conservative.
    started: Instant,
}

impl Fixture {
    async fn handshake_ops(&self) -> Vec<u64> {
        self.observed
            .lock()
            .await
            .handshakes
            .iter()
            .map(|(op, _)| *op)
            .collect()
    }

    /// Milliseconds from the fixture's zero point to the n'th handshake frame.
    async fn handshake_at(&self, n: usize) -> u64 {
        self.observed.lock().await.handshakes[n].1
    }
}

/// Spawn a fake gateway that runs `scripts[i]` on its i'th connection. A
/// connection beyond the end of `scripts` gets the last script repeated, so a
/// reconnect storm does not silently fall off the end of the fixture.
async fn fake_gateway(scripts: Vec<Vec<Act>>, hb_interval_ms: u64) -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let base = format!("ws://127.0.0.1:{port}");
    let connections = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(Mutex::new(Observed::default()));

    let started = Instant::now();
    let counter = Arc::clone(&connections);
    let obs = Arc::clone(&observed);
    let self_url = base.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let script = scripts
                .get(n)
                .or_else(|| scripts.last())
                .cloned()
                .unwrap_or_default();
            let obs = Arc::clone(&obs);
            let self_url = self_url.clone();
            tokio::spawn(async move {
                serve(stream, script, hb_interval_ms, obs, self_url, started).await;
            });
        }
    });

    Fixture {
        base,
        connections,
        observed,
        started,
    }
}

async fn serve(
    stream: tokio::net::TcpStream,
    script: Vec<Act>,
    hb_interval_ms: u64,
    observed: Arc<Mutex<Observed>>,
    self_url: String,
    started: Instant,
) {
    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut sink, mut source) = ws.split();

    if sink
        .send(WsMessage::Text(format!(
            r#"{{"op":10,"d":{{"heartbeat_interval":{hb_interval_ms}}}}}"#
        )))
        .await
        .is_err()
    {
        return;
    }

    // Read until the client's handshake frame arrives, answering heartbeats.
    loop {
        let Some(Ok(msg)) = source.next().await else {
            return;
        };
        let WsMessage::Text(text) = msg else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let op = v.get("op").and_then(|o| o.as_u64()).unwrap_or(u64::MAX);
        match op {
            1 => {
                observed.lock().await.heartbeats += 1;
                let _ = sink.send(WsMessage::Text(r#"{"op":11}"#.to_string())).await;
            }
            2 | 6 => {
                let at = started.elapsed().as_millis() as u64;
                observed.lock().await.handshakes.push((op, at));
                break;
            }
            _ => {}
        }
    }

    for act in script {
        match act {
            Act::Silence(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            Act::Ready(sid) => {
                let frame = format!(
                    r#"{{"op":0,"t":"READY","s":1,"d":{{"session_id":"{sid}","resume_gateway_url":"{self_url}"}}}}"#
                );
                if sink.send(WsMessage::Text(frame)).await.is_err() {
                    return;
                }
            }
            Act::Resumed => {
                if sink
                    .send(WsMessage::Text(
                        r#"{"op":0,"t":"RESUMED","s":5,"d":{}}"#.to_string(),
                    ))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Act::InvalidSession(resumable) => {
                if sink
                    .send(WsMessage::Text(format!(r#"{{"op":9,"d":{resumable}}}"#)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Act::Close(code) => {
                let _ = sink
                    .send(WsMessage::Close(Some(CloseFrame {
                        code: CloseCode::Library(code),
                        reason: "fixture".into(),
                    })))
                    .await;
                let _ = sink.close().await;
                return;
            }
            Act::DropSocket => return,
            Act::Hold(ms) => {
                let deadline = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < deadline {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let Ok(Some(Ok(msg))) = tokio::time::timeout(remaining, source.next()).await
                    else {
                        break;
                    };
                    let WsMessage::Text(text) = msg else { continue };
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    if v.get("op").and_then(|o| o.as_u64()) == Some(1) {
                        observed.lock().await.heartbeats += 1;
                        let _ = sink.send(WsMessage::Text(r#"{"op":11}"#.to_string())).await;
                    }
                }
            }
        }
    }
}

fn channel(gateway_base: String, heartbeat_grace_ms: u64) -> DiscordChannel {
    let cfg = DiscordConfig {
        credential_handle: "discord.test.bot_token".to_string(),
        allowed_channel_ids: Vec::new(),
        intents: 0,
        heartbeat_grace_ms,
        api_base_url: "http://127.0.0.1:1".to_string(),
        gateway_url: gateway_base.clone(),
    };
    DiscordChannel::with_bases(
        "discord",
        cfg,
        Arc::new(MapStore("bot-token-value".to_string())),
        // Unroutable on purpose: /users/@me is best-effort at start().
        "http://127.0.0.1:1".to_string(),
        gateway_base,
    )
}

/// Poll the adapter every 10 ms for `ms`, stamping each event with the
/// milliseconds elapsed since the call. The stamp is what lets a test say
/// "this arrived AFTER the server spoke" rather than only "this arrived".
async fn collect_timed(ch: &mut DiscordChannel, ms: u64) -> Vec<(u64, ChannelEvent)> {
    let start = Instant::now();
    let mut all = Vec::new();
    while start.elapsed() < Duration::from_millis(ms) {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if let Ok(evs) = ch.poll_events().await {
            let at = start.elapsed().as_millis() as u64;
            all.extend(evs.into_iter().map(|e| (at, e)));
        }
    }
    all
}

fn connected(events: &[(u64, ChannelEvent)]) -> Vec<u64> {
    events
        .iter()
        .filter(|(_, e)| {
            matches!(
                e,
                ChannelEvent::ConnectionStateChanged {
                    state: ConnectionState::Connected
                }
            )
        })
        .map(|(t, _)| *t)
        .collect()
}

fn count_state(events: &[(u64, ChannelEvent)], want: ConnectionState) -> usize {
    events
        .iter()
        .filter(
            |(_, e)| matches!(e, ChannelEvent::ConnectionStateChanged { state } if *state == want),
        )
        .count()
}

/// A fresh IDENTIFY publishes exactly one `Connected`, and not until READY.
///
/// The server stays silent for 700 ms after IDENTIFY. The pre-fix adapter
/// published `Connected` inside the first poll tick, so the timestamp — not
/// merely the count — is the assertion that separates the two builds.
#[tokio::test]
async fn a_fresh_connect_publishes_connected_only_after_ready() {
    let fx = fake_gateway(
        vec![vec![
            Act::Silence(700),
            Act::Ready("sess-A"),
            Act::Hold(60_000),
        ]],
        600_000,
    )
    .await;
    let mut ch = channel(fx.base.clone(), 600_000);
    ch.start().await.expect("start spawns the gateway task");

    let events = collect_timed(&mut ch, 2500).await;
    let at = connected(&events);

    assert_eq!(
        at.len(),
        1,
        "exactly one Connected per accepted handshake; got {events:?}"
    );
    assert!(
        at[0] >= 650,
        "Connected was published at t+{}ms but the server did not send READY \
         until t+700ms — the adapter declared itself healthy before Discord \
         had accepted anything. Full stream: {events:?}",
        at[0]
    );
    assert_eq!(
        fx.handshake_ops().await,
        vec![2],
        "the fixture must have received exactly one IDENTIFY"
    );

    let _ = ch.stop().await;
}

/// A rejected credential publishes NO `Connected` at all.
#[tokio::test]
async fn a_4004_before_ready_publishes_no_connected() {
    let fx = fake_gateway(vec![vec![Act::Close(4004)]], 600_000).await;
    let mut ch = channel(fx.base.clone(), 600_000);
    ch.start().await.expect("start spawns the gateway task");

    let events = collect_timed(&mut ch, 2000).await;

    assert_eq!(
        connected(&events).len(),
        0,
        "a token Discord refused must never read Healthy; got {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|(_, e)| matches!(e, ChannelEvent::AuthExpired { .. }))
            .count(),
        1,
        "the rejection itself must still be published; got {events:?}"
    );
    assert_eq!(
        fx.connections.load(Ordering::SeqCst),
        1,
        "fixture never ran, or the gateway retried a refused token"
    );

    let _ = ch.stop().await;
}

/// The measured one: a NON-auth close that sends the outer loop round again
/// must not announce a healthy channel once per lap.
///
/// This is the fixture form of the live 13/46 flap. `4013` is Discord's
/// "invalid intent(s)", it is not classified as a credential rejection, so the
/// gateway reconnects forever — which is correct — and the defect was that each
/// reconnection published `Connected` before being refused again.
#[tokio::test]
async fn a_repeating_non_auth_close_never_publishes_connected() {
    let fx = fake_gateway(vec![vec![Act::Close(4013)]], 600_000).await;
    let mut ch = channel(fx.base.clone(), 600_000);
    ch.start().await.expect("start spawns the gateway task");

    let events = collect_timed(&mut ch, 4000).await;

    assert_eq!(
        connected(&events).len(),
        0,
        "every reconnect lap flashed Healthy on a channel that has never \
         completed a handshake; got {events:?}"
    );
    let laps = fx.connections.load(Ordering::SeqCst);
    assert!(
        laps >= 2,
        "anti-vacuity: the absence above is only meaningful if the loop \
         really did keep reconnecting; it connected {laps} time(s)"
    );
    assert!(
        count_state(&events, ConnectionState::Reconnecting) >= 1,
        "the operator must still be told the channel is down; got {events:?}"
    );

    let _ = ch.stop().await;
}

/// A socket that dies between IDENTIFY and READY.
///
/// Two things are asserted, and the second is the one the cross-audit asked
/// for. First: no `Connected`, because nothing was ever accepted. Second: the
/// gateway loop's own vocabulary for this is `Reconnecting`, NOT `Disconnected`
/// — the only `Disconnected` the adapter can emit comes from `stop()`.
///
/// That asymmetry is deliberate and it is safe. Nothing in this workspace pairs
/// `Connected` with `Disconnected`: the only two consumers of
/// `ConnectionStateChanged` are `ChannelManager`'s poll loop
/// (wcore-channels/src/manager.rs:402), which is a stateless last-writer-wins
/// map into `HealthState`, and `InboundSubscriber`
/// (wcore-agent/src/channel_inbound.rs:252), which matches `MessageReceived`
/// only and ignores state changes entirely. A `Disconnected` from `stop()` on a
/// channel that never connected is the honest report, not a desync.
#[tokio::test]
async fn a_mid_handshake_drop_publishes_no_connected_and_no_unpaired_disconnected() {
    let fx = fake_gateway(vec![vec![Act::DropSocket]], 600_000).await;
    let mut ch = channel(fx.base.clone(), 600_000);
    ch.start().await.expect("start spawns the gateway task");

    let events = collect_timed(&mut ch, 3000).await;

    assert_eq!(
        connected(&events).len(),
        0,
        "the socket died before READY; nothing was accepted; got {events:?}"
    );
    assert_eq!(
        count_state(&events, ConnectionState::Disconnected),
        0,
        "the gateway loop must say Reconnecting, not Disconnected, for a \
         dropped socket; got {events:?}"
    );
    assert!(
        count_state(&events, ConnectionState::Reconnecting) >= 1,
        "anti-vacuity: the drop must have been reported at all; got {events:?}"
    );
    let laps = fx.connections.load(Ordering::SeqCst);
    assert!(laps >= 2, "the fixture served only {laps} connection(s)");

    // stop() is the sole producer of Disconnected, and it produces exactly one
    // even though no Connected preceded it. Pinned so a future change to the
    // pairing has to be deliberate.
    let _ = ch.stop().await;
    let after = ch.poll_events().await.expect("poll after stop");
    assert_eq!(
        after
            .iter()
            .filter(|e| matches!(
                e,
                ChannelEvent::ConnectionStateChanged {
                    state: ConnectionState::Disconnected
                }
            ))
            .count(),
        1,
        "stop() publishes exactly one Disconnected; got {after:?}"
    );
}

/// RESUME -> RESUMED publishes `Connected`, and not before RESUMED lands.
///
/// This is the path the previous lane declined to touch. The first connection
/// reaches READY and then loses its socket with no close frame, which is the
/// only way to make the adapter choose RESUME (op 6) over a fresh IDENTIFY. The
/// second connection then stays silent for 700 ms before accepting, so the
/// timestamp separates "RESUME sent" from "RESUME accepted".
#[tokio::test]
async fn a_resume_publishes_connected_on_resumed_and_not_before() {
    let fx = fake_gateway(
        vec![
            vec![Act::Ready("sess-A"), Act::Hold(300), Act::DropSocket],
            vec![Act::Silence(700), Act::Resumed, Act::Hold(60_000)],
        ],
        600_000,
    )
    .await;
    let mut ch = channel(fx.base.clone(), 600_000);
    ch.start().await.expect("start spawns the gateway task");

    // `collect_timed` starts its clock a few ms AFTER the fixture's, so
    // comparing an event stamp against a fixture stamp only ever understates
    // the gap. That direction is safe: it can turn a pass into a fail, never
    // the reverse.
    let started = fx.started;
    let events = collect_timed(&mut ch, 4000).await;
    let collect_offset = started.elapsed().as_millis() as u64 - 4000;
    let at = connected(&events);

    assert_eq!(
        fx.handshake_ops().await,
        vec![2, 6],
        "the second connection must be a RESUME (op 6), not a fresh IDENTIFY; \
         without that this test proves nothing about the resume path"
    );
    assert_eq!(
        at.len(),
        2,
        "one Connected for READY and one for RESUMED, no more; got {events:?}"
    );

    // The assertion that separates the builds: the fixture RECEIVED the RESUME
    // at `resume_at`, then said nothing for 700 ms before sending RESUMED. A
    // `Connected` published within that silence was published on the send.
    let resume_at = fx.handshake_at(1).await;
    let second_connected_at = at[1] + collect_offset;
    assert!(
        second_connected_at >= resume_at + 600,
        "the fixture received the RESUME at t+{resume_at}ms and stayed silent \
         until t+{}ms, but Connected was published at t+{second_connected_at}ms \
         — before Discord had accepted anything. Full stream: {events:?}",
        resume_at + 700
    );

    let _ = ch.stop().await;
}

/// A REJECTED resume must still end up `Connected`.
///
/// The realistic regression from moving the publish is not that a good RESUME
/// breaks — it is that a REFUSED one never republishes, leaving a perfectly
/// recovered channel permanently Degraded. Discord answers a stale session with
/// `op 9 d=false`; the adapter must drop the session, re-IDENTIFY, and announce
/// `Connected` on the READY that follows.
#[tokio::test]
async fn an_invalid_session_falls_back_to_identify_and_still_publishes_connected() {
    let fx = fake_gateway(
        vec![
            vec![Act::Ready("sess-A"), Act::Hold(300), Act::DropSocket],
            vec![Act::InvalidSession(false)],
            vec![Act::Silence(400), Act::Ready("sess-B"), Act::Hold(60_000)],
        ],
        600_000,
    )
    .await;
    let mut ch = channel(fx.base.clone(), 600_000);
    ch.start().await.expect("start spawns the gateway task");

    // The outer loop waits a random 1-5s after a fatal Invalid Session before
    // re-IDENTIFYing (Discord requires it), so the window must clear 5s + the
    // reconnect backoff with room to spare.
    let events = collect_timed(&mut ch, 12_000).await;

    assert_eq!(
        fx.handshake_ops().await,
        vec![2, 6, 2],
        "the sequence must be IDENTIFY, RESUME, then a FRESH IDENTIFY after \
         the invalid session; anything else and this test is measuring some \
         other path"
    );
    let at = connected(&events);
    assert_eq!(
        at.len(),
        2,
        "one Connected for each accepted handshake — the READY of the first \
         connection and the READY of the fallback. The refused RESUME must \
         contribute none. Got {events:?}"
    );

    // The load-bearing half: the channel is UP at the end, not stranded.
    let last_state = events
        .iter()
        .rev()
        .find_map(|(_, e)| match e {
            ChannelEvent::ConnectionStateChanged { state } => Some(*state),
            _ => None,
        })
        .expect("some connection state was published");
    assert_eq!(
        last_state,
        ConnectionState::Connected,
        "after a refused RESUME and a successful re-IDENTIFY the channel must \
         read Connected; leaving it Reconnecting is the permanent-degradation \
         regression this test exists for. Got {events:?}"
    );

    let _ = ch.stop().await;
}

/// Heartbeats start from HELLO, not from `Connected`.
///
/// Discord kills a connection that does not heartbeat, and HELLO arrives before
/// READY. If anything in the heartbeat path had been keyed off the `Connected`
/// publish, moving that publish to READY would starve it. The server here NEVER
/// sends READY, so a heartbeat can only be explained by HELLO.
///
/// Read the two assertions separately. The HEARTBEAT tally passes on the
/// pre-fix build as well, and that is the point of having it: it is the guard
/// that this change did not break something it was never supposed to touch. The
/// `Connected` count next to it does NOT pass pre-fix, so the test as a whole
/// goes red on the old build. That was measured, not assumed — an earlier draft
/// of this comment claimed the whole test passed pre-fix and the base run
/// refuted it.
#[tokio::test]
async fn heartbeats_start_from_hello_even_when_ready_never_arrives() {
    let fx = fake_gateway(vec![vec![Act::Hold(3000)]], 300).await;
    // The grace must exceed the interval or the client declares the link dead
    // before the tally is interesting.
    let mut ch = channel(fx.base.clone(), 5_000);
    ch.start().await.expect("start spawns the gateway task");

    let events = collect_timed(&mut ch, 3000).await;

    let beats = fx.observed.lock().await.heartbeats;
    assert!(
        beats >= 3,
        "with a 300ms interval and no READY the server should have received \
         several heartbeats; it received {beats}"
    );
    assert_eq!(
        connected(&events).len(),
        0,
        "READY never arrived, so nothing was accepted; got {events:?}"
    );

    let _ = ch.stop().await;
}
