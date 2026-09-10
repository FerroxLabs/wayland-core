# wayland#1352 — the ACP relay cancellation, named and repaired

> **Read the Review round at the end first.** An adversarial review of
> d25526a37 found a close racing eviction could drift and wrap the retained
> history total, and that the first guard test was vacuous. The repair was
> reworked (af7e6d53c, f7f6b96d8). The c2 repair paragraph, its "exact" cap
> claim and the commit list below describe 671109fa9 and are superseded there.

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

## Review round — adversarial review of d25526a37

Reopened in the ledger (bd11a056a). Every finding below was fixed as new
commits on top of 4ba1edfde; history was not rewritten. Receipts for every run
quoted here are in `review/` (`review/receipts-index.txt`), all `"complete": true`.

### BLOCKER 1 — a close racing eviction double-subtracted the total

**Forced deterministically.** `crates/wcore-acp/src/server/eviction_tests.rs::a_close_between_victim_choice_and_pop_keeps_retained_total_exact`
installs a `cfg(test)` gate between an evictor choosing its victim and popping
it, closes the victim's session at that gate, releases it, then compares
`retained_total` with the true sum of the remaining logs and requires zero once
every session is gone. Committed with the seam first (2f93f78b6) and run on the
unrepaired code: **FAIL**, counter 61,087,487 against a true sum of 61,349,662 --
short by 262,175 bytes, exactly one 256 KiB event charged twice (remote_exit 100).

**Repair (af7e6d53c; f7f6b96d8 points one older test at the new field).**
* Every change to a log's retained bytes goes through `SessionLog::mutate`,
  which applies the delta to that log's atomic mirror and to the cross-session
  total under that log's own lock. A log can therefore only give back bytes it
  added, so the total cannot go below the true sum.
* A closed log -- and a failed create -- is retired by `forget_log`, outside the
  map lock: it is EMPTIED under its own lock and gives its bytes back once. An
  evictor that chose it earlier then pops nothing and gives nothing back.
  Close does not take any eviction lock (there is none), so the deadlock the
  review warned about cannot arise.
* `RetainedHistory::account` subtracts with `checked_sub`. Underflow is
  unreachable by construction; if it ever happened a debug build panics and a
  release build logs an error and clamps, rather than wrapping silently to a
  total that would evict every log on every append.

GREEN at f7f6b96d8. **Red arm** `w15/relay1352-redarm-drift` (73bccf836, never
merge): close gives the bytes back without emptying the log, as the reviewed
repair did -- the forced test FAILS with the same 61,087,487 / 61,349,662.

`churn_over_the_cap_with_mixed_chunk_sizes_keeps_retained_total_exact`
(multi-thread: residents over the cap, churn sessions writing 26 x 256 KiB then
256 x 1 KiB and closing, equality and cap asserted at quiescence) passes on the
fix -- but it ALSO passed on the drift red arm, so it does not detect this race.
The forced test is the detector.

### MAJOR 2 — the guard test was vacuous

Replaced by
`crates/wcore-acp/tests/stabilization_backpressure.rs::a_reader_that_stops_reading_is_detached_by_its_wait_budget_beside_a_fast_one`:
the stalled turn is 4 MiB (under the 8 MiB per-log cap), time is paused, and
before the stalled stream is read the test proves `events_since(genesis)` still
returns all 17 events, so no replay gap exists; only the budget can detach it.
**Red arm** `w15/relay1352-redarm-budget` (0579423cf, never merge): delivery
budget set to a year -- the test FAILS, the stalled reader receives all 17
frames and no detach. The older
`slow_reader_has_explicit_overload_while_replay_retains_bounded_tail` has the
same weakness; it is kept, and now documents that it covers only the replay-gap
detach.

### MINOR 3 and 4 — cap exactness and serialization above the cap

The eviction mutex is gone. `evict_for_append` finds the largest log from the
per-log atomics without locking any log, locks only that victim for one O(1)
pop, and frees at most what its own append added. Its doc comment states the
bounds instead of claiming exactness: above the cap by at most the appends in
flight (one event, at most 1 MiB, per recording session), below it by at most
one event per concurrent evictor, at or under the cap once appends stop (the
churn test asserts this at quiescence). What an eviction can still wait on is
stated too: one map read per call (a create or close holding the map for write)
and the victim's lock (a resume cloning that log's tail, up to 8 MiB). The field
and `lock_log` doc comments were corrected to match.

### MINOR 5 — the c1 red test was narrow

It runs on one thread and holds the map only for READ, so it could not see a
return to per-event map reads.
`crates/wcore-acp/src/server/eviction_tests.rs::an_exclusive_hold_on_the_log_map_does_not_stall_a_running_turn`
runs multi-threaded, feeds a turn's first event, then holds the map for WRITE
while the rest of the turn records and delivers. **Red arm**
`w15/relay1352-redarm-mapread` (82212a446, never merge): one map read per
delivered event -- FAILS at 10 s, `a running turn stalled while the log map was
held exclusively`.

### Gates at f7f6b96d8

