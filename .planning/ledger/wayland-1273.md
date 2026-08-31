---
issue: 1273
repo: FerroxLabs/wayland
kind: defect
title: "CI runs the ledger coverage gate with --offline, so a tracker can go invisible — and 29 issues did, for two days"
status: closed
last_verified_commit: 07ee39f6
criteria:
  - id: c1
    text: "The coverage arm runs somewhere it can FAIL — the release path, a schedule, or a required job — not only in `just ledger-check-live`, which nothing invokes automatically. Where it runs and what it fails is stated."
    state: met
    evidence: "file:.github/workflows/release.yml"
    owner: core
    note: "MET BY CODE THAT PREDATES THE TICKET, and the ticket is corrected rather than credited: it claims the coverage arm runs only in `just ledger-check-live`. False. release.yml runs `python3 scripts/check-criteria-ledger.py` with no --offline, under `GH_TOKEN: secrets.LEDGER_ISSUES_TOKEN || secrets.GITHUB_TOKEN`, on the release path -- which is one of the three placements c1 names. Verified by reading the workflow, not by trusting the ticket that I filed myself."
  - id: c2
    text: "If coverage genuinely cannot run on some legs (it needs `gh` auth against two trackers), that reason is recorded AT the `--offline` call site in `ci.yml`, not only in the justfile. The next reader of that line learns what it is buying."
    state: met
    evidence: "file:.github/workflows/ci.yml"
    owner: core
    note: "The --offline call site carried NO reason, directly beneath a comment reading `A gate nothing runs is a gate that cannot fail`. It now states why (the coverage arm queries BOTH trackers and the repo-scoped GITHUB_TOKEN cannot see the second), where the live arm does run (release.yml), and what the choice costs -- with the measurement: 29 open in-scope issues carried no ledger for two days and no green run on this leg could have seen them."
  - id: c3
    text: "A red arm: an open in-scope issue with no ledger file makes the chosen enforcement point FAIL, shown failing before the ledger is added and passing after. A coverage check that has never been observed failing is not known to work."
    state: met
    evidence: "file:scripts/check-criteria-ledger.py"
    owner: core
    note: "RED ARM RUN 2026-08-31 on the real instrument, both directions. Removed `.planning/ledger/wayland-core-390.md` for an issue that is OPEN and in scope. LIVE arm: `COVERAGE: FerroxLabs/wayland-core#390 is OPEN and in scope with no ledger file`, FAIL 40. CONTROL, the same tree with --offline: the string COVERAGE appears ZERO times -- it is blind to the gap, which is the property under test. Its 4 complaints were anchor drifts from this change's own ci.yml edit, unrelated to the removal, and are fixed here. Restored, `git status --porcelain` = 0, live coverage back to 0."
  - id: c4
    text: "The gate's own `THIS IS NOT A PASS for coverage` warning reaches whoever reads a green run. A disclosure printed into a log nobody opens is not a disclosure — this is the same finding the product side of #400 is about."
    state: met
    evidence: "file:.github/workflows/ci.yml"
    owner: core
    note: "The gate prints `THIS IS NOT A PASS for coverage` to stdout inside a step that exits 0, so it reaches nobody. It is now written to $GITHUB_STEP_SUMMARY, which GitHub renders on the run page. This is wayland-core#400's defect -- a disclosure that exists and is not projected where the reader looks -- one layer up, in our own tooling, and it is named as such at the call site so the parallel is not lost."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

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
