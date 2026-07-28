| # | class | serial re-run on hetzner | test | why |
|---|-------|--------------------------|------|-----|
| 1 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_absolute_symlink_escape_is_refused_and_nothing_outside_changed` | `python3` absent from the CI image |
| 2 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_casefold_collision_keeps_both_peer_items_accounted` | `python3` absent from the CI image |
| 3 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_conservation_invariant_balances_across_every_corpus` | `python3` absent from the CI image |
| 4 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_data_that_merely_looks_executable_is_not_contained` | `python3` absent from the CI image |
| 5 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_deeply_nested_configuration_is_named_not_panicked` | `python3` absent from the CI image |
| 6 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_directory_symlink_escape_is_refused_and_nothing_outside_changed` | `python3` absent from the CI image |
| 7 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_env_credential_reports_its_name_and_never_its_value` | `python3` absent from the CI image |
| 8 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_every_case_declares_a_legitimate_outcome_and_what_it_attacks` | `python3` absent from the CI image |
| 9 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_exact_name_collision_is_reported_and_never_silently_overwrites` | `python3` absent from the CI image |
| 10 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_excessive_item_count_hits_the_declared_refusal` | `python3` absent from the CI image |
| 11 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_executable_claiming_to_be_data_is_still_contained` | `python3` absent from the CI image |
| 12 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_generator_can_go_red` | `python3` absent from the CI image |
| 13 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_manifest_payload_mismatch_is_refused_by_verification` | `python3` absent from the CI image |
| 14 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_name_distinctions_survive_on_this_platform_or_the_generator_says_so` | `python3` absent from the CI image |
| 15 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_normalform_collision_keeps_both_peer_items_accounted` | `python3` absent from the CI image |
| 16 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_oversized_member_hits_the_declared_refusal` | `python3` absent from the CI image |
| 17 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_refused_restore_leaves_an_occupied_target_byte_identical` | `python3` absent from the CI image |
| 18 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_secret_in_a_memory_note_never_reaches_a_plan_or_report` | `python3` absent from the CI image |
| 19 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_secret_in_a_persona_body_never_reaches_a_plan_or_report` | `python3` absent from the CI image |
| 20 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_secret_in_a_skill_body_is_contained_and_never_reported` | `python3` absent from the CI image |
| 21 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_traversal_symlink_escape_is_refused_and_nothing_outside_changed` | `python3` absent from the CI image |
| 22 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_truncated_configuration_is_named_not_panicked` | `python3` absent from the CI image |
| 23 | C1 no python3 | `PASS_SERIAL` | `wcore-cli::portability_hostile_corpus hostile_wrong_typed_configuration_is_named_not_panicked` | `python3` absent from the CI image |
| 24 | C2 no ps | `PASS_SERIAL` | `wcore-exec-backend orphan::tests::the_local_scanner_finds_a_descendant_that_was_deliberately_left_behind` | `ps` absent; wcore-exec-backend/src/orphan.rs:321 shells out to it |
| 25 | C2 no ps | `PASS_SERIAL` | `wcore-exec-backend orphan::tests::the_real_process_table_passes_its_own_self_test_on_this_host` | `ps` absent; wcore-exec-backend/src/orphan.rs:321 shells out to it |
| 26 | C2 no ps | `PASS_SERIAL` | `wcore-exec-backend orphan::tests::the_scanner_does_not_count_its_own_process` | `ps` absent; wcore-exec-backend/src/orphan.rs:321 shells out to it |
| 27 | C2 no ps | `PASS_SERIAL` | `wcore-exec-backend::conformance_matrix every_reference_backend_passes_the_same_harness_or_reports_why_it_did_not` | `ps` absent; wcore-exec-backend/src/orphan.rs:321 shells out to it |
| 28 | C2 no ps | `PASS_SERIAL` | `wcore-exec-backend::conformance_matrix the_local_backend_is_always_exercised_because_it_needs_nothing_external` | `ps` absent; wcore-exec-backend/src/orphan.rs:321 shells out to it |
| 29 | C2 no ps | `PASS_SERIAL` | `wcore-exec-backend::fail_closed_matrix the_local_scan_finds_an_orphan_that_no_registry_remembers` | `ps` absent; wcore-exec-backend/src/orphan.rs:321 shells out to it |
| 30 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-agent::typed_execution_policy_e2e_test typed_bypass_executes_bash_inside_required_sandbox` | no sandbox backend can enforce; test demands a live one |
| 31 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-cli::sandbox_activeness sandbox_exec_confines_a_write_that_escapes_the_workspace` | no sandbox backend can enforce; test demands a live one |
| 32 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-sandbox backends::bwrap::tests::required_live_bwrap_admission` | no sandbox backend can enforce; test demands a live one |
| 33 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-sandbox backends::bwrap::tests::required_live_bwrap_hard_containment_mint_and_drift` | no sandbox backend can enforce; test demands a live one |
| 34 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-sandbox backends::bwrap::tests::required_live_bwrap_retained_cwd_enforcement` | no sandbox backend can enforce; test demands a live one |
| 35 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-sandbox::hard_process_containment qualified_hard_containment_backend_preflight` | no sandbox backend can enforce; test demands a live one |
| 36 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::dispatch_smoke dispatches_4_noop_workers_in_parallel` | no sandbox backend can enforce; test demands a live one |
| 37 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::dispatch_smoke heartbeat_symlink_cannot_make_parent_disclose_host_data_or_hang` | no sandbox backend can enforce; test demands a live one |
| 38 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::dispatch_smoke malformed_heartbeat_fails_closed_and_preserves_bounded_diagnostic` | no sandbox backend can enforce; test demands a live one |
| 39 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::dispatch_smoke public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state` | no sandbox backend can enforce; test demands a live one |
| 40 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::heartbeat_test worker_writes_heartbeat_during_long_running_task` | worker could not start because dispatch needs a live sandbox |
| 41 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::swarm_worker_failure_reporting_e2e swarm_reports_failed_worker_status_and_succeeding_workers_complete` | no sandbox backend can enforce; test demands a live one |
| 42 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::worker_runtime_limits aborting_dispatch_future_kills_tree_and_releases_workspace` | worker could not start because dispatch needs a live sandbox |
| 43 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::worker_runtime_limits cancellation_kills_worker_descendant_and_releases_owned_workspace` | worker could not start because dispatch needs a live sandbox |
| 44 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::worker_runtime_limits cleanup_preserves_live_worker_until_owner_releases_it` | worker could not start because dispatch needs a live sandbox |
| 45 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::worker_runtime_limits many_entry_accounting_does_not_block_cancellation` | worker could not start because dispatch needs a live sandbox |
| 46 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::worker_runtime_limits multi_worker_output_exhaustion_fails_without_retaining_buffers` | no sandbox backend can enforce; test demands a live one |
| 47 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::worker_runtime_limits timeout_releases_workspace_and_capacity_before_return` | no sandbox backend can enforce; test demands a live one |
| 48 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-swarm::worker_runtime_limits workspace_growth_observer_kills_worker_releases_reservation_and_admits_successor` | no sandbox backend can enforce; test demands a live one |
| 49 | C3 no bubblewrap | `PASS_SERIAL` | `wcore-tools::bash_sandbox_routing_test delegated_mutation_required_live_sandbox_confines_parent_and_descendants` | no sandbox backend can enforce; test demands a live one |
| 50 | C4 descendant reaping | `PASS_SERIAL` | `wcore-eval-scenarios pty_capture::tests::direct_child_exit_reaps_pty_descendant_group` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 51 | C4 descendant reaping | `PASS_SERIAL` | `wcore-eval-scenarios pty_capture::tests::drop_reaps_live_pty_process_group` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 52 | C4 descendant reaping | `PASS_SERIAL` | `wcore-eval-scenarios::runner_contracts assertion_failure_still_reaps_owned_descendant_listener` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 53 | C4 descendant reaping | `PASS_SERIAL` | `wcore-eval-scenarios::runner_contracts cancellation_reaps_owned_descendant_listener` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 54 | C4 descendant reaping | `PASS_SERIAL` | `wcore-eval-scenarios::runner_contracts direct_child_early_exit_reaps_owned_descendant_listener` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 55 | C4 descendant reaping | `PASS_SERIAL` | `wcore-eval-scenarios::runner_contracts dropping_runner_future_reaps_owned_descendant_listener` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 56 | C4 descendant reaping | `PASS_SERIAL` | `wcore-eval-scenarios::runner_contracts normal_exit_reaps_owned_descendant_listener` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 57 | C4 descendant reaping | `PASS_SERIAL` | `wcore-eval-scenarios::runner_contracts outer_deadline_reaps_owned_descendant_listener` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 58 | C4 descendant reaping | `PASS_SERIAL` | `wcore-eval-scenarios::runner_contracts timeout_reaps_owned_descendant_listener` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 59 | C4 descendant reaping | `PASS_SERIAL` | `wcore-sandbox::process_capture linux::cancellation_kills_the_owned_process_tree` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 60 | C4 descendant reaping | `PASS_SERIAL` | `wcore-sandbox::process_capture linux::stdout_flood_is_bounded_and_kills_descendant` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 61 | C4 descendant reaping | `PASS_SERIAL` | `wcore-swarm worktree::tests::linux::status_output_cap_kills_git_descendant` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 62 | C4 descendant reaping | `PASS_SERIAL` | `wcore-swarm worktree::tests::linux::worktree_add_timeout_kills_tree_and_reports_preserved_residual` | MECHANISM NOT ESTABLISHED — passes serially on hetzner; NOT `ps` (process_tree.rs reads /proc) |
| 63 | C5 container timing | `ABSENT` | `wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed` | no resolvable 40-hex source SHA for the checkout inside the container |
| 64 | C5 container timing | `PASS_SERIAL` | `wcore-cli::f14_sigkill_recovery recovered_approval_approve_executes_effect_once_and_continues_once` | marker file never appeared / timed out; passes serially |
| 65 | C5 container timing | `PASS_SERIAL` | `wcore-cli::f14_sigkill_recovery sigkill_during_tool_execution_requires_reconciliation_without_reexecution` | marker file never appeared / timed out; passes serially |
| 66 | K1 KNOWN | `ABSENT` | `wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream` | BACKLOG.md:516 wall-clock-budgeted binary tests flaky under load (3.0014s vs a 3.0s budget) |
| 67 | R1 REAL DEFECT | `FAIL_SERIAL` | `wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte` | contract source-digest guard — see R1 |
| 68 | S1 STALE TEST | `PASS_SERIAL` | `wcore-cli sandbox_cmd::tests::sandbox_context_carries_the_contained_profile_and_the_selected_registry` | assert_ne on two values that are both "fail_closed" where no real backend exists |

- C1 no python3: 23
- C3 no bubblewrap: 20
- C4 descendant reaping: 13
- C2 no ps: 6
- C5 container timing: 3
- K1 KNOWN: 1
- R1 REAL DEFECT: 1
- S1 STALE TEST: 1
