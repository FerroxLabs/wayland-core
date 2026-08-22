//! Shared `reqwest::Client` constructors for LLM providers and HTTP tools.
//!
//! Bare `reqwest::Client::new()` ships with NO timeouts — a wedged TCP
//! handshake or a stalled stream hangs the agent indefinitely. v0.6.1
//! hardening: every client routes through this module so the timeout
//! policy is one edit, not five.
//!
//! Two policies, because streaming and request/response have opposite
//! needs:
//!
//! ## `build()` — streaming LLM providers
//!
//! - `connect_timeout(30s)` — TCP + TLS handshake must complete within
//!   30s. Catches DNS / routing / certificate failures fast.
//!
//! - `read_timeout(300s)` — gap between bytes must be under 5 min.
//!   Catches stalled streams without killing long generations.
//!
//!   L2 fix: the previous 120s ceiling false-tripped on extended-thinking
//!   models. A reasoning model can stream NO bytes for well over two
//!   minutes while it reasons server-side *before* the first
//!   `content_block` / `delta` — a perfectly healthy request that the old
//!   120s read timeout killed as a spurious `Connection` error. 5 min is
//!   above the realistic server-side reasoning gap while still catching a
//!   genuinely wedged stream (a truly hung connection never recovers, so
//!   the exact ceiling only affects how fast a real stall is reported).
//!
//! Deliberately NO request-level timeout — that would cap total stream
//! length and break long-form generation. For a token-by-token SSE
//! stream the `read_timeout` (between-bytes) is the correct hang guard.
//!
//! ## `build_tool_client()` — non-streaming HTTP tools
//!
//! AUDIT B-5: GitHub / GitLab / Linear / Notion tool backends do a
//! single request/response, not a stream. For them the `read_timeout`
//! is NOT enough — it is a between-bytes gap timer, so a slow-drip
//! ("slowloris") server that trickles one byte every 119s resets the
//! clock on every byte and the request runs unbounded. A request-level
//! `.timeout(...)` is the correct backstop: a hard wall-clock cap on the
//! whole request. Streaming generation is not a concern here (these are
//! finite REST/GraphQL responses), so the cap that would break `build()`
//! is exactly right for the tool client.

use std::sync::OnceLock;
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use wcore_types::llm::LlmEvent;

/// Result of polling a provider response stream while also observing whether
/// the engine-side event consumer still exists.
pub(crate) enum StreamPoll<T> {
    Item(T),
    End,
    ConsumerClosed,
}

/// The silence timer for one poll, resolving to the silence it measured.
///
/// `None` arms NO timer — the future simply never completes — rather than
/// arming one that is merely very long, so a disabled threshold costs nothing
/// per streamed token.
async fn silence_notice(after: Option<Duration>) -> Duration {
    match after {
        Some(after) => {
            tokio::time::sleep(after).await;
            after
        }
        None => std::future::pending().await,
    }
}

/// Poll one provider-stream item, returning immediately when the engine drops
/// its receiver (for example after user cancellation). Without this select,
/// spawned response workers can remain parked in `bytes_stream().next()` until
/// the five-minute read timeout even though nobody can consume their output.
///
/// Also the one place that can see a stream go quiet. Every provider polls
/// this function and nothing else sits between the socket and the engine, so
/// a gap of [`stream_silence_notice_after`] with no bytes emits ONE
/// [`LlmEvent::StreamSilent`] on the same channel the engine already consumes
/// and then keeps waiting. A `warn!` would not do: with `RUST_LOG` unset only
/// ERROR reaches stderr, so a log here reaches nobody while the user watches
/// a frozen cursor.
///
/// The signal carries the elapsed silence and no prose — the agent layer
/// renders it. It is at most one per silent gap (a stream silent for the full
/// read-timeout window does not repeat), and the first byte cancels the timer
/// rather than deferring it: the timer is owned by this call and dropped with
/// it, so nothing can fire late against a stream that already answered.
///
/// `try_send`, never `send().await`: this runs on the task that owns the
/// stream, and blocking it on a full channel would stall the very stream the
/// notice is about. A dropped notice costs one advisory line.
pub(crate) async fn next_or_consumer_closed<S, T>(
    stream: &mut S,
    tx: &mpsc::Sender<LlmEvent>,
) -> StreamPoll<T>
where
    S: Stream<Item = T> + Unpin,
{
    let notice = silence_notice(stream_silence_notice_after());
    tokio::pin!(notice);
    let mut notified = false;
    loop {
        tokio::select! {
            biased;
            _ = tx.closed() => return StreamPoll::ConsumerClosed,
            item = stream.next() => {
                return match item {
                    Some(item) => StreamPoll::Item(item),
                    None => StreamPoll::End,
                };
            }
            silent_for = &mut notice, if !notified => {
                notified = true;
                let _ = tx.try_send(LlmEvent::StreamSilent { silent_for });
            }
        }
    }
}

