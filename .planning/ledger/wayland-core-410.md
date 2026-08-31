---
issue: 410
repo: FerroxLabs/wayland-core
kind: task
title: "[Maintainer] AppContainer categorical-deny disposition: Q-368-honesty forbids the fix while a required soak job tests for it (core#368 c1-c5)"
status: open
last_verified_commit: 33e3edde1
criteria:
  - id: c1
    text: "The contradiction is resolved by a maintainer decision that is RECORDED in .planning/DECISIONS.md: either the standing no-Windows-sandbox decision stands and the required soak job stops testing for a fix that is forbidden, or the decision is reversed and core#368 c1-c5 are owned"
    state: not-met
    evidence: "file:.planning/DECISIONS.md"
    owner: maintainer
    note: "Filed 2026-08-31 by the core lane (win-appcontainer-scope) during the 0.13.12 release gate, and ledgered the same day when the coverage gate caught it had none. THIS IS NOT A REQUEST TO RE-ARGUE THE SANDBOX STRATEGY -- that decision is recorded and this ticket assumes it stands. What needs a maintainer is a CONSEQUENCE of it that no lane can resolve, because every available resolution is either forbidden by the decision or is itself a maintainer act. THE LOOP, three facts each verified against the tree at ca15a48bf and jointly unsatisfiable: (1) THE FIX IS FORBIDDEN -- DECISIONS.md Q-368-honesty reads DECLARE IT. Do not build the ACL fix, on the grounds that Windows ships with NO filesystem sandbox, the Job-object default is intended, and AppContainer is never to be chased again, months were lost to it; it obliges core#368 c1-c5 to stay OPEN and NOT-MET, owned by whoever reverses the standing decision. (2) THE DEFECT IS STILL MEASURED BY A REQUIRED JOB -- crates/wcore-sandbox/tests/live_fs_acl.rs:441 concurrent_allow_and_deny_identities_do_not_interfere runs in the windows-live-acceptance job of nightly-windows-soak.yml, one of three REQUIRED_JOBS in that workflow's aggregate check, and fails about one run in five with `ordinary allow identity must retain access` at live_fs_acl.rs:471. (3) A GREEN SOAK IS A RELEASE CRITERION -- wayland-core#350 c5 is superseded with successor core#368 and its own note says no green soak has been obtained, and one cannot be until #368 lands, so the blocker list is now exactly one: #368. The only thing that turns #350 c5 green is the fix Q-368-honesty forbids. A gate that cannot PASS is worth exactly as much as one that cannot fail, and this is one. NOT A CORE DECISION: reversing a recorded strategy decision, or removing a job from REQUIRED_JOBS, are both maintainer acts."
---

# A required job tests for a fix the recorded decision forbids

`core#368` asks for an AppContainer ACL fix. `DECISIONS.md` Q-368-honesty forbids building
it. `nightly-windows-soak.yml` runs a REQUIRED job whose test fails ~1 run in 5 precisely
because the fix is absent. And `core#350` c5 cannot go green until that soak does.

Every exit from that loop is a maintainer act: reverse the strategy decision, or take the
test out of the required set and say so. The core lane can do neither, so it stops here
rather than picking one quietly. Full reasoning and line-level citations are on the issue.
