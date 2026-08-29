---
issue: 1198
repo: FerroxLabs/wayland
kind: defect
title: "check-criteria-ledger.py cannot detect drift in a file:<path>:<line> anchor, which is the drift it exists to catch"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "A file: anchor carries a required content fragment that must be present at or near the named line, or bare line anchors are refused on files above a stated size"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D15, found while verifying wayland#1134). Nothing has been done. The measured finding, verbatim: `scripts/check-criteria-ledger.py` accepts `file:<path>:<line>` evidence on nothing but a line-count check - 'file exists AND has at least <line> lines'. Any number below the file length passes forever, so a positional anchor silently rots the moment anyone edits the file, which for a 2600-line ci.yml is every lane. Both #1134 anchors are already wrong (1806 lands on an unrelated 'voice suite' step, 1888 lands in the wrong one of the two legs it distinguishes), and one of them was already wrong at the commit the ledger records as last_verified."
  - id: c2
    text: "Both #1134 anchors are re-anchored to something that resolves to what the criterion claims"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D15). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "--self-test proves both directions: an anchor whose content moved goes red, and a correct anchor stays green"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D15). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

`scripts/check-criteria-ledger.py` accepts `file:<path>:<line>` evidence on nothing but a line-count check - 'file exists AND has at least <line> lines'. Any number below the file length passes forever, so a positional anchor silently rots the moment anyone edits the file, which for a 2600-line ci.yml is every lane. Both #1134 anchors are already wrong (1806 lands on an unrelated 'voice suite' step, 1888 lands in the wrong one of the two legs it distinguishes), and one of them was already wrong at the commit the ledger records as last_verified.

**Where.** scripts/check-criteria-ledger.py:48 and the FILE_EV branch at ~line 287

**Why it matters.** This is the offline arm of the gate that exists to stop a `met` claim from drifting away from its evidence, and for `file:` anchors it cannot detect drift at all - it is a gate that cannot fail on the failure mode it was built for. The script's own docstring already says to prefer `test:`/`symbol:`; the cheap fix is to make `file:<path>:<line>` carry a required content fragment (`file:<path>:<line>:<substring>`) and re-anchor, or to refuse bare line anchors on files over some size.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
