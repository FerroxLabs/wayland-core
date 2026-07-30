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
bot_user_id`**. We hold exactly one account (`@seandonahoe:matrix.org`), so a probe
message posted by that account is discarded by design and can never reach the product.

Consequence for the inbound leg: the channel's configured `user_id` must name a
*different* mxid than the sender, or nothing arrives. That is a real configuration
(the token and the `user_id` field are independent inputs), and it is disclosed.
It also yields a control that runs in **both directions** in the same session:

- `user_id = @wayland-probe-not-sean:matrix.org` → the event MUST arrive;
- `user_id = @seandonahoe:matrix.org` (the true sender) → the same event MUST NOT
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
