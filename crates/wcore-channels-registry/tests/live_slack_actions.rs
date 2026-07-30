//! The five Slack message actions — send, edit, delete, receive, idempotency —
//! driven against the **real Slack Web API**, through the adapter the product
//! actually ships.
//!
//! # Why this file exists
//!
//! Every existing Slack test in this workspace talks to `mockito`. A mock
//! answers whatever it was told to answer, so it can prove that our request has
//! the right shape and it can prove **nothing at all** about what Slack does
//! with it. That gap was not academic: `docs/delivery-semantics.md` declared
//! Slack **exactly-once** on the strength of a mock plus the sentence *"the key
//! was present on both attempts"* — which is a statement about our wire, not
//! about the destination's arrival count. Driving the real API on 2026-07-30
//! showed a replayed key producing a **second message**. See
//! [`leg_idempotency`].
//!
//! # What "real" means here, precisely
//!
//! * The adapter is built by [`channel_factory_for`] — the same production
//!   factory `auto_register_from_dir` uses at boot. A hand-rolled HTTP client,
//!   or a `SlackChannel::new` call, would be evidence about the test rather
//!   than about the shipped binary.
//! * Every write goes through the `Channel` trait: `send_message`,
//!   `send_message_idempotent`, `edit_message`, `delete_message`,
//!   `ingest_webhook`, `poll_events`.
//! * Every write is then **read back from `conversations.history`** before it
//!   is believed. An HTTP 200 is not evidence that state changed; it is
//!   evidence that a request was accepted. Those are different claims and this
//!   file only ever asserts the second one after checking the first.
//!
//! # Both directions, per LANE-BRIEF §3.2 and §3b-iii
//!
//! A gate that cannot fail is worthless, and one that cannot pass is worse.
//! Every leg below therefore carries a control that runs **in the same
//! invocation** as the positive case:
//!
//! | leg | can it pass | can it fail |
//! |---|---|---|
//! | send | the message is read back from history at the returned `ts` | a bogus channel id is refused with the platform's own `channel_not_found` |
//! | edit | history shows the NEW text, and no longer the old one | `chat.update` on a fabricated `ts` is refused with `message_not_found` |
//! | delete | history no longer contains the `ts` | a SECOND delete of that same `ts` is refused with `message_not_found` |
//! | receive | the real message is found in history, and the real record replayed through `ingest_webhook` surfaces as `MessageReceived` | a corrupted signature is rejected and enqueues nothing; a marker never posted is found zero times |
//! | idempotency | a genuinely DIFFERENT key adds one more arrival | a replayed key's arrival count must match what the adapter DECLARES; either side changing reddens |
//!
//! The idempotency row is the important one. It does not hardcode "1" or "2".
//! It reads [`Channel::supports_outbound_idempotency`] and requires the
//! platform's measured behaviour to match the declaration. So it reddens if
//! someone claims a guarantee Slack does not provide, **and** it reddens if
//! Slack starts honouring the key while we still say it does not. Neither
//! direction is hypothetical reasoning; both are reachable states of the same
//! assertion.
//!
//! # Why one test and not five
//!
//! The five legs share one live channel in a real company workspace. `cargo
//! test` runs test functions in parallel, so five functions would interleave
//! their writes and each one's arrival count would include the others' messages
//! — the shared-resource miscount LANE-BRIEF §6a-ii describes. Running them
//! sequentially inside one function also means the cleanup sweep at the end
//! runs **once, unconditionally**, after every leg has had its turn. Each leg
//! returns a `Result` rather than panicking precisely so that one failure
//! cannot skip the sweep and strand messages in a live workspace.
//!
//! # Running it
//!
//! ```text
//! export WL_LIVE_SLACK=1
//! export SLACK_BOT_TOKEN=…      # never echoed, never logged, never asserted on
//! export SLACK_SIGNING_SECRET=… #  ditto
//! export WL_SLACK_CHANNEL=C0BLR1UKKU6
//! cargo test -p wcore-channels-registry --test live_slack_actions -- --ignored --nocapture
//! ```
//!
//! `#[ignore]` keeps it out of CI, because it posts to and deletes from a real
//! workspace. But an explicitly-requested live run must never quietly do
//! nothing: with `--ignored` set and the environment incomplete, this test
//! **panics naming the missing variable** rather than returning early. An
//! env-gated early `return` printing `1 passed` for zero work is the exact
//! self-passing shape LANE-BRIEF §3.2 flavour (b) records.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use wcore_channels::event::ChannelEvent;
use wcore_channels::outgoing::OutgoingMessage;
use wcore_channels::{Channel, WebhookRequest};
use wcore_channels_registry::channel_factory_for;
use wcore_config::credentials::{CredentialsError, CredentialsStore};

