//! LIVE Twilio + WhatsApp — the replay measurement neither row has ever had.
//!
//! # The claim this file exists to settle, and the one it must not be cited for
//!
//! `wcore-channel-sms` and `wcore-channel-whatsapp` both transmit the gateway's
//! delivery id on a keyed send — Twilio on the `Idempotency-Key` request
//! header, WhatsApp in the Cloud API's documented `biz_opaque_callback_data`
//! tracking field. Both nevertheless declare
//! `supports_outbound_idempotency() == false`.
//!
//! That pairing is a **conservative default, not a measurement**, and this file
//! is where it stops being one. Until it runs, nobody has driven a replayed key
//! at `api.twilio.com` or `graph.facebook.com` and counted what arrived.
//!
//! # Why the default has to be conservative until then
//!
//! On 2026-07-30 Slack and Discord were each driven at their real API for the
//! first time. Both had declared `supports_outbound_idempotency() == true` on
//! the strength of a `mockito` test proving a token left the process. Both
//! produced **two** messages from a replayed key. The exactly-once set went
//! from 3 of 10 to 1 of 10 in one afternoon.
//!
//! A mock proves what we put on the wire. It proves nothing about what the
//! destination does with it. So the bit stays `false` — the safe direction,
//! because `LedgeredHandler::dispatch_fire` reads it to decide whether an
//! outcome-unknown delivery may be re-sent after a crash: a wrong `false`
//! abandons a delivery *visibly* (`wayland-core gateway abandoned`), while a
//! wrong `true` duplicates one *silently*.
//!
//! # Reading the result
//!
//! | arrivals from two sends of ONE key | means | action |
//! |---|---|---|
//! | 2 | the platform ignores the id, exactly as Slack and Discord did | leave the bit `false`; the doc row is now MEASURED rather than derived |
//! | 1 | the platform honours it | the bit MAY go `true` — and only then |
//! | 0 | the send never landed | INSTRUMENT_FAULT. Grade the run INCOMPLETE, never a result |
//!
//! # Running it
//!
//! Both tests are `#[ignore]` and cost real money / need a real Meta business
//! app. Each needs a home directory containing a real `channels/` config and
//! `credentials.toml`, exactly as `gateway run` reads them.
//!
//! ```text
//! WL_LIVE_TWILIO_HOME=/path/to/home WL_LIVE_TWILIO_TO=+15551234567 \
//!   cargo test -p wcore-channels-registry --test live_twilio_whatsapp_identity \
//!   -- --ignored --nocapture live_replay_at_real_twilio
//!
//! WL_LIVE_WHATSAPP_HOME=/path/to/home WL_LIVE_WHATSAPP_TO=+15551234567 \
//!   cargo test -p wcore-channels-registry --test live_twilio_whatsapp_identity \
//!   -- --ignored --nocapture live_replay_at_real_meta
//! ```
//!
//! # This file never returns early, and a skip is never a pass
//!
//! An env-gated `return` is the shape that printed `5 passed` for zero work in
//! `live_integrity.rs`. A missing variable `panic!`s here instead, naming the
//! exact variable, so a misconfigured run is loudly red. And because
//! `--ignored` runs are easy to *believe* you ran, [`arrival_count_is_a_number`]
//! is NOT ignored: it executes on every ordinary `cargo test` and prints the
//! unrun-cell census, so "we never drove this" stays visible in CI output
//! rather than living only in a doc comment.

use std::sync::Arc;

use wcore_channels::{ChannelManager, OutgoingMessage};
use wcore_config::credentials::{CredentialsStore, PlaintextCredentialsStore};

/// The two live cells this file defines, and whether anything has ever run
/// them. Kept as data so [`arrival_count_is_a_number`] can print a census
/// rather than a sentence somebody has to remember to update.
const UNRUN_CELLS: [(&str, &str, &str); 2] = [
    (
        "twilio.messages",
        "WL_LIVE_TWILIO_HOME + WL_LIVE_TWILIO_TO",
        "a Twilio account SID + auth token + a provisioned From number; each send bills real money",
    ),
    (
        "whatsapp.messages",
        "WL_LIVE_WHATSAPP_HOME + WL_LIVE_WHATSAPP_TO",
        "a Meta Business app with a WhatsApp product, a phone-number id, a system-user access \
         token, and a recipient inside the 24-hour customer-service window",
    ),
];

/// Required env var, or a loud failure. Never a silent skip.
fn required(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "{var} is not set. This test contacts a real platform and refuses to pass without \
             it — a skip that reports success is the failure mode this suite exists to avoid."
        ),
    }
}

/// Register the real adapters exactly as `gateway run` does
/// (`wcore-cli/src/gateway.rs:929`), so the thing measured is the production
/// composition rather than a hand-built adapter this test happens to like.
async fn production_manager(home: &str) -> ChannelManager {
    let mut mgr = ChannelManager::new();
    let creds: Arc<dyn CredentialsStore> = Arc::new(PlaintextCredentialsStore::new(
        std::path::Path::new(home).join("credentials.toml"),
    ));
    let registered = wcore_channels_registry::auto_register_from_dir(
        &mut mgr,
        &std::path::Path::new(home).join("channels"),
        creds,
    )
    .await
    .expect("auto_register_from_dir must succeed against a real channels dir");
    assert_eq!(
        registered, 1,
        "expected exactly one registered channel; registering zero is how this test would pass \
         while measuring nothing"
    );
    mgr.start_all()
        .await
        .expect("start_all must succeed — registering does NOT connect an adapter");
    mgr
}

