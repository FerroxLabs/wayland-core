# Codebase Concerns

**Analysis Date:** 2026-07-23

## Critical — Uncommitted Working-Tree Regression (Security)

**AppContainer sandbox privilege/token restrictions are currently disabled in the working tree:**
- Files: `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs`
- Evidence (uncommitted `git diff` on this worktree, not yet committed):
  - Line ~369: `DISABLE_MAX_PRIVILEGE` (the flag that strips the child token down to minimal privilege) was replaced with `0` behind the comment `// DIAGNOSTIC: preserve privileges (incl. SeChangeNotifyPrivilege/bypass-traverse)`.
  - Line ~744: `CreateProcessAsUserW`'s token argument was changed from `restricted_token.as_raw()` to `current_token.as_raw()` behind `// DIAGNOSTIC: was restricted_token — testing AAP-loss-from-restricted-token`.
  - A large block of `WCORE_DIAG_TOKEN`-gated diagnostic code was added that calls `OpenProcessToken`/`GetTokenInformation` to dump the child token's groups, privileges, and `TokenIsAppContainer` status via `eprintln!`.
- Impact: if this diff is committed as-is, the Windows AppContainer sandbox backend spawns child processes with the **caller's full, unrestricted token** instead of the deny-only-SID restricted AppContainer token, and skips privilege stripping. This is a full sandbox bypass on Windows, not a cosmetic diagnostic — it must be reverted (restore `restricted_token`/`DISABLE_MAX_PRIVILEGE`) before merge, and the `WCORE_DIAG_TOKEN` eprintln block should be removed or gated out of release builds.
- Fix approach: revert both DIAGNOSTIC swaps to their real values; either delete the token-dump block or wrap it in `#[cfg(debug_assertions)]` and confirm it never activates without the env var in production.

**Storage `.write(true)` gap — fix already staged, unverified live:**
- File: `crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs` (~line 412-419, uncommitted diff)
- The known defect (`OpenOptions` for `create_new` was missing `.write(true)`, causing "creating or truncating a file requires write or append access" on real Windows) has a fix staged in the dirty tree, but this fix has never been proven on the actual target OS — see "Aspirational Native Machinery" below. Confirm on a real Windows host before treating this as closed.

## Known Defects — Windows AppContainer Native Machinery (never run on target OS)

The Phase-20 Windows/macOS native-proof stack (`crates/wcore-sandbox/src/backends/appcontainer/`, `scripts/f20-native-*`) was authored without ever executing on Windows or macOS. The following concrete defects are confirmed by code inspection:

**`CreateRestrictedToken` deny-only SIDs likely break the AppContainer grant path:**
- File: `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs`
- The presence of the DIAGNOSTIC swap above (testing "AAP-loss-from-restricted-token") is itself direct evidence that the restricted-token path was found not to work as intended when someone finally exercised it — the deny-only SID list passed to `CreateRestrictedToken` appears to strip the AppContainer/package identity itself (or otherwise breaks the grant), forcing whoever wrote the diagnostic to fall back to `current_token` to isolate the cause. This is unresolved: the diagnostic never lands on a working restricted-token configuration in this diff.
- Fix approach: once real Windows access is available, use the `WCORE_DIAG_TOKEN` dump added in the same diff to compare `TokenIsAppContainer`/group SIDs between the restricted and current-token paths, identify which deny-only SID (or missing default DACL/token group) removes AppContainer-ness, and adjust the SID list rather than bypassing the restriction.

**Wrong `windows-sys` module for `READ_CONTROL`/`WRITE_DAC`:**
- File: `crates/wcore-agent/src/session_journal/snapshot.rs:654,661`
- `use windows_sys::Win32::Security::{READ_CONTROL, WRITE_DAC};` then `.access_mode(READ_CONTROL | WRITE_DAC)` — these two constants are generic access-right bits defined under `windows_sys::Win32::Storage::FileSystem` (or `Win32::System::SystemServices` depending on the windows-sys version), not `Win32::Security`. This either fails to compile on Windows or, if it happens to resolve via a re-export, is fragile and untested — nobody has compiled this path on the target OS. Verify the correct module and add a `#[cfg(windows)]`-gated compile check to CI.

