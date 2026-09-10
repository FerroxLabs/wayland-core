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
    state: not-met
    evidence: "file:.planning/FLAKE-CENSUS-20260910.md:119:19 distinct tests produced a retry-masked failure"
    owner: core
    note: "STILL NOT MET. CLAUSE 1 (the cluster is a POPULATION with a denominator) IS DONE and is CONFIRMED here against the primary source rather than inherited. CLAUSE 2 ('so the allowlist stops being written reactively') IS MEASURABLY UNFULFILLED, and this note supplies what the criterion actually needs next: the mechanism that would fulfil it, and its cost. CONFIRMATION OF CLAUSE 1, 2026-09-10 by the w15/mac lane. Two censuses exist and they disagree in their denominators, so the current one is named: CENSUS W (.planning/FLAKE-CENSUS-20260910.md and the .config/flaky-allowlist.txt lines) is n=201 ci.yml runs over 2026-08-31T04:13Z..2026-09-10T04:42Z, 338 artifacts, 354 nextest EXECUTIONS with JUnit -- macos-latest 104, linux-containerized 131, Array 110, windows-latest-hosted 9. The table in the body of THIS file is the earlier 200-run/103-artifact pass and its denominators are superseded; its per-test ordering is not. Under CENSUS W: 30 of 104 macos-latest executions carried at least one retry-masked failure over 17 DISTINCT TESTS, and the workspace-wide population is 47 distinct tests (macOS 17, Linux 18, Array 21, windows-hosted 2). The ticket's 'fifth member' framing was an artefact of grading four runs. CLAUSE 2, MEASURED: 19 distinct tests produced a retry-masked failure in W with NO allowlist line at all, while 4 of the 32 entries never fired once in 354 executions (lines 56, 57, 58 and 64). 19 misses against 32 entries, 4 of them covering nothing. The list is still tracking whichever member happened to redden a report job. WHY IT IS STILL REACTIVE, in one sentence: the ONLY signal that ever reaches a human is grade-retry-flakes.sh failing the report job for an UNLISTED flake that happened to fire in the run someone was looking at. Discovery is therefore one member per CI cycle by construction, and coverage is whatever fired -- exactly what this ticket names. THE MECHANISM THAT WOULD MAKE IT NON-REACTIVE. Invert the direction: derive the list from a POPULATION MEASUREMENT on a schedule, not from incidents. (1) A scheduled workflow (say flake-census.yml, weekly) that does what this cluster's grading has now done BY HAND TWICE: enumerate ci.yml runs in a fixed window, download every nextest-junit-* artifact, deduplicate every XML by sha256, and count flakyFailure/flakyError per test per leg against a denominator of executions-with-JUnit. Output a machine-readable census committed or attached as an artifact. (2) Two NEW gate outputs that nothing currently produces, both graded off that census rather than off an incident: COVERAGE MISSES -- every test with incidence > 0 and no allowlist line, reported with its rate BEFORE it happens to redden a run; and DEAD ENTRIES -- every allowlist line with zero observations across the window, flagged for DELETION at expiry rather than renewal. (3) Entries are then written from a rate with a denominator instead of from a single payload, which is the whole content of clause 2. WHAT IT COSTS, stated rather than waved at. BUILD: roughly one day; the hard part (dedup by sha256, the executions-not-runs denominator) is already solved and written down in FLAKE-CENSUS-20260910.md. RUNTIME: ~200 runs x up to 4 artifacts, 338 downloads in the measured window -- API-rate-limited but not expensive, and it must run often enough that ARTIFACT RETENTION does not silently shrink the denominator, which is a vacuity trap of its own (an expired artifact is indistinguishable from a leg that did not flake). MAINTENANCE: the census must encode the vacuous-zero traps or it will manufacture platform claims -- harness_tui_flow, f14_sigkill_recovery and wcore-mcp::transport::stdio::tests execute ZERO times across all 119 Windows executions in W, so every 'Windows: 0' for them is a report of absence and not of health; and redundant_walk_root_is_not_walked_twice has run in an isolated macOS step at --retries 0 --test-threads 1 since 2026-09-07T16:20Z with the main step excluding it, so its macOS denominator is 85 and not 104 and a macOS success at the release head is NOT evidence that its flake is fixed. THE CEILING: none of this can ever produce a --retries 0 rate. Everything a census reads is per-execution incidence under retries = 2. Turning incidence into a rate still needs a bounded job that runs the pinned list at --retries 0 with n >= 20 per test per platform, and on macOS that job is the expensive one this whole cluster has been owed since it was filed. WHAT WAS DELIBERATELY NOT DONE: the 19 unlisted tests were NOT bulk-written into the allowlist. An entry means 'this is known to need retries, someone owns it, and the debt has a date'; nineteen entries added in one commit have no owner and no measurement behind them, and they would convert 19 unknown flakes into 19 SILENT retries -- the precise harm #1169 built the gate to stop, at scale, and reactive writing rather than the end of it. It is also forbidden to this lane: .config/flaky-allowlist.txt is held by another worker. WHAT IS OWED FOR THIS ROW TO GO MET: the census workflow and its two gate outputs above (the criterion's second clause is about the WRITING PROCESS, so a mechanism is the only thing that can satisfy it), plus the bounded native macOS --retries 0 job for the rate. Line 68 is NOT deletable: it reproduced in runs 33399094711, 33474156159, 33771177611, 33781532317 on macOS and 33843558494 on Array, the last on 2026-09-04. Expiry stays 2026-10-01."
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
