# Retry-flake census, 2026-09-10

The measurement four release-blocking tickets asked for and none of them had:
wayland#1288 c1 (rate for the three Linux entries), wayland#1286 c1 (the true
cluster population), wayland#1284 / wayland#1285 (the per-platform attribution)
and wayland#1238 c4 (`TRY n FAIL` counted across CI logs, before and after).

Everything below is measured. Where a number could not be obtained it says so
rather than being estimated.

## Window and denominator, stated once

**CENSUS W** = every `ci.yml` run on `FerroxLabs/wayland-core` created
`2026-08-31T04:13Z .. 2026-09-10T04:42Z`, n = **201**
(`gh run list -R FerroxLabs/wayland-core --workflow ci.yml --limit 400`, then
filtered to that interval). The upper bound is run `34438211271` on
`integ/release-0.13.14`, the release head.

From those runs, all **338** `nextest-junit-*` artifacts were downloaded
(none expired). Every `.xml` in every artifact was read and **deduplicated by
sha256**: an artifact holds `junit.xml` (the final outer attempt) plus
`outer-attempts/outer-attempt-N.xml`, and when the final attempt also has a
numbered copy the two are byte-identical. Summing both double-counts.

> That double-count was a real bug in the first pass of this measurement, found
> by checking a JUnit-derived count of 2 against a job log that showed 1
> `TRY n FAIL`. It is also live in `.github/scripts/grade-retry-flakes.sh`,
> which globs `-name "*.xml"` with no dedup: run 34212497448's macOS artifact
> makes it print "FAILED 2 time(s)" where the job log has one `TRY 1 FAIL`.
> The gate's VERDICT is unaffected (allowed vs red is per-test, not per-count);
> only the attempt number in its message is wrong. Not fixed here.

**Denominator = nextest EXECUTIONS that produced a JUnit report holding at
least one `<testcase>`.** Not runs, not jobs, not test invocations:

| leg | executions |
|-----|-----------:|
| `CI (macos-latest)` | 104 |
| `CI (linux-containerized)` | 131 |
| `CI (Array)` (self-hosted Windows) | 110 |
| `CI (windows-latest, hosted)` | 9 |
| **total** | **354** |

A further 79 leg-jobs concluded success/failure without a JUnit artifact. Their
job logs were downloaded and graded directly: **76 died before nextest started**
(build/setup failure) and 3 ran it. Those 3 Linux executions carry 18
`TRY n FAIL` lines, all of them hard failures of
`wcore-cli::release_binary_smoke` (a real red, not a retry-mask), and **zero**
for any test named in this document.

**Numerator = `<flakyFailure>` / `<flakyError>` elements. One such element is
exactly one `TRY n FAIL` line.** That equivalence is verified against job logs,
not assumed:

* run `33719833206`, `CI (Array)`, job `100536520508` — JUnit says 2 flaky
  attempts for `parallel_spawn_caps_active_child_engines_across_shared_calls`;
  the log has `TRY 1 FAIL`, `TRY 2 FAIL`, `TRY 3 PASS`, and those two were the
  **only** `TRY n FAIL` lines in the entire job.
* run `34212497448`, `CI (macos-latest)`, job `102016587698` — JUnit says 1;
  the log has `TRY 1 FAIL`, `TRY 2 PASS`, `FLAKY 2/3`.

## What a zero here means, and when it means nothing

Every zero below is paired with an execution count. **A test that never runs on
a leg produces the same zero as a test that runs and never fails**, and several
of the platform claims this census was asked to check turn on exactly that
distinction. Measured in the same pass as the flake counts, so the control and
the finding come from one query.

## The eight tests the tickets name

Format: `flaked-in / executed-in (TRY-n-FAIL attempts)`.

