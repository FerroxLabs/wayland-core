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
/// Also where an ESTABLISHED stream is seen to go quiet; the window before it
/// is established belongs to [`awaiting_first_byte`]. Every provider polls
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

/// Observe the DISPATCH window — request sent, response head not yet arrived.
///
/// [`next_or_consumer_closed`] is the wrong instrument for this: it can only
/// watch a `Stream`, and a provider stream does not exist until `send().await`
/// has already resolved. Every provider builds its `mpsc::Sender` AFTER the
/// send returns 2xx (anthropic.rs:186, openai.rs:1557, azure_openai.rs:321,
/// gemini.rs:684, cohere.rs, vertex.rs:432), so the window this call covers is
/// the one window the product could not say anything during.
///
/// Measured live on this host before the fix, against a blackholed endpoint:
/// 30.076 s of total silence between the last startup notice and the first
/// retry line, at 0.02 s polling resolution on an unpiped raw file.
///
/// Coverage is now continuous. This call spans dispatch → response head;
/// [`next_or_consumer_closed`] spans response head → first byte and every
/// later gap. The two windows are adjacent and disjoint, so a silence anywhere
/// in the request is attributable to one of them.
///
/// DIAGNOSTIC ONLY — the dispatch is the only thing that can end this loop.
/// The timer arm emits and latches; it never breaks, returns, or drops the
/// dispatch. `dispatch` is awaited to completion and its own output is handed
/// back unchanged, so nothing here can cancel, shorten or retry a request.
/// `biased` polls the dispatch first, so a dispatch that resolves in the same
/// wake as the timer wins and stays silent.
///
/// Same properties as the stream-side notice, for the same reasons: at most
/// ONE notice per silent gap (the `notified` latch); the signal carries the
/// elapsed duration and no prose, because the agent layer owns the rendering;
/// the timer is owned by this call and dropped with it, so completing the
/// dispatch CANCELS it rather than deferring it onto a request that already
/// answered; and `try_send` never parks the task that owns the request on a
/// full channel — a dropped notice costs one advisory line.
pub async fn awaiting_first_byte<F>(dispatch: F, tx: &mpsc::Sender<LlmEvent>) -> F::Output
where
    F: std::future::Future,
{
    tokio::pin!(dispatch);
    let notice = silence_notice(stream_silence_notice_after());
    tokio::pin!(notice);
    let mut notified = false;
    loop {
        tokio::select! {
            biased;
            output = &mut dispatch => return output,
            silent_for = &mut notice, if !notified => {
                notified = true;
                let _ = tx.try_send(LlmEvent::StreamSilent { silent_for });
            }
        }
    }
}

