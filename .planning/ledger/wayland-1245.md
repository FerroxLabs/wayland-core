---
issue: 1245
repo: FerroxLabs/wayland
kind: defect
title: "Flaky: t19_live_negative_leg has a 45s drain window with zero headroom; it fails 3/3 above loadavg ~150"
status: open
last_verified_commit: 782efce3a
criteria:
  - id: c1
    text: "The zero-headroom construction is removed: the negative leg concludes absence on a signal that is not a fixed wall-clock window, or the window carries headroom measured against the observed completion, which lands at 45.5 s against a 45.0 s deadline even when it passes."
    state: met
    evidence: "absent:crates/wcore-cli/tests/migrate_quarantine.rs::Duration::from_secs(45)"
    owner: core
    note: "MET at 14aa2f1ca. The fixed 45 s wall clock is GONE from `drive_skill_turn`, and the `absent:` anchor re-reads the file on every gate run so a future edit that reintroduces it reds here. CONTROLLED rather than argued: the same token `Duration::from_secs(45)` DOES occur in the pre-fix body at 335328967, so an empty result now is the construction being removed and not the query being broken. WHAT REPLACED IT -- the criterion's FIRST branch, a signal that is not a fixed wall clock: absence is concluded on THE TURN'S OWN COMPLETION, a `tool_result` from the Skill tool followed by the `stream_end` that closes the assistant stream after it. Both are frames the product emits; neither is a clock the harness reads. MEASURED on hetzner-dsm, same command, comparable ambient load: t19 took 45.324 s at 1-min loadavg 29.50 before and 2.913 s at loadavg 25.99 after; under 100 busy-loop spinners it is 45.547-45.829 s before (n=9, loadavg 167-190) and 7.413-8.899 s after (n=17, loadavg 180-199). A SECOND DEFECT CLOSED IN THE SAME EDIT, stated because it was not asked for: the old loop concluded `sentinel absent, therefore contained` even when the child had stalled after answering, so a dead turn satisfied this leg VACUOUSLY. The 300 s clock that remains is a BACKSTOP for that case only -- reaching it now panics with the whole stream attached. LIMIT: this closes the CONSTRUCTION. It does not reproduce the failure the ticket was filed on, which is c2 and stays not-met."
  - id: c2
    text: "Shown RED against today's code at a load where it fails today -- loadavg above roughly 150 on hetzner-dsm, where it failed all three --profile ci attempts -- and green after, at the same load."
    state: not-met
    owner: core
    note: "STILL NOT MET, and now not-met on EVIDENCE rather than on nobody having tried. THE RED ARM WAS BUILT AND IT DID NOT REPRODUCE: 20 pre-fix observations at 335328967, ZERO failures, across five instruments and 1-min loadavg 29.50 to 207.08 -- `nextest -E t19+t20` alone (1 at 29.50; 4 at 167.04-168.20), plain `cargo test -p wcore-cli --test migrate_quarantine` with all 49 tests in ONE shared process (8 at 179.10-207.08, 49/49 each), the whole `-p wcore-cli --profile ci --retries 0` crate suite (1 at 74-102), and `nextest run --workspace --profile ci --retries 0 --no-fail-fast` over 18118 tests (1 unloaded at 45-63; 5 with 100 spinners at peak 174.99-189.55). Per-trial table in .planning/evidence/load-conditioned-flakes/RATES.md. THE LOAD WAS REAL AND IT REACHED THE TEST, controlled rather than asserted: the same full-suite run takes 87 s unloaded and 186-223 s loaded, and t20 -- the positive leg driving the SAME binary through the SAME turn -- goes from 2.5 s to 6.4-6.9 s. The child is genuinely starved, just not the 9x it would take. WHAT THIS SAYS ABOUT THE TICKET, and it is a correction: the headroom argument in the body does not establish the failure mode. 45.5 s against a 45.0 s deadline is not a near-miss, it is the negative leg's FLOOR -- the old loop always ran the window out and could not finish faster -- so it is evidence of waste, not of proximity to failure. The real margin is the driven turn's duration against 45 s, and that measured 5-7 s at loadavg 190. WHAT IS OWED: a reproduction of the body's 3/3 on the instrument that produced it. This lane's contention is manufactured runqueue pressure plus a real full-workspace suite; it carries almost no memory or page-cache pressure, which is the most likely difference from the containerised CI runner the original observation came from."
  - id: c3
    text: "The rate is recorded WITH the load figure beside it. A rate with no load number cannot separate this defect from ambient noise, and the whole finding is load-conditioned."
    state: met
    evidence: "file:.planning/evidence/load-conditioned-flakes/RATES.md"
    owner: core
    note: "MET at 14aa2f1ca. Every rate this lane measured is recorded in .planning/evidence/load-conditioned-flakes/RATES.md WITH the 1-min loadavg beside it, per instrument and per commit -- which is exactly what this criterion exists to force and what stops a rate being confused with ambient noise. THE RECORDED RATE IS ZERO, stated rather than smoothed: 0 failures in 20 pre-fix observations spanning loadavg 29.50-207.08, and 0 in 26 post-fix observations spanning loadavg 25.99-199.09. A ZERO RATE IS A MEASUREMENT AND IT IS NOT A CLOSE OF c2 -- it CONTRADICTS the issue body's `3/3 above loadavg ~150` and `scripts/check-test-env-globals.py`'s `at load ~130 it is 1 pass / 6 fail under cargo test`, so what it establishes is that this lane's contention is not the condition those two figures came from. HONESTY ABOUT THE DENOMINATORS: five further remote-proof invocations returned `complete: false` (`Proof slot busy; no build ran`, and once `checkout is not clean`); those runs never executed and are excluded from every denominator rather than counted as passes."
  - id: c4
    text: "The positive leg stays fast: the early return on the sentinel that keeps it fast is not removed to fix the negative one."
    state: met
    evidence: "file:crates/wcore-cli/tests/migrate_quarantine.rs:1210:if sentinel.exists() {"
    owner: core
    note: "MET at 14aa2f1ca. The sentinel early-return that keeps the positive leg fast is RETAINED -- `if sentinel.exists() {` still occurs exactly once in the file, at the head of the drain loop, and it now also sets the `concluded` latch so the positive leg leaves by the same door. MEASURED rather than asserted, both loads, both arms: t20 was 2.537 s at loadavg 29.50 and 6.374-6.678 s at loadavg 167-168 BEFORE; it is 2.550 s at loadavg 25.99 and 6.420-6.814 s at loadavg 180-182 AFTER -- unchanged inside noise. The whole 49-test `migrate_quarantine` binary under plain `cargo test` went from 52.50-56.05 s (n=8, loadavg 179-207) to 15.30-18.04 s (n=8, loadavg 181-191), so the negative leg got faster and nothing else got slower."
