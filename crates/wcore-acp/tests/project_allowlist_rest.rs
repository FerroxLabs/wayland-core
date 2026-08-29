//! FerroxLabs/wayland#305 c2 — the project approval allowlist, end to end over
//! the REST surface on a live listener.
//!
//! # What is being pinned
//!
//! The reported symptom is a host that answers the same approval gate for the
//! same checkout dozens of times a session, with only `--allow-all-tools`
//! (process-wide, root-equivalent, launch-time) as the alternative. These tests
//! pin the in-between:
//!
//! * the allowlist is READ AND WRITTEN over REST, so a Desktop can manage it at
//!   runtime rather than the operator restarting Core with a different flag;
//! * a session whose `cwd` is under an ENABLED entry never shows its host an
//!   `approval_required` frame, because the server answered the gate;
//! * a session under a LISTED BUT DISABLED entry still shows it — the flag is
//!   load-bearing, not decorative;
//! * a `cwd` no entry covers is REFUSED at create, because that value becomes
//!   the directory the session's tools run in;
//! * a request with NO `cwd` is untouched — every pre-#305 client keeps its
//!   exact behaviour.
//!
//! The disabled-entry and no-cwd cases are the controls. Without them a server
//! that simply never gated anything would pass the auto-approval assertion.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, Stream, StreamExt};

use wcore_acp::allowlist::ProjectAllowlist;
use wcore_acp::error::AcpError;
use wcore_acp::protocol::{MessageEvent, ToolCall, ToolResult};
use wcore_acp::server::AcpServer;
use wcore_acp::transport::RestTransport;
use wcore_acp::turn::{ApprovalDecision, TurnEngine, TurnRequest};

/// A `TurnEngine` that always gates its one tool call, and records every
/// resolution it is handed. The recording is the point: "no gate reached the
/// client" is only half the claim — the other half is that something ANSWERED
/// it, rather than the frame being quietly dropped and the tool left parked.
#[derive(Default)]
struct GatingEngine {
    resolved: Mutex<Vec<(String, String, bool)>>,
    /// The `cwd` of the last turn, so a test can prove the allowlisted
    /// directory actually reaches the engine instead of being validated and
    /// then forgotten.
    last_cwd: Mutex<Option<String>>,
}

impl GatingEngine {
    fn resolutions(&self) -> Vec<(String, String, bool)> {
        self.resolved.lock().expect("resolutions lock").clone()
    }
    fn last_cwd(&self) -> Option<String> {
        self.last_cwd.lock().expect("cwd lock").clone()
    }
}

fn write_call() -> ToolCall {
    ToolCall {
        id: "call-1".to_string(),
        name: "Write".to_string(),
        input: serde_json::json!({ "file_path": "notes.md", "content": "x" }),
    }
}

#[async_trait]
impl TurnEngine for GatingEngine {
    async fn run_turn(
        &self,
        req: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        *self.last_cwd.lock().expect("cwd lock") = req.cwd.clone();
        Ok(stream::iter(vec![
            MessageEvent::ToolCall { call: write_call() },
            MessageEvent::ApprovalRequired {
                call: write_call(),
                reason: "mutating tool Write requires approval".to_string(),
                resume_token: String::new(),
            },
            MessageEvent::ToolResult {
                result: ToolResult {
                    call_id: "call-1".to_string(),
                    output: serde_json::json!("written"),
                    is_error: false,
                },
            },
            MessageEvent::Done {
                stop_reason: "end_turn".to_string(),
                turn_id: String::new(),
            },
        ])
        .boxed())
    }

    async fn resolve_approval(
        &self,
        session_id: &str,
        call_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), AcpError> {
        self.resolved.lock().expect("resolutions lock").push((
            session_id.to_string(),
            call_id.to_string(),
            decision.approved,
        ));
        Ok(())
    }
}

