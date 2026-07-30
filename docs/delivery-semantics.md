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
| **3 of 10** adapters | exactly-once — Slack, Matrix, Discord |
| **7 of 10** adapters | at-most-once — a delivery whose outcome is unknown is **abandoned, not retried** |
| **0 of 10** adapters | at-least-once (the gateway never automatically re-sends to a destination that cannot recognise a replay) |
| **On Windows** | a known defect can produce a duplicate on **any** adapter, including the three above — see [§5](#5-windows-f24-gwp-h1) |

Nothing is ever silently dropped. An abandoned delivery is recorded, listed by
`wayland-core gateway abandoned`, and re-sendable by an operator.

---

## 2. The table

"Guarantee" is per **delivery id** — read [§4](#4-what-the-guarantee-is-scoped-to) before
relying on it, because that scope is narrower than "one message".

| Adapter | Platform primitive | Guarantee | Outcome-unknown delivery is… | On restart, expect | Replay measured at a real destination? |
|---|---|---|---|---|---|
| **Slack** | `Idempotency-Key` HTTP header on the send | **exactly-once** | **retried** with the same key | one message | **Yes** — real HTTP; the key was present on both attempts |
| **Matrix** | `PUT …/send/m.room.message/{txnId}` — the txn id *is* the idempotency slot | **exactly-once** | **retried** with the same key | one message; the homeserver returns the original `event_id` | **Yes** — replay driven against a real, fresh Synapse |
| **Discord** | `nonce` field on message create | **exactly-once**, *within Discord's dedup window* | **retried** with the same key | one message **if** the replay lands inside Discord's dedup window; the window's length is unknown | **No — mock only.** Key-on-wire is bound by a mockito test; no real Discord destination has been driven. Window length open as `BL-24C1-DISCORD-WINDOW` |
| **Telegram** | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Telegram | **Yes** — a replayed key produced **two** messages, no dedupe token on the wire |
| **Twilio SMS** | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Twilio | **Yes** — a replayed key produced **two** messages |
| **WhatsApp** (Meta Graph) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Meta | **Yes** — a replayed key produced **two** messages |
| **Email** (SMTP) | none that any MTA guarantees | **at-most-once** | **abandoned** | zero or one message — unknowable without checking the mailbox | **NOT MEASURED** |
| **Signal** (`signal-cli`) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Signal | **NOT MEASURED** |
| **iMessage** (AppleScript) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Messages.app. **macOS only** — on Linux and Windows the adapter is not compiled in and cannot be constructed at all | **NOT MEASURED** |
| **MS Teams** (Bot Framework) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Teams | **NOT MEASURED** |

**"NOT MEASURED" means not measured, and it is not a pass.** Five of the ten — Discord, Email,
Signal, iMessage, MS Teams — have never had a replay driven at a real destination. Their rows
are derived from source: the adapter transmits no key the destination honours (or, for Discord,
transmits one whose window nobody has bounded), so the capability bit and the spine's behaviour
follow mechanically. That is real evidence about *our* code and no evidence at all about the
*platform's* behaviour. It is weaker than the four rows above it and is labelled rather than
filled in optimistically.

The four live rows (Slack, Telegram, Twilio, WhatsApp) come from one run in which a single
delivery key was replayed twice through real adapters over real HTTP, built by the production
factory. That run is what makes the other rows interpretable: it is the known-positive proving
a duplicate is genuinely produced when no key is honoured, rather than a duplicate being merely
theorised.

### Where each guarantee comes from, in code

| Adapter | Capability declared at | Key reaches the wire at |
|---|---|---|
| Slack | `wcore-channel-slack/src/lib.rs:249` | `idempotency-key` request header (`lib.rs:338`, `:371`); bound by tests `lib.rs:489` (header present when keyed) and `lib.rs:521` (header **absent** when unkeyed) |
| Matrix | `wcore-channel-matrix/src/lib.rs:294` | `wcore-channel-matrix/src/rest.rs:63` `txn_id_for_key`, used `rest.rs:133-135`; bound by test `lib.rs:539` |
| Discord | `wcore-channel-discord/src/lib.rs:344` | `rest::nonce_for_key`, used `lib.rs:170-172`; bound by test `lib.rs:583` |
| the other seven | *no override* — they inherit the trait default `false` at `wcore-channels/src/lib.rs:139` | *nothing* — they inherit the pass-through `send_message_idempotent` at `wcore-channels/src/lib.rs:123-129`, which ignores the key |

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
- **The gateway never guesses.** For the seven at-most-once adapters it will not re-send, because
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
because the key differs. That is not hypothetical — see §5.

---

## 5. Windows (F24-GWP-H1)

**On Windows, a gateway whose runtime restarts across the Task Scheduler `PT1M` repetition
boundary re-fires cron jobs that have already fired.** Measured 2026-07-30 by
`lane/gateway-platforms` at the sink's own journal, not the product's own count:

| | arrival lines | distinct texts | arrivals per text |
|---|---|---|---|
| Windows | 27 | 13 | **`{2: 12, 3: 1}`** |
| macOS | 13 | 13 | `{1: 13}` |

All twelve deliveries arrived twice, the second pass in one burst at the repetition boundary.
It is **not** a lock failure — process count never exceeded 1. The ledger recorded **27
distinct delivery ids, each settled exactly once**: the spine did its job perfectly and the
duplicate was created *above* it, as a second delivery id.

**So on Windows the exactly-once rows in §2 are conditional on timing, for all three of them.**
A different key is not a replay, and Slack, Matrix and Discord will each post the second copy.
This applies to every adapter, not to a subset.

It is intermittent — a second Windows run that finished before crossing a boundary was clean.
The honest statement is: *whenever a Windows run crosses the `PT1M` boundary with live cron
jobs, deliveries repeat.*

**Do not grade this from the journey receipt's headline.** For that same run the headline read
`arrived: 12, duplicates: 0, losses: 0` (`F24-GWP-M1`). Only the per-adapter breakdown
dissented and only `wayland-journey verify` caught the disagreement. The table above is written
from the sink's journal for that reason.

Linux and macOS show no such defect in the same journey.

---

## 6. Why the seven cannot simply be fixed

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
   table says `exactly-once`;
4. asserts the row set and the constructible-adapter set are **the same set** — a new adapter
   with no row here fails the build, and a row here naming no adapter fails it too.

If you change an adapter's capability, this file is part of the change.

<!-- DELIVERY-SEMANTICS-MACHINE-READABLE
Do not edit by hand. Kept in step with the table in §2; the test reads BOTH and requires
them to agree, so a table edit that misses this block fails, and vice versa.
slack = exactly-once
matrix = exactly-once
discord = exactly-once
telegram = at-most-once
sms = at-most-once
whatsapp = at-most-once
email = at-most-once
signal = at-most-once
imessage = at-most-once
msteams = at-most-once
-->
