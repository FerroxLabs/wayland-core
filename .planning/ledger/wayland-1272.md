---
issue: 1272
repo: FerroxLabs/wayland
kind: defect
title: "0.13.12 release board: all 32 blocking issues mapped to an owner (17 in flight, 15 unassigned)"
status: open
last_verified_commit: 2fe7a70df
criteria:
  - id: c1
    text: "Every issue in `check-release-readiness.py`'s blocking list appears on this board with a named owner, and the board is updated whenever the list changes. An unowned issue is a bug in this ticket."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: "Tranche 3a is assigned to lanes and each of its 6 issues reaches CLOSED or DECOMPOSED — no issue filed by our own verification is left unowned because it was found late."
    state: not-met
    owner: core
    note: "Graded 2026-09-10. Five of six reached CLOSED/COMPLETED on the tracker: wayland#1231 2026-08-31T13:36:13Z, wayland#1252 2026-08-31T13:36:30Z (and decomposed, wayland#1276 is its c3 split and is OPEN at milestone 0.13.14), wayland#1254 2026-09-04T16:58:18Z, wayland-core#400 2026-08-31T13:37:45Z, wayland-core#393 2026-08-31T13:37:37Z. wayland#1256 is OPEN and is neither closed nor decomposed: its ledger is status open with c1 met, c2 met, c3 not-met, owner core and NO handoff, and its own note says the corpus instance is closed totally while the class is not, and that this criterion is why the issue stays open. The residual is exact -- a lane still cannot be prevented from reporting a tree green while a crate it chose not to run is red -- and two candidate closures are named there with neither decided. Closing #1256 administratively would make this criterion read met over a live gap, so it stays not-met and names wayland#1256 c3 as the whole of what is owed."
  - id: c3
    text: "Tranche 3b's 9 residuals are each either closed, decomposed with a `handoff:`, or explicitly recorded as a decision that is not core's to take."
    state: met
    evidence: "file:.planning/RECON-1269-1272-1324-2026-09-10.md:222:Three are OPEN and each carries the record the criterion demands"
    owner: core
    note: "MET 2026-09-10, all nine dispositioned, per-row detail in .planning/RECON-1269-1272-1324-2026-09-10.md. Six are CLOSED/COMPLETED on the tracker: wayland-core#382 2026-08-31T13:37:30Z, wayland-core#389 2026-08-31T17:04:18Z, wayland-core#369 2026-08-31T13:37:16Z, wayland#908 2026-08-31T13:36:45Z, wayland#1203 2026-08-31T07:24:02Z, wayland#1150 2026-08-31T15:33:11Z. Three are OPEN and each carries the required record in the ledger FIELD rather than in prose. wayland-core#368: c1-c5 all state blocked, owner maintainer, handoff FerroxLabs/wayland-core#410, c6 met -- that is the decision-not-core's-to-take arm, and #410 is OPEN at milestone 0.13.12. wayland#559: c4 blocked, owner desktop, handoff FerroxLabs/wayland#1193 (OPEN), other six met. wayland#388: c4 blocked, owner flux, handoff FerroxLabs/wayland#1184 (OPEN); c7 met with handoff FerroxLabs/wayland#1237 which is CLOSED/COMPLETED so that leg is done; other ten met. Every handoff target was checked to exist and to be open where it still carries work, and none of the three is blocked on core by core -- the failure mode this criterion exists to catch. LIMIT: this grades DISPOSITION, which is what the criterion asks. It does not re-verify the six closures against the tree, and it does not judge whether the three carriers will be worked."
  - id: c4
    text: "`wayland#1203` c3 and any other criterion requiring a **live run** is either measured on real hardware or restated as what the tree can actually grade — a criterion that cannot be run is as worthless as one that cannot fail."
    state: not-met
    owner: core
    note: "Half discharged, half not, graded 2026-09-10. The wayland#1203 c3 half IS discharged: that criterion is state superseded with successor and handoff FerroxLabs/wayland#1244, which is OPEN at milestone 0.13.14; two of its three legs were measured LIVE on hetzner with the shipped debug binary against a loopback fixture (a fresh launch and a --resume wrote three records to ~/.wayland/budget/spend-audit.jsonl all keyed 20244249fc21), and the third is restated on #1244 as a PTY-driven run because rebind_provider's only production callers are the TUI bridge and the --no-tui REPL registers no model handler. NOT MET on the second clause. Sweeping every ledger record for a criterion whose text names a live run, a PTY, real hardware or a soak and whose state is neither met nor superseded returns eleven, one of which is this criterion itself; the other ten are wayland#1228 c1, wayland#1244 c1 and c3, wayland#1309 c1, wayland#305 c4, wayland-core#386 c1, c2 and c3, wayland-core#410 c1, wayland-core#411 c1. None is orphaned -- each sits on a ticket with an owner -- but none has been graded on the question this criterion actually asks, which is not whether it is owned but whether it can be run at all. Owed: for each of the ten, one line saying runnable on a named host, or not runnable and restated as what the tree can grade. That restatement belongs on the ISSUE first, not in a ledger note, per this ledger's own rule below."
---

Created 2026-08-31 to close a COVERAGE gap. c2, c3 and c4 were graded
2026-09-10 by the w14/recon reconciliation lane against `2fe7a70df`; the
per-issue tracker states, handoff-target checks and the live-run sweep are in
`.planning/RECON-1269-1272-1324-2026-09-10.md`. `c1` was out of that lane's
scope and its entry is untouched.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