/// Default TCP+TLS connect timeout for provider clients.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default between-bytes read timeout for provider streams.
///
/// L2: raised from 120s to 300s so extended-thinking / long server-side
/// reasoning does not false-trip the timeout. See module docs.
pub const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Environment override for [`stream_silence_notice_after`], in whole
/// seconds. `off` (or `0`) disables the notice entirely; an unparsable value
/// keeps the default rather than failing a run over a typo in an env var.
///
/// Same shape as the `WAYLAND_MODEL_DISCOVERY` switch
/// (`model_catalog::discovery_enabled`) — this repo configures provider-side
/// behaviour by env override with a documented default, not by a new constant
/// nobody can reach.
pub const STREAM_SILENCE_NOTICE_ENV: &str = "WAYLAND_STREAM_SILENCE_NOTICE";

/// Default silence before a provider stream reports that it is still waiting.
///
/// Derived from [`READ_TIMEOUT`] rather than invented: one tenth of the window
/// that would eventually kill the request, which also lands on the same 30s
/// order as [`CONNECT_TIMEOUT`]. The bar it has to clear is that the user
/// hears something WELL before the read timeout fires — silence for the full
/// five minutes is what a hang looks like.
///
/// It is only a notice: nothing is cancelled, retried or failed, so a
/// reasoning model that legitimately thinks for four minutes still streams
/// normally afterwards. That is why this is much shorter than [`READ_TIMEOUT`]
/// and is allowed to be.
pub const STREAM_SILENCE_NOTICE_AFTER: Duration = Duration::from_secs(READ_TIMEOUT.as_secs() / 10);

/// Resolved silence threshold: [`STREAM_SILENCE_NOTICE_AFTER`] unless
/// [`STREAM_SILENCE_NOTICE_ENV`] overrides it, `None` when disabled.
///
/// Read once per process — this is polled on the hot path of every streamed
/// token, and the value cannot change under a running stream anyway.
pub fn stream_silence_notice_after() -> Option<Duration> {
    static RESOLVED: OnceLock<Option<Duration>> = OnceLock::new();
    *RESOLVED.get_or_init(|| {
        resolve_stream_silence_notice(std::env::var(STREAM_SILENCE_NOTICE_ENV).ok().as_deref())
    })
}

/// The decision half of [`stream_silence_notice_after`], split from the env
/// read so it can be tested without a process-global that the caching above
/// would freeze after the first test to run.
fn resolve_stream_silence_notice(raw: Option<&str>) -> Option<Duration> {
    let Some(raw) = raw.map(str::trim) else {
        return Some(STREAM_SILENCE_NOTICE_AFTER);
    };
    if raw.eq_ignore_ascii_case("off") {
        return None;
    }
    match raw.parse::<u64>() {
        // An explicit zero is the same instruction as `off`; firing instantly
        // would otherwise emit a notice on every healthy stream.
        Ok(0) => None,
        Ok(seconds) => Some(Duration::from_secs(seconds)),
        // A typo in an env var must not silently disable the notice, and must
        // not fail a run either.
        Err(_) => Some(STREAM_SILENCE_NOTICE_AFTER),
    }
}

