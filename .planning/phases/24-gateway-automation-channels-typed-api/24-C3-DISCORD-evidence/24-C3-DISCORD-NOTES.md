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

---

## M6 — HIGH PRODUCT DEFECT FOUND AND FIXED: the gateway URL had no path

Once the fixture could listen, every connect attempt failed `400 Bad Request`, and the
fixture's parser reported `HPE_INVALID_URL`.

Root cause: `with_gateway_query` was `format!("{base}?v=10&encoding=json")`, and
`DISCORD_GATEWAY_BASE` is `wss://gateway.discord.gg` with no trailing slash — so the connect URL
was `wss://gateway.discord.gg?v=10&encoding=json`, **a query with no path**. The module docs at
`lib.rs:11` have always documented the connect URL as
`wss://gateway.discord.gg/?v=10&encoding=json`, WITH the slash: the code disagreed with its own
documentation.

Why it matters: `tokio_tungstenite::connect_async` converts the string to an `http::Uri`, and
unlike the `url` crate, `http::Uri` does **not** normalise an empty path to `/`. The handshake
request-target came out as `GET ?v=10&encoding=json HTTP/1.1`, which is not a valid
request-target. A strict server rejects it outright.

Fixed by `ensure_path()` + two regression tests. **This is exactly the class of defect the lane
existed to find, and it was invisible for the whole of Phase 24 because Discord inbound had
never once been driven end to end.**

Honest limit: I cannot test against real Discord (no credential, and that is the point), so I
cannot say whether Discord's production edge tolerated the malformed target. Against a strict
server it is fatal, and the fix makes the code agree with its own documentation either way.

## M7 — LIVE RESULT: all six legs PASS, twice, no vendor credential

Binary `wayland-core 0.12.25` built on hetzner from lane HEAD; `gateway run` driven end to end.

| run | config | verdict | legs | arrivals | llm turns | conns_max | steady |
|-----|--------|---------|------|----------|-----------|-----------|--------|
| 6 | default (6 steady, 15s settle) | **PASS** rc=0 | 6/6 | 12 | 12 | 1 | 6 submitted, 0 lost, 0 dup |
| 7 | `--steady 12 --settle-ms 30000` | **PASS** rc=0 | 6/6 | 18 | 18 | 1 | 12 submitted, 0 lost, 0 dup |

Two INDEPENDENT journals agree exactly: `arrivals_total` (the fixture's REST journal, written by
a different OS process the binary cannot write to except by completing a real TCP round trip)
equals `llm_turns` (the model fixture's separate journal) — 12/12 and 18/18.

Internal consistency: `dispatched_total − 2 = arrivals` in BOTH runs (14−2=12, 20−2=18). The two
non-arriving dispatches are exactly the ones that MUST not produce a reply: the dedupe replay and
the access-denied stranger. A run where those two had leaked would break this identity.

### M7a. Does Discord share the destructive-read loss mode? NO — and here is the evidence

- **`max_concurrent_gateway_connections = 1`** in both runs, and `total_gateway_connections = 1`.
  The double-`ChannelManager` defect does **not** reach Discord: only one authenticated socket
  ever existed on the bot token.
- **`dispatch_socket_deliveries == dispatched_total`** (14/14, 20/20) — 1:1. No message was ever
  delivered to two sockets, so there is no duplication, which is the shape the defect WOULD take
  on a push transport.
- **Steady state: 0 lost of 6, and 0 lost of 12 after a 30s settle.** This is the leg that lifted
  the Telegram finding from MEDIUM to HIGH, run harder here than there.

So Discord shows **no loss mode of its own** and is not affected by the Telegram one. That is a
negative result, and it is only worth anything because the instrument was proven able to report
the positive — see M8.

## M8 — THE GATE CAN FAIL (mutation-proven, not asserted)

Mutated the fixture so `dispatchMessage` delivers to **zero** sockets — total inbound loss with
the connection still healthy — and re-ran:

```
MUT_RUNRC=1   verdict=FAIL   legs=0/6   steady lost=4/4   conns_max=1   instrument_fault=no
```

The two things that make this a real falsification rather than a formality:

1. **The universal-denial trap fired and was caught.** The access leg's primary condition
   (`stranger_replies=0`) was *satisfied* under total loss — that is what a green by universal
   denial looks like. Its in-run POSITIVE CONTROL (`allowed_control=0`, want 1) is what failed
   it. Without that control this run would have scored access as a PASS.
2. **`conns_max` stayed 1**, so the instrument correctly attributed the failure to DELIVERY and
   not to the connection — the two are separately observable, as designed.

Fixture restored afterwards; `git status --porcelain | wc -l` = 0.

Five further independent failing runs (2, 3, 4, 5 and the mutation) each produced a DIFFERENT and
correct diagnosis, which is stronger evidence of discrimination than a single red:

| run | what was wrong | what the instrument said |
|-----|----------------|--------------------------|
| 2 | fixture in-process, event loop blocked | "never opened a TCP connection — NOT MEASURED" |
| 3 | gateway URL had no path | "DIALLED (241 TCP) but no WS handshake completed — protocol/URL fault, NOT inbound loss", `HPE_INVALID_URL` |
| 4 | no unlocked vault on headless host | "admitted and ROUTED but the turn failed downstream … NOT inbound loss" |
| 5 | mangling detector false-positived | INCOMPLETE (not a false PASS, and not a false LOSS) |
| mut | total inbound loss | FAIL, lost=4/4 |

## M9 — three further instrument defects, all found and FIXED in-lane (§6b-ii)

1. **In-process fixture + `Atomics.wait`.** Every driver here sleeps with `Atomics.wait`, which
   blocks the whole Node event loop, so an in-process fixture cannot accept a TCP connection
   while the driver waits. Runs 2 and 3 reported "the binary never connected" — a PRODUCT defect
   — when the instrument simply was not listening. A `RUST_LOG=…=trace` run proved the adapter
   had been dialling correctly all along. Fixed by running the fixture as its own OS process with
   a `/__control/` API, which is why every sibling fixture is spawned separately.
2. **`tcp_connections` could not distinguish "nothing dialled" from "handshake refused".** Both
   read as 0 because only completed upgrades were counted. Fixed; this is the counter that then
   diagnosed M6 in one run.
3. **The mangling detector flagged all 12 healthy replies** (run 5 → INCOMPLETE despite 6/6 legs
   green). It re-derived the token by regex from the NORMALIZED text, whose whitespace had
   already been stripped, so `F24C3-REPLY f24c3-disc-admit-bf89` collapsed into one unbroken run
   and the regex swallowed the marker too. Fixed by judging against the tokens actually planted,
   with the three-assertion self-test including the proof that the old detector produced the
   false positive.

Self-test now **16 passed / 0 failed** on both macOS and Linux.

Also worth recording: my own `pkill -f "wayland-core gateway"` killed the ssh shell running it,
because the command line *contains* the pattern — the identical trap `f24-inbound-run.sh` was
written to avoid. Cost one run.

## Risk register (live)

- The instrument tends to carry the defect class it hunts (11 recorded instances). My fixture
  needs an explicit `instrument_fault` state grading suspect runs INCOMPLETE, not LOSS,
  self-tested 3 ways incl. "the old matcher would have missed it".
- A green by universal denial is the top trap: must count POSITIVE arrivals, zero arrivals
  must grade FAIL.