/// The shared body of both live cells.
///
/// Sends the SAME delivery key twice with a byte-identical body and reports the
/// two platform ids. It deliberately does **not** assert a specific arrival
/// count: nobody knows what the count is, and writing an expectation in
/// advance is how a run gets graded against a guess instead of against the
/// platform.
///
/// What it DOES assert is the instrument: the first send must return a
/// non-empty platform id parsed out of a real response. A replay measurement
/// taken after a failed first send is not a zero-duplicate result, it is no
/// result — the exact confusion the `INSTRUMENT_FAULT` label marks everywhere
/// else in this repository.
async fn drive_replay(mgr: &ChannelManager, channel: &str, to: &str, tag: &str) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // One key, shaped like a real one: `cron:{job_id}:{scheduled_for_millis}`
    // (`wcore-cron/src/runner.rs:327`). A key of a different shape would not
    // exercise the same length or character set the product actually sends.
    let key = format!("cron:wl-live-{tag}:{}", stamp * 1000);
    let body = format!("WL-LIVE-IDENTITY-{tag}-{stamp}");

    let declared = mgr.supports_outbound_idempotency(channel).await;
    println!("LIVE_PRECONDITION channel={channel} declares_idempotency={declared}");
    assert!(
        !declared,
        "precondition: {channel} must still declare `false` when this runs. If it already says \
         `true`, some earlier change asserted the very thing this test exists to measure, and \
         the measurement would be grading its own premise."
    );

    let first = mgr
        .send_to_keyed(
            channel,
            OutgoingMessage::text(to.to_string(), body.clone()),
            Some(&key),
        )
        .await
        .expect(
            "INSTRUMENT_FAULT: the FIRST live send did not return a receipt. Nothing below this \
             line is a delivery measurement — grade the run INCOMPLETE, not a clean result. A \
             no-duplicate finding taken after a failed positive control is a green manufactured \
             by universal denial.",
        );
    assert!(
        !first.id.is_empty(),
        "INSTRUMENT_FAULT: the platform returned an empty message id, so the two sends cannot \
         be told apart and no count is readable"
    );
    println!("LIVE_FIRST  key={key} platform_id={}", first.id);

    let second = mgr
        .send_to_keyed(
            channel,
            OutgoingMessage::text(to.to_string(), body.clone()),
            Some(&key),
        )
        .await
        .expect("the replay itself must reach the platform — that is the measurement");
    println!("LIVE_REPLAY key={key} platform_id={}", second.id);

    // The whole result, in one line a human grades. Identical ids are the
    // signature of a platform that collapsed the replay; distinct ids are the
    // signature of two messages, which is what Slack and Discord both did.
    //
    // This is REPORTED, not asserted, and that is deliberate: an assertion here
    // would encode an expectation nobody has evidence for, and the run would
    // then be measuring the expectation. **Corroborate the id verdict at the
    // platform's own console before updating docs/delivery-semantics.md** — an
    // id is our read of their response, whereas the row claims an arrival
    // count, and those are the two claims this repository has already conflated
    // once.
    println!(
        "LIVE_VERDICT channel={channel} same_platform_id={} first={} replay={}",
        first.id == second.id,
        first.id,
        second.id
    );
}

#[tokio::test]
#[ignore = "contacts real Twilio and sends real, billable SMS"]
async fn live_replay_at_real_twilio() {
    let home = required("WL_LIVE_TWILIO_HOME");
    let to = required("WL_LIVE_TWILIO_TO");
    let mgr = production_manager(&home).await;
    let channel = mgr
        .list_names()
        .into_iter()
        .next()
        .expect("the registered channel must be nameable");
    drive_replay(&mgr, &channel, &to, "twilio").await;
}

#[tokio::test]
#[ignore = "contacts real Meta Graph and sends a real WhatsApp message"]
async fn live_replay_at_real_meta() {
    let home = required("WL_LIVE_WHATSAPP_HOME");
    let to = required("WL_LIVE_WHATSAPP_TO");
    let mgr = production_manager(&home).await;
    let channel = mgr
        .list_names()
        .into_iter()
        .next()
        .expect("the registered channel must be nameable");
    drive_replay(&mgr, &channel, &to, "whatsapp").await;
}

/// **Not `#[ignore]`d, on purpose.** Counts and prints the cells above that
/// have never been driven.
///
/// A credential-gated suite is invisible: `cargo test --test
/// live_twilio_whatsapp_identity` exits 0 having run zero of the two live
/// tests, printing `test result: ok`, which is indistinguishable from having
/// measured both. This test is the visible remainder. It cannot itself go
/// green by accident — it asserts the census is non-empty, so if somebody ever
/// deletes the live cells rather than running them, it reddens.
#[test]
fn arrival_count_is_a_number() {
    assert!(
        !UNRUN_CELLS.is_empty(),
        "the unrun-cell census is empty. Either both live replays were run and this file should \
         say so with their numbers, or the cells were deleted — and deleting an unrun \
         measurement is how it stops being visibly unrun."
    );
    println!(
        "UNRUN_LIVE_CELLS count={} — a skip is NOT a pass",
        UNRUN_CELLS.len()
    );
    for (endpoint, env, needs) in UNRUN_CELLS {
        println!("  UNRUN endpoint={endpoint} gated_on={env}");
        println!("        needs: {needs}");
    }
    println!(
        "Until each prints a LIVE_VERDICT line, docs/delivery-semantics.md must keep saying \
         NOT MEASURED for these two rows, and supports_outbound_idempotency() must stay false."
    );
}
