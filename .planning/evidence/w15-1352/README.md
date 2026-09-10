# wayland#1352 — the ACP relay cancellation, named and repaired

Lane w15/relay1352, 2026-09-10. Host `hetzner-dsm` (Linux 6.8.0-101, 96 CPU,
shared; loadavg 26-70 across these runs, recorded with every sample). Builds and
tests through `tools/remote-proof.py` slot `parallel-1`; every receipt quoted
has the stated `remote_exit` and `"complete": true`. No SOURCE_INPUTS path was
touched (the change is confined to `crates/wcore-acp`).

## Binaries

Full hashes, cargo args and receipts are in `binaries.txt`.

| label | source | profile | sha256 |
|---|---|---|---|
| base-debug | 008d88ce9 (= base cd85dac88 except one test-file line) | debug | d0819d10... |
| base-release | cd85dac88 | release, `--features voice` | 9b2e24f2... |
| src3530-release | 3530199fb (NOT an ancestor of cd85dac88) | release, `--features voice` | 0f3d81cf... |
| diag-debug | 784419c4b, branch `w15/relay1352-diag` (instrument only) | debug | e5566d15... |
| fix-debug | 671109fa9 | debug | 70f44e05... |
| fix-release | 671109fa9 | release, `--features voice` | d73cb623... |

`git diff 3530199fb cd85dac88 -- crates/wcore-acp/src/bounded.rs crates/wcore-cli/src/acp_engine.rs`
is empty: the relay code is identical on both sources.

## c1 — the cause, and the instruments that named it

**The ledger hypothesis is refuted.** It guessed that a process-wide aggregate
budget, held by other sessions' slow or disconnected readers, exhausted a
keeping-up reader's one-second wait. Three measurements say otherwise.

1. **Which path cancels.** bpftrace v0.20.2 uprobes on the unmodified base-debug
   binary (`instruments/relay-cancel-stacks.bt`, `relay-cancel-caller.bt`). The
   reproducing arm (probe6, 8 fast 16 MiB readers, NO slow or disconnected
   readers) went 0/16 (`c1/stacks1.status`) and 0/8 (`c1/stacks2.status`), each
   turn cancelled at 3.6-4.1 s after 2.65-4.92 MiB. Each cancellation entered
   `Sender<E>::overload` once, and its direct caller -- the return address at
   `[rsp]` on entry -- was `send_retained::{{closure}}` in 8/8
   (`c1/stacks2.overload`): the turn's cumulative one-second wait budget ran out.
   Never `retain()` refusing the aggregate; never a synchronous `send()` overflow.
2. **The aggregate at that moment**, read from `wcore_acp::bounded::global::USED`
   by the same probe: 8,412,674 bytes at the first cancellation, falling ~1.05 MiB
   per cancellation to 1,051,910 at the eighth -- eight FULL 1 MiB stage-1 channels,
   against a 64 MiB aggregate that was never near exhausted.
3. **What the stage-1 consumer waited on.** diag-debug adds per-event timers to
   the ACP recorder and delivery tasks (`instruments/diag-784419c4b.patch`;
   output `c1/diag-c1-16m-optional.w15diag`, `c1/diag-c8-16m-optional.w15diag`):

   | per session | c1, 3 turns x 513 events, 3/3 pass | c8, 8 turns x 82-88 events, 0/8 pass |
   |---|---|---|
   | recorder waiting for the process-wide `events` WRITE lock | 4-52 ms | **1,116-1,630 ms** |
   | delivery waiting for the same lock's READ side | 0 ms | **1,115-1,631 ms** |
   | recorder holding it | 668-695 ms | 104-113 ms |
   | stage-1 engine waits / time waited | 0 / 0 ms | 951-1,123 / 999-1,001 ms, then overflow |
   | stage-1 peak queued bytes | 98-263 KiB | 1,018,257 (full) |

