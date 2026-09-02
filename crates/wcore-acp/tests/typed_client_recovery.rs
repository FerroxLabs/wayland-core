//! A REAL typed client recovers a REAL event gap over a REAL socket.
//!
//! Phase 24 Success Criterion 4, F24-04.
//!
//! # Why this file exists, stated plainly
//!
//! Plan 24-03 built `cursor.rs` — an ordered, gap-aware, stream-identified
//! event log with three explicit refusals and eleven unit tests. Its own
//! summary then graded Criterion 4 **NOT MET**, in these words:
//!
//! > a criterion that says clients *recover event gaps* is not met by a module
//! > that *could* let them.
//!
//! That is exactly right, and it is what this file answers. Everything here
//! runs against a server bound to a real ephemeral TCP port, driven by the
//! shipped [`AcpClient`] over HTTP and SSE. Nothing is called in-process, no
//! `EventLog` is touched directly, and the gap is produced by DROPPING a live
//! stream mid-turn rather than by simulating a disconnection.
//!
//! # The three properties, and why each is asserted the way it is
//!
//! 1. **A severed client can recover.** The client reads part of a turn, drops
//!    the stream, and resumes. The union of what it saw live and what it
//!    resumed must be the complete turn, in order, with each event exactly
//!    once. Counted, not inspected: `submitted`, `live`, `resumed`,
//!    `duplicates`, `losses`.
//!
//! 2. **Recording is independent of delivery.** This is the property that makes
//!    (1) possible and it is the one an implementation gets wrong silently. If
//!    the server logged events as the CLIENT consumed them, a disconnection
//!    would stop the log at exactly the point the client stopped reading, the
//!    resume would return an empty list, and the server would be telling a
//!    client that missed everything that it missed nothing. So the test
//!    disconnects EARLY and requires the later events to be recoverable.
//!
//! 3. **A cursor from another stream is refused, over the wire.** With a
//!    positive control on the same position, so the refusal is attributable to
//!    the stream identity and to nothing else.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use tokio::net::TcpListener;

use wcore_acp::client::{AcpClient, ResumeOutcome};
use wcore_acp::cursor::{Cursor, CursorError};
use wcore_acp::error::AcpError;
use wcore_acp::protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest};
use wcore_acp::server::AcpServer;
use wcore_acp::transport::HttpSseTransport;
use wcore_acp::turn::{TurnEngine, TurnRequest};

/// How many text events one turn emits before its terminal `Done`.
const BODY_EVENTS: usize = 12;
/// How many the client consumes before it severs the connection.
const CONSUMED_BEFORE_SEVER: usize = 3;
/// Gap between emitted events. Large enough that the client can sever while
/// the engine is demonstrably still producing, small enough to keep the test
/// quick.
const EMIT_GAP: Duration = Duration::from_millis(40);

/// A turn engine that emits a numbered body over time and then a `Done`.
///
/// The pacing is the point: a turn that completes instantly cannot be severed
/// mid-stream, so a test against one would prove the resume path works when
/// there is nothing to resume.
struct PacedEngine;

#[async_trait]
impl TurnEngine for PacedEngine {
    async fn run_turn(
        &self,
        _req: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        let s = futures::stream::unfold(0usize, |i| async move {
            if i > BODY_EVENTS {
                return None;
            }
            if i > 0 {
                tokio::time::sleep(EMIT_GAP).await;
            }
            let ev = if i == BODY_EVENTS {
                MessageEvent::Done {
                    stop_reason: "end_turn".to_string(),
                    turn_id: String::new(),
                }
            } else {
                MessageEvent::TextDelta {
                    text: format!("e{}", i + 1),
                }
            };
            Some((ev, i + 1))
        });
        Ok(Box::pin(s))
    }
}

/// Bind a real server on a real ephemeral port and return its base URL.
async fn serve() -> (String, AcpServer, tokio::task::JoinHandle<()>) {
    let server = AcpServer::new().with_turn_engine(Arc::new(PacedEngine));
    let shared = Arc::new(server.clone());
    let app = HttpSseTransport::new(shared).router();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), server, handle)
}

/// Render an event to a comparable token. `Done` gets its own token so the
/// terminal frame is counted like any other rather than being special-cased
/// into invisibility.
fn token(ev: &MessageEvent) -> String {
    match ev {
        MessageEvent::TextDelta { text } => text.clone(),
        MessageEvent::Done { stop_reason, .. } => format!("done:{stop_reason}"),
        other => format!("other:{other:?}"),
    }
}

