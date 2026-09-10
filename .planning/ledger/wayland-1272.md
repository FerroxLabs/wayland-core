---
issue: 1272
repo: FerroxLabs/wayland
kind: defect
title: "0.13.12 release board: all 32 blocking issues mapped to an owner (17 in flight, 15 unassigned)"
status: open
last_verified_commit: fdf4b1e1c
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
    note: "NOT MET, re-verified 2026-09-10 by the w15/rec lane against fdf4b1e1c; section 2.1 of .planning/RECORD-1269-1272-1324-2026-09-10-w15.md. Five of six reached CLOSED/COMPLETED on the tracker: wayland#1231 2026-08-31T13:36:13Z, wayland#1252 2026-08-31T13:36:30Z (and decomposed -- wayland#1276 is its c3 split, read OPEN at milestone 0.13.14 on 2026-09-10), wayland#1254 2026-09-04T16:58:18Z, wayland-core#400 2026-08-31T13:37:45Z, wayland-core#393 2026-08-31T13:37:37Z. wayland#1256 was read from the tracker again on 2026-09-10 and is still OPEN with no stateReason, milestone 0.13.14, label area:core -- neither closed nor decomposed. Its ledger at fdf4b1e1c is status open with c1 met, c2 met, c3 NOT-MET, owner core and NO handoff, and c3s own note says the corpus instance is closed totally while the class is not, and that this criterion is why the issue stays open. The residual is exact: a lane still cannot be prevented from reporting a tree green while a crate it chose not to run is red. Two candidate closures are named there and neither is decided. Nothing moved between 2fe7a70df and fdf4b1e1c. Administrative closure of #1256 would make this criterion read met over a live gap, so it stays not-met and names wayland#1256 c3 as the whole of what is owed. LIMIT: this grades tracker STATE and ledger state, not whether #1256 c3 is close to being closed."
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
    note: "Half discharged, half not; re-graded 2026-09-10 by the w15/rec lane against fdf4b1e1c, section 2.2 of .planning/RECORD-1269-1272-1324-2026-09-10-w15.md. The wayland#1203 c3 half IS discharged, unchanged: that criterion is state superseded with successor and handoff FerroxLabs/wayland#1244 (read OPEN, milestone 0.13.14, on 2026-09-10); two of its three legs were measured LIVE on hetzner and the third is restated on #1244 as a PTY-driven run. STILL NOT MET on the second clause, but the second clause is now graded rather than merely enumerated. Two defects in the enumeration were fixed first. (a) The sweep is not reproducible and quantifies over a GROWING set: re-running it at fdf4b1e1c returns a different set from the prior lanes at 2fe7a70df -- three prior rows (wayland#1228 c1, wayland#305 c4, wayland-core#386 c2) match no live-run keyword at all and were included by judgement, and three rows are new since (wayland#1349 c2, wayland-core#415 c1, plus one false positive). A criterion quantifying over any other criterion in a ledger set that grows can never be permanently met as written. (b) A case-insensitive `pty` matches inside the word EMPTY, which is why wayland#1328 c4 (`absent versus explicit-empty lists`) appeared; it is not a live-run criterion and is excluded. Any future sweep must anchor \\bpty\\b and \\bsoak\\b. The union of both sweeps minus that false positive is TWELVE rows, each now graded on the question the criterion actually asks -- not whether it is owned but whether it can be run at all. RUNNABLE ON hetzner-dsm (Linux, needs a build slot): wayland#1244 c1 and c3 (Linux has a pty; c3 needs a second mutated build), wayland#1309 c1 (and its first half is gradeable by READING the test -- it is a test-structure criterion wearing pty clothes), wayland#1349 c2 (both prior soaks ran there on a deterministic loopback fixture with 0 paid calls, ~2h per arm). RUNNABLE ON GITHUB ACTIONS ONLY: wayland-core#386 c1 (the red sibling is the hard part, which is why c3 exists), wayland-core#386 c2 (gated on c1; note the act is the WORKFLOWs, no human opens that issue), wayland-core#411 c1 (one lane/** push plus gh run view -- the cheapest row in the set, and not runnable by THIS lane which may not push). RUNNABLE ON SeanDesktop, the only Windows box: wayland-core#415 c1, and wayland#305 c4 (Windows plus WSL plus the Desktop app, but owner desktop, not cores to run). NEEDS NO HOST: wayland-core#386 c3 is dischargeable as a RECORD today -- it is that criterions own restatement escape hatch, and it is not this issues to write. NOT RUNNABLE BY ANY LANE: wayland#1228 c1 is a credential acquisition not a run (issue carries needs:overwatch), correctly blocked/maintainer. NOT A LIVE-RUN CRITERION AT ALL: wayland-core#410 c1 matched only on the word soak inside `accept a permanently amber soak` and is a maintainer DECISION; it should be dropped from future sweeps. SUMMARY: nothing in this set is un-runnable for want of hardware; the two genuinely stuck rows are stuck on a human decision (core#410) and a credential (#1228), and both already record that. STILL NOT MET because grading runnability is not measuring, and for the nine runnable rows this criterion asks for the measurement. The restatement owed -- that this criterion must be re-scoped to a named commit or converted into a standing gate, because as written it is true at a commit and false at the next filing -- belongs on wayland#1272 ITSELF and is deliberately NOT written as a narrowing here, per this ledgers own rule below. LIMIT: no measurement was taken by this lane; cargo is banned on this host, all three Linux slots were held, and this lane may not push."
---

Created 2026-08-31 to close a COVERAGE gap. c2, c3 and c4 were graded
2026-09-10 by the w14/recon reconciliation lane against `2fe7a70df`; the
per-issue tracker states, handoff-target checks and the live-run sweep are in
`.planning/RECON-1269-1272-1324-2026-09-10.md`. c2 and c4 were then re-graded
the same day by the `w15/rec` lane against `fdf4b1e1c` — tracker re-reads, the
twelve-row live-run RUNNABILITY grading, and the two defects found in the sweep
itself are in `.planning/RECORD-1269-1272-1324-2026-09-10-w15.md` section 2.
`c3` was not re-graded and its entry is carried forward from `2fe7a70df`
unchanged; its `met` evidence anchor was confirmed to still resolve.

`c1` has been out of scope for BOTH lanes and has still never been graded
against a tree. The file-level `last_verified_commit` was re-anchored to
`fdf4b1e1c` for c2 and c4; it does not mean c1 was verified there. c1 is the
oldest untouched thing on this ticket and should be somebody's next act.

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
