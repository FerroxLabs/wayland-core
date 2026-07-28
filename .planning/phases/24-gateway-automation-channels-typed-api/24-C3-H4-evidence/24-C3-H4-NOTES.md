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
