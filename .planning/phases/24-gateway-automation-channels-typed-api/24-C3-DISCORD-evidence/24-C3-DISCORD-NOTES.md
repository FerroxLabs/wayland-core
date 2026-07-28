# 24-C3-DISCORD — working NOTES (append-only, re-committed after every measurement)

Lane `lane/24-c3-discord`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-c3-discord`.
BASE (merge-base, captured once) = `ef1d97beb61f1b084bdfba745e8f49830924d757`.

## Goal

Make Discord inbound drivable WITHOUT a vendor credential. Three prior lanes recorded that
this was impossible without "Sean's" real bot token. The brief asserts that is false. This
file records what I actually measure, as I measure it.

---

## M1 — the premise check (measured, minute ~10)

**VERDICT SO FAR: the brief's premise is CORRECT, and one part of it is already better than
the brief states.**

### M1a. Telegram mints its own token — no vendor account (CONFIRMED)

`scripts/f24-tg-fixture.mjs` — the fixture IS the API, so it accepts whatever token it
minted. Nothing about the credential is vendor-issued. Discord's `credential_handle` is the
same shape: an opaque string looked up in the credentials store and echoed back as
`Bot <token>`. Neither adapter validates the token's provenance; only the SERVER does, and
if we are the server, we decide.

### M1b. Discord's Rust-level seam ALREADY EXISTS (brief slightly understates this)

`crates/wcore-channel-discord/src/lib.rs:94` — `DiscordChannel::with_bases(name, config,
creds, api_base, gateway_base)` is already `#[doc(hidden)]` pub, and overrides BOTH the REST
base and the gateway base. The two hardcoded constants at lib.rs:50/52 are only the
DEFAULTS applied by `new()` at lib.rs:76-88.

So the missing piece is narrower and cleaner than "add a seam": the seam exists in Rust but
is **unreachable from a TOML config**, so the SHIPPED BINARY can only ever reach
`discord.com` / `gateway.discord.gg`. `f24-inbound.mjs` drives the real binary via config
files, so it cannot reach a fixture. That is the actual gap.

### M1c. The config-level precedent is 4-for-4; Discord is the only holdout

| adapter  | config field    | default fn                     |
|----------|-----------------|--------------------------------|
| telegram | `api_base_url`  | `default_api_base()` (config.rs:48-49, 56-58) |
| slack    | `api_base_url`  | config.rs:35                   |
| whatsapp | `api_base_url`  | config.rs:50                   |
| sms      | `api_base_url`  | config.rs:30                   |
| **discord** | **ABSENT**   | — `DiscordConfig` has only credential_handle / allowed_channel_ids / intents / heartbeat_grace_ms (config.rs:30-48) |

Telegram's own comment (config.rs:37-40) states the rationale verbatim and cites the same
sibling adapters. Discord needs TWO fields, not one, because its inbound is WS not REST:
`api_base_url` + `gateway_url`.

### M1d. `deny_unknown_fields` — the compatibility trap the brief flags

`config.rs:29` carries `#[serde(deny_unknown_fields)]`, and there is an existing test
`unknown_field_rejected` (config.rs:101) asserting it. Adding fields with `#[serde(default)]`
keeps old configs parsing (a missing field is not an unknown field). I must prove BOTH
directions: old config still parses AND defaults are unchanged (control test).

---

## M2 — what is still unestablished

- [ ] Does a WS gateway fixture fit in budget? (handshake/HELLO op10, IDENTIFY op2,
      heartbeat op1/op11, dispatch op0 MESSAGE_CREATE). This is the real work.
- [ ] How does `f24-inbound.mjs` wire telegram/slack? Must read before adding discord.
- [ ] Does Discord share the destructive-read loss mode (F24-C3-H4)? `poll_events` at
      lib.rs:238-240 is `inbox.lock().await.drain(..)` — a DESTRUCTIVE READ, same mechanism.
      Whoever drains first wins; a second reader gets nothing. Needs a steady-state leg.

---

## M3 — the config seam LANDED and PROVEN (commit `1770f0d9`)

Added `api_base_url` + `gateway_url` to `DiscordConfig`, both `#[serde(default)]`, and made
`DiscordChannel::new` consume them (it previously hardcoded the two constants). Updated
`src/schemas/discord.json` (it carries `additionalProperties: false`, so the schema had to
learn the fields too).

