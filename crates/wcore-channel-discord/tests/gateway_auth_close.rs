//! Gateway close-code handling, driven end to end against a real local
//! WebSocket server.
//!
//! # Why this file exists
//!
//! The unit tests in `gateway.rs` cover the close-code CLASSIFIER in isolation.
//! They cannot see the WIRING — that the close arm actually reads the frame,
//! that `SessionExit::AuthRejected` reaches the outer loop, that the loop
//! publishes `AuthExpired` into the inbox before exiting, and that it then
//! STOPS instead of re-IDENTIFYing forever. Without this file that whole path
//! was proven only by the live four-quadrant run, and a unit suite that passes
//! while the wiring is severed is exactly the shape this program keeps finding.
//!
//! The fake gateway speaks just enough of the protocol to get to the decision
//! point: accept, send HELLO, read IDENTIFY, then close with a chosen code.

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
use wcore_channels::event::ChannelEvent;
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

/// Spawn a fake Discord gateway that HELLOs, waits for IDENTIFY, then closes
/// with `code`. Returns the ws:// base and a counter of accepted connections —
/// the counter is what distinguishes "stopped" from "reconnecting forever".
async fn fake_gateway(code: u16) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connections);

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let (mut sink, mut source) = ws.split();
                // HELLO — a long heartbeat interval so the heartbeat timer
                // never fires and cannot be confused with the close path.
                if sink
                    .send(WsMessage::Text(
                        r#"{"op":10,"d":{"heartbeat_interval":600000}}"#.to_string(),
                    ))
                    .await
                    .is_err()
                {
                    return;
                }
                // Wait for the client's IDENTIFY before refusing it, so the
                // refusal lands where Discord's real 4004 lands.
                let _ = source.next().await;
                let _ = sink
                    .send(WsMessage::Close(Some(CloseFrame {
                        code: CloseCode::Library(code),
                        reason: "test".into(),
                    })))
                    .await;
                let _ = sink.close().await;
            });
        }
    });

    (format!("ws://127.0.0.1:{port}"), connections)
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
        // Unroutable: `start()` resolves /users/@me best-effort and must not
        // need it. Pointing it at a dead port also proves that.
        "http://127.0.0.1:1".to_string(),
        gateway_base,
    )
}

/// Drain `poll_events` for up to ~5s, collecting everything seen.
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

/// The load-bearing behaviour: a real 4004 close frame from the peer must
/// become `AuthExpired` in the inbox, and the gateway must STOP.
#[tokio::test]
async fn a_4004_close_publishes_auth_expired_and_stops_the_gateway() {
    let (base, connections) = fake_gateway(4004).await;
    let mut ch = channel(base);
    ch.start().await.expect("start spawns the gateway task");

    let events = collect_events(&mut ch, 5).await;

    let expired: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            ChannelEvent::AuthExpired { reason } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(
        expired.len(),
        1,
        "a 4004 must publish exactly one AuthExpired; got {events:?}"
    );
    assert!(
        expired[0].contains("4004"),
        "the operator needs the code in the reason: {}",
        expired[0]
    );

    // Terminal means terminal. One IDENTIFY attempt, not a retry storm.
    let n = connections.load(Ordering::SeqCst);
    assert_eq!(
        n, 1,
        "a rejected token must not be re-IDENTIFYed; the gateway reconnected \
         {n} times"
    );

    let _ = ch.stop().await;
}

/// The known-negative, and the reason the test above means anything. A close
/// code that is NOT a credential rejection must publish NO `AuthExpired` and
/// must keep reconnecting — otherwise every dropped socket would strand the
/// channel in a permanent Unauthenticated that only a restart clears.
#[tokio::test]
async fn a_non_auth_close_publishes_no_auth_expired_and_keeps_reconnecting() {
    // 4009 = "session timed out", explicitly resumable in Discord's docs.
    let (base, connections) = fake_gateway(4009).await;
    let mut ch = channel(base);
    ch.start().await.expect("start spawns the gateway task");

    let events = collect_events(&mut ch, 5).await;

    let expired = events
        .iter()
        .filter(|e| matches!(e, ChannelEvent::AuthExpired { .. }))
        .count();
    assert_eq!(
        expired, 0,
        "4009 is resumable and must never be reported as a rejected \
         credential; got {events:?}"
    );

    // Positive control on the fixture itself: if the server never accepted a
    // connection, the assertion above would pass for the wrong reason.
    let n = connections.load(Ordering::SeqCst);
    assert!(
        n >= 2,
        "the gateway must retry a resumable close; it connected {n} time(s), \
         which means either it stopped (a regression) or the fixture never ran"
    );

    let _ = ch.stop().await;
}