/// Credential handles the fixture config points at. Arbitrary strings — the
/// point is only that the adapter resolves the secret through the
/// `CredentialsStore` seam exactly as it does in production, rather than
/// receiving it directly.
const BOT_TOKEN_HANDLE: &str = "live.slack.bot_token";
const SIGNING_SECRET_HANDLE: &str = "live.slack.signing_secret";

/// Scopes this file needs, and the leg each one serves.
///
/// `wayland-test` is a **private** channel, so the `channels:*` family does not
/// apply to it — private conversations need the `groups:*` twins. That
/// distinction cost this lane its first scope probe: a token holding
/// `channels:history` reads as "history is granted" and still returns
/// `missing_scope needed: groups:history` against a private channel.
const REQUIRED_SCOPES: &[(&str, &str)] = &[
    ("chat:write", "send / edit / delete"),
    (
        "groups:history",
        "reading writes back from a PRIVATE channel",
    ),
    ("groups:read", "resolving the private channel itself"),
];

/// Every message this run creates carries this tag, and the cleanup sweep
/// deletes exactly the messages that carry it.
///
/// Scoped per-run rather than a fixed string: two concurrent runs (or a run
/// racing a human in the same channel) must not delete each other's messages.
/// LANE-BRIEF §6a-ii — an over-broad glob that catches somebody else's output
/// is not your measurement, and here it would not even be your message.
const MARKER_PREFIX: &str = "WL-LIVE-SLACK";

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

/// The live configuration. Secret fields are read once and never rendered:
/// nothing in this file formats them into an assertion message, a panic, or a
/// printed line.
struct Live {
    channel_id: String,
    bot_token: String,
    signing_secret: String,
    run_tag: String,
}

impl Live {
    /// A unique message body for one leg of one run.
    fn marker(&self, leg: &str) -> String {
        format!("{MARKER_PREFIX} {} {leg}", self.run_tag)
    }
}

/// Read a required variable, or fail naming the VARIABLE (never the value).
fn required_env(key: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "{key} is unset or empty. This test was invoked with --ignored, which means a live \
             run was explicitly requested; returning quietly would print a pass for zero work. \
             Set WL_LIVE_SLACK=1, SLACK_BOT_TOKEN, SLACK_SIGNING_SECRET and WL_SLACK_CHANNEL."
        ),
    }
}

fn live_env() -> Live {
    let gate = required_env("WL_LIVE_SLACK");
    assert_eq!(
        gate, "1",
        "WL_LIVE_SLACK must be exactly \"1\" to drive a real workspace"
    );
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the epoch")
        .as_nanos();
    Live {
        channel_id: required_env("WL_SLACK_CHANNEL"),
        bot_token: required_env("SLACK_BOT_TOKEN"),
        signing_secret: required_env("SLACK_SIGNING_SECRET"),
        run_tag: format!("{}-{}", std::process::id(), nanos),
    }
}

// ---------------------------------------------------------------------------
// The adapter, built the way production builds it
// ---------------------------------------------------------------------------

struct MemStore(Mutex<HashMap<String, String>>);

impl CredentialsStore for MemStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

