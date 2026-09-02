---
issue: 410
repo: FerroxLabs/wayland-core
kind: task
title: "[Maintainer] AppContainer categorical-deny disposition: Q-368-honesty forbids the fix while a required soak job tests for it (core#368 c1-c5)"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "One of the four recorded outcomes is chosen and stated on this ticket: retire the assertion, reverse Q-368-honesty for this one defect, delete the AppContainer backend, or accept a permanently amber soak"
    state: blocked
    owner: maintainer
    note: "Blocked because every one of the four ways out is a maintainer act and none is a lane's. Retiring the assertion means weakening a containment test, which .planning/DECISIONS.md Q-368-honesty explicitly forbids a lane doing; reversing that decision is the maintainer's; deleting the backend removes a security boundary; and accepting an amber soak rewrites another issue's close condition. THE LOOP THIS TERMINATES, all three verified at ca15a48bf: Q-368-honesty says the AppContainer ACL fix is NOT to be built and obliges core#368 c1-c5 to stay open; concurrent_allow_and_deny_identities_do_not_interfere at crates/wcore-sandbox/tests/live_fs_acl.rs:441 nevertheless still runs in the windows-live-acceptance job of .github/workflows/nightly-windows-soak.yml, one of that workflow's three REQUIRED_JOBS, failing about one run in five; and .planning/ledger/wayland-core-350.md c5 is superseded onto core#368 and states no green soak can be obtained until it lands. So the only thing that discharges core#350 c5 is the fix a recorded decision forbids. Carries wayland-core#368 c1-c5"
  - id: c2
    text: "The choice is carried through: DECISIONS.md records it, and wayland-core#368 c1-c5 and wayland-core#350 c5 are re-graded against it"
    state: blocked
    owner: maintainer
    note: "Blocked on c1 and on nothing else -- this is core work the moment a direction exists, and it is a separate criterion so that choosing an outcome and landing it cannot be confused for one another. Each outcome obliges something different: retiring the assertion means core edits the soak job and closes core#368 c1-c5 as will-not-fix with the defect disclosed, which core#368 c6 already grades; reversing Q-368-honesty means the c1-c5 rows return to owner core and a lane is named for unsafe Win32 ACL work on the containment boundary; deleting the backend takes its twelve live-acceptance tests with it; accepting an amber soak means core#350 c5 is rewritten to exclude PHASE L and stops claiming a green soak is achievable. Carries wayland-core#350 c5"
---

Filed by the core lane (`win-appcontainer-scope`) on 2026-08-31 during the
0.13.12 release gate, and this ledger entry exists so the ticket can be graded
rather than re-derived from issue prose — the same gap that produced
`wayland-core#368`'s own entry.

`kind: task` by the schema's own definition, and for the same reason as
`wayland-core#364`: every remaining criterion is an act a human must perform,
with no code owed until the act happens. The DEFECT it serves keeps blocking on
its own row — `wayland-core#368` is `kind: defect`, its c1-c5 are `blocked` and
name this ticket in their `handoff:`.

WHY THE REPOINT WAS NEEDED AT ALL, stated here because the arithmetic is the
whole argument. Left as `not-met` / `owner: core`, `#368` c1-c5 were counted
OUTSTANDING against 0.13.12 by `scripts/check-release-readiness.py` — its
`kind: defect` with a criterion `state: not-met, owner: core` rule — for work a
recorded decision instructs core not to do. That is a criterion that can
neither pass nor be dropped, and the release board (`wayland#1272`) carried it
as `1 met / 5 not-met` against the 0.13.12 milestone.

TWO CLAIMS THE FILING LANE WAS SENT TO TEST AND WHICH BOTH CAME BACK FALSE.
They are recorded on the ticket body in full and summarised here so no future
sweep re-derives them wrongly:

  * `#368` c1-c5 are NOT blocked on `wayland-core#254` and are not subsumed by
    it. `#254` was closed UNMERGED on 2026-08-17, its seven-file diff never
    touched `acl_lease.rs`, and it could not have — that file was added
    2026-08-05 by `9150ff1fc`, thirteen days after `#254` opened.
  * `#368` c4 is NOT permanently red for want of an AppContainer-capable host.
    One exists and answered on 2026-08-31: SeanDesktop, Windows
    10.0.26200.9168, `sandbox status --json` reporting
    `{available:true, backend:appcontainer}`.

Neither finding changes what is owed. Both change WHO it is owed by, which is
the only thing this ticket is asking to settle.
