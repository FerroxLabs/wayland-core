# Delivery semantics, per channel adapter

**What guarantee you get when the gateway sends a message on your behalf, and what to expect
when it restarts.**

This document is a *description of what the code does today*, not an aspiration. Every cell
below traces to a source line or to a measurement, both cited. Where the honest answer is
"no guarantee", it says so.

It is enforced. `crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs`
constructs all ten adapters through the production factory and fails the build if any row in
the table below disagrees with the adapter's actual capability. See
[Keeping this document true](#keeping-this-document-true).

---

## 1. The short version

| | |
|---|---|
| **1 of 10** adapters | exactly-once — Matrix, and it is the only one ever proven at the real platform. **Conditional: only for a body that fits in one platform message** — see [§4.1](#41-exactly-once-stops-at-the-message-cap) |
| **9 of 10** adapters | at-most-once — a delivery whose outcome is unknown is **abandoned, not retried** |
| **0 of 10** adapters | at-least-once (the gateway never automatically re-sends to a destination that cannot recognise a replay) |
| **On every platform** | a **recurring** job that outlives its trigger period sends again, under a new delivery id. Not a duplicate, and not Windows-specific — see [§5](#5-a-recurring-job-delivers-again-and-that-is-not-a-duplicate) |

**On 2026-07-30 this table lost two of its three exactly-once rows.** Slack and Discord were
each driven at their real API for the first time, and each produced **two** messages from a
replayed key. Both had held the claim on the strength of a mock. A mock can only prove that we
put a token on the wire; it says nothing about whether the destination honours it, and for both
of these it did not. Matrix is the only row that was driven live before it was believed, and it
is the only one still standing.

Nothing is ever silently dropped. An abandoned delivery is recorded, listed by
`wayland-core gateway abandoned`, and re-sendable by an operator.

---

## 2. The table

"Guarantee" is per **delivery id** — read [§4](#4-what-the-guarantee-is-scoped-to) before
relying on it, because that scope is narrower than "one message".

| Adapter | Platform primitive | Guarantee | Outcome-unknown delivery is… | On restart, expect | Replay measured at a real destination? |
|---|---|---|---|---|---|
| **Slack** | none that Slack honours — the adapter sends an `Idempotency-Key` header and **`slack.com` ignores it** | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Slack | **Yes** — a replayed key produced **two** messages; see the correction note below |
| **Matrix** | `PUT …/send/m.room.message/{txnId}` — the txn id *is* the idempotency slot | **exactly-once, up to 16,384 chars; at-least-once above it** — see [§4.1](#41-exactly-once-stops-at-the-message-cap) | **retried** with the same key, below the cap | one message below the cap; the homeserver returns the original `event_id` | **BELOW the cap: Yes** — by the PRODUCT, against matrix.org, across a real `kill -9`; see [§9](#9-the-matrix-row-driven-end-to-end-2026-07-30). **ABOVE the cap: NOT MEASURED at a real destination** — the harness exists and has never completed a run; see [§4.1](#41-exactly-once-stops-at-the-message-cap) |
| **Discord** | `nonce` field on message create — **transmitted, but Discord does not dedupe on it** | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Discord | **Yes** — a replayed key produced **two** messages; see [§8](#8-discord-was-wrong-and-how-it-was-found) |
| **Telegram** | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Telegram | **NOT MEASURED at a real destination** — see the correction below |
| **Twilio SMS** | none that Twilio honours — the adapter sends an `Idempotency-Key` header and the `Messages` resource documents no dedup slot to read it | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Twilio | **NOT MEASURED at a real destination** — see the correction below |
| **WhatsApp** (Meta Graph) | none that Meta honours — the adapter sends the delivery id as `biz_opaque_callback_data`, which the Cloud API documents as *tracking* data, not a dedup slot | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Meta | **NOT MEASURED at a real destination** — see the correction below |
| **Email** (SMTP) | none that any MTA guarantees | **at-most-once** | **abandoned** | zero or one message — unknowable without checking the mailbox | **NOT MEASURED** |
| **Signal** (`signal-cli`) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Signal | **NOT MEASURED** |
| **iMessage** (AppleScript) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Messages.app. **macOS only** — on Linux and Windows the adapter is not compiled in and cannot be constructed at all | **NOT MEASURED** |
| **MS Teams** (Bot Framework) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Teams | **NOT MEASURED** |
| **WhatsApp bridge** (`backend = "baileys"` / `"whatsapp-web"`) | none — the bridge's `sendText` RPC carries no key and neither backend accepts one | **at-most-once** | **abandoned** | zero or one message — unknowable without checking WhatsApp | **NOT MEASURED, and no replay has been driven at all** — see the note below |

**The WhatsApp bridge row is COVERED as of 2026-08-29, and is still weaker than the rest.** It
was uncovered until then, and structurally so: the other ten rows describe adapters the registry
constructs from a platform string alone, while the bridge is reached through the *same* platform
string (`whatsapp`) with an opt-in `backend` key, so
`crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs` could not see it. That
harness now walks `wcore_channels_registry::constructible_selectors()` and the row is keyed
`whatsapp+baileys` / `whatsapp+whatsapp-web` (wayland-core#360). What the coverage buys is that
the row cannot silently disagree with the adapter any more. What it does NOT buy is evidence
about WhatsApp: the guarantee column is still derived from source only — the bridge's `sendText`
RPC transmits no idempotency token, so `supports_outbound_idempotency()` is left at the trait's
`false` default — and no replay of any kind has been driven. See
[whatsapp-bridge.md](whatsapp-bridge.md).

**Update 2026-07-30 — a message HAS now been sent, and the scope of that is narrow.** A real
number was QR-paired and a `sendText` delivered to a real WhatsApp account
(`messageId 3EB0404DBF5C774E89077E`, recipient confirmed receipt). The session also survived a
process kill and reconnected from stored `creds.json` with no re-pairing, which is the property
that matters for a deployment.

**But that was driven straight at the bridge over JSON-RPC, NOT through Core's
`WhatsappBridgeChannel`.** So what is proven is that *the bridge* can send, and that pairing and
session persistence work. The Rust adapter → bridge → WhatsApp path is **still unproven end to
end**, and no replay of any kind has been driven, so the guarantee column above is unchanged. The
distinction is recorded rather than blurred because collapsing it is precisely the error that put
two false `exactly-once` rows in this table.

**"NOT MEASURED" means not measured, and it is not a pass.** Seven of the ten — Email, Signal,
iMessage, MS Teams, **Twilio SMS, WhatsApp and Telegram** — have never had a replay driven at a
real destination. Their rows are derived: the adapter transmits nothing the destination is documented
to honour, so the capability bit and the spine's behaviour follow. That is real evidence about
*our* code and no evidence at all about the *platform's* behaviour. It is weaker than the rows
above it and is labelled rather than filled in optimistically.

Three rows have been driven at the real platform, all on 2026-07-30: Slack, which turned out to
be wrong — see the correction below; Discord, also wrong —
[§8](#8-discord-was-wrong-and-how-it-was-found); and Matrix, which held —
[§9](#9-the-matrix-row-driven-end-to-end-2026-07-30). Those are what make the derived rows
interpretable: two of them are the known-positive proving a duplicate is genuinely produced when
no key is honoured, rather than a duplicate being merely theorised.

### Correction, 2026-07-30 — Twilio and WhatsApp were never measured at a real destination

Until this date both rows read *"Replay measured at a real destination? **Yes** — a replayed key
produced **two** messages"*, and the paragraph above them said the four pre-existing rows "come
from one run in which a single delivery key was replayed twice through real adapters over real
HTTP".

**Every clause of that is true and the answer in the column is still wrong**, because the
question in the column header is *"at a real destination?"* and the run was
`crates/wcore-cli/tests/f24_c1_outbound_idempotency.rs` — a **`mockito`** fixture. Real adapters,
real HTTP, real production factory, and a destination we wrote. Measured with
`/usr/bin/grep` on 2026-07-30: **zero** files in `crates/` reference a Twilio or Meta live
credential (`TWILIO_ACCOUNT_SID`, `WHATSAPP_ACCESS_TOKEN`, `WA_ACCESS_TOKEN`, `live_twilio`,
`live_whatsapp`); the known-positive in the same search, `SLACK_BOT_TOKEN`, returns
`live_slack_actions.rs`, so the search was alive and the absence is real. **We hold no Twilio or
Meta credentials.**

This is the identical defect the Slack correction below diagnoses, in the two rows directly
beneath it: *the evidence column must state the same claim as the guarantee column.* It survived
that correction because the reviewer was looking at Slack.

**Telegram's row carried the same overstatement and has now been corrected too.**
`lane/twilio-whatsapp-identity` flagged it but declined to edit a row it had not measured, which
was the right call for a lane. The orchestrator verified it independently before changing it:
`TELEGRAM_BOT_TOKEN` and `live_telegram` return **four** hits across `crates/`, all four in
`wcore-safety/src/pii.rs` as a **redaction pattern** — no live test exists. Two known-positives in
the same sweep were alive (`SLACK_BOT_TOKEN` → `live_slack_actions.rs`,
`live_matrix`/`MATRIX_ACCESS_TOKEN` → `matrix_live_room.rs`), so the absence is real and not a
dead search. Telegram came from the same `mockito` run as the other two.

Neither guarantee changed. `at-most-once` was never in doubt for either: it follows from the
absence of a documented dedup slot, and that is a fact about the platforms' published APIs, not
about our fixture. What changed is the strength of the evidence claimed for it.

### Correction, 2026-07-30 — the Slack row was wrong, and how

Until this date the Slack row read **exactly-once**, "On restart, expect: **one message**",
live-proven. Its evidence column said *"real HTTP; the key was present on both attempts."*

**That is evidence for a different claim.** Key-on-wire is a fact about our request. Arrival
count is a fact about Slack. The row asserted the second and cited the first, and the three rows
beneath it show what the missing measurement looks like when it is actually taken — they say
"produced **two** messages", a count.

It has now been taken. Against `slack.com`, private channel `C0BLR1UKKU6`, through the adapter as
the production registry factory builds it:

```text
send   with key K -> ts 1785385438.299299
send   with key K -> ts 1785385438.564099     <- a SECOND message, not the first one
conversations.history: 2 arrivals with that body
```

Confirmed three independent ways in one run: two distinct `ts` values returned, two records read
back from `conversations.history`, and `chat.delete` succeeding on **both** (a delete that
succeeds proves the message existed; a delete of a fabricated `ts` returns `message_not_found`,
which is the control proving that instrument can fail). A raw-`curl` probe outside the adapter
reproduced it identically. Slack's Web API documents no request-level idempotency surface, and
the adapter's own `api.rs` already said the header was "inert against real Slack" — the capability
bit above it said the opposite, and the bit was the one the gateway read.

The practical consequence while the row was wrong: `LedgeredHandler::dispatch_fire` **re-sent**
outcome-unknown Slack deliveries on restart, believing the destination would collapse the replay.
Slack does not. Those re-sends were duplicates, and invisible from our side because the ledger
recorded one delivery. Slack now takes the same `abandoned` path as the other seven, which
surfaces the delivery to an operator instead of silently doubling it.

The header is still transmitted, because a Slack-compatible destination pointed at through
`api_base_url` may honour it. What changed is the claim about `slack.com`.

Standing lesson for this table: **the evidence column must state the same claim as the guarantee
column.** A row whose evidence is about our request cannot support a guarantee about their
arrival count.

### Where each guarantee comes from, in code

| Adapter | Capability declared at | Key reaches the wire at |
|---|---|---|
| Slack | `wcore-channel-slack/src/lib.rs` `supports_outbound_idempotency` — **`false`**, because `slack.com` ignores the key | the `idempotency-key` request header IS still sent on a keyed send (bound by the mockito test `a_keyed_send_puts_the_key_on_the_wire_though_slack_ignores_it`, and by its twin proving the header is **absent** when unkeyed) — but no Slack-honoured slot exists. The live arrival count is asserted by `wcore-channels-registry/tests/live_slack_actions.rs` |
| Matrix | `wcore-channel-matrix/src/lib.rs:294` | `wcore-channel-matrix/src/rest.rs:63` `txn_id_for_key`, used `rest.rs:133-135`; bound by test `lib.rs:539`, and by the live wire capture in [§9](#9-the-matrix-row-driven-end-to-end-2026-07-30) |
| Discord | **`false`**, overridden explicitly in `wcore-channel-discord/src/lib.rs` | `rest::nonce_for_key` IS still sent as `nonce` (`lib.rs:170-172`), and Discord ignores it for deduplication — see [§8](#8-discord-was-wrong-and-how-it-was-found) |
| Twilio SMS | **`false`**, overridden explicitly in `wcore-channel-sms/src/lib.rs` | the delivery id IS sent as an `Idempotency-Key` request header (`api::IDEMPOTENCY_HEADER`), bound in both directions by `a_keyed_send_puts_the_delivery_id_on_the_wire_though_twilio_ignores_it` and `an_unkeyed_send_carries_no_delivery_id_header`. Twilio's `Messages` resource documents no dedup slot, so nothing reads it there |
| WhatsApp | **`false`**, overridden explicitly in `wcore-channel-whatsapp/src/lib.rs` | the delivery id IS sent as `biz_opaque_callback_data`, the Cloud API's documented ≤512-char *tracking* string, which Meta echoes back in the `statuses` object of the `messages` webhook. Bound by `a_keyed_send_carries_the_delivery_id_as_biz_opaque_callback_data` and its absence twin |
| the other five | *no override* — they inherit the trait default `false` at `wcore-channels/src/lib.rs:139` | *nothing* — they inherit the pass-through `send_message_idempotent` at `wcore-channels/src/lib.rs:123-129`, which ignores the key |

#### Transmitting an id is ATTRIBUTION. It is not deduplication. (2026-07-30)

Four adapters now put the gateway's delivery id on the wire while declaring
`supports_outbound_idempotency() == false`: Slack, Discord, Twilio SMS and WhatsApp. That
combination is not a contradiction and it is not an oversight, so it is worth stating once.

- **Transmitting** the id is a fact about *our request*. It makes an arrival **attributable**: a
  destination that records what we sent can say which `cron:{job_id}:{scheduled_for_millis}`
  caused it, so a repeated body is judgeable as a replay (same identity twice — a real violation)
  or a recurrence (the trigger fired again — expected).
- **Deduplicating** is a fact about *their arrival count*, and only a run at the real platform
  can establish it.

Before 2026-07-30 Twilio and WhatsApp transmitted nothing, and the cost was not theoretical: in
the Windows journey run, **8 of 12 repeats were graded `indeterminate` — unjudgeable in
principle** — purely because `twilio.messages` and `whatsapp.messages` arrivals carried no
identity, and the receipt correctly refuses to call an unmeasurable property clean. The same
journey restricted to Slack returned 24 classified recurrences and `rc=0`.

**What this cost, stated rather than absorbed.** The old `false` for these two rested on a
*mechanical* argument — we send nothing, therefore nothing can dedupe. That argument is gone.
The bit stays `false` as a **conservative default pending
`wcore-channels-registry/tests/live_twilio_whatsapp_identity.rs`**, which is written, gated on
credentials we do not hold, and panics rather than skipping. The asymmetry that makes the trade
sound: a wrong `false` abandons a delivery *visibly* (`wayland-core gateway abandoned`), while a
wrong `true` duplicates one *silently*.

---

## 3. How the gateway decides, exactly

One code path, `LedgeredHandler::dispatch_fire` in
`crates/wcore-gateway/src/automation.rs:143-237`. Every scheduled channel delivery goes
through it.

| Ledger state when the delivery is seen again | Adapter declares | What happens | Line |
|---|---|---|---|
| first sight | either | attempt it | `:218` |
| `Settled` (succeeded **or** failed for a known reason) | either | **nothing is sent** | `:169-171` |
| `Attempted` — process died mid-send, outcome UNKNOWN | `true` | **re-attempt, carrying the original key**; the destination suppresses the replay | `:216-220` |
| `Attempted` — outcome UNKNOWN | `false` | **abandon**: recorded with reason `OutcomeUnknownNoDedup`, warned, not sent | `:201-215` |

Two consequences worth stating plainly:

- **A send that fails for a known reason is not retried by the delivery spine** (`:227-231`
  settles both arms). Conflating a known failure with an unknown outcome is what turns one
  failed send into a retry storm; the code deliberately does not.
- **The gateway never guesses.** For the eight at-most-once adapters it will not re-send, because
  re-sending to a destination that cannot recognise the replay *is* the duplicate. It records the
  delivery instead and hands the decision to you.

### What to do with an abandoned delivery

```
wayland-core gateway abandoned          # list them, with the reason
wayland-core gateway resend <id> --confirm-not-delivered
wayland-core gateway ack <id>           # you checked; nothing more to do
```

`resend` tells you, per destination, whether the replay is safe
(`wcore-cli/src/gateway.rs:981-990`): *"replay-safe: no — this destination cannot recognise a
replay; if the first copy did land, there are now two."* That sentence is generated from the
same capability bit as this table's Guarantee column, so it cannot disagree with it.

---

## 4. What the guarantee is scoped to

**Read this before quoting "exactly-once" to anyone.**

The guarantee is keyed on a *delivery id*, which is
(`crates/wcore-cron/src/runner.rs:324-338`):

```
cron:{job_id}:{scheduled_for_millis}[:{occurrence}]
```

That is a **(job, scheduled instant)** pair. Exactly-once means: *for one job firing at one
scheduled instant, the destination gets one message.*

It does **not** mean "the customer receives one message". If something upstream of the ledger
fires the same job for a *different* scheduled instant, that is a new delivery id, and to the
ledger and to every adapter it is a genuinely new delivery. No adapter's dedup can suppress it,
because the key differs.

That is not hypothetical, and it is not a defect either: **the ordinary way it happens is a
recurring trigger doing exactly what it was configured to do.** A job on `every:60` alive for
three minutes produces three delivery ids and three messages. See
[§5](#5-a-recurring-job-delivers-again-and-that-is-not-a-duplicate), which is the measured case
and the one this programme initially mis-filed as a Windows duplication defect.

### 4.1 Exactly-once stops at the message cap

**The Matrix row has a precondition, and until 2026-07-31 this document did not state it.**

Every adapter declares a single-message length cap through `Channel::max_message_len()`;
Matrix's is **16,384 characters** (`max_message_len()` in `crates/wcore-channel-matrix/src/lib.rs`; every cap is tabulated in [§4.2](#42-the-message-cap-per-adapter--declared-by-us-measured-by-nobody)). When a body
exceeds it, `ChannelManager::send_to_keyed`
(`crates/wcore-channels/src/manager.rs:776-812`) splits it and sends the pieces **with no
idempotency key at all**.

That is the correct behaviour, not a bug. An over-cap body becomes N messages at the
destination under one logical delivery, so one key cannot identify them; giving every chunk the
same key would make a *correct* destination suppress chunks 2..N as replays and silently
truncate the message. Dropping the key is the only honest option available.

But it means the guarantee inverts above the cap:

| Body | Key on the wire | Guarantee | A retry produces |
|---|---|---|---|
| ≤ 16,384 chars | yes — the txn id | **exactly-once** | one message; the homeserver returns the original `event_id` |
| > 16,384 chars | **none** | **at-least-once** | **a second full copy of every chunk** |

**So a retry above the cap duplicates, on Matrix, today.** The spine is what decides whether to
retry, and it used to ask a per-**adapter** question — `supports_outbound_idempotency()`, which
is a property of the connector and knows nothing about the body in hand. It therefore answered
`true` about sends that carried no key.

Callers now ask the per-**message** form,
`ChannelManager::supports_outbound_idempotency_for(channel, text)`, which is `true` only when
the adapter transmits a key *and* the body fits in one message. It reads the same
`chunks_for` decision the send itself uses, so the answer cannot drift from the behaviour. The
cap-blind form is retained for callers that genuinely ask about the connector — capability
reporting and the drift test below.

Two call sites moved, and one of them was printing the falsehood to a human:
`wayland-core gateway resend` reported `replay-safe: yes` for an over-cap body, at the exact
moment an operator is deciding whether a duplicate is possible. It now distinguishes "this
platform cannot deduplicate at all" from "this body was too long for the key to ride".

The other nine adapters are unaffected in practice: they are `at-most-once` at every length,
because they transmit nothing the destination honours whether the body is chunked or not.

#### The above-cap half is NOT MEASURED at a real destination (2026-07-31)

**Everything above this line is derived from our own source. The `at-least-once` claim for an
over-cap body has never been confirmed by counting arrivals at matrix.org**, and this section
would be repeating the exact mistake §8 and the Slack correction diagnose if it implied
otherwise. The reasoning is strong — no key is transmitted, and §8 established that a
destination with no honoured key produces a duplicate — but reasoning is what the Discord row
had.

The harness is written and committed:
`crates/wcore-channels-registry/tests/matrix_cap_replay.rs`. It sends an over-cap body, replays
the same delivery id, and counts arrivals through an independent read of the room — **and it
runs a below-cap control in the same session against the same room.** That control is not
decoration: "two arrivals above the cap" is equally well explained by "a replayed key always
duplicates here", which would make the §2 row false rather than conditional. Only the pair
distinguishes them. The test is `#[ignore]`d and **panics on missing configuration rather than
skipping**, so it cannot report green without having talked to a homeserver.

**Why it has not run:** the stored Matrix credential is dead. On 2026-07-31 matrix.org answered
the first authenticated call with `M_UNKNOWN_TOKEN — "Token is not active"`. A working token is
a Sean-only input, so this is blocked on a credential and not on engineering.

What that run DID establish, because it happens before the first network write:

```text
MCR_CAP=16384
MCR_BODY   ctrl_chars=51 ctrl_chunks=1   subj_chars=36814 subj_chunks=2
MCR_PREDICTED ctrl=true  subj=false
```

That is a measurement of **our** code and not of Matrix: the production `ChannelManager`, built
by the production loader from real channel TOML, answers `true` for a body that will carry the
key and `false` for one that will not. It is the fix working at the product surface. It is not
evidence about arrival counts, and it is not counted as any.

The run wrote nothing to the room — it failed on the baseline read, which precedes the first
send, and neither `MCR_CTRL_RECEIPTS` nor `MCR_SUBJ_RECEIPTS` was ever printed.

### 4.2 The message cap, per implementation — four measured live, two not decidable by that probe

**Generalised 2026-08-26, [FerroxLabs/wayland#934](https://github.com/FerroxLabs/wayland/issues/934).**
Until then exactly one cap in the product was bound to anything outside its own function.
`matrix.cap` was in the machine-readable block because §4.1's conditional guarantee needs a
boundary; the other six were each "covered" by a unit test of this shape:

```rust
assert_eq!(ch.max_message_len(), Some(1600));
```

That asserts the literal the function returns on the line above it. It restates the code. It
cannot fail except by someone editing both halves in one commit, and — the part that matters
— **it would keep passing if the number were wrong about the platform**, which is the only
way a cap can be wrong that anybody notices.

Every cap now has a row here and in the machine-readable block, and
`crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs` compares each one
against the adapter **the production factory builds**. `.cap` no longer means "the boundary
of a conditional guarantee"; it means "this adapter's `max_message_len()`".

| Adapter | Declared cap (chars) | What the vendor documents | Measured at the real platform? |
|---|---|---|---|
| **Slack** | 4,000 | **Quoted.** [`chat.postMessage`](https://docs.slack.dev/reference/methods/chat.postMessage), "Truncating content": *"For best results, limit the number of characters in the `text` field to 4,000 characters"*, and separately *"Slack will truncate messages containing more than 40,000 characters"*. 4,000 is advisory and is also the split point; 40,000 is silent truncation, not a catchable rejection. | **MEASURED 2026-08-27** — 4,040 is the largest single message; at 4,041 the API splits into 4,000-char messages. Was declared 39,000: the manager chunks on this value, so one 39,000-char send arrived as ten messages while `chunks_for(..).len() <= 1` marked it single-delivery. |
| **Matrix** | 16,384 | **Derived — the platform limit is BYTES.** [Client-Server API, Size limits](https://spec.matrix.org/latest/client-server-api/#size-limits): *"The complete event MUST NOT be larger than 65536 bytes … encoded as Canonical JSON."* Synapse enforces exactly that (`MAX_PDU_SIZE = 65536`); nothing documents a limit on `body` itself. `65536 / 4` is the largest scalar count whose UTF-8 encoding cannot exceed it. | **NOT MEASURED — and the two-point probe cannot measure it.** The derivation IS checked, hermetically and on every build, by `a_derived_cap_is_exactly_what_its_budget_admits`. The live arm owed is a SATURATING one: 16,384 astral-plane scalars, 65,536 UTF-8 bytes, the budget exactly. See the byte-budget note below. |
| **Discord** | 2,000 | **Quoted.** [Create Message](https://docs.discord.com/developers/resources/message): *"content?* — string — Message contents (up to 2000 characters)"*. The 25 MiB on the same page is the whole request. | **MEASURED 2026-08-27** — 2,000 accepted; 2,001 refused by the platform with HTTP 400 `50035 Invalid Form Body`. |
| **Telegram** | 4,096 | **Quoted.** [`sendMessage`](https://core.telegram.org/bots/api#sendmessage): *"text — String — Yes — Text of the message to be sent, 1-4096 characters after entities parsing"*. Unit unstated; `MessageEntity` on the same page indexes in UTF-16 code units. | **MEASURED 2026-08-29** — 4,096 accepted as one message, 4,097 refused `400: Bad Request: message is too long`. Probed in ASCII, so the cap is confirmed for ASCII text; the character-vs-UTF-16 question is still open. The arm that settles it is committed — `live_boundary_at_real_telegram` with `WL_LIVE_CAP_TELEGRAM_ASTRAL=1` — and if the answer is code units then the shipped 4,096 is UNSAFE for non-BMP text and must drop to 2,048. |
| **Twilio SMS** | 1,600 | **Quoted.** [Message resource](https://www.twilio.com/docs/messaging/api/message-resource): *"The text content of the outgoing message. Can be up to 1,600 characters in length."* The GSM-7/UCS-2 split in the same sentence governs segmentation and billing, not the maximum. | **MEASURED 2026-08-29** — 1,600 accepted as one concatenated message, 1,601 refused by Twilio before it reached a carrier: `400 code 21617, The concatenated message body exceeds the 1600 character limit`. Twilio's own error names the unit, so unlike Telegram this is unambiguously a CHARACTER limit and holds for either encoding. Probed in ASCII/GSM-7 at 11 segments. |
| **WhatsApp** | 4,096 | **Quoted.** [Cloud API text messages](https://developers.facebook.com/docs/whatsapp/cloud-api/messages/text-messages): *"Body text. … Maximum 4096 characters."* Unit unstated. | **NOT MEASURED** |
| **WhatsApp bridge** (`backend = "baileys"` / `"whatsapp-web"`) | 4,096 | **NOT a vendor figure — a POLICY.** Neither `baileys` nor `whatsapp-web.js` nor WhatsApp publishes a body limit for the Web/multi-device protocol these backends speak, so there is no page to quote. `BRIDGE_UNMEASURED_CHUNK_WIDTH` in `crates/wcore-channel-whatsapp/src/bridge/mod.rs` is a chunking width chosen for safety, and `cap_source` points at the decision rather than at a vendor: [wayland-core#360](https://github.com/FerroxLabs/wayland-core/issues/360). Overridable per channel with `max_message_chars`. | **NOT MEASURED** — it needs a running bridge (Node, an operator's own `bridge.js`, a QR-paired number), not a credential; nobody can issue one for a backend that authenticates by pairing. |
| **MS Teams** | 20,480 | **Derived — the platform limit is a UTF-16 PAYLOAD budget, and no character limit is documented at all.** [Format your bot messages](https://learn.microsoft.com/en-us/microsoftteams/platform/bots/how-to/format-your-bot-messages): *"The agent message size limit is 100 KB … it's recommended to ensure that the size of the message itself is within 80 KB … the agent receives a `413` status code (`RequestEntityTooLarge`) … `MessageSizeTooBig`."* `81920 / 4` (a scalar costs at most two UTF-16 code units). | **NOT MEASURED — and the two-point probe cannot measure it.** Same shape as Matrix: the derivation is checked hermetically on every build, and the live arm owed is 20,480 astral-plane scalars — 40,960 UTF-16 code units, 81,920 bytes. See the byte-budget note below. |
| **Email** | none | n/a | n/a — no cap to be wrong about |
| **Signal** | none | n/a | n/a — no cap to be wrong about |
| **iMessage** | none | n/a | n/a — no cap to be wrong about |

Every row's source is also in the machine-readable block as `<platform>.cap_source`, and
`crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs` fails the build if a
cap row has no source beside it. That does not make the number right — a citation can be
misread, and two of them just were — but it makes the number ACCOUNTABLE: a reader can open the
page and check, which is not true of a number that appears nowhere but in the function that
returns it. `Declared at` was dropped from this table because it duplicated the
`§4.2 → adapter` binding the declaration test already enforces; the file paths are unchanged
and are named in the machine-readable block's own prose.

#### Two of the seven were WRONG, found by reading the vendor documentation (2026-08-28, wayland#934)

**MS Teams was 28,000 and is now 20,480 — wrong three ways at once.** 28 KB is the **Incoming
Webhook** limit
([source](https://learn.microsoft.com/en-us/microsoftteams/platform/webhooks-and-connectors/how-to/add-incoming-webhook)),
a different surface from the Bot Framework one this adapter uses. The bot limit was never 28 KB:
it is 100 KB, and was 40 KB until 2025-09-16. And KB is not characters — the budget is the whole
payload "encoded as UTF-16", so reading "28 KB" as "28,000 characters" doubled even the wrong
number. Microsoft documents no character limit for a bot message's `text`, so 20,480 is derived
from the 80 KB figure Microsoft itself recommends, at four bytes per scalar worst case.

**Matrix was 32,768 and is now 16,384 — an arithmetic error with a live failure mode.** 32,768
is `65536 / 2`: it assumed two UTF-8 bytes per scalar. UTF-8 uses up to four. A 32,768-scalar CJK
body (3 bytes each) is 98,304 bytes, the homeserver rejects the event, and the message is
dropped — HIGH-6 reinstated for any non-Latin reply longer than about 21,800 characters. This
narrows the reach of the product's only exactly-once guarantee, and that is the correct trade:
a rejected event is a lost message, a chunked one merely loses its key.

**Neither correction is a measurement**, and both remain `cap_measured = no`. Each is still an
upper bound on the BODY, and each platform's real limit is on something the client cannot
compute: Matrix's is the complete signed PDU the homeserver assembles after the `PUT`; Teams's
is the serialized Activity including @-mentions and attachment JSON.

#### The WhatsApp bridge now has a row, because the guard can now reach it (2026-08-29, wayland-core#360)

Until this date the **WhatsApp bridge** had no row here and none in §2's machine-readable block,
and the reason was structural rather than an oversight: every gate enumerated PLATFORM STRINGS,
and the bridge is reached through the `whatsapp` platform string plus a `backend` key. It was the
eighth `max_message_len` in the product and the only one no test and no declaration row could
touch. Measuring its number without widening the guard would only have moved the blind spot to
the ninth adapter of the same shape.

So the guard was widened first. `wcore_channels_registry::constructible_selectors()` enumerates
what the registry can BUILD — nine implementations, not seven platforms — and both
`delivery_semantics_declaration.rs` and `live_message_cap_boundary.rs` walk that instead. Rows
are keyed by SELECTOR: the bare platform tag for a platform's default implementation, and
`platform+<backend>` where a config key selects a different one. Every row written before this
change keeps the name it had. The WhatsApp arm is derived from `WhatsappBackend::ALL_WIRE_NAMES`,
so a fourth backend appears in every gate downstream without a second list to remember.

**The number itself stopped borrowing.** It was `Some(4096)`, carried over from Meta's Cloud API
`text.body` documentation — a surface this code never touches, since the bridged backends speak
the WhatsApp Web/multi-device protocol through `baileys` or `whatsapp-web.js` and no vendor
publishes a body limit for it. It is now `BRIDGE_UNMEASURED_CHUNK_WIDTH`, documented in code as a
CHUNKING POLICY rather than a platform limit, with the safety argument that chose it: too high
loses messages (HIGH-6), too low only splits a reply that need not have been split, and `None` is
not available because it disables chunking and sends an unbounded body at a limit nobody knows.
An operator who has driven their own bridge overrides it per channel with `max_message_chars`,
which is the honest shape for a number the programme cannot source: ship the cautious default and
get out of the way of somebody with evidence.

#### What "NOT MEASURED" costs, and it is not symmetric

The generalisation above kills the **tautology**. It does not answer the question the issue was
filed about: *does the declared cap equal the platform's real limit?* Both numbers in every
comparison are still ours.

Being wrong is not cosmetic, and the two directions differ by platform:

- **Cap set too high — every platform.** The send exceeds what the destination accepts and is
  rejected. Chunking exists precisely so an over-long reply is not rejected and dropped
  (HIGH-6), so a too-high cap silently reinstates that bug. This is the dangerous direction and
  it applies to all nine capped implementations.
- **Cap set too low — Matrix, materially; the rest, cosmetically.** Bodies are chunked that did
  not need to be, and per [§4.1](#41-exactly-once-stops-at-the-message-cap) chunking is what
  drops the idempotency key. On Matrix that **downgrades exactly-once to at-least-once for
  messages that should have been covered by it**. On the other eight the guarantee is
  `at-most-once` at every length already, so an unnecessary split costs readability and
  nothing else.

For a CHARACTER cap, a live boundary probe answers it: send a body of exactly `cap` chars and
expect the platform to accept it, then send `cap + 1` chars **unchunked** and expect the
platform's own rejection. Both halves are needed — an accept at `cap` alone is equally well
explained by a cap that is far higher than we think.

#### Two of the caps are not character caps, and that probe cannot decide them (2026-08-29, wayland#934 c7)

**Matrix and MS Teams were listed as blocked on a credential. They were not.** Both caps are
derived from a payload BUDGET — Matrix's 65,536-byte Canonical-JSON PDU, Teams's 80 KB UTF-16
Activity — divided by the worst-case cost of one Unicode scalar. An ASCII body of `cap`
characters therefore spends a QUARTER of the budget, and `cap + 1` spends a quarter plus one
byte. Both arms land deep inside the accepted region, both come back accepted, and the probe
learns nothing. `enum Above` in `crates/wcore-channels-registry/tests/live_message_cap_boundary/cells.rs`
did not even have a variant for "accepted, normally, with no error" — the two-point shape had
no way to write down what these two platforms would actually have done. Issuing a Matrix token
would have bought two green arms and no measurement.

Two things replace it, and the first needs no credential at all:

- **The derivation is checked, on every build.** `derivation_faults` asserts that `cap`
  scalars spend at most the budget at the worst-case encoding, AND that `cap + 1` would exceed
  it — so the shipped number is exactly the derivation, not merely below it. Both of the
  mistakes recorded above fail it: 32,768 Matrix scalars cost 131,072 bytes against a 65,536
  budget, and 28,000 Teams scalars cost 112,000 against 81,920. Neither was visible to a
  cap-versus-document comparison, because in both cases the document and the adapter agreed
  with each other perfectly. `the_derivation_checker_rejects_the_two_caps_that_actually_shipped_wrong`
  drives the same checker over those exact numbers, so the rule has a red arm rather than only
  a clean input.
- **The live arm owed is a SATURATING one, not a boundary search.** One send of `cap`
  astral-plane scalars — the largest body the derivation claims is safe, and the one that
  spends the budget exactly. If the platform takes it, the derivation holds at its worst case;
  if it refuses, the refusal names the budget. An ASCII control at `cap + 1` runs beside it and
  its job is to be ACCEPTED, which is how "the two-point probe cannot decide this" becomes an
  observation in the run instead of an argument in a comment.

Even an accepted saturating arm is an **upper bound on the body, not the boundary**: Matrix's
budget covers the complete signed PDU the homeserver assembles after the `PUT`, and Teams's
covers the serialized Activity including @-mentions and attachment JSON. Neither is something
the client can size. That is recorded in each cell's `unmodelled` field rather than left for
the next reader to rediscover.

#### The unit question, and why an ASCII run cannot settle it (wayland#934 c8)

Every boundary this programme has driven was driven in ASCII, where one character is one
Unicode scalar is one UTF-16 code unit. So none of the four runs can tell a CHARACTER limit
from a CODE-UNIT limit, and those differ by a factor of two for astral-plane text — in the
dangerous direction. Telegram is the sharp case: `sendMessage` says *"1-4096 characters after
entities parsing"* while `MessageEntity` on the same page indexes in UTF-16 code units. If the
limit is code units, a 4,096-scalar emoji reply is 8,192 code units, the platform refuses it
**at the cap we ship**, and `send_to_keyed` does not re-send it.

`CapUnit` records which of the two each cell has settled, and `unit_safety_faults` refuses a
scalar cap above `limit / 2` once a UTF-16 verdict is recorded. No cell has settled one yet, so
that rule has nothing to enforce today — which is precisely the state in which a rule rots, so
`the_unit_rule_refuses_a_cap_a_utf16_verdict_makes_unsafe` constructs the verdict a Telegram
astral run would produce and requires the checker to refuse today's 4,096. The run itself is
one command against a credential the programme already holds, plus a destination chat:

```text
WL_LIVE_CAP_TELEGRAM_HOME=… WL_LIVE_CAP_TELEGRAM_CHANNEL=… WL_LIVE_CAP_TELEGRAM_TO=… \
WL_LIVE_CAP_TELEGRAM_ASTRAL=1 \
  cargo test -p wcore-channels-registry --test live_message_cap_boundary \
  -- --ignored --nocapture live_boundary_at_real_telegram
```

#### Which probe is blocked on which credential

The probe is per-platform and each needs a real destination. Following the
`live_twilio_whatsapp_identity.rs` pattern, each would be `#[ignore]`d, gated on a home
directory holding a real `channels/` config plus `credentials.toml`, and **panic rather than
skip** when that configuration is absent.

| Platform | Probe needs | Held? |
|---|---|---|
| **Slack** | `WL_LIVE_SLACK_HOME` (bot token with `chat:write`) + `WL_SLACK_CHANNEL` | **Yes** — `live_slack_actions.rs` drives a real workspace today |
| **Discord** | `WL_LIVE_DISCORD_HOME` (bot token) + `WL_LIVE_DISCORD_CHANNEL` (a real snowflake in a guild the bot has joined) | **Yes** — `live_discord_actions.rs` drives a real guild today |
| **Matrix** | `MATRIX_HOMESERVER` + `MATRIX_ACCESS_TOKEN` + `MATRIX_USER_ID` + `MATRIX_ROOM_ID`, and the SATURATING arm — 16,384 astral scalars — not the two-point probe | **Token is DEAD.** matrix.org answered `M_UNKNOWN_TOKEN — "Token is not active"` on 2026-07-31 and it has not been replaced. A working token is a Sean-only input. Note the token was never the whole blocker: the probe SHAPE was wrong too, and that half is fixed in-tree |
| **Telegram** | a `TELEGRAM_BOT_TOKEN` from BotFather + a chat id the bot may post to | **No.** Measured: the only hits for that name in `crates/` are the redaction pattern in `wcore-safety/src/pii.rs` |
| **Twilio SMS** | `WL_LIVE_TWILIO_HOME` (a `credentials.toml` carrying an account SID + auth token) + `WL_LIVE_TWILIO_TO`, and a Twilio-provisioned `from_number`. Costs real money per send | **No.** We hold no Twilio credential — see the 2026-07-30 correction in §2 |
| **WhatsApp** (Meta Cloud) | `WL_LIVE_WHATSAPP_HOME` (a Meta business app's access token + `phone_number_id`) + `WL_LIVE_WHATSAPP_TO`, with the recipient inside the 24-hour customer-service window | **No.** We hold no Meta credential |
| **WhatsApp bridge** | `WL_LIVE_CAP_WHATSAPP_BAILEYS_{HOME,CHANNEL,TO}` (or `…_WHATSAPP_WEB_…`), a Node runtime, the operator's own `bridge.js` with the backend package installed, and a WhatsApp number QR-paired to it | **No.** And not obtainable as a credential at all: both bridged backends authenticate by QR pairing, so there is nothing anybody can issue — it needs a running bridge and a real paired account |
| **MS Teams** | a Bot Framework app id + app password, a bot registered in a tenant, a `serviceUrl`/conversation id from a real Teams conversation, and the SATURATING arm — 20,480 astral scalars — not the two-point probe | **No.** No test in `crates/` references one. As with Matrix, the credential was never the whole blocker |

Four of the nine have been driven. Of the five that have not: two (Matrix, MS Teams) are
byte-budget shapes whose derivation is already checked in-tree and whose remaining arm is a
single saturating send; two (the bridge backends) cannot be unblocked by procurement at all,
because a QR-paired account is not a credential anybody issues; and one (Meta Cloud) is a
genuine procurement item. Telegram is separately owed an ASTRAL run on a credential the
programme already holds — that one is neither a shape problem nor a procurement item, only a
destination chat.

---

## 5. A recurring job delivers again, and that is not a duplicate

**A recurring cron job whose run outlives one trigger period delivers each body again, under a
NEW delivery id. That is the trigger working, on every platform.** Windows crosses the period
more often than the others, and nothing about the behaviour is Windows-specific.

This section previously read *"On Windows, a gateway whose runtime restarts across the Task
Scheduler `PT1M` repetition boundary re-fires cron jobs that have already fired"* and was filed
as the defect `F24-GWP-H1`. **Both halves of that sentence are wrong**, and the run it cited is
the evidence against it:

- *"re-fires jobs that have already fired"* — the second delivery of a body is a **different
  scheduled occurrence**, not a re-fire of the first. Every repeat in that run carried a
  **different delivery id**: 5 of 5 keyed jobs, **zero replays**.
- *"On Windows"* — the mechanism is platform-neutral. Windows crosses the window **reliably**,
  not exclusively.

**So the one remaining exactly-once row in §2 is conditional on timing — on every platform, not
just Windows.** A recurrence carries a different delivery id, and a different id is not a replay,
so Matrix will send the second copy too. That is correct behaviour for a recurring job and it is
not what the exactly-once guarantee is about; §4 is the section that says what the guarantee is
scoped to. Slack and Discord reach the same outcome by a plainer route: since the 2026-07-30
corrections above, neither has an honoured idempotency slot at all, so nothing anywhere is
deduplicating them.

### What was actually measured, 2026-07-30

The journey submits every job with `--trigger every:15`, which is **rate-floored to sixty
seconds** — `TriggerBound::new((*every_secs).max(60), 1)` (`wcore-cron/src/trigger.rs:238`),
applied to the resulting instant at `trigger.rs:366`. They are 60-second **recurring** jobs, so
any run alive past one period legitimately sees each body twice.

The internal control is in the same run: the **heartbeat** job, which was never inside a kill
window, recurred three times with scheduled deltas of **60068 ms** and **64940 ms** — the floor,
measured directly — and nobody ever called those duplicates.

| | arrival lines | distinct texts | arrivals per text | distinct delivery ids among repeats |
|---|---|---|---|---|
| Windows | 27 | 13 | `{2: 12, 3: 1}` | **all distinct — 5 of 5 keyed jobs, 0 replays** |
| macOS | 13 | 13 | `{1: 13}` | n/a — no repeats |

It is **deterministic, not intermittent**. Task Scheduler's minimum repetition interval is
`PT1M`, which exceeds the 60 s floor, so a Windows kill-and-recover leg always costs more than
one period; launchd and systemd restart inside it. The two Windows runs bear this out exactly:
the 67.7 s run produced 12 repeats, the 0.3 s run produced 0. Predicting the count from the run
duration is what makes this recurrence rather than a fault.

It was also **not** a lock failure — the process count never exceeded 1 — and the ledger
recorded 27 distinct delivery ids **each settled exactly once**. The spine did its job
perfectly, which is what you would expect, because there was nothing to suppress: see
[§4](#4-what-the-guarantee-is-scoped-to). A different key is not a replay.

### So do the §2 exactly-once rows still hold on Windows?

**Yes, unchanged.** Exactly-once is scoped to a delivery id, and every delivery id in that run
was delivered once. What a slow run changes is the number of delivery ids, not the guarantee
over each — and *more* of them is a stronger measurement of the property, not a weaker one.

What a reader should take from it is the §4 warning restated: **"exactly-once" does not mean
"one message".** A recurring job that is alive for three minutes will send three messages, and
no adapter's dedup can or should suppress that.

### The one real gap the same run exposed

Of 24 delivery arrivals, **only 8 carried an `idempotency_key` at all** — one adapter of the
three. `twilio.messages` and `whatsapp.messages` emit none, so for them a replay is
indistinguishable from a recurrence **in principle**, not merely in this harness. That is the
`at-most-once` row in §2 seen from the measurement side, and it is why the journey gate counts
an unclassifiable repeat **against** the run rather than passing it: an unmeasurable property
reported as a measured clean one is the failure this document exists to prevent.

### How to grade a run of this kind

**Not from the receipt headline.** For that same run the headline read `arrived: 12,
duplicates: 0, losses: 0` (`F24-GWP-M1`, since fixed). And **not from `duplicates` alone**
either, in the other direction: `duplicates` counts repeats of a message **body**, which is not
what exactly-once is about.

`wayland-journey verify` reports the classification and grades on it
(`crates/wcore-eval-scenarios/src/journey.rs`, `DeliveryIdentity`):

| bucket | meaning | grade |
|---|---|---|
| `replays` | the same delivery id arrived twice | **FAIL** — a real exactly-once violation |
| `recurrences` | the same body under different delivery ids | **PASS** — the trigger fired again |
| `indeterminate` | a repeat where an arrival carried no id | **FAIL** — unprovable is not clean |

Before that distinction existed, both the driver and the verifier refused any `duplicates != 0`,
so a Windows journey — which crosses the period every time — had **no reachable pass state at
all**. A gate that cannot pass proves as little as one that cannot fail, and it additionally
hides real progress.

---

## 6. Why the eight cannot simply be fixed

`supports_outbound_idempotency` is a **capability declaration, not a preference**
(`wcore-channels/src/lib.rs:131-138`). An adapter that returns `true` without transmitting a key
the destination will honour reintroduces exactly the duplicate the method exists to prevent — it
converts a *visible* duplicate into an *invisible* one. So `false` is not a gap to be closed by
editing a boolean; it is true.

Seven platforms provide no client-supplied deduplication token at all: Telegram Bot API, Twilio
Programmable Messaging, WhatsApp Cloud API, SMTP, `signal-cli`, AppleScript-driven Messages.app,
and Microsoft Bot Framework. Verified 2026-07-30 against a three-model panel — unanimous, 7 of 7
"no" from each of three independent models, with primary sources: Twilio documents SMS-send
POSTs as explicitly non-idempotent; RFC 5321 §6.1 permits duplicate delivery and RFC 5322 §3.6.4
makes `Message-ID` an identifier, not a dedup contract; signal-cli's JSON-RPC `id` only
correlates the response; the Bot Framework `activity.id` is channel-assigned and senders are
told not to deduplicate on it.

**Discord is the eighth, and it fails for a different and more instructive reason.** The other
seven expose no token to send. Discord exposes one — `nonce` — and *accepts* it, echoing the
value straight back in the create response. It simply never deduplicates on it. A token that is
accepted and ignored is strictly more dangerous than no token at all, because the adapter can
truthfully say "the key is on the wire" and be wrong about the only thing that matters. See
[§8](#8-discord-was-wrong-and-how-it-was-found).

One nuance, because "the platform" and "the API we use" are not the same thing: **Telegram's
lower-level MTProto API does have a `random_id` dedup token — the Bot API's `sendMessage` does
not expose it.** This adapter is a Bot API client, so the token is unreachable from where we
stand. Reaching it would mean writing an MTProto client, which is a different product, not a
fix to this one.

**The nearest miss is SMTP, and it is still a no.** `make_outbound_message_id`
(`wcore-channel-email/src/smtp.rs:287`) mints a fresh RFC 5322 `Message-ID` per send, and
deriving it from the delivery key instead would be a few lines. But no RFC and no common MTA
guarantees suppression of a duplicate `Message-ID`; a few destinations happen to. Declaring
`true` on the strength of "some mailboxes probably will" would be a reassuring sentence over
code that does not implement the guarantee, which is the failure mode this document exists to
prevent. It is left as it is, and recorded here as the one candidate a future product decision
could revisit.

---

## 7. Keeping this document true

A declaration that drifts from the code is worse than no declaration. This one is enforced by
`crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs`, which:

1. parses the **Guarantee** column of §2's table out of this very file at test time;
2. constructs **all ten adapters through the production factory**
   (`channel_factory_for`) with hermetic fixture configs and no real credentials;
3. asserts, per adapter, that `supports_outbound_idempotency()` is `true` exactly when this
   table says `exactly-once` or `exactly-once-below-cap`;
4. asserts the row set and the constructible-adapter set are **the same set** — a new adapter
   with no row here fails the build, and a row here naming no adapter fails it too;
5. asserts **every** cap against the wire, not only the conditional row's: a `<platform>.cap`
   line is present exactly when the constructed adapter returns `Some(n)` from
   `max_message_len()`, and equals that `n`. Generalised on 2026-08-26 (#934) from the
   Matrix-only rule added on 2026-07-31, when Matrix's cap had **no test of any kind** — the
   one number the surviving exactly-once claim is conditional on was the one number nothing
   checked;
6. asserts the guarantee-specific rules on top of that: `exactly-once-below-cap` requires a cap
   row, so a conditional promise cannot be made with its condition left unstated; and a row
   claiming bare `exactly-once` must belong to an adapter reporting **no** cap, which is what
   stops this document sliding back to the unconditional sentence it carried until 2026-07-31;
7. asserts every cap row carries a `<platform>.cap_measured` verdict, and that the verdict
   agrees with §4.2's table. Without it an `assert_eq!` against a number implies the number was
   verified; the verdict makes the difference between "checked against our adapter" and
   "checked against the platform" a thing the file has to state rather than imply;
8. asserts every cap row carries a `<platform>.cap_source` naming the vendor documentation the
   number came from, and that it is a URL rather than an assurance. Added 2026-08-28
   (wayland#934) after reading those pages found two of the seven caps wrong — Teams's had been
   taken from a different product surface's documentation and misread from KB into characters,
   and Matrix's assumed two UTF-8 bytes per character where UTF-8 uses four. Neither is a
   drift a `cap`-vs-adapter comparison could ever have caught, because both numbers agreed.

If you change an adapter's capability **or its `max_message_len`**, this file is part of the
change.

**What checks 5, 6 and 7 still do NOT establish — tracked as
[FerroxLabs/wayland#934](https://github.com/FerroxLabs/wayland/issues/934).** They compare the
cap in this document against the cap the adapter returns. Both numbers are ours. Whether either
equals the *platform's* real limit is unmeasured — which is why every row reads
`cap_measured = no` rather than being left to look verified.
[§4.2](#42-the-message-cap-per-adapter--declared-by-us-measured-by-nobody) states what being
wrong costs in each direction, and names, per platform, the exact credential the live boundary
probe is waiting on. Four of the seven are now measured. One more (WhatsApp) needs a credential; Matrix and
MS Teams CANNOT be measured by this two-point probe at all — see the note in the probe
file — because their caps are derived from a byte budget and both arms land inside the
accepted region.

**The probe itself is committed**, at
`crates/wcore-channels-registry/tests/live_message_cap_boundary.rs` (wayland#934 item 2). It
holds one cell per capped adapter carrying either the boundary that was measured and what the
platform did one character above it, or the credential the measurement is waiting on. Three of
its checks run on every ordinary `cargo test`: the shipped cap must not exceed a measured
boundary, every `cap_measured = live` row here must have a measured cell there (and the
reverse), and the never-driven cells are PRINTED as a census so an unmeasured cap is loud
rather than absent. The seven live cells are `#[ignore]`d and each PANICS naming its missing
variable, because a probe that cannot run must not report a pass.

---

## 8. Discord was wrong, and how it was found

Until 2026-07-30 this document put Discord in the exactly-once column. **It was wrong**, and the
row said why it might be: *"No — mock only. Key-on-wire is bound by a mockito test; no real
Discord destination has been driven."* A mockito test can only ever prove that we **send** a
stable token. Everything after that — that Discord would **honour** it — was inference, and the
inference was false.

Measured by `lane/discord-live` against a real bot, a real guild and a real channel.

### The platform does not deduplicate on `nonce`. There is no window.

Same channel, same author, byte-identical nonce, replayed at four delays:

| delay | first id | second id | result |
|---|---|---|---|
| 0 s | 1532233150594289704 | 1532233156847992891 | **two messages** |
| 5 s | 1532233158874108034 | 1532233181867278427 | **two messages** |
| 30 s | 1532233187801960489 | 1532233320211943434 | **two messages** |
| 90 s | 1532233322401370353 | 1532233706088038480 | **two messages** |

`BL-24C1-DISCORD-WINDOW` asked how long the dedup window is. The answer is that there isn't one.

Three controls, because a verdict that can only ever read "duplicate" would be a
permanently-red gate and worth nothing:

1. **The nonce is accepted, not rejected.** `POST` returns 200 and Discord echoes the value back
   (`nonce_sent == nonce_echoed`), so the token is well-formed and inside the 25-char cap.
2. **The comparator can report identity** — two GETs of one message compare equal.
3. **A same-id outcome is reachable through this very API** — `PATCH` returns the same id as the
   `POST`. So "deduplicated" was an achievable result; it just never happened.

### End to end through the gateway, one delivery id, two messages

Outcome-unknown was produced honestly rather than simulated: the adapter's own `api_base_url`
seam pointed at a proxy that forwards the create to real Discord and then never responds, so the
message lands and the product never learns the outcome — the `F24-C-H1` shape exactly.

| step | evidence |
|---|---|
| `once:` trigger — structurally cannot fire twice | job `97ce67c3-52f0-48da-92e1-80692363a555` |
| attempt 1 reached Discord, key on the wire | `FORWARDED id=1532234475344498829 nonce=wle82e6651cfa60bb8` |
| the nonce IS the derivation of the delivery id | `nonce_for_key("cron:97ce67c3-…:1785383566000") = wle82e6651cfa60bb8`; `millis+1` correctly differs |
| gateway killed `-9`, then restarted | gateway 2's own banner: `carried=1 (unattempted 0 / unknown-outcome 1)` |
| arrivals at Discord, baseline 0 | **2** |

Because the adapter declared `true`, the spine took the re-attempt arm at `automation.rs:216-220`
instead of the abandon arm at `:201-215`, and `wayland-core gateway abandoned` was empty before
and after. The product did not fail to notice a duplicate; **it created one on purpose**, on the
strength of a guarantee the platform does not provide.

### What changed

`supports_outbound_idempotency()` now returns `false` for Discord. The nonce is still sent —
it is useful to clients for optimistic reconciliation — but the spine no longer treats it as a
replay guard, so an outcome-unknown Discord delivery is abandoned, recorded and nameable, like
every other at-most-once adapter.

### The lesson worth keeping

The row that was wrong is the row that had never been driven at a real destination, and it said
so in its own last column. **A "NOT MEASURED" cell is a prediction, and predictions in this table
have now been wrong once.** The other four unmeasured rows — Email, Signal, iMessage, MS Teams —
predict `at-most-once`, which is the safe direction to be wrong in; Discord predicted
`exactly-once`, which is not.


---

## 9. The Matrix row, driven end to end (2026-07-30)

Every other exactly-once row in §2 rests on a key being *present on the wire*. The Matrix row
now rests on something stronger: the shipped `wayland-core` binary crashed mid-send against
`matrix.org` and the replay was collapsed by the homeserver.

Measured by `lane/matrix-live` on `hetzner-dsm`, room `!REDACTED-MATRIX-ROOM:matrix.org`. The
product spoke to the real homeserver through a recording forwarder
(`scripts/matrix-live-proxy.mjs`) which forwarded the first send upstream **for real** and then
withheld the response, so the event landed while the product's outcome stayed unknown. The room
was then read by a separate process talking directly to matrix.org.

| | txn id on the wire | homeserver's `event_id` |
|---|---|---|
| process life 1 (pid 3132637), response withheld, then `kill -9` | `cron:bf4c989c-…:1785385265000` | `$BAnrbBtxNCqVOn0q…` |
| process life 2 (pid 3138250), `carried=1 (unknown-outcome 1)` | `cron:bf4c989c-…:1785385265000` — **identical** | `$BAnrbBtxNCqVOn0q…` — **identical** |
| control: same body, **different** delivery id | `cron:99a26815-…:1785385376000` | `$8rWWSSH7nc9lgq3F…` — different |

Independent read of the room: **2 events**, not 3.

**The control is the point.** A count of one would have been equally explained by
exactly-once working and by the replay never being attempted. Two — with one of them
demonstrably produced by a *different* delivery id — distinguishes them, and is the live
demonstration of [§4](#4-what-the-guarantee-is-scoped-to): a different key is not a replay.

### The row was true and unreachable at the same time

This run also found the reason no one had ever driven it: `Target::Channel` carried no
destination, and the dispatcher passed the **channel name** as the outgoing `conversation_id`.
The first attempt produced

```text
PUT /_matrix/client/v3/rooms/mxlive/send/m.room.message/cron:…    <- `mxlive` is the CHANNEL NAME
403 M_FORBIDDEN "User @… not in room mxlive"
```

Not Matrix-specific: the per-adapter default-destination fallbacks (`slack lib.rs:416`,
`whatsapp :238`, `sms :250`) are all gated on an **empty** conversation id, which cron never
produced, so none of them was reachable from a scheduled delivery. `cron add --conversation`
and `Target::Channel::conversation_id` close it. Until then, the exactly-once guarantee in
§2 described a path that could not address a Matrix room at all.

### One thing a redaction does NOT tell you

`Channel::delete_message` reports success when the homeserver accepts the redaction, and
`rest.rs:342-349` calls that "the strongest guarantee the protocol offers". Measured the same
night: **matrix.org answers `200 {"event_id": …}` to a redaction of an event id that never
existed**, and to one with an empty event id. Acceptance is therefore compatible with nothing
having been redacted. Grade a delete by reading the event back and checking that `content.body`
is gone — never by the status code.

<!-- DELIVERY-SEMANTICS-MACHINE-READABLE
Do not edit by hand. Kept in step with the tables in §2 and §4.2; the test reads BOTH and
requires them to agree, so a table edit that misses this block fails, and vice versa.

Vocabulary: exactly-once | exactly-once-below-cap | at-most-once | at-least-once.

`exactly-once-below-cap` is the CONDITIONAL guarantee of §4.1 — the key rides only while the
body fits in one platform message. A row declaring it MUST also carry a `<platform>.cap` line.
Conversely a row declaring bare `exactly-once` MUST have `max_message_len() == None`: a finite
cap with an unconditional claim is the drift this vocabulary exists to make unsayable.

Rows are keyed by SELECTOR, not by platform (wayland-core#360, 2026-08-29). A selector key is
the bare platform tag for a platform's default implementation and `platform+<value>` where a
config key selects a different one, exactly as `ChannelSelector::key` renders it. Nine of the
eleven rows are bare platform tags and are unchanged; `whatsapp+baileys` and
`whatsapp+whatsapp-web` are the WhatsApp bridge, which the platform-keyed harness could not
reach at all. The gates walk `wcore_channels_registry::constructible_selectors()`, so a further
implementation reached by a config key appears here without a second list to update.

`<selector>.cap` is that implementation's `max_message_len()` in chars, for EVERY one that
declares one — not only the conditional row. Generalised 2026-08-26 (wayland#934) from the
Matrix-only meaning it had before, because a cap is a fact about the adapter rather than the
boundary of a guarantee, and it is load-bearing on every platform: `send_to_keyed` chunks on
it. A cap row is present exactly when the constructed adapter returns `Some(n)`, and equals
that `n`; an adapter returning `None` carries no cap row.

`<platform>.cap_measured` is REQUIRED beside every cap row. Vocabulary: no | live.
`no` means the number has never been checked against the PLATFORM — only against our own
adapter, which is a drift check and not a measurement. `live` may be written only for a
platform whose boundary probe (a send at the cap, and a send one char over it) has actually
run against a real destination. §4.2 names what each `no` row is waiting for.

`<selector>.cap_source` is REQUIRED beside every cap row too, and must be a URL. It names the
vendor page the number is derived from, so the number is checkable by a reader rather than
being an assertion about itself. Where no vendor page governs the surface — the two bridge rows,
whose backends speak a protocol nobody documents a body limit for — it names the decision that
chose the number instead, which is the accountable answer available: an unsourceable number
must say so at a URL a reader can open, not carry a citation from the wrong vendor. Added 2026-08-28 (wayland#934); reading these pages is what
found `msteams.cap` taken from the Incoming Webhook surface and misread from KB into
characters, and `matrix.cap` computed at two UTF-8 bytes per character instead of four. The
adapter caps are declared in `crates/wcore-channel-<platform>/src/lib.rs` in every case.
slack = at-most-once
slack.cap = 4000
slack.cap_measured = live
slack.cap_source = https://docs.slack.dev/reference/methods/chat.postMessage
matrix = exactly-once-below-cap
matrix.cap = 16384
matrix.cap_measured = no
matrix.cap_source = https://spec.matrix.org/latest/client-server-api/#size-limits
discord = at-most-once
discord.cap = 2000
discord.cap_measured = live
discord.cap_source = https://docs.discord.com/developers/resources/message
telegram = at-most-once
telegram.cap = 4096
telegram.cap_measured = live
telegram.cap_source = https://core.telegram.org/bots/api#sendmessage
sms = at-most-once
sms.cap = 1600
sms.cap_measured = live
sms.cap_source = https://www.twilio.com/docs/messaging/api/message-resource
whatsapp = at-most-once
whatsapp.cap = 4096
whatsapp.cap_measured = no
whatsapp.cap_source = https://developers.facebook.com/docs/whatsapp/cloud-api/messages/text-messages
whatsapp+baileys = at-most-once
whatsapp+baileys.cap = 4096
whatsapp+baileys.cap_measured = no
whatsapp+baileys.cap_source = https://github.com/FerroxLabs/wayland-core/issues/360
whatsapp+whatsapp-web = at-most-once
whatsapp+whatsapp-web.cap = 4096
whatsapp+whatsapp-web.cap_measured = no
whatsapp+whatsapp-web.cap_source = https://github.com/FerroxLabs/wayland-core/issues/360
email = at-most-once
signal = at-most-once
imessage = at-most-once
msteams = at-most-once
msteams.cap = 20480
msteams.cap_measured = no
msteams.cap_source = https://learn.microsoft.com/en-us/microsoftteams/platform/bots/how-to/format-your-bot-messages
-->
