//! The provider stream client must bound the CONNECT phase and nothing else.
//!
//! Issue #1077 asks for "a connect-phase timeout distinct from the request
//! timeout". The product has that distinction —
//! `EgressClient::streaming_with_read_timeout` sets `connect_timeout` and a
//! between-bytes `read_timeout` and deliberately NO request-level cap, because
//! a wall-clock cap on the whole request would truncate a long generation,
//! while `EgressClient::tool` adds exactly that cap for finite REST calls.
//! What was ungraded is the distinctness: nothing failed if the streaming
//! client grew a `.timeout(...)`.
//!
//! ## The connect deadline is live — measured, not assumed
//!
//! MEASURED on hetzner-dsm through the real binary against a blackholed
//! endpoint (`http://192.0.2.1:9999`, TEST-NET-1, packets dropped), by moving
//! `wcore_egress::CONNECT_TIMEOUT` and rebuilding:
//!
//! ```text
//! CONNECT_TIMEOUT = 30s  ->  62.7s end to end, silence notice at 15s
//! CONNECT_TIMEOUT =  5s  ->  16.0s end to end, silence notice at  2s
//! ```
//!
//! Both halves move with it, which is the point of the re-export in
//! `http_client`: the notice threshold is `CONNECT_TIMEOUT / 2`, and while
//! that module kept its OWN `from_secs(30)` the 5 s arm would have scheduled
//! the notice at 15 s — after the failure it exists to precede — with
//! `the_silence_threshold_must_beat_the_connect_deadline` still green against
//! the copy.
//!
//! ## Why the deadline's own value is not asserted here
//!
//! A test for it needs an address that swallows SYNs, and whether one exists
//! is a property of the running host's routing rather than of this code: a CI
//! host that answers TEST-NET-1 with ICMP unreachable fails the connect in a
//! millisecond whatever the deadline says, so the test would pass without
//! grading anything. The deadline is held by the re-export (structurally) and
//! by the live measurement above. This file grades only the DISTINCTNESS,
//! which is falsifiable in-process.

use std::time::{Duration, Instant};

use wcore_providers::http_client::build;

/// The connect bound must not become a REQUEST bound.
///
/// The one thing `build()` must never grow is a wall-clock cap on the whole
/// request: a token-by-token generation legitimately runs for many minutes and
/// a request timeout would truncate it. The tempting "fix" for #1077's 92 s is
/// exactly that edit — cap the request instead of bounding the connect — and
/// it would trade a misconfiguration that is reported in 92 s for long
/// generations that die silently at the cap.
///
/// Graded by the property that separates the two: a server that answers its
/// response head promptly and then trickles a body must keep streaming.
///
/// HONEST LIMIT: the probe streams for ~2 s, so it catches a request cap
/// shorter than that and not a generous one. That is the shape the mistake
/// takes — a cap added to make a hang fail fast is a small one — and a test
/// that streamed for minutes to catch a 5-minute cap would cost more than the
/// defect. Red arm observed: `.timeout(Duration::from_secs(1))` on
/// `EgressClient::streaming_with_read_timeout` fails this test.
#[tokio::test(flavor = "multi_thread")]
async fn the_connect_bound_is_not_a_request_bound() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback bind");
    let addr = listener.local_addr().expect("bound addr");

    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        // Drain the request head first; hyper rejects a response that arrives
        // before its request was read.
        let mut scratch = [0u8; 4096];
        let _ = socket.read(&mut scratch).await;
        // Response head immediately, then a body that trickles for longer than
        // the connect deadline.
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
            .await;
        for _ in 0..8 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if socket.write_all(b": keepalive\n\n").await.is_err() {
                return;
            }
        }
    });

    let started = Instant::now();
    let response = build()
        .post(format!("http://{addr}/v1/messages"))
        .send()
        .await
        .expect("the head arrives promptly; a connect bound must not affect it");
    assert!(response.status().is_success());

    let mut body = response.bytes_stream();
    let mut chunks = 0usize;
    while let Some(chunk) = futures::StreamExt::next(&mut body).await {
        chunk.expect(
            "a slow but live stream must not be cut off — `build()` must carry NO \
             request-level wall-clock timeout, only a connect and a between-bytes bound",
        );
        chunks += 1;
        if chunks >= 4 {
            break;
        }
    }
    assert!(
        chunks >= 4,
        "the stream ended early after {elapsed:?}; a request-level timeout has been added to \
         the provider client",
        elapsed = started.elapsed()
    );
}
