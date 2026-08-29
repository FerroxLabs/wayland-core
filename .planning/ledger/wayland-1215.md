---
issue: 1215
repo: FerroxLabs/wayland
kind: defect
title: "The #1177 evidence wrapper cannot create target/nextest and kills the containerized Linux test leg on both attempts"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "run-tests-with-attempt-evidence.sh creates its attempt directory successfully on the hosted runner in the presence of a root-owned target/ left by the container steps"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D33, found while verifying wayland#1177). Nothing has been done. The measured finding, verbatim: The #1177 evidence wrapper aborts with `mkdir: cannot create directory 'target/nextest': Permission denied` (exit 2) on both retry attempts, so the containerized Linux nextest suite never runs at all. The wrapper executes on the GitHub-hosted runner as uid 1001, but ci.yml's `docker run` intentionally passes no `-u` (see the comment block at ci.yml:1133-1156), so `target/` is created root-owned by the earlier `Pre-build tool_token_bench` / `Pre-build wcore-cli release binary` / clippy container steps. `mkdir -p '$ATTEMPT_DIR' || exit 2` at line 66 then cannot create `target/nextest`. The same wall applies to `rm -f '$JUNIT_PATH'` (line 82) and the `cp` (line 90), which need the runner to own `target/nextest/ci`."
  - id: c2
    text: "A real CI run is cited BY URL in which 'Run tests (nextest CI profile)' executes and uploads junit.xml"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D33). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "In that same run the rm -f at wrapper:82 and the cp at wrapper:90 succeed -- an attempt file is preserved and named"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D33). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The #1177 evidence wrapper aborts with `mkdir: cannot create directory 'target/nextest': Permission denied` (exit 2) on both retry attempts, so the containerized Linux nextest suite never runs at all. The wrapper executes on the GitHub-hosted runner as uid 1001, but ci.yml's `docker run` intentionally passes no `-u` (see the comment block at ci.yml:1133-1156), so `target/` is created root-owned by the earlier `Pre-build tool_token_bench` / `Pre-build wcore-cli release binary` / clippy container steps. `mkdir -p '$ATTEMPT_DIR' || exit 2` at line 66 then cannot create `target/nextest`. The same wall applies to `rm -f '$JUNIT_PATH'` (line 82) and the `cp` (line 90), which need the runner to own `target/nextest/ci`.

**Where.** .github/scripts/run-tests-with-attempt-evidence.sh:66 (also :82, :90), invoked from .github/workflows/ci.yml:1676. Reproduced in CI run 33227927478, job 99035159787 (FerroxLabs/wayland-core), 2026-08-29T02:33:48Z, at commit 2282de368. Still unrepaired at integ/f13 HEAD 5eb2d1ef.

**Why it matters.** This is the leg that runs the full ~12,775-test workspace suite, the swarm delegated-dispatch certification, the contract-corpus drift check, the release binary smoke, the F01 packaged driver gate and cargo audit — every step after `Run tests` was marked `skipped` in that run. A fix filed to stop CI evidence being erased currently prevents the evidence from existing. It is masked right now only because nine consecutive runs died at an earlier gate and skipped the test step; the first run that gets past those gates will red at mkdir on every attempt.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
