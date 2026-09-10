# wayland#1356 — stage-2 detach of a keeping-up reader on a large-then-small turn

Lane w15/stage2-1356, 2026-09-11. Host `hetzner-dsm`, shared; builds and tests
through `tools/remote-proof.py` slot `parallel-1`, every receipt quoted has
`"complete": true` (`receipts/receipts-index.txt`). Branch base b348328cc.

## The four detach paths, from the code

Stage-2 delivery (`AcpServer::tee_into_log`, b348328cc) detached a reader with
the ONE terminal `live delivery overloaded; resume from retained event cursor`
from four different places, so the error text cannot say which fired:

| path | limit | where |
|---|---|---|
| (A) positions window | the recorder's per-event position handoff refuses its 256th pending position (`LIVE_EVENTS`) | `bounded::Sender::send`, then delivery reads the reserved `Overload` terminal |
| (B) cursor charge | the aggregate refuses the tiny position charge | `bounded::retain(&cursor)` |
| (C) replay gap | the session log no longer holds the next event (1,024 events or 8 MiB per log, 64 MiB total) | `EventLog::next_after` -> TooOld |
| (D) wait budget | the reader left the byte-bounded live channel full for one second in total | `Sender::send_retained` |

Only (D) is the "real delivery pressure" the code comments name as the detach
criterion ("it retains the one-second bound").

## c1 — the mechanism, named by instrument

**Instrument** (branch `w15/stage2-1356-diag`, 2fe327290, never merge): a
`W1356DIAG` line at every stage-2 exit with the delivered count, the cursor
asked for, the log tip, oldest retained position, retained count and bytes and
the budget left, plus one where the recorder's position handoff is refused.
Driven in-process by one 16 x 512 KiB + 2,048 x 4 KiB turn against three
reader models (`tests/stage2_1356_diag.rs` on that branch):

| reader | outcome | instrument |
|---|---|---|
| paced, one virtual ms per frame (512 MiB/s on large frames) | detached after 3 frames, 1 MiB | `site=positions_full recorder_position=259 live_events_limit=256`, then `site=positions_overflow delivered=3 budget_left_ms=999 log_tip=2065 oldest_available=1042 retained_len=1024 retained_bytes=4222010` |
| unpaced, in process | all 2,065 frames, 16,777,216 bytes, Done | `site=completed delivered=2065` |
| real REST loopback (reqwest) | all 2,065 frames, 16,777,216 bytes, Done | `site=completed delivered=2065` |

**Named.** (A) fired first: a reader that takes any time per frame while the
recorder runs on is detached once 256 positions are pending, with 999 ms of its
one-second budget unspent. (C) stood directly behind it: by then the log had
already evicted positions 1-1,041 of a 2,065-event turn.

**Deterministic red test.**
`crates/wcore-acp/tests/stage2_small_event_burst.rs::a_reader_inside_its_budget_and_replay_window_is_not_detached_by_small_events`:
8 x 512 KiB + 512 x 4 KiB (521 events, ~6.1 MiB, entirely inside one log's
replay window), a reader paced at one virtual millisecond per frame on paused
time, and an assertion that `events_since(genesis)` still returns all 521
events, so (C) cannot be the cause. Committed first (e3adb7301) and run on the
unrepaired code: **FAIL**, detached after 3 frames (remote_exit 100).

LIVE CONFIRMATION: PLACEHOLDER (diag release on the churn arm, held for the #1349 soak).

## The repair (47e4a0d7a; leaked across turns, fixed by 4ab69b043, see Review round below)

Delivery no longer takes positions from a per-event queue. The recorder
publishes where the turn starts, then signals a `tokio::sync::watch` once per
event it records; delivery reads each next event from the session log itself,
waits on the watch only when caught up, and detaches only on (D) its unchanged
one-second budget, or (C) a position the log no longer retains (or a refused
charge / no log). The position queue and its count limit are gone; no bound was
raised (the live channel's 1 MiB byte bound, the one-second budget and the
replay window are unchanged).

* The red test passes at 47e4a0d7a.
* `a_reader_that_falls_out_of_the_replay_window_is_detached_and_cannot_resume`:
  the full 2,065-event turn read at the same pace IS still detached, and a
  resume from its last delivered frame is refused as TooOld.
* Gates at 47e4a0d7a: wcore-acp nextest 181/181; `cargo test -p wcore-acp --lib`
  in one process 2/2 at 147/147; clippy `-D warnings` clean on Linux and on
  `x86_64-pc-windows-gnu`; wcore-cli `acp_engine` 52/52.

## c3 — why the detach is not free, whatever the limit

* The SSE frames `acp serve` streams (`transport/rest.rs::prompt`) carry
  `event:` and `data:` only, no `id:`, and `MessageEvent` has no stream id or
  position; `SessionCreateResponse` and `SessionMetadata` carry neither. A host
  is never told the cursor of the frames it received.
* `acp serve` never calls `with_event_retention`, so each log keeps at most
  1,024 events and 8 MiB. A 16 MiB / 2,065-event turn exceeds that window.
* The resume route exists (`GET /sessions/:id/events`, merged into `acp serve`),
  but for exactly this case it refuses: the replay-window test above shows
  TooOld (HTTP 410) for the first undelivered position.

LIVE A/B: PLACEHOLDER (held for the #1349 soak).

## Not claimed

* That the live churn detaches were path (A): not yet instrumented live.
* That the repair clears the churn arm: not yet run.
* That a reader that falls out of the replay window can recover by resume: it cannot.

## Review round: turn scoping (BLOCKER on 47e4a0d7a)

47e4a0d7a delivered stage 2 from the session log after the turn's start
cursor. Two overlapping turns on one session interleave in that log, so a
turn's live stream could carry another turn's events and stop at that
turn's terminal.

Red first, committed before the fix: bccdbab16.

- `crates/wcore-acp/tests/stage2_turn_scoping.rs`: turn A records `A-1`,
  turn B starts, A records `A-2` and Done, B records `B-1` and Done. On
  47e4a0d7a B's stream was `[A-2, Done(A's turn_id)]` against `[B-1]`.
- `crates/wcore-cli/tests/stabilization_acp_lifecycle.rs`:
  `assert_one_terminal` now returns the terminal's turn_id; on 47e4a0d7a
  `delete_cancels_a_queued_turn_before_it_reaches_the_provider` failed with
  the queued stream ending at the first turn's turn_id.

Fix: 4ab69b043 (plus 581178a4b, the one Cargo.lock line cargo records for
the `test-support` self dev-dependency). The recorder tags each appended
event `(turn, seq)` and publishes the count recorded over a watch channel;
delivery looks up exactly its own next sequence. A recorded sequence no
longer retained is an eviction of this turn and detaches; foreign entries
are never delivered.

A limit was REMOVED, not raised: the 256-position run-ahead cap (the
positions handoff that refused the 259th position against LIVE_EVENTS 256)
is gone since 47e4a0d7a and stays gone. A reader's lag is bounded only by
the session log's replay window (1,024 events, 8 MiB per log, under the
64 MiB retained-history cap) and its one-second wait budget.

Also this round: `DeliveryBudget::isolated`, `charged` and
`AcpServer::with_isolated_delivery_budget` are compiled only under
`cfg(test)` or the dev-only `test-support` feature, and the cross-budget
rebalance in `send_retained` is gone (a debug_assert remains). The
falls-out-of-window test uses an isolated budget and is detached through
the replay gap, not the budget.
