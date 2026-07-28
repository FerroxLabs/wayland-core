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

## Risk register (live)

- The instrument tends to carry the defect class it hunts (11 recorded instances). My fixture
  needs an explicit `instrument_fault` state grading suspect runs INCOMPLETE, not LOSS,
  self-tested 3 ways incl. "the old matcher would have missed it".
- A green by universal denial is the top trap: must count POSITIVE arrivals, zero arrivals
  must grade FAIL.
