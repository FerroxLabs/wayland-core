---
issue: 415
repo: FerroxLabs/wayland-core
kind: defect
title: "Close the quarantine console bypass with a restricting-SID token, without removing the feature (from #389 c1)"
status: open
last_verified_commit: 509f4426b
criteria:
  - id: c1
    text: "Measured on real Windows with the reproduction control alive in the SAME run: a restricting-SID quarantine child cannot reach the operator-s console (WROTE_TO_USER_CONSOLE=false) while git --version, git status and cmd /c echo ok all exit 0 inside it. Both halves, or it is not a fix."
    state: not-met
    owner: core
    note: "REFUTED BY MEASUREMENT 2026-09-03, and it cannot be met -- see .planning/QUARANTINE-CONSOLE-RESTRICTED-TOKEN.md. Nine restricted-token configurations plus two controls on real Windows 10.0.26200.9168 in ONE run: the two halves are strictly anti-correlated, every arm that closes the console cannot spawn git and every arm that spawns git leaves the console open. The premise fails, not just the nine arms: with the console closed the child can already open cmd.exe, git.exe, ntdll.dll and its own image for READ|EXECUTE and CreateProcess still returns ERROR_ACCESS_DENIED, so filesystem pass-2 was never the binding constraint and no ACLing of the quarantine tree can rescue it. Control alive in every run (control-detached reproduces WROTE_TO_USER_CONSOLE=true) plus a detector self-test before any arm. Stays not-met and should be recorded as not-achievable rather than pending; c3 is the criterion that carries the disposition. ORIGINAL NOTE: The only candidate left standing after 2026-09-01-s measurements. Low IL closes the hole and costs the child every child process it might spawn (0xC0000142 on cmd /c echo ok, with a Low-labelled TMP and the full parent environment both ruled out as causes). Window stations and job UI limits are refuted with their own application proven, so they are real nulls and not vacuous ones."
  - id: c2
    text: "Whatever spawn path it needs preserves core#393-s process-tree containment, proven by core#393-s own tests rather than by inspection."
    state: not-met
    owner: core
    note: "MOOT, and recorded so rather than left pending: the candidate that would have needed CreateProcessAsUser is refuted (see c1), so no spawn path changes. For the record, measured alongside: CreateProcessAsUserW composes with job objects (create suspended, AssignProcessToJobObject, resume), so containment was never the obstacle -- the token was. ORIGINAL NOTE: CreateProcessAsUser replaces the spawn path core#393 just landed containment on. A fix that closes a console bypass by quietly dropping process-tree reaping is not a fix, and inspection is how that ships unnoticed."
  - id: c3
    text: "If it also proves unworkable, that is recorded with its measurement and core#389-s labelling branch becomes the permanent answer rather than the interim one."
    state: met
    evidence: "file:.planning/QUARANTINE-CONSOLE-RESTRICTED-TOKEN.md:12:#415 c3 applies"
    owner: core
    note: "MET at 509f4426b, and this is the flip the previous note asked a post-merge sync to make: it said the row stayed not-met ONLY because it was anchored at a commit that did not yet carry the measurement file. dfc9f2273 put that file on main. .planning/QUARANTINE-CONSOLE-RESTRICTED-TOKEN.md now records, in the tree, the refutation with its control alive in the same run (DETECTOR_SELFTEST PASS, and [control-detached] reproducing the 2026-09-01 baseline WROTE_TO_USER_CONSOLE=true byte for byte), the full twelve-arm table on real Windows 10.0.26200.9168, the mechanism that kills the premise rather than only the nine arms, and the one cell left unmeasured stated as such (SetTokenInformation(TokenDefaultDacl) returned 1344 in all three forms attempted, so Chromium-style default-DACL plumbing was not exercised and that is a harness limit, not a null result). It states the consequence this criterion demands: #389 labelling branch becomes the permanent answer rather than the interim one. c1 remains not-met and should be read as NOT ACHIEVABLE rather than pending -- its two halves are strictly anti-correlated across every configuration measured -- and c2 is MOOT, since the candidate that would have needed CreateProcessAsUser is refuted so no spawn path changes."
---

Split from core#389 c1 on 2026-09-01. core#389 took its own documented c2 branch --
label the quarantine-originated prompt -- which is shipped and live-verified on
real Windows. This carries the remaining possibility so the residual is tracked
rather than lost.

0.13.13, not 0.13.12: the operator is told the truth today by the labelling, and
this is more engineering than the Low-IL option that was rejected on cost.
