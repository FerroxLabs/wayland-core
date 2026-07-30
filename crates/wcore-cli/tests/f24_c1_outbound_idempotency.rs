//! F24-C1 — outbound idempotency, measured across the adapter matrix.
//!
//! # Why this file exists
//!
//! Phase 24 Success Criterion 1 claims a no-duplicate delivery guarantee. Its
//! 12-of-12 no-duplicate tally was taken **entirely through the Slack adapter**
//! (`scripts/f24-journey.mjs:380` sets `platform = "slack"` and is the only
//! `platform =` line in that driver), and when this file was written Slack was
//! the only adapter in the workspace overriding
//! `Channel::supports_outbound_idempotency` (trait default at
//! `wcore-channels/src/lib.rs:139` is `false`).
//!
//! **That is no longer true and this header is corrected rather than left to
//! rot (2026-07-30, `lane/24c1-declaration`).** Two adapters override it to
//! `true`: Slack (`wcore-channel-slack/src/lib.rs:249`) and Matrix
//! (`wcore-channel-matrix/src/lib.rs:294`). Eight are `false`.
//!
//! **Discord was the third until later the same day** (`lane/discord-live`).
//! It overrides the method too, but to `false`: driven at real Discord, an
//! identical `nonce` replayed after a genuine gateway restart produced a
//! SECOND message. The token is accepted and echoed and simply not honoured.
//! See `docs/delivery-semantics.md` §8. Discord is the reason this file's
//! own header warns that a four-adapter subset is not a census: it carried a
//! false `true` for months and nothing here could have caught it.
//!
//! **This file measures a FOUR-adapter subset** (Slack, Telegram, Twilio SMS,
//! WhatsApp) — the four that can be driven over real HTTP at a local fixture.
//! It is deliberately not a census, and it would still pass if a fifth adapter
//! silently changed its capability. The all-ten census lives in
//! `wcore-channels-registry/tests/delivery_semantics_declaration.rs`, which
//! binds every adapter to the published table in `docs/delivery-semantics.md`.
//!
//! The pre-existing planning record already states that the non-overriding
//! adapters have their outcome-unknown deliveries *abandoned* rather than
//! duplicated. What nothing in the phase had ever done is **measure** it on any
//! adapter other than Slack. Reasoning from the trait is not measurement: a
//! `false` could be a truthful capability declaration OR an unimplemented stub
//! on an adapter whose destination would in fact have deduplicated anyway, and
//! those two worlds call for opposite fixes.
//!
//! So this file measures, over real adapters built through the **production
//! factory** (`wcore_channels_registry::auto_register_from_dir`, the same path
//! a real deploy uses) and driven over real HTTP at a local fixture:
//!
//! 1. `capability_*` — what each adapter actually declares once constructed.
//! 2. `a_replayed_delivery_key_produces_two_messages_*` — the decisive one.
//!    Replay the SAME idempotency key through the real adapter and count what
//!    lands at the destination. Two arrivals carrying no dedupe token proves
//!    the `false` is TRUTHFUL and the spine's abandon is preventing a genuine
//!    duplicate.
//! 3. `slack_*` — the known-positive. The same replay through Slack must put
//!    the key on the wire both times, or the `true` is a lie.
//! 4. `the_bool_the_delivery_spine_reads_*` — the value is measured through the
//!    real `EngineJobHandler` + real `ChannelManager` composition, which is
//!    what `wcore-gateway`'s `LedgeredHandler` consults at
//!    `automation.rs:141`, rather than being asserted from source.
//!
//! # Anti-self-passing discipline (LANE-BRIEF §3.2)
//!
//! Every "no duplicate" style result in this program is worthless without a
//! positive control, because **a green can be manufactured by universal
//! denial**: if nothing sends at all, no duplicates appear. So each replay test
//! asserts the FIRST send returned a real, parsed receipt from the fixture
//! BEFORE it looks at any count, and labels that assertion INSTRUMENT_FAULT —
//! a run that trips it is INCOMPLETE, never a delivery result.
//!
//! The arrival count itself is asserted with mockito's `.expect(n)` +
//! `assert_async()`, which reports the ACTUAL hit count on failure. The
//! discriminator for "did a dedupe token ride the wire" is structural rather
//! than a log grep: the no-key mocks match only requests where the
//! `Idempotency-Key` header is `Matcher::Missing`, so an adapter that started
//! sending one would stop matching, fall through to mockito's unmatched-request
//! 501, and redden this file. It cannot silently pass.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use mockito::Matcher;
use tokio::sync::RwLock;
// `wcore-cli` deliberately has NO direct dependency edge to `wcore-channels`
// (see its Cargo.toml: the registry is the single seam, so the CLI does not
// grow a second edge). The registry re-exports the runtime for exactly this
// reason — `pub use wcore_channels;` in wcore-channels-registry/src/lib.rs:40 —
// so reaching the types through it keeps this test manifest-free.
use wcore_channels_registry::wcore_channels::{ChannelManager, OutgoingMessage};
use wcore_config::credentials::{CredentialsError, CredentialsStore};
use wcore_cron::job::Target;
use wcore_cron::runner::JobHandler;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// In-memory credentials so no test touches the real keyring and no secret is
/// ever written to disk.
struct MemCreds {
    inner: StdMutex<HashMap<String, String>>,
}

