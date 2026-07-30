# NOTES — lane `fix-keyringless-inbound`

Branch `lane/fix-keyringless-inbound`, base integration `e7bc6d88`.

Goal: (1) drive a REAL inbound turn on a REAL platform from a keyring-less host,
with the platform's own API as the arbiter of the reply; (2) close the three
keyring residuals the merged lane (`c73ac417`) named openly.

---

## Phase 0 — premise verification (Mac, read-only)

Every instrument below is `/usr/bin/grep` with a quoted glob (zsh eats
`--include=*.rs` unquoted — hit that immediately, first invocation returned
`no matches found` with rc=1, which would have read as a false absence).
Each absence claim below is paired with a known-positive in the same capture.

### P1 — "`durable_sessions_disabled_by_host()` has NO consumer" — HOLDS

```
/usr/bin/grep -rn "durable_sessions_disabled_by_host" --include='*.rs' crates/
```
5 hits, ALL inside `crates/wcore-config/src/config.rs`:
- 2484 `record_durable_sessions_disabled_by_host();`  (producer, in `Config::resolve`)
- 2598 `pub fn durable_sessions_disabled_by_host()`   (the getter)
- 2604 `fn record_durable_sessions_disabled_by_host()` (the setter)
- 5232/5234 — inside the crate's own `#[cfg(test)]` module.

So: zero production consumers, and zero consumers outside the defining crate.
Known-positive control in the same capture: `fn main` under `crates/wcore-cli/src`
returned 5 hits, so the instrument was alive.

### P2 — "`switch_active_session`, journal writer #3, is still unguarded" — HOLDS

`crates/wcore-agent/src/engine.rs:3696`. Read the body: it validates the
incoming journal (session-id match, canonical baseline present), then
unconditionally `self.session_journal = Some(journal);`. There is no
`config.session.enabled` consultation anywhere in the function.

Contrast with writer #2, which `c73ac417` DID guard — `engine.rs` +3342:
```rust
let session_journal = if config.session.enabled { session_journal } else { None };
```

Production call site: `crates/wcore-cli/src/tui/engine_bridge.rs:3081`.
That path is FENCED to another lane, so the guard must live in `engine.rs`.
It belongs there regardless: the invariant is the engine's, not the caller's.

### P3 — "`--json-stream` / Desktop hosts are never told" — TO MEASURE

Decision still open. Contract regeneration is orchestrator-only, which
constrains the shape of any answer.

---

## Phase 1 — the host is genuinely keyring-less, but NOT for the reason assumed

The brief said to prove the keyring-less state rather than assume it. Correct
instinct, and the naive check would have gone the WRONG way:

- `/run/user/0/keyring/pkcs11` EXISTS
- **three `gnome-keyring-daemon` processes are running, two of them
  `--components=secrets`**
- `/root/.local/share/keyrings/{user.keystore,login.keyring}` both exist
- `/run/user/0/bus` (a live session-bus socket) exists

So "there is no keyring on this box" is FALSE. What is true is narrower and is
the thing that actually matters:

| arm | `DBUS_SESSION_BUS_ADDRESS` | Secret Service ping | rc |
|-----|---------------------------|---------------------|----|
| bare non-login shell (the deployment shape) | unset | `Unable to autolaunch a dbus-daemon without a $DISPLAY` | **1** |
| pointed at `unix:path=/run/user/0/bus`      | set   | `method return ...` | **0** |
| pointed, but asking for `org.freedesktop.zzznosuch` | set | `ServiceUnknown` | **1** |

Row 3 is the control that stops row 2 being self-passing. `busctl --user list`
shows `org.freedesktop.secrets` present (1 hit) with `org.freedesktop.DBus` as
the known-positive.

**This is a gift, not an obstacle**: it means the same box can serve BOTH arms,
and the positive control can use a REAL OS keyring over the Secret Service —
which `c73ac417` explicitly listed as UNRUN ("proven via the vault rather than
a real OS keyring").

### Instrument defects found and repaired in this phase

1. **`ps aux | grep -c "[k]wallet"` returned 1 on a box with no kwallet.** The
   bracket trick stops grep matching itself, but `ps` was showing MY OWN ssh
   command line, which carried the pattern. Repaired by writing the probe to a
   script and building each needle at runtime from concatenated fragments
   (`"kwal""let"`), so the literal appears in no command line. Re-measured:
   kwallet **0**, and the known-negative `zzznosuchproc` **0** while the
   known-positive `sshd` stayed non-zero — i.e. all three assertions, including
   "the old broken matcher would have said 1".
2. **`DBUS_SEND_RC=$?` after a pipe reported `head`'s status.** It printed
   `RC=0` while `dbus-send` had actually failed. Exactly the §2a trap. Repaired
   by running each arm in a function that redirects to a file and reads `$?`
   with no pipe in between.

## Phase 2 — the brief's Task-1 platform choice is FALSE

> "Configure a real channel (Discord is the safest ...)"

**Discord cannot be driven automatically at all**, with any bot credential:

`crates/wcore-channel-discord/src/gateway.rs:322-324` in `map_message_create`:
```rust
let author_is_bot = msg.author.as_ref().is_some_and(|a| a.bot);
if author_is_bot { return None; }
```
Unconditional, before any allowlist or mention logic, with no config override.
Discord sets `bot: true` for the application's own user AND for webhook
authors, so:
- posting with our bot token -> dropped
- creating a channel webhook and posting through it -> dropped

A genuine Discord inbound therefore requires a **human, non-bot Discord
account** posting in the channel. `~/.wayland-secrets/discord.env` holds only
`DISCORD_BOT_TOKEN`. Driving a human account from a script would be a
self-bot (Discord ToS) and I hold no such token regardless.

Same single-identity problem on the other two:
- `slack.env` -> `SLACK_BOT_TOKEN`, prefix **`xoxb`** (bot, not `xoxp` user).
- `matrix.env` -> Sean's PERSONAL account; the gateway would authenticate as
  that same user, so his messages are `is_self`. A second Matrix account can't
  reach the room without an invite, which the brief forbids.

### The one credential set with two real identities: Twilio

`IncomingPhoneNumbers.json` -> **11 owned numbers**. `TWILIO_FROM_NUMBER` is
owned and its `sms_url` is the inert `demo.twilio.com/welcome/sms/reply/`, as
are 3 others. The other 7 point at `services.leadconnectorhq.com` — Sean's LIVE
business webhooks, to be left strictly alone.

So an SMS round trip between two owned demo numbers is a genuine two-identity
inbound, fully automatable, and the Twilio Messages API is an independent
platform-side arbiter for the reply. Prerequisites being measured next.

## Still to establish

- [ ] hetzner worktree at `e7bc6d88`, SHA asserted programmatically
- [ ] PROVE the keyring-less state rather than assume it (headless Linux may
      still carry a gnome-keyring socket — that is an absence claim and gets a
      known-positive like any other)
- [ ] Discord channel configured + `gateway run` with `channels registered>=1`
      (the prior lane's gateway evidence shows `registered=0`, so its gateway
      leg never had a channel at all)
- [ ] real inbound message -> turn -> reply, READ BACK FROM THE DISCORD API
- [ ] negative control on base `bc90ee1c` — must FAIL
- [ ] positive control with a working credential store — must PASS
- [ ] seeded-break control proving the harness can redden at all
