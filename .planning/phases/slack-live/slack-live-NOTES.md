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
2. ~~Author the live test.~~ **DONE** — `crates/wcore-channels-registry/tests/live_slack_actions.rs`.
3. ~~fmt + compile.~~ **DONE — see M5.**
4. ~~Live-run.~~ **DONE — see M5.**
5. Fix the HIGH the run reproduced. ← next

## M5 — LIVE RUN through the production adapter: 4/5 PASS, idempotency FAILS

`cargo check -p wcore-channels-registry --all-targets` on hetzner at
`81718bb276e2332a129ee4e5719b93abdbd4cd75` (worktree SHA asserted equal to the pushed head):
**rc=0**. `cargo fmt --all -- --check` on the Mac: **rc=0**.

Live run on hetzner (the Mac cannot compile). Credentials injected **on stdin only**, never in
argv, never written to disk — the §0 sanctioned exception, disclosed here and in the summary.
Secret sweep of the run log on hetzner **and** independently on the Mac against the live values:
**0 hits for the bot token, 0 for the signing secret.** Sweep instrument proven alive on a
known-positive in the same capture (`WL-LIVE-SLACK` found).

```
running 1 test
  PASS  send         sent ts=1785385433.854489 and read it back from history;
                     a fabricated channel was refused with channel_not_found
  PASS  edit         edited ts=1785385434.912479; history text changed "…edit-before" -> "…edit-after";
                     a fabricated ts was refused with message_not_found
  PASS  delete       deleted ts=1785385436.335589 and confirmed its ABSENCE from history;
                     the second delete of the same ts was refused with message_not_found
  PASS  receive      read ts=1785385437.708659 back from conversations.history; the real record,
                     independently signed and replayed through ingest_webhook, surfaced as
                     MessageReceived; a corrupted signature was rejected and enqueued nothing
  FAIL  idempotency  declares supports_outbound_idempotency() == true, so a replayed key must
                     produce 1 message. It produced 2. ts 1785385438.299299 then .564099
  clean channel: no marker messages remain
  4/5 legs passed
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

`WLRC=101`, `WLDONE` present. The executed count is **1 test run, 0 ignored, 0 filtered out** —
not a vacuous suite.

**The red is the correct outcome and is NOT being engineered away.** It reproduces M4's raw-API
finding through the shipped adapter and the production registry factory, which is the strongest
form of the evidence. The fix is to make the declaration true to the platform, not to soften the
test.

Note the failure message is generated from the declaration, not hardcoded: the leg asserts
`arrivals == if declared {1} else {2}`. After the fix it goes green, and it would redden again if
anyone re-asserted the guarantee or if Slack ever started honouring the header.

## M6 — fix landed and RE-PROVEN; the same gate driven red then green

Fix at `28098809`. Live matrix re-run at that commit: **5/5 PASS**,
`test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

```
PASS idempotency  declared=false; replayed key -> 2 arrival(s); a distinct key -> 3
5/5 legs passed
```

One assertion, two outcomes, caused by the product change:

| commit | declaration | arrivals | gate |
|---|---|---|---|
| `81718bb2` | `true` | 2 | **RED** |
| `28098809` | `false` | 2 | **GREEN** |

That is §3b-iii satisfied on the real thing rather than argued: the gate can fail and it can pass,
and both were observed.

Clippy `-D warnings` over the three touched crates initially caught a `needless_borrow` in my own
test — repaired at `3316fbe5`, re-run rc=0. `cargo check --workspace --all-targets` rc=0 with
**120 crates checked** (count read back; a no-op run cannot pass as a clean one).

Channel verified empty from the Mac after the final run: 2 `channel_join` events, 0 WL markers.
Final secret sweep across `.planning/`, `crates/` and `docs/`: **0 files** contain either live
value; sweep instrument proven alive (`WL-LIVE-SLACK` found in 5 files).