/// AUDIT B-5 — request-level wall-clock timeout for non-streaming HTTP
/// tools. A generous 300s cap: large GitHub/GitLab responses and slow
/// GraphQL queries still complete, but a slow-drip endpoint can no
/// longer hang a tool call forever (the between-bytes `read_timeout`
/// alone cannot catch that — see module docs).
pub const TOOL_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Request-level wall-clock cap for the non-streaming `GET /v1/models` model
/// discovery call. The streaming provider client ([`build`]) deliberately
/// carries no request timeout (it would truncate long generations), but
/// `list_models` is a finite request/response that must not hang the `/model`
/// picker if the endpoint wedges. 30s is generous for a small JSON listing
/// while still failing fast on a stalled endpoint.
pub const LIST_MODELS_TIMEOUT: Duration = Duration::from_secs(30);

/// Build an `EgressClient` with the streaming-provider timeout policy.
///
/// Panics on builder failure, which can only happen if the TLS backend
/// fails to initialize — that's a deployment-time problem, not a
/// runtime one, and surfacing it loudly at startup is correct.
pub fn build() -> wcore_egress::EgressClient {
    build_with_read_timeout(READ_TIMEOUT)
}

/// Build an `EgressClient` with a caller-specified between-bytes read
/// timeout (and the standard 30s connect timeout).
///
/// L2: additive escape hatch for callers that know a request will have
/// unusually long silent gaps (e.g. a thinking-heavy model run). `build()`
/// uses [`READ_TIMEOUT`]; this variant lets a provider raise it without a
/// breaking signature change.
pub fn build_with_read_timeout(read_timeout: Duration) -> wcore_egress::EgressClient {
    // B1: route through the egress chokepoint. EgressClient::streaming_with_read_timeout
    // carries the identical policy (30s connect, caller read timeout, redirects
    // disabled — the credential-exfil-on-302 mitigation, M-1 / U-1).
    wcore_egress::EgressClient::streaming_with_read_timeout(read_timeout)
}

