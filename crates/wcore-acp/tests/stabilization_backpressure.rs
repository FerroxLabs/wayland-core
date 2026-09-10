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
struct Burst {
    chunks: usize,
    chunk_bytes: usize,
}
#[async_trait]
impl TurnEngine for Burst {
    async fn close_session(&self, _: &str) -> Result<(), AcpError> {
        Ok(())
    }

    async fn run_turn(
        &self,
        _: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        let bytes = self.chunk_bytes;
        let events = (0..self.chunks)
            .map(move |_| MessageEvent::TextDelta {
                text: "x".repeat(bytes),
            })
            .chain(std::iter::once(MessageEvent::Done {
                stop_reason: "end_turn".into(),
                turn_id: "w05".into(),
            }));
        Ok(Box::pin(stream::iter(events)))
    }
}
/// Covers the REPLAY-GAP detach only: this 16 MiB turn overruns the 8 MiB
/// per-log cap, so delivery finds its next event already evicted and detaches.
/// It never reaches the delivery wait budget and would pass without one; the
/// budget is covered by
/// `a_reader_that_stops_reading_is_detached_by_its_wait_budget_beside_a_fast_one`.
#[tokio::test]
async fn slow_reader_has_explicit_overload_while_replay_retains_bounded_tail() {
    let server = AcpServer::new()
        .with_isolated_delivery_budget()
        .with_turn_engine(Arc::new(Burst {
            chunks: 64,
            chunk_bytes: 256 * 1024,
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

#[tokio::test]
async fn fast_rest_reader_receives_complete_sixteen_mib_burst() {
    use wcore_acp::transport::rest::RestTransport;
    let server = AcpServer::new()
        .with_isolated_delivery_budget()
        .with_turn_engine(Arc::new(Burst {
            chunks: 512,
            chunk_bytes: 32 * 1024,
        }));
    let app = RestTransport::new(Arc::new(server.clone())).router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let http = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    #[allow(clippy::disallowed_methods)] // Numeric loopback test; no external provider.
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let created: serde_json::Value = client
        .post(format!("{base}/v1/sessions"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["session_id"].as_str().unwrap();
    let response = client
        .post(format!("{base}/v1/sessions/{id}/prompt"))
        .json(&serde_json::json!({"text":"large fast burst"}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    let mut bytes = 0;
    let mut terminals = 0;
    for line in body.lines().filter_map(|line| line.strip_prefix("data: ")) {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_ne!(event["kind"], "error", "{event}");
        if event["kind"] == "text_delta" {
            bytes += event["text"].as_str().unwrap().len();
        }
        if event["kind"] == "done" {
            terminals += 1;
        }
    }
    assert_eq!(bytes, 16 * 1024 * 1024);
    assert_eq!(terminals, 1);
    assert_eq!(
        client
            .delete(format!("{base}/v1/sessions/{id}"))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let _ = stop.send(());
    http.await.unwrap();
}

#[tokio::test]
async fn aggregate_replay_pressure_preserves_a_late_stream_first_event() {
    let server = AcpServer::new()
        .with_isolated_delivery_budget()
        .with_turn_engine(Arc::new(Burst {
            chunks: 256,
            chunk_bytes: 32768,
        }));
    let mut sessions = Vec::new();
    // Nine independent near-8MiB histories exceed the 64MiB aggregate cap.
    // Keep their sessions resident but detach delivery while recording finishes.
    for _ in 0..9 {
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
        let response = server
            .send_message(MessageSendRequest {
                session_id: id.clone(),
                text: "fill replay".into(),
                tools: vec![],
            })
            .await
            .unwrap();
        drop(response);
        tokio::time::timeout(Duration::from_secs(10), async {
            while server.event_tip(&id).await.unwrap().position < 257 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached recording completed");
        sessions.push(id);
    }
    // Share the same session/log ownership, changing only this fixture's output.
    let late_server = server.clone().with_turn_engine(Arc::new(Burst {
        chunks: 1,
        chunk_bytes: 512 * 1024,
    }));
    let id = late_server
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
    let initial = late_server.event_tip(&id).await.unwrap();
    let response = late_server
        .send_message(MessageSendRequest {
            session_id: id.clone(),
            text: "late first event".into(),
            tools: vec![],
        })
        .await
        .unwrap();
    let frames = tokio::time::timeout(Duration::from_secs(5), response.collect::<Vec<_>>())
        .await
        .expect("late delivery finishes");
    let replay = late_server.events_since(&id, &initial).await;
    sessions.push(id);
    for id in sessions {
        late_server.delete_session(id).await.unwrap();
    }
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert!(matches!(&frames[0], MessageEvent::TextDelta { text } if text.len() == 512 * 1024));
    assert!(matches!(&frames[1], MessageEvent::Done { turn_id, .. } if !turn_id.is_empty()));
    assert_eq!(
        replay
            .expect("new stream retains its first event")
            .events
            .len(),
        2
    );
}

/// wayland#1352 c2 guard: the repair must not become "never cancel". While a
/// keeping-up reader in one session drains a full 16 MiB turn without any
/// error, a reader in ANOTHER session that stops reading is still detached by
/// its one-second delivery wait budget.
///
/// Reworked after review: the first version stalled a 16 MiB turn, which
/// overran the 8 MiB per-log cap, so the reader was detached by a replay gap
/// and the test passed with the budget removed. Here the stalled turn is
/// 4 MiB, its whole history is still retained when it is detached (so no
/// replay gap exists), and time is paused, so only the budget can detach it.
#[tokio::test(start_paused = true)]
async fn a_reader_that_stops_reading_is_detached_by_its_wait_budget_beside_a_fast_one() {
    let stalled_server = AcpServer::new()
        .with_isolated_delivery_budget()
        .with_turn_engine(Arc::new(Burst {
            chunks: 16,
            chunk_bytes: 256 * 1024,
        }));
    // Same session and log ownership, different fixture output.
    let fast_server = stalled_server.clone().with_turn_engine(Arc::new(Burst {
        chunks: 512,
        chunk_bytes: 32 * 1024,
    }));
    let create = || SessionCreateRequest {
        model: None,
        tools: vec![],
        system_prompt: None,
        agent: None,
        mcp_servers: vec![],
    };
    let stalled = stalled_server
        .create_session(create())
        .await
        .unwrap()
        .session_id;
    let fast = fast_server
        .create_session(create())
        .await
        .unwrap()
        .session_id;

    let stalled_genesis = stalled_server.event_tip(&stalled).await.unwrap();
    let stalled_response = stalled_server
        .send_message(MessageSendRequest {
            session_id: stalled.clone(),
            text: "stall".into(),
            tools: vec![],
        })
        .await
        .unwrap();
    let fast_frames = tokio::time::timeout(
        Duration::from_secs(20),
        fast_server
            .send_message(MessageSendRequest {
                session_id: fast.clone(),
                text: "keep up".into(),
                tools: vec![],
            })
            .await
            .unwrap()
            .collect::<Vec<_>>(),
    )
    .await
    .expect("the keeping-up reader finishes while the other reader is stalled");
    let fast_text: usize = fast_frames
        .iter()
        .map(|frame| match frame {
            MessageEvent::TextDelta { text } => text.len(),
            _ => 0,
        })
        .sum();
    assert!(
        !fast_frames
            .iter()
            .any(|frame| matches!(frame, MessageEvent::Error { .. })),
        "a keeping-up reader is never cancelled because another session's reader stalled"
    );
    assert_eq!(fast_text, 16 * 1024 * 1024);
    assert!(matches!(
        fast_frames.last(),
        Some(MessageEvent::Done { .. })
    ));

    tokio::time::timeout(Duration::from_secs(10), async {
        while stalled_server.event_tip(&stalled).await.unwrap().position < 17 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the stalled session's recording completes without its reader");
    // Still without reading, let well over one second of (paused) time pass.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let replay = stalled_server
        .events_since(&stalled, &stalled_genesis)
        .await
        .expect("the whole stalled turn is still retained, so no replay gap exists");
    assert_eq!(replay.events.len(), 17);

    // Only now does the stalled reader look at its stream.
    let stalled_frames: Vec<_> = stalled_response.collect().await;
    let stalled_text: usize = stalled_frames
        .iter()
        .map(|frame| match frame {
            MessageEvent::TextDelta { text } => text.len(),
            _ => 0,
        })
        .sum();
    assert!(
        stalled_frames
            .iter()
            .any(|frame| matches!(frame, MessageEvent::Error { .. })),
        "a reader that stopped reading is detached by its wait budget: {} frames",
        stalled_frames.len()
    );
    assert!(
        stalled_text < 16 * 256 * 1024,
        "the detach must drop undelivered frames, not deliver them late: {stalled_text}"
    );
    stalled_server.delete_session(stalled).await.unwrap();
    fast_server.delete_session(fast).await.unwrap();
}
