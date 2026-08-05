# 24-C3-DISCORD — Discord inbound, driven without a vendor credential

**Verdict: GOAL ACHIEVED.** Discord inbound is now drivable end to end with no vendor
credential, all six legs PASS live against the shipped binary, and the lane found and fixed a
HIGH product defect that had made the Discord gateway handshake malformed for the whole of
Phase 24.

Lane branch `lane/24-c3-discord`. Merge-base `ef1d97beb61f1b084bdfba745e8f49830924d757`.
Not merged, no PR, as instructed.

---

## 1. The premise was correct, and three prior lanes were wrong

Three lanes recorded that Discord inbound could only ever be proven with a real bot token
belonging to a human. That is false. `scripts/f24-tg-fixture.mjs` already mints Telegram's token
at run time and the fixture accepts it **because the fixture is the API**; Discord's
`credential_handle` is the same shape — an opaque string looked up in the credentials store and
echoed as `Bot <token>`. Only the SERVER validates it, and if we are the server, we decide.

One correction to the brief's framing, in the product's favour: a Rust-level seam
(`DiscordChannel::with_bases`, `lib.rs:94`) already existed. It is `#[doc(hidden)]` and reachable
only from in-process unit tests, and `wcore-channels-registry` builds the shipped adapter through
`new()`, which hardcoded both constants. So the gap was narrower and sharper than "no seam": the
seam existed but **no out-of-process harness could reach it**, which is why `f24-inbound.mjs`
(which drives the real binary from config files) could never point Discord anywhere.

---

## 2. What landed

### 2a. The config seam — `DiscordConfig::{api_base_url, gateway_url}`

Discord was the only adapter with no config-level base URL. Telegram, Slack, WhatsApp and SMS
all carry `api_base_url`; Discord carried none. It needs **two** fields where they need one,
because its inbound arrives over a WebSocket rather than by polling REST — redirecting
`api_base_url` alone would send outbound to a fixture while leaving inbound on production, the
half-configured state that makes a fixture run look green while measuring nothing.

Both fields are `#[serde(default)]`, so pre-existing configs still parse under
`deny_unknown_fields` (a missing field is not an unknown field). `new()` now consumes them, and
`src/schemas/discord.json` learned them too (it carries `additionalProperties: false`).

**The control test is the load-bearing one:** a config naming neither key must still reach
production Discord. Proven both from a struct literal and from real TOML.

### 2b. HIGH product defect — the gateway connect URL had a query but no path

`with_gateway_query` was `format!("{base}?v=10&encoding=json")` over a base with no trailing
slash, producing `wss://gateway.discord.gg?v=10&encoding=json`. The module docs at `lib.rs:11`
have **always** documented the connect URL as `wss://gateway.discord.gg/?v=10&encoding=json`,
with the slash — the code disagreed with its own documentation.

`tokio_tungstenite::connect_async` converts the string to an `http::Uri`, and unlike the `url`
crate `http::Uri` does **not** normalise an empty path to `/`. The handshake request-target came
out as `GET ?v=10&encoding=json HTTP/1.1`, which is not a valid request-target. Measured: every
connect attempt failed `400 Bad Request`, and the fixture's parser reported `HPE_INVALID_URL`.

Fixed via `ensure_path()` with two regression tests. **This was invisible for all of Phase 24
precisely because Discord inbound had never once been driven end to end** — it is the defect the
lane existed to find.

*Honest limit:* I cannot test against real Discord (no credential — that is the point), so I
cannot say whether Discord's production edge tolerated the malformed target. Against a strict
server it is fatal; the fix makes the code agree with its own documentation either way.

### 2c. The fixture, driver and self-test

- `scripts/f24-discord-fixture.mjs` — Gateway WebSocket + REST fixture. Hand-rolled RFC6455
  (zero npm deps: adding a dependency to run a test is a supply-chain decision). Runs as its own
  OS process with a `/__control/` API.
- `scripts/f24-discord-inbound.mjs` — the six-leg driver.
- `scripts/f24-discord-selftest.mjs` — **16 passed / 0 failed**, on macOS and Linux.

**Fixture coverage (implemented):** `op10 HELLO`, `op2 IDENTIFY`, `op0 READY`,
`op1`/`op11` heartbeat, `op6 RESUME` with replay, `op0 MESSAGE_CREATE` dispatch; REST
`/users/@me`, create-message, typing, reactions. It mints AND enforces its own token — a fixture
that accepted anything would pass an adapter sending no credential at all, which is a green by
universal *acceptance*.

