# wcore-cli::mcp_assistant_scoping_e2e::matching_assistant_dials_scoped_deferred_server (Windows only) — executable_readiness "unchecked" != "not_found"

**Confidence (self-reported):** partial

## Root cause

"unchecked" and "not_found" are not states of the MCP *connection* — they are `executable_readiness`, the non-spawning filesystem probe that answers "could this stdio server's command have been launched at all". The test's `diag` server points at a bogus command, so on Linux/macOS `resolve_mcp_stdio_executable` proves absence and returns `NotFound`, which projects to `"not_found"` plus the actionable `install_executable`/`fix_gui_launch_path`/`restart_desktop` remediation the test then asserts. On the hosted `windows-2025-vs2026` runner the same probe instead returns one of five errors that `wcore-mcp` collapses into `McpStdioExecutableReadinessStatus::Unchecked` (`Io`, `ProbeFailed`, `UncheckedDirectSearch`, `NetworkPathUnsupported`, `EnvironmentLimitExceeded` — stdio_readiness.rs:96-103), and `Unchecked` is the one status that carries no remediation at all. The plumbing is platform-identical (the deferred path records the same evidence on every OS via note_deferred_mcp_connect → record_executable_readiness, and the same manager evidence wins in project_server), so the divergence is inside the resolver, and it is structurally Windows-only: a bare-name lookup on Windows costs (|PATH|+1) × |PATHEXT| metadata probes and walks the real ambient PATH, whereas on Unix |PATHEXT| is empty so the cost is |PATH| and every candidate returns a clean ENOENT. Two of the five Unchecked branches (`UncheckedDirectSearch`, `ProbeTimedOut`→ no, that maps elsewhere) are excluded by construction: `UncheckedDirectSearch` only fires for the cmd/powershell/pwsh Direct strategy, and a timeout maps to its own `probe_timed_out` status. That leaves exactly three live mechanisms: (a) `MAX_EXECUTABLE_CANDIDATE_PROBES = 1_024` being exceeded — a measured real Windows box already needs 792 of that 1024 budget, so ~86+ PATH entries silently kills readiness; (b) a single PATH directory whose metadata call returns something other than NotFound/PermissionDenied, because `stronger_failure` ranks `Io(kind)` above `Missing` and one such candidate poisons the other ~1000 clean "definitely absent" answers; (c) a UNC entry in PATH. I could not read the runner's PATH, so I cannot say which of (a)/(b)/(c) fired. I measured a real Windows box (SeanDesktop): 65 PATH entries, 12-entry PATHEXT, 792 candidate probes — ZERO anomalous metadata errors and ZERO UNC entries across all 792, which is direct evidence against (b) and (c) and shows (a) is the only mechanism that scales with "bigger machine". The GH image is a much larger software install than that box. The patch fixes (a).

## Evidence

