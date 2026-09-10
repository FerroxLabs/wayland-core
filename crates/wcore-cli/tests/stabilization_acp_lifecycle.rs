//! W02 / wayland#1324: DELETE must stop admitted work before acknowledging.
//!
//! Real AcpServer -> EngineTurnEngine -> AgentEngine, with a provider future
//! held at an explicit barrier. No paid provider, socket, tool or credential
//! backend is involved. Durable recovery and transport status are separate
//! acceptance cases; this regression isolates the missing cancellation seam.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio::time::timeout;
use wcore_acp::protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest};
use wcore_acp::server::AcpServer;
use wcore_acp::transport::http::HttpHandler;
use wcore_cli::acp_engine::EngineTurnEngine;
use wcore_config::config::Config;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

const REPLY: &str = "provider completed after its release barrier";
const TEST_DEADLINE: Duration = Duration::from_secs(20);
const CLOSE_DEADLINE: Duration = Duration::from_secs(10);

struct HeldProvider {
    entered: Notify,
    release: Semaphore,
    active: AtomicBool,
    completions: AtomicUsize,
    entries: AtomicUsize,
}

impl HeldProvider {
    fn new() -> Self {
        Self {
            entered: Notify::new(),
            release: Semaphore::new(0),
            active: AtomicBool::new(false),
            completions: AtomicUsize::new(0),
            entries: AtomicUsize::new(0),
        }
    }
}

/// Dropping the provider future is observable even when it never completes.
struct ActiveCall<'a>(&'a AtomicBool);

impl Drop for ActiveCall<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[async_trait]
impl LlmProvider for HeldProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.active.store(true, Ordering::SeqCst);
        self.entries.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveCall(&self.active);
        self.entered.notify_one();
        let permit = self.release.acquire().await.expect("fixture stays open");
        permit.forget();
        self.completions.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(2);
        tx.try_send(LlmEvent::TextDelta(REPLY.to_string()))
            .expect("empty fixture channel");
        tx.try_send(LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage {
                input_tokens: 1,
                output_tokens: 1,
                ..Default::default()
            },
        })
        .expect("fixture channel has terminal capacity");
        Ok(rx)
    }
}

fn server(provider: Arc<HeldProvider>, workspace: &std::path::Path) -> AcpServer {
    // Default avoids loading the invoking account's config/credentials.
    let mut config = Config {
        model: "lifecycle-fixture".to_string(),
        ..Default::default()
    };
    config.session.enabled = false;
    config.memory.enabled = false;
    AcpServer::new().with_turn_engine(Arc::new(EngineTurnEngine::with_provider(
        config,
        workspace.to_string_lossy().into_owned(),
        provider,
    )))
}

async fn create_session(server: &AcpServer) -> String {
    server
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("create ACP session")
        .session_id
}

/// One terminal, last, naming its turn. Returns that turn id so a caller with
/// several streams can check each ended at its own turn (wayland#1356 review).
fn assert_one_terminal(frames: &[MessageEvent]) -> String {
    let terminal = |event: &&MessageEvent| {
        matches!(
            event,
            MessageEvent::Done { .. } | MessageEvent::Error { .. }
        )
    };
    assert_eq!(frames.iter().filter(terminal).count(), 1, "{frames:?}");
    let Some(MessageEvent::Done { turn_id, .. } | MessageEvent::Error { turn_id, .. }) =
        frames.last()
    else {
        panic!("terminal must be last: {frames:?}");
    };
    assert!(
        !turn_id.is_empty(),
        "the terminal names its turn: {frames:?}"
    );
    turn_id.clone()
}

