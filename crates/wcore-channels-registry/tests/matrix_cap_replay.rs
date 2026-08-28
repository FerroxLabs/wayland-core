//! LIVE — the Matrix exactly-once guarantee holds BELOW the length cap and
//! not above it.
//!
//! # The claim under test
//!
//! `docs/delivery-semantics.md` §2 called Matrix `exactly-once` with no
//! precondition until 2026-07-31. `ChannelManager::send_to_keyed` drops the
//! idempotency key whenever the body exceeds the connector's
//! `max_message_len`, because one key cannot identify the N destination
//! messages a chunked body becomes. That branch is correct. The DECLARATION
//! that sat above it was not: above the cap no key rides the wire, so a retry
//! duplicates — see `docs/delivery-semantics.md` §4.1.
//!
//! # Anti-vacuity: why the below-cap leg is mandatory, not decoration
//!
//! "The over-cap message arrived twice" does not establish that the CAP is the
//! discriminator. It is equally well explained by "a replayed key always
//! duplicates on this platform", which would mean the exactly-once row was
//! simply false rather than conditional. The below-cap leg is the control that
//! separates the two, and it runs in the SAME session, against the SAME room,
//! through the SAME adapter and manager, replaying its key the same way. The
//! only variable between the legs is the body length.
//!
//! So the run has a reachable pass state AND a reachable fail state in each
//! direction:
//!
//! | leg | body | expectation | what a wrong answer would mean |
//! |---|---|---|---|
//! | control | under the cap | replay collapses → **1** arrival | the guarantee never held at all |
//! | subject | over the cap | replay duplicates → **2 × chunks** arrivals | the cap is not the discriminator |
//!
//! # It also drives the fix
//!
//! `ChannelManager::supports_outbound_idempotency_for(name, text)` is the
//! per-message answer added with §4.1. The test asserts it says `true` for the
//! control body and `false` for the subject body BEFORE either is sent, so the
//! product's own prediction is recorded and then checked against the platform.
//!
//! # Safety
//!
//! This drives Sean's PERSONAL Matrix account. It only ever touches the room in
//! `MATRIX_ROOM_ID` and never joins, leaves or invites. Every event it creates
//! is redacted at the end, and the redactions are VERIFIED by reading the
//! events back — matrix.org answers 200 to a redaction of an event that never
//! existed (`docs/delivery-semantics.md` §9), so an accepted redaction is not
//! evidence of a redacted event.
//!
//! Run:
//! ```text
//! MATRIX_LIVE=1 MATRIX_ACCESS_TOKEN=… MATRIX_ROOM_ID=… MATRIX_USER_ID=… \
//! MATRIX_HOMESERVER=https://matrix.org MATRIX_NONCE=… \
//!   cargo test -p wcore-channels-registry --test matrix_cap_replay -- --ignored --nocapture
//! ```

use std::sync::Arc;

use wcore_channels::ChannelManager;
use wcore_channels::chunk::chunk_message;
use wcore_channels::outgoing::OutgoingMessage;
use wcore_channels_registry::auto_register_from_dir;
use wcore_config::credentials::{CredentialsError, CredentialsStore};

const HANDLE: &str = "matrix.live.access_token";
const CHANNEL: &str = "mxcap";

/// Matrix's declared cap (`wcore-channel-matrix/src/lib.rs`
/// `max_message_len`).
///
/// Not a free-floating literal: `docs/delivery-semantics.md` carries
/// `matrix.cap = 16384` in its machine-readable block, and
/// `delivery_semantics_declaration.rs` asserts that number against the adapter
/// the production factory builds. So if the adapter's cap changes and this
/// constant is not updated, that test fails first — this one cannot quietly
/// start sending the wrong sizes and calling the result a measurement.
const CAP: usize = 16_384;

struct EnvCreds;

impl CredentialsStore for EnvCreds {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        if key == HANDLE {
            return Ok(std::env::var("MATRIX_ACCESS_TOKEN").ok());
        }
        Ok(None)
    }
    fn put(&self, _key: &str, _value: &str) -> Result<(), CredentialsError> {
        Ok(())
    }
    fn delete(&self, _key: &str) -> Result<(), CredentialsError> {
        Ok(())
    }
}

/// Missing configuration is a FAILURE, never a skip. An env-gated early
/// `return` is the self-passing shape this programme has already been burned
/// by; the test is `#[ignore]`d instead, so it never runs unasked.
fn required(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| {
        panic!(
            "{var} is not set. This test is #[ignore]d and only runs when asked for \
             explicitly; running it without live configuration is a FAILURE, not a skip."
        )
    })
}

/// The production adapter, built by the production loader from real on-disk
/// channel TOML — not hand-constructed.
async fn production_manager(dir: &std::path::Path, user_id: &str, base: &str) -> ChannelManager {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join(format!("{CHANNEL}.toml")),
        format!(
            r#"name = "{CHANNEL}"
platform = "matrix"
enabled = true

[options]
homeserver_url = "{base}"
credential_handle_access_token = "{HANDLE}"
user_id = "{user_id}"
"#
        ),
    )
    .unwrap();

    let mut mgr = ChannelManager::new();
    let creds: Arc<dyn CredentialsStore> = Arc::new(EnvCreds);
    let n = auto_register_from_dir(&mut mgr, dir, creds)
        .await
        .expect("the production loader must build the matrix adapter");
    assert_eq!(n, 1, "expected exactly one adapter from {}", dir.display());
    mgr.start_all().await.expect("start_all");
    mgr
}