/// The complete, ordered turn every assertion below is measured against.
fn expected_turn() -> Vec<String> {
    let mut v: Vec<String> = (1..=BODY_EVENTS).map(|i| format!("e{i}")).collect();
    v.push("done:end_turn".to_string());
    v
}

/// Wait until the server has finished recording the turn, or fail.
///
/// Bounded and LOUD. A silent timeout that returned whatever had accumulated
/// would let a server that records nothing after a disconnection pass by
/// returning early with a short list — the precise defect this file exists to
/// catch. The wait therefore panics rather than degrading.
async fn await_full_recording(client: &AcpClient, session: &str, stream_id: &str) -> Vec<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let genesis = Cursor {
        stream_id: stream_id.to_string(),
        position: 0,
    };
    loop {
        match client
            .resume_events(session, &genesis)
            .await
            .expect("resume transport")
        {
            ResumeOutcome::Served(r) => {
                let got: Vec<String> = r.events.iter().map(|p| token(&p.event)).collect();
                if got.len() == BODY_EVENTS + 1 {
                    return got;
                }
                if std::time::Instant::now() > deadline {
                    panic!(
                        "the server stopped recording at {} of {} events after the client \
                         disconnected — recording is coupled to delivery, so the events a \
                         severed client most needs are exactly the ones that were never \
                         retained. Recorded: {got:?}",
                        got.len(),
                        BODY_EVENTS + 1
                    );
                }
            }
            ResumeOutcome::Refused(r) => panic!("genesis resume was refused: {r:?}"),
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_severed_typed_client_recovers_the_exact_gap_over_a_real_socket() {
    let (base, server, _h) = serve().await;
    let client = AcpClient::new(&base).expect("client");

    let created = client
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("create");
    let session = created.session_id.clone();

    // The stream identity is issued by the SERVER and read by the client from
    // the server's own answer. A client that invented its own would be
    // asserting the very thing under test.
    let tip = server.event_tip(&session).await.expect("stream exists");
    let stream_id = tip.stream_id.clone();
    assert_eq!(tip.position, 0, "a fresh session has emitted nothing");

    // ── Consume part of the turn, then SEVER ────────────────────────────────
    let mut live: Vec<String> = Vec::new();
    {
        let mut stream = client
            .send_message(MessageSendRequest {
                session_id: session.clone(),
                text: "go".to_string(),
                tools: Vec::new(),
            })
            .await
            .expect("send");
        for _ in 0..CONSUMED_BEFORE_SEVER {
            let ev = stream
                .next()
                .await
                .expect("the turn must still be producing")
                .expect("event decodes");
            live.push(token(&ev));
        }
        // Dropping the stream drops the underlying HTTP body, which closes the
        // socket. This is the severing — no flag, no mock, no simulated flag on
        // the server side.
    }
    assert_eq!(
        live.len(),
        CONSUMED_BEFORE_SEVER,
        "the client must have taken a genuine PREFIX of the turn, not all of it, \
         or there is no gap to recover and this test proves nothing"
    );
    assert!(
        live.len() < BODY_EVENTS + 1,
        "positive control on the severing itself: the client must NOT have \
         received the whole turn"
    );

    // ── Resume from exactly where the client stopped ────────────────────────
    let recorded = await_full_recording(&client, &session, &stream_id).await;
    assert_eq!(
        recorded,
        expected_turn(),
        "the server's record of the turn must be the whole turn, in order"
    );

    let cursor = Cursor {
        stream_id: stream_id.clone(),
        position: live.len() as u64,
    };
    let resumed = match client
        .resume_events(&session, &cursor)
        .await
        .expect("resume transport")
    {
        ResumeOutcome::Served(r) => r,
        ResumeOutcome::Refused(r) => panic!("an in-range cursor was refused: {r:?}"),
    };
    let resumed_tokens: Vec<String> = resumed.events.iter().map(|p| token(&p.event)).collect();

    // ── Count. Do not inspect. ──────────────────────────────────────────────
    let expected = expected_turn();
    let mut delivered = live.clone();
    delivered.extend(resumed_tokens.clone());

    let submitted = expected.len();
    let arrived = delivered.len();
    let unique: std::collections::BTreeSet<&String> = delivered.iter().collect();
    let duplicates = arrived - unique.len();
    let losses = expected.iter().filter(|e| !unique.contains(e)).count();

    assert_eq!(
        delivered,
        expected,
        "live prefix + resumed suffix must reconstruct the turn EXACTLY and IN \
         ORDER; submitted={submitted} arrived={arrived} live={} resumed={}",
        live.len(),
        resumed_tokens.len()
    );
    assert_eq!(duplicates, 0, "duplicates must be zero, got {duplicates}");
    assert_eq!(losses, 0, "losses must be zero, got {losses}");
    assert_eq!(arrived, submitted, "arrived must equal submitted");

    // The resumed segment must be the SUFFIX the client actually missed —
    // non-empty, or the reconstruction above would have been satisfied by the
    // client having received everything live and this test would be vacuous.
    assert!(
        !resumed_tokens.is_empty(),
        "the recovered gap must be non-empty, or nothing was recovered"
    );
    assert_eq!(
        resumed.events.first().map(|p| p.position),
        Some(live.len() as u64 + 1),
        "the first resumed event must be the one immediately after the cursor"
    );
    assert_eq!(
        resumed.next_position,
        submitted as u64 + 1,
        "the stream's next position must account for every event emitted"
    );
}

#[tokio::test]
async fn a_cursor_from_another_stream_is_refused_over_the_wire_not_silently_served() {
    let (base, server, _h) = serve().await;
    let client = AcpClient::new(&base).expect("client");
    let created = client
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("create");
    let session = created.session_id.clone();
    let stream_id = server
        .event_tip(&session)
        .await
        .expect("stream exists")
        .stream_id;

    // Drive one full turn so there is real history to be wrongly served.
    let mut s = client
        .send_message(MessageSendRequest {
            session_id: session.clone(),
            text: "go".to_string(),
            tools: Vec::new(),
        })
        .await
        .expect("send");
    while let Some(ev) = s.next().await {
        ev.expect("event decodes");
    }
    let _ = await_full_recording(&client, &session, &stream_id).await;

    // POSITIVE CONTROL: position 2 on the RIGHT stream is servable. Without
    // this the refusal below could be caused by the position, the session, an
    // unreachable route, or a server that refuses every resume.
    let ok = client
        .resume_events(
            &session,
            &Cursor {
                stream_id: stream_id.clone(),
                position: 2,
            },
        )
        .await
        .expect("resume transport");
    match ok {
        ResumeOutcome::Served(r) => assert!(
            !r.events.is_empty(),
            "positive control: position 2 must be servable with events after it"
        ),
        ResumeOutcome::Refused(r) => panic!("positive control failed: {r:?}"),
    }

    // The stale cursor is minted by a SECOND SERVER INSTANCE — a different
    // process run — rather than written by hand.
    //
    // This distinction is not cosmetic and it was measured: with a hand-written
    // foreign id, this test passed even against a server whose stream ids were
    // the bare session id and therefore carried no run identity at ALL (harness
    // mutation M2 survived). Any invented string mismatches anything, so such a
    // test proves the mismatch CHECK and says nothing about the IDENTITY. Asking
    // another run what it would have called this same stream is the only form
    // that fails when the run identity is dropped.
    let other_run = AcpServer::new();
    assert_ne!(
        other_run.instance_id(),
        server.instance_id(),
        "two server instances must be two distinguishable runs"
    );
    let stale = Cursor {
        stream_id: other_run.stream_id_for(&session),
        position: 2,
    };
    assert_ne!(
        stale.stream_id, stream_id,
        "a different run must name this session's stream differently, or a \
         pre-restart cursor is indistinguishable from a current one and the \
         refusal below can never fire in production"
    );
    match client
        .resume_events(&session, &stale)
        .await
        .expect("resume transport")
    {
        ResumeOutcome::Served(r) => panic!(
            "a cursor from a dead stream was SERVED {} events — the client would \
             believe itself continuous and would have silently missed everything \
             before them",
            r.events.len()
        ),
        ResumeOutcome::Refused(refused) => {
            assert_eq!(
                refused.status, 409,
                "a wrong-stream cursor is a conflict, not a not-found and not a \
                 bad request: the client must re-subscribe, not retry"
            );
            assert!(
                matches!(refused.cursor, Some(CursorError::StreamMismatch { .. })),
                "the refusal must be STRUCTURED so a client can branch on it; got \
                 {:?}",
                refused.cursor
            );
        }
    }
}

#[tokio::test]
async fn the_ahead_and_unknown_session_refusals_survive_the_wire_with_their_own_statuses() {
    // The three refusals are distinct in `cursor.rs`. They are only USEFUL if
    // they stay distinct after crossing the transport — a transport that
    // flattened them to one status would leave a client unable to tell "resync
    // from here" from "your bookkeeping is wrong".
    let (base, server, _h) = serve().await;
    let client = AcpClient::new(&base).expect("client");
    let created = client
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("create");
    let session = created.session_id.clone();
    let stream_id = server.event_tip(&session).await.expect("stream").stream_id;

    let mut s = client
        .send_message(MessageSendRequest {
            session_id: session.clone(),
            text: "go".to_string(),
            tools: Vec::new(),
        })
        .await
        .expect("send");
    while let Some(ev) = s.next().await {
        ev.expect("decodes");
    }
    let _ = await_full_recording(&client, &session, &stream_id).await;

    // Ahead: a position this stream never emitted.
    match client
        .resume_events(
            &session,
            &Cursor {
                stream_id: stream_id.clone(),
                position: 9_999,
            },
        )
        .await
        .expect("transport")
    {
        ResumeOutcome::Refused(r) => {
            assert_eq!(r.status, 400, "an impossible position is a bad request");
            assert!(
                matches!(r.cursor, Some(CursorError::Ahead { .. })),
                "got {:?}",
                r.cursor
            );
        }
        ResumeOutcome::Served(r) => panic!(
            "a position ahead of the stream was answered with {} events — an \
             empty or partial answer here tells a client it is caught up when \
             the server has lost what it is waiting for",
            r.events.len()
        ),
    }

    // A session this server does not hold is NOT an empty list.
    match client
        .resume_events(
            "no-such-session",
            &Cursor {
                stream_id: "no-such-session@x".to_string(),
                position: 0,
            },
        )
        .await
        .expect("transport")
    {
        ResumeOutcome::Refused(r) => assert_eq!(r.status, 404),
        ResumeOutcome::Served(r) => panic!(
            "an unknown session was answered with {} events instead of a refusal",
            r.events.len()
        ),
    }
}

#[tokio::test]
async fn a_cursor_that_fell_out_of_retention_is_refused_naming_where_to_resync() {
    // The third refusal, over the wire. Retention is set small so the eviction
    // boundary is reachable in a test rather than 1024 events away — the
    // default is what an operator gets, not what a test can exercise, and a
    // refusal nobody can reach is a refusal nobody has checked.
    let server = AcpServer::new()
        .with_turn_engine(Arc::new(PacedEngine))
        .with_event_retention(4);
    let app = HttpSseTransport::new(Arc::new(server.clone())).router();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let base = format!("http://{addr}");
    let client = AcpClient::new(&base).expect("client");

    let created = client
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("create");
    let session = created.session_id.clone();
    let stream_id = server.event_tip(&session).await.expect("stream").stream_id;

    let mut s = client
        .send_message(MessageSendRequest {
            session_id: session.clone(),
            text: "go".to_string(),
            tools: Vec::new(),
        })
        .await
        .expect("send");
    while let Some(ev) = s.next().await {
        ev.expect("decodes");
    }
    // 13 events emitted into a 4-event window, so positions 1..9 are gone.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let tip = server.event_tip(&session).await.expect("stream");
        if tip.position as usize == BODY_EVENTS + 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the turn never finished recording; tip stalled at {}",
            tip.position
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // POSITIVE CONTROL: a cursor INSIDE the retained window is servable, so the
    // refusal below is caused by eviction and not by the window being empty.
    match client
        .resume_events(
            &session,
            &Cursor {
                stream_id: stream_id.clone(),
                position: (BODY_EVENTS as u64) - 1,
            },
        )
        .await
        .expect("transport")
    {
        ResumeOutcome::Served(r) => assert!(!r.events.is_empty()),
        ResumeOutcome::Refused(r) => panic!("positive control failed: {r:?}"),
    }

    match client
        .resume_events(
            &session,
            &Cursor {
                stream_id: stream_id.clone(),
                position: 1,
            },
        )
        .await
        .expect("transport")
    {
        ResumeOutcome::Refused(r) => {
            assert_eq!(
                r.status, 410,
                "an evicted position is GONE — the client must resynchronise \
                 deliberately, not retry the same cursor forever"
            );
            match r.cursor {
                Some(CursorError::TooOld {
                    oldest_available, ..
                }) => assert!(
                    oldest_available > 1,
                    "the refusal must NAME where to resume from, or the client \
                     has to guess and will guess by skipping"
                ),
                other => panic!("expected a structured TooOld, got {other:?}"),
            }
        }
        ResumeOutcome::Served(r) => panic!(
            "an evicted position was served {} events — the client would \
             silently skip everything that was dropped",
            r.events.len()
        ),
    }
}
