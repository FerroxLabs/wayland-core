---
issue: 1290
repo: FerroxLabs/wayland
kind: defect
title: "f14_sigkill_recovery: ZERO provider-dispatch checkpoints persist (left 0, right 1) - the exactly-once DOUBLING premise this was filed on is refuted"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "On a failing run, establish which of the two remaining possibilities holds: the provider-dispatch checkpoint was NEVER WRITTEN (a product defect), or it was written and not fsynced before the SIGKILL (a test defect). Measured at --retries 0, n>=20, on a host that has actually exhibited it."
    state: not-met
    evidence: "file:crates/wcore-cli/tests/f14_sigkill_recovery.rs"
    owner: core
    note: "CRITERION REPLACED 2026-09-03, because the question it used to ask cannot be answered. It read 'inspect whether the journal genuinely contains TWO provider-dispatch checkpoints', and the payload measured in PR #417 run 33553656600 (outer-attempt-1.xml, f14_sigkill_recovery.rs:806:5) is LEFT 0 RIGHT 1. Zero, not two. There is nothing to count. The second possibility must not be assumed because it is cheaper: an exactly-once recovery that persists NOTHING is a data-loss shape, and it fails the same assertion from the opposite side."
  - id: c2
    text: "The relationship to gh#1289 is settled by measurement rather than by co-occurrence: either the :806 left-0 failures are shown to co-occur with :593 credential-store failures at n>=20, which folds this into gh#1289, or they are shown to occur independently, which keeps it separate."
    state: not-met
    owner: core
    note: "CIRCUMSTANTIAL AND EXPLICITLY NOT PROVEN. The same attempt file carries two flakyFailure elements for f14_seed_recoverable_turn_helper at :593 with SessionAuthority('the configured credential store did not answer within 5s'). Three flakes, one attempt, one file, and a 'nothing was written' assertion sitting beside two 'the credential store did not answer' failures is consistent with ONE cause - which is why the retry-allowlist entry for this test cites gh#1289 and not this ticket. Co-occurrence in a single attempt is not causation, and this criterion exists so nobody grades it as such."
  - id: c3
    text: "The ticket is NOT closed as corrected. Closing it discards a real unexplained failure."
    state: not-met
    owner: core
    note: "PLAN-0.13.13-v2 listed this as 'close as corrected'. That is wrong, and it is the second-order version of the error the ticket itself made: the premise was refuted, not the defect. The correction is to the title and to c1; the observation - an exactly-once assertion failing under a simulated crash, deliberately NOT allowlisted so the gate keeps reporting it - stands exactly as filed."
---

# The premise died. The failure did not.

`left: 0` cannot be two checkpoints, so the doubling this ticket was filed to
chase is refuted. What remains is a different and equally real question: an
exactly-once recovery path that persisted NOTHING under SIGKILL.

Deliberately still not allowlisted. An exactly-once assertion failing under a
simulated crash is the signal, not the noise, and the gate reporting it is the
gate working.
