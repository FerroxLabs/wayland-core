# wayland#1349 c2 — the single-flight repair, and what it leaves

Lane w15/leak2, 2026-09-10. Host `hetzner-dsm` (Linux 6.8.0-101, 96 CPU,
251 GiB), shared. Builds through `tools/remote-proof.py` slot `parallel-1`;
every receipt quoted here has `"complete": true`.

    base  38f82b87fbe2aabd01f06c469d3bab8c602c7987   binary sha256 6cc6a44b...
    fix   008d88ce960322967adcef7637b44a5cfad44207   binary sha256 d0819d10...
          = f6eccfda5 (the repair) + 008d88ce9 (doc-only follow-up)

## The repair

`crates/wcore-agent/src/recovery_confidential.rs`. c1 named the dominant term:
every engine started its own `wayland-recovery-key` loader thread, the file
store serialises loads on `credentials.confidential-key.lock`, and each load
that outlived its caller's budget stranded one live thread and one held lock fd.

Engines asking for the key of exactly the same store at the same time now share
ONE in-flight load (`start_or_join_key_load`). Only a load in flight is shared.
A flight leaves the registry before its answer is published, so no key and no
refusal ever answers a caller that arrived after it was produced. The share key
is `KeyLoadIdentity`: `[storage.credentials]`, the resolved credentials path,
the working directory, a digest of the complete environment, and whether the
load may create the key. It never keys on the backend label, and anything that
cannot be resolved loads alone. Per-engine budgets, the #1289 NeverAsked
extension and the failure-authority rule are unchanged.

No `SOURCE_INPUTS` path was touched (`engine.rs` and `bootstrap.rs` are
unmodified).

## Files

| file | what it holds |
|---|---|
| `red-arms.txt` | the three committed mutations (backend-label key, never share, keep settled flights) and the exact test each one reds |
| `security-ab.txt` | base-vs-fix runs of the recovery, credential, durable-session and sigkill-recovery suites: 71/5/96 on base, 76/5/96 on fix, all green |
| `census-and-authz-base.txt` | the live `acp serve` measurements: the thread/fd census base vs fix, the four refusal modes base vs fix, the 640-cycle slope A/B, smaps by mapping, the payload term, and the relay limit found on the way |
| `soak-run1.txt` | the fresh 7200-second soak against the original limits |

## Findings, in one place

1. **The census collapses.** At concurrency 32 under the soak's own
   `require_durability = true`, the base strands 293 threads and 293 lock fds
   after 320 deleted sessions and refuses 318 of 320 turns at about 5.1 s. The
   fix strands 0 and 0 and refuses none. The #1289 extension does not change
   the base's refusals, because the store was asked.
2. **The authorization boundary holds.** Live, on both binaries: a wrong
   passphrase, a vault corrupted mid-run and a plaintext backend each serve 0 of
   123 turns at concurrency 1, 8 and 32. A healthy vault goes from 5/123 to
   123/123 served.
3. **The payload term is a bounded warm-up.** At concurrency 1 with 16 MiB
   payloads and n=70, RSS rises about 11 MB over the first ~30 cycles and then
   stays flat (second-half slope −6,087 B/cycle, R² 0.067). The linear ~248 MB
   projection is refuted.
4. **A second concurrency term remains.** The fix halves the concurrency-32
   slope (109,058 → about 55,000 B/cycle), and what is left is glibc arena
   retention: RSS rises inside a flat count of anonymous mappings, after the
   idle trim, with no stranded threads. It is decelerating but not flat by
   cycle 640.
5. **A pre-existing functional limit.** Eight concurrent fast 16 MiB readers
   are cancelled by the ACP relay bound (`protocol relay overloaded`) on the
   base as well as the fix.

## The soak, and the c2 verdict

A fresh 7200-second soak ran on hetzner-dsm (`soak-run1.txt`). It used the
original driver, with only its pinned binary hash changed, against the fix
binary. It launched at loadavg 26.83 with no builds running, and **aborted at
125.162 s**. In batch 5 (concurrency 32), 1 of its 82 cycles failed the driver's
per-row check: a fast 16 MiB reader was cancelled with `protocol relay
overloaded` after 3,932,160 bytes. The driver aborts on any failing row, so it
never computed `rss_bound_pass`. There is no RSS verdict against either original
limit (127,151,104 mixed, 33,554,432 confirmation). No limit was moved and the
oracle was not relaxed.

**c2 stays `not-met`.** What is left, in order:

1. **The relay cancellation of concurrent fast 16 MiB readers.** It fails on
   the base as well as the fix, and it currently prevents any fresh soak from
   completing on this tree. It needs its own carrier.
2. **The arena-retention concurrency term.** It remains after the repair and
   is decelerating but not flat by cycle 640 (second-half slope 30–49 KB/cycle
   at concurrency 32). Once (1) is out of the way, it is likely to exceed the
   32 MiB floor that the driver's `max(10%, 32 MiB)` formula applies whenever
   the first-100 median is below 335,544,320 bytes.
3. **One 7200-second soak, graded against the original limits**, after (1)
   and (2).