/// AUDIT B-5 — build an `EgressClient` for non-streaming HTTP tools.
///
/// Identical connect + read timeouts to [`build`], PLUS a request-level
/// `.timeout(TOOL_REQUEST_TIMEOUT)` wall-clock cap. Use this for any
/// finite request/response HTTP tool (REST, GraphQL); use [`build`] only
/// for token-streaming LLM providers where a request-level cap would
/// truncate a legitimate long generation.
///
/// Panics on builder failure — same rationale as [`build`].
pub fn build_tool_client() -> wcore_egress::EgressClient {
    // B1: EgressClient::tool carries the identical non-streaming policy (connect
    // + read timeouts PLUS the request-level wall-clock cap, redirects disabled).
    wcore_egress::EgressClient::tool()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stream_poll_stops_when_event_consumer_is_dropped() {
        let (tx, rx) = mpsc::channel::<LlmEvent>(1);
        drop(rx);
        let mut stream = futures::stream::pending::<u8>();

        let result = tokio::time::timeout(
            Duration::from_millis(100),
            next_or_consumer_closed(&mut stream, &tx),
        )
        .await
        .expect("a dropped event consumer must cancel the pending stream poll");

        assert!(matches!(result, StreamPoll::ConsumerClosed));
    }

    #[tokio::test]
    async fn stream_poll_preserves_items_and_end_of_stream() {
        let (tx, _rx) = mpsc::channel::<LlmEvent>(1);
        let mut stream = futures::stream::iter([7_u8]);

        assert!(matches!(
            next_or_consumer_closed(&mut stream, &tx).await,
            StreamPoll::Item(7)
        ));
        assert!(matches!(
            next_or_consumer_closed(&mut stream, &tx).await,
            StreamPoll::End
        ));
    }

    #[test]
    fn build_constructs_a_client() {
        // `build()` must not panic — the TLS backend initializes.
        let _client = build();
    }

    #[test]
    fn build_tool_client_constructs_a_client() {
        // AUDIT B-5 — the tool client must construct without panicking.
        let _client = build_tool_client();
    }

    #[tokio::test]
    async fn build_client_does_not_follow_redirects() {
        // M-1 / U-1: a 302 must NOT be followed — the client returns the 3xx
        // response itself rather than chasing the Location to a second host
        // (which would re-send any URL/header secret). We stand up a TCP
        // listener that always answers `302 Location: http://attacker/` and
        // assert reqwest yields the 302 status, not a followed-through 200.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let resp = "HTTP/1.1 302 Found\r\nLocation: http://240.0.0.1:9/\r\nContent-Length: 0\r\n\r\n";
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });

        let client = build();
        let resp = client
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("request completes");
        assert_eq!(
            resp.status().as_u16(),
            302,
            "the client must surface the 302, not follow it"
        );

        server.abort();
    }

    #[tokio::test]
    async fn tool_client_request_times_out_on_a_slow_drip_server() {
        // AUDIT B-5 — a server that accepts the connection but never
        // sends a full response must NOT hang the tool client forever.
        // The request-level `.timeout(...)` is the backstop the
        // between-bytes `read_timeout` cannot provide. We assert the
        // timeout is WIRED by issuing a real request against a TCP
        // listener that accepts but withholds the response body and
        // confirming reqwest reports a timeout — with a short-TTL
        // client so the test runs fast.
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Accept the connection, read the request, then hold the socket
        // open forever without replying — the classic slowloris shape.
        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                // Never write a response; never close. Park until the
                // test drops the task.
                std::future::pending::<()>().await;
            }
        });

        // A tool client with a 200ms request cap — same construction
        // path as `build_tool_client`, just a fast TTL for the test.
        let client = wcore_egress::EgressClient::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .timeout(Duration::from_millis(200))
            .build()
            .expect("client builds");

        let result = client.get(format!("http://{addr}/")).send().await;
        assert!(
            result.is_err(),
            "a slow-drip server must trip the request-level timeout"
        );
        let err = result.unwrap_err();
        assert!(
            err.is_timeout(),
            "the failure must be a timeout, got: {err}"
        );

        server.abort();
    }

    /// SLOW FIRST BYTE — a stream that has produced NOTHING must surface a
    /// signal the user can actually observe.
    ///
    /// `next_or_consumer_closed` is the single chokepoint every provider polls
    /// (bedrock.rs:806, cohere.rs:336, gemini.rs:836, openai.rs:1709 and :1821,
    /// anthropic_shared.rs:362 — 6 call sites, verified by grep), and the only
    /// channel it holds to the user is `tx`. Between dispatch and the first
    /// byte the product currently says NOTHING for up to `READ_TIMEOUT` (300 s),
    /// which the user cannot tell apart from a hang.
    ///
    /// This test asserts the OBSERVABLE, deliberately not a log line: with
    /// `RUST_LOG` unset only ERROR reaches stderr, so a `warn!` here reaches
    /// nobody. A test that asserted on a log would pass while the user still
    /// stared at a frozen cursor.
    ///
    /// The bar is the minimum defensible one, tied to an existing constant
    /// rather than an invented threshold: before `READ_TIMEOUT` expires and
    /// kills the request, the user must have been told at least once that the
    /// stream is still silent. A fix should fire well before that; this test
    /// only refuses total silence.
    ///
    /// Virtual clock (`start_paused`), so the 300 s window costs no wall time.
    #[tokio::test(start_paused = true)]
    async fn a_stream_silent_past_the_threshold_surfaces_a_signal_to_the_user() {
        let (tx, mut rx) = mpsc::channel::<LlmEvent>(4);
        let started = tokio::time::Instant::now();

        // A provider stream that accepted the request and then produced nothing
        // at all — the slow-first-byte shape.
        let poll = tokio::spawn(async move {
            let mut stream = futures::stream::pending::<u8>();
            next_or_consumer_closed(&mut stream, &tx).await
        });

        let observed = tokio::time::timeout(READ_TIMEOUT, rx.recv()).await;
        poll.abort();

        let Ok(event) = observed else {
            panic!(
                "a provider stream produced no bytes for {READ_TIMEOUT:?} and told \
                 the user NOTHING: no LlmEvent reached the consumer before the \
                 read timeout would have killed the request. RUST_LOG is unset in \
                 production, so a warn! here is invisible — the signal has to be \
                 observable on the channel the engine actually consumes."
            );
        };
        let event = event.expect("the event channel must not be closed while the poll is live");
        println!(
            "slow-first-byte signal fired after {:?}: {event:?}",
            started.elapsed()
        );
    }

    /// The threshold is configurable, and every branch of the override is
    /// pinned: the default has to survive an absent or malformed value, and
    /// `off` has to actually mean off rather than "very soon".
    #[test]
    fn the_silence_threshold_reads_its_configured_override() {
        // Default: 30s, one tenth of the read timeout that would kill the
        // request — a notice long before the window a hang looks like.
        assert_eq!(resolve_stream_silence_notice(None), Some(READ_TIMEOUT / 10));
        assert_eq!(
            resolve_stream_silence_notice(None),
            Some(STREAM_SILENCE_NOTICE_AFTER)
        );
        assert_eq!(
            resolve_stream_silence_notice(Some(" 45 ")),
            Some(Duration::from_secs(45))
        );
        // Disabled, both spellings.
        assert_eq!(resolve_stream_silence_notice(Some("off")), None);
        assert_eq!(resolve_stream_silence_notice(Some("OFF")), None);
        assert_eq!(resolve_stream_silence_notice(Some("0")), None);
        // A typo keeps the default: it must neither disable the notice
        // silently nor fail the run.
        assert_eq!(
            resolve_stream_silence_notice(Some("banana")),
            Some(STREAM_SILENCE_NOTICE_AFTER)
        );
        assert_eq!(
            resolve_stream_silence_notice(Some("")),
            Some(STREAM_SILENCE_NOTICE_AFTER)
        );
    }

    /// A DISABLED threshold must arm NOTHING — not a timer that fires
    /// eventually. Virtual clock, so "eventually" is cheap: ten read-timeout
    /// windows pass and the notice must still not have completed.
    #[tokio::test(start_paused = true)]
    async fn a_disabled_threshold_arms_no_timer() {
        let notice = silence_notice(resolve_stream_silence_notice(Some("off")));
        tokio::pin!(notice);
        tokio::select! {
            biased;
            silent_for = &mut notice => {
                panic!("a disabled threshold fired a notice after {silent_for:?}")
            }
            _ = tokio::time::sleep(READ_TIMEOUT * 10) => {}
        }

        // Positive control for the mechanism: an ENABLED threshold fires
        // inside the same window, so the pass above is an absence and not a
        // select that never polls the notice at all.
        let notice = silence_notice(resolve_stream_silence_notice(Some("5")));
        tokio::pin!(notice);
        tokio::select! {
            biased;
            silent_for = &mut notice => assert_eq!(silent_for, Duration::from_secs(5)),
            _ = tokio::time::sleep(READ_TIMEOUT * 10) => {
                panic!("control: an enabled threshold must fire")
            }
        }
    }

    /// NEGATIVE CONTROL for the test above: a stream that answers immediately
    /// must NOT raise the slow-stream signal. Without this, a fix that fires
    /// unconditionally would pass.
    ///
    /// Passes today (nothing is ever emitted); it exists to fail if the fix
    /// over-fires.
    #[tokio::test(start_paused = true)]
    async fn a_fast_first_byte_does_not_fire_the_slow_stream_signal() {
        let (tx, mut rx) = mpsc::channel::<LlmEvent>(4);
        let mut stream = futures::stream::iter([7_u8]);

        assert!(matches!(
            next_or_consumer_closed(&mut stream, &tx).await,
            StreamPoll::Item(7)
        ));
        assert!(
            rx.try_recv().is_err(),
            "a stream that answered immediately must not raise a slow-stream signal"
        );

        // And the timer must be CANCELLED by the first byte, not merely
        // not-yet-due: a fix that spawns a detached timer would fire it late,
        // long after the bytes arrived, and spam a healthy stream.
        tokio::time::sleep(READ_TIMEOUT * 2).await;
        assert!(
            rx.try_recv().is_err(),
            "the slow-stream signal must be cancelled by the first byte, not deferred: \
             it fired {:?} after a stream that had already delivered its item",
            READ_TIMEOUT * 2
        );
    }
}
