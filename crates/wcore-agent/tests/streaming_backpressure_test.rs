//! Wave RA RELIABILITY MAJOR — backpressure for sub-agent streaming and
//! file-watcher events. Pre-fix, both used `mpsc::unbounded_channel`, so
//! a slow consumer let a fast producer drive memory growth without
//! bound. Post-fix:
//!
//! - `ChannelSink` uses `mpsc::channel(CHANNEL_CAPACITY=256)` with
//!   `try_send` semantics (drop on full).
//! - `FileWatcher` uses `mpsc::channel(EVENT_CHANNEL_CAPACITY=1024)` with
//!   `try_send` in the notify callback (drop on full).
//!
//! This test exercises ChannelSink because that's the streaming-tool
//! path (the "ToolOutputSink backpressure" described in the RA brief).
//! FileWatcher backpressure is verified by the bounded-channel switch
//! itself + the `notify`-callback `try_send` (covered structurally; an
//! e2e for the platform watcher callback is racy on a packaged test
//! runner).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use wcore_agent::agents::channel_sink::{CHANNEL_CAPACITY, ChannelSink, SubAgentRelay};
use wcore_tools::ToolOutputSink;

/// A slow consumer producing N chunks against a bounded channel must
/// (a) NOT block the producer's sync `emit_chunk` calls, and (b) the
/// channel buffer must not exceed `CHANNEL_CAPACITY` at any time —
/// `try_send` drops the excess on the floor instead.
#[tokio::test]
async fn channel_sink_drops_excess_when_consumer_is_slow() {
    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(CHANNEL_CAPACITY);
    let sink = Arc::new(ChannelSink::new("c-bp".into(), "agent-bp".into(), tx));

    // Slow consumer: 100ms between recvs. Pulls until the channel
    // closes (when the sink is dropped).
    let consumer = tokio::spawn(async move {
        let mut count = 0usize;
        while let Some(_relay) = rx.recv().await {
            count += 1;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        count
    });

    // Fast producer: 1000 emit_chunk calls in a tight loop. With the
    // pre-fix unbounded channel, all 1000 would buffer; the consumer
    // would take 100s to drain them. With the post-fix bounded channel,
    // most emissions hit the full-channel branch of `try_send` and get
    // dropped silently.
    let sink_clone = Arc::clone(&sink);
    let producer = tokio::spawn(async move {
        for i in 0..1000u32 {
            // Synchronous emit — must NOT block, even with the channel
            // full. The whole loop must complete in well under 100ms.
            <ChannelSink as ToolOutputSink>::emit_chunk(&sink_clone, &format!("chunk-{i}"));
        }
    });

    // Producer should finish near-instantly (it's CPU-bound; no awaits
    // inside ChannelSink).
    let producer_started = std::time::Instant::now();
    producer.await.expect("producer task panicked");
    let producer_elapsed = producer_started.elapsed();
    assert!(
        producer_elapsed < Duration::from_secs(2),
        "fast producer blocked on slow consumer for {producer_elapsed:?} \
         — bounded ChannelSink should drop on full instead of blocking"
    );

    // Drop the sink so the consumer's recv loop terminates. After this
    // point the consumer drains whatever's still buffered (≤ capacity)
    // then exits.
    drop(sink);

    // Give the consumer a generous budget to drain. With a 100ms slow
    // consumer and a capacity-256 channel that's plausibly full, the
    // tail drain can take up to ~30s in the absolute worst case. Use a
    // shorter wait + assert that the consumer count is meaningfully
    // less than the producer count (proves drop-on-full happened).
    let received = tokio::time::timeout(Duration::from_secs(45), consumer)
        .await
        .expect("consumer must drain within 45s")
        .expect("consumer task panicked");

    // The consumer cannot have received more than:
    //   (producer count) + 0      because messages can only be created by the producer
    // and must have received at most ~CHANNEL_CAPACITY + (drain ticks)
    // worth of messages. The key invariant: it received WAY fewer than
    // 1000 — proving the producer wasn't blocked into delivering all
    // 1000.
    assert!(
        received < 1000,
        "consumer received {received} messages — expected the producer \
         to be throttled by drop-on-full, not to deliver all 1000"
    );
    assert!(
        received > 0,
        "consumer received zero — channel should have buffered at least one"
    );
    assert!(
        received <= CHANNEL_CAPACITY,
        "consumer received {received} messages — must not exceed channel \
         capacity {CHANNEL_CAPACITY} because the producer is sync and the \
         consumer's recv loop and the producer's emits cannot interleave \
         until the producer task yields (which it doesn't until after the \
         tight loop ends)"
    );
}

/// Sanity: under a fast consumer the channel never fills, so no drops
/// happen. Guards against a regression where the bounded channel
/// incorrectly drops messages it shouldn't.
#[tokio::test]
async fn channel_sink_delivers_all_when_consumer_is_fast() {
    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(CHANNEL_CAPACITY);
    let sink = ChannelSink::new("c-fast".into(), "agent-fast".into(), tx);

    // Drain task spins fast — no per-message sleep.
    let drain = tokio::spawn(async move {
        let mut count = 0usize;
        while let Some(_relay) = rx.recv().await {
            count += 1;
        }
        count
    });

    // Emit fewer than CHANNEL_CAPACITY so even a brief consumer hiccup
    // doesn't fill the buffer.
    let n = (CHANNEL_CAPACITY / 2).max(64);
    for i in 0..n {
        <ChannelSink as ToolOutputSink>::emit_chunk(&sink, &format!("chunk-{i}"));
        // Tiny pause every batch so the consumer can run.
        if i % 16 == 0 {
            tokio::task::yield_now().await;
        }
    }
    drop(sink); // closes channel; drain task exits

    let got = tokio::time::timeout(Duration::from_secs(5), drain)
        .await
        .expect("consumer must finish")
        .expect("consumer panic");
    assert_eq!(got, n, "fast consumer should have received every chunk");
}