/// Independent read of the room: a different code path from the adapter,
/// talking straight to the homeserver. Counting arrivals with the same client
/// that produced them would let one bug hide another.
async fn room_events(base: &str, token: &str, room: &str) -> Vec<serde_json::Value> {
    let http = wcore_egress::EgressClient::new();
    let url = format!("{base}/_matrix/client/v3/rooms/{room}/messages");
    let resp = http
        .get(url)
        .bearer_auth(token)
        .query(&[("dir", "b"), ("limit", "200")])
        .send()
        .await
        .unwrap_or_else(|e| panic!("room read: transport error: {e}"));
    let v: serde_json::Value = resp
        .json()
        .await
        .unwrap_or_else(|e| panic!("room read: response was not JSON: {e}"));
    v.get("chunk")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_else(|| panic!("room read returned no `chunk` array: {v}"))
}

fn body_of(ev: &serde_json::Value) -> &str {
    ev.get("content")
        .and_then(|c| c.get("body"))
        .and_then(|b| b.as_str())
        .unwrap_or("")
}

fn event_id_of(ev: &serde_json::Value) -> &str {
    ev.get("event_id").and_then(|e| e.as_str()).unwrap_or("")
}

/// Events whose body carries `marker`, newest-first as the homeserver returned
/// them.
fn matching(events: &[serde_json::Value], marker: &str) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|e| body_of(e).contains(marker))
        .cloned()
        .collect()
}

async fn redact(base: &str, token: &str, room: &str, event_id: &str, txn: &str) {
    let http = wcore_egress::EgressClient::new();
    let url = format!("{base}/_matrix/client/v3/rooms/{room}/redact/{event_id}/{txn}");
    let _ = http
        .put(url)
        .bearer_auth(token)
        .json(&serde_json::json!({ "reason": "wayland-core automated test cleanup" }))
        .send()
        .await;
}

