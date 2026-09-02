---
issue: 1291
repo: FerroxLabs/wayland
kind: defect
title: "report-gate wiring: 5 self-test assertions fail on integ/f13; build-darwin-selfhosted is aggregated by nothing"
status: open
last_verified_commit: 67fa14db6
criteria:
  - id: c1
    text: "The report job's aggregate gate grades the set it depends on by construction rather than by a hand-written list, every ci.yml job is either aggregated or declared not-aggregated, and the gate's prerequisites carry the gate's own admission."
    state: met
    evidence: "file:.github/workflows/ci.yml"
    owner: core
    note: "MET 2026-09-01 at 9e92f3bb5, and CONFIRMED GREEN IN CI: job 'fmt + clippy (workspace, all targets)' step 3 'Self-test the CI evidence gates' conclusion success -- the STEP was checked, not just the job label, so a skipped step cannot read as a pass. Six red assertions across two scripts went to zero. Three mechanisms: (1) the aggregate now takes NEEDS_JSON from toJSON(needs) and iterates it via assert-no-dependency-failed.sh, which already existed in the tree fully written and never wired, so there is no second list to drift; (2) checkout, artifact download, the aggregate and the evidence gate all carry !cancelled(), because a step with no if: is implicitly if: success() and stands down exactly when a gate matters -- measured on run 33320774111, where the evidence gate died exit 127 'No such file or directory' when it meant 'a dependency failed'; (3) build-darwin-selfhosted is declared not-aggregated rather than added to needs, because it runs on push to lane/** only and so cannot run on the path that gates main, while adding it would let an offline self-hosted Mac pend a required check for 24h. The job-level if: always() is deliberately UNCHANGED: a conditionally skipped job is recorded as successful even when required. CROSS-REVIEWED with Kimi K3 and Codex 5.6 Sol, which converged; Codex supplied the always()-vs-skipped point above. RED ARMS on this tree, restored and re-verified at 0 FAILs after each: reverting the evidence gate to a needs-result expression gives 3 FAILs including the new assertion naming the offending condition, deleting the not-aggregated declaration gives 1, re-introducing one hand-enumerated check gives 1. SUPERSEDED ONE ASSERTION deliberately: report-gate-wiring.test.sh grepped for a literal needs-result expression that gate-admission.py now forbids, so the two rules contradicted; it now asserts the property instead, with an anti-vacuity arm. KNOWN RESIDUAL, raised by Codex and NOT closed here: the aggregate treats skipped as OK, so a dependency skipped by ACCIDENT is still invisible."
  - id: c2
    text: "A dependency that is SKIPPED BY ACCIDENT is visible to the aggregate: the gate distinguishes a job skipped by a declared, intended condition from one skipped for any other reason, rather than treating every skip as OK."
    state: not-met
    evidence: "file:.github/scripts/assert-no-dependency-failed.sh"
    owner: core
    note: "OPEN, and it is why this issue stays open rather than being closed on c1. Raised by Codex 5.6 Sol during the c1 cross-review and NOT closed by c1's fix. assert-no-dependency-failed.sh treats `skipped` as OK, deliberately -- the macOS and Windows legs are rationed by design and a skipped leg contributes no red -- but that makes EVERY skip indistinguishable, so a job skipped because its own `if:` silently stopped matching, or because an upstream need failed and cascaded, is graded exactly like an intended ration. That is the same defect class this issue is about, one level down: the aggregate grades the set it depends on, but not the REASON each member concluded as it did. Closing it needs an explicit statement of which jobs may skip and under what condition, so anything else fails closed. NOT attempted in 0.13.12 -- the fix would change the admission arithmetic of a REQUIRED check under release pressure, which is how a gate is made to stand down silently."
---

# The aggregate now grades what it depends on

c1 is fixed in 0.13.12 at 9e92f3bb5 and confirmed green in CI -- the STEP, not just the job
label. c2 is the residual Codex named during the cross-review and is NOT fixed, which is
why this issue is milestoned 0.13.13 and stays open: an accidental skip is still
indistinguishable from an intended one.
