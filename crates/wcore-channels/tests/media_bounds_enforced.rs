//! `media_bounds()` is load-bearing — proof, not assertion.
//!
//! Before this suite existed, `Channel::media_bounds` was declared on the
//! trait, overridden by two adapters, and READ at exactly one site in the whole
//! workspace: a unit test. Every adapter that enforced a cap enforced an
//! unrelated hardcoded constant that diverged from the declaration, in both
//! directions — discord advertised 25 MiB and fetched up to 100, email
//! advertised 10 MiB and never inlined past 2, and seven further adapters
//! advertised nothing while enforcing 100/64/16 MiB.
//!
//! # Why each test here carries THREE assertions, not two
//!
//! A known-negative ("oversize media is refused") is self-passing on a dead
//! instrument: a fake that never returns bytes, a manager that errors for an
//! unrelated reason, or a channel that was never registered all produce a
//! refusal for free. So every case below asserts:
//!
//!   1. a KNOWN-POSITIVE — a payload at exactly the bound is returned intact,
//!      proving the path is alive and not refusing everything;
//!   2. a KNOWN-NEGATIVE — one byte over the bound is refused, and the reason
//!      names the measurement;
//!   3. THE OLD SHAPE WOULD HAVE MISSED IT — the unchecked trait method still
//!      hands back the oversize payload, so the refusal in (2) is demonstrably
//!      produced by the enforcement this change added and not by something that
//!      was already there. Without (3) the suite passes against the pre-fix
//!      code, which is precisely how a self-passing gate is built.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use wcore_channels::Channel;
use wcore_channels::error::ChannelError;
use wcore_channels::event::{Attachment, ChannelEvent, MessageReceipt};
use wcore_channels::manager::ChannelManager;
use wcore_channels::media::MediaBounds;
use wcore_channels::outgoing::OutgoingMessage;

/// A channel that declares a bound and serves a payload of a size the test
/// chooses. The two are set INDEPENDENTLY on purpose: that is the whole defect
/// being reproduced — an adapter whose advertised number and delivered number
/// are unrelated.
struct BoundedChannel {
    name: String,
    declared: MediaBounds,
    /// How many bytes `fetch_media` hands back, regardless of `declared`.
    serves_bytes: usize,
    /// Counts real calls, so a test can prove the adapter was actually reached
    /// rather than short-circuited before it.
    fetches: Arc<AtomicUsize>,
}

impl BoundedChannel {
    fn new(name: &str, declared_max: u64, serves_bytes: usize) -> Self {
        Self {
            name: name.to_string(),
            declared: MediaBounds {
                max_bytes: declared_max,
                max_attachments: 10,
            },
            serves_bytes,
            fetches: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl Channel for BoundedChannel {
    fn name(&self) -> &str {
        &self.name
    }
    fn platform(&self) -> &str {
        "bounded-test"
    }
    async fn start(&mut self) -> Result<(), ChannelError> {
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), ChannelError> {
        Ok(())
    }
    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        Ok(vec![])
    }
    async fn send_message(
        &mut self,
        _msg: OutgoingMessage,
    ) -> Result<MessageReceipt, ChannelError> {
        Err(ChannelError::Unsupported {
            op: "send".to_string(),
            platform: "bounded-test".to_string(),
        })
    }
    fn config_schema(&self) -> &str {
        "{}"
    }

    fn media_bounds(&self) -> MediaBounds {
        self.declared
    }

    /// Deliberately UNCHECKED against `self.declared`. This models every real
    /// adapter as it behaved before the fix: the declaration existed and the
    /// fetch path ignored it.
    async fn fetch_media(&self, _attachment: &Attachment) -> Result<Vec<u8>, ChannelError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        Ok(vec![b'x'; self.serves_bytes])
    }
}

fn att() -> Attachment {
    Attachment {
        url: "https://example.invalid/a.png".to_string(),
        ..Default::default()
    }
}

/// ASSERTION 1 — KNOWN-POSITIVE.
///
/// A payload of exactly `max_bytes` is returned intact. Without this, the
/// refusal cases below would pass against a `fetch_media_on` that refuses
/// unconditionally, which proves nothing at all.
#[tokio::test]
async fn a_payload_at_exactly_the_declared_bound_is_returned_intact() {
    let bound = 1024u64;
    let ch = BoundedChannel::new("at-bound", bound, bound as usize);
    let fetches = Arc::clone(&ch.fetches);

    let mut mgr = ChannelManager::new();
    mgr.register(Box::new(ch)).await;

    let bytes = mgr
        .fetch_media_on("at-bound", &att())
        .await
        .expect("a payload at the bound must be accepted; the bound is inclusive");

    assert_eq!(
        bytes.len(),
        bound as usize,
        "the payload must come back whole, not truncated — truncation would be \
         a silent drop, which is the one thing the media contract forbids"
    );
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        1,
        "the adapter must actually have been reached; a zero here would mean \
         the test proved nothing about the fetch path"
    );
}

