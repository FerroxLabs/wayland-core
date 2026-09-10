//! wayland#1352 c2 review: retained-history accounting when a session close
//! races eviction, and a running turn's independence from the shared log map.
use super::*;
use crate::turn::{TurnEngine, TurnRequest};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

/// A pause point inside eviction, after it has chosen a victim and before it
/// pops that victim. Keyed by the address of one server's retained-byte total,
/// so a gate installed for one test can never pause another test's server.
pub(super) struct EvictionGate {
    reached: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

fn gates() -> &'static std::sync::Mutex<Vec<(usize, EvictionGate)>> {
    static GATES: std::sync::OnceLock<std::sync::Mutex<Vec<(usize, EvictionGate)>>> =
        std::sync::OnceLock::new();
    GATES.get_or_init(Default::default)
}

/// Eviction calls this between choosing its victim and popping it. A no-op
/// unless a test installed a gate for this server.
pub(super) async fn pause_at_eviction_gate(total: &AtomicUsize) {
    let key = total as *const AtomicUsize as usize;
    let gate = {
        let mut gates = gates().lock().unwrap_or_else(|e| e.into_inner());
        gates
            .iter()
            .position(|(installed, _)| *installed == key)
            .map(|index| gates.swap_remove(index).1)
    };
    if let Some(gate) = gate {
        let _ = gate.reached.send(());
        let _ = gate.release.await;
    }
}

impl AcpServer {
    fn retained_total_key(&self) -> usize {
        &self.retained.total as *const AtomicUsize as usize
    }

    /// Pause this server's next eviction once it has chosen a victim. The first
    /// channel says it got there; sending on the second lets it pop.
    fn gate_next_eviction(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached, reached_rx) = oneshot::channel();
        let (release_tx, release) = oneshot::channel();
        gates()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((self.retained_total_key(), EvictionGate { reached, release }));
        (reached_rx, release_tx)
    }

    /// The cross-session counter, and the true sum of every log the map holds.
    /// Only meaningful at quiescence.
    async fn retained_accounting(&self) -> (usize, usize) {
        let logs: Vec<SharedLog> = self.events.read().await.values().cloned().collect();
        let sum = logs.iter().map(|log| lock_log(log).retained_bytes()).sum();
        (self.retained.total.load(Ordering::Acquire), sum)
    }
}

/// Replays a fixed script as one turn.
struct Script(Vec<MessageEvent>);

#[async_trait]
impl TurnEngine for Script {
    async fn close_session(&self, _: &str) -> Result<(), AcpError> {
        Ok(())
    }
    async fn run_turn(
        &self,
        _: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        Ok(stream::iter(self.0.clone()).boxed())
    }
}

/// Streams whatever the test feeds it, so a test decides when events arrive.
struct Fed(std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<MessageEvent>>>);

#[async_trait]
impl TurnEngine for Fed {
    async fn close_session(&self, _: &str) -> Result<(), AcpError> {
        Ok(())
    }
    async fn run_turn(
        &self,
        _: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        let rx = self
            .0
            .lock()
            .unwrap()
            .take()
            .expect("the fed engine serves one turn");
        Ok(stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
        )
        .boxed())
    }
}

fn chunks(count: usize, bytes: usize) -> Vec<MessageEvent> {
    (0..count)
        .map(|_| MessageEvent::TextDelta {
            text: "x".repeat(bytes),
        })
        .collect()
}

fn turn(mut events: Vec<MessageEvent>) -> Vec<MessageEvent> {
    events.push(MessageEvent::Done {
        stop_reason: "end_turn".to_string(),
        turn_id: String::new(),
    });
    events
}

fn create_request() -> SessionCreateRequest {
    SessionCreateRequest {
        model: None,
        tools: Vec::new(),
        system_prompt: None,
        agent: None,
        mcp_servers: Vec::new(),
    }
}

fn prompt(session_id: &str) -> MessageSendRequest {
    MessageSendRequest {
        session_id: session_id.to_string(),
        text: "go".to_string(),
        tools: Vec::new(),
    }
}

/// Run one complete turn of `events` in a new session sharing `server`'s state.
async fn recorded_session(server: &AcpServer, events: Vec<MessageEvent>) -> String {
    let server = server.clone().with_turn_engine(Arc::new(Script(events)));
    let id = server
        .create_session(create_request())
        .await
        .unwrap()
        .session_id;
    let frames: Vec<_> = server
        .send_message(prompt(&id))
        .await
        .unwrap()
        .collect()
        .await;
    assert!(
        matches!(frames.last(), Some(MessageEvent::Done { .. })),
        "{:?}",
        frames.last()
    );
    id
}

async fn retained_bytes_of(server: &AcpServer, id: &str) -> usize {
    let log = server.events.read().await.get(id).cloned().unwrap();
    lock_log(&log).retained_bytes()
}

const BIG: usize = 256 * 1024;

