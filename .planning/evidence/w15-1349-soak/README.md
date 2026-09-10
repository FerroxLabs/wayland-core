# wayland#1349 c2 -- the fresh 7200-second soak on the release+voice binary

Lane w15/soak1349, 2026-09-10. Host `hetzner-dsm` (Linux 6.8.0-101-generic,
96 CPU, MemTotal 263,609,704 kB), the same box both original runs used. Shared
host. One run, no relaunch.

## Result: PASS on the driver's own verdict, against both original limits

| field | value |
|---|---|
| receipt | `/root/w15-soak1349/soak/run1/receipt.json` on hetzner-dsm, sha256 `4be438ae06d15401daad44d78293cd0225736abacebdef0eb7e753ede1cd1afe` (19.6 MB, kept on the host) |
| completed / error | `true` / `null` |
| elapsed | 7,201.81 s (soak_started 17:32:54.99Z, finished 19:32:57.11Z) |
| batches / cycles | 720 / **9,840**, all 9,840 passing (240 batches at each of concurrency 1, 8, 32) |
| rows with an error frame | 0 (so no relay cancellation and no stage-2 detach) |
| first-100 quiescent median | 100,139,008 |
| last-100 quiescent median | 91,203,584 |
| growth | **-8,935,424** |
| driver `max_growth_bytes` | 33,554,432 (the 32 MiB floor, because the first-100 median is below 335,544,320) |
| `rss_bound_pass` | **true** |
| growth <= 127,151,104 (original mixed limit) | true |
| growth <= 33,554,432 (original confirmation limit) | true |
| cleanup | `remaining_owned_pids []`, `sampler_alive false` |
| namespace | fresh `unshare --net`, `routes []` |
| informational | LSQ slope -2,694 B/cycle (R^2 0.52); max batch peak RSS 1,728,745,472 |

The per-row pass table (concurrency x reader x payload, 27 cells, every cell
N/N) is in `run1-summary.txt`. `run1-batches.csv` has every batch's quiescent
and peak RSS, so the medians can be recomputed without the receipt.

**Cycle count against the originals.** The soak is bounded by 7200 s, not by
cycles. This run reached 9,840 cycles, more than the original mixed run's 8,938
(3530199fb) and the confirmation run's 8,897 (6b7da6a0f). So the verdict covers
at least as many cycles as either original.

**Why one run covers both profiles.** `evidence/overnight-memory/confirmation2h/driver.py`
differs from `evidence/mixed-soak353/run2/driver.py` on line 234 only, the
pinned binary hash (diff taken on hetzner-dsm). Both runs are the same soak
under the same `max(10%, 32 MiB)` formula, on different binaries. 127,151,104
is 10% of 3530199fb's 1,271,511,040 first-100 median. 33,554,432 is the floor,
applied because 6b7da6a0f's median was 93,503,488. This run's limit is that
same floor, and its growth is negative, so both numbers are met.

## What ran

