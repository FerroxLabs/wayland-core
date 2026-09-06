//! W05: the host metadata admission cap includes never-initialized sessions.
use wcore_acp::{protocol::SessionCreateRequest, server::AcpServer, transport::http::HttpHandler};
fn request() -> SessionCreateRequest {
    SessionCreateRequest {
        model: None,
        tools: vec![],
        system_prompt: None,
        agent: None,
        mcp_servers: vec![],
    }
}
#[tokio::test]
async fn metadata_admission_is_bounded_and_delete_releases_capacity() {
    let server = AcpServer::new();
    let mut sessions = Vec::new();
    for _ in 0..256 {
        sessions.push(server.create_session(request()).await.unwrap().session_id);
    }
    assert!(
        server.create_session(request()).await.is_err(),
        "257th metadata session must be refused"
    );
    server
        .delete_session(sessions.pop().unwrap())
        .await
        .unwrap();
    let replacement = server.create_session(request()).await.unwrap().session_id;
    server.delete_session(replacement).await.unwrap();
    for id in sessions {
        server.delete_session(id).await.unwrap();
    }
}

#[tokio::test]
async fn actual_cli_turn_uses_host_assigned_terminal_identity() {
    use futures::StreamExt;
    use std::sync::Arc;
    use wcore_acp::turn::{TurnEngine, TurnRequest};
    use wcore_types::{
        llm::{LlmEvent, LlmRequest},
        message::{FinishReason, StopReason, TokenUsage},
    };
    struct Provider;
    #[async_trait::async_trait]
    impl wcore_providers::LlmProvider for Provider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, wcore_providers::ProviderError> {
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            tx.try_send(LlmEvent::TextDelta("identity-control".into()))
                .unwrap();
            tx.try_send(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::Stop,
                usage: TokenUsage::default(),
            })
            .unwrap();
            Ok(rx)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut config = wcore_config::config::Config::default();
    config.session.enabled = false;
    config.memory.enabled = false;
    let engine = wcore_cli::acp_engine::EngineTurnEngine::with_provider(
        config,
        root.path().to_string_lossy().into_owned(),
        Arc::new(Provider),
    );
    let session = uuid::Uuid::new_v4().to_string();
    let frames: Vec<_> = engine
        .run_turn_with_id(
            TurnRequest {
                session_id: session.clone(),
                text: "answer".into(),
                tools: vec![],
                agent: None,
                mcp_servers: vec![],
            },
            "host-owned-turn".into(),
        )
        .await
        .unwrap()
        .collect()
        .await;
    assert!(frames.iter().any(|f|matches!(f,wcore_acp::protocol::MessageEvent::TextDelta{text} if text=="identity-control")));
    assert!(
        matches!(frames.last(),Some(wcore_acp::protocol::MessageEvent::Done{turn_id,..}) if turn_id=="host-owned-turn")
    );
    engine.close_session(&session).await.unwrap();
}
