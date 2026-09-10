# wayland#1349 — where the 5.28 GB actually lives

Host `hetzner-dsm` (Linux 6.8.0-101, 96 CPU, 251 GiB). Product surface identical
to `evidence/mixed-soak353/driver.py`: `wayland-core acp serve`, one cycle =
`POST /v1/sessions` -> `POST /v1/sessions/{id}/prompt` (streamed against a local
SSE fixture provider) -> `DELETE /v1/sessions/{id}` -> `GET` 404. Quiescent RSS is
sampled once per batch after the batch's DELETEs and `GET /v1/sessions` == `[]`.

Binaries, both `cargo build --bin wayland-core` (debug, `-C debuginfo=0`, mold)
through `tools/remote-proof.py` slot `parallel-2`, each with `remote_exit 0` and
`"complete": true`:

    base   e7b46387eaf09cb51893d653f9549eeef6116402
           sha256 2d6d69b16cf6a4cf75499647c994b88c657f2f346759a8113a61ba82c53af27d
    fixed  16a34638c45947699648a8528120d2cf29e68370
           sha256 2d69764b1037cabe7cb60e462165ee3a9b308fe2f543fe56c35c1e2ff693babc

Probe driver: `leak-probe`, a reduced form of the soak driver whose independent
variable is CYCLE COUNT rather than wall time. Files under `/root/w15-leak/` on
`hetzner-dsm`; each arm writes a `receipt.json` with the full per-cycle series.

## 1. The growth reproduces in hundreds of cycles, not 8,938

    arm         source     conc payload  durab.    n    pass  B/cycle   R^2    hdr_ms first10 -> last10
    probe-a     e7b46387e   1   32 KiB   required  400  400/400   7,033  0.850   63.9 ->  112.1
    od1         e7b46387e   1   32 KiB   optional  300  300/300  12,908  0.734   67.0 ->   82.7
    od8         e7b46387e   8   32 KiB   optional  320  320/320  69,136  0.166  140.9 ->  136.3
    od32        e7b46387e  32   32 KiB   optional  640  640/640 101,104  0.671  148.9 ->  450.7
    od32arena2  e7b46387e  32   32 KiB   optional  640  640/640  99,704  0.873  237.8 ->  193.0  (MALLOC_ARENA_MAX=2)
    fixA        16a34638c   1   32 KiB   required  400  400/400   3,120  0.359   65.1 ->   95.5
    fix-od32    16a34638c  32   32 KiB   optional  640  640/640 114,202  0.786  241.8 ->  143.1

For comparison the two-hour soak measured 417,727 B/cycle at R^2 0.8432 over
8,938 cycles with concurrency [1,8,32] and payloads [32 KiB, 1 MiB, 16 MiB].

Two things the table settles that the receipt arithmetic could not:

* THE GROWTH SCALES WITH CONCURRENCY. Holding payload at 32 KiB, the
  per-session slope goes 7,033 -> 69,136 -> 101,104 B/cycle across concurrency
  1 -> 8 -> 32. That is the axis the soak's mixed [1,8,32] matrix rode.
* PAYLOAD IS *NOT* EXCLUDED, and an earlier draft of this file wrongly said it
  was. `pay16` (concurrency 1, 16 MiB payload, n=60, 60/60 pass) goes 141.9 MB
  -> 153.5 MB: 101,883 B/cycle over the whole run at R^2 0.565, and 83,247
  B/cycle at R^2 0.562 over the last 30 batches with the warm-up dropped. At
  n=60 that cannot be separated from a saturating curve, but it is plainly not
  flat, and that process carries ZERO leaked recovery threads (section 3). So
  there are TWO terms, not one: the thread leak on the concurrency axis, and a
  second payload-driven term at concurrency 1 that heaptrack also cannot see —
  consistent with glibc retaining large freed transient buffers that
  `malloc_trim(0)` cannot return because they are not at the arena top. The
  second term is measured here but NOT named; it needs its own instrument
  (`/proc/<pid>/smaps` deltas across a payload sweep) and more than 60 points.
* THE 28x TTFB DEGRADATION REPRODUCES. `od32` runs 148.9 ms -> 450.7 ms over
  640 cycles, the same shape the soak saw over 8,938.

## 2. It is NOT a heap leak — heaptrack says so directly

`heaptrack` (Ubuntu 1.5.0) wrapping the same binary, same cycle, A/B on session
count. `heaptrack_print -f <big> -d <small> -l 1` diffs leaked bytes by stack.

    arm        conc   sessions   total memory leaked
    ht50         1        50           6.87 M
    ht200        1       200           7.42 M      -> 3,845 B/session
    fix-ht50     1        50           6.33 M
    fix-ht200    1       200           6.39 M      ->   419 B/session
    c32ht64     32        64         120.58 M
    c32ht320    32       320         120.92 M      -> 1,329 B/session