- **Source** `b348328cc139c70e86bd3254b1afb743e79a3a15` (integration: #1349
  single-flight, the #1352 relay repair with its close-during-eviction review
  fix, #1353, #1354, #1355).
- **Binary** sha256 `a24886726e8cf9b84d75afe6347e98b8ead301654699b9ea0e92c06e22569e84`,
  `--build-info` = `wayland-core 0.13.14 (source b348328cc...)`, ELF `.comment`
  rustc 1.95.0 + mold 2.30.0. Pinned by copy to `/root/w15-soak1349/bin/`
  before the shared slot could overwrite it.
- **Build**: `WCORE_PROOF_SLOT=parallel-1 python3 tools/remote-proof.py worktrees/w15-soak1349 build --locked --release --features voice -p wcore-cli -j 6`.
  Receipt `build-proof-1789061230-4361.json`: remote_exit 0, complete true, 323.8 s.
- **How that recipe was matched, from receipts rather than assumed.** The
  original binaries are `/root/waylandcore-stabilization-20260905/binaries/wayland-core-release-voice-{3530199fb...,6b7da6a0f...}`
  (sha256 `c68855d5...` and `9fc61db0...`, re-hashed live). They were built by
  the same adapter, per `evidence/proof-1788700021-35225.json` (3530199fb) and
  `evidence/proof-1788736282-41519.json` (6b7da6a0f): `build --locked --release
  --features voice -p wcore-cli -j 6` (`-j6` on the second), remote_exit 0,
  complete true. The adapter sets `CARGO_INCREMENTAL=0` and `RUSTFLAGS='-C
  debuginfo=0 -C link-arg=-fuse-ld=mold'`, and both originals' ELF `.comment`
  show mold. Both original receipts record `build_profile: release-voice`, a
  label the driver only writes when it is passed. This run passed
  `--build-profile release-voice` too. The aborted w15/leak2 run1 used a DEBUG
  binary (`cargo build --bin wayland-core`) and passed no label.
- **Driver**: `evidence/mixed-soak353/run2/driver.py`, sha256
  `f64b20bd620911a08406449817f88a9df96d8ddc3dc0b2673e75f261ede318ce`, copied
  with exactly one line changed (`driver-run1.diff`), run copy sha256
  `3c62f3a345186f98d4e2d25853ff734a684fd8a8a0d9a08a6561e28736ad040d`:

      234c234
      <  assert receipt["binary_sha256"] == "c68855d5...18da", "immutable binary hash mismatch"
      >  assert receipt["binary_sha256"] == "a2488672...9e84", "immutable binary hash mismatch"

  The oracle is untouched: 7200 s, concurrency [1, 8, 32], readers [fast,
  disconnected, slow], payloads [32 KiB, 1 MiB, 16 MiB], `[session]
  require_durability = true`, encrypted_file vault, `rss_bound_pass` formula.
- **Launcher** `instruments/soak-launch.sh`. It refuses unless the original
  driver hashes to f64b20bd..., the binary's build-info names the source, and
  the diff is one hunk of two lines. It waits up to 1200 s for no cargo, rustc,
  cargo-nextest or mold process, then launches under `setsid nohup unshare
  --net` and records what was running. **Monitor** `instruments/soak-monitor.py`
  wrote `run1.status.jsonl` once a minute (121 records). **Summary**
  `instruments/soak-summary.py`.
- **Positive control for the graders.** `control/run2-summary.txt` is
  `soak-summary.py` run on the original 3530199fb receipt. It reproduces the
  ledger's own figures exactly: 8,938 cycles, growth 5,277,444,096 (41.505x),
  417,726.7 B/cycle at R^2 0.8432, headers_ms 59.20 -> 1663.83, `rss_bound_pass`
  False. The monitor's final-record path was checked on the same receipt.

## Load

- **At launch** (`run1.launch`, `run1.wait`): 17:32:54Z, loadavg `29.02 28.61
  31.38`, 0 build processes running, 0 s waited.
- **Periodic** (1-minute loadavg from `run1.status.jsonl`):
  - 17:32-18:00: 26.3-30.7.
  - A spike of 33.4 -> 94.6 -> 34.2 at 18:01-18:09, while other lanes' builds
    ran (monitor build count 13-16; cargo cwds in `/root/w-f13/integ` and in
    this adapter's default and parallel-1 slots).
  - 18:10:59Z: the coordinator killed 24 orphaned CPU busy-loops
    (`/root/scratch/f14load.sh`, about 14.8 days old) that made up much of the
    box's long-standing baseline of about 26-30. From 18:11 loadavg was
    15.3 -> 2.7-13.0.
  - 19:00-19:33: 2.8-8.3.
- **At end**: `4.99 4.21 4.75` (receipt), `4.18 4.07 4.69` at the monitor's
  final record, 19:33:12Z.
- No other `wayland-core` process ran on the host at any of the seven census
  points taken between 17:49Z and 18:45Z, listed by executable path. There was
  no census after 18:45Z.

**Consequence for the verdict, stated rather than assumed away.** The first-100
window ran at loadavg about 29 and the last-100 window at about 4. That confound
cannot explain a passing RSS result: quiescent RSS is taken after every batch
settles, and lower load near the end is not a mechanism that would release
memory retained earlier. It is disclosed because the two windows did not run
under the same host conditions.

## Not claimed

- **Latency, not graded here.** At concurrency 32, the median time to headers
  rose from 69.8 ms (first 10 batches) to 1,307.5 ms (last 10), and the median
  fast turn from 358 to 2,428 ms, while load FELL (`latency-conc32.txt`). The
  original run showed the same shape. c2 does not grade it and this lane does
  not attribute it.
- **Replication.** This is one run. It does not show the result would repeat,
  or hold on other hosts.
- **Code paths outside the soak.** The glibc arena term the leak2 lane measured
  at n=640 (30-49 KB/cycle, decelerating) did not show up as growth here: the
  slope is negative over 9,840 cycles. This run does not test whether that term
  would appear under a schedule other than the soak's.
