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

