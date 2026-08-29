---
issue: 368
repo: FerroxLabs/wayland-core
kind: defect
title: "AppContainer deny is categorical: a deny identity strips a concurrent identity's grant"
status: open
last_verified_commit: b52fb934
criteria:
  - id: c1
    text: "apply_protected_deny deletes only the calling identity's own package-SID ALLOW aces, never every S-1-15-2 ALLOW on the object"
    state: not-met
    owner: core
    note: "RE-CONFIRMED AGAINST THE TREE, not taken from the #324 note: acl_lease.rs:1332 still gates deletion on `is_app_package_sid(ace_sid)` alone, which is true for ANY AppContainer package SID, and nothing in the function compares an ace against the identity being applied. The caller already has that identity: `apply_intents(intents, sid)` at :661 holds it as `sid` and passes it to `explicit_access_for_sid` on the Allow arm, so plumbing it into the Deny arm and switching the predicate to an EqualSid comparison is a small change. This criterion is the cheap half."
  - id: c2
    text: "Before protecting, the package ALLOWs other identities hold on that object by inheritance are materialised as explicit aces"
    state: not-met
    owner: core
    note: "This is the criterion that actually fixes the measured failure, and c1 alone does NOT. The #324 c1 instrumented run found the allow identity reaching the secret ONLY by inheritance from the granted directory - it held no explicit ace on the secret at all - so deleting fewer explicit aces changes nothing for it. What removes its access is the `PROTECTED_DACL_SECURITY_INFORMATION` write at acl_lease.rs:1343, which severs inheritance for everybody. Materialising means clearing INHERITED_ACE on the other identities' package ALLOWs so they survive the protect as explicit entries. The comment at :1337-1339 states the protect is 'always' applied precisely so an inheritable package ALLOW cannot re-apply - that intent is correct for the CALLING identity and wrong for every other one, and this criterion is that distinction."
  - id: c3
    text: "apply_explicit_access reaches PROTECTED descendants of a granted root, so a grant applied after a protect still lands"
    state: not-met
    owner: core
    note: "The converse ordering, and the harder half. `apply_explicit_access` (acl_lease.rs:1258) writes ONE object with `SUB_CONTAINERS_AND_OBJECTS_INHERIT`, so a grant on a directory reaches descendants purely by inheritance - which a PROTECTED descendant does not accept. Both orderings occur: the mutation lock serialises the two identities' ACL phases but does not order them, and the #324 c1 run shows the two executions overlapping wholly (allow window 0..148 ms, deny window 0..136 ms). A fix that closes only the deny-second ordering will pass some runs and fail others, which is indistinguishable from the current flake."
  - id: c4
    text: "concurrent_allow_and_deny_identities_do_not_interfere passes at retries 0 over N of at least 20 on an AppContainer-capable host"
    state: not-met
    owner: core
    note: "The acceptance, carried verbatim from core#324 c2. THE HOST IS NOT A BLOCKER: #324 c1 reproduced this locally on SeanDesktop over OpenSSH in roughly one run in five at seconds per iteration, and refuted the older claim that it needed ferrox-win-msvc. N>=20 at one-in-five is a few minutes of loop. What it is blocked on is c1-c3 being written, not on access."
  - id: c5
    text: "The deny half stays non-vacuous: the deny child still exits non-zero with the secret absent from its stdout"
    state: not-met
    owner: core
    note: "Its own criterion rather than a clause on c4, so it cannot be satisfied by inattention. A change that makes the allow arm pass by making the deny arm stop denying meets the letter of c4 and fails the issue - and that is the EASY wrong fix here, since the simplest way to stop a deny stripping a concurrent grant is to stop the deny doing anything. Baseline to preserve, from the #324 c1 instrumented run: `deny exit_code : 1` with `deny stdout : \"\"`. Denial is presently REAL and is the containment property the sandbox sells."
---

Split out of core#324, whose c1 measurement is done and recorded. This carries
the fix half. The verdict is that this is a PRODUCT race in ACE application, not
a fixture race: apply_protected_deny deletes EVERY AppContainer-package ALLOW ace
on the denied object -- its own and any concurrent identity's alike -- then sets
PROTECTED_DACL_SECURITY_INFORMATION so no inheritable package ALLOW can re-apply.
Denial by absence of a grant is per-OBJECT, and the object is shared, so it is
categorical across identities by construction.

This ledger entry exists because the remainder was decomposed onto an issue with
no ledger file, which means nothing could grade it and the next sweep would have
had to re-derive it from issue prose.

NOT TAKEN BY THE win-native lane, deliberately and not for lack of a host. This
is unsafe Win32 ACL code on the sandbox's containment boundary, where an
over-broad grant is a silent security regression that no test in this repo would
catch, and c2 and c3 are a design change to how AppContainer denial is applied
rather than a tightening of the existing code. It wants its own review.

It is the SOLE remaining blocker on core#350 c5 (a green nightly-windows-soak),
now that core#374 is closed.
