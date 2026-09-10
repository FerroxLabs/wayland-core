---
issue: 1286
repo: FerroxLabs/wayland
kind: defect
title: "macOS retry-flake cluster: redundant_walk_root_is_not_walked_twice is the 5th member; discovery is one per CI cycle"
status: open
last_verified_commit: e16de82cd
criteria:
  - id: c1
    text: "The cluster is characterised as a population rather than discovered one member per CI cycle, so the allowlist stops being written reactively."
    state: not-met
    evidence: "file:.planning/FLAKE-CENSUS-20260910.md:119:19 distinct tests produced a retry-masked failure"
    owner: core
    note: "STILL NOT MET, but the FIRST HALF IS DONE: the cluster is now enumerated as a population with a denominator instead of one member per CI cycle. See the census table in the body of this file. CI-HISTORY CENSUS, 2026-09-10, and it REFUTES the deletion branch of this criterion for all six lines this lane was asked to grade. Method: every `ci.yml` run `gh run list` returns at limit 200 (2026-08-31T04:13 .. 2026-09-09T12:32); for each, every unexpired `nextest-junit-<leg>` artifact downloaded and every `<testcase>` carrying `<flakyFailure>`/`<flakyError>` counted. 126 of the 200 runs had JUnit artifacts: macos-latest 103, linux-containerized 112, Array 110, windows-latest-hosted 8. This is a per-RUN flake incidence at the CI profile's `retries = 2`, NOT a per-execution failure rate at `--retries 0` -- a run counts once if the test failed at least one attempt and then passed. It cannot substitute for the `--retries 0` population job any of these tickets asks for, and is not offered as one. RESULT: over 103 macos-latest JUnit artifacts (93 of those legs concluded success), 30 runs carried at least one macOS flakyFailure and SEVENTEEN distinct tests appear -- not five. The ticket's `5th member` framing was a discovery artefact of grading four runs, exactly the method defect this ticket names. The distribution has a heavy tail: five tests account for 26 of the 39 macOS flake-run observations and twelve are singletons. THE CLUSTER IS ALSO NOT A macOS DISEASE. Of the five named members only two are macOS-exclusive across all legs in the window (f016_real_spawn_uses_sanitized_launch_context 7/103 macOS, 0 Linux, 0 Array; the_live_backend_timeout_bounds_the_manifest_build_and_names_it 5/103, 0, 0). redundant_walk_root_is_not_walked_twice is 4/103 macOS but ALSO fired on Array in run 33843558494 (2026-09-04), so line 68's `MEASURED FLAKY ON macOS ONLY` is FALSIFIED. resume_repaints_prior_conversation_into_the_transcript is 4/103 macOS against 16/112 Linux -- predominantly a Linux flake. And the second-heaviest macOS member, wcore-agent::turn_cost_per_byte_guard::one_turn_costs_about_two_whole_payload_scrub_passes at 6/103, is platform-independent (6 macOS / 3 Linux / 8 Array) and was never part of the `macOS cluster` frame at all. So the population is not `the macOS flakes`; it is the tail of a workspace-wide flake distribution, sampled through whichever leg someone happened to grade. WHY THIS IS STILL NOT MET: the criterion's second clause is `so the allowlist stops being written reactively`, and the census measures that it has NOT. EIGHTEEN distinct tests produced a flakyFailure in this window with no allowlist line at all (including all four wcore-cli::stabilization_recovery::w04_* members on Linux, wcore-agent::bootstrap_test x2 and wcore-tools::bash::tests::a_cancelled_streaming_bash_does_not_wait_for_the_secret_deny_walk on macOS, and wcore-tools::walk_parallel_identity_test::a_disjoint_grant_really_does_cost_a_second_walk on both), while four allowlisted entries never fired in 200 runs. WHAT IS OWED: (1) the proactive allowlist write covering the enumerated population -- forbidden to this lane, two other workers hold pending edits in that file; (2) the bounded native macOS job at `--retries 0`, n>=10 over the pinned list, which is the only thing that turns this per-run incidence into a rate. Line 68 is NOT deletable: it reproduced in runs 33399094711, 33474156159, 33771177611, 33781532317 (macOS) and 33843558494 (Array), the last on 2026-09-04. Its expiry is 2026-10-01, not 2026-09-20."
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