/// Build and start the Slack adapter through the production registry factory.
async fn started_adapter(live: &Live) -> Box<dyn Channel> {
    let options: toml::Table = toml::from_str(&format!(
        "workspace_name = \"wl-live\"\n\
         default_channel_id = \"{}\"\n\
         credential_handle_bot_token = \"{BOT_TOKEN_HANDLE}\"\n\
         credential_handle_signing_secret = \"{SIGNING_SECRET_HANDLE}\"\n",
        live.channel_id
    ))
    .expect("fixture options must parse");

    let mut secrets = HashMap::new();
    secrets.insert(BOT_TOKEN_HANDLE.to_string(), live.bot_token.clone());
    secrets.insert(
        SIGNING_SECRET_HANDLE.to_string(),
        live.signing_secret.clone(),
    );
    let store: Arc<dyn CredentialsStore> = Arc::new(MemStore(Mutex::new(secrets)));

    let factory = channel_factory_for("slack").expect("the registry must know the slack platform");
    let mut channel = factory("wl-live-slack".to_string(), &options, store)
        .expect("the production factory must construct the slack adapter");
    channel
        .start()
        .await
        .expect("start() must resolve both credentials");
    // start() enqueues a ConnectionStateChanged; drain it so the receive leg's
    // inbox assertions see only what that leg put there.
    let _ = channel.poll_events().await;
    channel
}

// ---------------------------------------------------------------------------
// A read path. The adapter has none — it receives by webhook — so verifying a
// write means calling Slack directly.
// ---------------------------------------------------------------------------

/// Raw `GET https://slack.com/api/<method>` with the bot token.
async fn slack_get(live: &Live, method: &str, query: &[(&str, &str)]) -> serde_json::Value {
    let http = wcore_egress::EgressClient::new();
    let resp = http
        .get(format!("https://slack.com/api/{method}"))
        .bearer_auth(&live.bot_token)
        .query(query)
        .send()
        .await
        .unwrap_or_else(|e| panic!("{method}: transport error: {e}"));
    resp.json::<serde_json::Value>()
        .await
        .unwrap_or_else(|e| panic!("{method}: response was not JSON: {e}"))
}

/// Every message currently in the bound channel.
async fn history(live: &Live) -> Vec<serde_json::Value> {
    let v = slack_get(
        live,
        "conversations.history",
        &[("channel", live.channel_id.as_str()), ("limit", "200")],
    )
    .await;
    assert_eq!(
        v.get("ok").and_then(|b| b.as_bool()),
        Some(true),
        "conversations.history failed: {:?}",
        v.get("error")
    );
    v.get("messages")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default()
}

fn text_of(m: &serde_json::Value) -> &str {
    m.get("text").and_then(|t| t.as_str()).unwrap_or("")
}

fn ts_of(m: &serde_json::Value) -> &str {
    m.get("ts").and_then(|t| t.as_str()).unwrap_or("")
}

/// Messages in the channel whose text is EXACTLY `body`.
async fn arrivals(live: &Live, body: &str) -> Vec<serde_json::Value> {
    history(live)
        .await
        .into_iter()
        .filter(|m| text_of(m) == body)
        .collect()
}

/// The one message at `ts`, if it is still there.
async fn message_at(live: &Live, ts: &str) -> Option<serde_json::Value> {
    history(live).await.into_iter().find(|m| ts_of(m) == ts)
}

// ---------------------------------------------------------------------------
// Leg plumbing
// ---------------------------------------------------------------------------

/// Assert inside a leg without panicking — a panic here would skip the sweep
/// and strand messages in a live company workspace.
macro_rules! need {
    ($cond:expr, $($arg:tt)*) => {
        if !$cond {
            return Err(format!($($arg)*));
        }
    };
}

type LegResult = Result<String, String>;

/// Render the trait error the way an operator would see it. Slack's own error
/// code has to survive to this string, which is the thing several assertions
/// below check for.
fn rendered(e: &wcore_channels::ChannelError) -> String {
    e.to_string()
}

// ---------------------------------------------------------------------------
// Leg 1 — send
// ---------------------------------------------------------------------------

