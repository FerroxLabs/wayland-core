---
issue: 324
repo: FerroxLabs/wayland-core
title: "AppContainer ACL race: a deny identity strips a concurrent allow identity's access (4 of last 8 nightlies, never tracked)"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "An instrumented run establishes whether the failure is a product race in AppContainer ACE application or a race in the test fixture"
    state: not-met
    owner: core
    note: "nobody has ever run this test alone. Step one is N of at least 20 alone and N of at least 20 with the two execute() calls serialized, on ferrox-win-msvc through the runner service - a session-0 SSH logon reports is_available() false, so it cannot be observed over SSH at all"
  - id: c2
    text: "concurrent_allow_and_deny_identities_do_not_interfere passes at retries=0 over N of at least 20 on the AppContainer-capable host"
    state: not-met
    owner: core
    note: "confirmed failing at live_fs_acl.rs:471 in five nightlies - 2026-08-18, 08-19, 08-20, 08-25 and 08-27. The issue title says four of eight; the 08-27 run 33053333326 is a sixth data point it does not carry"
  - id: c3
    text: "Whichever arm the measurement indicts, the deny half of the test is still non-vacuous afterwards"
    state: not-met
    owner: core
    note: "the assertions at live_fs_acl.rs:475-478 must not become vacuous while making the allow arm green - that would convert a real Windows multi-agent defect into a passing test"
---

Two Windows sandbox identities race the same secret.txt: one arm carries a
read-deny for it, the other does not, and the allow arm intermittently loses its
access. The assertion that fires is that an ordinary allow identity must retain
access.

The important thing about this issue is what it is not yet. Nobody has
established whether the race lives in AppContainerBackend's ACE application - a
deny ACE written by one identity stripping another's allow ACE rather than being
additive per-SID, which would be a real multi-agent Windows defect - or in the
fixture's own hold window. So the next step is a measurement, not a fix, and
that is what c1 says.

This is the only issue in its cluster that cannot be touched from Linux at all.
It also went untracked for three weeks for a structural reason: the nightly soak
closes its tracker from a job that cannot see this job's result, which is #325.
Until #325 lands, any tracker row for this vanishes on the next green soak.

Criteria come from the cluster C verification note of 2026-08-29.
