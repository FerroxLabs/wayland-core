# NOTES — lane `twilio-whatsapp-identity`

Branch `lane/twilio-whatsapp-identity`, based on integration `4caaa31c`.
Append-and-recommit after every measurement (LANE-BRIEF §6b-i).

---

## Minute 0-15 — premise verification, before any edit

### P1. Twilio and WhatsApp adapters DO inherit the pass-through `send_message_idempotent`

Measured, unproxied grep over the four adapters that could plausibly override it:

```
/usr/bin/grep -n "send_message_idempotent|supports_outbound_idempotency|idempotency" \
  crates/wcore-channel-{slack,discord,sms,whatsapp}/src/lib.rs
```

25 hits. **All 25 are in slack/lib.rs or discord/lib.rs.** Zero in
`wcore-channel-sms/src/lib.rs`, zero in `wcore-channel-whatsapp/src/lib.rs`.
Known-positive in the same invocation: slack:234 `async fn send_message_idempotent`
and discord:325 the same — so the instrument was alive and the needle correct.

=> Both inherit `wcore-channels/src/lib.rs:123-129` pass-through (`_key` discarded)
and the trait default `supports_outbound_idempotency() -> false` at `:139`.
**The delivery key never reaches the wire for either.** Brief's premise HOLDS.

### P2. The sink hardcodes `idempotency_key: null` for both endpoints

`scripts/f24-sink.mjs`:
- `:132` — `chat.postMessage` reads `req.headers['idempotency-key']`.
- `:176` — `whatsapp.messages` calls `record(..., /*key*/ null, false)`.
- `:193` — `twilio.messages` calls `record(..., /*key*/ null, false)`.

=> **This is a SECOND, independent cause of `unidentified`.** Even if the adapter
put a key on the wire tomorrow, the sink would still journal `null` and the gate
would still read NOT-PROVEN. Any fix that touches only the adapter is untestable
through the journey gate. The brief did not mention this half.

### P3. The journey gate's identity comes from the SINK journal, not our ledger

`scripts/f24-journey.mjs:1235` reads `a.idempotency_key` off arrivals; `journey.rs`
`DeliveryIdentity.unidentified` counts arrivals with no identity.

=> **A platform-returned id (`sid` / `wamid.*`) recorded in OUR journal does NOT
reduce `unidentified`.** The sink is the destination; it mints those ids itself.
The brief's proposed mechanism ("record the platform-returned id against the
delivery id") is a real improvement to *our* reconciliation but is **not** the
thing the journey gate measures. Flagged as a premise to argue with, see SUMMARY.

### P4. `docs/delivery-semantics.md` already says Twilio/WhatsApp are at-most-once

Rows already read `at-most-once` / `abandoned` and the evidence column already
claims "**Yes** — a replayed key produced **two** messages". That claim needs
scrutiny: we hold no Twilio or Meta credentials, so where did a *real-destination*
replay measurement come from? Open question O1.

---

## Open questions — all three now answered

### O1 CLOSED — the doc's Twilio/WhatsApp evidence cells were FALSE

Both rows read *"Replay measured at a real destination? **Yes** — a replayed key
produced **two** messages."* The run behind them is
`crates/wcore-cli/tests/f24_c1_outbound_idempotency.rs`, which is **mockito**.

Absence measured with a control in the same invocation (LANE-BRIEF §3b-i):

```
/usr/bin/find crates -name "*.rs" | /usr/bin/xargs /usr/bin/grep -ln \
  "TWILIO_ACCOUNT_SID\|WHATSAPP_ACCESS_TOKEN\|WA_ACCESS_TOKEN\|live_twilio\|live_whatsapp"
  -> 0 files
control, same shape:
/usr/bin/find crates -name "*.rs" | /usr/bin/xargs /usr/bin/grep -ln "SLACK_BOT_TOKEN"
  -> crates/wcore-channels-registry/tests/live_slack_actions.rs   (1 file)
```