async fn leg_send(live: &Live, ch: &mut Box<dyn Channel>) -> LegResult {
    let body = live.marker("send");

    let receipt = ch
        .send_message(OutgoingMessage::text(&live.channel_id, &body))
        .await
        .map_err(|e| format!("send_message failed: {}", rendered(&e)))?;
    need!(!receipt.id.is_empty(), "receipt carried an empty ts");

    // The 200 is not the evidence. This is.
    let found = arrivals(live, &body).await;
    need!(
        found.len() == 1,
        "expected exactly one arrival for the sent body, history holds {}",
        found.len()
    );
    need!(
        ts_of(&found[0]) == receipt.id,
        "the receipt's ts ({}) is not the ts the message actually has ({})",
        receipt.id,
        ts_of(&found[0])
    );
    need!(
        receipt.conversation_id == live.channel_id,
        "receipt names conversation {} but we sent to {}",
        receipt.conversation_id,
        live.channel_id
    );

    // Instrument control: the finder must be able to return zero. Without this
    // a finder that returned [] for everything would pass every "is it gone?"
    // assertion in this file, and a finder that returned the whole channel
    // would pass none of them for the wrong reason.
    let never_posted = arrivals(live, &format!("{body} NEVER-POSTED")).await;
    need!(
        never_posted.is_empty(),
        "the arrival finder reported {} hits for a body that was never sent — the instrument is \
         not measuring what it claims",
        never_posted.len()
    );

    // Failing direction: the platform must refuse a channel that does not
    // exist, with its own error code, rather than succeeding quietly.
    let neg = ch
        .send_message(OutgoingMessage::text(
            "C00000000000BOGUS",
            &live.marker("send-negative-control"),
        ))
        .await;
    let err = match neg {
        Ok(r) => {
            return Err(format!(
                "sending to a fabricated channel SUCCEEDED, ts {} — the negative control cannot \
                 fail, so the positive case above proves nothing",
                r.id
            ));
        }
        Err(e) => rendered(&e),
    };
    need!(
        err.contains("channel_not_found") || err.contains("invalid_arguments"),
        "the platform's own refusal code must reach the operator, got: {err}"
    );

    Ok(format!(
        "sent ts={} and read it back from history; a fabricated channel was refused with {err}",
        receipt.id
    ))
}

// ---------------------------------------------------------------------------
// Leg 2 — edit
// ---------------------------------------------------------------------------

async fn leg_edit(live: &Live, ch: &mut Box<dyn Channel>) -> LegResult {
    let before = live.marker("edit-before");
    let after = live.marker("edit-after");

    let receipt = ch
        .send_message(OutgoingMessage::text(&live.channel_id, &before))
        .await
        .map_err(|e| format!("edit leg: seed send failed: {}", rendered(&e)))?;

    // Precondition: the OLD text is genuinely there first, so that the change
    // measured below was caused by the edit and not by the message having been
    // that way all along.
    let seeded = message_at(live, &receipt.id)
        .await
        .ok_or_else(|| format!("edit leg: seeded message {} is not in history", receipt.id))?;
    need!(
        text_of(&seeded) == before,
        "edit leg: seeded text is {:?}, expected {before:?}",
        text_of(&seeded)
    );

    ch.edit_message(&live.channel_id, &receipt.id, &after)
        .await
        .map_err(|e| format!("edit_message failed: {}", rendered(&e)))?;

    // Read back. Both halves matter: the new text is present AND the old text
    // is gone. Asserting only the first would pass against a platform that
    // appended rather than replaced.
    let edited = message_at(live, &receipt.id)
        .await
        .ok_or_else(|| format!("edit leg: message {} vanished during the edit", receipt.id))?;
    need!(
        text_of(&edited) == after,
        "history still shows {:?} after the edit, expected {:?}",
        text_of(&edited),
        after
    );
    need!(
        text_of(&edited) != before,
        "the text did not actually change — chat.update returned 200 and the state is unchanged"
    );

    // Failing direction: editing a message that does not exist must error.
    let neg = ch
        .edit_message(&live.channel_id, "9999999999.000000", "must not apply")
        .await;
    let err = match neg {
        Ok(r) => {
            return Err(format!(
                "editing a fabricated ts SUCCEEDED (returned {}) — chat.update is not validating \
                 the target",
                r.id
            ));
        }
        Err(e) => rendered(&e),
    };
    need!(
        err.contains("message_not_found"),
        "expected the platform's message_not_found on a fabricated ts, got: {err}"
    );

    Ok(format!(
        "edited ts={}; history text changed {before:?} -> {after:?}; a fabricated ts was refused \
         with {err}",
        receipt.id
    ))
}

