# MATRIX-LIVE — running notes

Lane `lane/matrix-live`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-matrix-live`, base
`43c69ca71bc788dcd925fc070204d6918c2d7e0f`.

Goal: close the **Matrix half** of `24-C3` with live proof against `matrix.org` —
send / edit / delete / receive / **outbound idempotency across a real process restart**.

> **Secret discipline.** `MATRIX_ACCESS_TOKEN` appears in no file in this directory,
> no commit, no log, no capture. Every capture is swept before commit; the sweep result
> and its known-positive control are recorded in §6.

---

## 1. Premise verification (LANE-BRIEF: "your brief's measurements are probably stale")

| brief claim | verified? | evidence |
|---|---|---|
| `lib.rs:294` overrides `supports_outbound_idempotency` | **HELD** | `crates/wcore-channel-matrix/src/lib.rs:294-296` returns `true` |
| the override was added after fixing a restart-unstable id | **HELD** | `rest.rs:1-30` module docs; the id came from `AtomicU64::new(1)`, reset per process |
| Matrix txn id is the idempotency mechanism | **HELD** | `rest.rs:63 txn_id_for_key`, used `rest.rs:133-135` |
| delivery id is `cron:{job}:{millis}` scoped, not per-message | **HELD** | `docs/delivery-semantics.md` §4, `wcore-cron/src/runner.rs:324-338` |
| hetzner has outbound HTTPS to matrix.org | **HELD** | `curl https://matrix.org/_matrix/client/versions` → **HTTP 200** from `hetzner-dsm` |
| hetzner disk | 967G free on `/` | `df -h /root` |

## 2. Structural findings BEFORE any run (source-measured, not inferred)

### F-ML-1 — the self-echo filter blocks the single-account inbound leg

`sync.rs:414-416` — `parse_sync_events` **skips every event whose `sender ==
bot_user_id`**. We hold exactly one account (`@REDACTED-MATRIX-USER:matrix.org`), so a probe
message posted by that account is discarded by design and can never reach the product.

Consequence for the inbound leg: the channel's configured `user_id` must name a
*different* mxid than the sender, or nothing arrives. That is a real configuration
(the token and the `user_id` field are independent inputs), and it is disclosed.
It also yields a control that runs in **both directions** in the same session:

- `user_id = @wayland-probe-not-sean:matrix.org` → the event MUST arrive;
- `user_id = @REDACTED-MATRIX-USER:matrix.org` (the true sender) → the same event MUST NOT
  arrive, and that is the self-echo filter working, not an instrument failure.

### F-ML-2 — `edit_message` / `delete_message` have NO production caller

Measured with `/usr/bin/grep` across `crates/`, with a known-positive in the same
sweep. Outside the adapter crates and the conformance test, there is no call site:
no CLI verb, no agent tool, no gateway path, no protocol command. `wayland-core
channel actions --require edit` exists and gates on a capability **nothing in the
shipped product can invoke.**

So "drive edit through the product" has no binary-level path today. The strongest
available proof is the production factory (`auto_register_from_dir` → the same
`MatrixChannel` the gateway builds, from a real on-disk channel config and a real
credentials store) invoking the trait method. That is what will be built, and the
missing surface is reported as a defect rather than papered over.

## 3. Plan

| # | capability | driver | independent corroboration |
|---|---|---|---|
| 1 | send | `wayland-core` binary: `cron add --channel` + `gateway run` | `curl /messages` read of the room timeline |
| 2 | edit | production factory + `Channel::edit_message` | `curl` read-back: `m.replace` relation present AND the replacement is what a client renders |
| 3 | delete | production factory + `Channel::delete_message` | `curl` read-back of the **event body**, not the status code |
| 4 | receive | `gateway run` `/sync` loop | product-side arrival with non-empty content + the F-ML-1 both-direction control |
| 5 | restart idempotency | two separate `wayland-core` **processes**, same delivery key | count of events carrying the nonce in the room: must be exactly 1 |

Every observer check gets a known-negative in the same capture (an event id that does
not exist; a nonce never sent) so a dead observer cannot produce a green.

---

## 4. Log

- **T+0** worktree created at `43c69ca7`; brief and `docs/delivery-semantics.md` read.
- **T+10** source survey done; F-ML-1 and F-ML-2 recorded above **before** any run.
- **T+14** hetzner reachable, 967G free, HTTPS to matrix.org = 200.

