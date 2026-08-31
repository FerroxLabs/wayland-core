---
issue: 412
repo: FerroxLabs/wayland-core
kind: defect
title: "A redaction violation in one ledger file silently disables 24 ci-linux gate steps on the shipping tree"
status: open
last_verified_commit: c7f188c49
criteria:
  - id: c1
    text: "The two violations on the shipping tree are resolved -- redacted, or allowlisted in the checker with the one-line reason the checker itself asks for -- proven by `scripts/check-no-personal-identifiers.py` exiting 0 on that tree, with `main` re-run in the same session as the control that a 0 means the checker still fires."
    state: met
    evidence: "file:scripts/check-no-personal-identifiers.py"
    owner: core
    note: "MET on integ/f13. Both violations fixed at SOURCE, not by moving the baseline -- the checker-s own docstring says bumping it teaches everyone to bump the baseline, which is how a ratchet dies. (1) wayland-1252.md quoted a URL USERINFO component in a note about host parsing; not a personal address, but a real email shape in committed content, and the detector is right to be blind to intent. Local part angle-bracketed so the shape dissolves while the example still reads as the URL it always was -- no domain allowlisted, detector not weakened. (2) Two doc comments in appcontainer/acl_lease transcribed a real operator-s Windows home directory from live diagnostic output; placeholdered. CONTROL, same checker, same tree, one session: RC=1 before the three edits and RC=0 after, home-path ratchet 32 -> 30 against a baseline of 30, and the checker-s own --self-test RC=0 after the change. main was RC=0 throughout, which is what makes this a regression inside the integration window rather than a standing condition."
  - id: c2
    text: "A failing early gate in `ci-linux` no longer suppresses the gates after it: the independent check steps run under `!cancelled()` (or equivalent) so one red measures one thing. PROVEN by a live run in which an early gate step fails and a later gate step still reports its own verdict -- not by reading the YAML."
    state: not-met
    owner: core
    note: "THE ACTUAL DEFECT; c1 is only what exposed it. `ci-linux`'s `No personal identifiers in committed content` is the SIXTH step and carries no `if: always()` / `!cancelled()`, so everything after it is skipped. MEASURED on run 33401094665, job `CI (linux-containerized)`: 24 steps `skipped`, including `Criteria ledger is anchored and parseable`, `Release-readiness gate can still fail`, `Windows CI failures stay attributable`, `macOS/Windows admission control is consistent`, `Check formatting`, `Clippy (warnings = errors)`, `Run tests (nextest CI profile)`, `Check Desktop protocol contract corpus drift`, `F01 packaged wayland-eval driver gate` and `Security audit`. NOT a silent green -- the job concludes `failure` and `report` fails with it -- so what is lost is the MEASUREMENT, not the alarm. SCOPE, stated rather than overclaimed: the macOS and self-hosted Windows legs run fmt, clippy and the workspace suite independently, so those platforms still gate; what is unmeasured is the Linux suite and the host-side gate battery, which exists in exactly one place. PRECEDENT: ci.yml already documents this ordering defect twice (clippy before tests on the matrix leg; the contract-drift check) and `ci-windows-hosted` was deliberately built tests-first with clippy after under `!cancelled()` for this reason. `ci-linux` did not get the same treatment."
  - id: c3
    text: "The suppression cannot silently return: a check grades the ordering and is driven RED by reintroducing it, so `ci-linux` cannot go back to reporting one failure on behalf of twenty-four unmeasured steps."
    state: not-met
    owner: core
    note: "Without this, c2 is a one-time edit that the next step added to `ci-linux` undoes by default -- a new step written without `!cancelled()` re-creates the suppression for everything after it, and no gate in the repo would notice. The red arm is the whole requirement: a check that only passes on today's file is the shape this repo has already been burned by."
---

Filed 2026-08-31 by `lane/f13-s3-ci-routing` as an incidental finding while discharging
wayland-core#409 c6. It is filed rather than mentioned because a lane summary is not a
carrier, and because the finding is about the gates the RELEASE depends on rather than
about this lane's criterion.

WHY kind: defect. On the tree that is about to ship, no CI run measures the Linux test
suite, the criteria-ledger gate, the release-readiness gate, the contract-drift check or
the security audit. Over-blocking costs a conversation; shipping with the gate battery
unmeasured is the thing the battery exists to prevent.

NOT FIXED HERE, and the reason is not shyness: c1's two violations are in another lane's
ledger file and in a repo-wide ratchet, and c2 is a structural edit to the job that is
every lane's critical path. Both belong to whoever owns integration, not to a lane that
happened to notice.
