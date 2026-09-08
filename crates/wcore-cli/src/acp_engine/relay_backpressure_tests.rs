//! W05: actual relay text delivery waits within the existing turn budget.
use super::*;
use std::time::Duration;

fn fixture() -> (RelaySink, RelayTarget, UnboundedReceiver<ProtocolEvent>) {
    let (sender, receiver) = protocol_channel("bounded-text-turn").unwrap();
    let cancel = CancellationToken::new();
    let overflow_cancel = cancel.clone();
    sender.set_on_overflow(move || overflow_cancel.cancel());
    let target = RelayTarget::new(sender, cancel);
    let sink = RelaySink::new(Arc::new(Mutex::new(Some(target.clone()))));
    (sink, target, receiver)
}

fn fill(target: &RelayTarget, text: &str) {
    target
        .sender
        .send(ProtocolEvent::TextDelta {
            text: text.into(),
            msg_id: "bounded-text-turn".into(),
        })
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn stabilization_async_text_waits_without_losing_frames() {
    let (sink, target, mut rx) = fixture();
    let text = "x".repeat(wcore_acp::bounded::LIVE_BYTES / 2);
    fill(&target, &text);
    let pending = sink.emit_text_delta_async(&text, "bounded-text-turn");
    tokio::pin!(pending);
    assert!(futures::poll!(&mut pending).is_pending());
    assert!(!target.sender.overloaded());
    tokio::time::advance(Duration::from_millis(200)).await;
    assert!(
        matches!(rx.recv().await, Some(ProtocolEvent::TextDelta { text: body, .. }) if body == text)
    );
    pending.await;
    assert!(
        matches!(rx.recv().await, Some(ProtocolEvent::TextDelta { text: body, msg_id })
        if body == text && msg_id == "bounded-text-turn")
    );
    assert!(!target.sender.overloaded());
    assert!(!target.cancel.is_cancelled());
    assert_eq!(*target.wait_budget.lock().await, Duration::from_millis(800));
}

#[tokio::test(start_paused = true)]
async fn stabilization_async_text_uses_one_cumulative_turn_wait_budget() {
    let (sink, target, mut rx) = fixture();
    let text = "x".repeat(wcore_acp::bounded::LIVE_BYTES / 2);
    fill(&target, &text);
    {
        let pending = sink.emit_text_delta_async(&text, "bounded-text-turn");
        tokio::pin!(pending);
        assert!(futures::poll!(&mut pending).is_pending());
        tokio::time::advance(Duration::from_millis(600)).await;
        assert!(matches!(
            rx.recv().await,
            Some(ProtocolEvent::TextDelta { .. })
        ));
        pending.await;
    }
    assert_eq!(*target.wait_budget.lock().await, Duration::from_millis(400));
    let pending = sink.emit_text_delta_async(&text, "bounded-text-turn");
    tokio::pin!(pending);
    assert!(futures::poll!(&mut pending).is_pending());
    tokio::time::advance(Duration::from_millis(401)).await;
    pending.await;
    assert!(target.sender.overloaded());
    assert!(target.cancel.is_cancelled());
    assert!(target.wait_budget.lock().await.is_zero());
    assert!(
        matches!(rx.recv().await, Some(ProtocolEvent::Error { msg_id: Some(id), error })
        if id == "bounded-text-turn" && error.code == "resource_limit")
    );
    assert!(rx.recv().await.is_none(), "exactly one correlated terminal");
}

#[tokio::test(start_paused = true)]
async fn stabilization_async_text_close_cancels_a_pending_capacity_wait() {
    let (sink, target, mut rx) = fixture();
    let text = "x".repeat(wcore_acp::bounded::LIVE_BYTES / 2);
    fill(&target, &text);
    let pending = sink.emit_text_delta_async(&text, "bounded-text-turn");
    tokio::pin!(pending);
    assert!(futures::poll!(&mut pending).is_pending());
    target.cancel.cancel();
    assert!(futures::poll!(&mut pending).is_ready());
    assert_eq!(*target.wait_budget.lock().await, Duration::from_secs(1));
    assert!(
        matches!(rx.recv().await, Some(ProtocolEvent::Error { msg_id: Some(id), .. })
        if id == "bounded-text-turn")
    );
    assert!(rx.recv().await.is_none());
}
