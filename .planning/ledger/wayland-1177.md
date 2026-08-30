---
issue: 1177
repo: FerroxLabs/wayland
kind: defect
title: "An outer workflow retry overwrites junit.xml, erasing a genuine failure before any grader sees it"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "A failure on attempt 1 followed by a pass on attempt 2 leaves evidence the required report check can read"
    state: not-met
    evidence: "file:.github/scripts/run-tests-with-attempt-evidence.sh:101:cp "$JUNIT_PATH" "$ATTEMPT_DIR/outer-attempt-${attempt}.xml""
    owner: core
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: was :90, which the 2026-08-29 sweep recorded as the `cp` line and which is now an echo -- the file drifted 11 lines under the anchor. The preservation copy is at :101. Each outer attempt's junit.xml is copied to outer-attempt-<n>.xml; wired at ci.yml:1657 under nick-fields/retry max_attempts: 2, uploaded at ci.yml:2005-2007 and read by grade-retry-flakes.sh:168 inside the required report job. The outer retry was PRESERVED, not dropped: the script also removes the stale report before each attempt and writes final-status.txt so 'retried into green' is distinguishable from 'red on the final attempt'. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: Evidence resolves — `.github/scripts/run-tests-with-attempt-evidence.sh:90` is the `cp '$JUNIT_PATH' '$ATTEMPT_DIR/outer-attempt-${attempt}.xml'` line, wired at ci.yml:1676 under nick-fields/retry@v3 max_attempts:2, uploaded at ci.yml:2024-2026, read by grade-retry-flakes.sh:167-202 which runs inside the required `report` job via assert-test-evidence.sh:85-91. The logic is right on paper and I confirmed it end-to-end against fixtures. But it has NEVER once worked on a real runner, and the one time it executed it did not merely fail to preserve evidence — it destroyed the whole test leg. On the fix's own CI run (33227927478, job 99035159787, 2026-08-29T02:33:48Z, sha 2282de368) the log reads verbatim: `##[group]Attempt 1` `mkdir: cannot create directory ‘target/nextest’: Permission denied` `##[warning]Attempt 1 failed. Reason: Child_process exited with error code 2` `##[group]Attempt 2` `mkdir: cannot create directory ‘target/nextest’: Permission denied` `##[error]Final attempt failed. Child_process exited with error code 2` Cause: ci.yml:1156 documents that `docker run` here has NO `-u`, so the container runs as root while the bind-mounted workspace is uid 1001. `target/` is therefore created root-owned by the earlier `Pre-build`/`clippy` container steps, and the wrapper — which runs on the HOST as uid 1001 — dies at `mkdir -p '$ATTEMPT_DIR' || exit 2` (line 66) before it ever invokes nextest. So nextest never ran, no junit.xml was written, no attempt was preserved, and the required `report` check received zero evidence from this leg. Every subsequent run I checked (33255873933, 33256631848, 33257261615, 33257637102, 33258318108, 33258433476, 33258531983, 33258742623, 33258852685) has `Run tests (nextest CI profile)` = skipped because the job died earlier, so nothing has re-exercised it. No later commit repairs it: `git log 2282de36..HEAD -- ci.yml run-tests-with-attempt-evidence.sh` shows only unrelated changes, there is no `chown`/`sudo` of `target/` anywhere in ci.yml, and line 66 at HEAD (5eb2d1ef) is still the bare `mkdir -p`. The criterion as written — 'leaves evidence the required report check can read' — is false in the only environment that matters. UPDATE 2026-08-30: the mkdir BLOCKER is removed. e7144c30 reserves target/nextest/ci/outer-attempts as the runner user before any container step makes target/ root-owned, and on run 33264117264 the wrapper reached `outer-retry attempt 1` with `attempt dir : target/nextest/ci/outer-attempts` and no permission error; the leg then ran 17,341 tests, all passing. THAT IS NOT THIS CRITERION AND MUST NOT BE READ AS IT. That run contains ZERO occurrences of `outer-retry attempt 2` -- attempt 1 passed, so the retry path was never exercised and nothing was preserved or read. The criterion needs attempt 1 to FAIL and attempt 2 to PASS, with the required report job reading outer-attempt-1.xml. TO CLOSE IT: make one test fail deterministically on the first attempt only (an env-var-gated fixture the wrapper unsets between attempts), push, and quote the report job reading the preserved attempt. A green run where attempt 1 succeeded is evidence for the BLOCKER being gone and evidence for nothing else -- grading it as c1 is the substituted-property failure this cycle has now shipped six times."
  - id: c2
    text: "A test demonstrates that visibility and fails against today's workflow"
    state: not-met
    evidence: "file:.github/scripts/tests/outer-retry-evidence.test.sh:87:failing_junit "$A/outer-attempts/outer-attempt-1.xml" "races_under_load""
    owner: core
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: moved off the comment at :84 onto the line that builds the failed-attempt-1 fixture, which is the demonstration itself. Three parts: grade-retry-flakes.sh on fixture evidence, the writer against a stub that fails then passes, and a grep of the ci.yml wiring. Two negative controls (a clean suite stays green; an ordinary red is not re-reported) plus the agent-crash shape where no JUnit is written. It grades the scripts and the wiring, not a live CI replay. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: The test exists and is real work. `.github/scripts/tests/outer-retry-evidence.test.sh:84` resolves (a comment inside the primary defect case at :83-93). I ran it at HEAD on hetzner: 19 passed, 0 failed. It is wired into CI at `.github/workflows/lint.yml:129` (`Self-test the CI evidence gates`, default `bash -e -o pipefail`, so a failure reds the step). It genuinely fails against the pre-fix tree: I reconstructed 2282de36^ (974e2474) into a scratch dir and ran today's test against it — 8 passed, 11 failed, including `attempt-1 failure retried into a green step reds the report (want exit=1, got exit=0)`. It is not vacuous: five mutations, each verified to land on CODE and each reverted with `touch` afterward — delete the `cp` at wrapper:90 → 17/2; break the fail-closed on absent final-status at grader:171 → 18/1; delete `rm -f '$JUNIT_PATH'` at wrapper:82 → 18/1; drop `[ '$OUTER_UNLISTED' -gt 0 ] ||` from grader:311 → 16/3; strip the wrapper from ci.yml:1676 → 18/1. Baseline restored to 19/0 after each. Both negative controls pass in both arms. Why it still does not hold: the ticket asks for 'a test demonstrating that a failure on attempt 1 followed by a pass on attempt 2 is still visible to the required `report` check'. This test substitutes a weaker property — Part A/B run the scripts in a tmp dir the test itself owns, and Part C is two `grep -qF` calls against ci.yml plus one against assert-test-evidence.sh. A grep for the string `bash .github/scripts/run-tests-with-attempt-evidence.sh` is not a demonstration that the report check sees anything. The proof is that this suite is 19/19 GREEN on a tree where the mechanism is provably 100% inoperative on the real runner. The ledger note concedes it ('It grades the scripts and the wiring, not a live CI replay') — that concession is exactly the hole the break shipped through. This is the same substituted-property failure already recorded on wayland#559 c6 and wayland-core#358 c1."
  - id: c3
    text: "The #1169 retry-flake grader keeps working on the nextest layer it already covers"
    state: met
    evidence: "file:.github/scripts/grade-retry-flakes.sh:119:find "$EVIDENCE_DIR" -type f -name "*.xml" -print0"
    owner: core
    note: "The #1169 nextest-layer scan is intact at :119; the outer-attempt scan at :168 is an ADDITIONAL pass keyed on outer-attempt-*.xml plus final-status.txt, with an absent status graded fail-closed."
---

`.github/workflows/ci.yml` wraps the containerized Linux workspace test run in
`nick-fields/retry@v3` with `max_attempts: 2`. The second attempt overwrites
`target/nextest/ci/junit.xml`, so a test that genuinely failed on attempt 1 and
passed on attempt 2 leaves no structured trace at all. It is not even recorded
as a flake, because the flake record lived in the file that was just
overwritten.

This is the same defect as #1169 one layer up, and strictly worse. #1169's
grader runs inside the required `report` job and reads that XML — so it cannot
see this layer, because the outer retry destroys the evidence before the grader
runs. The fix for #1169 is real and incomplete.

Nothing has been done. The issue also records a practical obstacle for whoever
picks it up: editing `ci.yml` needs a token with `workflow` scope, which the
hetzner build box does not have. That is a routing detail, not a blocker on
another lane, so this stays core's and not-met rather than blocked.
