---
issue: 414
repo: FerroxLabs/wayland-core
kind: defect
title: "gate-admission.py fails 5 of its own assertions on the shipping branch, and has for some time"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "Each of the five failing assertions is either FIXED, or declared with the reason it is permanently inapplicable -- decided one at a time, not waved through as a block."
    state: not-met
    owner: core
    note: "MEASURED by A/B on two pinned worktrees, same script, same command: base be28da9c7 fails 5, HEAD 852f5acaa fails the IDENTICAL 5, and `comm -13` of the sorted lists is empty. Checked at BOTH granularities -- assertion level AND offender level (5 offenders on each side, none added) -- because an assertion-level A/B can hide a new offender inside an already-failing assertion. So this predates the core#412 work and is not a regression from it."
  - id: c2
    text: "The reconciliation with core#412 c2 is written down where both gates can be read together: which admission forms satisfy both rules, and which satisfy only one."
    state: not-met
    owner: core
    note: "Two gates now hold rules about the same fact. gate-admission.py asserts that in an unconditional job every step up to its last gate is unconditional; core#412 c2 requires independent checks to survive an earlier failure. They ARE reconcilable -- a bare `!cancelled()` satisfies both, while `!cancelled() && steps.X.outcome == 0x27success0x27` satisfies only c2 -- but nothing records that, so the next person to touch either finds out by breaking the other. That is the same two-instruments-one-fact drift that left a closed-carrier rule fixed in one gate and stale in its twin."
  - id: c3
    text: "gate-admission.py exits 0 on the shipping branch, PROVEN by a run in the fmt + clippy job, and driven RED by re-introducing one of the five so the green is not the green of a disabled check."
    state: not-met
    owner: core
    note: "A gate that cannot pass is worth as little as one that cannot fail: with five standing failures nobody can tell a NEW admission defect from the constant, and the job going red for a permanent reason trains everyone to read that red as expected."
---

Filed 2026-08-31 while discharging core#412 c2/c3. The `fmt + clippy (workspace,
all targets)` job is red on `integ/f13` for this reason alone, independently of
what the code does.

0.13.13 rather than 0.13.12: it is a gate that is stuck red, not a gate that is
falsely green, so it degrades signal without certifying anything untrue. core#412
was the falsely-silent case and that one blocked.