wcore-acp nextest 177/177 (every test above included). Clippy `--all-targets
-D warnings`: clean on Linux and on `--target x86_64-pc-windows-gnu`. wcore-cli
`test(acp_engine::)` 52/52. fix2-release built from f7f6b96d8
(`build --locked --release --features voice -p wcore-cli -j 6`), sha256
`b08ee6d7e00dad1b984719fab7c37f7a83fc95d346f1b6802f7b76a6aff74eb9`.

### Rerun A/B through the real `acp serve` (release)

Driver `instruments/w15-relay1352-mixed.v2.py` (sha256 5116380e..., adds
`--mixed-chunks`: first half of each turn in 512 KiB events, the rest in 4 KiB),
queue `instruments/queue-c2-rerun.sh` (sha256 5b2acee3...), receipts
`review/c2-rerun1/`. 2026-09-10 16:56:51-17:16:51Z, loadavg 30-47.

| arm | binary | schedule | c8 fast pass | c32 fast pass | stage-1 | stage-2 | slow+disc | peak RSS | quiescent first -> last |
|---|---|---|---|---|---|---|---|---|---|
| base-release | 9b2e24f2 | standard | 32/32 | 32/32 | 0 | 0 | 64/64 | 4,503,220,224 | 74,813,440 -> 116,445,184 |
| **fix2-release** | b08ee6d7 | standard | **32/32** | **32/32** | **0** | **0** | 64/64 | 4,552,253,440 | 74,018,816 -> 109,060,096 |
| base-release | 9b2e24f2 | churn, mixed chunks | 12/12 | 111/112 | 0 | 1 | 124/124 | 4,196,319,232 | 115,089,408 -> 136,523,776 |
| fix1 (pre-review, 671109fa9) | d73cb623 | churn, mixed chunks | 12/12 | 111/112 | 0 | 1 | 124/124 | 3,979,567,104 | 119,656,448 -> 138,596,352 |
| **fix2-release** | b08ee6d7 | churn, mixed chunks | 12/12 | **108/112** | **0** | **4** | 124/124 | 3,958,349,824 | 117,391,360 -> 136,876,032 |

Standard schedule `8,8,32,8,8,8,32,8,8,8`; churn schedule
`32,8,32,32,8,32,32,8,32,32` keeps history over the 64 MiB cap while sessions
close every cycle. Every batch of every arm quiesced.

**Drift, live.** `retained_total` cannot be read from outside a live process.
Drift low would make later turns detach at their first event, drift high would
detach them through early eviction; later churn batches did not degrade on any
binary, including fix1, which carries the race. That is consistent with the race
not being hit live and is not proof of absence -- the forced test is the proof.

**A separate finding, not this ticket's cancellation.** In the churn arms, fast
c32 rows were detached at STAGE 2 (`live delivery overloaded; resume from
retained event cursor`): 1/112 on base, 1/112 on fix1, 4/112 on fix2. Every such
row had received exactly 16 or 11 of its 512 KiB events (7,864,320 or 5,242,880
bytes) in 1.8-2.9 s, i.e. it was detached at or just before the switch to 4 KiB
events, when the recorder writes about 2,048 small events at once. That fits the
reader falling 256 positions behind its own recorder (the positions window) or
1,024 events behind it (the per-log event cap, a replay gap); these receipts
cannot tell which. It occurs on the base as well; 4/112 against 1/112 does not
establish that the repair makes it more likely (Fisher exact p about 0.4); and
it did not occur in any standard-schedule row (32 KiB chunks, the #1349 soak's
shape) in either A/B.

### Facts for the #1349 soak

* fix-release for the soak: source f7f6b96d8, sha256
  `b08ee6d7e00dad1b984719fab7c37f7a83fc95d346f1b6802f7b76a6aff74eb9`.
* The soak driver (`evidence/mixed-soak353/run2/driver.py`) DELETEs every session
  at the end of its cycle while the other cycles of its batch are still
  recording, and in a concurrency-32 batch about a third of the rows are 16 MiB
  turns whose logs reach the 8 MiB per-log cap, so retained history demand
  exceeds the 64 MiB cap: the soak does exercise session close while other
  sessions are over the cap. That is from reading the driver, not measured in
  the soak.

### Review-round commits on `w15/relay1352`

1. `bd11a056a` ledger: reopen #1352 c2 after adversarial review
2. `2f93f78b6` test(acp): force close-during-eviction drift; isolate the wait budget -- red on the unrepaired code
3. `ef0b0afcf` test(acp): end the fed turn before closing its session -- the first run of the exclusive-hold test failed only because its fed engine never ended, so close hit its 10 s deadline; the turn itself passed every assertion
4. `af7e6d53c` fix(acp): retire closed logs and pay eviction per append
5. `f7f6b96d8` test(acp): read the retained total from its new home -- af7e6d53c did not compile its tests on its own
6. the evidence and ledger commit that adds this section

