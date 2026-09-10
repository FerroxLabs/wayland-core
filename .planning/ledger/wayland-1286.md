---
issue: 1286
repo: FerroxLabs/wayland
kind: defect
title: "macOS retry-flake cluster: redundant_walk_root_is_not_walked_twice is the 5th member; discovery is one per CI cycle"
status: open
last_verified_commit: 3c6d9a78c
criteria:
  - id: c1
    text: "The cluster is characterised as a population rather than discovered one member per CI cycle, so the allowlist stops being written reactively."
    state: met
    evidence: "file:scripts/flake-census.py"
    owner: core
    note: "MET 2026-09-10 by the w15/mac2 lane. Both of the things the previous grading said were owed now exist: the MECHANISM that ends reactive writing, and the bounded native-macOS --retries 0 job that turns incidence into a rate. Clause 1 (the cluster is a POPULATION with a denominator) was already confirmed against CENSUS W and is unchanged -- 30 of 104 macos-latest executions retry-masked over 17 distinct tests, 47 workspace-wide. What follows is clause 2. ===== THE MECHANISM ===== `scripts/flake-census.py` and `.github/workflows/flake-census.yml` (weekly cron plus workflow_dispatch). The script enumerates ci.yml runs over a window, downloads every nextest-junit-* artifact, DEDUPLICATES EVERY XML BY SHA256, and counts flakyFailure / flakyError per test per leg against a denominator of executions-with-JUnit. Its --gate mode emits the two outputs nothing previously produced, both derived from the census and not from an incident: COVERAGE MISSES (every test with incidence > 0 and no allowlist line, with its rate, BEFORE it happens to redden a run) and DEAD ENTRIES (every allowlist line with zero observations, flagged for deletion at expiry rather than renewal). The workflow publishes both into the job summary and does NOT fail on coverage misses -- it is a report, not a merge gate -- but DOES fail when the census itself is degenerate. THE FOUR VACUOUS-ZERO TRAPS ARE ENCODED IN THE OUTPUT, not in a comment. (1) A (test, leg) zero prints `VACUOUS (never executed)` when that leg's reports never contained the test, never `0`. (2) Expired and unreadable artifacts are counted, printed at the TOP of the report, and exit DEGENERATE above a threshold, because an expired artifact is indistinguishable from a leg that did not flake. (3) Per-(test, leg) denominators come from PRESENCE IN THE REPORT, so a test excluded from a leg's main step gets the smaller denominator automatically and is reported `NOT-EVERYWHERE`, never recommended for deletion. (4) Every rate carries the sentence that it is per-execution incidence under retries = 2 and that no census can produce a --retries 0 rate. IT HAS ACTUALLY RUN, twice, against the live API, and its zeros were checked rather than trusted. Over 2026-09-08T12:54Z..2026-09-10T12:54Z: 16 completed runs (2 in flight excluded), 3 with no JUnit artifact at all, 34 artifacts downloaded, 0 expired (0.0 %), 33 XML reports after 1 byte-identical duplicate dropped; denominators macos-latest 9, linux-containerized 13, Array 11, windows-latest-hosted 0. POSITIVE CONTROL: a wider 3-day run found the known members rather than a suspicious zero -- resume_repaints 3/23 on linux, f016 1/19 on macOS, test_f033_index_lock_parallel 3/20 on Array -- and reported 4 coverage misses with rates. TRAP (3) FIRED ON ITS OWN: the tool independently returned line 68, redundant_walk_root_is_not_walked_twice, as `NOT-EVERYWHERE (absent from macos-latest)` with `24 (macos-latest 0, linux 13, Array 11)` and REFUSED to recommend deleting it -- rediscovering, from the data alone, the isolated macOS step this ticket had to work out by hand. Dedup is measured rather than assumed: artifacts 10139399538 and 10138687658 of run 34438211271 are both 1272282 bytes with sha256 314c11cf139b, a SECOND double-count source (the -checkpoint leg) that the hand pass never named. ===== THE BOUNDED NATIVE macOS --retries 0 ARM, and it did not confirm health ===== INSTRUMENT: GitHub-hosted macos-latest, macOS 26.6.2 arm64, 3 logical CPUs, from THROWAWAY branches `measure/w15-mac2*` scoped so ci.yml (which lists lane/**, not measure/**) never fired. Runs 34478038368, 34478358189, 34479186713. Every arm at --retries 0, every count paired with a PRESENT-as-an-executed-testcase control so no zero below is a report of absence. redundant_walk_root_is_not_walked_twice: 3 FAILURES IN 50 INVOCATIONS (6.0 %) of `nextest run -p wcore-tools --test walk_parallel_identity_test --profile ci --retries 0` at DEFAULT PARALLELISM -- 2 of 30 on an idle host and 1 of 20 under a 2x-logical-CPU spinner load (loadavg 16-41), present in 20/20 and 20/20. Payloads: `a grant already covered by the workspace root walk cost 1.41x (baseline 76.092 ms, subject 107.277 ms)`, 1.45x (59.130/85.507) and 1.59x (85.046/135.454), against the test's `ratio < 1.35`. THIS IS THE POINT OF THE WHOLE ARM. Since 2026-09-07T16:20Z the macOS leg has run this binary in an isolated step at `--retries 0 --test-threads 1` with the main step excluding it, and that step has been GREEN 19 consecutive runs including the release head. Those greens have been read as reassurance. They are not: `--test-threads 1` removes the parallel contention the flake needs, so the step changes the condition rather than measuring it, and restoring default parallelism reproduces the failure at 6 %. Note also that the loaded arm's rate equals the idle arm's, so this is not load either -- it is a ratio between two wall-clock medians on a 3-core box with only a 1.35x margin. f016_real_spawn_uses_sanitized_launch_context: 0 of 30 (present 30/30) after the gh#1285 repair, and 1 of 1 on the FIRST invocation with that repair mutated out. resume_repaints_prior_conversation_into_the_transcript: 0 of 20 (present 20/20). tui_renders_the_chrome_and_every_tab_on_boot: 0 of 20 (present 20/20). AND THE WORKFLOW ITSELF HAS EXECUTED ON GITHUB ACTIONS, not only locally: run 34479719553, `Retry-flake population census`, GATE: PASSED, script exit 0. Over its default 14-day window (2026-08-27T12:57Z..2026-09-10T12:57Z) it read 489 completed ci.yml runs, downloaded 612 artifacts with ZERO expired or unreadable, counted 696 XML reports after dropping 71 byte-identical duplicates, and produced denominators macos-latest 150, linux-containerized 368, Array 156, windows-latest-hosted 22. It found 70 DISTINCT FLAKY TESTS and 46 COVERAGE MISSES -- against census W's 47 distinct tests and 19 misses over a 10-day window graded by hand. That is the difference between a population someone had time to enumerate once and one a schedule enumerates. ===== WHAT THIS ROW DOES NOT CLAIM ===== (1) The workflow has run on a THROWAWAY branch (measure/w15-mac2-census, with a temporary push trigger that is NOT in the file being merged); the weekly cron has never fired, and cannot until this lands on the default branch. (2) The 19 unlisted tests are STILL not bulk-written into the allowlist, deliberately and for the reason the previous grading gave -- nineteen entries in one commit have no owner and no measurement behind them and would convert 19 unknown flakes into 19 silent retries. The census REPORTS them with rates; writing an entry stays a decision with an owner, which is the whole content of 'stops being written reactively'. A future lane that pastes the coverage-miss table into the allowlist wholesale would defeat this row. (3) The macOS rates above are 20-50 invocations per test, not the n >= 20 per test per PLATFORM the ceiling paragraph asks for on every leg; Windows still executes none of these binaries and its zeros remain vacuous. (4) The walk finding is a REAL open defect handed on, not something this row closes: line 68 must not be deleted at its 2026-10-01 expiry, and the isolated `--test-threads 1` step must not be cited as evidence that the test is fixed."
