---
issue: 364
repo: FerroxLabs/wayland-core
kind: task
title: "[Maintainer] Two 0.13.12 dispositions no lane can perform: close core#113 as refuted, and decide the future of the WhatsApp cap"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The core#113 disposition is chosen and stated on the ticket"
    state: blocked
    owner: maintainer
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass. Blocked on a maintainer ruling: this ticket IS the handoff target for wayland#1186 c5 / wayland#934 c5 and core#113 c5, and no lane can perform either disposition."
  - id: c2
    text: "The WhatsApp cap disposition is chosen and stated on the ticket; if the disclosure route is chosen, the core lane lands it and re-grades wayland#1186 c5 and wayland#934 c5 against it"
    state: blocked
    owner: maintainer
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass. Blocked on a maintainer ruling: this ticket IS the handoff target for wayland#1186 c5 / wayland#934 c5 and core#113 c5, and no lane can perform either disposition."
---

Criteria are the ticket's own acceptance wording, transcribed so the release gate can count this work. Nothing has been done by the bookkeeping pass that created this file, and nothing here has been graded against the tree.