Never merge: `w15/relay1352-redarm-drift` (73bccf836), `w15/relay1352-redarm-budget` (0579423cf), `w15/relay1352-redarm-mapread` (82212a446), plus the earlier `w15/relay1352-diag` and `w15/relay1352-redarm-lock`.

### Not claimed in this round

* That the stage-2 count-bound detach at mixed chunk sizes is harmless, or that
  the repair does not raise its rate: it is recorded above, its path is not
  named, and it needs its own decision.
* Live drift absence (not observable from outside the process).
* Everything in the earlier Not claimed list still stands, except that the cap
  is now stated with bounds instead of as exact.

## Second review round — adversarial review of f7f6b96d8

No blocker; one MAJOR and three minors, fixed as new commits on
`w15/stage2-1356` (base b348328cc, which already carries every #1352 commit).
Receipts for every run below are in `review2/`, all `"complete": true`.

### MAJOR 1 — concurrent evictors over-trimmed the largest log

Every over-cap recorder chose the largest log from the lock-free counters and
popped with no re-check, so evictors parked on that log's lock (behind a resume
copy, a delivery copy or its own append) each popped once more after the total
was already back under the cap.

**Forced.** `crates/wcore-acp/src/server/eviction_tests.rs::evictors_parked_on_one_victim_trim_it_only_as_far_as_the_cap_needs`
builds retained history just under the 64 MiB cap with one strictly largest
log, then starts four trigger turns: the first crosses the cap, the next three
keep it over. Each trigger's evictor chooses the same victim and parks at the
`cfg(test)` gate before popping; all four are then released together.
* f3b6ef72e (the test, unrepaired code): **FAIL**, "the largest log was trimmed
  4 events where one was enough" (remote_exit 100).
* f237dcf2b (`victim.mutate` returns 0 when `!history.over_cap()` under the
  victim's lock): passes.
* Red arm `w15/stage2-1356-redarm-overtrim` (6889d95b4, never merge), the fix
  with only the re-check removed: **FAIL** with the same message.

### MINOR 2, MINOR 3

* The `evict_for_append` doc no longer says a call never frees more than it
  added: a call stops once it has freed at least what its append added, so a
  small append can free one larger event.
* `RetainedHistory::account` saturates inside the `fetch_update` closure; the
  racy `store(0)` that could overwrite other logs' concurrent adds is gone. The
  comment now says it only fires when drift exceeds the whole remaining total,
  so it is a last resort and not a drift detector; the deterministic eviction
  tests are what prove the accounting exact.

### MINOR 4 — tests shared one process-wide delivery budget

* **Measured first** (e3adb7301, before the seam): `cargo test -p wcore-acp --lib`,
  one process, 5/5 runs at 145/145; `cargo test -p wcore-acp --test
  stabilization_backpressure`, 4/4 completed runs at 5/5 (the fifth was refused
  for a busy slot, `complete: false`, and does not count). No flake observed.
* **Isolated by construction anyway** (bf69d6ff3). `bounded::DeliveryBudget`
  owns the aggregate counter and its notifier. `DeliveryBudget::process()` is
  the single static production uses, and the free functions `channel`,
  `retain` and `retain_encoded` still use it, so wcore-cli and every production
  path are unchanged and nothing is relaxed. `AcpServer::with_isolated_delivery_budget()`
  (hidden, documented test-only) gives one test's server its own budget; the
  eviction tests, the #1352 server tests, the streaming integration tests in
  `stabilization_backpressure.rs` and `stage2_small_event_burst.rs` opt in.
  `bounded::backpressure_tests::exhausting_an_isolated_budget_leaves_other_budgets_untouched`
  fills one isolated budget to refusal and shows another still admits a channel
  and a charge. After the seam, the one-process lib suite ran 3/3 at 147/147.

### Gates at bf69d6ff3

* wcore-acp nextest **179/180**. The one failure is
  `stage2_small_event_burst::a_reader_inside_its_budget_and_replay_window_is_not_detached_by_small_events`,
  the wayland#1356 c1 red test committed at e3adb7301: a separate ticket, red by
  design until that repair lands.
* clippy `--all-targets -D warnings`: clean on Linux and on
  `--target x86_64-pc-windows-gnu`.
* wcore-cli `test(acp_engine::)`: 52/52.

### Second-round commits on `w15/stage2-1356`

1. `f3b6ef72e` test(acp): parked evictors must not over-trim the largest log -- red on the unrepaired code
2. `f237dcf2b` fix(acp): re-check the cap under the victim's lock (also MINOR 2 and MINOR 3)
3. `bf69d6ff3` test(acp): isolate each heavy test's delivery budget (MINOR 4)
4. the ledger and evidence commit that adds this section

Never merge: `w15/stage2-1356-redarm-overtrim` (6889d95b4).

### Not claimed in this round

* The `acp serve` A/B was not rerun after these fixes; it is held while the
  #1349 soak runs.
* That MAJOR 1 explains the churn arm's stage-2 detaches. The base binary
  9b2e24f2, which has no such evictor, also detached 1/112, and the in-process
  instrument for wayland#1356 names the stage-2 positions window instead.
