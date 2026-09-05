use super::*;
use futures::StreamExt;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::sync::{Notify, Semaphore, mpsc};
use wcore_acp::protocol::{MessageSendRequest, SessionCreateRequest};
use wcore_acp::server::AcpServer;
use wcore_acp::transport::http::HttpHandler;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{StopReason, TokenUsage};

struct FixtureProvider {
    calls: AtomicUsize,
    send: bool,
}

#[async_trait]
impl wcore_providers::LlmProvider for FixtureProvider {
    async fn stream(
        &self,
        _: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, wcore_providers::ProviderError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let tool = self.send && call % 2 == 0;
        let (tx, rx) = mpsc::channel(2);
        if tool {
            tx.try_send(LlmEvent::ToolUse {
                id: "send-fixture".into(),
                name: "send_message".into(),
                input: serde_json::json!({"target":"slack:fixture", "message":"outbound control"}),
                extra: None,
            })
            .expect("tool frame");
        } else {
            tx.try_send(LlmEvent::TextDelta("fixture reply".into()))
                .expect("text frame");
        }
        tx.try_send(LlmEvent::Done {
            stop_reason: if tool {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        })
        .expect("terminal frame");
        Ok(rx)
    }
}

fn engine(workspace: &std::path::Path, send: bool) -> Arc<EngineTurnEngine> {
    let mut config = Config {
        model: "lifecycle-fixture".into(),
        ..Default::default()
    };
    config.session.enabled = false;
    config.memory.enabled = false;
    config.inbound_webhook.enabled = true; // outbound-only must override this.
    Arc::new(EngineTurnEngine::with_provider(
        config,
        workspace.to_string_lossy().into_owned(),
        Arc::new(FixtureProvider {
            calls: AtomicUsize::new(0),
            send,
        }),
    ))
}

async fn create(server: &AcpServer) -> String {
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
async fn closing_held_initialization_signals_before_admission_and_retry_converges() {
    let workspace = tempfile::tempdir().expect("workspace");
    let engine = engine(workspace.path(), false);
    let server = AcpServer::new().with_turn_engine(engine.clone());
    let id = create(&server).await;
    let other = create(&server).await;
    let initialization = Arc::new(Initialization::new());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let owner = engine.clone();
    let target = id.clone();
    let signal = initialization.clone();
    let gate = release.clone();
    let started = entered.clone();
    // Hold the owned production initialization call before it can return an
    // engine. Cancellation intentionally does not release this barrier.
    let task = tokio::spawn(async move {
        let _completion = InitializationCompletion(signal.clone());
        started.notify_one();
        gate.acquire().await.expect("gate").forget();
        let result = owner
            .build_session(
                &target,
                None,
                &[],
                &[],
                signal.cancel.clone(),
                signal.cleanup.clone(),
            )
            .await;
        signal
            .result
            .send_replace(Some(result.map(|_| ()).map_err(|e| e.to_string())));
    });
    *initialization.task.lock().await = Some(task);
    engine
        .initializers
        .lock()
        .await
        .insert(id.clone(), initialization.clone());
    entered.notified().await;
    let sender = server.clone();
    let target = id.clone();
    let send = tokio::spawn(async move {
        sender
            .send_message(MessageSendRequest {
                session_id: target,
                text: "held".into(),
                tools: Vec::new(),
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while initialization.result.receiver_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("send is waiting on the held initializer");
    // A different session still builds and executes while the first is held.
    let control = tokio::time::timeout(Duration::from_secs(20), async {
        server
            .send_message(MessageSendRequest {
                session_id: other,
                text: "control".into(),
                tools: Vec::new(),
            })
            .await
            .expect("other session")
            .collect::<Vec<_>>()
            .await
    })
    .await
    .expect("unrelated initialization progressed");
    assert!(
        control
            .iter()
            .any(|e| matches!(e, MessageEvent::TextDelta { .. }))
    );
    let incomplete =
        tokio::time::timeout(Duration::from_secs(12), server.delete_session(id.clone()))
            .await
            .expect("bounded close classification");
    assert!(matches!(incomplete, Err(AcpError::Cleanup(_))));
    let cancellation_signalled = initialization.cancel.is_cancelled();
    let sender_released = send.is_finished();
    release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(20), initialization.wait())
        .await
        .expect("initializer deadline")
        .expect("initializer returns");
    // Ensure the first absolute-deadline operation published its failure before
    // starting the retry (the caller's timer can fire just before that task).
    tokio::task::yield_now().await;
    server
        .delete_session(id.clone())
        .await
        .expect("cleanup retry");
    let send_result = tokio::time::timeout(Duration::from_secs(2), send)
        .await
        .expect("send settles")
        .expect("send task");
    assert!(
        cancellation_signalled,
        "close must signal the owned initializer"
    );
    assert!(
        sender_released && send_result.is_err(),
        "held sender must release admission before bootstrap resumes"
    );
    assert!(!engine.sessions.lock().await.contains_key(&id));
    assert!(!engine.initializers.lock().await.contains_key(&id));
}

struct OutboundChannel {
    sends: Arc<AtomicUsize>,
    starts: Arc<AtomicUsize>,
}

#[async_trait]
impl wcore_channels::Channel for OutboundChannel {
    fn name(&self) -> &str {
        "slack"
    }
    fn platform(&self) -> &str {
        "slack"
    }
    fn config_schema(&self) -> &str {
        r#"{"type":"object","properties":{}}"#
    }
    async fn start(&mut self) -> Result<(), wcore_channels::ChannelError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), wcore_channels::ChannelError> {
        Ok(())
    }
    async fn poll_events(
        &mut self,
    ) -> Result<Vec<wcore_channels::ChannelEvent>, wcore_channels::ChannelError> {
        Ok(Vec::new())
    }
    async fn send_message(
        &mut self,
        msg: wcore_channels::OutgoingMessage,
    ) -> Result<wcore_channels::MessageReceipt, wcore_channels::ChannelError> {
        self.sends.fetch_add(1, Ordering::SeqCst);
        Ok(wcore_channels::MessageReceipt {
            id: "fixture".into(),
            conversation_id: msg.conversation_id,
            ts_secs: 0,
        })
    }
}

#[tokio::test]
async fn outbound_channel_transport_survives_while_acp_avoids_inbound_ownership() {
    let workspace = tempfile::tempdir().expect("workspace");
    let engine = engine(workspace.path(), true);
    let server = AcpServer::new().with_turn_engine(engine.clone());
    let first = create(&server).await;
    let second = create(&server).await;
    let sends = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(AtomicUsize::new(0));
    for id in [&first, &second] {
        let session = engine
            .session_for(id, None, &[], &[])
            .await
            .expect("bootstrap");
        assert!(
            !session.lifetime.inbound_started,
            "ACP must not start inbound workers/webhooks"
        );
        session
            .lifetime
            .channels
            .as_ref()
            .expect("outbound manager")
            .write()
            .await
            .register(Box::new(OutboundChannel {
                sends: sends.clone(),
                starts: starts.clone(),
            }))
            .await;
    }
    for id in [&first, &second] {
        let frames = server
            .send_message(MessageSendRequest {
                session_id: id.clone(),
                text: "Send the configured fixture message.".into(),
                tools: Vec::new(),
            })
            .await
            .expect("send turn")
            .collect::<Vec<_>>()
            .await;
        assert!(
            frames
                .iter()
                .any(|e| matches!(e, MessageEvent::ToolResult { result } if !result.is_error)),
            "{frames:?}"
        );
        if id == &first {
            server
                .delete_session(first.clone())
                .await
                .expect("close first");
        }
    }
    assert_eq!(
        sends.load(Ordering::SeqCst),
        2,
        "configured outbound transport must deliver for both sessions"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    server.delete_session(second).await.expect("close second");
}

#[tokio::test]
async fn failed_initializer_retires_join_handle_before_cleanup_retry() {
    let workspace = tempfile::tempdir().expect("workspace");
    let engine = engine(workspace.path(), false);
    let server = AcpServer::new().with_turn_engine(engine.clone());
    let id = create(&server).await;
    let initializer = Arc::new(Initialization::new());
    let signal = initializer.clone();
    let handle = tokio::spawn(async move {
        let _completion = InitializationCompletion(signal);
        panic!("fixture initializer failure");
    });
    *initializer.task.lock().await = Some(handle);
    engine
        .initializers
        .lock()
        .await
        .insert(id.clone(), initializer.clone());
    assert!(
        server.delete_session(id.clone()).await.is_err(),
        "unknown failed bootstrap cleanup cannot claim success"
    );
    assert!(
        initializer.task.lock().await.is_none(),
        "a completed failing JoinHandle must be retired before retry"
    );
    let retry = tokio::time::timeout(Duration::from_secs(2), server.delete_session(id))
        .await
        .expect("retry must complete")
        .expect_err("bootstrap cleanup remains unproven");
    assert!(
        !retry.to_string().contains("did not publish completion"),
        "retry task panicked: {retry}"
    );
}
