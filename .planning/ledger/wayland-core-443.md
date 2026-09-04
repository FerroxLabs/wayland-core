---
issue: 443
repo: FerroxLabs/wayland-core
kind: defect
title: "[nightly-windows-soak] FAIL - 2026-09-04"
status: open
last_verified_commit: 57e2a244e
criteria:
  - id: c1
    text: "The nightly Windows soak failure is triaged: either the failing job is fixed, or the run is shown to have failed for an infrastructure reason and the issue is closed."
    state: not-met
    evidence: ""
    owner: core
    note: "Auto-filed by github-actions at 2026-09-04T06:08:04Z from run 33841172783, DURING the 0.13.13 integration swarm. Ledgered immediately for the reason core#416 records: the coverage arm refuses a release while an open in-scope issue on either tracker has no ledger file, so a bot filing overnight reds the release gate on coverage alone. UNTRIAGED -- this entry buys coverage, it does not claim a diagnosis. What the run actually reports, quoted from the issue body rather than inferred: windows-soak=success, keyring-blob-size=success, windows-live-acceptance=FAILURE. So the failing leg is the self-hosted live-acceptance job, NOT the soak proper, and its log must be read directly (the artifact route in the auto-filed triage steps covers windows-soak only). NOT graded on the sibling greens: two of three passing is exactly the partial-green that core#325 was filed to stop being read as a pass. This issue carries NO milestone, so it is outside the 0.13.13 readiness scope and does not join the 42; it is in ledger-coverage scope only. Per core#325 it closes automatically on the next nightly tick where EVERY gating job is green, so a subsequent green closes it without a code change -- and core#416 records why that is worth stating out loud: the failure stops reproducing, nobody names a root cause, and it returns later as a new issue with no history attached."
---

# An auto-filed nightly soak failure, ledgered for coverage, untriaged

Filed by automation mid-swarm on 2026-09-04. Ledgered the same day so the release
gate coverage arm can see it; this file makes no claim about the defect.

The failing leg is `windows-live-acceptance` on the self-hosted Windows runner. A
Windows live-acceptance red is a real product signal and should be read before the
0.13.13 cut, not waved through on the two sibling greens.

Structural note worth carrying: an automated nightly can red the release gate at any
hour purely by filing, because coverage scopes EVERY open issue on either tracker.
`ci.yml` runs the checker `--offline`, which skips tracker coverage and divergence
entirely, so this class is invisible on every PR and surfaces only at release.