impl MemCreds {
    fn new(entries: &[(&str, &str)]) -> Arc<Self> {
        let mut m = HashMap::new();
        for (k, v) in entries {
            m.insert((*k).to_string(), (*v).to_string());
        }
        Arc::new(Self {
            inner: StdMutex::new(m),
        })
    }
}

impl CredentialsStore for MemCreds {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        Ok(self.inner.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        self.inner
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        self.inner.lock().unwrap().remove(key);
        Ok(())
    }
}

/// Every credential handle any adapter under test resolves at `start()`.
fn all_creds() -> Arc<dyn CredentialsStore> {
    MemCreds::new(&[
        ("slack.f24c1.bot_token", "xoxb-f24c1-not-a-real-token"),
        ("slack.f24c1.signing_secret", "f24c1-signing"),
        ("telegram.f24c1.bot_token", TELEGRAM_TOKEN),
        ("sms.f24c1.account_sid", TWILIO_SID),
        ("sms.f24c1.auth_token", "f24c1-twilio-auth"),
        ("whatsapp.f24c1.access_token", "f24c1-wa-access"),
        ("whatsapp.f24c1.app_secret", "f24c1-wa-secret"),
    ])
}

const TELEGRAM_TOKEN: &str = "111:AAAA-f24c1-bot-token";
const TWILIO_SID: &str = "ACf24c100000000000000000000000000";
const WA_PHONE_ID: &str = "10987654321";

/// The stable delivery identity a ledger replay reuses. Deliberately shaped
/// like a real one (`cron:<job>:<instant-ms>`), because that is what
/// `FireContext::delivery_id()` produces.
const REPLAY_KEY: &str = "cron:f24c1-job:1785121776528";

/// Build a `ChannelManager` from real on-disk TOML through the PRODUCTION
/// factory, then start it. Returns the manager with every adapter connected.
///
/// Going through `auto_register_from_dir` rather than calling the adapter
/// constructors directly is deliberate: it is the path a real deploy takes, it
/// exercises each adapter's `#[serde(deny_unknown_fields)]` config schema, and
/// it means a typo in a fixture config fails loudly here instead of silently
/// producing an adapter pointed at the production API.
async fn manager_from_configs(files: &[(&str, String)]) -> ChannelManager {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, body) in files {
        std::fs::write(dir.path().join(format!("{name}.toml")), body).expect("write config");
    }
    let mut mgr = ChannelManager::new();
    let registered =
        wcore_channels_registry::auto_register_from_dir(&mut mgr, dir.path(), all_creds())
            .await
            .expect("auto_register_from_dir");
    assert_eq!(
        registered,
        files.len(),
        "INSTRUMENT_FAULT: the production factory did not build every fixture \
         config, so any capability or arrival number below would be measuring \
         fewer adapters than it claims. Registered {registered} of {}.",
        files.len()
    );
    mgr.start_all().await.expect("start_all");
    // Keep the tempdir alive for the life of the process; the adapters have
    // already parsed their configs, so leaking the handle is harmless and
    // avoids a drop-order footgun.
    std::mem::forget(dir);
    mgr
}

