//! FerroxLabs/wayland#305 c3 — a turn that stops answering is DISCLOSED on the
//! protocol stream, not left to hang.
//!
//! # Why this is a protocol-stream test and not a log test
//!
//! The failure this closes is the most-reported class in the tracker: a request
//! that never returns and never says so (UAT measured 902 seconds against a bad
//! base URL, still counting). Writing the stall to `tracing` would not fix it —
//! `RUST_LOG` is unset for ordinary users, so only `ERROR` reaches stderr and a
//! host consuming SSE never sees any of it. The disclosure has to arrive as a
//! frame on the same stream the caller is already reading, which is what these
//! tests read it off: a real listener, a real SSE body, parsed as
//! `MessageEvent`s.
//!
//! # The three arms
//!
//! 1. an approval gate nobody answers ends the stream with a terminal `Error`;
//! 2. a turn that stalls with no gate at all does the same (the tool-exec case);
//! 3. **control** — a turn that completes normally is NOT touched by the guard,
//!    and a guard disabled with `None` restores the old hanging behaviour. Arm 3
//!    is what stops arm 1 passing on a server that simply errors everything.
//!
//! Every arm is wrapped in an outer `tokio::time::timeout`, so a regression that
//! reinstates the hang fails this suite in seconds instead of hanging CI — the
//! failure mode under test must not become the failure mode of the test.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, Stream, StreamExt};

use wcore_acp::error::AcpError;
use wcore_acp::protocol::{ErrorCode, MessageEvent, ToolCall};
use wcore_acp::server::{AcpServer, DEFAULT_TURN_STALL_TIMEOUT};
use wcore_acp::transport::RestTransport;
use wcore_acp::transport::http::HttpHandler;
use wcore_acp::turn::{TurnEngine, TurnRequest};

/// How long the tests give the server before calling it hung. Comfortably
/// larger than the configured stall bound, far smaller than a CI timeout.
const OUTER_LIMIT: Duration = Duration::from_secs(20);
/// The stall bound the tests configure. Short enough to run, long enough that a
/// slow machine cannot trip it between two adjacent frames.
const STALL_BOUND: Duration = Duration::from_millis(400);

/// Emits `head`, then never yields again — the shape of a parked tool or an
/// unanswered approval gate.
struct StallingEngine {
    head: Vec<MessageEvent>,
}

#[async_trait]
impl TurnEngine for StallingEngine {
    async fn run_turn(
        &self,
        _req: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        let head = stream::iter(self.head.clone());
        Ok(head.chain(stream::pending()).boxed())
    }
}

/// A turn that finishes normally. The control.
struct CompletingEngine;

#[async_trait]
impl TurnEngine for CompletingEngine {
    async fn run_turn(
        &self,
        _req: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        Ok(stream::iter(vec![
            MessageEvent::TextDelta {
                text: "done thinking".to_string(),
            },
            MessageEvent::Done {
                stop_reason: "end_turn".to_string(),
                turn_id: String::new(),
            },
        ])
        .boxed())
    }
}

fn gate_frame() -> MessageEvent {
    MessageEvent::ApprovalRequired {
        call: ToolCall {
            id: "call-1".to_string(),
            name: "Bash".to_string(),
            input: serde_json::json!({ "command": "ls" }),
        },
        reason: "mutating tool Bash requires approval".to_string(),
        resume_token: String::new(),
    }
}

/// Serve `server` on an ephemeral port and return its base URL.
async fn serve(server: Arc<AcpServer>) -> String {
    let app = RestTransport::new(server).router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    format!("http://{addr}")
}

