# 24-C1 idempotency lane — running NOTES

Lane `lane/24-idempotency`. Base SHA `ef1d97beb61f1b084bdfba745e8f49830924d757`.
Subject: **outbound** delivery idempotency. (Inbound path + `scripts/f24-inbound.mjs` belong to
lanes `24-c3-tg-email` / `24-c3-discord` — untouched here.)

Append-and-recommit after every measurement. Do not save the write-up for the end.

---

## Step 1 — verification of the inventory finding (COMPLETE, all three claims TRUE)

**Claim A — the trait method defaults to `false`.** TRUE.
`crates/wcore-channels/src/lib.rs:139`:
```rust
fn supports_outbound_idempotency(&self) -> bool { false }
```
The doc comment above it is explicit that this is a CAPABILITY declaration, not a preference,
and that a `true` here without actually transmitting the key would reintroduce the duplicate.

**Claim B — Slack is the sole override in the workspace.** TRUE.
Full grep of `supports_outbound_idempotency` (9 hits in `*.rs`, `/target/` excluded) yields
exactly one adapter override: `crates/wcore-channel-slack/src/lib.rs:234`. Other hits are the
trait default (`wcore-channels/src/lib.rs:139`), a forwarding method on the manager
(`wcore-channels/src/manager.rs:665`), the spine's consumer (`wcore-agent/src/cron.rs:152`),
and doc/test references.

Adapter count = **10** crates (`crates/wcore-channel-{discord,email,imessage,matrix,msteams,
signal,slack,sms,telegram,whatsapp}`), each with exactly one `impl Channel for`. So the
1-of-10 / 9-of-10 split in the finding is arithmetically correct.

**Claim C — the 12-of-12 tally ran entirely through Slack.** TRUE.
`scripts/f24-journey.mjs:380` is `'platform = "slack"',` and it is the **only**
`platform = "..."` line in the whole driver — the journey has exactly one channel config, and
it is Slack. Confirmed by grep (1 match, 1 file).

=> The finding survives verification. Proceeding.

## Step 1b — CORRECTION to the brief's framing: this is NOT unfiled

The brief describes this as "unfiled, found by an independent inventory an hour ago". That is
wrong, and it matters for how much of this lane is new work. The 9-of-10 gap is already
recorded, in near-identical language, in **four** committed planning documents at base:

- `.planning/CRITERIA-GAP-LEDGER.md:244`
- `.planning/phases/24-.../24-C-SUMMARY.md:241` (and :46, :17)
- `.planning/REQUIREMENTS.md:233` (F24-05)
- `.planning/ROADMAP.md:223`

All four already assert the *answer* to the brief's question (2): "*nine channel adapters
inherit `supports_outbound_idempotency() == false`, for which an outcome-unknown delivery is
now correctly **abandoned** rather than duplicated — safe and honest, and not the same thing as
delivered*".

So the disposition is pre-filed. What is NOT filed anywhere is a **measurement** of it on any
adapter other than Slack. That is the gap this lane can actually close, and it is what the
brief asks for in (2): measure, do not reason from the trait.

## Step 2 — mechanism read (source read done; MEASUREMENT STILL OWED)

Decisive site: `crates/wcore-gateway/src/automation.rs:129-209`, `LedgeredHandler::dispatch_fire`.

```
destination_dedupes = inner.dispatch_is_idempotent(target)     // -> the trait bit, via cron.rs:152
match ledger.accept(&id):
  Accepted  => fall through, attempt
  Duplicate =>
     if state == Settled            -> return Ok(())            // never re-attempted
     if !destination_dedupes
        && state == Attempted       -> abandon(); flush(); warn!; return Ok(())
     else                           -> fall through, re-attempt
begin_attempt(); flush()
outcome = inner.dispatch_fire(...)
settle(&id, outcome.is_ok()); flush()                           // BOTH arms settle
```

**The abandonment is much narrower than "every retry on nine adapters".** It fires only in the
intersection of three conditions:

1. the delivery id was seen before (`Accept::Duplicate`), AND
2. its state is `Attempted`, not `Settled` — i.e. the process **died between `begin_attempt`
   and `settle`**, so the outcome is genuinely UNKNOWN, AND
3. the destination cannot recognise a replay (`!destination_dedupes`).

A known failure does NOT reach it: both outcome arms call `settle`, so a failed send becomes
`Settled` and the next fire with the same id short-circuits at `return Ok(())`. A plain retry
of a known-failed send is therefore not abandoned — it is not retried at all, which is a
different (and separately arguable) behaviour.

So the honest characterisation to test is: **on nine of ten adapters, a delivery that was
in flight when the process was killed is abandoned.** Not "all sends are abandoned".