#[tokio::test]
async fn delete_acknowledgement_waits_for_admitted_provider_cancellation() {
    let workspace = tempfile::tempdir().expect("isolated workspace");
    let provider = Arc::new(HeldProvider::new());
    let server = server(provider.clone(), workspace.path());
    let session_id = create_session(&server).await;
    let stream = timeout(
        TEST_DEADLINE,
        server.send_message(MessageSendRequest {
            session_id: session_id.clone(),
            text: "Return the fixture reply without tools.".to_string(),
            tools: Vec::new(),
        }),
    )
    .await
    .expect("bootstrap deadline")
    .expect("admit turn");
    timeout(TEST_DEADLINE, provider.entered.notified())
        .await
        .expect("the real engine reached the held provider");
    assert!(provider.active.load(Ordering::SeqCst));

    // This is the same acknowledgement the REST handler maps to HTTP 204.
    let deleted = timeout(CLOSE_DEADLINE, server.delete_session(session_id.clone())).await;
    let active_at_ack = provider.active.load(Ordering::SeqCst);
    // Release and drain even on the broken implementation, before asserting,
    // so the regression does not abandon the very turn it exposes.
    provider.release.add_permits(1);
    let frames: Vec<_> = timeout(TEST_DEADLINE, stream.collect())
        .await
        .expect("turn terminal deadline");

    deleted.expect("DELETE cleanup deadline").expect("DELETE");
    assert_one_terminal(&frames);
    assert!(server.get_session(session_id).await.is_err());
    assert!(
        !active_at_ack,
        "DELETE acknowledged while provider work was still active; \
         completions after release: {}; stream: {frames:?}",
        provider.completions.load(Ordering::SeqCst)
    );
    assert_eq!(provider.completions.load(Ordering::SeqCst), 0);
    assert!(
        !frames
            .iter()
            .any(|event| matches!(event, MessageEvent::TextDelta { text } if text.contains(REPLY)))
    );
}

#[tokio::test]
async fn open_session_control_completes_the_same_held_provider() {
    let workspace = tempfile::tempdir().expect("isolated workspace");
    let provider = Arc::new(HeldProvider::new());
    let server = server(provider.clone(), workspace.path());
    let session_id = create_session(&server).await;
    let stream = timeout(
        TEST_DEADLINE,
        server.send_message(MessageSendRequest {
            session_id,
            text: "Return the fixture reply without tools.".to_string(),
            tools: Vec::new(),
        }),
    )
    .await
    .expect("bootstrap deadline")
    .expect("admit control turn");
    timeout(TEST_DEADLINE, provider.entered.notified())
        .await
        .expect("control reached provider");
    assert!(provider.active.load(Ordering::SeqCst));
    provider.release.add_permits(1);
    let frames: Vec<_> = timeout(TEST_DEADLINE, stream.collect())
        .await
        .expect("control terminal deadline");
    assert_one_terminal(&frames);
    assert_eq!(provider.completions.load(Ordering::SeqCst), 1);
    assert!(
        frames
            .iter()
            .any(|event| matches!(event, MessageEvent::TextDelta { text } if text.contains(REPLY)))
    );
    assert!(matches!(
        frames.last(),
        Some(MessageEvent::Done { stop_reason, .. }) if stop_reason == "end_turn"
    ));
}

#[tokio::test]
async fn delete_cancels_a_queued_turn_before_it_reaches_the_provider() {
    let workspace = tempfile::tempdir().expect("isolated workspace");
    let provider = Arc::new(HeldProvider::new());
    let server = server(provider.clone(), workspace.path());
    let session_id = create_session(&server).await;
    let first = server
        .send_message(MessageSendRequest {
            session_id: session_id.clone(),
            text: "first".into(),
            tools: Vec::new(),
        })
        .await
        .expect("first turn");
    timeout(TEST_DEADLINE, provider.entered.notified())
        .await
        .expect("first entered");
    let queued = server
        .send_message(MessageSendRequest {
            session_id: session_id.clone(),
            text: "queued".into(),
            tools: Vec::new(),
        })
        .await
        .expect("queued turn");
    let deleted = timeout(CLOSE_DEADLINE, server.delete_session(session_id.clone())).await;
    provider.release.add_permits(2);
    let (first, queued): (Vec<_>, Vec<_>) = timeout(TEST_DEADLINE, async {
        tokio::join!(first.collect(), queued.collect())
    })
    .await
    .expect("both streams terminate");
    deleted.expect("delete deadline").expect("delete");
    let first_turn = assert_one_terminal(&first);
    let queued_turn = assert_one_terminal(&queued);
    assert_ne!(
        first_turn, queued_turn,
        "each stream must end at its own turn's terminal"
    );
    assert_eq!(
        provider.entries.load(Ordering::SeqCst),
        1,
        "queued turn dispatched"
    );
    assert_eq!(provider.completions.load(Ordering::SeqCst), 0);
    assert!(
        server
            .send_message(MessageSendRequest {
                session_id,
                text: "late".into(),
                tools: Vec::new(),
            })
            .await
            .is_err()
    );
}