// ---------------------------------------------------------------------------
// Leg 3 — delete
// ---------------------------------------------------------------------------

async fn leg_delete(live: &Live, ch: &mut Box<dyn Channel>) -> LegResult {
    let body = live.marker("delete");

    let receipt = ch
        .send_message(OutgoingMessage::text(&live.channel_id, &body))
        .await
        .map_err(|e| format!("delete leg: seed send failed: {}", rendered(&e)))?;
    need!(
        message_at(live, &receipt.id).await.is_some(),
        "delete leg: the message to delete is not in history to begin with, so its later absence \
         would prove nothing"
    );

    ch.delete_message(&live.channel_id, &receipt.id)
        .await
        .map_err(|e| format!("delete_message failed: {}", rendered(&e)))?;

    // The read-back is the proof. `chat.delete` returning ok:true is the claim.
    need!(
        message_at(live, &receipt.id).await.is_none(),
        "chat.delete returned success but message {} is still in the channel",
        receipt.id
    );

    // Failing direction: deleting the same ts a second time must error. This is
    // also a second, independent witness that the first delete really landed.
    let neg = ch.delete_message(&live.channel_id, &receipt.id).await;
    let err = match neg {
        Ok(()) => {
            return Err(
                "deleting the SAME ts twice both succeeded — either delete is a no-op that always \
                 reports success, or the first one did not happen"
                    .to_string(),
            );
        }
        Err(e) => rendered(&e),
    };
    need!(
        err.contains("message_not_found"),
        "expected message_not_found on the second delete, got: {err}"
    );

    Ok(format!(
        "deleted ts={} and confirmed its absence from history; the second delete was refused with \
         {err}",
        receipt.id
    ))
}

// ---------------------------------------------------------------------------
// Leg 4 — receive
// ---------------------------------------------------------------------------

/// Slack's `v0` request signature, computed here **independently** of the
/// adapter's own `auth::expected_signature`.
///
/// Signing with the product's own helper and then verifying with the product's
/// own verifier would agree with itself no matter what either one computed —
/// the tautology LANE-BRIEF §3b-i describes. This is a second implementation
/// from Slack's published spec, so agreement between them is information.
fn slack_v0_signature(secret: &str, timestamp: &str, body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts a key of any length");
    mac.update(format!("v0:{timestamp}:{body}").as_bytes());
    format!("v0={}", hex::encode(mac.finalize().into_bytes()))
}

/// Wrap a real `conversations.history` record in the Events API envelope Slack
/// posts to a webhook.
///
/// **Honest scope of this construction.** The message fields are real — they
/// were produced by Slack in response to our send and read back from Slack. The
/// envelope around them is built here, because Slack cannot POST to this test:
/// the Events API needs a publicly reachable HTTPS endpoint and an event
/// subscription on the app, which is Slack-app configuration and not something
/// a scope grant provides. So this exercises the adapter's real signature
/// verification, real replay window and real parser over real message data; it
/// does not exercise Slack's own delivery. That limitation is recorded rather
/// than papered over.
fn events_api_envelope(record: &serde_json::Value, channel_id: &str) -> String {
    let mut event = record.clone();
    let obj = event
        .as_object_mut()
        .expect("a history record is an object");
    obj.insert(
        "channel".to_string(),
        serde_json::Value::String(channel_id.to_string()),
    );
    // `bot_profile` and `blocks` are noise for the parser and make the payload
    // large; the parser reads type/subtype/channel/user/text/ts/team.
    obj.remove("bot_profile");
    obj.remove("blocks");
    serde_json::json!({ "type": "event_callback", "event": event }).to_string()
}