### Gate results (hetzner `hz/24-c3-discord` @ `1770f0d9`, isolated targeted runs)

| run | command | result |
|-----|---------|--------|
| fmt | `cargo fmt --all -- --check` (Mac) | rc=0, **0 bytes** output |
| unit | `cargo test -p wcore-channel-discord` | **54 passed / 0 failed / 0 ignored** |
| registry | `cargo test -p wcore-channels-registry` | **11 passed / 0 failed / 0 ignored** |

All 5 new tests confirmed EXECUTED BY NAME (not merely "suite green"):
`control_absent_keys_still_reach_production_discord`,
`backcompat_a_preexisting_full_config_still_parses`,
`both_bases_are_independently_overridable`,
`new_honours_the_config_bases_so_the_shipped_path_is_redirectable`,
`control_new_with_a_default_config_still_points_at_production`.

### M3a. The gate CAN fail — mutation-proven, not assumed

Reverted `new()` to the hardcoded constants and re-ran:

```
MUTATED_RC=101
tests::new_honours_the_config_bases_so_the_shipped_path_is_redirectable ... FAILED
tests::control_new_with_a_default_config_still_points_at_production ... ok
test result: FAILED. 53 passed; 1 failed
```

This is the discriminating result I wanted: the seam test went red, and the production-default
control stayed GREEN (correctly — the mutation preserves the production default). A mutation
that reddened both would have meant my control was not actually a control. File restored;
`git status --porcelain | wc -l` = 0.

### M3b. Trap encountered and confirmed, first hand

`echo "FMT_EXIT=${PIPESTATUS[0]}"` after a pipeline printed **`FMT_EXIT=`** (empty), exactly as
the brief warns. Cause: this shell is **zsh**, where the array is `$pipestatus` and is
1-indexed; `PIPESTATUS` is a bash-ism and expands to nothing. Every exit status in this lane is
therefore taken from an unpiped command via `$?` written to a variable on the same line, or from
a file. I did not use `${PIPESTATUS[0]}` again.

### M3c. LOW finding (BACKLOG, not fixed — out of scope per AGENTS.md §3)

`src/schemas/discord.json` declares `"intents": {"default": 33792}`, but the code's
`DEFAULT_INTENTS` is **37376** (config.rs:23-24, after DIRECT_MESSAGES bit 12 was added). The
shipped schema's advertised default has drifted from the real one. Descriptive-only (serde
supplies the real default, and `default_intents()` is what actually runs), so LOW → BACKLOG.
I did not fix it: it predates this change and is not required by it.

---

## M4 — fixture scope decision: BUILD IT (measured surface, not a guess)

Read the exact protocol surface the Rust client requires before estimating. It is small:

| need | source | size |
|------|--------|------|
| RFC6455 server handshake + frame codec | hand-rolled, zero npm deps | ~120 lines |
| `op=10 HELLO` on connect | gateway.rs:264 reads `d.heartbeat_interval` | trivial |
| accept `op=2 IDENTIFY` | gateway.rs:216 `identify_frame` — `{token,intents,properties}` | trivial |
| `op=0 t="READY"` | gateway.rs:280-283; `d.session_id` required, `resume_gateway_url` optional | trivial |
| `op=1` → `op=11 HEARTBEAT_ACK` | gateway.rs:232 `heartbeat_frame` | trivial |
| `op=0 t="MESSAGE_CREATE"` | gateway.rs:81-109; **only `id` + `channel_id` are required**, everything else `#[serde(default)]` | trivial |

`with_gateway_query()` appends `?v=10&encoding=json`, and the client accepts plain `ws://`
(existing unit tests already dial `ws://127.0.0.1:1`). No TLS needed. **Well inside budget** —
so the brief's fallback ("land the seam and say so") is NOT the outcome; I am building it.

One-server design: WS is an HTTP Upgrade, so a single Node server serves BOTH
`api_base_url=http://127.0.0.1:PORT` (REST: `/users/@me`, `POST /channels/{id}/messages`) and
`gateway_url=ws://127.0.0.1:PORT`. This is only possible because the seam has two independent
fields.

