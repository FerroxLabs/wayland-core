//! Role refusals and command idempotency, enforced on the SERVER's request
//! path and observed by a real client over a real socket.
//!
//! Phase 24 Success Criterion 4, F24-04, threat T-24-03-02.
//!
//! # What this file is answering
//!
//! Plan 24-03 built `roles.rs` and `idempotency.rs` and then said of them:
//!
//! > The contracts exist; the plane does not. `server.rs` does not yet call
//! > `authorize` before dispatch.
//!
//! Every assertion below therefore crosses the network. The role decision is
//! taken from the principal the SERVER's verifier produced — never from
//! anything in the request body — and the idempotency identity is a header a
//! real client sent.
//!
//! # The two refusals must stay apart, and that is measured on ONE server
//!
//! `401` and `403` are asserted against the same running instance in the same
//! test, because the failure mode being excluded is not "403 is missing" but
//! "everything collapses into one shape". Two tests against two servers could
//! both pass while the server answered every refusal identically.
//!
//! # Effects are counted from somewhere other than the ledger
//!
//! The idempotency assertions never ask the ledger how many effects happened.
//! They ask the SESSION LIST — an independent surface that would show two
//! sessions if two were created. A ledger reporting on its own guarantee is a
//! delivery system attesting to its own completeness, which is the exact shape
//! this programme has been burned by.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use tokio::net::TcpListener;

use wcore_acp::auth::{AuthSchemeKind, Principal, Verifier};
use wcore_acp::client::AcpClient;
use wcore_acp::error::AcpError;
use wcore_acp::protocol::{
    ErrorCode, JsonRpcError, MessageEvent, MessageSendRequest, SessionCreateRequest,
};
use wcore_acp::roles::{Role, RolePolicy};
use wcore_acp::server::AcpServer;
use wcore_acp::transport::{HttpHandler, HttpSseTransport};
use wcore_acp::turn::{TurnEngine, TurnRequest};

/// Maps an API key to the principal id it authenticates.
///
/// A test-local [`Verifier`] rather than `ApiKeyVerifier`, which reads the OS
/// keychain — a hermetic test must not depend on machine state, and a test that
/// silently skipped when the keychain was unavailable would be a gate that
/// cannot fail.
struct MapVerifier {
    keys: HashMap<String, String>,
}

impl Verifier for MapVerifier {
    fn verify(&self, headers: &[(String, String)]) -> Result<Principal, AcpError> {
        let presented = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-api-key"))
            .map(|(_, v)| v.clone())
            .ok_or_else(|| AcpError::Auth("missing api key".to_string()))?;
        self.keys
            .get(&presented)
            .map(|id| Principal {
                id: id.clone(),
                scheme: AuthSchemeKind::ApiKey,
            })
            .ok_or_else(|| AcpError::Auth("api key mismatch".to_string()))
    }
}

struct DoneEngine;

#[async_trait]
impl TurnEngine for DoneEngine {
    async fn run_turn(
        &self,
        _req: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        Ok(futures::stream::iter(vec![MessageEvent::Done {
            stop_reason: "end_turn".to_string(),
            turn_id: String::new(),
        }])
        .boxed())
    }
}

fn verifier() -> Arc<dyn Verifier> {
    let mut keys = HashMap::new();
    keys.insert("key-viewer".to_string(), "acct-viewer".to_string());
    keys.insert("key-operator".to_string(), "acct-operator".to_string());
    keys.insert("key-admin".to_string(), "acct-admin".to_string());
    keys.insert("key-stranger".to_string(), "acct-stranger".to_string());
    Arc::new(MapVerifier { keys })
}

fn policy() -> RolePolicy {
    // `acct-stranger` is DELIBERATELY absent: it authenticates and holds no
    // role, which is the case that separates "deny-all on an omission" from
    // "fall through to the lowest role".
    RolePolicy::new()
        .grant("acct-viewer", Role::Viewer)
        .grant("acct-operator", Role::Operator)
        .grant("acct-admin", Role::Admin)
}

/// Serve a real server with auth installed, optionally with a role policy.
async fn serve(with_policy: bool) -> (String, AcpServer, tokio::task::JoinHandle<()>) {
    let mut server = AcpServer::new().with_turn_engine(Arc::new(DoneEngine));
    if with_policy {
        server = server.with_role_policy(policy());
    }
    let app = HttpSseTransport::new(Arc::new(server.clone()))
        .with_verifier(verifier())
        .router();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), server, handle)
}

