---
issue: 372
repo: FerroxLabs/wayland-core
kind: feature
title: "GEPA online evolution is mutation without selection: close the loop before defaulting it on"
status: open
last_verified_commit: 1798076f
criteria:
  - id: c1
    text: "The online path scores the CANDIDATE, not the session: incumbent and child are scored on the same observations and the child is only eligible if it beats the incumbent, with the fitness function stated and justified"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c2
    text: "The online path goes through PromptStore::record_variant + CuratorPort::submit -> Decision::Promote|Archive, never a bare file; the $WAYLAND_HOME/evolved/*.md write is deleted or documented as inert"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c3
    text: "Promotion requires accumulated evidence, not n=1; the threshold is stated and justified"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c4
    text: "The mutator does not use the session's frontier model, and the per-session cost is measured and recorded"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c5
    text: "Selection is tested BOTH ways with the fixture-replay provider: child better -> Promote, child worse -> Archive, child equal -> Archive"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c6
    text: "A two-session integration test proves the loop CLOSES: session 1 promotes a variant, session 2's SkillRouter demonstrably uses it"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
---

Criteria are the ticket's own acceptance wording, transcribed so the release gate can count this work. Nothing has been done by the bookkeeping pass that created this file, and nothing here has been graded against the tree.

## Classification ruling, 2026-08-30 (re-grade lane; classification ONLY, no work done)

`kind:` changed from `defect` to `feature`. `scripts/check-release-readiness.py` had
flagged the disagreement itself -- *"says `kind: defect` while FerroxLabs/wayland-core#372
is labelled enhancement. Blocking anyway -- over-blocking is the safe direction -- but the
classification is worth a second look."* This is that second look, and four independent
signals agree, which is why the safe-direction default is being departed from rather than
left to stand:

1. **The tracker label is `enhancement`**, not `bug`. This matters mechanically as well as
   editorially: the readiness gate's live check fails an entry marked `kind: feature` whose
   tracker labels it `bug`, and that check does not fire here, so the corroboration the gate
   asks for is present rather than absent.
2. **The ticket says so in its own words**: *"0.13.12 is a defect release; this is a feature
   with a fitness function at its centre."*
3. **It is deferred to 0.13.13 by a recorded decision** dated 2026-08-29, and the ticket was
   filed *"so the deferral is tracked rather than remembered"* -- i.e. to be visible, not to
   block the current cut.
4. **No shipped default behaviour is wrong.** `observability.online_evolution` defaults
   **false**, so no user of 0.13.12 reaches the unselected-mutation path at all. The
   README's "self-evolving" claim is backed by a different, live loop
   (`observability.skills_lifecycle`, default **true**), and the ticket explicitly declines
   to dispute that claim: *"This ticket does not dispute the claim."* There is no user-facing
   falsehood and no user-reachable defect -- the gap is a feature that is not finished.

What is NOT being claimed: none of c1..c6 has been graded against the tree, none has been
worked, and every one stays `not-met`. The reclassification changes only which release owes
the work -- 0.13.13, per the deferral -- and it does not make the criteria any less real.
If `online_evolution` is ever moved to default-on, this becomes a defect again on that day,
because claim 4 above is the load-bearing one and it is a property of a default, not of the
code.