---

# The discovery method, not just the member

Ledgered for coverage. Characterisation is 0.13.13 work.

## macOS flake population, measured 2026-09-10

Census over every `ci.yml` run `gh run list` returns at limit 200
(2026-08-31T04:13 .. 2026-09-09T12:32). Every unexpired `nextest-junit-<leg>`
artifact was downloaded and every `<testcase>` carrying `<flakyFailure>` or
`<flakyError>` counted. Legs with JUnit: macos-latest 103, linux-containerized 112,
Array 110, windows-latest-hosted 8.

This is a per-RUN flake incidence at the CI profile's `retries = 2` -- a run counts
once if the test failed at least one attempt and then passed. It is NOT a per-execution
failure rate at `--retries 0`, and it does not substitute for the bounded native macOS
job this ticket asks for.

30 of 103 macos-latest runs carried at least one flakyFailure. Seventeen distinct
tests appear, not five. `L` and `A` are the same test's run counts on
linux-containerized (n=112) and Array (n=110), which is what shows the cluster is not
macOS-specific.

| macOS runs | L | A | allowlisted | test |
|-----------:|--:|--:|-------------|------|
| 7/103 | 0 | 0 | yes | `wcore-mcp::transport::stdio::tests::f016_real_spawn_uses_sanitized_launch_context` |
| 6/103 | 3 | 8 | yes | `wcore-agent::turn_cost_per_byte_guard::one_turn_costs_about_two_whole_payload_scrub_passes` |
| 5/103 | 0 | 0 | yes | `wcore-tools::bash_manifest_bound_live_backend::the_live_backend_timeout_bounds_the_manifest_build_and_names_it` |
| 4/103 | 16 | 0 | yes | `wcore-cli::harness_tui_flow::resume_repaints_prior_conversation_into_the_transcript` |
| 4/103 | 0 | 1 | yes | `wcore-tools::walk_parallel_identity_test::redundant_walk_root_is_not_walked_twice` |
| 1/103 | 0 | 0 | **no** | `wcore-browser::sidecar_egress_proxy_test::a_name_that_resolves_to_nothing_is_refused_at_the_proxy` |
| 1/103 | 3 | 0 | yes | `wcore-cli::f14_sigkill_recovery::sigkill_during_model_stream_resumes_as_provider_reconciliation_without_redispatch` |
| 1/103 | 3 | 0 | yes | `wcore-cli::f14_sigkill_recovery::sigkill_during_tool_execution_requires_reconciliation_without_reexecution` |
| 1/103 | 1 | 0 | **no** | `wcore-cli::f14_sigkill_recovery::sigkill_while_awaiting_approval_restores_gate_without_provider_or_tool_replay` |
| 1/103 | 0 | 0 | **no** | `wcore-agent::bootstrap_test::bootstrap_budget_trip_keeps_first_typed_termination_reason` |
| 1/103 | 0 | 0 | **no** | `wcore-agent::bootstrap_test::bootstrap_builds_engine_with_model_in_prompt` |
| 1/103 | 7 | 0 | yes | `wcore-cli::harness_tui_flow::tui_renders_the_chrome_and_every_tab_on_boot` |
| 1/103 | 0 | 0 | **no** | `wcore-cli::headless_acp_boot::an_interactive_first_run_still_prints_the_key` |
| 1/103 | 0 | 0 | **no** | `wcore-tools::bash::tests::a_cancelled_streaming_bash_does_not_wait_for_the_secret_deny_walk` |
| 1/103 | 0 | 0 | **no** | `wcore-agent::bootstrap_memory_enabled_smoke::bootstrap_with_memory_disabled_spawns_no_scheduler` |
| 1/103 | 1 | 0 | **no** | `wcore-tools::walk_parallel_identity_test::a_disjoint_grant_really_does_cost_a_second_walk` |
| 1/103 | 0 | 0 | yes | `wcore-swarm::workspace_authority::independent_cli_processes_cannot_overbook_shared_capacity` |

Eighteen distinct tests produced a flakyFailure somewhere in this window with no
allowlist line at all, while four allowlisted entries never fired in 200 runs. That
gap is the reactive-writing this ticket names, quantified.