**Two facts that cut AGAINST the brief's HIGH hypothesis** (which was premised on the loss
being silent and producing no error, like the inbound HIGH fixed last night):

- the abandon path emits `tracing::warn!(delivery = %id, "...abandoning rather than
  duplicating it")` — it is not silent;
- `l.abandon(&id)` + `l.flush()` records it durably as a terminal, operator-nameable state —
  the 24-B drain evidence shows abandoned deliveries being listed by name.

That is a *deliberate, announced, recorded* choice between two bad outcomes, not an accident.
It is still a non-delivery, and whether that is HIGH is exactly what the measurement must
inform. I will not grade it before measuring.

## Still to establish (nothing below this line is measured yet)

- [ ] Drive a real outcome-unknown retry on >=3 adapters with different transports.
      Candidate fixture seams to check for an `api_base_url`-style redirect:
      telegram (`scripts/f24-tg-fixture.mjs`), whatsapp, sms, msteams, discord.
- [ ] Count arrivals at an independent sink: prove the POSITIVE path first (messages arrive,
      counted) before trusting any no-duplicate/no-arrival number — a universal denial
      manufactures a green.
- [ ] Distinguish the three possible outcomes per adapter: abandoned / duplicated / other.
- [ ] Grade Criterion 1 honestly; state severity with reasoning; cost the fix.

## Instrument discipline for this lane (§6b-ii)

- explicit `instrument_fault` state; a suspect run grades INCOMPLETE, never LOSS.
- byte-count every capture; no `echo "EXIT=${PIPESTATUS[0]}"` after a pipeline.
- self-test the matcher three ways incl. "the old broken matcher would have missed it".
- run test targets by file, never by filter (a filter matching no test exits 0 on 0 tests).

---

## Step 3 — harness siting decided (no manifest change, no Cargo.lock churn)

Composition seam found: **`crates/wcore-cli`** already depends on `wcore-agent` (test-utils),
`wcore-channels-registry` (which depends on all 10 adapter crates), `wcore-cron`,
`wcore-gateway`, and already has `mockito = "1"` as a dev-dependency. So a NEW integration
test file under `crates/wcore-cli/tests/` reaches the entire spine with **zero Cargo.toml and
zero Cargo.lock edits** — which matters because Cargo.lock is a Phase-24 shared seam that
concurrent lanes conflict on deterministically.

`crates/wcore-cli/src/lib.rs` and `src/main.rs` are the fenced files; `tests/` is not fenced,
and a new file there is additive and belongs to this lane alone.

Public entry points confirmed:
- `wcore_gateway::automation::AutomationPlane::start(home, store, handler, history_path)` —
  public, and it is what wraps the injected handler in the (private) `LedgeredHandler` at
  `automation.rs:281`. So the real spine is reachable without touching gateway internals.
- `wcore_channels::ChannelManager::supports_outbound_idempotency(name)` (manager.rs:665) and
  `send_to_keyed(name, msg, key)` (manager.rs:674).
- `wcore_channels_registry::auto_register_from_dir(mgr, dir, creds)` — builds adapters from
  real on-disk TOML through the PRODUCTION factory (`channel_factory_for`), so the measurement
  runs the same construction path a real deploy does, not a hand-rolled one.

Adapters with a real fixture seam (`api_base_url` in the TOML `[options]` table, all
`#[serde(deny_unknown_fields)]`, so a typo fails loudly rather than silently reaching
production):

| adapter  | config field   | transport / shape                                  |
|----------|----------------|----------------------------------------------------|
| slack    | `api_base_url` | Slack Web API, JSON + Bearer      — declares `true` |
| telegram | `api_base_url` | Bot API, token-in-path            — declares false  |
| sms      | `api_base_url` | Twilio REST, form-encoded + Basic — declares false  |
| whatsapp | `api_base_url` | Meta Graph, JSON + versioned path — declares false  |

Four genuinely different transports, one of which is the known-positive. `email` (SMTP),
`imessage` (AppleScript) and `signal` (subprocess) have no HTTP seam and are out of scope for
an HTTP fixture; that limit will be stated in the report rather than papered over.

## Step 3b — the measurement that actually decides this (design, not yet run)

The question "abandon vs duplicate" has TWO halves and the finding only names one:

- **Half 1 — is the `false` TRUTHFUL?** If an adapter declaring `false` were in fact able to
  dedupe at its destination, the spine would be abandoning deliveries it did not need to
  abandon, and the defect would be over-conservatism, not loss. Test: send the SAME key twice
  through the real adapter at a fixture sink and count arrivals + inspect the wire for any
  dedupe token. Two arrivals with no token = the `false` is truthful and the abandon is
  protecting a genuine duplicate.
