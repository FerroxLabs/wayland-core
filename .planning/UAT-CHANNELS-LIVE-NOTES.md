# UAT-CHANNELS-LIVE — running notes (append-only, committed continuously)

Lane: `lane/uat-channels-live`. Base integration commit: `e9bed1af931f02aea094469d44eed291af0c4c96`.

Goal: drive Slack / Discord / Matrix **as a first-time user would**, through the shipped
release binary — configure from nothing, start the gateway, send a message from the real
platform client, get a real agent reply back on that platform. Adapter-level proofs already
exist; the *product journey* does not.

## T+0 — setup

- Mac worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-uat-channels-live`,
  `git rev-parse --show-toplevel` asserted, HEAD `e9bed1af…`.
- hetzner worktree: `/root/wayland-uatlive`, branch `hz/uat-channels-live`, HEAD asserted
  `e9bed1af931f02aea094469d44eed291af0c4c96` (matches Mac). `df -h /root` → 995G free.
- Release build started on hetzner: `cargo build --release --locked -p wcore-cli`,
  log `/root/wayland-uatlive-build.log`, rc sentinel `/root/wayland-uatlive-build.rc`
  (`WLRC=<code>` then `WLDONE`).
- Note: lane `uat-tui-unix` is concurrently building the same target in
  `/root/wayland-uat-tui-unix`. I use **my own** binary, not theirs (attribution).

### Where the live calls must run — decided, not assumed

The binary is Linux x86_64 (hetzner is the only permitted build host; the Mac may not build).
A Linux binary cannot run on the Mac. **Therefore every live platform call in this lane runs
on hetzner**, and the channel credentials must reach hetzner. That is the LANE-BRIEF §0
sanctioned exception: stdin-only injection, never argv, never a log, never a commit, swept
afterwards. Disclosed in the report.

Caveat already known (LANE-BRIEF §3b-ii): `/root/.wayland/.env` on hetzner injects
`ANTHROPIC_API_KEY` into the product regardless of the shell environment. Any claim about
*which provider produced the reply* must be read back out of the product's own output.

## Secrets inventory (names only — no values anywhere in this repo)

| file | keys present |
|---|---|
| `~/.wayland-secrets/slack.env` | `SLACK_BOT_TOKEN`, `SLACK_SIGNING_SECRET` |
| `~/.wayland-secrets/discord.env` | `DISCORD_BOT_TOKEN` |
| `~/.wayland-secrets/matrix.env` | `MATRIX_ACCESS_TOKEN`, `MATRIX_USER_ID`, `MATRIX_ROOM_ID`, `MATRIX_HOMESERVER` |
| `~/.wayland-secrets/flux.env` | `FLUX_API_KEY` |

## Premise checks against the brief (LANE-BRIEF §"your brief's MEASUREMENTS are probably stale")

To verify at HEAD, not assume:

- [ ] `map_message_create` at `gateway.rs:322-325` drops bot authors.
- [ ] `channel health` exits 0 while `Degraded`.
- [ ] Inbound is fail-closed with an empty DM allowlist by default.
- [ ] The documented configuration path in `docs/channels.md` actually works end to end.

Status: see below.

## T+40 — binary identity (hetzner build complete)

```
host      hetzner-dsm (Ubuntu 24.04, x86_64)
path      /root/wayland-uatlive/target/release/wayland-core
build     cargo build --release --locked -p wcore-cli   (WLRC=0, WLDONE present)
version   wayland-core 0.12.25
sha256    54d11b191b6d26232c28c419ab5210d5aecfbc2eb1c34d69bf3448f2430fe6c7
size      98434984 bytes
```

## T+40 — source reading, before any live run

### PREMISE 1 — CONFIRMED, and worse than the brief said

`crates/wcore-channel-discord/src/gateway.rs` `map_message_create` (`author_is_bot` →
`return None`) drops **every** bot author unconditionally. The brief's line numbers are
right for the region (295+28 ≈ 323).

The sharper point: `bot_id` (the receiving bot's own user id, from READY) is **already a
parameter of this function** and is used for `is_self` and mention detection — so a
loop guard scoped to *self* was available and the code chose the blanket bot filter
instead. Consequence for this lane: the Discord inbound leg cannot be driven by any
credential we hold (we have a bot token, not a user token), so it is UNRUN unless a
human types.

### PREMISE 3 — CONFIRMED in source

`InboundPolicy::default()` = `dm: Allowlist` + empty `dm_allowlist`, `group: Disabled`,
`require_mention: true`, `tools: Conversational`. `decide_access` denies both a DM and a
group message under that default (`access.rs`). Live confirmation still owed.

### NEW FINDING (source-level, to be confirmed live) — the documented config surface is not the working one

- `docs/channels.md` documents **only** the `[inbound]` table. It never shows `name`,
  `platform`, or `[options]` — so a first-time user reading the one doc named "Channels"
  cannot write a config file that loads at all.
- `wcore-channels/src/config.rs` documents a `[secrets]` table whose values are
  `keychain:<service>:<account>` references. **Nothing resolves them.** Grepped the whole
  `crates/` tree for `keychain:`: 10 hits, all doc comments and test fixtures. The only
  consumer of `cfg.secrets` (`wcore-channels-registry/src/lib.rs:459`) reads the **key
  names** for a report and never the values.
- The credential actually used is `credential_handle` (Discord), `credential_handle_bot_token`
  / `credential_handle_signing_secret` (Slack), `credential_handle_access_token` (Matrix),
  all inside `[options]`, resolved through `wcore_config::credentials::CredentialsStore`.
- **There is no CLI verb that writes a channel credential into that store.** `wayland-core
  --help` lists `channel {list,probe,health,reload}` — no `set`/`add`. `auth` is explicitly
  *provider* API keys only. The whole `wcore-cli` tree contains exactly **one** `.put(` call
  (`tui/engine_bridge.rs:2390`, a provider OAuth token). So the documented-and-shipped path
  for a channel credential is: hand-write `$WAYLAND_HOME/credentials.toml`.
  `docs/channels.md` itself lists "a setup doctor / token-probe CLI" under *Not yet built*.

### Home resolution (matters for isolating this test)

`wayland_config_dir()` honours `WAYLAND_HOME` for `config.toml` **and** `credentials.toml`
(`credentials_storage_path()` = `app_config_dir()/credentials.toml`), and
`wcore_gateway::resolve_home()` honours it for `channels/`. So a single
`WAYLAND_HOME=/root/wl-uatlive-home` isolates the whole test from `/root/.wayland`.

### Access-control test design (both directions), derived from `decide_access`

`ChatType::{Group,Channel}` → `policy.group`; `ChatType::Direct` → `policy.dm`.
Slack `C0BLR1UKKU6` → `ChatType::Channel` (id prefix `C`). Matrix room → `Direct` iff the
room summary reports exactly 2 joined members, else `Group`.

Matrix runs against **Sean's personal account**, and `MatrixConfig` has **no room filter** —
the adapter syncs every room the account is in. The containment is therefore the access
policy itself: `group = "allowlist"` + `group_allowlist = [test room]` +
`sender_allowlist = [Sean's MXID]`, `dm = "disabled"`. That is simultaneously the safety
fence and the ALLOW arm of the access test; the DENY arm flips one list entry.

## T+1h — credential reality check (read-only, from the Mac; every probe carries a control)

| platform | identity the credential authenticates as | live result |
|---|---|---|
| Slack | bot `wayland_core_test` `U0BLBKR56NT` (bot_id `B0BMMB78XEU`), team `Trade Canyon, Inc.` `T3Q7ANTRU` | `auth.test ok:true` |
| Discord | bot `WaylandCoreBot` `1532224324075913297`, `bot:true` | `GET /users/@me` **200** |
| Matrix | — | **`M_UNKNOWN_TOKEN` — "Token is not active"** |
| Flux | — | `GET /v1/models` **200**, 77 models |

Controls run in the same invocations: Slack unauth → `not_authed`; Discord unauth → **401**;
Matrix unauth → `M_MISSING_TOKEN` (a *different* error, so the header IS being sent) and
`/_matrix/client/versions` unauth → **200** with a real payload. The Matrix instrument is
therefore alive in both directions and the token is genuinely revoked. `token_len=41`, no
stray quotes, no whitespace — not an `.env` parsing artifact.

### Consequence for the whole "human sends, agent replies" leg

**Every credential this lane holds is a BOT identity, and the one user identity is dead.**
So there is no way for this lane to originate a human-authored message on any of the three
platforms:

- **Discord** — bot token only; `map_message_create` drops all bot authors.
- **Slack** — `xoxb` bot token only; `inbound.rs:171` sets `is_bot = ev.bot_id.is_some()`,
  and `classify()` drops `is_bot` before access is even consulted.
- **Matrix** — the token is Sean's *user* account, so even alive the gateway would
  authenticate AS the sender and the message would be `is_self` → dropped. And it is dead.

Slack channel `C0BLR1UKKU6` (`wayland-test`, `is_private: true`) has exactly two members:
`U3PGRDZGA` (the human) and `U0BLBKR56NT` (the bot).

## T+1h30 — the configure-from-nothing journey, measured (Slack, on hetzner)

`WAYLAND_HOME=/root/wl-uatlive-home`, so `config.toml`, `credentials.toml` and `channels/`
are all isolated from `/root/.wayland`.

**Step 1 — write exactly what `docs/channels.md` "Recommended deployment baseline" shows.**

```
channels in /root/wl-uatlive-home/channels:
  slack                             UNKNOWN PLATFORM
      config error: TOML parse error at line 1, column 1 … missing field `name`
```

**Step 2** add `name` → `missing field platform`. **Step 3** add `platform` → loads, but
`channel probe` → `config parse: missing field workspace_name`. **Step 4** add `[options]`
with `workspace_name`, `default_channel_id` and the two `credential_handle_*` keys → loads.

**Four edit-and-rerun round trips from the documented starting point**, and none of the four
fields the user had to discover appears anywhere in `docs/channels.md`. The errors themselves
are good — they name the exact missing field and the exact file — but they are the only
documentation of the required schema.

### `channel probe` — what it does and does not tell you

With the handles configured but the credentials store empty:

```
discord   Incomplete    finding: credential "discord.waylandtest.bot_token" is not present in the credentials store
matrix    Unsupported   config: INCOMPLETE   auth: NOT authenticated   finding: adapter implements no setup probe
slack     Unsupported   config: INCOMPLETE   auth: NOT authenticated   finding: adapter implements no setup probe
```

After writing the real credentials into `$WAYLAND_HOME/credentials.toml`:

```
discord   Ok   config: complete   auth: authenticated   identity: 1532224324075913297
matrix    Unsupported  (unchanged)
slack     Unsupported  (unchanged)
```

Discord's `identity` matches the id my own `curl` returned — a real live platform call made
by the product. **Only 3 of the 10 shipped channel adapters implement `probe` at all**
(`/usr/bin/grep -rln 'async fn probe' crates/wcore-channel-*/src/` → discord, email,
whatsapp-bridge). Two of the three MVP channels are among the seven that do not — and for
those two the report prints `config: INCOMPLETE` and `auth: NOT authenticated` when the truth
is *unknown*. A first-time user with a perfectly good Slack token is told it is not
authenticated.

### Instrument correction (recorded because it nearly became a false HIGH)

My first reading of `channel probe`'s exit code was `0` for "3 of 3 not ready" — which would
have been a self-passing gate worth a HIGH. It was `$?` **after a pipe to `tail`**, exactly
the trap LANE-BRIEF §2a names. Re-measured unpiped into a `WLRC=`/`WLDONE` file: **`WLRC=1`**.
`channel probe` gates correctly. No finding.

## T+2h — `gateway run` starts, and `channel health` LIES (HIGH)

```
[gateway] channels registered=3
[gateway] inbound: subscriber spawned, webhook host listening bind=127.0.0.1:8787 policies=3
[gateway] started pid=4054807 role=Owner profile=default carried=0 … quarantined=0
WARN /sync failed; backing off  error=HTTP 401 {"errcode":"M_UNKNOWN_TOKEN", …}   (repeating)
```

The gateway starts, registers all three, and the Matrix failure is loud and correct **in the
log**. `channel health`, taken against that same live process:

```
configured: 3   registered: 3
matrix (matrix)   state: Healthy   reason: -   errors: 0   reconnects: 0
```

Simultaneity capture (one shell, one gateway, timestamps from the gateway's own log):

```
last 401 BEFORE health : 2026-07-30T10:23:36.395456Z    401_count_before=8
health taken at        : 2026-07-30T10:23:40Z
                         matrix -> state "healthy", consecutive_errors 0
last 401 AFTER health  : 2026-07-30T10:23:53.137330Z    401_count_after=9
```

The adapter is failing every poll across the measurement window and the health surface reports
a clean zero. This is the exact false-zero shape `channel.rs`'s own module docs claim to have
closed ("**A zero from a surface that was not looking is not a zero**", citing F24-C-M2 and
F24-B-H3). The brief's premise — *"`channel health` exits 0 even while Degraded"* — is
**understated**: for a revoked Matrix credential it never reaches `Degraded` at all.