/// ASSERTION 2 — KNOWN-NEGATIVE, with the reason naming the measurement.
#[tokio::test]
async fn one_byte_over_the_declared_bound_is_refused_and_the_reason_names_it() {
    let bound = 1024u64;
    let ch = BoundedChannel::new("over-bound", bound, bound as usize + 1);
    let fetches = Arc::clone(&ch.fetches);

    let mut mgr = ChannelManager::new();
    mgr.register(Box::new(ch)).await;

    let err = mgr
        .fetch_media_on("over-bound", &att())
        .await
        .expect_err("one byte over the declared bound must be refused");

    let msg = err.to_string();
    assert!(
        msg.contains("1025"),
        "the refusal must name the measured size so an operator can tell an \
         oversize payload from a broken fetch, got: {msg}"
    );
    assert!(
        msg.contains("1024"),
        "the refusal must name the bound it was measured against, got: {msg}"
    );
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        1,
        "the adapter must have been reached — a refusal produced by never \
         calling the adapter is the self-passing shape this suite exists to \
         rule out"
    );
}

/// ASSERTION 3 — THE OLD SHAPE WOULD HAVE MISSED IT.
///
/// This is the only assertion that proves the enforcement does anything. It
/// reproduces the exact discord divergence — declare 25 MiB, deliver 100 MiB —
/// and shows both halves in one test:
///
///   - the UNCHECKED trait method (`Channel::fetch_media`, which is what every
///     adapter did and all the pre-fix code ever called) hands the oversize
///     payload straight back;
///   - the CHECKED manager path refuses the identical payload.
///
/// Run this against the pre-fix `fetch_media_on` — a two-line body that
/// delegated to `guard.fetch_media(attachment)` — and the second half fails,
/// because the bytes came back. That is what makes this suite non-vacuous.
#[tokio::test]
async fn the_unchecked_path_still_yields_oversize_bytes_that_the_manager_refuses() {
    // The real numbers, scaled down so the test does not allocate 100 MiB:
    // discord declared 25 and served up to 100, a 4x divergence.
    let declared = 25 * 1024u64;
    let served = 100 * 1024usize;
    let ch = BoundedChannel::new("discord-shape", declared, served);

    // --- Half A: the pre-fix shape. Call the trait method directly, exactly
    // as `fetch_media_on` used to. The declaration is ignored and the oversize
    // payload is produced.
    let direct = ch
        .fetch_media(&att())
        .await
        .expect("the unchecked adapter path returns bytes");
    assert_eq!(
        direct.len(),
        served,
        "sanity: the adapter really does serve more than it declares — if this \
         ever fails the fixture stopped reproducing the defect and the rest of \
         this test would be checking nothing"
    );
    assert!(
        direct.len() as u64 > declared,
        "the fixture must be OVER its own declared bound for this test to mean \
         anything: served {} vs declared {declared}",
        direct.len()
    );

    // --- Half B: the fixed shape. Same adapter, same payload, routed through
    // the only production path to adapter media.
    let mut mgr = ChannelManager::new();
    mgr.register(Box::new(ch)).await;

    let err = mgr
        .fetch_media_on("discord-shape", &att())
        .await
        .expect_err(
            "the manager must refuse a payload over the channel's DECLARED \
             bound — this is the assertion that fails against the pre-fix \
             two-line fetch_media_on, and the reason this suite is not \
             self-passing",
        );
    assert!(
        err.to_string().contains(&declared.to_string()),
        "the refusal must cite the declared bound, got: {err}"
    );
}

