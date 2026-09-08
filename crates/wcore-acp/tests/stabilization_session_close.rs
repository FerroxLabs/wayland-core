//! Transport-neutral close ownership: cleanup errors, retries and caller loss.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use tokio::sync::{Notify, Semaphore};
use wcore_acp::server::AcpServer;
use wcore_acp::transport::http::HttpHandler;
use wcore_acp::turn::{ApprovalDecision, ApprovalScopeWire, TurnEngine, TurnRequest};
use wcore_acp::{
    AcpError,
    protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest},
};

struct ControlledClose {
    entered: Notify,
    release: Semaphore,
    fail_next: AtomicBool,
    panic_next: AtomicBool,
    closes: AtomicUsize,
    approvals: AtomicUsize,
}

impl ControlledClose {
    fn new(fail: bool) -> Self {
        Self {
            entered: Notify::new(),
            release: Semaphore::new(0),
            fail_next: AtomicBool::new(fail),
            panic_next: AtomicBool::new(false),
            closes: AtomicUsize::new(0),
            approvals: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl TurnEngine for ControlledClose {
    async fn run_turn(
        &self,
        _: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        Ok(Box::pin(stream::iter([MessageEvent::Done {
            stop_reason: "end_turn".into(),
            turn_id: "fixture".into(),
        }])))
    }

    async fn close_session(&self, _: &str) -> Result<(), AcpError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        assert!(
            !self.panic_next.swap(false, Ordering::SeqCst),
            "injected cleanup task panic"
        );
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(AcpError::Cleanup("fixture retained resource".into()));
        }
        self.entered.notify_one();
        self.release
            .acquire()
            .await
            .expect("fixture semaphore")
            .forget();
        Ok(())
    }

    async fn resolve_approval(
        &self,
        _: &str,
        _: &str,
        _: ApprovalDecision,
    ) -> Result<(), AcpError> {
        self.approvals.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn session(server: &AcpServer) -> String {
    server
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("create")
        .session_id
}

#[tokio::test]
async fn failed_close_keeps_admission_closed_and_retry_completes() {
    let engine = Arc::new(ControlledClose::new(true));
    let server = AcpServer::new().with_turn_engine(engine.clone());
    let id = session(&server).await;
    assert!(matches!(
        server.delete_session(id.clone()).await,
        Err(AcpError::Cleanup(_))
    ));
    assert!(
        server.get_session(id.clone()).await.is_ok(),
        "failed cleanup retains ownership"
    );
    assert!(matches!(
        server
            .send_message(MessageSendRequest {
                session_id: id.clone(),
                text: "late".into(),
                tools: Vec::new(),
            })
            .await,
        Err(AcpError::Cleanup(_))
    ));
    assert!(matches!(
        server
            .resolve_approval(
                id.clone(),
                "pending".into(),
                ApprovalDecision {
                    approved: true,
                    scope: ApprovalScopeWire::Once,
                    answer: None,
                    resume_token: None,
                }
            )
            .await,
        Err(AcpError::Cleanup(_))
    ));
    assert_eq!(engine.approvals.load(Ordering::SeqCst), 0);
    engine.release.add_permits(1);
    server.delete_session(id.clone()).await.expect("retry");
    assert_eq!(engine.closes.load(Ordering::SeqCst), 2);
    assert!(server.get_session(id).await.is_err());
}

#[tokio::test]
async fn disconnecting_delete_caller_does_not_abandon_cleanup() {
    let engine = Arc::new(ControlledClose::new(false));
    let server = AcpServer::new().with_turn_engine(engine.clone());
    let id = session(&server).await;
    let owned_server = server.clone();
    let owned_id = id.clone();
    let caller = tokio::spawn(async move { owned_server.delete_session(owned_id).await });
    tokio::time::timeout(Duration::from_secs(2), engine.entered.notified())
        .await
        .expect("cleanup entered");
    caller.abort();
    assert!(caller.await.expect_err("caller aborted").is_cancelled());
    assert!(server.get_session(id.clone()).await.is_ok());
    engine.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.get_session(id.clone()).await.is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned cleanup completed without its caller");
    assert_eq!(engine.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn panicked_close_can_retry_instead_of_reusing_unpublished_result() {
    let engine = Arc::new(ControlledClose::new(false));
    engine.panic_next.store(true, Ordering::SeqCst);
    let server = AcpServer::new().with_turn_engine(engine.clone());
    let id = session(&server).await;

    let first = tokio::time::timeout(Duration::from_secs(2), server.delete_session(id.clone()))
        .await
        .expect("panicked cleanup must be reported without waiting for the deadline");
    assert!(matches!(first, Err(AcpError::Cleanup(_))));
    assert!(server.get_session(id.clone()).await.is_ok());
    assert!(matches!(
        server
            .send_message(MessageSendRequest {
                session_id: id.clone(),
                text: "late".into(),
                tools: Vec::new(),
            })
            .await,
        Err(AcpError::Cleanup(_))
    ));

    engine.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), server.delete_session(id.clone()))
        .await
        .expect("retry deadline")
        .expect("retry performs cleanup");
    assert_eq!(engine.closes.load(Ordering::SeqCst), 2);
    assert!(server.get_session(id).await.is_err());
}

#[tokio::test]
async fn same_key_different_delete_target_conflicts_before_second_effect() {
    let engine = Arc::new(ControlledClose::new(false));
    let server = AcpServer::new().with_turn_engine(engine.clone());
    let first = session(&server).await;
    let second = session(&server).await;
    let owner = server.clone();
    let target = first.clone();
    let pending = tokio::spawn(async move {
        owner
            .delete_session_idempotent(Some("same-key"), target)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), engine.entered.notified())
        .await
        .expect("first cleanup started");
    let conflict = server
        .delete_session_idempotent(Some("same-key"), second.clone())
        .await;
    assert!(matches!(conflict, Err(AcpError::Protocol(_))));
    assert!(server.get_session(second).await.is_ok());
    assert_eq!(engine.closes.load(Ordering::SeqCst), 1);
    engine.release.add_permits(1);
    pending.await.expect("caller task").expect("first cleanup");
    server
        .delete_session_idempotent(Some("same-key"), first.clone())
        .await
        .expect("receipt replay");
    assert!(matches!(
        server.delete_session(first).await,
        Err(AcpError::Session(_))
    ));
}

struct CancelledTool {
    close: Arc<Semaphore>,
    turn_id: std::sync::Mutex<Option<String>>,
}

#[async_trait]
impl TurnEngine for CancelledTool {
    async fn run_turn(
        &self,
        _: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        let close = self.close.clone();
        Ok(Box::pin(
            stream::iter([MessageEvent::ToolCall {
                call: wcore_acp::protocol::ToolCall {
                    id: "owned-call".into(),
                    name: "hold_child".into(),
                    input: serde_json::json!({}),
                },
            }])
            .chain(stream::once(async move {
                close.acquire().await.expect("close signal").forget();
                MessageEvent::Done {
                    stop_reason: "cancelled".into(),
                    turn_id: String::new(),
                }
            })),
        ))
    }

    async fn run_turn_with_id(
        &self,
        req: TurnRequest,
        turn_id: String,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        *self.turn_id.lock().unwrap() = Some(turn_id.clone());
        Ok(wcore_acp::turn::bind_turn_id(
            self.run_turn(req).await?,
            turn_id,
        ))
    }

    async fn close_session(&self, _: &str) -> Result<(), AcpError> {
        self.close.add_permits(1);
        Ok(())
    }
}

#[tokio::test]
async fn close_preserves_unread_tool_call_and_correlated_engine_cancel_terminal() {
    let engine = Arc::new(CancelledTool {
        close: Arc::new(Semaphore::new(0)),
        turn_id: std::sync::Mutex::new(None),
    });
    let server = AcpServer::new().with_turn_engine(engine.clone());
    let id = session(&server).await;
    let response = server
        .send_message(MessageSendRequest {
            session_id: id.clone(),
            text: "run tool".into(),
            tools: vec![],
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.event_tip(&id).await.unwrap().position < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("tool call recorded before close");
    // Keep every live frame unread until cleanup has acknowledged completion.
    tokio::time::timeout(Duration::from_secs(2), server.delete_session(id))
        .await
        .expect("bounded close")
        .expect("owned cleanup");
    let frames = tokio::time::timeout(Duration::from_secs(1), response.collect::<Vec<_>>())
        .await
        .expect("delivery completed");
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert!(matches!(&frames[0], MessageEvent::ToolCall { call } if call.id == "owned-call"));
    let expected = engine.turn_id.lock().unwrap().clone().unwrap();
    assert!(!expected.is_empty());
    assert!(
        matches!(&frames[1], MessageEvent::Done { stop_reason, turn_id }
        if stop_reason == "cancelled" && turn_id == &expected),
        "{frames:?}"
    );
}
