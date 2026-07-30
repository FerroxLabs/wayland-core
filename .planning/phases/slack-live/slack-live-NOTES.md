# slack-live — running NOTES (append-only, committed after every measurement)

Lane: `slack-live`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-slack-live`,
branch `lane/slack-live`, base `5013505e7caefa5561f0de40c75406afe1b42fc3` (asserted with
`/usr/bin/git rev-parse HEAD`).

Goal: prove send / edit / delete / receive / idempotency for the Slack adapter against the REAL
Slack Web API, bound to exactly one channel — private `wayland-test`, `C0BLR1UKKU6`, in the LIVE
Trade Canyon workspace. Leave the channel empty.

---

## M1 — scope probe (first action, live, 2026-07-30)

`auth.test` succeeded. `x-oauth-scopes: chat:write,channels:history,channels:read`.

| call | result |
|---|---|
| `auth.test` | `ok:true`, team `Trade Canyon, Inc.`, bot `wayland_core_test` |
| `conversations.info C0BLR1UKKU6` | `ok:false` `missing_scope` `needed: groups:read` |
| `conversations.history C0BLR1UKKU6` | `ok:false` `missing_scope` `needed: groups:history` |

**The dispatch brief's scope snapshot HELD at this timestamp.** Read scopes still absent. Re-probe
before each read-dependent leg.

## M2 — premise read of the code (before any live mutation)

The brief said to follow `live_discord_actions.rs`. **It does not exist on my base** — no
`live_*` test exists under any `wcore-channel*` crate. Closest analogs are
`crates/wcore-channels-registry/tests/{native_action_matrix,delivery_semantics_declaration}.rs`,
which build adapters through the PRODUCTION factory `channel_factory_for(platform)`. That is the
shape I will follow: drive the shipped adapter, not a hand-rolled HTTP client.

What the adapter actually has (`crates/wcore-channel-slack/src/`):

- `send_message` → `chat.postMessage` (retry + Retry-After). Real.
- `edit_message` → `chat.update`. Real, declared `Implemented`.
- `delete_message` → `chat.delete`. Real, declared `Implemented`.
- receive → **Events API webhook only** (`ingest_event`, HMAC + 5-min replay window). There is NO
  history-polling receive path in the adapter at all.
- idempotency → `send_message_idempotent` attaches an **`Idempotency-Key` HTTP header**
  (`api.rs:77`), and `supports_outbound_idempotency()` returns **`true`** (`lib.rs:249`).

### The premise I most doubt, stated before measuring

`api.rs:73-76` says the header "is inert against real Slack, which ignores unknown request
headers". Yet `docs/delivery-semantics.md:38` declares Slack **exactly-once**, "On restart,
expect: **one message**", and marks it live-proven — but the evidence column reads *"real HTTP;
the key was present on both attempts"*. **That is evidence for a different claim.** Key-on-wire is
not arrival-count. The Telegram/Twilio/WhatsApp rows in the same table cite an actual count
("produced **two** messages"); the Slack row does not.

`supports_outbound_idempotency()==true` is what licenses
`LedgeredHandler::dispatch_fire` to RE-SEND an outcome-unknown Slack delivery
(`docs/delivery-semantics.md:84`). If Slack does not honour the header, that re-send is a real
production duplicate.

**Prediction to be falsified: a replayed `Idempotency-Key` against real Slack yields TWO distinct
`ts` values, i.e. two messages.**

### Counting arrivals WITHOUT the read scope

`groups:history` is absent, but arrival count is still measurable, two independent ways:

1. `chat.postMessage` returns the created message's `ts`. Two distinct `ts` = two messages.
2. `chat.delete` on each `ts` — a delete that returns `ok:true` proves that message existed.
   `chat.delete` on a fabricated `ts` returns `message_not_found`, which is the known-negative
   proving the instrument can fail.

So the idempotency leg is provable live NOW. Only the *receive* leg is genuinely scope-blocked.

### Receive has TWO blockers, not one — the scope is only the smaller one

Even with `groups:history`, reading history exercises **no adapter code**: the adapter receives
via a signed Events API webhook. Driving that live needs a public HTTPS endpoint plus an Events
API subscription on the Slack app — Sean-reserved app configuration, not a scope grant. I will not
report "receive proven" off a `conversations.history` read; that is arrival verification at the
platform, not the adapter's receive path.

## M3 — scopes ARRIVED mid-run; re-measured independently (2026-07-30)

The coordinator reported the two read scopes had landed. I did not trust the message; I re-ran my
own probe:

```
x-oauth-scopes: chat:write,channels:history,channels:read,groups:history,groups:read
conversations.info    C0BLR1UKKU6 -> ok:true  name=wayland-test is_private=true is_member=true
conversations.history C0BLR1UKKU6 -> ok:true, 2 messages
```

**Confirmed by my own instrument.** The dispatch brief's scope table is now stale in the
product's favour — all five legs are runnable live and no scope-skip is justified.

### True starting state of the channel, measured

| ts | subtype | user | text |
|---|---|---|---|
| 1785382174.613739 | `channel_join` | U0BLBKR56NT (the bot) | `<@U0BLBKR56NT> has joined the channel` |
| 1785382157.617439 | `channel_join` | U3PGRDZGA (Sean) | `<@U3PGRDZGA> has joined the channel` |

**Zero leftover probe messages.** The coordinator's belief that its own probe messages were
already deleted is confirmed independently. The two residents are join events, which are not
deletable by `chat.delete` and are not ours to remove. "Channel empty" therefore means: back to
exactly these two join events.

## M4 — GROUND TRUTH: real Slack IGNORES `Idempotency-Key`. Prediction confirmed.

Three `chat.postMessage` calls with an identical body: #1 and #2 carried the **same**
`Idempotency-Key`, #3 a different one.

```
post1:          ok=True ts=1785384703.758879
post2_same_key: ok=True ts=1785384704.229169
post3_diff_key: ok=True ts=1785384704.734389
distinct ts returned: 3
SAME-KEY replay returned same ts? False
ARRIVALS IN CHANNEL carrying the marker: 3     <- read back from conversations.history
```

Two independent arrival counts agree: three distinct `ts` values returned, and three marker
messages actually present in the channel. A **replayed key produced a second message.**

Confirmed a third way, by deletion — a delete that succeeds proves the message existed:

```
del_1: ok=True   del_2: ok=True   del_3: ok=True
del_repeat (same ts twice): ok=False error=message_not_found   <- delete's failing direction
del_bogus  (fabricated ts):  ok=False error=message_not_found   <- instrument known-negative
marker messages remaining: 0 ; total remaining: 2 (both channel_join)
```

### What this falsifies

1. **`docs/delivery-semantics.md:38`** — "On restart, expect: **one message**" for Slack is
   **FALSE**. So is the `exactly-once` guarantee in the same row, and the "3 of 10" headline at
   line 21.
2. **`wcore-channel-slack/src/lib.rs:249`** — `supports_outbound_idempotency() -> true` is a claim
   the wire does **not** back. `api.rs:73-76` already said the header "is inert against real
   Slack"; the capability bit above it says the opposite, and the bit is the one the gateway reads.
3. **Production consequence.** `LedgeredHandler::dispatch_fire` re-sends an `Attempted`,
   outcome-unknown delivery when the adapter declares `true` (`delivery-semantics.md:84`). For
   Slack that re-send **is** a duplicate. The adapters that declare `false` are abandoned instead —
   which is the correct behaviour Slack is currently opted out of.

Severity **HIGH**: a customer-facing delivery guarantee, contradicted by measurement, driving a
live retry path.

The pre-existing evidence for the Slack row was *"real HTTP; the key was present on both
attempts"* — key-on-wire, never arrival-count. The row inferred the count it never measured.
That is the LANE-BRIEF §3b-i shape exactly: an assertion whose stated evidence supports a
different, weaker claim.

## Plan

1. ~~Raw-curl ground truth for the idempotency question.~~ **DONE — see M4.**
2. Author `crates/wcore-channels-registry/tests/live_slack_actions.rs` driving the production
   factory, env-gated, every leg with a both-directions control.
3. `cargo fmt --all -- --check` on the Mac; `cargo check --workspace --all-targets` on hetzner.
4. Live-run the test from the Mac (network egress).
5. Report skipped cells as skipped, with the exact missing scope. A skip is not a pass.
