---
issue: 324
repo: FerroxLabs/wayland-core
kind: defect
title: "AppContainer ACL race: a deny identity strips a concurrent allow identity's access (4 of last 8 nightlies, never tracked)"
status: closed
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "An instrumented run establishes whether the failure is a product race in AppContainer ACE application or a race in the test fixture"
    state: met
    evidence: "symbol:crates/wcore-sandbox/src/backends/appcontainer/acl_lease.rs::apply_protected_deny"
    owner: core
    note: "MEASURED 2026-08-29 on SEANDESKTOP (Windows 11 build 26200) over OpenSSH, tree at ab6b602f. VERDICT: it is a PRODUCT race in ACE application. Not the fixture. FIRST, THE PREMISE OF THIS FILE WAS WRONG: the previous note said the measurement needs ferrox-win-msvc because `session-0 SSH reports is_available() false`, i.e. Sean-only infra. The SSH session is fine. AppContainer was unavailable for an unrelated reason that the probe states plainly once you ask it - a single abandoned lease, `%LOCALAPPDATA%\\Wayland\\Core\\AppContainerLeases\\v1\\WCore-00007f0c-01dd2dfbe3fc00f2-0000000000000000.toml`, dated 2026-08-17, 620139 bytes, 4367 intents, whose first intent grants `\\\\?\\C:\\Users\\seand` (the whole home directory). Every `ExecutionIdentity::start` sweeps it and its recovery can never complete: `sandbox child execution failed: AppContainer ACE cleanup verification failed for \\\\?\\C:\\Users\\seand\\.bun\\install\\cache\\@GH@WhiskeySockets-libsignal-node-bcea72d@@@1\\.npmrc (still failing after 3 attempts, so this is not a transient race)`. One poisoned lease had disabled the Windows sandbox on this account since 2026-08-17. Quarantining that one file (moved, not deleted, to D:\\wf13w\\.scratch\\quarantined-lease\\) flipped the probe to `AVAILABLE=true` immediately. Filed separately - see the DECOMPOSITION note on c2. SECOND, THE RACE REPRODUCES LOCALLY: `cargo nextest run -p wcore-sandbox --test live_fs_acl --run-ignored all -E \"test(concurrent_allow_and_deny_identities_do_not_interfere)\" --retries 0`, five consecutive runs -> FAIL,PASS,PASS,PASS,PASS, the failure at the `ordinary allow identity must retain access` assertion, i.e. the same assertion the nightlies report. THIRD, THE INSTRUMENT. A temporary copy of the test recorded, on the failing interleaving, the two children's exit codes and streams, the overlap of the two execution windows, and `icacls` on the secret afterwards. RECORD, verbatim: `allow retained access : false` / `allow window (ms) : 0..148` / `deny  window (ms) : 0..136` / `allow exit_code : 1` / `allow stdout : \"\"` / `allow stderr : \"Access is denied.\"` / `deny  exit_code : 1` / `deny  stdout : \"\"`, and the secret's DACL after both runs carried BUILTIN\\Administrators, NT AUTHORITY\\SYSTEM, INTERACTIVE, SERVICE and BATCH both explicitly AND as `(I)` inherited copies - and NO AppContainer package (`S-1-15-2-...`, `APPLICATION PACKAGE AUTHORITY`) ACE of any kind. WHY THAT IS DECISIVE. The allow child did not misbehave and the fixture did not mis-time: the KERNEL refused it, and at the moment it ran the secret carried no package grant for anybody. The allow identity reaches the secret only by inheritance from the directory it was granted, and `apply_protected_deny` (acl_lease.rs:1303) deletes EVERY AppContainer-package ALLOW ace on the target - its own and any concurrent identity's alike - and then sets `PROTECTED_DACL_SECURITY_INFORMATION` precisely so that no inheritable package ALLOW can re-apply. The duplicated explicit/inherited normal-SID pairs in the DACL are that protection's fingerprint. Denial-by-absence-of-a-grant is per-OBJECT, and the object is shared, so it is categorical across identities by construction. The fixture's own hold window is not implicated: both executions ran to completion inside 148 ms and the failure is an OS access check, not a timeout."
  - id: c2
    text: "concurrent_allow_and_deny_identities_do_not_interfere passes at retries=0 over N of at least 20 on the AppContainer-capable host"
    state: superseded
    successor: FerroxLabs/wayland-core#368
    owner: core
    handoff: "FerroxLabs/wayland-core#368"
    note: "DECOMPOSED into FerroxLabs/wayland-core#368, not deferred. c1 was the measurement half and is closed; this is the FIX half, and the measurement showed the fix is a design change to how AppContainer denial is applied, not a tightening of the existing code. THE CONTRACT the new ticket carries, written from the c1 evidence: (1) `apply_protected_deny` must delete only the CALLING identity's own package-SID ALLOW aces, never every `S-1-15-2-...` ALLOW on the object - today it deletes a concurrent identity's grant along with its own; (2) protecting the object is still required to sever the deny identity's own inheritable grant, but the protect must first MATERIALISE, as explicit aces, the package ALLOWs other identities currently hold by inheritance, so protection does not silently revoke them; (3) the converse ordering has to be closed too, and it is the harder half - when a grant is applied to a directory AFTER a descendant has been protected, the inheritable ace cannot reach that descendant, so `apply_explicit_access` must reach protected descendants of a granted root explicitly. Both orderings occur: the mutation lock serialises the two ACL phases but does not order them, and the c1 run shows the two executions overlapping wholly. ACCEPTANCE, unchanged from this criterion: the named test passes at retries=0 over N>=20 on an AppContainer-capable host, with the deny half still non-vacuous (c3). WHY IT IS NOT DONE HERE: this is unsafe Win32 ACL code on the sandbox's containment boundary, where an over-broad grant is a silent security regression, and it wants its own review rather than a rider on a run-the-Windows-box lane. The host is NOT a blocker any more - c1 proves the loop is local and takes seconds per iteration. DECOMPOSITION COMPLETED 2026-08-29 on lane/f13-u-win-native: #368 had no ledger entry, so the remainder existed only as issue prose and nothing could grade it. .planning/ledger/wayland-core-368.md now carries five criteria taken from the c1 evidence and the issue contract - the three mechanism changes, the N>=20 acceptance, and the non-vacuity of the deny half as its own criterion so a repair cannot satisfy the acceptance by making the deny arm stop denying. RE-CONFIRMED against the tree at b52fb934 rather than taken from the note: `apply_protected_deny` at acl_lease.rs:1303 still deletes on `is_app_package_sid(ace_sid)` alone - it never compares against the calling identity - and its caller `apply_intents` at :661 already HAS that identity in hand as `sid`, so criterion 1 is a small change and criteria 2 and 3 are the real work."
  - id: c3
    text: "Whichever arm the measurement indicts, the deny half of the test is still non-vacuous afterwards"
    state: superseded
    successor: FerroxLabs/wayland-core#368
    owner: core
    handoff: "FerroxLabs/wayland-core#368"
    note: "Travels with c2 to the same ticket, FerroxLabs/wayland-core#368, and is now a live constraint rather than a moot one: the arm the c1 measurement indicts IS the deny implementation, so any repair is a repair to the code the deny assertions grade. The deny assertions at live_fs_acl.rs:475-478 remain unmodified today. Recorded so the fix cannot quietly satisfy c2 by weakening them: in the c1 instrumented run the deny child returned `exit_code 1` with empty stdout, so denial is presently REAL and is what a repair must preserve - a change that makes the allow arm pass by making the deny arm stop denying meets c2 and fails the issue."
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