/// Review BLOCKER 1, forced. A recorder over the cap chooses the largest log
/// as its victim; that log's session closes and gives back its bytes; only
/// then does the evictor pop. The total must still equal the logs it counts,
/// and must be zero once every session is gone -- not low, and never wrapped.
#[tokio::test]
async fn a_close_between_victim_choice_and_pop_keeps_retained_total_exact() {
    let server = AcpServer::new();
    // The victim: the largest log, whole, under the 8 MiB per-log cap.
    let victim = recorded_session(&server, turn(chunks(31, BIG))).await;
    let mut others = Vec::new();
    for _ in 0..7 {
        others.push(recorded_session(&server, turn(chunks(29, BIG))).await);
    }
    let (total, sum) = server.retained_accounting().await;
    assert_eq!(total, sum);
    assert!(
        total < RETAINED_HISTORY_BYTES,
        "setup must stay under the cap: {total}"
    );
    let victim_bytes = retained_bytes_of(&server, &victim).await;
    for id in &others {
        assert!(retained_bytes_of(&server, id).await < victim_bytes);
    }

    let (reached, release) = server.gate_next_eviction();
    let trigger_server = server
        .clone()
        .with_turn_engine(Arc::new(Script(turn(chunks(31, BIG)))));
    let trigger = trigger_server
        .create_session(create_request())
        .await
        .unwrap()
        .session_id;
    let response = trigger_server.send_message(prompt(&trigger)).await.unwrap();
    // Keep reading, so the trigger turn is never detached for being unread.
    let reader = tokio::spawn(response.collect::<Vec<_>>());
    tokio::time::timeout(Duration::from_secs(60), reached)
        .await
        .expect("the trigger turn's recorder went over the cap and chose a victim")
        .unwrap();
    // The chosen victim's session closes before the evictor pops it.
    server.delete_session(victim.clone()).await.unwrap();
    release.send(()).unwrap();
    let frames = tokio::time::timeout(Duration::from_secs(60), reader)
        .await
        .expect("the trigger turn finishes")
        .unwrap();
    assert!(matches!(frames.last(), Some(MessageEvent::Done { .. })));

    let (total, sum) = server.retained_accounting().await;
    assert_eq!(
        total, sum,
        "retained_total drifted from the logs it counts after a close raced eviction"
    );
    for id in others.into_iter().chain([trigger]) {
        server.delete_session(id).await.unwrap();
    }
    assert_eq!(
        server.retained_accounting().await,
        (0, 0),
        "every session is gone, so nothing may remain counted"
    );
}

/// Review BLOCKER 1 in the traffic shape the review named: history held over
/// the cap by residents while churn sessions record one large burst then many
/// small deltas and close, on several threads. Whatever the interleaving, the
/// total must equal the logs it counts at quiescence, respect the cap there,
/// and be zero at the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn churn_over_the_cap_with_mixed_chunk_sizes_keeps_retained_total_exact() {
    let server = AcpServer::new();
    let mut residents = Vec::new();
    for _ in 0..8 {
        residents.push(recorded_session(&server, turn(chunks(28, BIG))).await);
    }
    let mut workers = Vec::new();
    for _ in 0..8 {
        let server = server.clone();
        workers.push(tokio::spawn(async move {
            for _ in 0..4 {
                let mut events = chunks(26, BIG);
                events.extend(chunks(256, 1024));
                let id = recorded_session(&server, turn(events)).await;
                server.delete_session(id).await.unwrap();
            }
        }));
    }
    for worker in workers {
        worker.await.unwrap();
    }
    let (total, sum) = server.retained_accounting().await;
    assert_eq!(total, sum, "drift after churn over the cap");
    assert!(
        total <= RETAINED_HISTORY_BYTES,
        "quiescent history over the cap: {total}"
    );
    for id in residents {
        server.delete_session(id).await.unwrap();
    }
    assert_eq!(server.retained_accounting().await, (0, 0));
}

/// Review MINOR 5. The c1 test holds the map for READ, which a return to
/// per-event map reads would not notice. Once a turn is running, neither its
/// recorder nor its delivery task may touch the map at all: holding it
/// EXCLUSIVELY, as another session's create or close does, must not stall a
/// running turn on a multi-threaded runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_exclusive_hold_on_the_log_map_does_not_stall_a_running_turn() {
    let (feed, rx) = tokio::sync::mpsc::unbounded_channel();
    let server = AcpServer::new().with_turn_engine(Arc::new(Fed(std::sync::Mutex::new(Some(rx)))));
    let id = server
        .create_session(create_request())
        .await
        .unwrap()
        .session_id;
    let mut response = server.send_message(prompt(&id)).await.unwrap();
    feed.send(MessageEvent::TextDelta {
        text: "first".to_string(),
    })
    .unwrap();
    // The first delivered frame proves both tasks have found their log.
    let first = tokio::time::timeout(Duration::from_secs(10), response.next())
        .await
        .expect("first frame")
        .expect("stream open");
    assert!(matches!(first, MessageEvent::TextDelta { .. }));

    let exclusive = server.events.write().await;
    for event in turn(chunks(64, 32 * 1024)) {
        feed.send(event).unwrap();
    }
    let rest = tokio::time::timeout(Duration::from_secs(10), response.collect::<Vec<_>>()).await;
    drop(exclusive);

    let rest = rest.expect("a running turn stalled while the log map was held exclusively");
    let text: usize = rest
        .iter()
        .map(|frame| match frame {
            MessageEvent::TextDelta { text } => text.len(),
            _ => 0,
        })
        .sum();
    assert_eq!(text, 64 * 32 * 1024);
    assert!(matches!(rest.last(), Some(MessageEvent::Done { .. })));
    assert_eq!(server.event_tip(&id).await.unwrap().position, 66);
    // End the fed upstream so recording finishes; close awaits the recorder.
    drop(feed);
    server.delete_session(id).await.unwrap();
}
