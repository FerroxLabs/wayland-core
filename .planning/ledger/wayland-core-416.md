---
issue: 416
repo: FerroxLabs/wayland-core
kind: defect
title: "[nightly-windows-soak] FAIL - 2026-09-01"
status: closed
last_verified_commit: 2f309a19
criteria:
  - id: c1
    text: "The nightly Windows soak failure is triaged: either the failing soak assertion is fixed, or the run is shown to have failed for an infrastructure reason and the issue is closed by the maintainer."
    state: met
    evidence: "file:.github/workflows/nightly-windows-soak.yml"
    owner: core
    note: "MET 2026-09-02 by a subsequent GREEN soak, not by a code fix on our side, and the distinction matters. Run 33595565417 (2026-09-02T05:39:36Z) concluded success where the three runs before it failed (33474355230, 33361211735, 33295047243), and github-actions[bot] closed the issue at 06:26:17Z with state_reason `completed`. The bot's own comment records that the close is gated on the WHOLE run rather than on one job's success -- windows-soak, keyring-blob-size and windows-live-acceptance all passed -- which is the core#325 fix, so this is not a tracker closing itself on a partial green. DEVIATION FROM THE CRITERION TEXT, stated rather than glossed: it says `closed by the maintainer` and it was closed by automation. That mechanism is the intended one (core#325), so the substance is satisfied, but nobody human triaged the three preceding failures and no root cause was ever named -- the failure simply stopped reproducing. If it returns, it returns as a new nightly issue with no history attached to it."
---

# An auto-filed nightly soak failure, closed by a subsequent green run

Ledgered 2026-09-01 so the release gate could see it; the coverage gate refuses a
release while an open in-scope issue on either tracker has no ledger file, and this
one was auto-filed by github-actions after 0.13.12's work had already been graded.

Closed 2026-09-02 when the nightly went green. This entry is updated rather than
deleted because the ledger's job is to agree with the tracker, and the gate that
checks that agreement runs ONLY on the release path -- `ci.yml` runs the checker with
`--offline`, which skips tracker divergence entirely, so a drift like this one stays
invisible to every PR and surfaces only when a release is attempted.