---

Created 2026-08-31. This issue was filed 2026-08-29/30 by this cycle's own
verification, was in scope for the release gate from that moment, and had no
ledger file -- so scripts/check-release-readiness.py, which reads ledger files
and nothing else, could not count it. CI runs the coverage arm with --offline,
which is the arm that would have said so.

Its body declared no acceptance criteria, so it could not have been closed as
filed either. The criteria above are AUTHORED from measurements the body
already records.

Both green runs land at 45.5 s against a 45.0 s deadline, so the assertions are
satisfied in the drain loop AFTER the window closes. The test is already over
its own budget on the runs that pass, which is why the pass is not evidence.

2026-09-10, w15/loadD. The fix landed in 14aa2f1ca and the measurements are in
.planning/evidence/load-conditioned-flakes/RATES.md.

ONE STALE ARTEFACT IS LEFT DELIBERATELY, so it is not lost. The
`SHARED_PROCESS_SKIPS` entry for this test in `scripts/check-test-env-globals.py`
justifies itself with "it drives the real CLI binary through a 45 s bounded agent
turn and asserts the Skill tool answered inside that window" -- a window that no
longer exists after 14aa2f1ca. This lane did NOT edit it: removing the skip puts
the test back on a blocking gate leg, and rewording it would mean inventing a
justification this lane does not have evidence for. What this lane does have is
the measurement that would support removing it: 8/8 green under plain
`cargo test -p wcore-cli --test migrate_quarantine` at loadavg 180.71-191.14
post-fix, and 8/8 green pre-fix at loadavg 179.10-207.08. That is a decision for
whoever owns that gate file.
