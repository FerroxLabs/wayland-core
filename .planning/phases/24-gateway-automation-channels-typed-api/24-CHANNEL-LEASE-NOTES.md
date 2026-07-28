# 24-CHANNEL-LEASE — running NOTES (§6b-i)

Lane: `lane/channel-lease`. Base `ef1d97be` (`plan/f20-unified-audit-repair`).
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-channel-lease`.

This file is appended and re-committed after every measurement. If this lane dies,
resume from the last entry.

---

## T+00 — established by reading source (no execution yet)

### The three production sites are real

Verified by reading, all present at base `ef1d97be`:

| # | Site | Constructs `ChannelManager` | Calls `start_all()` | Reached by |
|---|------|------------------------------|---------------------|------------|
| 1 | `crates/wcore-agent/src/bootstrap.rs:3092` (`ChannelManager::new()`), `:3254` (`start_all`) | yes | yes | **every ordinary `wayland-core` session** — guarded only by `self.without_channels`, default false |
| 2 | `crates/wcore-agent/src/cron.rs:403` (`new()`), `:432` (`start_all`) | yes | yes | `wayland-core cron daemon` (headless handler, `channels == None` arm) |
| 3 | `crates/wcore-cli/src/gateway.rs:725` (`ChannelManager::new()`) | yes | yes (later in fn) | installed gateway service |

Site 2 already has an *in-process* guard: `build_headless_cron_handler_with_channels`
returns early ("not registering or starting a second one") when the caller passes its
own `Arc`. That is F24-C3-H4's fix. It is scoped to one process — it says nothing about
a second process.

`grep -rn "flock\|ScheduleLease\|LeaseAttempt" crates/wcore-channels/` → **no match**.
There is no cross-process exclusion anywhere in the channel stack. Confirms the brief.

### The lease mechanism to reuse

`crates/wcore-cron/src/lease.rs` — `ScheduleLease`. Already exactly the right shape:

- `attempt(dir, holder) -> LeaseAttempt::{Owner(ScheduleLease), Observer{holder_pid}}`
  — contention is a **role, not an error**. That is the API this lane needs, because the
  loser must do something observable rather than fail.
- Exclusion is `flock(LOCK_EX|LOCK_NB)` on Unix / `LockFileEx` on Windows, taken on a
  one-byte `schedule.lock` sentinel, with a freely-readable `schedule.owner` JSON record
  alongside (never locked, so an observer can name the owner).
- **Release is by OS descriptor close** — survives SIGKILL, panic, power loss. No
  timestamp heuristic. This is precisely the property that stops the "stale lease wedges
  everything forever" failure the sandbox lane hit last night. Reusing it means I inherit
  that property rather than having to re-establish it.
- `LeaseHandle` is a cheap clonable `Arc<AtomicBool>` consulted *immediately before each
  action*, not once at the top — the mid-loop-loss guard.

Its own unit tests already cover: second-attempt-in-one-process refused (the test that
would silently pass under `fcntl`), release-lets-next-win, released-lease-leaves-no-record,
sentinel-stays-one-byte, record-readable-while-held.

**Decision (provisional):** reuse `ScheduleLease` rather than write a channel-specific
lock. Inventing a second exclusion concept is literally how the double-manager bug
happened. Open question is the dependency edge — `wcore-channels` may not depend on
`wcore-cron`. To be resolved before writing code; noted here so a resume knows it is open.

### Destructive-read claim — to be verified, not assumed

The brief asserts inbound polling is a destructive read. Before I claim loss I must show
it: Telegram `getUpdates` offset advance, IMAP `\Seen`, Discord one-session-per-token.
`scripts/f24-tg-fixture.mjs` (12.7K) mints its own token — usable without any vendor
credential. **Other lanes own `scripts/f24-inbound.mjs` and the Discord/Telegram fixtures
— I must not edit them. Harness will be my own file.**

### Still to establish

1. Reproduce the two-process loss with real binaries + fixture. **If it does not
   reproduce, that is the result and I stop.**
2. Where the lease goes (crate/dep edge) and what identity it is keyed on — per *account*,
   not per process, or two accounts serialise needlessly.
3. What the loser does. Must be observable. Silent no-channels is a new silent failure.
4. Ungraceful-kill takeover.
5. Positive path: holder receives **every** message, counted — else universal denial
   manufactures a green.
6. Steady-state leg, not just startup.

### Traps I am pre-committing to

- `instrument_fault` state → grade INCOMPLETE, not LOSS. Self-test with three assertions
  including "the old matcher would have missed it".
- Byte-count every capture; `echo "EXIT=${PIPESTATUS[0]}"` after a pipeline returns empty
  in this environment.
- Assert executed test counts (`N passed`), never exit status.
- No cargo on the Mac except `cargo fmt --all -- --check`. Builds on hetzner.