fn slack_cfg(base: &str) -> String {
    format!(
        r#"
name = "f24c1slack"
platform = "slack"
enabled = true

[options]
workspace_name = "f24c1"
default_channel_id = "C0F24C1"
credential_handle_bot_token = "slack.f24c1.bot_token"
credential_handle_signing_secret = "slack.f24c1.signing_secret"
api_base_url = "{base}"
max_retry_attempts = 1
"#
    )
}

fn telegram_cfg(base: &str) -> String {
    format!(
        r#"
name = "f24c1tg"
platform = "telegram"
enabled = true

[options]
credential_handle = "telegram.f24c1.bot_token"
parse_mode = "HTML"
api_base_url = "{base}"
"#
    )
}

fn sms_cfg(base: &str) -> String {
    format!(
        r#"
name = "f24c1sms"
platform = "sms"
enabled = true

[options]
from_number = "+15550000000"
credential_handle_account_sid = "sms.f24c1.account_sid"
credential_handle_auth_token = "sms.f24c1.auth_token"
api_base_url = "{base}"
max_retry_attempts = 1
"#
    )
}

fn whatsapp_cfg(base: &str) -> String {
    format!(
        r#"
name = "f24c1wa"
platform = "whatsapp"
enabled = true

[options]
workspace_name = "f24c1"
phone_number_id = "{WA_PHONE_ID}"
default_recipient = "+15551234567"
credential_handle_access_token = "whatsapp.f24c1.access_token"
credential_handle_app_secret = "whatsapp.f24c1.app_secret"
api_base_url = "{base}"
graph_version = "v18.0"
max_retry_attempts = 1
"#
    )
}