---

## 5. Live results (running)

- **T+45** LEG 2 (edit) and LEG 3 (delete) **PASS** live against matrix.org, room
  `!REDACTED-MATRIX-ROOM`. Edit graded by the homeserver's own bundled
  `unsigned.m.relations.m.replace` on the ORIGINAL event; delete graded by read-back
  body (`state=REDACTED body_present=false`). Both have a working known-negative.
- **T+50** LEG 4 (inbound) **PASS**. `text_len=47`, sender preserved, through the
  production `/sync` loop and `ChannelManager::subscribe()`. Self-echo control:
  `MLR_INBOUND_SELF_ECHO_ADMITTED=false`.
- **T+58** LEG 1 (binary send) **FAILED — and it found a HIGH.** See F-ML-5.

### F-ML-3 (MEDIUM) — a Matrix redaction of a nonexistent event returns 200

`MLR_CONTROL_DELETE_BOGUS_ok=true`. Corroborated outside the product:
`curl PUT …/redact/%24absolutely-no-such-event-mlact/… → 200 {"event_id": …}`.

`rest.rs:342-349` states the operation "reports success when the homeserver accepted
the redaction, which is the strongest guarantee the protocol offers". Acceptance
guarantees nothing: it is returned for a target that never existed. An `Ok(())` from
`delete_message` therefore carries no information about whether anything was deleted.
The edit path is NOT symmetric — matrix.org rejects a relation to an unknown event
with `400 M_UNKNOWN`.

### F-ML-5 (HIGH) — every scheduled channel delivery addresses the CHANNEL NAME as the destination conversation

`crates/wcore-agent/src/cron.rs:137`:

```rust
let msg = OutgoingMessage::text(channel_name.to_string(), text.to_string());
```

`OutgoingMessage::text(conversation_id, text)` — so `conversation_id == channel_name`.
`Target::Channel` (`wcore-cron/src/job.rs:24`) carries only `{ channel_name, text }`:
there is **no conversation field**, and `cron add` has **no flag** that could supply
one (full flag census run; the only value-bearing flags are `--slash --channel --text
--skill --args --trigger --describe`).

Measured live, the shipped binary:

```
PUT /_matrix/client/v3/rooms/mxlive/send/m.room.message/cron:bd8831fa-…:1785384138000
403 M_FORBIDDEN "User @REDACTED-MATRIX-USER:matrix.org not in room mxlive"
```

`mxlive` is the channel's NAME. Ledger: `accepted → attempted → settled delivered:false`.

**Why this is not Matrix-specific.** The per-adapter fallbacks (`slack lib.rs:416`,
`whatsapp :238`, `sms :250`) fire only when `conversation_id.is_empty()`. Cron always
supplies a non-empty string, so **no adapter's configured default destination is
reachable from a cron delivery.** A scheduled channel message can only arrive if the
operator happens to have named the channel exactly the destination's conversation id.

**This is why the defect survived a live proof.** `24-C1-abandon-surface/f24c1-live3.sh`
counted real arrivals for `--channel f24c1csink` — because the destination was a
fixture sink that accepts any channel id. A real Slack would have answered
`channel_not_found`.

**Consequence for `docs/delivery-semantics.md`.** §4 scopes the exactly-once guarantee
to the delivery id `cron:{job}:{scheduled_millis}`, and cron is the only production
minter of those ids. So the exactly-once row for Matrix describes a path that, as
shipped, cannot address a Matrix room at all.

---

## 6. Final log

- **T+70** F-ML-5 fixed (`Target::Channel::conversation_id` + `cron add --conversation`).
  Binary rebuilt. LEG 1 re-run: `delivered:true`, room count 1, pre-fix nonce 0.
- **T+80** LEG 5 **PASS**: identical txn id across `kill -9`, identical `event_id` back,
  control under a different delivery id produced a second event, room = 2 not 3.
- **T+90** cleanup: 9 events redacted, all verified by read-back; `originals=0` with a
  live positive control. Secret sweep 0/0 with known-positive 1/1.
- **T+95** `cargo test -p wcore-cli --lib cron::` found five `cfg(test)` initializers the
  release build never compiled. Fixed; 7 passed 0 failed.

See `MATRIX-LIVE-SUMMARY.md` for the full result.