fn client(base: &str, key: &str) -> AcpClient {
    AcpClient::new(base).expect("client").with_api_key(key)
}

/// A raw HTTP client for the assertions that must read the wire itself.
///
/// Built through the workspace egress chokepoint rather than `reqwest`
/// directly — the lint that forbids the latter is a real boundary, and a test
/// is not a reason to step around it.
fn raw_http() -> wcore_egress::EgressClient {
    wcore_egress::EgressClient::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("egress client")
}

fn create_req() -> SessionCreateRequest {
    SessionCreateRequest {
        model: None,
        tools: Vec::new(),
        system_prompt: None,
        agent: None,
        mcp_servers: Vec::new(),
    }
}

// ── Roles ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_role_refusal_and_an_auth_failure_are_different_answers_from_one_server() {
    let (base, _srv, _h) = serve(true).await;

    // POSITIVE CONTROL, first and deliberately. An operator creates a session
    // and sends a message. Without this, every refusal below could be caused by
    // the route being broken, the verifier rejecting everyone, or the policy
    // granting nothing — and all three would look identical to a passing test.
    let operator = client(&base, "key-operator");
    let created = operator
        .create_session(create_req())
        .await
        .expect("an operator may create a session");
    let mut s = operator
        .send_message(MessageSendRequest {
            session_id: created.session_id.clone(),
            text: "go".to_string(),
            tools: Vec::new(),
        })
        .await
        .expect("an operator may send a message");
    assert!(
        s.next().await.is_some(),
        "the positive control must actually stream a turn"
    );

    // A VIEWER on the very same route, same server, same session.
    let viewer = client(&base, "key-viewer");
    // The Ok variant is a boxed stream and is not `Debug`, so `expect_err`
    // cannot be used here — match instead.
    match viewer
        .send_message(MessageSendRequest {
            session_id: created.session_id.clone(),
            text: "go".to_string(),
            tools: Vec::new(),
        })
        .await
    {
        Err(AcpError::Forbidden(_)) => {}
        Err(other) => panic!(
            "a role refusal must reach the client as Forbidden, not as a generic \
             transport failure and not as an auth challenge; got {other:?}"
        ),
        Ok(_) => panic!("a viewer must not be able to drive a turn"),
    }

    // A BAD CREDENTIAL on the same route. This is the discriminator: if the
    // server collapsed both refusals into one shape, this assertion and the one
    // above could not both hold.
    let stranger = AcpClient::new(&base).expect("client").with_api_key("nope");
    let err = stranger
        .list_sessions()
        .await
        .expect_err("an unknown key must not authenticate");
    assert!(
        matches!(err, AcpError::Auth(_)),
        "an unrecognised credential must be an AUTH failure; got {err:?}"
    );

    // …and a viewer CAN read, so the refusal above is about the method and not
    // about the viewer being locked out entirely.
    viewer
        .list_sessions()
        .await
        .expect("a viewer may list sessions");
}

#[tokio::test]
async fn the_wire_carries_the_distinct_codes_and_statuses_not_just_distinct_types() {
    // The typed client's mapping is one hop. This asserts the bytes on the
    // wire, so a client fixing up a wrong status cannot hide a wrong server.
    let (base, _srv, _h) = serve(true).await;
    let http = raw_http();
    let created = client(&base, "key-operator")
        .create_session(create_req())
        .await
        .expect("create");

    let forbidden = http
        .post(format!("{base}/sessions/{}/messages", created.session_id))
        .header("X-API-Key", "key-viewer")
        .json(&serde_json::json!({"text":"go"}))
        .send()
        .await
        .expect("send");
    assert_eq!(forbidden.status().as_u16(), 403);
    let body: JsonRpcError = forbidden.json().await.expect("error body");
    assert_eq!(
        body.code,
        ErrorCode::Forbidden.code(),
        "a role refusal must carry the Forbidden code, not the auth code"
    );
    assert!(
        body.message.contains("operator") && body.message.contains("viewer"),
        "the refusal must name what is required and what is held, or the \
         operator has nothing to act on: {}",
        body.message
    );
    assert!(
        !body.message.contains("acct-viewer"),
        "the refusal must not name the principal: {}",
        body.message
    );

    let unauthorized = http
        .get(format!("{base}/sessions"))
        .header("X-API-Key", "not-a-key")
        .send()
        .await
        .expect("send");
    assert_eq!(unauthorized.status().as_u16(), 401);
    let body: JsonRpcError = unauthorized.json().await.expect("error body");
    assert_eq!(body.code, ErrorCode::AuthRequired.code());
}