**Declared limits (not implemented):** zlib/ETF compression, shard negotiation, rate-limit
buckets, guild/permission state, voice, `op9` invalid-session negotiation. These are reported in
the result JSON's `fixture_coverage` rather than left for a reader to infer from a green.

---

## 3. Live results — the shipped binary, no vendor credential

`wayland-core 0.12.25` built on hetzner from lane HEAD and driven through `gateway run`.

| run | config | verdict | legs | arrivals | llm turns | conns_max | steady |
|-----|--------|---------|------|----------|-----------|-----------|--------|
| 6 | 6 steady, 15s settle | **PASS** rc=0 | 6/6 | 12 | 12 | 1 | 6 submitted, **0 lost, 0 dup** |
| 7 | `--steady 12 --settle-ms 30000` | **PASS** rc=0 | 6/6 | 18 | 18 | 1 | 12 submitted, **0 lost, 0 dup** |

Per-leg (both runs): `admit` 1 reply exactly; `dedupe` replay produced no second reply with the
different-id positive control at 1; `access` stranger 0 with allowed control 1; `bind` A→900000001
and B→900000002; `route` A's reply carried A's token and did not leak B's; `steady` all answered.

**Two independent journals agree exactly.** `arrivals_total` comes from the fixture's REST
journal — a different OS process the binary cannot write to except by completing a real TCP round
trip — and `llm_turns` from the model fixture's separate journal: 12/12 and 18/18.

**Internal consistency check:** `dispatched_total − 2 = arrivals` in both runs (14−2=12,
20−2=18). The two non-arriving dispatches are exactly the ones that must not produce a reply (the
dedupe replay and the access-denied stranger). Had either leaked, this identity would break.

### Does Discord share the destructive-read loss mode? **No.**

The race has the **opposite shape** on Discord, and a naive port of the Telegram instrument would
have measured nothing. Telegram polls a destructive server-side queue, so two `ChannelManager`s
mean one STEALS ⇒ **loss**. Discord is pushed per session: two managers would open two sockets
and Discord delivers to **both** ⇒ **duplication**. `poll_events` is a destructive `drain()`
(`lib.rs:238-240`) but the inbox is per-instance and not shared between managers, so it is not
the contention point `getUpdates` was.

Evidence that Discord is clean:
- `max_concurrent_gateway_connections = 1` and `total_gateway_connections = 1` — the
  double-`ChannelManager` defect does not reach Discord; only one authenticated socket ever
  existed on the token.
- `dispatch_socket_deliveries == dispatched_total` (14/14, 20/20) — strictly 1:1, no duplication.
- **Steady state: 0 lost of 6, and 0 lost of 12 after a 30s settle** — the leg that lifted the
  Telegram finding from MEDIUM to HIGH, run harder here than there.

That is a negative result, and it is only worth anything because the instrument was proven able
to report the positive.

---

## 4. The gate can fail — mutation-proven

Mutated the fixture to deliver every dispatch to **zero** sockets (total inbound loss, connection
still healthy):

```
MUT_RUNRC=1  verdict=FAIL  legs=0/6  steady lost=4/4  conns_max=1  instrument_fault=no
```

Two things make this a real falsification:

1. **The universal-denial trap fired and was caught.** The access leg's primary condition
   (`stranger_replies=0`) was *satisfied* under total loss — that is exactly what a green by
   universal denial looks like. Its in-run positive control (`allowed_control=0`, want 1) is what
   failed it. Without that control, access would have scored PASS on a totally broken run.
2. `conns_max` stayed 1, so the instrument attributed the failure to DELIVERY, not to the
   connection — the two are separately observable by design.

Fixture restored; working tree clean afterwards.

Five further failing runs each produced a **different and correct** diagnosis, which discriminates
better than a single red: run 2 "never opened a TCP connection — NOT MEASURED"; run 3 "DIALLED
(241 TCP) but no WS handshake completed — protocol/URL fault, NOT inbound loss" with
`HPE_INVALID_URL`; run 4 "admitted and ROUTED but the turn failed downstream — NOT inbound loss";
run 5 INCOMPLETE (not a false PASS and not a false LOSS).

---

## 5. Instrument defects found and FIXED in this lane (§6b-ii)

Four, all repaired here rather than written up and left — a documented instrument defect is a
defect you have agreed to keep.

1. **`check()` reported async failures as passes.** One self-test was written `async`; an async
   assertion failure *rejects* rather than throws, so `check` saw no exception and incremented
   `passed`. Measured on node v22: with the trailing `process.exit(failed===0?0:1)` this file
   actually had, a **deliberately false assertion printed `ok`, printed `passed=1 failed=0`, and
   exited 0** — fully silent. A self-passing gate inside the file whose job is to prove nothing
   else self-passes. Repaired *structurally* (any thenable now hard-fails), not by making the one
   test sync.