async fn leg_receive(live: &Live, ch: &mut Box<dyn Channel>) -> LegResult {
    let body = live.marker("receive");

    let receipt = ch
        .send_message(OutgoingMessage::text(&live.channel_id, &body))
        .await
        .map_err(|e| format!("receive leg: seed send failed: {}", rendered(&e)))?;

    // --- R1: the platform really holds it, and we can read it back. ---
    let record = message_at(live, &receipt.id)
        .await
        .ok_or_else(|| format!("receive leg: {} is not readable from history", receipt.id))?;
    need!(
        text_of(&record) == body,
        "history returned {:?} for our own send, expected {:?}",
        text_of(&record),
        body
    );

    // --- R2: the adapter's real inbound path over that real record. ---
    let raw = events_api_envelope(&record, &live.channel_id);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the epoch")
        .as_secs()
        .to_string();
    let sig = slack_v0_signature(&live.signing_secret, &ts, &raw);

    let request = |signature: &str| WebhookRequest {
        method: "POST".to_string(),
        full_url: "https://example.invalid/channels/slack/wl-live-slack/webhook".to_string(),
        headers: vec![
            ("x-slack-signature".to_string(), signature.to_string()),
            ("x-slack-request-timestamp".to_string(), ts.clone()),
            ("content-type".to_string(), "application/json".to_string()),
        ],
        query: Vec::new(),
        body: raw.clone(),
    };

    let response = ch.ingest_webhook(&request(&sig)).await.map_err(|e| {
        format!(
            "ingest_webhook rejected a correctly signed payload: {}",
            rendered(&e)
        )
    })?;
    need!(
        response.status == 200,
        "webhook returned status {}",
        response.status
    );

    let events = ch
        .poll_events()
        .await
        .map_err(|e| format!("poll_events failed: {}", rendered(&e)))?;
    let received: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ChannelEvent::MessageReceived { msg } => Some(msg),
            _ => None,
        })
        .collect();
    need!(
        received.len() == 1,
        "expected exactly one MessageReceived from the signed webhook, got {} (all events: {:?})",
        received.len(),
        events
    );
    let msg = received[0];
    need!(
        msg.text == body,
        "the received text is {:?}, expected {:?}",
        msg.text,
        body
    );
    need!(
        msg.id == receipt.id,
        "the received id is {:?}, expected the real ts {:?}",
        msg.id,
        receipt.id
    );
    need!(
        msg.conversation_id == live.channel_id,
        "the received conversation is {:?}, expected {:?}",
        msg.conversation_id,
        live.channel_id
    );

    // Failing direction: corrupt one byte of the signature. It must be
    // rejected, AND nothing must land in the inbox — a verifier that rejected
    // the response while still enqueueing the event would be worse than one
    // that accepted it.
    let mut bad: Vec<char> = sig.chars().collect();
    let last = bad.len() - 1;
    bad[last] = if bad[last] == 'a' { 'b' } else { 'a' };
    let bad: String = bad.into_iter().collect();
    need!(bad != sig, "failed to actually corrupt the signature");

    let neg = ch.ingest_webhook(&request(&bad)).await;
    need!(
        neg.is_err(),
        "a payload with a corrupted signature was ACCEPTED — signature verification is not \
         running, and every positive result in this leg is therefore meaningless"
    );
    let leaked = ch.poll_events().await.map_err(|e| {
        format!(
            "poll_events after the bad signature failed: {}",
            rendered(&e)
        )
    })?;
    need!(
        !leaked
            .iter()
            .any(|e| matches!(e, ChannelEvent::MessageReceived { .. })),
        "a rejected webhook still enqueued a message: {leaked:?}"
    );

    Ok(format!(
        "read ts={} back from conversations.history; the real record, independently signed and \
         replayed through ingest_webhook, surfaced as MessageReceived; a corrupted signature was \
         rejected and enqueued nothing",
        receipt.id
    ))
}

