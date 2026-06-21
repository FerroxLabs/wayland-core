# OS-Sandbox Secret-Read-Deny — SDD progress (separate from spine ledger)

Plan: docs/superpowers/plans/2026-06-21-os-secret-read-deny.md (v3, 10 tasks).
Base: feat/workspace-policy HEAD e90d638a. Branch: feat/os-secret-deny.
PR placement (stacked vs extend #59) DEFERRED to Sean — DO NOT push, DO NOT touch PR.

- Task 1: b3e65085 — DONE — fs_read_deny field (#[serde(default)]) + SandboxBackend::enforces_read_deny() (default false) + 5 tests (37 pass, 0 warnings)
- Task 2: 7e446c4a — DONE — macOS SBPL read-deny after allows (last-match-wins) + enforces_read_deny()=true + 4 tests incl. live denial (41 pass, 0 code warnings)
- Task 3: cb7ba75b — DONE — Linux bwrap stat-at-bind overlay deny (/dev/null for files, --tmpfs for dirs) + fs_read_deny path validation + enforces_read_deny()=true + live test (bwrap-gated, Linux-only) (41 pass, 8 ignored, 0 warnings)
- Task 4: 76b841aa — DONE — Windows AppContainer DENY ACE (DENY_ACCESS import + deny_appcontainer_dacl) + DaclGrantGuard.deny_paths + guard-condition fix + enforces_read_deny()=true (windows only) + live test; cross-target clippy clean (41 pass, 8 ignored, 0 warnings)
- Task 5: c8964037 — DONE — Docker /dev/null deny mounts (files) + empty-dir bind (dirs) + duplicate-bind skip + enforces_read_deny()=true (#[cfg(live-docker)]) + 2 unit tests + 1 live integration test (42 pass, 8 ignored, 0 code warnings)
- Task 6: 02600510 — DONE — secret_deny_paths() cached+readable-scoped+mode-aware; CREDENTIAL_STORES+SYSTEM_CREDENTIAL_STORES; is_secret_path_static(); ignore walker; symlink→target deny; 4 new tests (12 pass, 0 code warnings)
- Task 7: 29ce9e92 — DONE — build_sandbox_pieces populates manifest.fs_read_deny from secret_deny_paths(); 3 new tests (Contained/Trusted/None paths); 1057 pass, 0 warnings
- Task 8: 8912d02e — DONE — exec-time gate in bash.rs (both ctx paths, TOCTOU-free); bootstrap UX gate via platform_enforces_read_deny(); Bash added to WORKSPACE_FS_TOOLS (gated on read_deny_enforced); 9 new tests (3 exec-time refusal + 6 channel_tools); 12 bash_routing + 11 channel_tools pass, 0 code warnings
- Task 9: d73c8f92 — DONE — adversarial matrix (5 cases a-e in secret_read_deny.rs) + Workspace Bash gating assertions (3 cases in workspace_bash_deny.rs); 8 new tests pass, 0 clippy warnings
- Task 9 (reviewer fix): 3dbdda74 — DONE — drop needless_return in both cfg arms of host_backend(); redesign case (c) as behavioural no-over-deny proof (non-secret README.md neighbour readable with content asserted, replaces trivially-true static deny-list check); 5/5 tests pass, 0 clippy lint warnings
- Task 9 (reviewer fix 2): 36cf0cfa — DONE — replace case (c) plain-neighbour (README.md) with a symlink crossing the primary allowed root boundary to an external non-secret file in a second tempdir; both fs_read_allow entries added so both macOS SBPL and Linux bwrap can resolve the symlink target; test renamed secret_read_deny_case_c_symlink_to_external_non_secret_is_readable; 5/5 pass, 0 clippy warnings
- Task 10: 13cdda7c — DONE — E2E verified: cargo nextest run --workspace (4168 pass, only 3 pre-existing exclusions fail/timeout); clippy --workspace --all-targets zero lint warnings; fmt --all --check clean; cross-target clippy --target x86_64-pc-windows-msvc -p wcore-sandbox clean; docs/architecture.md §WorkspacePolicy updated (OS deny implemented, residuals documented, capability gate documented, MCP deferred); fixed channel_tool_posture_test::workspace_posture_jails_filesystem_reads to match post-Task-8 Bash-in-Workspace contract