/// Default TCP+TLS connect timeout for provider clients.
///
/// Ten seconds, down from thirty. A connect timeout on a retrying path is a
/// RETRY TRIGGER, not a failure: the cost of cutting it too fine is one extra
/// attempt, and the cost of leaving it long is that every attempt against a
/// blackholed endpoint parks for the full window before the next one starts.
///
/// Measured against real provider endpoints, the TCP+TLS handshake completes
/// in 19-232 ms. Under an emulated 800 ms RTT / 10 % loss "hotel wifi" path
/// the p90 is 2.9 s and the worst observed 3.9 s; under a deliberately awful
/// 1600 ms / 20 % path the p90 is 9.1 s. Ten seconds therefore clears the
/// realistic worst case, and because a timeout only costs a retry, a whole
/// turn is lost only if EVERY attempt in the budget exceeds it — about 1e-11
/// on the awful path at the shipped budget.
///
/// DECLARED, not re-exported, and that is a decision rather than an oversight.
/// The provider streaming client is built by
/// `EgressClient::streaming_with_read_timeout`, which reads
/// `wcore_egress::CONNECT_TIMEOUT`, so a second literal here is a COPY of the
/// live deadline — and everything in this module that reasons about the connect
/// window ([`STREAM_SILENCE_NOTICE_AFTER`], the
/// `the_silence_threshold_must_beat_the_connect_deadline` guard, and
/// `wcore_agent`'s silent-stall retry ceiling) would go on agreeing with the
/// copy while the client used the other number. `pub const CONNECT_TIMEOUT:
/// Duration = wcore_egress::CONNECT_TIMEOUT;` would make that unrepresentable,
/// but it would also make `the_two_connect_deadlines_agree` a tautology that
/// can never fail. A guard that cannot fail is not a guard. The literal is kept
/// and the test is the thing that holds the two in step — and it earned that:
/// this very edit moved BOTH declarations from 30 s to 10 s, which is exactly
/// the drift it exists to catch.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Default silence before a request reports that it is still waiting.
///
/// Derived from [`CONNECT_TIMEOUT`], not [`READ_TIMEOUT`]: the binding
/// constraint is the EARLIEST deadline the notice has to precede, never the
/// latest. The old derivation (`READ_TIMEOUT` / 10) came out at 30 s — exactly
/// [`CONNECT_TIMEOUT`] — so on the dispatch window the notice was scheduled for
/// the same instant as the failure it exists to precede and could never reach
/// the user first.
///
/// The derivation, not the number, is the thing worth keeping. When
/// [`CONNECT_TIMEOUT`] came down from 30 s to 10 s this threshold followed it
/// from 15 s to 5 s, and that is deliberate: pinning it at 15 s would have
/// re-created the exact defect above, one worse — a notice scheduled five
/// seconds AFTER the deadline it exists to precede can never fire on the
/// dispatch window at all. `the_silence_threshold_must_beat_the_connect_deadline`
/// refuses that pin.
///
/// Five seconds does not nag. This threshold has exactly one production caller
/// — [`awaiting_first_byte`], wrapping the provider dispatch — so it measures
/// time-to-first-byte and nothing else, it is latched to at most one line per
/// dispatch, and measured first-byte latency against real endpoints is under a
/// quarter second. It is still sixty times below [`READ_TIMEOUT`], so an
/// established stream that goes quiet is announced long before the read
/// timeout would kill it.
///
/// It is only a notice: nothing is cancelled, retried or failed, so a
/// reasoning model that legitimately thinks for four minutes still streams
/// normally afterwards. That is why this is much shorter than [`READ_TIMEOUT`]
/// and is allowed to be.
pub const STREAM_SILENCE_NOTICE_AFTER: Duration =
    Duration::from_secs(CONNECT_TIMEOUT.as_secs() / 2);

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
        // Default: half the connect deadline it has to precede — and still
        // far below the read timeout that would kill a quiet stream.
        assert_eq!(
            resolve_stream_silence_notice(None),
            Some(CONNECT_TIMEOUT / 2)
        );
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

    /// #1077 — PRE-CONNECT SILENCE. Reproduction of the live symptom.
    ///
    /// Measured on hetzner-dsm against `--base-url http://192.0.2.1:9999`
    /// (TEST-NET-1, SYNs dropped), isolated `WAYLAND_HOME`, raw redirect to a
    /// file polled every 50 ms — no pipe on the binary, because a pipe
    /// collapses every arrival to the moment it flushes:
    ///
    /// ```text
    /// +000.060s  notice: crash replay protection is OFF ...
    /// +000.122s  warning: no secure credential backend ...
    /// +030.193s  Provider stream failed (... deadline has elapsed); retrying (attempt 1/2)
    /// +060.709s  ... (attempt 2/2)
    /// +091.727s  error: Provider stream failed after retries
    /// ```
    ///
    /// 30.071 s of total silence. The instrument resolved the 62 ms gap between
    /// the two startup writes, so that silence is the product's, not a flush
    /// artifact. `LlmEvent::StreamSilent` never fired.
    ///
    /// TWO independent reasons it cannot fire — this test covers the structural
    /// one, `the_silence_threshold_must_beat_the_connect_deadline` the other:
    ///
    /// 1. STRUCTURAL — the notice is armed inside the stream poll loop, and the
    ///    channel it would send on is not constructed until `send().await` has
    ///    already returned 2xx. A connect that never completes never reaches it.
    /// 2. ARITHMETIC — `STREAM_SILENCE_NOTICE_AFTER` (`READ_TIMEOUT` / 10 = 30 s)
    ///    equals `CONNECT_TIMEOUT` (30 s) exactly, so even once wired it would
    ///    tie with the failure it is supposed to precede.
    ///
    /// The bar is the reference behaviour: Claude Code arms its slow-first-byte
    /// timer on REQUEST SENT, not on stream established, and reports before the
    /// request dies. Diagnostic only — nothing here cancels or fails anything.
    ///
    /// Asserts the OBSERVABLE (`LlmEvent` on the channel the engine consumes),
    /// never a log line: with `RUST_LOG` unset only ERROR reaches stderr, so a
    /// `warn!` would satisfy a log-shaped test while the user still watched a
    /// frozen cursor for thirty seconds.
    ///
    /// Virtual clock, so the 30 s window costs no wall time.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_dispatch_surfaces_a_silence_signal_before_the_connect_timeout() {
        let (tx, mut rx) = mpsc::channel::<LlmEvent>(4);
        let started = tokio::time::Instant::now();

        // A dispatch that was sent and has produced nothing at all — the
        // blackholed-endpoint shape above. It never resolves, exactly as the
        // live send did not resolve until the 30 s connect deadline killed it.
        let dispatch =
            tokio::spawn(
                async move { awaiting_first_byte(std::future::pending::<()>(), &tx).await },
            );

        let observed = tokio::time::timeout(CONNECT_TIMEOUT, rx.recv()).await;
        dispatch.abort();

        let Ok(event) = observed else {
            panic!(
                "a dispatch that produced no bytes told the user NOTHING for the whole \
                 {CONNECT_TIMEOUT:?} connect window: no LlmEvent reached the consumer \
                 before the connect timeout would have killed the request. This is the \
                 measured 30.071s of live silence — the notice is armed in the stream \
                 poll loop, which a never-completing connect never reaches."
            );
        };
        let event = event.expect("the event channel must not be closed while the dispatch is live");
        assert!(
            matches!(event, LlmEvent::StreamSilent { .. }),
            "the dispatch window must surface a silence signal, got: {event:?}"
        );
        // STRICTLY before. A notice that lands exactly ON the connect deadline
        // is worth nothing to the user — the failure arrives in the same
        // instant. This is the half that the 30 s == 30 s tie fails.
        assert!(
            started.elapsed() < CONNECT_TIMEOUT,
            "the silence signal must PRECEDE the connect deadline, not tie with it: \
             fired after {:?} against a {CONNECT_TIMEOUT:?} connect timeout",
            started.elapsed()
        );
    }

    /// The same connect deadline is declared in two crates, because
    /// `wcore-egress` sits below `wcore-providers` and cannot import it back.
    /// Two literals is exactly how they drift, and a drift here is invisible
    /// in use: the two clients would simply give up on the same endpoint at
    /// different times, and only one of them is on the retrying path.
    #[test]
    fn the_two_connect_deadlines_agree() {
        assert_eq!(
            CONNECT_TIMEOUT,
            wcore_egress::CONNECT_TIMEOUT,
            "the provider client and the egress client must abandon a connect \
             at the same instant"
        );
    }

    /// The arithmetic half of the defect above, isolated so it cannot be fixed
    /// by accident and so its failure is unambiguous.
    ///
    /// A notice threshold that equals the connect timeout can never precede a
    /// connect failure — it is scheduled for the same instant the request dies.
    /// The default is `READ_TIMEOUT` / 10 = 30 s and `CONNECT_TIMEOUT` is 30 s,
    /// so the dispatch-window notice has no room to fire at all.
    #[test]
    fn the_silence_threshold_must_beat_the_connect_deadline() {
        assert!(
            STREAM_SILENCE_NOTICE_AFTER < CONNECT_TIMEOUT,
            "the silence notice is scheduled at {STREAM_SILENCE_NOTICE_AFTER:?} but the \
             connect deadline is {CONNECT_TIMEOUT:?}: a notice that ties with the failure \
             it is meant to precede can never reach the user first. The dispatch window \
             needs a threshold strictly below the connect timeout."
        );
    }

    /// NEGATIVE CONTROL (a) — a dispatch that answers immediately must raise
    /// NOTHING. Without this, a fix that fires unconditionally would pass the
    /// reproduction above.
    ///
    /// Passes today (the seam emits nothing); it exists to fail if the fix
    /// over-fires, and to fail if the fix spawns a DETACHED timer that fires
    /// late against a request that already answered.
    #[tokio::test(start_paused = true)]
    async fn a_fast_dispatch_does_not_fire_the_connect_silence_signal() {
        let (tx, mut rx) = mpsc::channel::<LlmEvent>(4);

        let answered = awaiting_first_byte(std::future::ready(7_u8), &tx).await;
        assert_eq!(
            answered, 7,
            "the seam must return the dispatch's own output"
        );
        assert!(
            rx.try_recv().is_err(),
            "a dispatch that answered immediately must not raise a silence signal"
        );

        // Cancelled, not merely not-yet-due.
        tokio::time::sleep(READ_TIMEOUT * 2).await;
        assert!(
            rx.try_recv().is_err(),
            "the dispatch silence signal must be cancelled by the response, not deferred: \
             it fired {:?} after a dispatch that had already answered",
            READ_TIMEOUT * 2
        );

        // KNOWN-POSITIVE CONTROL for the two absences above. Both assert that
        // NOTHING arrived, and an absence proves nothing unless the instrument
        // could have seen a presence — a closed or already-drained receiver
        // would satisfy them for free. Put one real event through the same
        // receiver and require it to land.
        tx.try_send(LlmEvent::StreamSilent {
            silent_for: Duration::from_secs(1),
        })
        .expect("control: the channel must accept an event");
        assert!(
            matches!(rx.try_recv(), Ok(LlmEvent::StreamSilent { .. })),
            "control: this receiver CAN observe a silence signal, so the two \
             absence assertions above are real absences and not a dead channel"
        );
    }

    /// DIAGNOSTIC ONLY — the notice must not touch the request it reports on.
    ///
    /// That a silence signal must never abort, shorten or otherwise alter a
    /// request is not covered by either control above: both use a dispatch
    /// that answers before the timer, so neither can observe what happens to a
    /// dispatch the notice has already fired against. A fix that returned,
    /// broke, or dropped the dispatch on the timer arm would pass every other
    /// test in this file and silently kill every slow request in production.
    ///
    /// Here the notice fires first and the dispatch resolves afterwards. It
    /// must still resolve, with its OWN value, and exactly one notice may exist.
    #[tokio::test(start_paused = true)]
    async fn a_notified_dispatch_still_completes_and_returns_its_own_result() {
        let (tx, mut rx) = mpsc::channel::<LlmEvent>(8);
        let threshold = stream_silence_notice_after().expect("the default is enabled");

        // Outlives the notice by a wide margin, and still has to be waited for.
        let answered = awaiting_first_byte(
            async {
                tokio::time::sleep(threshold * 3).await;
                "the dispatch resolved on its own"
            },
            &tx,
        )
        .await;
        assert_eq!(
            answered, "the dispatch resolved on its own",
            "the silence notice must not cancel or shorten the request it reports on"
        );

        assert!(
            matches!(
                rx.try_recv(),
                Ok(LlmEvent::StreamSilent { silent_for }) if silent_for == threshold
            ),
            "the notice must have fired once, carrying the elapsed silence"
        );
        assert!(
            rx.try_recv().is_err(),
            "a dispatch silent for {:?} against a {threshold:?} threshold must still \
             raise exactly ONE notice, not one per elapsed window",
            threshold * 3
        );
    }

    /// NEGATIVE CONTROL (b) — the case that ALREADY WORKS must keep working.
    ///
    /// An established stream that goes quiet still emits EXACTLY ONE notice:
    /// one when the gap opens, and not one per elapsed window afterwards. Ten
    /// further read-timeout windows pass here and nothing more may arrive.
    ///
    /// Passes today. It is the regression guard on the stream-poll path that a
    /// fix to the dispatch path must not disturb.
    #[tokio::test(start_paused = true)]
    async fn an_established_stream_that_goes_quiet_still_emits_exactly_one_notice() {
        let (tx, mut rx) = mpsc::channel::<LlmEvent>(8);

        let poll = tokio::spawn(async move {
            let mut stream = futures::stream::pending::<u8>();
            next_or_consumer_closed(&mut stream, &tx).await
        });

        let first = tokio::time::timeout(READ_TIMEOUT, rx.recv())
            .await
            .expect("an established stream going quiet must still raise its notice")
            .expect("the event channel must not be closed while the poll is live");
        assert!(
            matches!(first, LlmEvent::StreamSilent { .. }),
            "expected the established-stream silence notice, got: {first:?}"
        );

        tokio::time::sleep(READ_TIMEOUT * 10).await;
        assert!(
            rx.try_recv().is_err(),
            "the established-stream notice must be at most ONE per silent gap, not one \
             per elapsed window"
        );

        poll.abort();
    }
}
