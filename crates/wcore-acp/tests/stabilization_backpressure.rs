//! W05: recording continues independently of a reader, under byte bounds.
use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use std::{pin::Pin, sync::Arc, time::Duration};
use wcore_acp::{
    AcpError,
    cursor::Cursor,
    protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest},
    server::AcpServer,
    transport::http::HttpHandler,
    turn::{TurnEngine, TurnRequest},
};
struct Burst;
#[async_trait]
impl TurnEngine for Burst {
    async fn run_turn(
        &self,
        _: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        let events = (0..64)
            .map(|_| MessageEvent::TextDelta {
                text: "x".repeat(256 * 1024),
            })
            .chain(std::iter::once(MessageEvent::Done {
                stop_reason: "end_turn".into(),
                turn_id: "w05".into(),
            }));
        Ok(Box::pin(stream::iter(events)))
    }
}
#[tokio::test]
async fn slow_reader_has_explicit_overload_while_replay_retains_bounded_tail() {
    let server = AcpServer::new().with_turn_engine(Arc::new(Burst));
    let id = server
        .create_session(SessionCreateRequest {
            model: None,
            tools: vec![],
            system_prompt: None,
            agent: None,
            mcp_servers: vec![],
        })
        .await
        .unwrap()
        .session_id;
    let initial = server.event_tip(&id).await.unwrap();
    let response = server
        .send_message(MessageSendRequest {
            session_id: id.clone(),
            text: "burst".into(),
            tools: vec![],
        })
        .await
        .unwrap();
    // Deliberately hold the receiver while the recorder drains the real upstream.
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.event_tip(&id).await.unwrap().position < 65 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("upstream recording cannot stall on a slow reader");
    assert!(
        server.events_since(&id, &initial).await.is_err(),
        "16MiB history must evict old events under8MiB replay cap"
    );
    let live: Vec<_> = response.collect().await;
    assert!(
        live.iter()
            .any(|ev| matches!(ev, MessageEvent::Error { .. })),
        "live overflow must be explicit"
    );
    let tip = server.event_tip(&id).await.unwrap();
    let tail = server
        .events_since(
            &id,
            &Cursor {
                stream_id: tip.stream_id,
                position: 64,
            },
        )
        .await
        .unwrap();
    assert_eq!(tail.events.len(), 1);
    assert!(matches!(tail.events[0].event, MessageEvent::Done { .. }));
    server.delete_session(id).await.unwrap();
}