At concurrency 32 the LIVE HEAP IS FLAT — 120.58 M at 64 sessions, 120.92 M at
320 sessions, a 0.34 MB difference across a 5x increase in sessions — while
quiescent RSS over the same run rises linearly (R^2 0.959 on the heaptrack arm,
0.671 on the clean `od32` arm). Whatever holds the memory is not a reachable
malloc'd object, so no allocation-site profile can name it. That is why the
first pass at this ticket found only small sites.

`MALLOC_ARENA_MAX=2` (`od32arena2`) changes the slope by 1.4% — glibc arena
count is refuted as the mechanism too.

## 3. NAMED: one abandoned OS thread and one open lock fd per session

`crates/wcore-agent/src/recovery_confidential.rs:691`, in `acquire_key`:

    std::thread::Builder::new()
        .name("wayland-recovery-key".to_owned())
        .spawn(move || { let _ = tx.send(load()); })

The doc comment above it states the design outright: *"The load runs on its own
thread because the store call is synchronous and uncancellable: a deadline can
only be imposed on the WAIT, never on the call. That is why a timeout leaves a
thread behind."* On `KEY_STORE_ACQUIRE_BUDGET` expiry the receiver is parked in
`ProtectorState::pending` and the loader thread is abandoned. `ProtectorState`
is per-engine and dies with the session; the THREAD does not.

The confidential-key load serialises on `credentials.confidential-key.lock`, so
under concurrency all but one session per wave exceed the budget. Live census of
the running ACP process, taken with every session already deleted and
`GET /v1/sessions` returning `[]`:

    arm         session files  wayland-recovery-key threads  total threads  confidential-key.lock fds  VmRSS
    od1  (c=1)      1502                    0                     102                  0               141,292 kB
    pay16(c=1)       287                    0                     101                  0               364,920 kB  (mid-16 MiB-stream)
    fixA (c=1)      1810                    1                     103                  1               201,296 kB
    od32arena2(c=32) ~461 sessions        462                     563                461               292,228 kB

One leaked thread and one leaked fd PER SESSION at concurrency 32, ZERO at
concurrency 1 — the same split as the RSS slope (101,104 vs 7,033 B/cycle).
`VmData` on the concurrency-32 process was 2,118,644 kB, consistent with ~560
thread stacks. Thread stacks are mapped by pthread, not malloc'd, which is
exactly why heaptrack reports flat live heap while RSS climbs.

This mechanism accounts for every property the soak receipt established:
linear in cycle count, independent of reader mode, per-session, surviving the
session's deletion, and degrading time-to-first-byte (each new session's key
load queues behind the accumulated holders of the same lock file). It does NOT
account for the concurrency-1 payload term in section 1 — that is a second,
still-unnamed contribution, and 417,727 B/cycle in the soak is the sum of both.

## 4. Separately: at HEAD, concurrency refuses turns outright

With the soak's own `[session] require_durability = true`, every concurrent turn
but one fails in 5.09 s with a single terminal frame:

    {"kind":"error","error":{"code":-32003,"message":"Session persistence authority
     unavailable: [session] require_durability = true, but this profile's credential
     store was asked for the recovery key and did not answer in time, ..."}}

Measured at e7b46387e on 6 concurrent prompts: 5 refused at 5.087-5.091 s, 1
served. At concurrency 8 it was 238 of 240 cycles; at 32, 318 of 320. The soak
at source 3530199fb reported 0 failures in 8,938 cycles at the same
concurrencies, so this is either a regression since 3530199fb or it is
load-dependent — NOT ESTABLISHED EITHER WAY here, and it needs its own ticket.
It is the same lock contention as section 3, seen from the caller's side.
All concurrency arms above therefore run `require_durability = false`, where the
timeout degrades to a notice and the turn completes (640/640 exact-byte passes).

## 5. What was repaired, and what was not

REPAIRED (commit `16a34638c`): `crates/wcore-agent/src/bootstrap.rs:3091` dropped
the `JoinHandle` from `ApprovalBridge::spawn_reaper` into `_reaper_handle`, which
DETACHES the task rather than aborting it, while the comment beside it claimed
the opposite. One immortal 30-second ticker per session-ever-created, each
holding an `ApprovalBridge` clone for the life of the process. The heaptrack diff
attributed exactly 150 leaked allocations — one per extra session — to that call
site plus 150 each to `ApprovalBridge::new` reached through it. Parking the
handle on `engine.push_decay_handle` (the contract the memory decay scheduler and
the agent bus observer already use) takes per-session leaked heap from 3,845 B to
419 B and the concurrency-1 RSS slope from 7,033 to 3,120 B/cycle.

NOT REPAIRED: the abandoned `wayland-recovery-key` thread of section 3, which is
the dominant term. It is not a one-line fix — the load is abandoned by design
because the store call is uncancellable, so the fix has to remove the NEED to
re-run it: memoize the loaded confidential key per process, keyed by the resolved
store identity, so the second and later sessions never start a loader at all.
That is a change to a security-sensitive path and it should not be made without
review. Until it lands, no soak can pass the original limits, so c2 stays open.
