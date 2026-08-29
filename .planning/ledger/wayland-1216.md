---
issue: 1216
repo: FerroxLabs/wayland
kind: defect
title: "The report job's evidence floor cannot notice that the leg running the whole workspace suite contributed nothing"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The evidence floor is per-leg: a leg that uploads zero junit files fails the required report check rather than being covered by another leg's upload"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D34, found while verifying wayland#1177). Nothing has been done. The measured finding, verbatim: The report job's evidence floor cannot notice that the Linux containerized leg contributed nothing. `assert-test-evidence.sh` is invoked with `EXPECTED_MIN: 1` across ALL legs aggregated into `junit-reports/`, so if any single leg uploads a junit.xml the gate is satisfied even when the leg that runs the full workspace suite uploaded zero files (`if-no-files-found: ignore` at ci.yml:2027 makes that silent). Separately, the preserved `outer-attempt-*.xml` files now also land inside `junit-reports/` and are counted by the same `find ... -name '*.xml'` COUNT, so a leg's preserved failures inflate the number that is supposed to prove coverage."
  - id: c2
    text: "Preserved outer-attempt-*.xml files are not counted toward the coverage figure they are meant to prove"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D34). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A test under .github/scripts/tests/ drives both directions and is wired into lint.yml so a failure reds the step"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D34). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The report job's evidence floor cannot notice that the Linux containerized leg contributed nothing. `assert-test-evidence.sh` is invoked with `EXPECTED_MIN: 1` across ALL legs aggregated into `junit-reports/`, so if any single leg uploads a junit.xml the gate is satisfied even when the leg that runs the full workspace suite uploaded zero files (`if-no-files-found: ignore` at ci.yml:2027 makes that silent). Separately, the preserved `outer-attempt-*.xml` files now also land inside `junit-reports/` and are counted by the same `find ... -name '*.xml'` COUNT, so a leg's preserved failures inflate the number that is supposed to prove coverage.

**Where.** .github/workflows/ci.yml:2624-2635 (EXPECTED_MIN: 1) and :2015-2027 (upload with if-no-files-found: ignore); counting logic in .github/scripts/assert-test-evidence.sh:55-67.

**Why it matters.** It is the same defect class the report gate was built for (wayland#1115: a green `report` on a suite that never ran), one level finer — per-leg rather than repo-wide. The ci.yml comment at :2620-2623 records the decision to keep EXPECTED_MIN at 1 deliberately, so this is a known trade rather than an oversight, but defect 1 above is a live instance of the leg it cannot see. Today the `ci` job is red on its own account so no run is green on this, but the gate contributes nothing to catching it.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
