//! wayland#1356: a reader that is keeping up by the delivery budget, and whose
//! whole turn is still inside the session log's replay window, must not be
//! detached because the recorder wrote a burst of small events ahead of it.
use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use std::{pin::Pin, sync::Arc, time::Duration};
use wcore_acp::{
    AcpError,
    protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest},
    server::AcpServer,
    transport::http::HttpHandler,
    turn::{TurnEngine, TurnRequest},
};

const LARGE: usize = 512 * 1024;
const SMALL: usize = 4 * 1024;

/// `large` text events of 512 KiB, then `small` of 4 KiB, then Done: the
/// shape of the #1352 churn arm's provider.
struct LargeThenSmall {
    large: usize,
    small: usize,
}

#[async_trait]
impl TurnEngine for LargeThenSmall {
    async fn close_session(&self, _: &str) -> Result<(), AcpError> {
        Ok(())
    }
    async fn run_turn(
        &self,
        _: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        let large = (0..self.large).map(|_| MessageEvent::TextDelta {
            text: "x".repeat(LARGE),
        });
        let small = (0..self.small).map(|_| MessageEvent::TextDelta {
            text: "x".repeat(SMALL),
        });
        let done = std::iter::once(MessageEvent::Done {
            stop_reason: "end_turn".into(),
            turn_id: "w1356".into(),
        });
        Ok(Box::pin(stream::iter(large.chain(small).chain(done))))
    }
}

fn create() -> SessionCreateRequest {
    SessionCreateRequest {
        model: None,
        tools: vec![],
        system_prompt: None,
        agent: None,
        mcp_servers: vec![],
    }
}

/// wayland#1356 c1. Measured by the w15/stage2-1356-diag instrument: with a
/// reader that takes one virtual millisecond per frame, the recorder's
/// position handoff refused its 259th position against LIVE_EVENTS 256, and
/// delivery detached after 3 frames with 999 ms of its one-second budget
/// unspent. Here the whole 521-event, ~6.1 MiB turn stays inside one log's
/// replay window (1,024 events, 8 MiB) and the reader's total wait stays far
/// inside its budget, so the only thing that can detach it is that count of
/// pending positions. Time is paused, so the result does not depend on load.
#[tokio::test(start_paused = true)]
async fn a_reader_inside_its_budget_and_replay_window_is_not_detached_by_small_events() {
    let server = AcpServer::new()
        .with_isolated_delivery_budget()
        .with_turn_engine(Arc::new(LargeThenSmall {
            large: 8,
            small: 512,
        }));
    let id = server.create_session(create()).await.unwrap().session_id;
    let genesis = server.event_tip(&id).await.unwrap();
    let mut response = server
        .send_message(MessageSendRequest {
            session_id: id.clone(),
            text: "go".into(),
            tools: vec![],
        })
        .await
        .unwrap();
    let mut frames = Vec::new();
    while let Some(frame) = tokio::time::timeout(Duration::from_secs(60), response.next())
        .await
        .expect("the stream keeps making progress")
    {
        frames.push(frame);
        // One virtual millisecond per frame: 512 MiB/s on the large frames.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    let replay = server
        .events_since(&id, &genesis)
        .await
        .expect("the whole turn is still inside the replay window");
    assert_eq!(replay.events.len(), 521, "no replay gap can have formed");
    let error = frames.iter().find_map(|frame| match frame {
        MessageEvent::Error { error, .. } => Some(error.message.clone()),
        _ => None,
    });
    assert!(
        error.is_none(),
        "a reader inside its budget and the replay window was detached after {} frames: {error:?}",
        frames.len()
    );
    let text: usize = frames
        .iter()
        .map(|frame| match frame {
            MessageEvent::TextDelta { text } => text.len(),
            _ => 0,
        })
        .sum();
    assert_eq!(text, 8 * LARGE + 512 * SMALL);
    assert!(matches!(frames.last(), Some(MessageEvent::Done { .. })));
    server.delete_session(id).await.unwrap();
}

/// wayland#1356 guard, and the evidence against treating this detach as free
/// (criterion c3). A 2,065-event, 16 MiB turn read at one virtual millisecond
/// per frame falls out of one log's replay window (1,024 events, 8 MiB),
/// because recording never waits for a reader. That reader IS still detached,
/// and a host cannot recover what it missed by resuming: the events after its
/// last frame are already evicted.
#[tokio::test(start_paused = true)]
async fn a_reader_that_falls_out_of_the_replay_window_is_detached_and_cannot_resume() {
    use wcore_acp::cursor::{Cursor, CursorError, ResumeError};
    let server = AcpServer::new().with_turn_engine(Arc::new(LargeThenSmall {
        large: 16,
        small: 2048,
    }));
    let id = server.create_session(create()).await.unwrap().session_id;
    let genesis = server.event_tip(&id).await.unwrap();
    let mut response = server
        .send_message(MessageSendRequest {
            session_id: id.clone(),
            text: "go".into(),
            tools: vec![],
        })
        .await
        .unwrap();
    let mut frames = Vec::new();
    while let Some(frame) = tokio::time::timeout(Duration::from_secs(60), response.next())
        .await
        .expect("the stream keeps making progress")
    {
        frames.push(frame);
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    assert!(
        matches!(frames.last(), Some(MessageEvent::Error { .. })),
        "a reader outside the replay window is still detached: {} frames",
        frames.len()
    );
    // Every frame before the terminal is one logged event, in order.
    let last_delivered = genesis.position + (frames.len() as u64 - 1);
    let resume = server
        .events_since(
            &id,
            &Cursor {
                stream_id: genesis.stream_id.clone(),
                position: last_delivered,
            },
        )
        .await;
    assert!(
        matches!(resume, Err(ResumeError::Cursor(CursorError::TooOld { .. }))),
        "resuming after the last delivered frame must be refused as evicted: {}",
        match &resume {
            Ok(served) => format!("served {} events", served.events.len()),
            Err(error) => error.to_string(),
        }
    );
    server.delete_session(id).await.unwrap();
}
