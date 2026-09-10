## c3 — the concurrency-1 16 MiB latency, measured on both sources

Driver `w15-leak2-probe6.py` (sha256 ba74608b...), one fast reader, 16 MiB in
32 KiB provider chunks, `[session] require_durability = true` (the soak's own
config). Arms interleaved in blocks of 5 so host drift lands on every arm.

**Run 1** (`c3-latency1`, 15:30-15:36Z, loadavg 28.3-31.6), n=10 per arm, 10/10 pass each:

| arm | source | profile | median | min-max |
|---|---|---|---|---|
| dbg-base | 008d88ce9 | debug | 22,405 ms | 20,900-23,412 |
| rel-base | cd85dac88 | release | 3,754 ms | 3,021-4,422 |
| rel-3530 | 3530199fb | release | 1,892 ms | 1,767-4,156 |

**Run 2** (`c3-trim1`, 15:41-15:43Z, loadavg 27.6-29.6), n=10 per arm, 10/10 pass each:

| arm | source | profile | median | min-max |
|---|---|---|---|---|
| rel-base | cd85dac88 | release | 1,946 ms | 1,887-4,258 |
| rel-base + `GLIBC_TUNABLES=glibc.malloc.trim_threshold=268435456` | cd85dac88 | release | 1,962 ms | 1,846-2,337 |
| rel-3530 | 3530199fb | release | 1,754 ms | 1,599-4,117 |

The fix does not move it either: fix-debug (671109fa9) at concurrency 1 took
21,900-23,265 ms (n=3), the same as base-debug.

**Named cause: the build profile of the binary under test, not the source.**
The "~21.7 s on this tree" figure was measured on base-debug (`cargo build --bin
wayland-core`, debug_info, as the w15/leak2 lane built it); the "~2.6 s at
3530199fb" figure was the soak's `release-voice` binary. Measured on the same
driver and interleaved, the SAME code goes from 22.4 s (debug) to 1.9-3.8 s
(release), and the two sources at release are within 1.9-3.8 s vs 1.75-1.9 s.
The diag instrument places the debug time in the engine and projection, not the
relay: at concurrency 1 the recorder spent 19.9-20.4 s of each 22 s turn waiting
for upstream, and the stage-1 channel never made the engine wait (0 waits).

**What was measured and ruled out on the way.** An strace memory-syscall census
(2 turns each) found the release base making 22,637 `brk` calls against 17 at
3530199fb. It comes from `crates/wcore-config/src/allocator.rs`, new on this
line, whose `mallopt(M_MMAP_THRESHOLD, 256 KiB)` disables glibc's dynamic
threshold and leaves the main heap trimming and regrowing at the default trim
threshold. Raising only the trim threshold through a glibc tunable, same
binary, cut `brk` to 66 calls -- and the median did not move (1,962 vs 1,946 ms).
That churn is real but is NOT the latency.

**Not claimed.** A residual release-vs-release difference is not attributed:
the base/3530 ratio was 1.98 in run 1 and 1.11 in run 2, so it is not stable at
n=10 on this shared host. No latency repair was made, and allocator.rs (the
#1349 memory work) was not changed.