**Named cause.** `AcpServer.events` was ONE `RwLock<HashMap<session, EventLog>>`
for the whole process. Every session's recorder took it for WRITE once per event,
and re-encoded the event inside it. Every session's delivery task took it for READ
once per event, and encoded and cloned the event inside it. At concurrency 8 each
recorder waited 13-19 ms per event behind OTHER sessions' holders, fell behind its
own engine, and the engine's `send_retained` spent the turn's one-second budget
on a full stage-1 channel: `protocol relay overloaded`. The HTTP reader plays no
part -- the recorder never waits for it, which is why a fast reader was cancelled.
It shows on the debug build because encoding inside the lock is roughly ten times
slower there; the same arms on base-release pass (c8 16/16, median 3.18 s; c32
32/32, median 8.26 s, `c1/release-base1.status`), which is also why the
3530199fb soak (release) never met it. The coupling exists in both builds.

**Deterministic red test.**
`crates/wcore-acp/src/server.rs::another_sessions_event_log_work_does_not_stall_this_sessions_turn`
holds the shared event-log state, standing in for another session's log work,
while this session streams 64 x 32 KiB, and requires the turn to record and
deliver within 5 s. It depends on no timing or load: at base the recorder cannot
take its write lock at all while the guard is held. Committed first (1e824da3b)
and run there before any repair: **FAIL** at 5.012 s, `this session's turn
stalled while another session held shared event-log state: Elapsed(())`,
remote_exit 100.

## c2 — the repair, the A/B, and the red arm

**Repair (671109fa9, `crates/wcore-acp/src/{server.rs,server/lifecycle.rs,cursor.rs,bounded.rs}`).**
Each session's event log now has its OWN lock (`SharedLog`). The recorder and the
delivery task resolve their session's log once per turn and never take the
process-wide map lock per event. The recorder reuses the size it already encoded;
delivery charges the size recorded for that event (`bounded::retain_encoded`), so
nothing is encoded under any log lock. The 64 MiB cross-session retained-history
cap is enforced exactly as before -- always trimming the largest log -- through an
exact `retained_total` kept under each log's lock and an `eviction` mutex that
serializes evictors only. A closed session (and a failed create) returns its
bytes to the total.

**No bound changed.** `LIVE_BYTES` 1 MiB, `LIVE_EVENTS` 256, `EVENT_BYTES`,
`AGGREGATE_BYTES` 64 MiB, the 8 MiB per-log and 64 MiB total history caps, and
both one-second wait budgets are untouched.

**A/B through the real `acp serve`** (`instruments/w15-relay1352-mixed.py`
sha256 16a40854..., `instruments/queue-c2-mixed.sh` sha256 6d10ea63...). It uses
the soak driver's fixture provider, config (`require_durability = true`,
encrypted_file vault) and reader modes, with a fixed composition so every arm
gets its fast rows: c8 = 4 fast + 2 slow + 2 disconnected, c32 = 16 fast + 8 slow +
8 disconnected, all 16 MiB; schedule `8,8,32,8,8,8,32,8,8,8`; no abort on a
failing row. 15:56:48-16:08:22Z, arms interleaved base/fix per profile.

| arm | c8 fast pass | c8 relay cancel | c32 fast pass | c32 relay cancel | slow+disc pass | peak RSS | quiescent RSS first -> last (max) |
|---|---|---|---|---|---|---|---|
| base-debug | 17/32 | 15 | 1/32 | 31 | 64/64 | 1,647,243,264 | 161,906,688 -> 208,392,192 (212,627,456) |
| **fix-debug** | **32/32** | **0** | **32/32** | **0** | 64/64 | 5,117,382,656 | 159,805,440 -> 213,221,376 (214,417,408) |
| base-release | 32/32 | 0 | 32/32 | 0 | 64/64 | 4,280,897,536 | 73,490,432 -> 109,867,008 (112,435,200) |
| **fix-release** | **32/32** | **0** | **32/32** | **0** | 64/64 | 4,292,141,056 | 76,595,200 -> 108,220,416 (111,230,976) |

Stage-2 (`live delivery overloaded`) on fast rows: 0 in every arm. Every batch of
every arm quiesced to zero sessions and zero active fixture requests. Receipts:
`c2/c2-mixed1/*.receipt.json`, per-batch lines in `c2/c2-mixed1/*.stdout`.