- **Half 2 — what does the SPINE do?** Drive `AutomationPlane` with a real adapter, prove the
  positive path (message arrives, counted) FIRST, then take the outcome-unknown path and count
  arrivals again.

Half 1 is the novel half — nothing in the phase has measured it on any adapter — and it is the
one that decides whether the 9-of-10 gap is "missing capability" or "wrong default".

Positive-control discipline: no no-duplicate/no-arrival number is trusted until the sink has
been shown to record a real arrival in the same run. A universal denial manufactures a green.

---

## Step 4 — MEASURED (hetzner `hz/24-idempotency` @ 5d71b7e3, isolated run)

`cargo test -p wcore-cli --test f24_c1_outbound_idempotency`
→ `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
Executed count read back = 6, not zero. Status file `WLRC=0` + `WLDONE`.

**The replay really does duplicate.** Mutation M1 (`.expect(2)` → `.expect(1)` at all 4 sites)
turned all four arrival tests RED and mockito printed the true hit counts:

```
POST /bot111:AAAA-f24c1-bot-token/sendMessage      idempotency-key: (missing)   ...but received 2
POST /2010-04-01/Accounts/AC…/Messages.json        idempotency-key: (missing)   ...but received 2
POST /v18.0/10987654321/messages                   idempotency-key: (missing)   ...but received 2
POST /api/chat.postMessage   idempotency-key: cron:f24c1-job:1785121776528      ...but received 2
```

So on Telegram, Twilio SMS and WhatsApp a replayed delivery key produces **two messages at the
destination and carries no dedupe token**. The `false` is TRUTHFUL. Slack carries the key on
both attempts — the known-positive holds, which is what makes the other three interpretable.

Mutation M2 (trait default `false` → `true` in `wcore-channels/src/lib.rs:139`) reddened
exactly the two capability tests and nothing else, proving they read the real adapter → real
`ChannelManager` → real `EngineJobHandler` chain rather than restating source. Reverted; clean
re-run back to 6 passed / 0 failed.

## Step 5 — the finding that outranks the one I was sent for

The abandon path's own comment justifies itself as: *"recorded, terminal, and nameable by an
operator"*. **The code does not implement that.**

- `ledger.rs:214 pending()` filters to `Accepted | Attempted` — `Abandoned` is EXCLUDED.
- `ledger.rs:223 pending_count()` — same filter, and its doc says *"the number drain publishes"*.
- `ledger.rs:253 compact()` classes `Abandoned` as terminal history, subject to the
  `retain_settled` bound — so an abandoned delivery can be **compacted out of the journal**.
- `DeliveryState::Abandoned` has **no consumer anywhere outside `ledger.rs`** (workspace grep;
  the other `Abandoned` hits are unrelated types — `CronFireOutcome`, `HookPhaseState`).

Net: the only signal that a delivery was abandoned is one `tracing::warn!`. No gateway verb
lists it, nothing re-sends it, and the record is eligible for deletion. That is what turns a
defensible "recorded non-delivery" into an effectively unrecoverable one — and unlike the
platform limits, it is **entirely inside our code**.

## Step 6 — fix cost is NOT uniform across the nine (this reframes the whole finding)

- **7 of 10 platforms have no idempotency primitive at all** (Telegram Bot API, Twilio
  Messages, Meta Graph, SMTP, signal-cli, AppleScript iMessage, MS Teams). For these `false`
  is a permanent, truthful statement about the platform. Not fixable by writing code.
- **2 of the 9 already put a token on the wire** and are cheap:
  - **Matrix** — `rest.rs:47` PUTs `…/send/m.room.message/{txn_id}`, which is the Matrix
    protocol's NATIVE idempotency slot, but `txn_id` comes from
    `static TXN_COUNTER: AtomicU64 = AtomicU64::new(1)` (`rest.rs:13`) — a process-local
    counter that **resets to 1 on every restart**, i.e. it does not survive the exact event
    the ledger exists for.
  - **Discord** — already sends a dedup `nonce` and reuses it across the in-adapter retry
    loop (closing HIGH-7), but `next_nonce()` is `{wall-clock-ms:x}-{counter:x}`, documented
    as deliberately *"distinct across restarts"* — so a post-restart replay gets a new nonce
    and Discord will not dedupe it.
- **UNMEASURED HYPOTHESIS, do not grade as a finding:** because Matrix's counter restarts at
  1, a fresh process reuses txnIds 1,2,3… for DIFFERENT messages. If a homeserver still holds
  the old txn in its per-access-token cache it would return the original event and silently
  drop the new message. That is a distinct potential loss path. I did not measure it and it
  must not be reported as fact.