// ---------------------------------------------------------------------------
// Leg 5 — idempotency
// ---------------------------------------------------------------------------

/// Does a replayed idempotency key produce one message at the destination, or
/// two — and does that match what the adapter tells the delivery spine?
///
/// [`Channel::supports_outbound_idempotency`] is not a preference. The gateway
/// reads it in `LedgeredHandler::dispatch_fire` to decide whether an
/// `Attempted`, outcome-unknown delivery may be **re-sent** on restart. An
/// adapter that answers `true` at a destination which ignores the key turns
/// every such restart into a duplicate, and the duplicate is invisible from our
/// side because our own ledger records one delivery.
///
/// So this leg asserts the declaration against the platform rather than
/// asserting a constant. Both directions are live states of one assertion:
/// claim a guarantee Slack does not honour and it reddens; keep claiming its
/// absence after Slack starts honouring it and it reddens too.
async fn leg_idempotency(live: &Live, ch: &mut Box<dyn Channel>) -> LegResult {
    let declared = ch.supports_outbound_idempotency();
    let body = live.marker("idempotency");
    let key = format!("wl-live-{}-replayed", live.run_tag);

    let first = ch
        .send_message_idempotent(OutgoingMessage::text(&live.channel_id, &body), &key)
        .await
        .map_err(|e| format!("first keyed send failed: {}", rendered(&e)))?;
    let replay = ch
        .send_message_idempotent(OutgoingMessage::text(&live.channel_id, &body), &key)
        .await
        .map_err(|e| format!("replayed keyed send failed: {}", rendered(&e)))?;

    let after_replay = arrivals(live, &body).await.len();
    let expected = if declared { 1 } else { 2 };
    need!(
        after_replay == expected,
        "the adapter declares supports_outbound_idempotency() == {declared}, which means a \
         replayed key must produce {expected} message(s) at the destination. It produced \
         {after_replay}. First send ts={}, replay ts={}. If {after_replay} is the truth, the \
         declaration is wrong and the gateway will duplicate every outcome-unknown Slack delivery \
         it retries.",
        first.id,
        replay.id
    );

    // The other direction, and the control that proves the counter can move: a
    // genuinely different key is not a replay, so it must add one arrival. If
    // the count were stuck (a broken finder, a cached history response), this
    // reddens.
    let other_key = format!("wl-live-{}-distinct", live.run_tag);
    let third = ch
        .send_message_idempotent(OutgoingMessage::text(&live.channel_id, &body), &other_key)
        .await
        .map_err(|e| format!("distinct-key send failed: {}", rendered(&e)))?;
    let after_distinct = arrivals(live, &body).await.len();
    need!(
        after_distinct == after_replay + 1,
        "a DIFFERENT key must always add a message: count went {after_replay} -> \
         {after_distinct} (third send ts={})",
        third.id
    );

    Ok(format!(
        "declared={declared}; replayed key -> {after_replay} arrival(s) (ts {} then {}); a \
         distinct key -> {after_distinct}",
        first.id, replay.id
    ))
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

/// Delete every message this run created, then prove the channel holds none of
/// ours — including strays from any earlier run of this file.
///
/// Returns the surviving non-marker messages so the caller can report what was
/// left behind. `channel_join` records are not deletable and are not ours.
async fn sweep(live: &Live, ch: &mut Box<dyn Channel>) -> Result<Vec<String>, String> {
    for m in history(live).await {
        if text_of(&m).contains(&live.run_tag) {
            let ts = ts_of(&m).to_string();
            ch.delete_message(&live.channel_id, &ts)
                .await
                .map_err(|e| format!("sweep could not delete {ts}: {}", rendered(&e)))?;
        }
    }

    let remaining = history(live).await;
    let ours: Vec<String> = remaining
        .iter()
        .filter(|m| text_of(m).contains(&live.run_tag))
        .map(|m| ts_of(m).to_string())
        .collect();
    if !ours.is_empty() {
        return Err(format!(
            "the sweep left {} of this run's messages in the channel: {ours:?}",
            ours.len()
        ));
    }

    // Strays from an earlier interrupted run are not a failure of THIS run, but
    // they are left in a live workspace, so they are reported.
    Ok(remaining
        .iter()
        .filter(|m| text_of(m).contains(MARKER_PREFIX))
        .map(|m| format!("{} {:?}", ts_of(m), text_of(m)))
        .collect())
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "live: posts to and deletes from a real Slack workspace; run with --ignored"]
async fn five_message_actions_against_the_real_slack_api() {
    let live = live_env();
    let mut ch = started_adapter(&live).await;

    // Scope probe. Written first as a skip-gate, because at dispatch time the
    // token held only `channels:*` and the private-channel reads were refused.
    // The grant landed mid-run, so it is an ASSERT-gate: a skip is not a pass
    // (LANE-BRIEF §3b-iii), and with the scopes present a missing one is now a
    // real regression rather than an expected absence. It still names exactly
    // which scope is missing, which is the whole value of the probe.
    let identity = slack_get(&live, "auth.test", &[]).await;
    assert_eq!(
        identity.get("ok").and_then(|b| b.as_bool()),
        Some(true),
        "auth.test failed: {:?}",
        identity.get("error")
    );
    let granted = granted_scopes(&live).await;
    let missing: Vec<&str> = REQUIRED_SCOPES
        .iter()
        .filter(|(s, _)| !granted.iter().any(|g| g == s))
        .map(|(s, why)| {
            eprintln!("MISSING SCOPE {s} — needed for {why}");
            *s
        })
        .collect();
    assert!(
        missing.is_empty(),
        "the token is missing {missing:?}. Granted: {granted:?}. These legs cannot run and MUST \
         NOT be reported as passing."
    );

    let legs: Vec<(&str, LegResult)> = vec![
        ("send", leg_send(&live, &mut ch).await),
        ("edit", leg_edit(&live, &mut ch).await),
        ("delete", leg_delete(&live, &mut ch).await),
        ("receive", leg_receive(&live, &mut ch).await),
        ("idempotency", leg_idempotency(&live, &mut ch).await),
    ];

    // Unconditional, whatever the legs did.
    let sweep_outcome = sweep(&live, &mut ch).await;

    println!("\n=== live slack action matrix (run {}) ===", live.run_tag);
    for (name, outcome) in &legs {
        match outcome {
            Ok(evidence) => println!("  PASS  {name:<12} {evidence}"),
            Err(why) => println!("  FAIL  {name:<12} {why}"),
        }
    }
    match &sweep_outcome {
        Ok(strays) if strays.is_empty() => println!("  clean channel: no marker messages remain"),
        Ok(strays) => println!(
            "  channel holds {} stray marker(s): {strays:?}",
            strays.len()
        ),
        Err(why) => println!("  SWEEP FAILED: {why}"),
    }
    let passed = legs.iter().filter(|(_, o)| o.is_ok()).count();
    println!("  {passed}/{} legs passed\n", legs.len());

    let failures: Vec<String> = legs
        .iter()
        .filter_map(|(n, o)| o.as_ref().err().map(|e| format!("{n}: {e}")))
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} live legs failed:\n  {}",
        failures.len(),
        legs.len(),
        failures.join("\n  ")
    );
    sweep_outcome.expect("the channel must be left clean");
}

/// The scopes the token actually holds, from `auth.test`'s `x-oauth-scopes`
/// response header.
///
/// Read from the header rather than from a config file or this lane's brief:
/// the grant changed *while this lane was running*, so any cached answer would
/// have been stale within the hour.
async fn granted_scopes(live: &Live) -> Vec<String> {
    let http = wcore_egress::EgressClient::new();
    let resp = http
        .post("https://slack.com/api/auth.test")
        .bearer_auth(&live.bot_token)
        .send()
        .await
        .expect("auth.test transport");
    resp.headers()
        .get("x-oauth-scopes")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