Instrument alive, needle correct, absence real. This is the SAME defect the doc's
own Slack correction diagnoses (*"the evidence column must state the same claim as
the guarantee column"*) sitting in the two rows directly beneath it. Corrected to
`NOT MEASURED at a real destination`. Guarantees unchanged — `at-most-once` never
depended on the fixture. Telegram's row carries the same overstatement and is left
standing with a note; not this lane's row to edit.

### O2 CLOSED — the two platforms are ASYMMETRIC

| | carrier | source |
|---|---|---|
| WhatsApp Cloud API | **`biz_opaque_callback_data`** — optional tracking string, ≤512 chars, echoed back in the `statuses` object of the `messages` webhook | Cloud API v25 coverage doc + typed client (`/ericvera/whatsapp-cloudapi`, High reputation) |
| Twilio `Messages` create | **nothing.** Full optional-parameter list is `StatusCallback`, `ApplicationSid`, `MaxPrice`, `ProvideFeedback`, `Attempt`, `ValidityPeriod`, `ForceDelivery`, `ContentRetention`, `AddressRetention`, `SmartEncoded`, `PersistentAction`, `TrafficType`, `ShortenUrls`, `ScheduleType`, `SendAt`, `SendAsMms`, `ContentVariables`, `RiskCheck` — not one is a dedup or opaque slot | `twilio-go` `CreateMessageParams` reference |

So WhatsApp gets a real production-useful carrier and Twilio gets an inert
`Idempotency-Key` header, mirroring Slack. The only other Twilio echo channel is
query params on a `StatusCallback` URL, and the SMS crate has **zero** references
to `StatusCallback` — it would need config plumbing and a public URL. Out of scope,
recorded.

### O3 CLOSED — Desktop does NOT solve this. It only sends.

Read-only inspection of `/Users/seandonahoe/dev/wayland`, nothing mutated, nothing
executed.

- **Twilio** (`SmsTwilioPlugin.ts`): `client.messages.create({to, body, from |
  messagingServiceSid, mediaUrl})` via the official npm SDK, returns `result.sid`
  upward. **No idempotency token, no opaque field, no persistence of the sid
  against any delivery id.** `sendWithRetry` retries 429/5xx up to
  `TWILIO_MAX_RETRIES` **with no token at all** — so a 5xx arriving after Twilio
  accepted the message produces a duplicate. That is strictly weaker than Core,
  which abandons rather than blind-retries.
- **WhatsApp** (`WhatsAppPlugin.ts` + `backends/meta-business.js`): posts
  `{messaging_product, to, type, text}` and reads `res.messages[0].id`.
  `rememberSentId(id)` is **echo suppression** — ignore our own message coming
  back through `messages.upsert` — not delivery identity.
- `biz_opaque_callback_data` outside `node_modules`: **0 hits**; control
  `messaging_product`: **3 hits**. Desktop does not use the field.
- Outbound delivery ledger: `deliveryId` 0 files, `delivery_id` 0 files,
  `DeliveryLedger` 0 files, `outboundLedger` 0 files; control
  `IUnifiedOutgoingMessage` 55 files. `exactly-once` appears in 27 files, 24 of
  which are shipped `skills-library` markdown and the rest chat send-on-wake /
  workflow session — **none** outbound-channel delivery.
- The only "idempot" hits in `app/src/process/channels/` outside `node_modules`
  are `connection-tokens.ts`, `PairingService.ts`, `EmailImapConnection.ts`,
  `WeixinTyping.ts`, `GoogleChatPubSub.ts`, `tunnel/types.ts`,
  `gateway/PluginManager.ts`. Neither tier1 plugin is among them.

**Verdict: nothing to port.** "Desktop has them working" is true and means
*messages arrive*, which was already true of Core. Nothing was taken from Desktop,
so no `THIRD-PARTY-NOTICES.md` entry is owed and the word "ported" does not appear
anywhere in this lane's output.

---

## What was built

1. `wcore-channel-sms`: `send_message_idempotent` override; delivery id on the
   `Idempotency-Key` header (`api::IDEMPOTENCY_HEADER`), re-attached inside the
   retry loop; explicit `supports_outbound_idempotency() -> false`.
2. `wcore-channel-whatsapp`: same shape; delivery id in
   `biz_opaque_callback_data`, clamped to 512 chars on a char boundary; shared by
   every part of a multi-attachment send (one logical delivery). Cloud-API-only
   caveat named in code for the incoming `whatsapp-bridge` lane.
3. `scripts/f24-sink.mjs`: `readKey()` reads header-or-body; both endpoints now
   journal the identity instead of `null`.
4. `crates/wcore-channels-registry/tests/identity_at_the_sink.rs` +
   `scripts/f24-identity-arms.mjs`: the three-arm differential, grading with the
   journey's OWN `classifyRepeats`.
5. `crates/wcore-channels-registry/tests/live_twilio_whatsapp_identity.rs`:
   Phase B, credential-gated, panics rather than skipping, with a NON-ignored
   census test so the unrun cells print on every ordinary `cargo test`.
6. `docs/delivery-semantics.md`: rows corrected; a new subsection separating
   attribution from deduplication.

## Gate results (hetzner, worktree `/root/wayland-twilio-whatsapp-identity`)

- `cargo test -p wcore-channel-sms -p wcore-channel-whatsapp` @ `13571bf1`:
  sms **28 passed, 0 failed, 0 ignored, 0 filtered out**;
  whatsapp **42 passed, 0 failed, 0 ignored, 0 filtered out**.
- `cargo test -p wcore-cli --test f24_c1_outbound_idempotency` @ `13571bf1`:
  **6 passed, 0 failed, 0 ignored, 0 filtered out**.
- `cargo fmt --all -- --check` on the Mac: rc=0, 0 lines.