/// Platform-correct absolute path for a project root.
fn abs(rest: &str) -> String {
    if cfg!(windows) {
        format!("C:\\{}", rest.replace('/', "\\"))
    } else {
        format!("/{rest}")
    }
}

fn child_of(root: &str, child: &str) -> String {
    if cfg!(windows) {
        format!("{root}\\{}", child.replace('/', "\\"))
    } else {
        format!("{root}/{child}")
    }
}

struct Harness {
    base: String,
    client: reqwest::Client,
    engine: Arc<GatingEngine>,
    server: Arc<AcpServer>,
}

async fn harness() -> Harness {
    let engine = Arc::new(GatingEngine::default());
    let server = Arc::new(
        AcpServer::new()
            .with_turn_engine(Arc::clone(&engine) as Arc<dyn TurnEngine>)
            .with_allowlist(Arc::new(ProjectAllowlist::new())),
    );
    let app = RestTransport::new(Arc::clone(&server)).router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    Harness {
        base: format!("http://{addr}"),
        #[allow(clippy::disallowed_methods)] // localhost roundtrip; no proxy policy needed
        client: reqwest::Client::new(),
        engine,
        server,
    }
}

impl Harness {
    async fn add_project(&self, path: &str, enabled: bool) -> reqwest::Response {
        self.client
            .put(format!("{}/v1/approvals/projects", self.base))
            .json(&serde_json::json!({ "path": path, "enabled": enabled }))
            .send()
            .await
            .unwrap()
    }