/// Create a session and read one prompt's SSE stream to completion.
async fn prompt_frames(base: &str) -> Vec<MessageEvent> {
    #[allow(clippy::disallowed_methods)] // localhost roundtrip; no proxy policy needed
    let client = reqwest::Client::new();
    let created: serde_json::Value = client
        .post(format!("{base}/v1/sessions"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["session_id"].as_str().expect("session id");

    let resp = client
        .post(format!("{base}/v1/sessions/{id}/prompt"))
        .json(&serde_json::json!({ "text": "run it" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    body.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| serde_json::from_str::<MessageEvent>(p).expect("SSE data is a MessageEvent"))
        .collect()
}

fn terminal_error(frames: &[MessageEvent]) -> &wcore_acp::protocol::JsonRpcError {
    match frames.last() {
        Some(MessageEvent::Error { error, .. }) => error,
        other => panic!(
            "the stream must END with a disclosed Error frame; last was {other:?} \
             (all frames: {frames:?})"
        ),
    }
}

#[tokio::test]
async fn an_unanswered_approval_gate_is_disclosed_as_an_error_frame() {
    let engine = Arc::new(StallingEngine {
        head: vec![gate_frame()],
    });
    let server = Arc::new(
        AcpServer::new()
            .with_turn_engine(engine as Arc<dyn TurnEngine>)
            .with_turn_stall_timeout(Some(STALL_BOUND)),
    );
    let base = serve(server).await;

    let frames = tokio::time::timeout(OUTER_LIMIT, prompt_frames(&base))
        .await
        .expect(
            "the prompt stream must terminate on its own. Hanging here IS the \
             defect under test: an approval gate nobody answers left the caller \
             with no frame and no ending.",
        );

    assert!(
        frames
            .iter()
            .any(|e| matches!(e, MessageEvent::ApprovalRequired { .. })),
        "the gate itself must still reach the host - the disclosure replaces the \
         silence, not the question; frames={frames:?}"
    );
    let error = terminal_error(&frames);
    assert_eq!(
        error.code,
        ErrorCode::Timeout.code(),
        "a stall is its own code, not a generic internal error: {error:?}"
    );
    assert!(
        error.message.contains("no output"),
        "the message must say what happened, not just that something did: {error:?}"
    );
}

#[tokio::test]
async fn a_tool_that_never_returns_is_disclosed_too() {
    // No gate at all - the exec half of the criterion. A tool call announced
    // and then nothing, which is what a hung command looks like on the wire.
    let engine = Arc::new(StallingEngine {
        head: vec![MessageEvent::ToolCall {
            call: ToolCall {
                id: "call-1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({ "command": "sleep infinity" }),
            },
        }],
    });
    let server = Arc::new(
        AcpServer::new()
            .with_turn_engine(engine as Arc<dyn TurnEngine>)
            .with_turn_stall_timeout(Some(STALL_BOUND)),
    );
    let base = serve(server).await;

    let frames = tokio::time::timeout(OUTER_LIMIT, prompt_frames(&base))
        .await
        .expect("an exec that never returns must end the stream, not hang it");
    assert_eq!(terminal_error(&frames).code, ErrorCode::Timeout.code());
}

/// The disclosure is RECORDED, not only delivered. A client that dropped the
/// connection mid-stall resumes and is told what happened; without this the
/// resume would replay a stream that simply stops.
#[tokio::test]
async fn the_disclosure_is_in_the_resumable_event_log() {
    let engine = Arc::new(StallingEngine {
        head: vec![gate_frame()],
    });
    let server = Arc::new(
        AcpServer::new()
            .with_turn_engine(engine as Arc<dyn TurnEngine>)
            .with_turn_stall_timeout(Some(STALL_BOUND)),
    );

    let created = server
        .create_session(wcore_acp::protocol::SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            cwd: None,
        })
        .await
        .expect("session");
    let session_id = created.session_id.clone();

    let drained = async {
        let stream = server
            .send_message(wcore_acp::protocol::MessageSendRequest {
                session_id: session_id.clone(),
                text: "run it".to_string(),
                tools: Vec::new(),
            })
            .await
            .expect("stream");
        let frames: Vec<MessageEvent> = stream.collect().await;
        frames
    };
    let frames = tokio::time::timeout(OUTER_LIMIT, drained)
        .await
        .expect("the in-process stream must terminate too");
    assert_eq!(terminal_error(&frames).code, ErrorCode::Timeout.code());

    let replay = server
        .events_since(
            &session_id,
            &wcore_acp::cursor::Cursor {
                stream_id: server.stream_id_for(&session_id),
                position: 0,
            },
        )
        .await
        .expect("resume");
    assert!(
        replay
            .events
            .iter()
            .any(|e| matches!(&e.event, MessageEvent::Error { error, .. }
                if error.code == ErrorCode::Timeout.code())),
        "a client resuming after the stall must be told about it; got {:?}",
        replay.events
    );
}

// ── Controls ─────────────────────────────────────────────────────────────

/// The guard must not touch a healthy turn. Without this, a server that
/// terminated every stream with an error would pass every assertion above.
#[tokio::test]
async fn a_turn_that_completes_is_untouched_by_the_guard() {
    let server = Arc::new(
        AcpServer::new()
            .with_turn_engine(Arc::new(CompletingEngine) as Arc<dyn TurnEngine>)
            .with_turn_stall_timeout(Some(STALL_BOUND)),
    );
    let base = serve(server).await;

    let frames = tokio::time::timeout(OUTER_LIMIT, prompt_frames(&base))
        .await
        .expect("a completing turn must complete");
    assert!(
        matches!(frames.last(), Some(MessageEvent::Done { .. })),
        "a healthy turn keeps its Done terminal; frames={frames:?}"
    );
    assert!(
        !frames
            .iter()
            .any(|e| matches!(e, MessageEvent::Error { .. })),
        "no error frame belongs on a turn that finished; frames={frames:?}"
    );
}

/// Turning the guard OFF restores the old behaviour exactly. This is the
/// known-negative: it shows the disclosure comes from the guard and not from
/// something else in the stack that would have ended the stream anyway.
#[tokio::test]
async fn with_the_guard_disabled_the_stream_hangs_as_it_did_before() {
    let engine = Arc::new(StallingEngine {
        head: vec![gate_frame()],
    });
    let server = Arc::new(
        AcpServer::new()
            .with_turn_engine(engine as Arc<dyn TurnEngine>)
            .with_turn_stall_timeout(None),
    );

    let created = server
        .create_session(wcore_acp::protocol::SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            cwd: None,
        })
        .await
        .expect("session");

    let drained = async {
        let stream = server
            .send_message(wcore_acp::protocol::MessageSendRequest {
                session_id: created.session_id.clone(),
                text: "run it".to_string(),
                tools: Vec::new(),
            })
            .await
            .expect("stream");
        let frames: Vec<MessageEvent> = stream.collect().await;
        frames
    };
    assert!(
        tokio::time::timeout(STALL_BOUND * 4, drained)
            .await
            .is_err(),
        "with the guard disabled the stall must still hang - if this stream ends \
         on its own, the disclosure the other tests observe is not coming from \
         the guard and those tests prove nothing about it"
    );
}

/// The shipped default is a real number, and it is the one the server uses when
/// nobody configures it. A default of zero (or of "no guard") would make every
/// deployment that never calls the builder keep the reported defect.
#[test]
fn the_default_stall_bound_is_finite_and_generous() {
    assert!(DEFAULT_TURN_STALL_TIMEOUT >= Duration::from_secs(60));
    assert!(DEFAULT_TURN_STALL_TIMEOUT <= Duration::from_secs(3600));
    assert!(
        format!("{:?}", AcpServer::new()).contains("turn_stall_timeout: Some("),
        "a freshly constructed server must carry the guard; an operator reading \
         its state must be able to see which posture is live"
    );
}