/// Telegram's `start()` POSTs `deleteWebhook` and then spawns a long-poll task
/// against `getUpdates`. Neither is under test here, but both must be answered
/// or `start()` fails for a reason that has nothing to do with idempotency.
async fn telegram_background_mocks(server: &mut mockito::ServerGuard) {
    server
        .mock(
            "POST",
            format!("/bot{TELEGRAM_TOKEN}/deleteWebhook").as_str(),
        )
        .with_status(200)
        .with_body(r#"{"ok":true,"result":true}"#)
        .expect_at_least(0)
        .create_async()
        .await;
    server
        .mock("POST", format!("/bot{TELEGRAM_TOKEN}/getUpdates").as_str())
        .with_status(200)
        .with_body(r#"{"ok":true,"result":[]}"#)
        .expect_at_least(0)
        .create_async()
        .await;
    server
        .mock("GET", format!("/bot{TELEGRAM_TOKEN}/getUpdates").as_str())
        .with_status(200)
        .with_body(r#"{"ok":true,"result":[]}"#)
        .expect_at_least(0)
        .create_async()
        .await;
}

// ---------------------------------------------------------------------------
// 1. The decisive measurement: is the `false` truthful?
//
// Replay one delivery key twice through a real adapter and count arrivals at
// the destination. Each of these three adapters speaks a DIFFERENT protocol
// (Bot API token-in-path JSON / Twilio form-encoded + Basic auth / Meta Graph
// versioned-path JSON + Bearer), so a shared accident is unlikely.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_delivery_key_produces_two_messages_on_telegram() {
    let mut server = mockito::Server::new_async().await;
    telegram_background_mocks(&mut server).await;

    // Matching on `Idempotency-Key: Missing` is the structural discriminator:
    // if the adapter ever starts transmitting a dedupe token this mock stops
    // matching, the request falls through to mockito's 501, and the send below
    // fails. The test cannot pass while quietly changing meaning.
    let send = server
        .mock("POST", format!("/bot{TELEGRAM_TOKEN}/sendMessage").as_str())
        .match_header("Idempotency-Key", Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"ok":true,"result":{"message_id":7,"date":1700000000,"chat":{"id":42}}}"#)
        .expect(2)
        .create_async()
        .await;

    let mgr = manager_from_configs(&[("f24c1tg", telegram_cfg(&server.url()))]).await;

    let first = mgr
        .send_to_keyed(
            "f24c1tg",
            OutgoingMessage::text("42", "f24c1 telegram body"),
            Some(REPLAY_KEY),
        )
        .await;
    let first = first.expect(
        "INSTRUMENT_FAULT: the FIRST send did not reach the fixture and parse a \
         receipt. Nothing below this line is a delivery measurement — grade the \
         run INCOMPLETE, not a loss. A no-duplicate result taken after a failed \
         positive control is a green manufactured by universal denial.",
    );
    assert_eq!(
        first.id, "7",
        "positive control: the receipt must come from the fixture's real response body"
    );

    // The replay: byte-identical message, identical key.
    let second = mgr
        .send_to_keyed(
            "f24c1tg",
            OutgoingMessage::text("42", "f24c1 telegram body"),
            Some(REPLAY_KEY),
        )
        .await
        .expect("the replay itself must succeed — that is the point");
    assert_eq!(second.id, "7");

    // Reports the ACTUAL hit count in the panic message when it disagrees.
    send.assert_async().await;
}

#[tokio::test]
async fn a_replayed_delivery_key_produces_two_messages_on_sms() {
    let mut server = mockito::Server::new_async().await;
    let send = server
        .mock(
            "POST",
            format!("/2010-04-01/Accounts/{TWILIO_SID}/Messages.json").as_str(),
        )
        .match_header("Idempotency-Key", Matcher::Missing)
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"sid":"SMf24c1000000000000000000000000","status":"queued"}"#)
        .expect(2)
        .create_async()
        .await;

    let mgr = manager_from_configs(&[("f24c1sms", sms_cfg(&server.url()))]).await;

    let first = mgr
        .send_to_keyed(
            "f24c1sms",
            OutgoingMessage::text("+15551234567", "f24c1 sms body"),
            Some(REPLAY_KEY),
        )
        .await
        .expect(
            "INSTRUMENT_FAULT: first Twilio send never landed — run is INCOMPLETE, \
             not a loss measurement",
        );
    assert_eq!(first.id, "SMf24c1000000000000000000000000");

    mgr.send_to_keyed(
        "f24c1sms",
        OutgoingMessage::text("+15551234567", "f24c1 sms body"),
        Some(REPLAY_KEY),
    )
    .await
    .expect("the replay itself must succeed");

    send.assert_async().await;
}

#[tokio::test]
async fn a_replayed_delivery_key_produces_two_messages_on_whatsapp() {
    let mut server = mockito::Server::new_async().await;
    let send = server
        .mock("POST", format!("/v18.0/{WA_PHONE_ID}/messages").as_str())
        .match_header("Idempotency-Key", Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"messaging_product":"whatsapp","messages":[{"id":"wamid.F24C1"}]}"#)
        .expect(2)
        .create_async()
        .await;

    let mgr = manager_from_configs(&[("f24c1wa", whatsapp_cfg(&server.url()))]).await;

    let first = mgr
        .send_to_keyed(
            "f24c1wa",
            OutgoingMessage::text("+15551234567", "f24c1 whatsapp body"),
            Some(REPLAY_KEY),
        )
        .await
        .expect(
            "INSTRUMENT_FAULT: first WhatsApp send never landed — run is INCOMPLETE, \
             not a loss measurement",
        );
    assert_eq!(first.id, "wamid.F24C1");

    mgr.send_to_keyed(
        "f24c1wa",
        OutgoingMessage::text("+15551234567", "f24c1 whatsapp body"),
        Some(REPLAY_KEY),
    )
    .await
    .expect("the replay itself must succeed");

    send.assert_async().await;
}

// ---------------------------------------------------------------------------
// 2. The known-positive. Without this, the three tests above could equally be
//    explained by "the key never reaches any adapter", which would be an
//    instrument defect rather than a product fact.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn slack_is_the_known_positive_and_puts_the_same_key_on_the_wire_both_times() {
    let mut server = mockito::Server::new_async().await;
    let send = server
        .mock("POST", "/api/chat.postMessage")
        .match_header("Idempotency-Key", Matcher::Exact(REPLAY_KEY.to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"ok":true,"ts":"1700000000.000100","channel":"C0F24C1"}"#)
        .expect(2)
        .create_async()
        .await;

    let mgr = manager_from_configs(&[("f24c1slack", slack_cfg(&server.url()))]).await;

    let first = mgr
        .send_to_keyed(
            "f24c1slack",
            OutgoingMessage::text("C0F24C1", "f24c1 slack body"),
            Some(REPLAY_KEY),
        )
        .await
        .expect(
            "INSTRUMENT_FAULT: Slack's first send did not carry the key to the \
             fixture. If this trips, the three no-key results in this file are \
             NOT evidence about those adapters — they are evidence the key never \
             left the manager. Grade the whole run INCOMPLETE.",
        );
    assert_eq!(first.id, "1700000000.000100");

    mgr.send_to_keyed(
        "f24c1slack",
        OutgoingMessage::text("C0F24C1", "f24c1 slack body"),
        Some(REPLAY_KEY),
    )
    .await
    .expect("replay through the deduping adapter must also succeed");

    // Both requests matched a mock that REQUIRES the exact key header, so this
    // asserting 2 is simultaneously an arrival count and a wire assertion.
    send.assert_async().await;
}

// ---------------------------------------------------------------------------
// 3. Capability as the production composition reports it — not as source says.
// ---------------------------------------------------------------------------

#[tokio::test]
/// Renamed 2026-07-30: this asserted "by Slack ALONE" while checking four of
/// ten adapters, so it would have passed unchanged after Matrix and Discord
/// gained the capability — a name claiming a census the body does not perform.
/// The census is
/// `wcore-channels-registry::delivery_semantics_declaration::exactly_two_adapters_are_exactly_once`
/// (renamed from `exactly_three_` when Discord was removed on live evidence).
async fn slack_declares_the_capability_and_the_three_http_fixture_adapters_do_not() {
    let mut server = mockito::Server::new_async().await;
    telegram_background_mocks(&mut server).await;
    let base = server.url();

    let mgr = manager_from_configs(&[
        ("f24c1slack", slack_cfg(&base)),
        ("f24c1tg", telegram_cfg(&base)),
        ("f24c1sms", sms_cfg(&base)),
        ("f24c1wa", whatsapp_cfg(&base)),
    ])
    .await;

    assert!(
        mgr.supports_outbound_idempotency("f24c1slack").await,
        "Slack is the one adapter that transmits the key; if this goes false the \
         12-of-12 journey tally loses its only basis"
    );
    for name in ["f24c1tg", "f24c1sms", "f24c1wa"] {
        assert!(
            !mgr.supports_outbound_idempotency(name).await,
            "{name} unexpectedly declares outbound idempotency — if an adapter \
             gained the capability, this file's arrival counts must be re-derived"
        );
    }

    // An unresolvable destination must answer the conservative `false` rather
    // than erroring: the question is "is a retry safe here".
    assert!(!mgr.supports_outbound_idempotency("no-such-channel").await);
}

// ---------------------------------------------------------------------------
// 4. The exact boolean the delivery spine reads, measured through the real
//    EngineJobHandler + real ChannelManager composition.
//
//    `wcore-gateway`'s `LedgeredHandler::dispatch_fire` calls
//    `inner.dispatch_is_idempotent(target)` (automation.rs:141) and abandons an
//    outcome-unknown delivery when it is false. `inner` in production is this
//    `EngineJobHandler` (cron.rs:146-159). This test measures what that call
//    returns with real adapters behind it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_bool_the_delivery_spine_reads_is_false_for_every_adapter_but_slack() {
    let mut server = mockito::Server::new_async().await;
    telegram_background_mocks(&mut server).await;
    let base = server.url();

    let mgr = manager_from_configs(&[
        ("f24c1slack", slack_cfg(&base)),
        ("f24c1tg", telegram_cfg(&base)),
        ("f24c1sms", sms_cfg(&base)),
        ("f24c1wa", whatsapp_cfg(&base)),
    ])
    .await;

    let handler =
        wcore_agent::cron::EngineJobHandler::new(Some(Arc::new(RwLock::new(mgr))), None, None);

    let chan = |name: &str| Target::Channel {
        channel_name: name.to_string(),
        text: "f24c1".to_string(),
        conversation_id: None,
    };

    assert!(
        handler.dispatch_is_idempotent(&chan("f24c1slack")).await,
        "the spine must be told Slack can absorb a replay"
    );
    for name in ["f24c1tg", "f24c1sms", "f24c1wa"] {
        assert!(
            !handler.dispatch_is_idempotent(&chan(name)).await,
            "the spine is told {name} cannot absorb a replay — this is the value \
             that makes an outcome-unknown delivery ABANDONED at automation.rs:176"
        );
    }

    // A non-delivery target is not a delivery at all; the conservative answer
    // is the correct one and must not drift to true.
    assert!(
        !handler
            .dispatch_is_idempotent(&Target::Slash {
                command: "/noop".to_string()
            })
            .await
    );
}