#[tokio::test]
#[ignore = "live: drives a real Matrix homeserver; requires MATRIX_* configuration"]
async fn matrix_exactly_once_holds_below_the_cap_and_not_above_it() {
    assert_eq!(
        required("MATRIX_LIVE"),
        "1",
        "MATRIX_LIVE must be exactly 1 to drive a real room"
    );
    let base = required("MATRIX_HOMESERVER");
    let room = required("MATRIX_ROOM_ID");
    let user_id = required("MATRIX_USER_ID");
    let nonce = required("MATRIX_NONCE");
    let token = required("MATRIX_ACCESS_TOKEN");

    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("WAYLAND_HOME", tmp.path()) };
    let mgr = production_manager(&tmp.path().join("channels"), &user_id, &base).await;

    println!("MCR_ROOM={room}");
    println!("MCR_NONCE={nonce}");
    println!("MCR_CAP={CAP}");
    assert!(
        mgr.supports_outbound_idempotency(CHANNEL).await,
        "the per-ADAPTER bit must still be true — this test is about the precondition on it, \
         not about removing it"
    );

    // ---------------------------------------------------------------- bodies
    // Control: comfortably under the cap, one chunk.
    let ctrl_marker = format!("{nonce}-CTRL");
    let ctrl_body = format!("wayland-core cap-replay control {ctrl_marker}");

    // Subject: over the cap. Every LINE carries the marker so that every chunk
    // does, which makes "arrivals" a count of delivered chunks rather than a
    // count of first-chunks. `chunk_message` breaks on newlines, so the marker
    // cannot be split across a boundary.
    let subj_marker = format!("{nonce}-OVER");
    let mut subj_body = String::new();
    let mut line = 0usize;
    while subj_body.chars().count() <= CAP + 4_000 {
        subj_body.push_str(&format!(
            "{subj_marker} line {line:05} wayland-core cap-replay subject padding padding\n"
        ));
        line += 1;
    }

    let ctrl_chunks = chunk_message(&ctrl_body, CAP).len();
    let subj_chunks = chunk_message(&subj_body, CAP).len();
    println!(
        "MCR_BODY ctrl_chars={} ctrl_chunks={} subj_chars={} subj_chunks={}",
        ctrl_body.chars().count(),
        ctrl_chunks,
        subj_body.chars().count(),
        subj_chunks
    );
    assert_eq!(ctrl_chunks, 1, "the control body must be a single chunk");
    assert!(
        subj_chunks >= 2,
        "the subject body must actually exceed the cap, got {subj_chunks} chunk(s)"
    );

    // ------------------------------------------- the product's own prediction
    // Recorded BEFORE anything is sent, so the platform result below either
    // confirms or refutes it rather than being fitted to it.
    let ctrl_predicted = mgr
        .supports_outbound_idempotency_for(CHANNEL, &ctrl_body)
        .await;
    let subj_predicted = mgr
        .supports_outbound_idempotency_for(CHANNEL, &subj_body)
        .await;
    println!("MCR_PREDICTED ctrl={ctrl_predicted} subj={subj_predicted}");
    assert!(
        ctrl_predicted,
        "the per-message answer must be `true` below the cap, or the fix has broken the \
         guarantee it was meant to qualify"
    );
    assert!(
        !subj_predicted,
        "the per-message answer must be `false` above the cap — this is the whole of the fix"
    );

    // ------------------------------------------------------------- baselines
    let before = room_events(&base, &token, &room).await;
    let ctrl_before = matching(&before, &ctrl_marker).len();
    let subj_before = matching(&before, &subj_marker).len();
    println!("MCR_BASELINE ctrl={ctrl_before} subj={subj_before}");
    assert_eq!(
        (ctrl_before, subj_before),
        (0, 0),
        "the markers must be unique to this run; reuse would make the counts meaningless"
    );

    // ------------------------------------------------ control leg: below cap
    // The SAME delivery key twice, exactly as `dispatch_fire`'s re-attempt arm
    // would replay it.
    let ctrl_key = format!("cron:capreplay-ctrl-{nonce}:1785400000000");
    let c1 = mgr
        .send_to_keyed(
            CHANNEL,
            OutgoingMessage::text(&room, ctrl_body.clone()),
            Some(&ctrl_key),
        )
        .await
        .expect("control send 1");
    let c2 = mgr
        .send_to_keyed(
            CHANNEL,
            OutgoingMessage::text(&room, ctrl_body.clone()),
            Some(&ctrl_key),
        )
        .await
        .expect("control send 2 (the replay)");
    println!("MCR_CTRL_RECEIPTS a={} b={}", c1.id, c2.id);

    // ------------------------------------------------ subject leg: above cap
    let subj_key = format!("cron:capreplay-over-{nonce}:1785400000000");
    let s1 = mgr
        .send_to_keyed(
            CHANNEL,
            OutgoingMessage::text(&room, subj_body.clone()),
            Some(&subj_key),
        )
        .await
        .expect("subject send 1");
    let s2 = mgr
        .send_to_keyed(
            CHANNEL,
            OutgoingMessage::text(&room, subj_body.clone()),
            Some(&subj_key),
        )
        .await
        .expect("subject send 2 (the replay)");
    println!("MCR_SUBJ_RECEIPTS a={} b={}", s1.id, s2.id);

    // Give the homeserver a moment to settle before the independent read.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // ---------------------------------------------------------------- counts
    let after = room_events(&base, &token, &room).await;
    let ctrl_events = matching(&after, &ctrl_marker);
    let subj_events = matching(&after, &subj_marker);
    println!(
        "MCR_ARRIVALS ctrl={} subj={} subj_expected={}",
        ctrl_events.len(),
        subj_events.len(),
        subj_chunks * 2
    );
    for e in ctrl_events.iter().chain(subj_events.iter()) {
        println!("MCR_EVENT {}", event_id_of(e));
    }

    // ------------------------------------------------------------- cleanup
    // Before the assertions, so a FAILING run still cleans up Sean's room.
    let mut redacted: Vec<String> = Vec::new();
    for (i, e) in ctrl_events.iter().chain(subj_events.iter()).enumerate() {
        let id = event_id_of(e).to_string();
        if id.is_empty() {
            continue;
        }
        redact(&base, &token, &room, &id, &format!("wlredact-{nonce}-{i}")).await;
        redacted.push(id);
    }
    // VERIFY the redactions rather than trusting the 200 (§9): read the room
    // again and require the marker to be gone from every event.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let post = room_events(&base, &token, &room).await;
    let ctrl_left = matching(&post, &ctrl_marker).len();
    let subj_left = matching(&post, &subj_marker).len();
    println!(
        "MCR_CLEANUP redacted={} ctrl_left={ctrl_left} subj_left={subj_left}",
        redacted.len()
    );

    // ------------------------------------------------------------ the verdict
    assert_eq!(
        ctrl_events.len(),
        1,
        "CONTROL FAILED: a below-cap body replayed under the same delivery key produced {} \
         arrivals, not 1. Either the exactly-once guarantee does not hold at all — in which \
         case the §2 row is false rather than conditional — or this instrument is not \
         measuring what it thinks. Nothing about the over-cap result below can be \
         interpreted until this reads 1.",
        ctrl_events.len()
    );
    assert_eq!(
        subj_events.len(),
        subj_chunks * 2,
        "SUBJECT: an over-cap body replayed under the same delivery key produced {} arrivals; \
         {} chunks × 2 attempts = {} was expected. The control above produced exactly 1, so \
         the cap IS the discriminator and the guarantee is conditional on it.",
        subj_events.len(),
        subj_chunks,
        subj_chunks * 2
    );
    assert_eq!(
        (ctrl_left, subj_left),
        (0, 0),
        "cleanup incomplete: {ctrl_left} control and {subj_left} subject events still carry \
         their body after redaction. This is a real room; leaving test traffic in it is not \
         acceptable."
    );
    println!("MCR_DONE");
}