#[tokio::test]
async fn a_principal_the_policy_never_named_is_denied_even_a_read() {
    let (base, _srv, _h) = serve(true).await;
    let stranger = client(&base, "key-stranger");
    // It AUTHENTICATES — the verifier knows the key — and holds no role.
    let err = stranger
        .list_sessions()
        .await
        .expect_err("an unroled principal must be denied");
    assert!(
        matches!(err, AcpError::Forbidden(_)),
        "an authenticated principal with no role must be FORBIDDEN, not \
         unauthenticated: {err:?}"
    );
}

#[tokio::test]
async fn with_no_policy_installed_the_server_performs_no_role_gating_and_reports_that() {
    // This pins the feature-OFF state so it cannot drift into either lie:
    // silently gating (locking out an existing operator) or being reported as
    // enforcement that passed. A `has_role_policy()` of false is "not
    // configured" — it is neither a green nor a deny-all.
    let (base, srv, _h) = serve(false).await;
    assert!(
        !srv.has_role_policy(),
        "this server was built without a policy and must say so"
    );
    let viewer = client(&base, "key-viewer");
    let created = viewer
        .create_session(create_req())
        .await
        .expect("with no policy, roles gate nothing");
    let streamed = viewer
        .send_message(MessageSendRequest {
            session_id: created.session_id,
            text: "go".to_string(),
            tools: Vec::new(),
        })
        .await
        .expect("with no policy, roles gate nothing");
    drop(streamed);

    // Authentication is still enforced — the two are independent, and turning
    // roles off must not turn auth off.
    let err = AcpClient::new(&base)
        .expect("client")
        .with_api_key("nope")
        .list_sessions()
        .await
        .expect_err("auth is still enforced with no role policy");
    assert!(matches!(err, AcpError::Auth(_)), "got {err:?}");
}

#[tokio::test]
async fn an_unclassified_route_is_refused_rather_than_reaching_dispatch() {
    // A route added without a role-table entry must fail LOUDLY for ordinary
    // principals rather than quietly becoming world-reachable. `/agents` is
    // classified; a made-up path is not.
    let (base, _srv, _h) = serve(true).await;
    let http = raw_http();
    // Positive control: a classified read succeeds for a viewer.
    let ok = http
        .get(format!("{base}/agents"))
        .header("X-API-Key", "key-viewer")
        .send()
        .await
        .expect("send");
    assert_eq!(ok.status().as_u16(), 200);
    // An unclassified path resolves to itself, requires Admin, and a viewer is
    // refused BEFORE axum ever gets to report the route as missing.
    let refused = http
        .get(format!("{base}/some-route-added-later"))
        .header("X-API-Key", "key-viewer")
        .send()
        .await
        .expect("send");
    assert_eq!(
        refused.status().as_u16(),
        403,
        "an unclassified route must be refused, not fall through to the lowest \
         privilege"
    );
}

// ── Idempotency ──────────────────────────────────────────────────────────

#[tokio::test]
async fn a_repeated_key_yields_one_session_counted_from_the_session_list() {
    let (base, _srv, _h) = serve(true).await;
    let admin = client(&base, "key-admin");

    let first = admin
        .create_session_idempotent("req-1", create_req())
        .await
        .expect("first create");
    let second = admin
        .create_session_idempotent("req-1", create_req())
        .await
        .expect("the repeat must be a replay, not a refusal");

    assert_eq!(
        first.session_id, second.session_id,
        "both receipts must name the SAME session, or the caller acted on an \
         answer the server has since contradicted"
    );
    // The effect count comes from a DIFFERENT surface than the ledger.
    let list = admin.list_sessions().await.expect("list");
    assert_eq!(
        list.sessions.len(),
        1,
        "exactly one session must exist; the ledger's own opinion is not \
         evidence of how many effects occurred"
    );

    // Positive control: a DIFFERENT key really does create a second session, so
    // the single session above is caused by the repeated key and not by the
    // create path being broken.
    admin
        .create_session_idempotent("req-2", create_req())
        .await
        .expect("a fresh key creates");
    assert_eq!(admin.list_sessions().await.expect("list").sessions.len(), 2);
}