    async fn create_session(&self, cwd: Option<&str>) -> reqwest::Response {
        let body = match cwd {
            Some(cwd) => serde_json::json!({ "cwd": cwd }),
            None => serde_json::json!({}),
        };
        self.client
            .post(format!("{}/v1/sessions", self.base))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn prompt(&self, session_id: &str) -> Vec<MessageEvent> {
        let resp = self
            .client
            .post(format!("{}/v1/sessions/{session_id}/prompt", self.base))
            .json(&serde_json::json!({ "text": "write the notes" }))
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
}

fn has_gate(frames: &[MessageEvent]) -> bool {
    frames
        .iter()
        .any(|e| matches!(e, MessageEvent::ApprovalRequired { .. }))
}

// ── The REST surface itself ──────────────────────────────────────────────

#[tokio::test]
async fn the_allowlist_is_listed_edited_and_deleted_over_rest() {
    let h = harness().await;
    let root = abs("srv/webapp");

    let listed: serde_json::Value = h
        .client
        .get(format!("{}/v1/approvals/projects", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        listed["projects"].as_array().map(Vec::len),
        Some(0),
        "a fresh server lists no approved projects"
    );

    let created: serde_json::Value = h.add_project(&root, true).await.json().await.unwrap();
    let id = created["id"].as_str().expect("entry id").to_string();
    assert_eq!(created["enabled"], serde_json::json!(true));

    // Re-PUT the same path with enabled=false: one entry, flipped in place.
    let flipped: serde_json::Value = h.add_project(&root, false).await.json().await.unwrap();
    assert_eq!(flipped["id"].as_str(), Some(id.as_str()));
    let listed: serde_json::Value = h
        .client
        .get(format!("{}/v1/approvals/projects", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["projects"].as_array().map(Vec::len), Some(1));
    assert_eq!(listed["projects"][0]["enabled"], serde_json::json!(false));

    let resp = h
        .client
        .delete(format!("{}/v1/approvals/projects/{id}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    let resp = h
        .client
        .delete(format!("{}/v1/approvals/projects/{id}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        404,
        "a second delete of the same id is a clean not-found"
    );
}

#[tokio::test]
async fn a_relative_project_path_is_refused_by_the_route() {
    let h = harness().await;
    let resp = h.add_project("relative/dir", true).await;
    assert_eq!(
        resp.status(),
        400,
        "a non-absolute project root cannot be compared against a session cwd, \
         so it is refused at the door rather than stored and silently never matching"
    );
}

// ── The behaviour the allowlist exists for ───────────────────────────────

#[tokio::test]
async fn a_session_under_an_enabled_project_never_shows_its_host_a_gate() {
    let h = harness().await;
    let root = abs("srv/webapp");
    let cwd = child_of(&root, "packages/api");
    assert_eq!(h.add_project(&root, true).await.status(), 200);

    let resp = h.create_session(Some(&cwd)).await;
    assert_eq!(resp.status(), 200, "an enabled project accepts a session");
    let created: serde_json::Value = resp.json().await.unwrap();
    let id = created["session_id"].as_str().unwrap().to_string();

    let frames = h.prompt(&id).await;
    assert!(
        !has_gate(&frames),
        "a session under an ENABLED project must not prompt; frames={frames:?}"
    );
    assert!(
        frames
            .iter()
            .any(|e| matches!(e, MessageEvent::ToolResult { .. })),
        "the tool result must still be reported - the stream loses the question, \
         never the record; frames={frames:?}"
    );

    let resolutions = h.engine.resolutions();
    assert_eq!(
        resolutions,
        vec![(id.clone(), "call-1".to_string(), true)],
        "the gate must be ANSWERED through the same resolve path a host uses, \
         not merely dropped from the stream"
    );
    assert_eq!(
        h.engine.last_cwd(),
        Some(cwd),
        "the allowlisted directory must reach the engine; validating it and then \
         running somewhere else would make the whole grant a fiction"
    );
}

#[tokio::test]
async fn a_session_under_a_disabled_project_still_gates() {
    let h = harness().await;
    let root = abs("srv/webapp");
    let cwd = child_of(&root, "packages/api");
    assert_eq!(h.add_project(&root, false).await.status(), 200);

    let created: serde_json::Value = h.create_session(Some(&cwd)).await.json().await.unwrap();
    let id = created["session_id"].as_str().unwrap().to_string();

    let frames = h.prompt(&id).await;
    assert!(
        has_gate(&frames),
        "a LISTED BUT DISABLED project must keep gating; if this passes only \
         because nothing ever gates, the enabled-project test proves nothing. \
         frames={frames:?}"
    );
    assert!(
        h.engine.resolutions().is_empty(),
        "nothing may answer a gate on the host's behalf under a disabled entry"
    );
}

#[tokio::test]
async fn a_cwd_outside_the_allowlist_is_refused_at_create() {
    let h = harness().await;
    assert_eq!(h.add_project(&abs("srv/webapp"), true).await.status(), 200);

    let resp = h.create_session(Some(&abs("etc"))).await;
    assert_eq!(
        resp.status(),
        400,
        "cwd becomes the directory the session's tools run in, so an unlisted \
         one is refused rather than accepted and quietly ignored"
    );

    // The classic string-prefix hole, over the wire.
    let resp = h.create_session(Some(&abs("srv/webapp-staging"))).await;
    assert_eq!(
        resp.status(),
        400,
        "'webapp-staging' merely shares a name prefix with the approved 'webapp'"
    );
}

#[tokio::test]
async fn a_session_with_no_cwd_is_exactly_the_pre_change_behaviour() {
    let h = harness().await;
    assert_eq!(h.add_project(&abs("srv/webapp"), true).await.status(), 200);

    let resp = h.create_session(None).await;
    assert_eq!(resp.status(), 200);
    let created: serde_json::Value = resp.json().await.unwrap();
    let id = created["session_id"].as_str().unwrap().to_string();

    let frames = h.prompt(&id).await;
    assert!(
        has_gate(&frames),
        "an enabled project elsewhere on the list must not auto-approve a session \
         that never named one; frames={frames:?}"
    );
    assert_eq!(
        h.engine.last_cwd(),
        None,
        "no cwd means the server's own launch directory, as before"
    );
    // The server is reachable and the allowlist is non-empty - so the gate above
    // is the posture, not an accident of an unconfigured server.
    assert!(!h.server.allowlist().is_empty().await);
}