**`live_fs_acl.rs` — `choice.exe` exit-code assumption is wrong:**
- File: `crates/wcore-sandbox/tests/live_fs_acl.rs`
- `type_and_hold()` (line ~118) builds a `cmd.exe` script `type "<file>" & %SystemRoot%\System32\choice.exe /T {seconds} /D Y >nul`. `choice.exe` with `/D Y` returns the **1-based index of the selected/default choice** as its exit code (1 for the first option "Y"), not `0`. Tests such as `one_execution_grant_never_leaks_to_another_identity` (line ~347-350: `a.await...expect("execution A").exit_code, 0`) and `concurrent_allow_and_deny_identities_do_not_interfere` (lines ~296, ~303) assert `exit_code == 0` after a `type_and_hold` invocation — this will spuriously fail on real Windows even when sandbox behavior is correct, because the test itself measures the wrong terminal command's exit code.
- Fix approach: either append `& exit /b 0` after the `choice.exe` call in the script, or change the assertions to accept exit code `1` for the hold commands (and keep `0` only for `type_file`, which doesn't hold).

**`wcore-swarm/tests/dispatch_smoke.rs` — non-portable `fs::rename` across filesystems:**
- File: `crates/wcore-swarm/tests/dispatch_smoke.rs:289,304`
- `std::fs::rename(&repo, tmp.path().join("original-repo")).unwrap()` (and the symmetric rename back at line 304) assumes source and destination are on the same filesystem/volume. `tempfile::TempDir` and the original `repo` path may not share a filesystem on all CI runners (notably cross-volume on Windows, or when `/tmp` is a separate mount from the workspace on some Linux runners), where `rename` returns `EXDEV` and the `.unwrap()` panics. This has not been exercised on Windows/macOS runners.
- Fix approach: use `fs_extra::dir::move_dir` or copy+remove as a cross-device-safe fallback, or explicitly constrain the temp dir to the same volume as `repo`.

**`hard_process_containment.rs` — two "Windows" proof targets are wired to Linux-only Bubblewrap:**
- File: `crates/wcore-sandbox/tests/hard_process_containment.rs`
- Despite the doc comment describing this as a general "06D black-box proof," the only live test, `qualified_hard_containment_backend_preflight` (line ~92), is gated `#[cfg_attr(not(target_os = "linux"), ignore = "hard containment (bwrap) is Linux-only")]` and directly instantiates `wcore_sandbox::backends::bwrap::BubblewrapBackend` — there is no Windows or macOS equivalent hard-containment backend or test in this file. Given the phase-20 framing described in this task ("two 'Windows' proof targets... wired to Linux-only Bubblewrap tests"), any test in this file or its sibling that is documented/labeled as validating Windows containment but instantiates `BubblewrapBackend` is effectively a no-op on Windows (always `#[ignore]`d) and gives false confidence that Windows hard-containment has proof coverage. No actual Windows hard-containment backend (job-object/AppContainer equivalent to `owns_descendants_hard`) is exercised by this file.
- Fix approach: either add a genuine Windows Job Object hard-containment backend test parallel to the bwrap one, or rename/re-scope this file's documentation so it's not implied to cover Windows.

## Aspirational Native Machinery (authored, never proven on target OS)

- `crates/wcore-sandbox/src/backends/appcontainer/` — entire AppContainer backend (`windows_impl/{process,command,handles}.rs`, `acl_lease/{storage,acl_lease,mutation_lock,sha256}.rs`) was written and reviewed without ever compiling/running on a real Windows machine. The two live-fix diffs currently sitting uncommitted in this worktree (`.write(true)` in `storage.rs`, and the DISABLE_MAX_PRIVILEGE/restricted_token diagnostics in `process.rs`) are the first evidence that this code has actually been run at all, and that first run surfaced the restricted-token defect that is still unresolved (see above).
- `scripts/f20-native-macos-proof.sh`, `scripts/f20-native-windows-proof.ps1`, `scripts/f20-native-uat-proof.mjs`, `scripts/f20-native-uat-proof.test.mjs` — proof harnesses for the native sandbox paths; treat any prior "PASS" claim from these scripts as unverified until re-run on Hetzner/native Windows/macOS hardware with the diagnostic reverts above applied.
- Practical implication: do not treat any `#[ignore = "explicit native Windows AppContainer acceptance"]` test in `crates/wcore-sandbox/tests/live_fs_acl.rs` as a real acceptance gate until it has actually executed (not just compiled) on Windows with the current-token diagnostic reverted.

## Tech Debt

**Unbounded engine pool / no session persistence in channel dispatch:**
- File: `crates/wcore-agent/src/channel_dispatch.rs:30-42`
- Four stacked `TODO(phase)` comments document: (1) the engine pool is unbounded with no LRU/idle eviction, (2) every new session re-runs the full `AgentBootstrap` (no bootstrap caching), (3) channel history is in-memory only and lost on process restart, (4) per-session engines retain boot-default config even if config changes mid-run.
- Impact: long-running channel-connected deployments (Discord/Matrix/Signal/etc.) will leak memory over many sessions and lose conversation history on any restart.
- Fix approach: add capacity-bounded LRU eviction to the engine pool, cache bootstrap artifacts keyed by config hash, and persist channel history to the existing session-journal store.

**Deferred hook-driven compaction hint:**
- File: `crates/wcore-agent/src/hooks/mod.rs:275,765`
- `TODO(C1): PreCompact contribution → compaction hint (deferred)` — the `run_pre_compact` hook path does not yet feed its output into the actual compaction decision, so `PreCompact` hooks currently cannot influence what gets compacted.

**Deferred hot-swap of skill/config reload channel:**
- File: `crates/wcore-agent/src/bootstrap.rs:2847,2859`
- `TODO(W3-B-follow-on): thread reload_tx into ...` — in-session hot-swap of reloaded resources is explicitly not wired up yet; the surrounding comment documents this as a known gap rather than a silent omission.

**Stub session id in stage-3 code path:**
- File: `crates/wcore-agent/src/bootstrap.rs:2220`
- `TODO(stage3): use the live per-conversation session id.` — a placeholder session id is used instead of the real per-conversation one.

**HTTP-error-class tests pending across three providers:**
- Files: `crates/wcore-providers/src/vertex.rs:398`, `crates/wcore-providers/src/cohere.rs:150`, `crates/wcore-providers/src/azure_openai.rs:284` (see also doc comment at `azure_openai.rs:13`)
- All three carry `TODO(http-error-class): wiremock tests pending for <provider> HTTP error` — the structured HTTP-error-class mapping for these three providers lacks wiremock-backed test coverage, unlike (presumably) the providers that already have it. Risk: provider-specific error responses (rate limits, auth failures, malformed request errors) can silently mis-classify without a regression test catching it.

**Per-model thinking-capability table needs a pricing audit refresh:**
- File: `crates/wcore-config/src/compat.rs:366`
- `TODO(pricing-audit-2026-05-24): per-model thinking capability table —` (comment truncated in source) flags this table as due for a refresh tied to a pricing audit dated 2026-05-24, which is in the past relative to this analysis date (2026-07-23) — likely stale.

**Deferred `wcore-protocol` app-side gap (F-005):**
- File: `crates/wcore-protocol/src/commands.rs:231`
- `**F-005 (CRIT app-side gap — TODO Cluster L):**` — flagged as a CRITICAL-severity gap on the host/app integration side, not the engine side; the comment states the engine handles its part correctly but the app-side contract is incomplete. Cross-check with the JSON-stream-protocol consumer (Wayland Desktop) before relying on this command.

## Fragile Areas — Oversized Modules

Files exceeding ~4000 lines are all single Rust modules (not split into submodules despite AGENTS.md's "keep files under 1000 lines" rule), making them hard to review safely and prone to merge conflicts:

| File | Lines |
|---|---|
| `crates/wcore-agent/src/engine.rs` | 28,546 |
| `crates/wcore-cli/src/tui/surfaces/workspace.rs` | 8,586 |
| `crates/wcore-config/src/config.rs` | 8,189 |
| `crates/wcore-cli/src/main.rs` | 7,640 |
| `crates/wcore-cli/src/tui/surfaces/mod.rs` | 7,122 |
| `crates/wcore-providers/src/openai.rs` | 5,649 |
| `crates/wcore-cli/src/tui/surfaces/config.rs` | 5,298 |
| `crates/wcore-agent/src/spawner.rs` | 4,935 |
| `crates/wcore-cli/src/tui/protocol_bridge.rs` | 4,670 |
| `crates/wcore-cli/src/tui/engine_bridge.rs` | 4,643 |
| `crates/wcore-agent/src/orchestration/mod.rs` | 4,554 |
| `crates/wcore-agent/src/session_journal/reducer.rs` | 4,313 |
| `crates/wcore-agent/src/bootstrap.rs` | 4,154 |
| `crates/wcore-agent/src/session_journal.rs` | 4,067 |

`crates/wcore-agent/src/engine.rs` at 28.5k lines is by far the largest single file in the workspace and the highest-risk target for accidental behavior change during any edit — treat any diff to this file as high-review-priority regardless of line count changed.

**Fix approach:** extract cohesive sub-responsibilities out of `engine.rs`, `config.rs`, and the `tui/surfaces/*` files into submodules per AGENTS.md's own file-organization rule; prioritize `engine.rs` first given its size and central role.

## Test Coverage Gaps

**High raw `.unwrap()` count in non-test source:**
- Grep of `crates/*/src/**/*.rs` for `.unwrap()` (excluding `tests/` directories, but including inline `#[cfg(test)]` blocks which inflate the count) returns ~7,159 occurrences workspace-wide. AGENTS.md explicitly forbids `unwrap()` in production code "unless the invariant is proven and commented." A full audit of which of these 7,159 sites are genuinely inside `#[cfg(test)]` blocks vs. production logic was not performed here — this number should be narrowed (e.g. via `cargo clippy -- -W clippy::unwrap_used` scoped to non-test code) before treating it as a hard defect count, but it is large enough to warrant a dedicated audit pass.
- Fix approach: run `cargo clippy --workspace --all-targets -- -W clippy::unwrap_used -W clippy::expect_used` and triage the non-test hits; add `// SAFETY:`-style justification comments to the legitimate ones per AGENTS.md's own rule, replace the rest with proper error propagation.

**Windows AppContainer acceptance tests never actually run in CI:**
- File: `crates/wcore-sandbox/tests/live_fs_acl.rs` — every test in this file is `#[ignore = "explicit native Windows AppContainer acceptance"]`, meaning `cargo test`/`cargo nextest run` never executes them by default anywhere, including CI, unless something explicitly passes `--ignored` on a native Windows runner. Combined with the unresolved restricted-token defect above, there is currently no automated signal that the AppContainer backend actually sandboxes anything correctly.
- Risk: High — this is the security boundary for Windows sandboxed execution.
- Priority: High — should be wired into a native-Windows CI job (or at minimum a documented manual native-proof gate) before Phase-20 Windows support is claimed as complete anywhere in release notes or docs.

---

*Concerns audit: 2026-07-23*
