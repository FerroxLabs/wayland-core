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