| test | macos-latest | linux-containerized | Array | windows-hosted | verdict on its platform claim |
|------|-------------|--------------------|-------|---------------|-------------------------------|
| `wcore-tools::bash_manifest_bound_live_backend::the_live_backend_timeout_bounds_the_manifest_build_and_names_it` (#1284) | **5/104 (6)** | 0/131 | 0/108 | 0/9 | **macOS-only SURVIVES** on real controls |
| `wcore-mcp::transport::stdio::tests::f016_real_spawn_uses_sanitized_launch_context` (#1285) | **7/104 (7)** | 0/131 | not run | not run | macOS-only survives vs Linux, **untested vs Windows** |
| `wcore-cli::harness_tui_flow::resume_repaints_prior_conversation_into_the_transcript` (#1285) | 4/104 (4) | **17/131 (25)** | not run | not run | **macOS-only FALSE — it is a Linux flake** |
| `wcore-tools::walk_parallel_identity_test::redundant_walk_root_is_not_walked_twice` (#1286) | 4/**85** (4) | 0/131 | **1/108 (1)** | 0/9 | **macOS-only FALSE — fired on Array** |
| `wcore-cli::f14_sigkill_recovery::sigkill_during_tool_execution_requires_reconciliation_without_reexecution` (#1288) | 1/104 (1) | **3/131 (4)** | not run | not run | Linux, as filed |
| `wcore-cli::f14_sigkill_recovery::stop_during_active_host_continue_preserves_unknown_provider_authority` (#1288) | 0/104 + 1 hard | **3/131 (3)** | not run | not run | Linux, as filed |
| `wcore-cli::harness_tui_flow::tui_renders_the_chrome_and_every_tab_on_boot` (#1288) | 1/104 (1) | **7/131 (7)** | not run | not run | Linux, as filed |
| `wcore-agent::spawner::spawn_task_set_tests::parallel_spawn_caps_active_child_engines_across_shared_calls` (#1238) | 0/104 | 0/131 | **1/109 (2)** | 0/9 + 1 hard | Windows self-hosted |

Four of those "not run" cells are the vacuous zeros. `wcore-cli::harness_tui_flow`,
`wcore-cli::f14_sigkill_recovery` and `wcore-mcp::transport::stdio::tests`
execute **zero times** on either Windows leg across all 119 Windows executions
in W, so no Windows zero for those tests is evidence of anything.

### wayland#1288 — the three Linux entries now have a rate

Their allowlist lines said `the rate here is NOT measured` and instructed
deletion at the 2026-09-20 expiry if a normal-duration ci-linux run did not
reproduce them. **All three reproduce, several times, on normal-duration legs.**
The deletion branch is falsified; the lines are rewritten to carry the numbers
above. Occurrences:

* `sigkill_during_tool_execution...` — runs 33474156159, 33488263125 (2
  attempts), 33618437825. Per-attempt on Linux: 4 of 135 executed attempts (3.0 %).
* `stop_during_active_host_continue...` — runs 33488263125, 33584616526,
  33733145558. Per-attempt: 3 of 134 (2.2 %).
* `tui_renders_the_chrome_and_every_tab_on_boot` — runs 33488263125,
  33724413765, 33752019921, 33843483817, 33930737901, 34108695034, 34173361738,
  across six branches. Per-attempt: 7 of 138 (5.1 %). The heaviest of the three.

The per-attempt figures are an ESTIMATE, not a `--retries 0` rate: retries run
at a different point in the suite and therefore under different load.

### wayland#1286 — the population is 47, and the allowlist is still reactive

`CI (macos-latest)`: **30 of 104 executions carried at least one retry-masked
failure, over 17 distinct tests** — not five. Workspace-wide the population is
**47 distinct tests**: macos-latest 17, linux-containerized 18, Array 21,
windows-latest-hosted 2.

The criterion's second clause — "so the allowlist stops being written
reactively" — is **measurably unfulfilled**:

* **19 distinct tests produced a retry-masked failure in W with no allowlist
  line at all.**
* **4 allowlisted entries never fired once** in all 354 executions: lines 56,
  57, 58 (expiry 2026-10-15) and line 64 (expiry 2026-09-15) of
  `.config/flaky-allowlist.txt`.

That is 19 misses against 32 entries, 4 of which are covering nothing. The
allowlist is still tracking whichever member happened to fire.

### wayland#1238 c4 — `TRY n FAIL` before and after, from the logs

`DRAIN_BACKSTOP` landed in `774c40f5a` (2026-09-03T16:55Z). Every run in W was
classified by **ancestry, not by date** —
`gh api repos/FerroxLabs/wayland-core/compare/774c40f5a...<headSha>`, where
`ahead`/`identical` means the tree contains the fix and `behind`/`diverged`
means it does not. 59 runs contain it, 65 do not.

`parallel_spawn_caps_active_child_engines_across_shared_calls`:

| arm | macos | linux | Array | win-hosted | total executions | TRY n FAIL | hard fail |
|-----|------:|------:|------:|-----------:|-----------------:|-----------:|----------:|
| **BEFORE** (tree lacks 774c40f5a) | 49 | 69 | 56 | 9 | **183** | **2** | 1 |
| **AFTER** (tree contains it) | 55 | 62 | 53 | 0 | **170** | **0** | 0 |

The 2 BEFORE attempts are both run `33719833206`, `CI (Array)` — whose tree
`compare` reports as `diverged` from the fix, so it genuinely predates it. The
1 BEFORE hard failure is run `33584616526` on `windows-latest-hosted`.

**This record does not demonstrate the fix worked.** 1 flaked execution in 183
against 0 in 170 is not a distinguishable difference at these n; the rule of
three puts the AFTER arm's upper bound at ~1.8 %, which comfortably contains the
BEFORE arm's 0.55 %. What is now on the record is the rate, before and after,
which is what c4 asked for — not a demonstration of repair, which it did not.

`windows-latest-hosted` contributes 0 executions to the AFTER arm: that leg ran
9 times in W and all 9 predate the fix.

## The macOS walk-identity step: a `--retries 0` arm nobody noticed

`wcore-tools::walk_parallel_identity_test` results are **absent from every
macos-latest JUnit report from 2026-09-07T16:20Z onward** (19 consecutive
reports, including the release head 34438211271) while present in all Linux and
Array reports of the same runs. That is why its macOS denominator above is 85
and not 104.

The cause is not a lost test. `.github/workflows/ci.yml` gained a step
`Walk identity controls (isolated macOS execution)`, `if: runner.os == 'macOS'`,
running that binary at `--retries 0 --test-threads 1`; the main `Run tests` step
then excludes the binary, and its `junit.xml` overwrites the isolated step's.
Confirmed in the release-head macOS job log (`102747677856`): all five cases
`PASS`, `redundant_walk_root_is_not_walked_twice` at 1.944s.

Harvested from all 25 macOS job logs since the step landed:

* **19 executions at `--retries 0` on native macOS, 0 failures**, from
  34142759867 (2026-09-07T16:20Z) to 34438211271 (2026-09-10T04:42Z).
* 5 macOS legs never reached the step (the job failed earlier).
* 1 leg (34108695034, 2026-09-07T09:55Z) predates the step; there
  `a_disjoint_grant_really_does_cost_a_second_walk` flaked `TRY 1 FAIL` /
  `TRY 2 PASS` inside the main suite.

**This is NOT the population arm #1286 asks for.** `--test-threads 1` removes
the parallel contention the flake needs, so it changes the condition rather
than measuring it. Two consequences that are on the record either way:

1. There is now real native-macOS `--retries 0` data for this binary, n = 19, 0
   failures, single-threaded.
2. Line 68 of the allowlist is **unreachable on macOS**. With `--retries 0` a
   macOS failure is a hard step failure, never a `<flakyFailure>`, so
   `grade-retry-flakes.sh` can never allow it. The entry can now only ever be
   exercised by the Array leg.

## Limits of this census

1. **It is not a `--retries 0` rate.** Every figure except the walk-identity
   arm is a per-execution incidence under the CI profile's `retries = 2`. It
   cannot substitute for the bounded population job #1284, #1285 and #1286 each
   ask for.
2. **Retries execute under different load than first attempts**, so the
   per-attempt percentages are estimates.
3. **JUnit coverage is not total.** 354 executions are graded from artifacts and
   3 more from logs; 76 leg-jobs died before nextest and contribute nothing.
4. **Only `ci.yml` is covered.** The shared-process `cargo test` lib suite has
   no retries and no JUnit, so it cannot mask a failure and is out of scope —
   but it is also not measured here.
5. **Ancestry classification is per-run `compare` against one commit.** It is
   exact for 774c40f5a and was not repeated for any other fix.
6. **No test was run for this census.** No build slot was used, no `cargo`
   invocation was made. Everything is CI history.

## Appendix: every test with a retry-masked failure in W

`flaked-in/executed-in (attempts)`; `not run` means the test never executed
on that leg, so its zero is vacuous. 47 tests; 19 carry no allowlist line.

| test | allowlisted | macos-latest | linux-containerized | Array | windows-latest-hosted |
|------|-------------|-------------|--------------------|-------|----------------------|
| `wcore-cli::harness_tui_flow::resume_repaints_prior_conversation_into_the_transcript` | yes | 4/104 (4 att) | 17/131 (25 att) | not run | not run |
| `wcore-agent::turn_cost_per_byte_guard::one_turn_costs_about_two_whole_payload_scrub_passes` | yes | 6/104 (9 att) | 3/131 (3 att) | 8/109 (9 att) | 0/7 |
| `wcore-agent::session::tests::test_f033_index_lock_parallel` | yes | 0/104 | 0/131 | 11/109 (12 att) | 2/9 (2 att) |
| `wcore-agent::workflow_limits_test::fix1_dispatch_budget_aborts_with_partial_result` | yes | 0/104 | 0/131 | 6/90 (7 att) | 7/9 (10 att) |
| `wcore-cli::migrate_quarantine::t19_live_negative_leg_quarantined_payload_does_not_execute` | yes | 0/104 | 0/131 | 11/108 (14 att) | 0/9 |
| `wcore-cli::f14_sigkill_recovery::packaged_host_continue_and_non_genesis_reconnect_are_exactly_once` | yes | 0/104 | 8/131 (9 att) | not run | not run |
| `wcore-cli::harness_tui_flow::tui_renders_the_chrome_and_every_tab_on_boot` | yes | 1/104 (1 att) | 7/131 (7 att) | not run | not run |
| `wcore-mcp::transport::stdio::tests::f016_real_spawn_uses_sanitized_launch_context` | yes | 7/104 (7 att) | 0/131 | not run | not run |
| `wcore-cli::quarantine_console_authority_windows::quarantine_child_has_no_console_at_creation_on_windows` | yes | not run | not run | 6/108 (6 att) | 0/9 |
| `wcore-config::credentials::chunk_crash_injection::interrupted_rotations_do_not_leak_entries_without_bound` | yes | 0/104 | 0/131 | 6/108 (7 att) | 0/9 |
| `wcore-agent::approval_pty_raw_partial_line::raw_mode_with_nothing_typed_still_denies` | **NO** | 0/104 | 5/131 (5 att) | not run | not run |
| `wcore-cli::f14_sigkill_recovery::a_session_whose_key_is_gone_is_refused_by_name_and_only_that_session` | yes | not run | 5/131 (6 att) | not run | not run |
| `wcore-tools::bash_manifest_bound_live_backend::the_live_backend_timeout_bounds_the_manifest_build_and_names_it` | yes | 5/104 (6 att) | 0/131 | 0/108 | 0/9 |
| `wcore-tools::walk_parallel_identity_test::redundant_walk_root_is_not_walked_twice` | yes | 4/85 (4 att) | 0/131 | 1/108 (1 att) | 0/9 |
| `wcore-cli::f14_sigkill_recovery::packaged_fresh_process_reopens_sealed_request_and_dispatches_once` | yes | 0/104 | 4/131 (7 att) | not run | not run |
| `wcore-cli::f14_sigkill_recovery::sigkill_during_model_stream_resumes_as_provider_reconciliation_without_redispatch` | yes | 1/104 (1 att) | 3/131 (3 att) | not run | not run |
| `wcore-cli::f14_sigkill_recovery::sigkill_during_tool_execution_requires_reconciliation_without_reexecution` | yes | 1/104 (1 att) | 3/131 (4 att) | not run | not run |
| `wcore-cli::f14_sigkill_recovery::stop_during_active_host_continue_preserves_unknown_provider_authority` | yes | 0/104 | 3/131 (3 att) | not run | not run |
| `wcore-config::credential_storage_test::keyring_backend_round_trip_when_available` | yes | 0/104 | 0/131 | 3/108 (3 att) | 0/9 |
| `wcore-agent::issue_1280_skills_ceiling_test::a_mistyped_skill_name_does_not_dump_the_whole_catalogue` | yes | 0/102 | 0/131 | 2/106 (2 att) | 0/5 |
| `wcore-cli::f14_sigkill_recovery::sigkill_while_awaiting_approval_restores_gate_without_provider_or_tool_replay` | **NO** | 1/104 (1 att) | 1/131 (2 att) | not run | not run |
| `wcore-cli::quarantine_terminal_authority_windows::a_quarantine_child_does_not_inherit_the_users_console` | **NO** | not run | not run | 2/108 (2 att) | 0/9 |
| `wcore-providers::chatgpt_bearer_expiry_147::a_bearer_that_dies_midstream_does_not_disturb_the_accepted_stream` | **NO** | 0/104 | 0/131 | 2/108 (2 att) | 0/9 |
| `wcore-tools::walk_parallel_identity_test::a_disjoint_grant_really_does_cost_a_second_walk` | **NO** | 1/85 (1 att) | 1/131 (1 att) | 0/108 | 0/9 |
| `wcore-agent::bootstrap_memory_enabled_smoke::bootstrap_with_memory_disabled_spawns_no_scheduler` | **NO** | 1/104 (1 att) | 0/131 | 0/109 | 0/9 |
| `wcore-agent::bootstrap_test::bootstrap_budget_trip_keeps_first_typed_termination_reason` | **NO** | 1/104 (1 att) | 0/131 | 0/109 | 0/9 |
| `wcore-agent::bootstrap_test::bootstrap_builds_engine_with_model_in_prompt` | **NO** | 1/104 (1 att) | 0/131 | 0/109 | 0/9 |
| `wcore-agent::dangerous_lease_e2e_test::dangerous_expiry_cancels_production_streaming_bash_process_tree` | yes | 0/104 | 1/131 (1 att) | not run | not run |
| `wcore-agent::dangerous_lease_e2e_test::dangerous_expiry_reaches_bootstrapped_spawn_child` | yes | 0/104 | 1/131 (1 att) | not run | not run |
| `wcore-agent::spawner::spawn_task_set_tests::parallel_spawn_caps_active_child_engines_across_shared_calls` | **NO** | 0/104 | 0/131 | 1/109 (2 att) | 0/9 |
| `wcore-browser::sidecar_egress_proxy_test::a_name_that_resolves_to_nothing_is_refused_at_the_proxy` | **NO** | 1/104 (1 att) | 0/131 | 0/109 | 0/9 |
| `wcore-cli::headless_acp_boot::an_interactive_first_run_still_prints_the_key` | **NO** | 1/104 (1 att) | 0/131 | not run | not run |
| `wcore-cli::json_stream_startup_refusal::healthy_start_still_emits_ready` | **NO** | 0/104 | 0/131 | 1/108 (1 att) | 0/9 |
| `wcore-cli::stabilization_recovery::w04_finalizer_during_delete` | **NO** | 0/21 | 1/4 (1 att) | not run | not run |
| `wcore-cli::stabilization_recovery::w04_journal_failure_during_cleanup` | **NO** | 0/21 | 1/4 (2 att) | not run | not run |
| `wcore-cli::stabilization_recovery::w04_physical_completion_before_receipt` | **NO** | 0/21 | 1/4 (1 att) | not run | not run |
| `wcore-cli::stabilization_recovery::w04_receipt_before_settlement` | **NO** | 0/21 | 1/4 (1 att) | not run | not run |
| `wcore-config::credentials::chunk_write_lock_verification::a_second_writer_cannot_commit_over_a_parked_writers_parts` | yes | 0/104 | 0/131 | 1/108 (1 att) | 0/9 |
| `wcore-config::credentials::chunk_write_lock_verification::racing_writers_never_yield_a_spliced_credential` | yes | 0/104 | 0/131 | 1/108 (1 att) | 0/9 |
| `wcore-config::multi_account_provider_test::two_accounts_on_one_provider_each_resolve_their_own_stored_key` | **NO** | 0/104 | 0/131 | 1/108 (1 att) | 0/9 |
| `wcore-skills::watcher_tests::tc06_file_modify_triggers_notification` | yes | 0/104 | 0/131 | 1/108 (1 att) | 0/9 |
| `wcore-skills::watcher_tests::tc07_file_delete_triggers_notification` | yes | 0/104 | 0/131 | 1/108 (1 att) | 0/9 |
| `wcore-skills::watcher_tests::tc08_file_rename_triggers_notification` | yes | 0/104 | 0/131 | 1/108 (1 att) | 0/9 |
| `wcore-skills::watcher_tests::tc09_debounce_coalesces_multiple_events` | yes | 0/104 | 0/131 | 1/108 (1 att) | 0/9 |
| `wcore-skills::watcher_tests::tc20_version_monotonically_increasing` | **NO** | 0/104 | 0/131 | 1/108 (1 att) | 0/9 |
| `wcore-swarm::workspace_authority::independent_cli_processes_cannot_overbook_shared_capacity` | yes | 1/104 (1 att) | 0/131 | 0/108 | 0/9 |
| `wcore-tools::bash::tests::a_cancelled_streaming_bash_does_not_wait_for_the_secret_deny_walk` | **NO** | 1/104 (1 att) | 0/131 | 0/108 | 0/9 |

Allowlisted but never fired in any of the 354 executions:

* `wcore-swarm::worker_runtime_limits::multi_worker_output_exhaustion_fails_without_retaining_buffers` (line 56, expiry 2026-10-15)
* `wcore-agent::crucible_council::slow_proposer_hits_per_proposer_deadline` (line 57, expiry 2026-10-15)
* `wcore-cli::deterministic_openai_loop::packaged_f04_run_is_repeatable_and_content_addressed` (line 58, expiry 2026-10-15)
* `wcore-channel-matrix::refresh_cross_process::two_processes_issue_exactly_one_refresh_post` (line 64, expiry 2026-09-15)
