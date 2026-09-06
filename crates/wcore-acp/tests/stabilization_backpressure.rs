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
    async fn close_session(&self, _: &str) -> Result<(), AcpError> {
        Ok(())
    }

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
    let overflow_id = live
        .iter()
        .find_map(|event| match event {
            MessageEvent::Error { turn_id, .. } => Some(turn_id),
            _ => None,
        })
        .unwrap();
    assert!(!overflow_id.is_empty());
    assert!(matches!(&tail.events[0].event,MessageEvent::Done{turn_id,..} if turn_id==overflow_id));
    server.delete_session(id).await.unwrap();
}

#[tokio::test]
async fn byte_overload_keeps_one_terminal_and_releases_owned_capacity() {
    let terminal = MessageEvent::Error {
        error: wcore_acp::protocol::JsonRpcError {
            code: -32003,
            message: "overload".into(),
            data: None,
        },
        turn_id: "w05".into(),
    };
    let (tx, mut rx) = wcore_acp::bounded::channel(terminal.clone()).unwrap();
    let mut accepted = 0;
    while tx
        .send(MessageEvent::TextDelta {
            text: "x".repeat(32768),
        })
        .is_ok()
    {
        accepted += 1;
    }
    assert!(
        accepted > 0 && accepted < 256,
        "byte limit must precede count limit for large events"
    );
    assert!(matches!(rx.recv().await, Some(MessageEvent::Error { .. })));
    assert!(rx.recv().await.is_none());
    drop(rx);
    drop(tx);
    let (tx, mut rx) = wcore_acp::bounded::channel(terminal).unwrap();
    tx.send(MessageEvent::TextDelta {
        text: "positive-control".into(),
    })
    .unwrap();
    assert!(matches!(
        rx.recv().await,
        Some(MessageEvent::TextDelta { .. })
    ));
}