- crates/wcore-cli/tests/mcp_assistant_scoping_e2e.rs:250-255 — the panic line is the readiness assertion, not the connection one: `assert_eq!(scoped["connection"], scoped_connection);` PASSES with "failed", then `if scoped_connection == "failed" { assert_eq!(scoped["executable_readiness"], "not_found"); }` fails with left: "unchecked"
- CI run 30867082257, job 91861301225 `CI (windows-latest, hosted)`: TRY 1/2/3 all FAIL identically — deterministic, not a flake. Same log line 9981-9982 shows `absent_assistant_...` and `nonmatching_assistant_...` PASS (they assert the scoped-out branch, where no probe ever runs and "unchecked" is the untouched default from runtime_diagnostics.rs:173-177)
- Same job log lines 1-20: `Image: windows-2025-vs2026`, `Version: 20260728.188.1`, `Microsoft Windows Server 2025`, work dir `D:\a\wayland-core\wayland-core` — this is the GitHub-hosted leg (.github/workflows/ci.yml:639 `runs-on: windows-latest`), not the self-hosted box
- crates/wcore-mcp/src/transport/stdio_readiness.rs:96-103 — the Unchecked bucket: `Err(ExecutableReadinessError::Io { .. } | ProbeFailed { .. } | UncheckedDirectSearch { .. } | NetworkPathUnsupported { .. } | EnvironmentLimitExceeded { .. }) => McpStdioExecutableReadinessStatus::Unchecked`
- crates/wcore-config/src/shell/executable_readiness.rs:128 — `const MAX_EXECUTABLE_CANDIDATE_PROBES: usize = 1_024;` enforced at :758-775 as `base_count * extensions.len().max(1)`; on Windows base_count = |PATH| + 1 (cwd inserted at :360-362) and variants = |PATHEXT| (:625-693). On Unix `executable_extensions` returns an empty Vec at :613-615 so variants == 1 — the ceiling is unreachable there
- crates/wcore-config/src/shell/executable_readiness.rs:813-827 `stronger_failure` — rank(Missing)=0 < rank(Io)=3, so ONE candidate metadata error that is neither NotFound nor PermissionDenied overrides every clean absence proof and turns the whole answer into `Unchecked`
- MEASURED on a real Windows box (ssh SeanD@seandesktop, PowerShell, read-only): PATH_ENTRIES=65, PATHEXT=.COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC;.CPL, PATHEXT_ENTRIES=12, PROBES=792 (77% of the 1024 ceiling); emulating the resolver's candidate loop over all 792 candidates for the same bogus program name: WEIRD_COUNT=0 (no non-NotFound/non-PermissionDenied errors) and no UNC PATH entries
- crates/wcore-mcp/src/manager.rs:1627-1665 — the only test that pins `NotFound` readiness through the manager is `#[cfg(unix)] health_records_failure_for_an_unspawnable_server`, and it uses an absolute path (1 candidate). No Windows test ever exercised bare-name resolution against a real ambient PATH; `wcore-mcp transport::stdio_readiness::tests::explicit_path_source_survives_redaction` PASSED on the runner (log line 12275) but forces PATH="", so it probes only the cwd
- crates/wcore-cli/src/runtime_diagnostics.rs:365-377 and :433-453 — `project_readiness(Unchecked) -> McpExecutableReadiness::Unchecked` serialises as "unchecked"; :455-474 `readiness_failure` and :476-505 `append_readiness_remediation` both give Unchecked nothing, which is why this status is the worst possible answer for an operator
- crates/wcore-protocol/src/contract/spec.rs:1018-1022 — `executable_readiness.rs`, `mcp_stdio_launch_context.rs` and `stdio_readiness.rs` are all contract SOURCE_INPUTS, so ANY edit to them requires a contract-corpus regeneration or the contract-drift gate goes red

## How to verify

WHAT I ACTUALLY VERIFIED (no cargo was run anywhere): the patch applies cleanly to integration head 7accc0c1 — `git apply --check --verbose` over the three hunks returns clean; and `rustfmt --edition 2024 --check` is clean on all three patched files (and on their unpatched baselines, so the check is meaningful). Everything below is UNRUN.

1. New platform-independent gate, runnable on Linux (hetzner) — this is the cheap one and it does not need Windows:
   `cargo nextest run -p wcore-config shell::executable_readiness::tests::windows_readiness_survives_an_ordinary_sized_path_and_pathext`
   Observable: PASS with the patch. Revert only the constant to `1_024` and it FAILS with `EnvironmentLimitExceeded { limit: CandidateProbes }`.
   Also re-run the amended bound test, which must still PASS and must still be reachable:
   `cargo nextest run -p wcore-config shell::executable_readiness::tests::readiness_bounds_path_pathext_and_candidate_probes_before_io`

2. The actual defect — only a hosted `windows-latest` run answers it, because the trigger is that image's PATH:
   `vx just test-ci` on the `CI (windows-latest, hosted)` leg, or locally on a Windows box `cargo nextest run -p wcore-cli --test mcp_assistant_scoping_e2e`.
   Observable that distinguishes fixed from not: `executable_readiness` for server `diag` is `"not_found"` and the remediation array contains `install_executable`, `fix_gui_launch_path`, `restart_desktop`. If it is STILL `"unchecked"`, the cause was mechanism (b) or (c), not the probe ceiling — and the `tracing::warn!` added by hunk 3 now names the exact error variant (`target: mcp.readiness`, field `evidence`), which is the measurement that settles it. To see it, the child's stderr must be surfaced: run the binary by hand (`wayland-core --json-stream --provider anthropic --assistant concierge` with the test's config.toml) rather than through the test, which pipes and discards child stderr.

