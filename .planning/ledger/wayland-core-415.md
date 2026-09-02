---
issue: 415
repo: FerroxLabs/wayland-core
kind: defect
title: "Close the quarantine console bypass with a restricting-SID token, without removing the feature (from #389 c1)"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "Measured on real Windows with the reproduction control alive in the SAME run: a restricting-SID quarantine child cannot reach the operator-s console (WROTE_TO_USER_CONSOLE=false) while git --version, git status and cmd /c echo ok all exit 0 inside it. Both halves, or it is not a fix."
    state: not-met
    owner: core
    note: "The only candidate left standing after 2026-09-01-s measurements. Low IL closes the hole and costs the child every child process it might spawn (0xC0000142 on cmd /c echo ok, with a Low-labelled TMP and the full parent environment both ruled out as causes). Window stations and job UI limits are refuted with their own application proven, so they are real nulls and not vacuous ones."
  - id: c2
    text: "Whatever spawn path it needs preserves core#393-s process-tree containment, proven by core#393-s own tests rather than by inspection."
    state: not-met
    owner: core
    note: "CreateProcessAsUser replaces the spawn path core#393 just landed containment on. A fix that closes a console bypass by quietly dropping process-tree reaping is not a fix, and inspection is how that ships unnoticed."
  - id: c3
    text: "If it also proves unworkable, that is recorded with its measurement and core#389-s labelling branch becomes the permanent answer rather than the interim one."
    state: not-met
    owner: core
    note: "A residual that is neither closed nor explicitly declared closed-as-impossible decays into nothing. This criterion is what stops that."
---

Split from core#389 c1 on 2026-09-01. core#389 took its own documented c2 branch --
label the quarantine-originated prompt -- which is shipped and live-verified on
real Windows. This carries the remaining possibility so the residual is tracked
rather than lost.

0.13.13, not 0.13.12: the operator is told the truth today by the labelling, and
this is more engineering than the Low-IL option that was rejected on cost.
