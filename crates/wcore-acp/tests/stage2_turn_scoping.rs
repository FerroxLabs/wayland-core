//! wayland#1356 review BLOCKER: a turn's live stream must carry only its own
//! turn's events, even when two turns on one session overlap and interleave
//! in the session's shared event log.
use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use wcore_acp::{
    AcpError,
    protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest},
    server::AcpServer,
    transport::http::HttpHandler,
    turn::{TurnEngine, TurnRequest},
};

type FedTurn = (mpsc::UnboundedReceiver<MessageEvent>, oneshot::Sender<()>);

/// Streams each turn from a channel the test feeds, keyed by the prompt text,
/// and says when that turn's upstream is first polled (its recorder started).
struct Fed {
    turns: Mutex<HashMap<String, FedTurn>>,
}

#[async_trait]
impl TurnEngine for Fed {
    async fn close_session(&self, _: &str) -> Result<(), AcpError> {
        Ok(())
    }
    async fn run_turn(
        &self,
        req: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        let (rx, polled) = self
            .turns
            .lock()
            .unwrap()
            .remove(&req.text)
            .expect("a fed turn for this prompt");
        Ok(
            stream::unfold((Some(polled), rx), |(mut polled, mut rx)| async move {
                if let Some(polled) = polled.take() {
                    let _ = polled.send(());
                }
                rx.recv().await.map(|event| (event, (None, rx)))
            })
            .boxed(),
        )
    }
}

fn prompt(session_id: &str, text: &str) -> MessageSendRequest {
    MessageSendRequest {
        session_id: session_id.to_string(),
        text: text.to_string(),
        tools: vec![],
    }
}

fn delta(text: &str) -> MessageEvent {
    MessageEvent::TextDelta {
        text: text.to_string(),
    }
}

fn done() -> MessageEvent {
    MessageEvent::Done {
        stop_reason: "end_turn".into(),
        turn_id: String::new(),
    }
}

fn texts(frames: &[MessageEvent]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|frame| match frame {
            MessageEvent::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn done_turn(frames: &[MessageEvent]) -> String {
    match frames.last() {
        Some(MessageEvent::Done { turn_id, .. }) => turn_id.clone(),
        other => panic!("the stream must end with its own Done, got {other:?} in {frames:?}"),
    }
}

/// Review BLOCKER. Turn A is mid-reply when turn B starts, so B's recorder
/// begins inside A's events; A then finishes before B records anything. Each
/// live stream must still deliver only its own text and end at its own Done.
#[tokio::test]
async fn overlapping_turns_on_one_session_each_deliver_only_their_own_events() {
    let wait = Duration::from_secs(10);
    let (a_feed, a_rx) = mpsc::unbounded_channel();
    let (b_feed, b_rx) = mpsc::unbounded_channel();
    let (a_polled, a_started) = oneshot::channel();
    let (b_polled, b_started) = oneshot::channel();
    let turns = HashMap::from([
        ("turn A".to_string(), (a_rx, a_polled)),
        ("turn B".to_string(), (b_rx, b_polled)),
    ]);
    let server = AcpServer::new()
        .with_isolated_delivery_budget()
        .with_turn_engine(Arc::new(Fed {
            turns: Mutex::new(turns),
        }));
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

    let mut a = server.send_message(prompt(&id, "turn A")).await.unwrap();
    tokio::time::timeout(wait, a_started)
        .await
        .unwrap()
        .unwrap();
    a_feed.send(delta("A-1")).unwrap();
    let first = tokio::time::timeout(wait, a.next())
        .await
        .expect("A's first frame")
        .expect("A's stream open");

    // B starts while A is still replying: B's recorder begins inside A's events.
    let b = server.send_message(prompt(&id, "turn B")).await.unwrap();
    tokio::time::timeout(wait, b_started)
        .await
        .unwrap()
        .unwrap();

    // A finishes, and its events are in the log, before B records anything.
    a_feed.send(delta("A-2")).unwrap();
    a_feed.send(done()).unwrap();
    tokio::time::timeout(wait, async {
        while server.event_tip(&id).await.unwrap().position < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("A's reply is recorded");
    b_feed.send(delta("B-1")).unwrap();
    b_feed.send(done()).unwrap();

    let mut a_frames = vec![first];
    a_frames.extend(
        tokio::time::timeout(wait, a.collect::<Vec<_>>())
            .await
            .expect("A's stream ends"),
    );
    let b_frames = tokio::time::timeout(wait, b.collect::<Vec<_>>())
        .await
        .expect("B's stream ends");

    assert_eq!(texts(&a_frames), ["A-1", "A-2"], "A's stream: {a_frames:?}");
    assert_eq!(texts(&b_frames), ["B-1"], "B's stream: {b_frames:?}");
    let (a_turn, b_turn) = (done_turn(&a_frames), done_turn(&b_frames));
    assert!(!a_turn.is_empty() && !b_turn.is_empty());
    assert_ne!(a_turn, b_turn, "each stream ends at its own turn's Done");
    drop((a_feed, b_feed));
    server.delete_session(id).await.unwrap();
}
