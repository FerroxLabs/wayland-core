| # | class | also fails on Linux | test | why |
|---|-------|---------------------|------|-----|
| 1 | K1 KNOWN | YES | `wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream` | wall-clock budget under load |
| 2 | K1 KNOWN | YES | `wcore-swarm::worker_runtime_limits multi_worker_output_exhaustion_fails_without_retaining_buffers` | wall-clock budget under load |
| 3 | R1 REAL DEFECT | YES | `wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte` | same contract digest guard as Linux |
| 4 | UNCLASSIFIED | no | `wcore-cli::deterministic_openai_loop packaged_core_exhausts_a_real_read_timeout` | thread 'packaged_core_exhausts_a_real_read_timeout' (50288) panicked at crates\wcore-cli\t |
| 5 | UNCLASSIFIED | no | `wcore-cli::smoke_p0 smoke_06_first_prompt_uses_configured_provider_and_key` | thread 'smoke_06_first_prompt_uses_configured_provider_and_key' (36264) panicked at crates |
| 6 | UNCLASSIFIED | no | `wcore-cli::smoke_p0 smoke_10_model_override_reaches_outgoing_request` | thread 'smoke_10_model_override_reaches_outgoing_request' (53044) panicked at crates\wcore |
| 7 | UNCLASSIFIED | no | `wcore-swarm worktree::tests::a_live_peer_transaction_is_never_reclaimed` | thread 'worktree::tests::a_live_peer_transaction_is_never_reclaimed' (50380) panicked at c |
| 8 | UNCLASSIFIED | no | `wcore-swarm worktree::tests::abandoned_transaction_reservations_are_reclaimed` | thread 'worktree::tests::abandoned_transaction_reservations_are_reclaimed' (40556) panicke |
| 9 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_absolute_symlink_escape_is_refused_and_nothing_outside_changed` | `python3` absent from the Windows runner too |
| 10 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_casefold_collision_keeps_both_peer_items_accounted` | `python3` absent from the Windows runner too |
| 11 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_conservation_invariant_balances_across_every_corpus` | `python3` absent from the Windows runner too |
| 12 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_data_that_merely_looks_executable_is_not_contained` | `python3` absent from the Windows runner too |
| 13 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_deeply_nested_configuration_is_named_not_panicked` | `python3` absent from the Windows runner too |
| 14 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_directory_symlink_escape_is_refused_and_nothing_outside_changed` | `python3` absent from the Windows runner too |
| 15 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_env_credential_reports_its_name_and_never_its_value` | `python3` absent from the Windows runner too |
| 16 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_every_case_declares_a_legitimate_outcome_and_what_it_attacks` | `python3` absent from the Windows runner too |
| 17 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_exact_name_collision_is_reported_and_never_silently_overwrites` | `python3` absent from the Windows runner too |
| 18 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_excessive_item_count_hits_the_declared_refusal` | `python3` absent from the Windows runner too |
| 19 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_executable_claiming_to_be_data_is_still_contained` | `python3` absent from the Windows runner too |
| 20 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_generator_can_go_red` | `python3` absent from the Windows runner too |
| 21 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_manifest_payload_mismatch_is_refused_by_verification` | `python3` absent from the Windows runner too |
| 22 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_name_distinctions_survive_on_this_platform_or_the_generator_says_so` | `python3` absent from the Windows runner too |
| 23 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_normalform_collision_keeps_both_peer_items_accounted` | `python3` absent from the Windows runner too |
| 24 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_oversized_member_hits_the_declared_refusal` | `python3` absent from the Windows runner too |
| 25 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_refused_restore_leaves_an_occupied_target_byte_identical` | `python3` absent from the Windows runner too |
| 26 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_secret_in_a_memory_note_never_reaches_a_plan_or_report` | `python3` absent from the Windows runner too |
| 27 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_secret_in_a_persona_body_never_reaches_a_plan_or_report` | `python3` absent from the Windows runner too |
| 28 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_secret_in_a_skill_body_is_contained_and_never_reported` | `python3` absent from the Windows runner too |
| 29 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_traversal_symlink_escape_is_refused_and_nothing_outside_changed` | `python3` absent from the Windows runner too |
| 30 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_truncated_configuration_is_named_not_panicked` | `python3` absent from the Windows runner too |
| 31 | W1 no python3 | YES | `wcore-cli::portability_hostile_corpus hostile_wrong_typed_configuration_is_named_not_panicked` | `python3` absent from the Windows runner too |
| 32 | W10 sandbox/AppContainer | YES | `wcore-agent::typed_execution_policy_e2e_test typed_bypass_executes_bash_inside_required_sandbox` | Windows sandbox path; not investigated in this lane |
| 33 | W10 sandbox/AppContainer | YES | `wcore-cli::sandbox_activeness sandbox_exec_confines_a_write_that_escapes_the_workspace` | Windows sandbox path; not investigated in this lane |
| 34 | W11 unimplemented on platform | no | `wcore-agent engine::audit_2026_05_22_tests::corrupt_filesystem_checkpoint_blocks_restart_without_dispatch_or_mutation` | the test asserts a capability the platform layer says it does not have |
| 35 | W11 unimplemented on platform | no | `wcore-agent engine::audit_2026_05_22_tests::missing_filesystem_checkpoint_blocks_restart_without_dispatch_or_mutation` | the test asserts a capability the platform layer says it does not have |
| 36 | W12 journal guard not enforced on Windows | no | `wcore-agent::session_journal_compaction_test restart_rejects_hard_linked_snapshot` | symlink/hardlink/DACL/oversize snapshot rejections do not fire |
| 37 | W12 journal guard not enforced on Windows | no | `wcore-agent::session_journal_compaction_test restart_rejects_oversize_snapshot` | symlink/hardlink/DACL/oversize snapshot rejections do not fire |
| 38 | W12 journal guard not enforced on Windows | no | `wcore-agent::session_journal_compaction_test restart_rejects_public_snapshot_dacl` | symlink/hardlink/DACL/oversize snapshot rejections do not fire |
| 39 | W12 journal guard not enforced on Windows | no | `wcore-agent::session_journal_compaction_test restart_rejects_symlinked_snapshot` | symlink/hardlink/DACL/oversize snapshot rejections do not fire |
| 40 | W13 traversal refusal | no | `wcore-agent orchestration::d1_refusal_terminal_tests::failed_grep_leaves_turn_committable` | path refusal does not fire on Windows separators |
| 41 | W14 runner contract on Windows | YES | `wcore-eval-scenarios::runner_contracts assertion_failure_still_reaps_owned_descendant_listener` | runner reports Hung/other where the test expects a specific failure |
| 42 | W14 runner contract on Windows | YES | `wcore-eval-scenarios::runner_contracts cancellation_reaps_owned_descendant_listener` | runner reports Hung/other where the test expects a specific failure |
| 43 | W14 runner contract on Windows | YES | `wcore-eval-scenarios::runner_contracts direct_child_early_exit_reaps_owned_descendant_listener` | runner reports Hung/other where the test expects a specific failure |
| 44 | W14 runner contract on Windows | YES | `wcore-eval-scenarios::runner_contracts normal_exit_reaps_owned_descendant_listener` | runner reports Hung/other where the test expects a specific failure |
| 45 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::deterministic_openai_loop packaged_core_blocks_a_denied_write` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 46 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::deterministic_openai_loop packaged_core_calls_a_streamable_http_mcp_tool` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 47 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::deterministic_openai_loop packaged_core_completes_a_scripted_openai_turn` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 48 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::deterministic_openai_loop packaged_core_executes_an_approved_write` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 49 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::deterministic_openai_loop packaged_core_preserves_declared_duplicate_deltas` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 50 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::deterministic_openai_loop packaged_core_recovers_after_a_bounded_429` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 51 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::deterministic_openai_loop packaged_core_recovers_after_a_real_read_timeout` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 52 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::deterministic_openai_loop packaged_core_recovers_after_a_truncated_stream` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 53 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::deterministic_openai_loop packaged_core_recovers_after_two_503_responses` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 54 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::deterministic_openai_loop packaged_core_satisfies_a_hidden_repository_outcome` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 55 | W2 keyring unusable as NETWORK SERVICE | YES | `wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 56 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-cli::smoke_p0 stop_mid_turn_does_not_strand_json_stream_session` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 57 | W2 keyring unusable as NETWORK SERVICE | no | `wcore-config credentials::tests::confidential_auto_uses_encrypted_vault_without_plaintext_fallback` | the runner service account has no usable credential store; the engine treats this as a fatal non-retryable session error |
| 58 | W3 unix-only path literal | no | `wcore-agent child_transaction::gate_executor::tests::fails_closed_at_each_gate_execution_stage` | test hardcodes a POSIX absolute path that is not absolute on Windows |
| 59 | W3 unix-only path literal | no | `wcore-agent journal_effects::tests::filesystem_receipt_is_durable_and_unknown_can_reconcile_not_started` | test hardcodes a POSIX absolute path that is not absolute on Windows |
| 60 | W3 unix-only path literal | no | `wcore-eval-scenarios::openai_fixture_contract workspace_aware_identity_is_root_independent_and_collision_free` | test hardcodes a POSIX absolute path that is not absolute on Windows |
| 61 | W3 unix-only path literal | no | `wcore-eval-scenarios::receipt_contract critical_usability_finding_is_a_receipt_gate_failure` | test hardcodes a POSIX absolute path that is not absolute on Windows |
| 62 | W3 unix-only path literal | no | `wcore-eval-scenarios::receipt_contract filesystem_evidence_is_observed_only_after_capture_completes` | test hardcodes a POSIX absolute path that is not absolute on Windows |
| 63 | W3 unix-only path literal | no | `wcore-eval-scenarios::receipt_contract scenario_receipt_normalizes_only_the_owned_workspace` | test hardcodes a POSIX absolute path that is not absolute on Windows |
| 64 | W3 unix-only path literal | no | `wcore-eval-scenarios::receipt_contract workspace_token_cannot_collide_with_literal_or_sibling_paths` | test hardcodes a POSIX absolute path that is not absolute on Windows |
| 65 | W4 HOME does not isolate | no | `wcore-cli::plugin_discovery_e2e ready_event_advertises_plugin_capabilities_when_backends_can_start` | FIXED IN THIS LANE — needs WAYLAND_HOME; measured |
| 66 | W4 HOME does not isolate | no | `wcore-cli::plugin_discovery_e2e ready_event_withdraws_plugin_capabilities_when_backends_cannot_start` | FIXED IN THIS LANE — needs WAYLAND_HOME; measured |
| 67 | W4 HOME does not isolate | no | `wcore-cli::release_binary_smoke release_binary_ready_event_advertises_plugin_capabilities` | FIXED IN THIS LANE — needs WAYLAND_HOME; measured |
| 68 | W4 HOME does not isolate | no | `wcore-cli::release_binary_smoke release_binary_withdraws_plugin_capabilities_when_backends_cannot_start` | FIXED IN THIS LANE — needs WAYLAND_HOME; measured |
| 69 | W5 service-account temp DACL | no | `wcore-agent session_journal::snapshot::tests::windows_private_dacl_accepts_restrictive_deny_ace` | C:\WINDOWS\ServiceProfiles\NetworkService temp dir denies the ACL the test writes |
| 70 | W5 service-account temp DACL | no | `wcore-agent session_journal::snapshot::tests::windows_private_dacl_rejects_null_empty_and_broad_allow` | C:\WINDOWS\ServiceProfiles\NetworkService temp dir denies the ACL the test writes |
| 71 | W6 unfailable gate | no | `wcore-cli::build_provenance binary_matches_repo_head` | FIXED IN THIS LANE — 40-hex compared against --short HEAD |
| 72 | W7 case preservation | no | `wcore-config shell::executable_readiness::tests::native_windows_command_shell_resolves_cmd_and_bat_from_effective_cwd` | PATHEXT resolution returns the probe casing, not the on-disk casing |
| 73 | W8 platform error class | no | `wcore-egress tests::transport_failure_records_one_stable_error_class` | a closed port times out on Windows where it refuses on Linux |
| 74 | W9 downstream of W2 | no | `wcore-cli::acp_gate_d012 d012_json_stream_force_does_not_gate_write` | turn never ran because the session refused to persist |
| 75 | W9 downstream of W2 | no | `wcore-cli::acp_gate_d012 d012_json_stream_gated_write_emits_approval_before_execution` | turn never ran because the session refused to persist |
| 76 | W9 downstream of W2 | no | `wcore-cli::migrate_quarantine t19_live_negative_leg_quarantined_payload_does_not_execute` | turn never ran because the session refused to persist |
| 77 | W9 downstream of W2 | no | `wcore-cli::migrate_quarantine t20_live_positive_control_same_payload_executes_once_promoted` | turn never ran because the session refused to persist |
| 78 | W9 downstream of W2 | no | `wcore-cli::smoke_p0 approval_mode_auto_edit_from_config_reaches_json_stream_session` | turn never ran because the session refused to persist |
| 79 | W9 downstream of W2 | no | `wcore-cli::smoke_p0 gap_d012_acp_protocol_gates_mutating_tools` | turn never ran because the session refused to persist |
| 80 | W9 downstream of W2 | no | `wcore-cli::smoke_p0 smoke_17_force_posture_auto_approves_mutating_tool_in_engine` | turn never ran because the session refused to persist |
| 81 | W9 downstream of W2 | no | `wcore-tools::bash_sandbox_routing_test delegated_mutation_uses_only_owner_checkout_and_private_scratch` | turn never ran because the session refused to persist |

- W1 no python3: 23
- W2 keyring unusable as NETWORK SERVICE: 13
- W9 downstream of W2: 8
- W3 unix-only path literal: 7
- UNCLASSIFIED: 5
- W12 journal guard not enforced on Windows: 4
- W14 runner contract on Windows: 4
- W4 HOME does not isolate: 4
- K1 KNOWN: 2
- W10 sandbox/AppContainer: 2
- W11 unimplemented on platform: 2
- W5 service-account temp DACL: 2
- R1 REAL DEFECT: 1
- W13 traversal refusal: 1
- W6 unfailable gate: 1
- W7 case preservation: 1
- W8 platform error class: 1