**Delivery stays bounded; the fix adds no memory.** On release, where the base
does not cancel, base and fix do the same work and peak within 0.3% (4.28 vs
4.29 GiB) with the same quiescent RSS. The higher fix-debug peak is completed
work, not a looser bound: per batch, base-debug's c32 batches delivered 23 and
41 MiB of fast text (peaks 872 and 978 MiB) because it cancelled them, while
fix-debug delivered 256 MiB each (peaks 4,724 and 4,880 MiB); c8 batches that
delivered the same 64 MiB peaked at 1,459-1,571 MiB on base and 1,384-1,846 MiB on
fix.

The fast-only reproducing arm, fix-debug (`c2/fixdebug1/`): c8 16 MiB 16/16 and
c32 32/32 with 0 relay cancellations, against base-debug's 0/16 and 0/8.

**Red arm** (branch `w15/relay1352-redarm-lock`, 98269e4bd, NEVER MERGE): the fix
plus one mutation that puts back a per-event process-wide exclusive section
around the recorder's append. The c1 test goes **FAIL** at 5.011 s with the same
message, remote_exit 100.

**A reader that is not keeping up is still cancelled.**
`crates/wcore-acp/tests/stabilization_backpressure.rs::a_stalled_reader_is_still_detached_while_another_sessions_reader_keeps_up`
runs a stalled reader and a keeping-up 16 MiB reader in two sessions of one
server: the keeping-up reader receives every byte with no error, and the stalled
one is still detached with its explicit overload terminal while its record stays
resumable. The wcore-cli W05 relay tests, which cancel a stage-1 consumer that
stops draining, are unchanged and pass.

**Test and lint runs.** wcore-acp clippy `--all-targets -D warnings` at
671109fa9: exit 0. wcore-acp nextest at 671109fa9: 173/173; at d25526a37:
174/174 (adds the stalled-reader test). wcore-cli `test(acp_engine::)` at
d25526a37: 52/52.

## c3 — the concurrency-1 16 MiB latency

See `c3/c3-latency.md` for both interleaved runs, the strace census and the
trim-threshold A/B. In short: the "~21.7 s here vs ~2.6 s at 3530199fb"
comparison set a debug binary against a release binary. Measured with one driver
at n=10 per arm, the same code takes 22.4 s in debug and 1.9-3.8 s in release,
and 3530199fb in release takes 1.75-1.9 s. The one mechanism difference found
between the release sources -- 22,637 `brk` calls against 17, from allocator.rs's
fixed mmap threshold -- was removed with a glibc tunable on the same binary
without moving the median (1,962 vs 1,946 ms), so it is not the latency.

## Commits

On `w15/relay1352` (base cd85dac88):
1. `1e824da3b` test(acp): another session's log work must not stall a turn -- the c1 red test, committed before any repair
2. `671109fa9` fix(acp): give each session's event log its own lock -- the repair
3. `d25526a37` test(acp): a stalled reader is still detached beside a fast one
4. the evidence and ledger commit that adds this directory

Never to merge: `w15/relay1352-diag` (`784419c4b`, timers), `w15/relay1352-redarm-lock` (`98269e4bd`).

## Not claimed

* That no load could ever cancel a keeping-up reader at stage 1. The cumulative
  one-second budget is unchanged. The measured cross-session coupling -- per-event
  work under a process-wide lock -- is gone. Host-wide CPU starvation, the global
  `capacity_changed` notifier (about 1,000 wakeups per waiting turn at c8 debug;
  that costs CPU, not budget), the aggregate mutex, and the eviction mutex once
  history is over the 64 MiB cap are still shared, and are not shown to be harmless
  at every concurrency.
* That the release base cancels under the mixed schedule: it did not. The
  base-vs-fix cancellation difference is shown on debug, the build the #1349
  soak ran.
* Anything about the #1349 7200 s soak or its RSS verdict. It was not rerun.
* The origin of the ~4.3 GiB release peak at c32 (32 concurrent 16 MiB turns).
  It is equal on base and fix, and was not examined further.
* A cause for the residual latency difference between the two release sources;
  it was not stable across the two runs.
