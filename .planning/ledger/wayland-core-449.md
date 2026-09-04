---
issue: 449
repo: FerroxLabs/wayland-core
kind: defect
title: "[mutants-nightly] wcore-cron — surviving mutants (2026-09-04)"
status: open
last_verified_commit: 57e2a244e
criteria:
  - id: c1
    text: "The surviving mutants reported for wcore-cron are dispositioned: each is either killed by a new or strengthened test, or recorded with a reason it is not worth killing."
    state: not-met
    evidence: ""
    owner: core
    note: "Auto-filed by github-actions at 2026-09-04T10:14:16Z from run 33844721279, DURING the 0.13.13 integration swarm and after this swarm's own gate run had already started. Ledgered on the core#416 precedent: an open in-scope issue with no ledger file fails the coverage arm outright, so a bot filing mid-session reds the release gate on coverage alone. UNTRIAGED -- this entry buys coverage and claims no disposition. NO milestone, so it is outside 0.13.13 readiness scope and does not join the 42; ledger-coverage scope only. Reported figures, quoted not inferred: catch rate 75.4%, 339 mutants tested in 52m, 64 missed, 196 caught, 77 unviable, 2 timeouts."
  - id: c2
    text: "core#424 is re-graded against this run, since its recorded premise -- that mutants-nightly has produced zero data across 87 runs -- is refuted by a run that produced 339 mutants."
    state: not-met
    evidence: ""
    owner: core
    note: "THE REASON THIS FILE MATTERS MORE THAN ITS OWN CONTENT. core#424 is one of the 42 issues blocking 0.13.13 and its criteria rest on the claim that the mutation leg has never produced a data point. Run 33844721279 produced one. So core#424 must be re-graded against this run before it is either fixed or deferred -- and it must be re-graded from the ARTIFACT, not the job log, because core#424's own history records that grepping the job log returned 10 false hits for the summary line where the truth was 0, the log having echoed the workflow's own comments. Not asserting core#424 is fixed: producing data is necessary for its criteria, not obviously sufficient, and one green run is not the property its criteria demand."
---

# An auto-filed mutation-testing result, ledgered for coverage, untriaged

Filed by automation mid-swarm on 2026-09-04. This file makes no claim about the
surviving mutants; it records what the run reported and why it bears on core#424.

Worth a human eye before the cut, though it is not itself a release blocker: the
missed mutants cluster hard in `crates/wcore-cron/src/lease.rs`, and specifically
in `sys::try_lock_exclusive` and `sys::unlock` -- replacing `unlock` with `()`
survives, and `try_lock_exclusive` survives being replaced by both `Ok(true)` and
`Ok(false)`. That is the file-locking path, and this repo has already shipped one
measured lock defect of exactly that family (a data-file lock duplicated through
`fork()`, refusing 47.6% of reopens under load). Under-tested locking code is not
proof of a second such defect, but it is the wrong place to be blind.

Structural note, the same one core#443 carries: an automated nightly can red the
release gate at any hour purely by filing, because coverage scopes EVERY open issue
on either tracker. Two fired during this one session, four hours apart. `ci.yml`
runs the checker `--offline`, which skips coverage and divergence entirely, so the
class is invisible on every PR and surfaces only when a release is attempted.
