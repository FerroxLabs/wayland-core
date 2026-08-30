---
issue: 395
repo: FerroxLabs/wayland-core
kind: defect
title: "engine.run() cost is ~linear in tool-result size (~100 s/MB in the test profile), and it is not the spill path"
status: open
last_verified_commit: 7e27d9562
criteria:
  - id: c1
    text: "The debug-vs-release question is SETTLED by measurement: the same probe is run under --release at 240,000 and 480,000 chars, and the per-byte term is either reproduced or shown to collapse"
    state: not-met
    owner: core
    note: "Filed 2026-08-30, split out of wayland-core#378 c1 by the w3-test-infra lane. NOTHING HAS BEEN DONE on this row. The debug-profile measurement that motivates it is complete and is quoted on #378 c1: hetzner-dsm, binary alone, phases timed separately, 240,000 chars -> engine.run() 24.586 s and 480,000 -> 48.763 s at host load ~40, i.e. 2.00x the bytes for 1.98x the seconds. NO RELEASE ARM WAS EVER RUN, and until one is this must not be described as a user-facing latency: everything measured is the unoptimized test profile, where a per-byte constant of ~100 ns is unremarkable for an ordinary linear scan that would be ~2 ns optimized. If release collapses it, the honest close is 'test-profile artifact', not a product fix."
  - id: c2
    text: "If the per-byte term survives release, the function carrying it is NAMED by measurement -- a profiler, or bisecting instrumentation inside run_turn -- and not inferred"
    state: not-met
    owner: core
    note: "Filed 2026-08-30. NOTHING HAS BEEN DONE. Blocked behind c1 by construction -- if release collapses the term there is no function to name. TOOLING NOTE, measured rather than assumed: hetzner-dsm has NEITHER perf NOR gdb (`which perf gdb` returns nothing), which is why the w3 lane attributed by CONTROL (shed on vs shed off, interleaved) instead of by profile. ONE CANDIDATE ALREADY READ AND FOUND UNPROMISING, recorded so it is not re-read: `wcore-agent::compact::estimate::estimate_tokens_from_messages_inner` is O(n) over `.len()` with no allocation, which cannot account for ~100 ns/byte. That is one inference, not a finding; c2 asks for a measurement."
  - id: c3
    text: "A regression guard exists for whichever answer c1 gives"
    state: not-met
    owner: core
    note: "Filed 2026-08-30. NOTHING HAS BEEN DONE. The instrument this row would build on already exists and is deliberately free: `crates/wcore-agent/tests/spill_timing_probe.rs::timing_probe`, `#[ignore]`d so it costs the suite nothing, with SPILL_PROBE_BYTES and SPILL_PROBE_CEILING as levers and a PHASE line reporting fixture / engine.run() / read-back separately. A guard written against it must assert a RATIO or a per-byte slope, never a wall-clock constant: hetzner-dsm is a shared 96-core box whose load moved 40 -> 250 during the original measurement, and a wall-clock bound is exactly the fragile-timing shape wayland-core#337 was filed about."
---

Split out of wayland-core#378 c1. #378's own criteria are about a slow TEST and
are closed by re-sizing its fixture; this ticket carries the product finding the
attribution surfaced, which no criterion on #378 repairs.

## What was measured, and what it is not

`AgentEngine::run()` costs roughly a fixed ~4-5 s plus a term ~linear in the
size of the tool result carried through the turn — about 0.1 ms per KB, i.e.
roughly 100 s per megabyte, in the unoptimized test profile.

Two candidates were excluded by CONTROL rather than by reading the code:

* **The fixture** is not the cost. Constructing the payload is 0.001-0.002 s and
  reading the spilled file back through the session's own jail is 0.000-0.001 s
  — together under 0.005% of the total.
* **The spill/shed path** is not the cost. With the context ceiling raised so
  the #636 shed never fires, the SAME 480,000-char payload still costs
  48.029 s. Shed-on and shed-off arms, run INTERLEAVED in one session, are
  indistinguishable: shed-on 52.779 s then a >60 s kill; shed-off a >60 s kill
  then 48.029 s.

## A retracted reading, recorded so it is not re-derived

The FIRST shed-on/shed-off pair was unpaired and read as "the shed is slower".
That is **false** and is retracted, not softened. The interleaved repeat is what
refuted it. Any future arm on this ticket must interleave, because the host load
moves far enough during a single series to invent an effect of that size.

## Why the answer is not obvious in either direction

A tool returning a megabyte is ordinary (`Read` on a large file, a verbose
`Bash`), so if the term survives `--release` a user pays it on a routine path,
and pays it whether or not the shed fires. But ~100 ns/byte in a debug build is
also exactly what an ordinary optimized-away linear scan looks like. c1 exists
because both stories fit the evidence collected so far, and neither has been
tested.

handoff: FerroxLabs/wayland-core#378