#[tokio::test]
async fn a_key_reused_for_a_different_command_is_refused_and_the_original_still_replays() {
    let (base, _srv, _h) = serve(true).await;
    let admin = client(&base, "key-admin");
    let original = SessionCreateRequest {
        model: Some("model-a".to_string()),
        ..create_req()
    };
    let first = admin
        .create_session_idempotent("req-1", original.clone())
        .await
        .expect("first create");

    let different = SessionCreateRequest {
        model: Some("model-b".to_string()),
        ..create_req()
    };
    let err = admin
        .create_session_idempotent("req-1", different)
        .await
        .expect_err(
            "a different command under a used identity must be refused: accepting \
             it performs a second effect under a key the caller believes is \
             idempotent, and replaying it hands back someone else's answer",
        );
    assert!(matches!(err, AcpError::Protocol(_)), "got {err:?}");

    // POSITIVE CONTROL: the ORIGINAL command under the same key still replays,
    // so the refusal is caused by the command differing and not by the identity
    // merely being present.
    let replay = admin
        .create_session_idempotent("req-1", original)
        .await
        .expect("the original command must still replay");
    assert_eq!(replay.session_id, first.session_id);
    assert_eq!(
        admin.list_sessions().await.expect("list").sessions.len(),
        1,
        "the refused command must not have created anything"
    );
}

#[tokio::test]
async fn a_repeated_delete_under_one_key_does_not_become_a_spurious_not_found() {
    let (base, _srv, _h) = serve(true).await;
    let admin = client(&base, "key-admin");
    let created = admin.create_session(create_req()).await.expect("create");

    admin
        .delete_session_idempotent("del-1", &created.session_id)
        .await
        .expect("first delete");
    admin
        .delete_session_idempotent("del-1", &created.session_id)
        .await
        .expect(
            "the retry must succeed: re-issuing the delete would now report \
             not-found, turning a successful retry into a failure, which is the \
             precise reason the caller sent a key",
        );
    assert_eq!(admin.list_sessions().await.expect("list").sessions.len(), 0);

    // POSITIVE CONTROL on the refusal path: WITHOUT a key, a second delete is
    // still an honest not-found. Idempotency is opt-in and must not have
    // quietly made every delete succeed.
    let again = admin.create_session(create_req()).await.expect("create");
    admin
        .delete_session(&again.session_id)
        .await
        .expect("first bare delete");
    let err = admin
        .delete_session(&again.session_id)
        .await
        .expect_err("a bare repeated delete is still not-found");
    assert!(matches!(err, AcpError::Session(_)), "got {err:?}");
}

/// A handler with no idempotency implementation, exercising the trait default.
struct BareHandler;

#[async_trait]
impl HttpHandler for BareHandler {
    async fn create_session(
        &self,
        _req: SessionCreateRequest,
    ) -> Result<wcore_acp::protocol::SessionCreateResponse, AcpError> {
        Ok(wcore_acp::protocol::SessionCreateResponse {
            session_id: "sess-1".to_string(),
            model: None,
        })
    }
    async fn list_sessions(&self) -> Result<wcore_acp::protocol::SessionListResponse, AcpError> {
        Ok(wcore_acp::protocol::SessionListResponse {
            sessions: Vec::new(),
        })
    }
    async fn get_session(
        &self,
        _id: String,
    ) -> Result<wcore_acp::protocol::SessionGetResponse, AcpError> {
        Err(AcpError::Session("session not found".to_string()))
    }
    async fn delete_session(&self, _id: String) -> Result<(), AcpError> {
        Ok(())
    }
    async fn send_message(
        &self,
        _req: MessageSendRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        Ok(futures::stream::empty().boxed())
    }
}

#[tokio::test]
async fn a_handler_that_cannot_honour_a_key_refuses_it_rather_than_ignoring_it() {
    // Ignoring the header is the dangerous option: the client believes its
    // retry is protected, retries, and gets a second effect. And the resume
    // route on such a handler must say UNSUPPORTED, never answer "you missed
    // nothing" from a surface that retains nothing.
    let app = HttpSseTransport::new(Arc::new(BareHandler)).router();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let base = format!("http://{addr}");
    let http = raw_http();

    // Positive control: without the header this handler works normally.
    let ok = http
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("send");
    assert_eq!(ok.status().as_u16(), 200);

    let refused = http
        .post(format!("{base}/sessions"))
        .header("Idempotency-Key", "req-1")
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("send");
    assert_eq!(
        refused.status().as_u16(),
        400,
        "a key this handler cannot honour must be refused, not ignored"
    );

    let resume = http
        .get(format!("{base}/sessions/sess-1/events"))
        .query(&[("stream_id", "sess-1@x"), ("position", "0")])
        .send()
        .await
        .expect("send");
    assert_eq!(
        resume.status().as_u16(),
        501,
        "a handler that retains no events must say so; answering with an empty \
         list would tell a client that asked what it missed that it missed \
         nothing, from a surface that never looked"
    );
}