3. MANDATORY follow-up before this can be merged: `executable_readiness.rs` and `stdio_readiness.rs` are both in `SOURCE_INPUTS` (crates/wcore-protocol/src/contract/spec.rs:1018,1022), so the contract corpus must be regenerated (`wcore-contract` regenerate + key-diff) or the contract-drift gate goes red on Linux.

4. THE MEASUREMENT THAT WOULD HAVE SETTLED THE ROOT CAUSE OUTRIGHT, and still should be taken: add one throwaway step to the hosted Windows job — `pwsh -c "$p=($env:PATH -split ';'|? {$_}); $e=($env:PATHEXT -split ';'|? {$_}); \"$($p.Count) $($e.Count) $((($p.Count+1)*$e.Count))\""`. If the third number exceeds 1024, mechanism (a) is confirmed and this patch is the whole fix.

## Mutant

Two independent mutants, both of which must turn the new gate RED:
(1) Set `MAX_EXECUTABLE_CANDIDATE_PROBES` back to `1_024` in crates/wcore-config/src/shell/executable_readiness.rs — `windows_readiness_survives_an_ordinary_sized_path_and_pathext` then fails with `EnvironmentLimitExceeded { limit: CandidateProbes }` instead of `NotFound` (96 PATH entries + cwd = 97, times the stock 11-entry PATHEXT = 1_067 > 1_024). Any ceiling below 1_067 fails it; 1_067 or above passes it, which is exactly the boundary the defect sits on.
(2) Set the ceiling absurdly high (e.g. `usize::MAX`) — the AMENDED existing test `readiness_bounds_path_pathext_and_candidate_probes_before_io` then fails, first on its new `probe_directories <= MAX_EFFECTIVE_PATH_ENTRIES` assertion, proving the CandidateProbes guard has not been quietly made unreachable. Without mutant (2) the fix could have been "raise the limit until it can never fire", which is the permanently-green twin of a permanently-red gate.

## Unknowns

- WHICH of the three surviving Unchecked branches actually fires on the hosted windows-2025-vs2026 image. I could not read that runner's PATH — GHA logs do not print it, the runner-images README does not document it, and I will not push a workflow change. My patch fixes only mechanism (a), the candidate-probe ceiling. If the real cause is (b) — one PATH directory returning a metadata error that is neither NotFound nor PermissionDenied, which `stronger_failure` ranks above `Missing` and lets poison ~1000 clean answers — the CI test stays red and the follow-up is a decision about whether `Io` should outrank `Missing` at all (I deliberately did NOT change that: 'some candidates were indeterminate so we cannot prove absence' is a defensible fail-closed semantic, and weakening it silently would be worse than the bug).
- The exact PATH-entry and PATHEXT-entry counts on the runner, hence whether 1_024 was exceeded by a lot or by a little. My 96-entry / 11-extension model in the new test is an estimate anchored on a MEASURED real box (65 / 12 = 792), not on the runner.
- Whether the `tracing::warn!` I added actually reaches the child's stderr under `--json-stream` with the CLI's default subscriber. I did not verify the subscriber configuration, and the e2e test pipes and discards child stderr, so in CI it may be invisible — it is useful for a hand-driven repro, not automatically for the next CI run.
- Nothing was compiled. `git apply --check` and `rustfmt --check` both pass, but no `cargo check`/`clippy`/test was run on any host, per the task constraints. The `matches!(outcome.as_ref().err(), Some(...))` form and the `tracing::warn!` field syntax are unverified against the compiler and against the repo's deny-warnings clippy gate.
- SHARED-ROOT-CAUSE QUESTION, answered NO with evidence: the confirmed defect in .planning/HANDOFF-2026-08-04-rc-and-tvcontrol.md section 3 (tvcontrol connects, 101 tools advertised, none callable) lives in the ToolSearch hydration path — `record_hydrated_tools` -> `hydrated_tool_names` -> force-admit into `tools[]` (engine.rs:12266/15327/15448, tool_search.rs:87). It concerns whether a SUCCESSFULLY connected server's tools reach the model. This defect concerns the readiness PROBE of a server that never connects, and its projection into runtime diagnostics. Different files, different mechanism, no shared cause. They are two defects.
- Whether the deferred-path declaration origin is `EffectiveConfig` (so `record_executable_readiness` actually lands) — I did not confirm it, but it does not affect the diagnosis: `project_server` prefers the manager's own readiness evidence, which is populated for every stdio connect on every platform, so the recorded-declaration path cannot be the platform difference.

