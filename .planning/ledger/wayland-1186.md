---
issue: 1186
repo: FerroxLabs/wayland
kind: task
title: "[Credentials request] Five platform credentials needed to measure adapter message caps (#934 c5)"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "telegram: a credential exists and a boundary probe sends at max_message_len() and at +1 against the real platform"
    state: met
    evidence: "file:docs/delivery-semantics.md:834:was then read by a separate process talking directly to matrix.org."
    owner: maintainer
    note: "MEASURED 2026-08-29: 4096 accepted, 4097 refused with 400 Bad Request message is too long. telegram.cap_measured is now live. RESIDUAL: the probe was driven in ASCII, so whether the platform counts characters or UTF-16 code units is STILL OPEN and must not be read as settled."
  - id: c2
    text: "sms (Twilio): a credential exists and a boundary probe sends at max_message_len() and at +1 against the real platform"
    state: met
    evidence: "file:docs/delivery-semantics.md:838:| process life 1 (pid 3132637), response withheld, then `kill -9` | `c"
    owner: maintainer
    note: "MEASURED 2026-08-29: 1600 accepted, 1601 refused with 400 code 21617. Twilio names the unit itself, so unlike telegram the unit is unambiguous. sms.cap_measured is now live."
  - id: c3
    text: "matrix: the declared 16384 cap is verified against the real platform limit"
    state: superseded
    owner: core
    note: "RESCOPED 2026-08-29 and handed back to wayland#934 c7, which is open and carries it. This is NOT a credentials problem: the cap derives from a byte budget (the assembled PDU), so BOTH probe arms land inside the accepted region and the harness enum Above has no variant meaning accepted-normally. A token would not close it; a probe-shape change would, and that is core's."
  - id: c4
    text: "msteams: the declared 20480 cap is verified against the real platform limit"
    state: superseded
    owner: core
    note: "RESCOPED 2026-08-29 and handed back to wayland#934 c7, which is open and carries it. Same refutation as matrix: the 20,480 figure is derived from an 80 KB UTF-16 budget on the serialized Activity, so both arms land inside the accepted region. The Bot Framework credential is still the highest-setup item on the original list but it is no longer the binding constraint."
  - id: c5
    text: "whatsapp (Meta Cloud API): a credential exists and a boundary probe sends at cap and at +1"
    state: blocked
    owner: maintainer
    note: "BLOCKED on Meta's 15-app-per-developer cap; the account already carries 44 apps. This is not a credential anyone can hand us without deleting apps, so it is a maintainer account decision rather than a shopping-list item."
  - id: c6
    text: "Any credential that cannot be obtained is recorded with its reason, so cap_measured = no stays an honest disclosure"
    state: met
    evidence: "file:docs/delivery-semantics.md:412:narrows the reach of the product's only exactly-once guarantee, and th"
    owner: core
    note: "docs/delivery-semantics.md records for matrix and msteams that neither correction is a measurement and why the real limit is on something the client cannot compute. The WhatsApp bridge disclosure sits at :417-426. The Meta app-cap reason still needs recording on the ticket itself."
---

Split out of `wayland#934` so its `c5` blocker is a shopping list rather than a
sentence. Every item was an account or a token the core lane cannot obtain for
itself.

RESCOPED 2026-08-29 and the shape of the request changed materially. Two of the
five are DONE by live measurement: Telegram at 4,096/4,097 and Twilio SMS at
1,600/1,601, both now `cap_measured = live`. Two more turned out not to be
credential problems at all — Matrix and MS Teams are NOT MEASURABLE by the
two-point boundary probe, because each cap is derived from a byte budget and both
probe arms therefore land inside the accepted region, which the harness has no
vocabulary for. That is a code change, and it is core's. The fifth, WhatsApp
Cloud API, is blocked on Meta's 15-app-per-developer limit against an account
holding 44 apps, which no credential can lift.

So the list is no longer five credentials. It is one account decision, two
core-owned probe changes, and two measurements already banked.