// ── The second door into the same server (F24-E-H1) ──────────────────────

#[tokio::test]
async fn the_rest_surface_cannot_be_used_to_walk_around_a_role_refusal() {
    // MEASURED LIVE before it was fixed: with `--role viewer` on the shipped
    // binary, the SAME key on the SAME server was refused `POST /sessions`
    // with 403 and ACCEPTED at `POST /v1/sessions` with 200. Authentication was
    // shared between the two surfaces and authorization was not, so a control
    // the operator had switched on guarded exactly one of two doors into one
    // `AcpServer`.
    //
    // Both routers are mounted on ONE listener here, exactly as
    // `wayland-core acp serve` mounts them, because a test that served only the
    // REST router would not be testing the situation that produced the defect.
    let server = AcpServer::new()
        .with_turn_engine(Arc::new(DoneEngine))
        .with_role_policy(policy());
    let shared = Arc::new(server);
    let acp = HttpSseTransport::new(Arc::clone(&shared))
        .with_verifier(verifier())
        .router();
    let rest = wcore_acp::transport::RestTransport::new(Arc::clone(&shared))
        .with_verifier(verifier())
        .router();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, acp.merge(rest)).await;
    });
    let base = format!("http://{addr}");
    let http = raw_http();

    // POSITIVE CONTROL: an OPERATOR reaches the REST create, so the refusal
    // below is caused by the role and not by the REST surface being broken,
    // unreachable, or refusing everyone.
    let allowed = http
        .post(format!("{base}/v1/sessions"))
        .header("X-API-Key", "key-operator")
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("send");
    assert_eq!(
        allowed.status().as_u16(),
        200,
        "positive control: an operator must still reach POST /v1/sessions"
    );

    // The two doors must now agree for a VIEWER.
    let acp_code = http
        .post(format!("{base}/sessions"))
        .header("X-API-Key", "key-viewer")
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("send")
        .status()
        .as_u16();
    let rest_code = http
        .post(format!("{base}/v1/sessions"))
        .header("X-API-Key", "key-viewer")
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("send")
        .status()
        .as_u16();
    assert_eq!(acp_code, 403, "the ACP surface refuses a viewer");
    assert_eq!(
        rest_code, 403,
        "the REST surface must refuse the same principal the same operation; \
         a role control that guards one of two doors into one server is not a \
         control, and the operator who switched it on has no way to see that"
    );

    // Authentication is still enforced on REST, and stays DISTINCT from the
    // role refusal — the same 401-vs-403 separation, on the second surface.
    let unauth = http
        .post(format!("{base}/v1/sessions"))
        .header("X-API-Key", "not-a-key")
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("send");
    assert_eq!(unauth.status().as_u16(), 401);

    // And a read the viewer IS entitled to still works on REST, so the fix
    // refuses by role rather than by blanket-denying the surface.
    let read = http
        .get(format!("{base}/v1/sessions"))
        .header("X-API-Key", "key-viewer")
        .send()
        .await
        .expect("send");
    assert_eq!(
        read.status().as_u16(),
        200,
        "a viewer may still read on REST; the fix must gate by role, not shut \
         the surface"
    );
}

#[tokio::test]
async fn the_unauthenticated_spec_routes_stay_reachable() {
    // `/openapi.json` and `/doc` are a documented public carve-out. The
    // authorization added above sits INSIDE the authenticated layer, so it must
    // not have swept them up — a fix that broke spec discovery would be a new
    // defect wearing the old one's clothes.
    let server = AcpServer::new().with_role_policy(RolePolicy::new());
    let rest = wcore_acp::transport::RestTransport::new(Arc::new(server))
        .with_verifier(verifier())
        .router();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, rest).await;
    });
    let http = raw_http();
    for path in ["/openapi.json", "/doc"] {
        let code = http
            .get(format!("http://{addr}{path}"))
            .send()
            .await
            .expect("send")
            .status()
            .as_u16();
        assert_eq!(code, 200, "{path} must stay unauthenticated and reachable");
    }
}