### M4a. The consumption race has a DIFFERENT SHAPE on Discord — and the naive port would measure nothing

This is the most important thing I have worked out, and getting it wrong would have produced a
confident false green.

Telegram is POLLING with a destructive server-side read: two `ChannelManager`s poll one bot
token, and whoever calls `getUpdates` first CONSUMES the update. The other gets nothing.
Symptom = **LOSS**. Instrument = `max_concurrent_getupdates`.

Discord is PUSH over a per-connection session. Two `ChannelManager`s do NOT contend for one
queue — each builds its own `DiscordChannel` with its own `inbox` (`lib.rs:63`) and its own WS
connection, and Discord delivers MESSAGE_CREATE to **every** connected session. So the same
root cause (double manager) produces the **opposite** symptom: **DUPLICATION**, not loss.

`poll_events` (`lib.rs:238-240`) is `inbox.lock().await.drain(..)` — destructive — but the
inbox is per-instance and not shared between managers, so it is not the contention point that
`getUpdates` was.

Consequences for the instrument, both of which I have built in:
1. The Discord analogue of `max_concurrent_getupdates` is **concurrent gateway connections
   authenticated with the same bot token**. 2 = the double-manager defect reaches Discord.
   0 = a runtime that connected nothing, which must be a DISTINCT and FAILING answer so a
   "fix" that works by starting nothing cannot pass.
2. Loss and duplication are graded and reported **separately**. A driver that only counted
   loss would report Discord CLEAN under the very defect it was built to find.

---

## M5 — fixture + instrument built and self-tested (commit `292bd38a`)

`scripts/f24-discord-fixture.mjs` (WS gateway + REST, hand-rolled RFC6455, zero npm deps),
`scripts/f24-discord-inbound.mjs` (6-leg driver), `scripts/f24-discord-selftest.mjs`.

**Self-test: `passed=13 failed=0`, rc=0.** Proven working against a hand-rolled WS client in a
separate socket: HELLO → IDENTIFY → READY → HEARTBEAT_ACK completes, MESSAGE_CREATE dispatches
with the two fields the Rust decoder actually requires (`id`, `channel_id`), a non-minted token
yields op9 and is COUNTED as an auth failure rather than dropped.

### M5a. HIGH (instrument, mine, found AND fixed in-lane) — `check()` reported async failures as passes

Writing the self-test I put one test (`fixture C`) behind an `async` arrow. `check()` was
`try { fn(); passed += 1 }`. An async fn's assertion failure REJECTS rather than throws, so
`check` saw no exception, printed `ok`, and incremented `passed`.

Measured on node v22, both shapes:

| shape | reported | rc |
|-------|----------|-----|
| no trailing `process.exit` | `ok` + `passed=1 failed=0`, then unhandled-rejection crash | 1 (AFTER a green summary) |
| **with** the trailing `process.exit(failed===0?0:1)` this file actually had | `ok` + `passed=1 failed=0` | **0** |

So the exact shape I had shipped reported a **deliberately false assertion** as a pass with a
**zero exit status** — completely silent. A self-passing gate living inside the file whose job
is to prove the other instruments cannot self-pass.

**Repaired structurally, not locally.** I did not just make `fixture C` synchronous — `check()`
now hard-fails on any thenable, so a future async test cannot reintroduce it. Per §6b-ii the
repair carries its own three assertions (`harness A/B/C` + `-verify` partners): a passing sync
test still reports ok; a false sync test is counted; and **the async-false test is now counted
as a FAILURE and specifically NOT as a pass** — that last one is the assertion that proves the
guard does anything, since the pre-repair harness incremented `passed` and exited 0 there.

The two `FAIL harness B/C (expected FAIL)` lines in the output are deliberate, and each is
verified by its `-verify` partner; the intentional failures are un-counted afterwards.

## Risk register (live)

- The instrument tends to carry the defect class it hunts (11 recorded instances). My fixture
  needs an explicit `instrument_fault` state grading suspect runs INCOMPLETE, not LOSS,
  self-tested 3 ways incl. "the old matcher would have missed it".
- A green by universal denial is the top trap: must count POSITIVE arrivals, zero arrivals
  must grade FAIL.
