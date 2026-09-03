---
issue: 1300
repo: FerroxLabs/wayland
kind: defect
title: "Windows-only: crashed-holder recovery in the chunked credential write lock is bistable (48x), and it is what times out interrupted_rotations_do_not_leak_entries_without_bound"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "On Windows at --retries 0, n>=20, the per-crash-round recovery cost of credentials::chunk_crash_injection::* has a max/min spread below 3x. Today it is 48.2x for interrupted_rotations and 25x across the sibling sweeps inside a single run."
    state: not-met
    owner: core
    note: "Filed 2026-09-03 while classifying three retry-flakes that were blocking an eight-PR merge train. THE TICKET IS NOT WHAT ITS TEST NAME SAYS. The visible failure is <flakyFailure type='test timeout' time='180.114'> with EMPTY text -- the harness killed it at exactly the global 90s x 2, so the unbounded-leak assertion NEVER EXECUTED. Where it does execute the invariant HOLDS: Linux 0/15 at --retries 0 (in-test 3.74-6.48s) with the census bounded and periodic ([13, 4] repeated over 20 rounds, no growth), macOS 0/10 (4.96-6.86s). The real defect is a Windows-only bistability: passing times 3.09/3.64/27.44s against 131.44-149.03s on trees of identical test count. NOT MEASURED ON WINDOWS, which is the platform that decides -- SeanDesktop was running two live Runner.Worker jobs for this merge train and has no warm checkout, so measuring there meant a cold build stealing cores from CI that had to go green. That gap is why this criterion is stated as a Windows measurement."
  - id: c2
    text: "The five sibling sweep tests no longer split into a ~1.5s and a ~36-97s mode within a single run."
    state: not-met
    owner: core
    note: "This is the arm that rules OUT the two easy explanations, which is why it is a criterion and not a note. Within one run, one binary, at one moment, run 33574606424 gave 36.00 / 50.35 / 1.66 / 39.78 / 1.52. Process-spawn cost cannot vary 25x between two tests in the same binary, so it is not spawn cost; and the same sweep flips sides across runs (sw_same_size: 36.0, 1.63, 1.45, 37.1, 35.9, 38.6, 35.4, 46.0, 1.64, 1.54), so it is not deterministic-per-test. Linux shows none of it (n=8, sweeps 0.93-3.01s, uniform)."
  - id: c3
    text: "The cause is NAMED BY MEASUREMENT rather than inferred: is_stale is instrumented to record the observed mtime age and the metadata/modified error kind on each poll, and the result shows which of (a) a future-dated mtime, (b) a metadata error, or (c) something else accounts for the 1.7-4.0s stalls."
    state: not-met
    owner: core
    note: "CANDIDATE MECHANISM, NAMED BUT EXPLICITLY NOT PROVEN. The acquisition loop's only wait is POLL = 50ms, and is_stale() ends in two fail-closed arms -- `.map(|r| r.unwrap_or(false))` for a future-dated mtime and `.unwrap_or(false)` for ANY metadata error -- either of which converts straight into 50ms polls up to the 10s wait_ceiling. A 1.7-4.0s stall is what an mtime reading future-dated by 1.7-4.0s would produce. This criterion exists so nobody grades the ticket met on that story: it has to be measured, not argued."
  - id: c4
    text: "is_stale's two unwrap_or(false) arms stop being silent: an unreadable or future-dated lock mtime is observable rather than merely slow."
    state: not-met
    owner: core
    note: "Independent of c3's outcome. A fail-closed default that swallows the reason is why this took bimodal timings to find at all -- the product had the information and discarded it. Note that warn! alone does not satisfy this: with RUST_LOG unset only ERROR reaches stderr, which this repo has already recorded as a defeat for three separate features."
  - id: c5
    text: "The negative controls stay green after any change: Linux 0/15 and macOS 0/10 at --retries 0, and the census stays [13, 4]-periodic."
    state: not-met
    owner: core
    note: "Pinned because the obvious fixes here (raising a timeout, widening the ceiling) would make the symptom vanish on Windows while quietly weakening the leak invariant on the two platforms where it currently runs clean. The controls are what stop that."
  - id: c6
    text: "Recorded as UNPROVEN and explicitly out of scope: whether the same clock behaviour can make a live 2s-heartbeating holder look stale to a 6s stale_after waiter."
    state: not-met
    owner: core
    note: "Production uses stale_after 60s with 2s heartbeats and STALE_AFTER_SECS >= HEARTBEAT_SECS * 3, so seconds-scale mtime unreliability does not obviously threaten it. But the DIRECTION of any skew is unmeasured, and if mtime can read OLDER than reality by more than 4s a live holder could be stolen -- which the code says must never happen. Carried as a stated unknown rather than dropped, because a deferral with no trigger decays into nothing."
---

# The test name says leak. The payload says timeout.

`interrupted_rotations_do_not_leak_entries_without_bound` never reached its leak
assertion: the harness killed it at 180.114s and the `<flakyFailure>` text is empty.
On the two platforms where the assertion does execute it passes, and the census is
flat -- `[13, 4]` repeated over twenty rounds with no growth.

What is underneath is a Windows-only bistability in recovering a lock from a crashed
holder: ~65-77ms per crash-round on Linux and on fast Windows runs, against 1.7-4.0s
on slow ones, with no middle. Reading the payload rather than the test name is what
separated a scary-sounding credential leak from a real but different reliability bug.