/// An adapter that declares NOTHING inherits a finite default and is bounded by
/// it — the seven-adapter case. Pre-fix these were bounded only by whatever
/// hardcoded constant happened to be in their download path, and by nothing at
/// all in the manager.
#[tokio::test]
async fn an_adapter_that_declares_nothing_is_still_bounded_by_the_finite_default() {
    struct SilentChannel {
        payload: usize,
    }
    #[async_trait]
    impl Channel for SilentChannel {
        fn name(&self) -> &str {
            "silent"
        }
        fn platform(&self) -> &str {
            "bounded-test"
        }
        async fn start(&mut self) -> Result<(), ChannelError> {
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), ChannelError> {
            Ok(())
        }
        async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
            Ok(vec![])
        }
        async fn send_message(
            &mut self,
            _msg: OutgoingMessage,
        ) -> Result<MessageReceipt, ChannelError> {
            Err(ChannelError::Unsupported {
                op: "send".to_string(),
                platform: "bounded-test".to_string(),
            })
        }
        fn config_schema(&self) -> &str {
            "{}"
        }
        // NOTE: no `media_bounds` override — this is the seven-adapter shape.
        async fn fetch_media(&self, _a: &Attachment) -> Result<Vec<u8>, ChannelError> {
            Ok(vec![b'x'; self.payload])
        }
    }

    let over = MediaBounds::DEFAULT_MAX_BYTES as usize + 1;
    let mut mgr = ChannelManager::new();
    mgr.register(Box::new(SilentChannel { payload: over }))
        .await;

    let err = mgr
        .fetch_media_on("silent", &att())
        .await
        .expect_err("the finite default must bind an adapter that declares nothing");
    assert!(
        err.to_string()
            .contains(&MediaBounds::DEFAULT_MAX_BYTES.to_string()),
        "the refusal must cite the default it applied, got: {err}"
    );
}

/// The bound is read from the ORIGINATING channel, not from some global. Two
/// channels with different declarations must be judged differently for the same
/// payload — otherwise "per-adapter bounds" would be per-adapter in name only,
/// which is the failure class this whole change addresses.
#[tokio::test]
async fn each_channel_is_judged_against_its_own_declaration() {
    let payload = 4096usize;
    let mut mgr = ChannelManager::new();
    // Generous declares more than the payload; strict declares less.
    mgr.register(Box::new(BoundedChannel::new(
        "generous",
        payload as u64 * 2,
        payload,
    )))
    .await;
    mgr.register(Box::new(BoundedChannel::new(
        "strict",
        payload as u64 / 2,
        payload,
    )))
    .await;

    assert!(
        mgr.fetch_media_on("generous", &att()).await.is_ok(),
        "the generous channel declared room for this payload"
    );
    assert!(
        mgr.fetch_media_on("strict", &att()).await.is_err(),
        "the strict channel did not — same bytes, different declaration, so \
         the bound must be sourced per channel"
    );
}

/// `media_bounds_on` reports the declaration the enforcement uses. If these two
/// could disagree, the count bound applied by the enricher would be read from a
/// different place than the byte bound applied here — reintroducing exactly the
/// two-numbers-for-one-limit defect at a new seam.
#[tokio::test]
async fn the_reported_bounds_are_the_bounds_that_get_enforced() {
    let declared = 2048u64;
    let mut mgr = ChannelManager::new();
    mgr.register(Box::new(BoundedChannel::new(
        "reporter",
        declared,
        declared as usize + 1,
    )))
    .await;

    let reported = mgr
        .media_bounds_on("reporter")
        .await
        .expect("a registered channel must report its bounds");
    assert_eq!(reported.max_bytes, declared);

    let err = mgr.fetch_media_on("reporter", &att()).await.unwrap_err();
    assert!(
        err.to_string().contains(&reported.max_bytes.to_string()),
        "the number enforced must be the number reported, got: {err}"
    );

    assert!(
        mgr.media_bounds_on("no-such-channel").await.is_none(),
        "an unknown channel must report None, not a default that would read as \
         a real declaration"
    );
}