## Proposed patch (NOT APPLIED, NOT COMPILED)

```diff
--- a/crates/wcore-config/src/shell/executable_readiness.rs
+++ b/crates/wcore-config/src/shell/executable_readiness.rs
@@ -125,7 +125,27 @@
 const MAX_EFFECTIVE_PATH_ENTRIES: usize = 256;
 const MAX_EFFECTIVE_PATHEXT_LENGTH: usize = 4_096;
 const MAX_EFFECTIVE_PATHEXT_ENTRIES: usize = 64;
-const MAX_EXECUTABLE_CANDIDATE_PROBES: usize = 1_024;
+/// Upper bound on `<candidate directory> x <PATHEXT variant>` metadata probes.
+///
+/// A bare-name Windows lookup costs `(|PATH| + 1) * |PATHEXT|` probes. That is
+/// the launcher's own arithmetic, not ours, and it is large on ordinary
+/// machines: a measured developer box (65 PATH entries, 12-entry PATHEXT)
+/// already needs 792. At the previous ceiling of 1_024 that left ~23% headroom,
+/// and any machine with roughly 86 or more PATH entries fell off the edge.
+/// `EnvironmentLimitExceeded` is projected all the way out to the `unchecked`
+/// MCP readiness status -- the ONE status that carries no install/repair
+/// remediation -- so the operator of a big-PATH Windows box was told nothing at
+/// all about a genuinely missing MCP executable. Unix never reached the ceiling
+/// (no PATHEXT, so probes == |PATH|), which is why the degradation was
+/// Windows-only.
+///
+/// The bound still exists to keep the off-thread probe comfortably inside
+/// [`EXECUTABLE_RESOLUTION_TIMEOUT`]; 8_192 metadata calls on non-existent
+/// paths are tens of milliseconds. It also stays REACHABLE within the
+/// PATH/PATHEXT entry caps (`(129 + 1) * 64 = 8_320`), so the guard can still
+/// fire and its test can still fail -- a limit that cannot trip is dead code,
+/// not a limit.
+const MAX_EXECUTABLE_CANDIDATE_PROBES: usize = 8_192;
 const EXECUTABLE_RESOLUTION_TIMEOUT: Duration = Duration::from_secs(1);
 static EXECUTABLE_RESOLUTION_PERMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
 
--- a/crates/wcore-config/src/shell/executable_readiness_tests.rs
+++ b/crates/wcore-config/src/shell/executable_readiness_tests.rs
@@ -455,7 +455,15 @@
         }
     ));
 
-    let probe_path = (0..16)
+    // Derived from the constants, never hand-counted: the candidate-probe guard
+    // must stay reachable inside the PATH/PATHEXT entry caps even when the
+    // probe ceiling moves, otherwise this assertion silently becomes vacuous.
+    let probe_directories = MAX_EXECUTABLE_CANDIDATE_PROBES / MAX_EFFECTIVE_PATHEXT_ENTRIES + 1;
+    assert!(
+        probe_directories <= MAX_EFFECTIVE_PATH_ENTRIES,
+        "the candidate-probe guard must be reachable before the PATH-entry guard fires"
+    );
+    let probe_path = (0..probe_directories)
         .map(|index| format!("p{index}"))
         .collect::<Vec<_>>()
         .join(";");
@@ -477,6 +485,60 @@
             ..
         }
     ));
+}
+
+/// An ORDINARY Windows launch environment must still produce an ACTIONABLE
+/// readiness answer for a program that is genuinely absent.
+///
+/// A bare-name lookup costs `(|PATH| + 1) * |PATHEXT|` metadata probes. With
+/// the probe ceiling at 1_024, an ordinary machine -- 96 PATH entries and the
+/// stock 11-entry Windows PATHEXT is 1_067 -- exceeded it, and the resolver
+/// answered `EnvironmentLimitExceeded`. `wcore-mcp` maps that to the
+/// `Unchecked` readiness status, and `wcore-cli`'s runtime diagnostics then
+/// report `executable_readiness: "unchecked"` with none of the
+/// `install_executable` / `fix_gui_launch_path` / `restart_desktop`
+/// remediation an operator needs. That is what
+/// `wcore-cli::mcp_assistant_scoping_e2e::matching_assistant_dials_scoped_deferred_server`
+/// observed on the hosted `windows-2025` CI image.
+///
+/// The modelled environment is deliberately unremarkable: every entry is a
+/// well-formed absolute directory that simply does not exist, so the only
+/// thing asserted is that SIZE ALONE does not destroy the answer.
+///
+/// HOW THIS FAILS IF THE BUG RETURNS: lower `MAX_EXECUTABLE_CANDIDATE_PROBES`
+/// back under 1_067 and this test fails with
+/// `EnvironmentLimitExceeded { limit: CandidateProbes }` instead of
+/// `NotFound`. It is platform-independent -- `resolve_for_windows` simulates
+/// Windows resolution semantics -- so it guards from Linux and macOS CI too.
+#[test]
+fn windows_readiness_survives_an_ordinary_sized_path_and_pathext() {
+    let temp = tempfile::tempdir().unwrap();
+    let search_path = (0..96)
+        .map(|index| {
+            temp.path()
+                .join(format!("absent-tool-dir-{index}"))
+                .to_string_lossy()
+                .into_owned()
+        })
+        .collect::<Vec<_>>()
+        .join(";");
+    // The stock Windows Server PATHEXT, verbatim.
+    let stock_pathext = ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC";
+
+    let error = resolve_for_windows(
+        OsStr::new("wayland_mcp_scoping_probe_absent_cmd"),
+        temp.path(),
+        Some(OsStr::new(&search_path)),
+        Some(OsStr::new(stock_pathext)),
+    )
+    .unwrap_err();
+
+    assert!(
+        matches!(error, ExecutableReadinessError::NotFound { .. }),
+        "an absent executable in an ordinary-sized Windows launch environment \
+         must report NotFound so the operator gets install/repair remediation; \
+         got {error:?}"
+    );
 }
 
 #[test]
--- a/crates/wcore-mcp/src/transport/stdio_readiness.rs
+++ b/crates/wcore-mcp/src/transport/stdio_readiness.rs
@@ -70,7 +70,28 @@
 ) -> McpStdioExecutableReadiness {
     let path_source = context.path_source();
     let pathext_source = context.pathext_source();
-    let status = match context.resolve_executable(command.as_ref()).await {
+    let outcome = context.resolve_executable(command.as_ref()).await;
+    // `Unchecked` is the one readiness status that produces NO remediation, so
+    // an operator who lands on it is told nothing at all. Name the branch that
+    // caused it. The error values are documented to retain no PATH/PATHEXT
+    // contents and no resolved directory, so Debug is secret-free.
+    if matches!(
+        outcome.as_ref().err(),
+        Some(
+            ExecutableReadinessError::Io { .. }
+                | ExecutableReadinessError::ProbeFailed { .. }
+                | ExecutableReadinessError::UncheckedDirectSearch { .. }
+                | ExecutableReadinessError::NetworkPathUnsupported { .. }
+                | ExecutableReadinessError::EnvironmentLimitExceeded { .. }
+        )
+    ) {
+        tracing::warn!(
+            target: "mcp.readiness",
+            evidence = ?outcome.as_ref().err(),
+            "MCP stdio executable readiness is unchecked; no install or repair remediation can be offered for this server"
+        );
+    }
+    let status = match outcome {
         Ok(_) => McpStdioExecutableReadinessStatus::Resolved,
         Err(ExecutableReadinessError::InvalidExecutable { .. }) => {
             McpStdioExecutableReadinessStatus::InvalidExecutable

```
