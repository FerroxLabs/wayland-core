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

## Risk register (live)

- The instrument tends to carry the defect class it hunts (11 recorded instances). My fixture
  needs an explicit `instrument_fault` state grading suspect runs INCOMPLETE, not LOSS,
  self-tested 3 ways incl. "the old matcher would have missed it".
- A green by universal denial is the top trap: must count POSITIVE arrivals, zero arrivals
  must grade FAIL.
