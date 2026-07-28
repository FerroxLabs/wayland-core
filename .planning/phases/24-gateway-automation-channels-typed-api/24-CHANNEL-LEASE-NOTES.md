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

---

## T+35 — reproduction design fixed, before writing any harness code

### Facts established by execution (not reading)

- hetzner worktree `hz/channel-lease` at `3d7d4a01`; `/root/wayland-channel-lease`.
  `cargo build -p wcore-cli` → **BUILDRC=0**, binary 328465528 bytes at
  `target/debug/wayland-core`. Disk 717G free (well over the 150G floor).
- node v22.21.1 present on hetzner.
- `wayland-core [PROMPT]...` — a one-shot prompt IS an ordinary session. `--help` confirms
  "Initial prompt (if omitted, enters interactive REPL mode)".
- `wayland-core cron daemon` exists — "Spawn the cron runner as a detached background daemon".
- `.without_channels(true)` callers, whole repo: **20 hits, every one a test**, plus
  `channel_dispatch.rs:230` (the per-session recursion guard). ZERO production callers.
  So every ordinary session starts channels. The brief's claim is exact.

### The fixture already carries the right observable — and I must not edit it

`scripts/f24-tg-fixture.mjs` (owned by another lane; I USE it, I do NOT modify it) serves
real Telegram offset semantics and exposes `/__control/submit` and `/__control/report`.
`report` returns `max_concurrent_getupdates`, counted **in another process** from
overlapping open requests. That is the anti-universal-denial discriminator, and the fixture's
own header already names the property I need:

> two managers polling one token show up as 2, one manager as 1, and a runtime that polls
> NOTHING shows up as 0 — which is a distinct, failing answer, so a fix that works by making
> nothing start cannot pass.

`report` also gives per-update `deleted_by` (which poll destroyed it), `served_to`, and
`still_pending`.

### Setup recipe (learned by reading the prior lane's driver; reimplemented in my own file)

- `$WAYLAND_HOME/credentials.toml` `[secrets]` `"telegram.<h>.bot_token" = "<minted>"`
- **`WAYLAND_VAULT_PASSPHRASE` must be set** or every turn refuses host-wide with
  "Session persistence authority unavailable" — 24-C3-H2 measured this. Without it I would
  attribute a credentials-posture refusal to the polling path. Minted per run, never printed.
- channel toml: `api_base_url = <fixture>`, `long_poll_timeout_secs = 1`
- `[inbound_webhook] enabled = false` — this measures the POLLING path.

### The two legs, and why attribution is not a problem

The hard part is attributing a poll to a process; the fixture sees TCP, not pids. I am NOT
solving that by trusting the binary's own stderr (that would be the tautology the brief
warns about). Instead the **delta between process counts is the attribution**:

**Leg 1 — startup / backlog theft (sequential, zero ambiguity).**
Submit N updates. Run ONLY the ordinary session B; it starts channels, polls, CONFIRMS
(server-side delete), exits. Then start gateway A and let it poll. A receives **0 of N**
and `still_pending` is empty. No concurrency, so nothing to attribute — the messages are
provably destroyed by a process that then died with them. This is the brief's headline
scenario exactly: service installed, user opens a session, backlog gone.

**Leg 2 — steady state (concurrent).**
A running and polling alone → report `max_concurrent_getupdates == 1`. Start B alongside;
submit updates continuously. If `max` goes to **2**, two processes are polling one account.
`poll_total` rate over equal windows is the second, independent signal for the same claim,
so an alternating (non-overlapping) pair cannot silently read as 1.

Steady state is included deliberately: it is what raised the in-process version of this from
MEDIUM to HIGH, and a startup-only run would have missed it.

**Leg 3 — the loser must be loud.** Fixture `max == 1` AND the second process emits an
observable refusal. A second process that silently has no channels is a NEW silent failure
replacing the old one, so `max == 1` alone is not a pass.

**Leg 4 — ungraceful release.** `SIGKILL` the holder; the second process must take over.
Observable purely from the fixture: polls continue after the holder's death.

**Anti-universal-denial:** every leg asserts the POSITIVE path too — the holder receives
every message, counted from `report.updates[].served_to`, and `max == 0` is a FAIL.

### Instrument discipline I am pre-committing to (§6b-ii)

`instrument_fault` state: any run where the fixture is unreachable, the journal is
zero-byte, the binary never emitted a poll at all, or the process lifetimes did not overlap
as intended → graded **INCOMPLETE, never LOSS**. Self-test with three assertions:
known-positive passes, known-negative fails, and **the naive matcher would have missed it**.

### Deviation from the brief's suggested order

The brief lists "reproduce, then apply the lease". I am reproducing first as instructed.
If Leg 1/Leg 2 do NOT reproduce, that is the result and I stop and say so.

---

## T+80 — **THE DEFECT IS REPRODUCED.** Raw wire evidence, base commit, unfixed binary

Binary: `wayland-core 0.12.25 (source 3d7d4a016f3eed830f9f4b8824f638e98d68e2e7)`
Run dir `/root/f24cl-run-base`, tg journal 19354 bytes.

Eight updates submitted to the fixture. **Nothing else running.** Then a single ordinary
session — `wayland-core "f24cl session l1 <runid>"`, the shipped one-shot prompt surface,
no flags, no test hooks — was started. Its channel poller armed and, on its SECOND poll,
two milliseconds after its first:

```
{'kind': 'getUpdates.open', 'at': '19:40:02.076Z', 'poll': 1, 'offset': 0, 'open': 1}
{'kind': 'confirm',        'at': '19:40:02.078Z', 'poll': 2, 'offset': 9,
                            'deleted': [1, 2, 3, 4, 5, 6, 7, 8]}
```

**`offset=9` deleted updates 1-8. All eight. Permanently, server-side, for every consumer.**
Journal kind counts for the whole leg: `submit 8, deleteWebhook 1, getUpdates.open 70,
getUpdates.close 70, confirm 1`.

An ordinary interactive session, which the user opened to do something entirely unrelated,
consumed the entire inbound backlog of the account the installed service is supposed to be
serving — and the service had not even started yet. Nothing errored. Nothing warned. The
messages are simply gone.

This is the brief's headline scenario, measured on the real binary against a
destructive-read endpoint, not argued from source.

### What this does NOT yet establish

- The gateway leg did not run (harness aborted first), so "the service then receives 0 of 8"
  is INFERRED from `deleted: [1..8]` rather than measured. The deletion itself is measured
  and is the load-bearing fact — the updates cannot be served to anybody again.
- The steady-state (concurrent) leg has not run yet.

### Two harness defects found, both repaired in-lane (§6b-ii), neither affecting the above

1. **The LLM stub answered plain JSON, not SSE.** The session logged
   `OpenAI SSE stream closed before any terminal event ... retrying (attempt 2/2)` then
   `primary circuit is open`, so it hung ~90s instead of exiting. Note this did NOT taint
   the measurement: the consumption happened at 19:40:02, seven seconds BEFORE the first LLM
   hit at 19:40:09. The channel poller is armed during bootstrap, ahead of the turn.
2. `write EPIPE` aborted the leg after the 90s hang. Gateway stdin moved to `ignore`;
   `report()` given a retry.

### Instrument behaved correctly under both faults

Both runs graded **INCOMPLETE**, never LOSS — reasons `llm stub never bound` and
`write EPIPE`. That is the `instrument_fault` discipline working in the field on the first
two attempts: neither degraded run was allowed to be reported as a result, in either
direction. The evidence above was recovered from the fixture's own journal, which is written
by a different process and fsync'd before each answer.