2. **In-process fixture + `Atomics.wait`.** Every driver here sleeps with `Atomics.wait`, which
   blocks the whole Node event loop, so an in-process fixture cannot accept a single TCP
   connection while the driver waits. Runs 2 and 3 reported "the binary never connected" — a
   PRODUCT defect — when the instrument simply was not listening. A `RUST_LOG=…=trace` run proved
   the adapter had been dialling correctly all along. This is why every sibling fixture is spawned
   as its own process; mine now is too.
3. **`tcp_connections` could not distinguish "nothing dialled" from "handshake refused"** — both
   read 0, which are opposite diagnoses. Fixed, and that counter then diagnosed the HIGH URL
   defect in a single run.
4. **The mangling detector flagged all 12 healthy replies** (run 5 graded INCOMPLETE despite 6/6
   legs green). It re-derived the token by regex from the NORMALIZED text, whose whitespace had
   already been stripped, so `F24C3-REPLY f24c3-disc-admit-bf89` collapsed to one unbroken run and
   the regex swallowed the marker. Fixed by judging against the tokens actually planted.

Each repair carries the **three**-assertion self-test the brief requires: known-positive passes,
known-negative fails, **and the old broken instrument would have missed it**. That third
assertion is the only one that proves a repair does anything.

The correlation matcher itself was repaired pre-emptively rather than reactively: Discord sends
`content` raw today, so the naive `includes()` would happen to work *right now* — but it breaks
silently the moment anything escapes, wraps or splits the text, including Discord's own 2000-char
cap cutting a token in half.

---

## 6. Gate results (real numbers, read back)

| gate | command | result |
|------|---------|--------|
| fmt | `cargo fmt --all -- --check` (Mac) | rc=0, **0 bytes** |
| discord unit | `cargo test -p wcore-channel-discord` | **56 passed / 0 failed / 0 ignored** |
| registry | `cargo test -p wcore-channels-registry` | **11 passed / 0 failed / 0 ignored** |
| self-test (Linux) | `node scripts/f24-discord-selftest.mjs` | **16 passed / 0 failed**, rc=0 |
| live inbound | `f24-discord-inbound.mjs` | **PASS 6/6**, rc=0, twice |
| seam mutation | `new()` reverted to constants | rc=101, seam test red, control green |
| loss mutation | dispatch to zero sockets | rc=1, FAIL 0/6, lost=4/4 |

Executed counts are asserted, not inferred from exit status; all five new seam tests were
confirmed present by name. Isolated targeted runs, not a full-workspace run under lane contention.

---

## 7. Findings for the phase

- **HIGH (FIXED):** Discord gateway connect URL had a query and no path, producing an invalid
  HTTP request-target. Fixed in `with_gateway_query`/`ensure_path` with regression tests. Cannot
  be verified against production Discord from here.
- **NEGATIVE (established):** Discord does **not** share Telegram's inbound loss mode, and shows
  no loss or duplication mode of its own across 18 messages including steady state after a 30s
  settle. `max_concurrent_gateway_connections=1` shows the double-`ChannelManager` defect does not
  reach it.
- **LOW (BACKLOG, not fixed):** `src/schemas/discord.json` advertises `"intents": {"default":
  33792}` but `DEFAULT_INTENTS` is **37376** (config.rs:23-24, after DIRECT_MESSAGES bit 12 was
  added). Descriptive only — serde supplies the real default — and it predates this change, so
  out of scope per AGENTS.md §3. Worth a one-line fix by whoever owns the schema.

## 8. What I did NOT do

- Did not merge, open a PR, tag, or close anything.
- Did not touch `crates/wcore-cli/src/lib.rs` or `main.rs` — fence diff vs the merge-base SHA is
  **0 files**.
- Did not run `wcore-contract generate`; no contract change was needed.
- Did not test against real Discord, and did not obtain, read or embed any vendor credential.
- Did not run a full-workspace build (targeted `-p` runs only, per §2).
- Did not implement compression, sharding, rate-limit buckets, guild state or voice in the
  fixture; those limits are declared in `fixture_coverage` in every result file.

## 9. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-C3-DISCORD-evidence/`
— `24-C3-DISCORD-NOTES.md` (append-only working record), `run6-result.json`, `run7-result.json`,
`mutation-total-loss-result.json`, `selftest-linux.log`.
