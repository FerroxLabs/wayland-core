---
issue: 1288
repo: FerroxLabs/wayland
kind: defect
title: "Three Linux retry-flakes surfaced in the run that overran its 120-min timeout; rate unmeasured, entries expire 2026-09-20"
status: open
last_verified_commit: e16de82cd
criteria:
  - id: c1
    text: "Each of the three entries is either deleted at expiry because a normal-duration ci-linux run does not reproduce it, or carries a measured rate from a run that did not overrun its timeout."
    state: met
    evidence: "file:.planning/FLAKE-CENSUS-20260910.md:95:The deletion branch is falsified"
    owner: core
    note: "MET 2026-09-10 by the SECOND branch, not the first. The deletion branch is dead: normal-duration ci-linux legs DO reproduce all three, so nothing is deleted at the 2026-09-20 expiry. What closes this is that each of the three `.config/flaky-allowlist.txt` entries now carries a measured incidence in place of `the rate here is NOT measured`. MEASUREMENT, recorded in full at .planning/FLAKE-CENSUS-20260910.md: CENSUS W = every ci.yml run on FerroxLabs/wayland-core created 2026-08-31T04:13Z..2026-09-10T04:42Z (n=201, upper bound is the release head 34438211271 on integ/release-0.13.14); all 338 nextest-junit artifacts downloaded, every XML read and DEDUPLICATED BY SHA256 because junit.xml is a byte-identical copy of the final outer attempt and summing both double-counts. DENOMINATOR = nextest EXECUTIONS producing a JUnit report with at least one testcase -- macos-latest 104, linux-containerized 131, Array 110, windows-latest-hosted 9, total 354. NUMERATOR = flakyFailure/flakyError elements, one per `TRY n FAIL` line; that equivalence is VERIFIED against two job logs rather than assumed (run 33719833206 CI (Array) job 100536520508: JUnit 2, log `TRY 1 FAIL`+`TRY 2 FAIL`+`TRY 3 PASS`, and those were the only two `TRY n FAIL` lines in the whole job; run 34212497448 CI (macos-latest) job 102016587698: JUnit 1, log `TRY 1 FAIL`+`TRY 2 PASS`). THE THREE RATES, on linux-containerized: sigkill_during_tool_execution_requires_reconciliation_without_reexecution 3/131 executions (2.3 %), 4 attempts, runs 33474156159, 33488263125, 33618437825; stop_during_active_host_continue_preserves_unknown_provider_authority 3/131 (2.3 %), 3 attempts, runs 33488263125, 33584616526, 33733145558; tui_renders_the_chrome_and_every_tab_on_boot 7/131 (5.3 %), 7 attempts, runs 33488263125, 33724413765, 33752019921, 33843483817, 33930737901, 34108695034, 34173361738 across six branches. All three are also seen on macos-latest at 1/104 or a hard failure, and NONE of them executes on either Windows leg (0 of 110 Array and 0 of 9 windows-hosted EXECUTIONS of the test), so their Windows zeros are vacuous and are recorded as such. WHAT THIS IS NOT: it is a per-execution incidence at the CI profile's `retries = 2`, NOT a per-execution failure rate at `--retries 0`. The per-attempt readings the entries also carry (3.0 %, 2.2 %, 5.1 %) are estimates, because retries execute at a different point in the suite and therefore under different load. A `--retries 0` Linux characterisation at n>=20 per test would still upgrade these; this census cannot, and does not claim to."
---

# Three Linux flakes seen once, under a degraded run

Ledgered for coverage; the honest disposition is deletion at expiry unless reproduced.
