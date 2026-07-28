# 24-C3-H4 — running notes (append-only, re-committed after every measurement)

Lane `lane/24-c3-h4`, branched from `plan/f20-unified-audit-repair` @ `e6abc748`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-c3-h4`.

## T0 — source reading (no measurement yet)

The finding as inherited from 24-C3-H2 §5: `run_gateway` registers and `start_all`s
TWO `ChannelManager`s. Read for myself, at `e6abc748`:

- `crates/wcore-cli/src/gateway.rs:686` — `run_gateway` builds the cron `EngineJobHandler`
  via `wcore_agent::cron::build_headless_cron_handler(&cwd)`.
- `crates/wcore-agent/src/cron.rs:358-395` — that function constructs
  `ChannelManager::new()`, calls `auto_register_from_user_config(...)` on it, wraps it in
  `Arc<RwLock<..>>`, calls `start_all()` on it, and hands it to
  `EngineJobHandler::new(Some(channels), None, Some(skill_sink))`. **Manager #1.**
- `crates/wcore-cli/src/gateway.rs:723-844` — `run_gateway` then constructs its OWN
  `ChannelManager::new()`, calls `auto_register_from_dir(&channels_dir)` on it, wraps it in
  `Arc<RwLock<..>>`, spawns the inbound subscriber + webhook host on THAT Arc, then
  `start_all()`s it. **Manager #2.**

So the two managers register from two different directory expressions that normally resolve
to the same files:
- #1: `auto_register_from_user_config` → `wcore_config::config::wayland_config_dir()/channels`
- #2: `auto_register_from_dir(home.join("channels"))`

`run_gateway` sets `WAYLAND_HOME=home` when `--home` was passed and the env var was unset
(gateway.rs:655-661), so `wayland_config_dir()` resolves under the same home and the two
paths are the SAME directory. That is why 24-C3-H2 saw six registration events for three
channels rather than three plus zero.

Only manager #2 has a subscriber. Manager #1 exists solely so Channel *cron* jobs have a
send path.

### Why the polling half might not be benign

Webhook adapters (slack/whatsapp/sms as driven by `f24-inbound.mjs`) receive by HTTP POST
into the webhook host, which holds manager #2's Arc — so manager #1 never sees them and the
9-arrival / dedupe-green result in 24-C3-H2 is consistent with a harmless duplication.

Polling adapters consume as they poll:
- Telegram `getUpdates?offset=N` — confirming offset N permanently drops updates < N
  server-side. Two pollers with the same token race, and whichever poller confirms first
  destroys the other's chance of ever seeing that update.
  (`crates/wcore-channel-telegram/src/longpoll.rs`, `offset_store.rs`)
- IMAP sets `\Seen`.
- Discord gateway holds one session.

Manager #1 has no subscriber, so anything it wins is dropped on the floor — silent inbound
message loss with no error anywhere.

## T0 — plan (in order, so a death mid-run is resumable)

1. Reproduce the double registration myself on a real `gateway run`. Count registration
   events. If it does not reproduce at `e6abc748`, that is a DISPROVED and I say so.
2. Build the polling fixture seam. Telegram is the cheapest honest seam:
   `TelegramChannel::with_api_base` already exists but is `#[doc(hidden)]` and the registry
   (`wcore-channels-registry::make_telegram`) only calls `TelegramChannel::new`, so the
   SHIPPED binary can only ever reach `api.telegram.org`. A config-level `api_base` option
   on `TelegramConfig` closes that, and gives the whole program its first polling-inbound
   fixture seam — the thing whose absence has kept these adapters unmeasured all phase.
3. Measure the consumption race with a local `getUpdates`-shaped endpoint that CONSUMES on
   offset-confirm, exactly as Telegram does. Pre-fix binary vs post-fix binary.
4. Fix the double start: the cron handler must share the runtime's manager.
5. Prove the positive path BOTH ways — inbound still arrives (counts) AND cron still fires
   through a channel (counts). A fix that makes nothing start passes every
   "no duplicate registration" check.

## Status

- [x] source read
- [ ] double-start reproduced live
- [ ] seam built
- [ ] consumption race measured
- [ ] fix
- [ ] positive proof (inbound + cron)

---

## T1 — the seam, the fix, and the instrument (Mac; nothing compiled yet)

### The seam, and why it is a two-line change rather than a new subsystem

Telegram is the ONLY HTTP channel adapter without a config-level base-URL
override. Slack, WhatsApp and SMS all already carry
`#[serde(default = "default_api_base")] pub api_base_url: String` — which is
precisely why 24-C3's matrix could drive those three and not telegram.
`TelegramChannel::with_api_base` existed but is `#[doc(hidden)]` and
`wcore_channels_registry::make_telegram` calls `new`, so no config a shipped
binary can load could point the polling adapter anywhere but api.telegram.org.

Added `api_base_url` to `TelegramConfig` with the same default-to-production
pattern, and made `new` honour it. Two tests: `new_honours_the_configs_api_base_url`
(through `new`, the constructor the registry calls) and the control
`new_without_an_override_still_points_at_production_telegram`.

### The fix

`build_headless_cron_handler_with_channels(cwd, Some(arc))` adopts a
caller-owned manager and registers/starts nothing. `run_gateway` now builds its
channel stack (register → Arc → subscriber → `start_all`) BEFORE the automation
plane, and hands the plane's handler that same Arc. Plane after channels,
because `plane.resume()` dispatches carried deliveries and adapters that resolve
their credential in `start()` cannot send before `start_all` has run.

### The instrument, proven before use

`scripts/f24-tg-fixture.mjs` — a Telegram-shaped endpoint with faithful
consumption semantics: `offset=N` permanently deletes every pending update with
id < N, and the deletion is attributed to the poll that caused it. It counts
`max_concurrent_getupdates` from overlapping open requests, in another OS
process — so the number of pollers is measured from real HTTP traffic, not from
a log line the binary prints about itself.

It deliberately does NOT answer a second concurrent `getUpdates` with real
Telegram's `409 Conflict`. 409ing would make the second poller fail loudly,
which is the easy case. Serving both is the quiet case that produces silent
loss, and it is the one worth measuring.

`scripts/f24-c3-h4-fixture-selftest.mjs` — the instrument against a
known-positive and a known-negative, run on the Mac at this commit:

```
ok  1 NEGATIVE single poller is served all four — served 1,2,3,4
ok  1 NEGATIVE nothing was deleted before it was served — [1,1,1,1]
ok  1 NEGATIVE exactly one poller was seen — max=1
ok  2 POSITIVE the thief was served all four — thief got 4
ok  2 POSITIVE the victim is served ZERO after the confirm — victim got 0
ok  2 POSITIVE every deletion is attributed to the poll that caused it — [2,2,2,2]
ok  3 CONCURRENCY two overlapping getUpdates read as 2 — max=2
ok  4 FLOOR a run with no poller reads as 0 — max=0 polls=0
SELFTEST failures=0
```

Assertion 4 is the anti-universal-denial guard: a "fix" that works by making
NOTHING start reads as 0, not as 1, so it cannot pass as "one manager".

## Status

- [x] source read
- [ ] double-start reproduced live  ← next, needs a hetzner build
- [x] seam built (not yet compiled)
- [x] instrument built and self-proven
- [ ] consumption race measured
- [x] fix written (not yet compiled)
- [ ] positive proof (inbound + cron)
