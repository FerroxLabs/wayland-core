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
| **2 of 10** adapters | exactly-once — Slack, Matrix |
| **8 of 10** adapters | at-most-once — a delivery whose outcome is unknown is **abandoned, not retried** |
| **0 of 10** adapters | at-least-once (the gateway never automatically re-sends to a destination that cannot recognise a replay) |
| **On Windows** | a known defect can produce a duplicate on **any** adapter, including the two above — see [§5](#5-windows-f24-gwp-h1) |

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
| **Discord** | `nonce` field on message create — **transmitted, but Discord does not dedupe on it** | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Discord | **Yes** — a replayed key produced **two** messages; see [§8](#8-discord-was-wrong-and-how-it-was-found) |
| **Telegram** | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Telegram | **Yes** — a replayed key produced **two** messages, no dedupe token on the wire |
| **Twilio SMS** | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Twilio | **Yes** — a replayed key produced **two** messages |
| **WhatsApp** (Meta Graph) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Meta | **Yes** — a replayed key produced **two** messages |
| **Email** (SMTP) | none that any MTA guarantees | **at-most-once** | **abandoned** | zero or one message — unknowable without checking the mailbox | **NOT MEASURED** |
| **Signal** (`signal-cli`) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Signal | **NOT MEASURED** |
| **iMessage** (AppleScript) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Messages.app. **macOS only** — on Linux and Windows the adapter is not compiled in and cannot be constructed at all | **NOT MEASURED** |
| **MS Teams** (Bot Framework) | none | **at-most-once** | **abandoned** | zero or one message — unknowable without checking Teams | **NOT MEASURED** |

**"NOT MEASURED" means not measured, and it is not a pass.** Four of the ten — Email, Signal,
iMessage, MS Teams — have never had a replay driven at a real destination. Their rows are
derived from source: the adapter transmits no key the destination honours, so the capability bit
and the spine's behaviour follow mechanically. That is real evidence about *our* code and no evidence at all about the
*platform's* behaviour. It is weaker than the four rows above it and is labelled rather than
filled in optimistically.

The live rows (Slack, Matrix, Telegram, Twilio, WhatsApp and now Discord) come from runs in which a single
delivery key was replayed twice through real adapters over real HTTP, built by the production
factory. That run is what makes the other rows interpretable: it is the known-positive proving
a duplicate is genuinely produced when no key is honoured, rather than a duplicate being merely
theorised.

### Where each guarantee comes from, in code

| Adapter | Capability declared at | Key reaches the wire at |
|---|---|---|
| Slack | `wcore-channel-slack/src/lib.rs:249` | `idempotency-key` request header (`lib.rs:338`, `:371`); bound by tests `lib.rs:489` (header present when keyed) and `lib.rs:521` (header **absent** when unkeyed) |
| Matrix | `wcore-channel-matrix/src/lib.rs:294` | `wcore-channel-matrix/src/rest.rs:63` `txn_id_for_key`, used `rest.rs:133-135`; bound by test `lib.rs:539` |
| Discord | **`false`**, overridden explicitly in `wcore-channel-discord/src/lib.rs` | `rest::nonce_for_key` IS still sent as `nonce` (`lib.rs:170-172`), and Discord ignores it for deduplication — see [§8](#8-discord-was-wrong-and-how-it-was-found) |
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
A different key is not a replay, and Slack and Matrix will each post the second copy.
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
   table says `exactly-once`;
4. asserts the row set and the constructible-adapter set are **the same set** — a new adapter
   with no row here fails the build, and a row here naming no adapter fails it too.

If you change an adapter's capability, this file is part of the change.

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

<!-- DELIVERY-SEMANTICS-MACHINE-READABLE
Do not edit by hand. Kept in step with the table in §2; the test reads BOTH and requires
them to agree, so a table edit that misses this block fails, and vice versa.
slack = exactly-once
matrix = exactly-once
discord = at-most-once
telegram = at-most-once
sms = at-most-once
whatsapp = at-most-once
email = at-most-once
signal = at-most-once
imessage = at-most-once
msteams = at-most-once
-->
