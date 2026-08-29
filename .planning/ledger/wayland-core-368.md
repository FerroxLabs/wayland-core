---
issue: 368
repo: FerroxLabs/wayland-core
kind: defect
title: "AppContainer deny is categorical: a deny identity strips a concurrent identity's grant, and a grant cannot reach an already-protected object"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "concurrent_allow_and_deny_identities_do_not_interfere passes at retries=0 over N >= 20 on an AppContainer-capable host"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c2
    text: "The deny half stays non-vacuous: a change that makes the allow arm pass by making the deny arm stop denying fails this issue"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
---

Criteria are the ticket's own acceptance wording, transcribed so the release gate can count this work. Nothing has been done by the bookkeeping pass that created this file, and nothing here has been graded against the tree.
