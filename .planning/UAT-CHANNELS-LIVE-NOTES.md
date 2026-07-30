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

Status: unverified at time of writing.
