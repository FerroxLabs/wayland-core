---
issue: 397
repo: FerroxLabs/wayland-core
kind: defect
title: "Stale doc comment on CredentialsBackend::Auto claims a plaintext fallback the code refuses"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "The doc comment at crates/wcore-config/src/credentials.rs:40-47 and the module header at :8-12 state the fail-closed behaviour the implementation actually has: build_ladder mounts keyring then encrypted vault and nothing else, and put refuses when no secure rung is mounted."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "A test or a grep gate fails when the documented ladder and the rungs build_ladder actually mounts disagree again. The failure mode here is 2,650 lines of distance between the claim and the code, and distance does not shrink on its own."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c3
    text: "The correction is verified at tag v0.13.11, where the claim was made, AND at HEAD -- so it is not graded on a tree where the comment already differed."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c4
    text: "The plaintext store's real status is stated where the stale comment was: read-and-delete-only legacy, or an explicit backend = plaintext opt-out that warns on stderr. Deleting the false sentence without stating the true one invites the next reader to guess again."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
---

Created 2026-08-31. This issue was filed 2026-08-29/30 by this cycle's own
verification, was in scope for the release gate from that moment, and had no
ledger file -- so scripts/check-release-readiness.py, which reads ledger files
and nothing else, could not count it. CI runs the coverage arm with --offline,
which is the arm that would have said so.

Its body declared no acceptance criteria, so it could not have been closed as
filed either. The criteria above are AUTHORED from measurements the body
already records.

Not cosmetic. In one afternoon this comment produced a false Core falls back to
plaintext credentials claim in THREE independent readings -- two external audit
models and one drafting pass -- and came within one review of being published
in a public threat model, conceding a security weakness fixed nine releases
ago. The cost of this defect is already measured and it is not zero.
