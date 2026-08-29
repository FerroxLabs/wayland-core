---
issue: 364
repo: FerroxLabs/wayland-core
kind: feature
title: "[Maintainer] Two 0.13.12 dispositions no lane can perform: close core#113 as refuted, and decide the future of the WhatsApp cap"
status: open
last_verified_commit: be4467ed
criteria:
  - id: c1
    text: "wayland-core#113 is closed as refuted, or Q-113 is reversed and a lane is named to build the opposite"
    state: blocked
    owner: maintainer
    note: "Blocked because only the maintainer closes issues in this repo -- that is the entire remainder, and there is no code owed. core#113 c1-c4 refute or supersede three of the four reported claims with a test each, and the fourth (deny-by-default) is the intended posture whose refusal now carries a config snippet round-tripped through the real serde types to an actual Allow decision. The decision was taken as Q-113 in .planning/DECISIONS.md and the record was posted on #113 on 2026-08-29, so nothing is waiting on core. Carries wayland-core#113 c6"
  - id: c2
    text: "One of the three WhatsApp Cloud outcomes is chosen: free slots on the existing Meta account, a separate developer account, or accept the cap stays permanently unmeasured"
    state: blocked
    owner: maintainer
    note: "Blocked on an account constraint no credential can lift: Meta's 15-app-per-developer cap against an account already holding 44 apps. The lane cannot free slots because the apps may belong to other work. Outcome 3 is not 'do nothing' -- it obliges core to reword docs/delivery-semantics.md from 'a credential we have not obtained' to 'not obtainable', the way the two QR-paired WhatsApp bridge backends are already worded, and then wayland#1186 c5 and wayland#934 c5 are re-graded against that. Carries wayland#1186 c5"
---

Filed by the core lane on 2026-08-29 during the 0.13.12 handoff audit, and
classified `kind: feature` deliberately.

This ticket describes no defect in wayland-core. It is a queue of two acts that
only the maintainer can perform, filed so that two real remainders stop living
in ledger notes where nobody outside the lane can find them. The DEFECTS they
serve keep their own classification and keep blocking: `wayland-core#113` and
`wayland#934` / `wayland#1186` are all `kind: defect`, and each names this
ticket as the carrier of the half it cannot do itself.

Classifying this one `defect` would have been the wrong kind of caution. Both
criteria are `blocked` and owned by the maintainer, so a `defect` classification
would demand a `handoff:` from each of them — and the only ticket that carries
them is this one. A ledger entry handing its own criteria to itself is a green
gate tracking nothing, which is precisely the failure the handoff rule exists to
prevent. The honest alternative is to say plainly what this is: not a bug, a
decision request.

The two items are unrelated in subject and are together only because they share
an owner. Either can be done without the other; the ticket closes when both are.
