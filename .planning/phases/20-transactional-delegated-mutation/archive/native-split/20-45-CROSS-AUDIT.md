# 20-45 Pre-native FOUR-WAY cross-audit — sealed third-repaired-successor `3f839309`

Gate: pre-native attestation before the scarce Sean-gated native run (20-46) is re-spent.
Panel: **Codex 5.6 Sol + Gemini 3.1 Pro + Kimi K3** (three external CLIs, run in parallel)
**+ internal Claude non-author adversarial reviewer** (prompted to REFUTE, default-refuted-if-uncertain).

- **sealed source_sha:** `3f839309574d6741eed416cd3820f56447f74eba`
- **sealed source_tree:** `3092475bb4102d010b6ff5f6c9d8080cb4f51928` (verified `3f839309^{tree}` == this)
- **review base tuple:** `source_sha` over `20-44-SUMMARY.md`; delta prosecuted = 20-43 repair over `daf27337`
- **branch:** `plan/f20-unified-audit-repair` (isolated checkout `/Users/seandonahoe/dev/waylandcore-ferrox`; `.git` is a directory; all git ops via `/usr/bin/git`)
- **deferred (only):** `native_macos`, `native_windows` (proven at 20-46)
- **raw outputs preserved:** `.planning/phases/20-transactional-delegated-mutation/20-45-raw/{codex-sol,gemini-pro,kimi-k3}.raw.txt` + shared `audit-context.shared.txt`

## External CLI invocations (verbatim)

| Auditor | Invocation | Exit | Reachable | Schema-valid |
|---|---|---|---|---|
| Codex 5.6 Sol | `codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "<prompt>"` (brief on stdin) | 0 | YES | YES |
| Gemini 3.1 Pro | `gemini --skip-trust -m gemini-3.1-pro-preview -o text -p "<brief>"` (`GEMINI_CLI_TRUST_WORKSPACE=true`) | 0 | YES | YES |
| Kimi K3 | `/Users/seandonahoe/.kimi-code/bin/kimi -p "<brief>" --output-format text` (absolute path) | 0 | YES | YES |

All three external auditors were reachable and returned schema-validatable JSON, so the fail-closed
"external auditor unreachable/invalid" path was NOT triggered. This is a **finding-based BLOCK**, not an
incomplete-panel BLOCK.

## Per-auditor disposition

| # | Auditor id | Disposition | blocker/critical/high/medium/low | Guardrail A (admission budget) | Guardrail B (quoting-layer only) | Kimi watch-items carried |
|---|---|---|---|---|---|---|
| 1 | Codex 5.6 Sol (`f20-native-crossaudit.codex-sol`) | **BLOCK** | 0/0/1/2/0 | PASS | FAIL (ends_with imprecision) | PASS |
| 2 | Gemini 3.1 Pro (`f20-native-crossaudit.gemini-pro`) | PASS | 0/0/0/0/0 | PASS | PASS | PASS |
| 3 | Kimi K3 (`f20-native-crossaudit.kimi-k3`) | PASS | 0/0/0/0/0 | PASS | PASS | PASS |
| 4 | Claude adversarial (`wayland-core.phase20-independent-review.v1`) | **BLOCK** | 0/0/0/1/1 | PASS | PASS (isolation) / imprecise (classifier) | PASS |

## GATE DISPOSITION: **BLOCK — does NOT admit 20-46**

Two of four auditors (Codex 5.6 Sol + internal Claude adversarial) raise **real, non-deferred findings**.
Under the plan's ALL-FOUR-MUST-PASS rule, any single non-deferred finding from any one auditor stops the
sequence and routes to a **further repaired successor**. Gemini and Kimi returned zero-finding PASS but both
**missed** the post-close reap-assertion degradation — the exact class of shared blind spot a four-independent
panel exists to catch ("one auditor's miss is caught by the others").

**The findings are NOT rationalized away.** The blocking defect is a statically-provable code fact (not a
Windows-runtime question, not a hallucination):

1. **[MEDIUM — real, impact-mitigated] `active_process_cap_is_enforced` post-close reap assertion is vacuous.**
   `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` — the fan-out idlers are **bare** `choice.exe`
   (choice rejects an injected tag), so `tagged_choice_descendant_count(&tag)` scopes them by the tagged parent
   cmd's `ParentProcessId`. By the time the post-close check runs, `run.await` has returned → the AppContainer job
   closed → the tagged parent cmd is dead → the CIM query for `cmd.exe` with the tag returns empty → `$parents`
   is empty → the count is **structurally 0** regardless of any leaked/orphaned `choice.exe`. The final
   `wait_until(|| tagged_choice_descendant_count(&tag) == 0, ...)` is therefore satisfied vacuously on the first
   poll. The prior host-wide `image_count("choice.exe") <= baseline` would have detected a leaked survivor; the new
   scoping cannot. The helper's own doc ("without weakening any containment assertion") and 20-43-SUMMARY's "Every
   containment assertion is preserved verbatim" are inaccurate for this assertion. **Impact-mitigated** because the
   reap-after-close *property* is robustly proven by the sibling `job_close_reaps_detached_descendant_with_no_residue`
   (whose grandchild carries `rem {tag}` on its OWN command line, so `tagged_cmd_count` survives parent death) —
   but the specific active-process-cap reap check is a degraded near-tautology. Flagged HIGH by Codex; honestly
   rated MEDIUM here given the sibling coverage.
   **Fix for the further-repaired successor:** capture the fan-out `choice.exe` PIDs (or the parent cmd `ProcessId`)
   BEFORE job close and re-check them by fixed PID after close, or retain a tag-independent host-wide safety net
   for the reap assertion.

2. **[LOW — real, low reachability] `resolved_program_is_cmd` uses `ends_with("cmd.exe")`.**
   `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs` — `to_ascii_lowercase().ends_with("cmd.exe")`
   suffix-matches a hypothetical `notcmd.exe`/`foocmd.exe`, which would route its `/c`|`/k` payload through the
   cmd-specific `quote_cmd_payload` instead of `quote_arg`. Reachability is bounded (the resolver pins bare `cmd`
   to `System32\cmd.exe` and only `cmd` runs under the Low-IL token), so this is not an isolation-boundary
   regression — but it is an imprecise classifier. Flagged MEDIUM by Codex; honestly rated LOW here.
   **Fix:** compare the final path component `== "cmd.exe"` rather than a suffix match.

**Codex's third finding (evidence-integrity FAIL) is NOT carried as a real defect.** Codex read only
`20-43-SUMMARY.md` (the environmentally-disk-pressured `11508/1/48` run bound to `92cac8bb`) and did not reconcile
it with `20-44-SUMMARY.md`, which re-ran clean at **11509/0/48** (nextest run `32f1b4ba-ed6d-4d14-853f-f831d7798731`)
against the sealed tree `3092475b`, proved `--locked --workspace --all-features` exit 0, and sealed the candidate at
`3f839309` (adding exactly the one-line `Cargo.lock` dunce edge). The seal chain is correct; the `Cargo.lock` is part
of `daf27337..3f839309`. It is recorded here as an auditor observation, not a candidate defect.

**Guardrail verdicts (all four legs concur):**
- **Guardrail A (DispatchAdmission budget preserved):** PASS (4/4). Probe returns `Result<u64>`; on spawn/capture
  failure, non-zero status, or parse failure it returns `SwarmError::DispatchAdmission` (fail-closed). No
  `u64::MAX`, no `unwrap_or`, no catch-to-unlimited. `WCORE_SWARM_PROBE_ROOT` env transport changes delivery only,
  not the free-space computation. `dunce::simplified` is no-op on unix / genuine UNC.
- **Guardrail B (quoting-layer only, boundary primitives untouched):** PASS on isolation (4/4). The delta touches
  no `is_unc_or_device_path`, no Low-IL restricted token, no Job-Object limits, no ACL lease; argv discipline kept
  (payload is one caller-supplied argv entry, not `format!`-interpolated); the 20-36 `exit /b 0` normalization is
  intact. The only sub-finding is the LOW classifier imprecision above (Codex records it under Guardrail B).
- **Kimi watch-items carried into 20-46:** PASS (4/4). `20-46-PLAN.md` lines 24, 61, 83 carry git-ops-over-
  de-verbatimized-swarm_root, `tagged_cmd_count` CIM visibility under NetworkService, bash-worker-under-AppContainer
  + no-Windows-Docker-fallback, and worker test-exe DLL-load under Low-IL as explicit next-layer 20-46 checks
  (plus the extra `dispatch.rs:604` canonicalize watch-item).

---

## Artifact 1 — `f20-native-crossaudit.codex-sol` (Codex 5.6 Sol, external)

```json
{
  "schema": "f20-native-crossaudit.codex-sol",
  "source_sha": "3f839309574d6741eed416cd3820f56447f74eba",
  "source_tree": "3092475bb4102d010b6ff5f6c9d8080cb4f51928",
  "reviewer_id": "codex-5.6-sol",
  "reviewer_kind": "external-cli",
  "invocation": "codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check (brief on stdin)",
  "reachable": true,
  "schema_validated": true,
  "checks": {
    "all_severity": "FAIL",
    "evidence_integrity": "FAIL",
    "integration_authority": "FAIL"
  },
  "deferred": ["native_macos", "native_windows"],
  "findings": { "blocker": 0, "critical": 0, "high": 1, "medium": 2, "low": 0 },
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "FAIL",
  "kimi_watchitems_carried": "PASS",
  "evidence": [
    {
      "command": "hard_process_containment_windows.rs active_process_cap post-close reap: after the tagged parent exits, $parents is empty and tagged_choice_descendant_count returns 0 even if detached choice.exe descendants leaked — false-green containment assertion",
      "severity": "high",
      "result": "FAIL"
    },
    {
      "command": "command.rs resolved_program_is_cmd + process.rs cmd-payload routing: ends_with(\"cmd.exe\") misclassifies absolute executables such as notcmd.exe, whose /c|/k payload then receives cmd-specific raw quoting instead of CRT quoting",
      "severity": "medium",
      "result": "FAIL"
    },
    {
      "command": "Cargo.lock + 20-43-SUMMARY: alleged verbatim range omits Cargo.lock and planning/evidence files; the summary Codex read binds to another SHA and reports 11508/1/48 not 11509/0/48 — evidence integrity unproven from that summary alone [EXECUTOR NOTE: refuted — the clean 11509/0/48 seal is in 20-44-SUMMARY against tree 3092475b; not carried as a real candidate defect]",
      "severity": "medium",
      "result": "FAIL"
    }
  ],
  "disposition": "BLOCK"
}
```

## Artifact 2 — `f20-native-crossaudit.gemini-pro` (Gemini 3.1 Pro, external)

```json
{
  "schema": "f20-native-crossaudit.gemini-pro",
  "source_sha": "3f839309574d6741eed416cd3820f56447f74eba",
  "source_tree": "3092475bb4102d010b6ff5f6c9d8080cb4f51928",
  "reviewer_id": "gemini-3.1-pro-preview",
  "reviewer_kind": "external-cli",
  "invocation": "gemini --skip-trust -m gemini-3.1-pro-preview -o text -p <brief> (GEMINI_CLI_TRUST_WORKSPACE=true)",
  "reachable": true,
  "schema_validated": true,
  "checks": {
    "all_severity": "PASS",
    "evidence_integrity": "PASS",
    "integration_authority": "PASS"
  },
  "deferred": ["native_macos", "native_windows"],
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 0, "low": 0 },
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "kimi_watchitems_carried": "PASS",
  "evidence": [
    {
      "command": "Gemini returned a zero-finding PASS across all three checks and both guardrails; it did NOT surface the active_process_cap post-close reap-assertion degradation that Codex + the internal Claude leg caught (panel blind-spot coverage)",
      "result": "PASS"
    }
  ],
  "disposition": "PASS"
}
```

## Artifact 3 — `f20-native-crossaudit.kimi-k3` (Kimi K3, external)

```json
{
  "schema": "f20-native-crossaudit.kimi-k3",
  "source_sha": "3f839309574d6741eed416cd3820f56447f74eba",
  "source_tree": "3092475bb4102d010b6ff5f6c9d8080cb4f51928",
  "reviewer_id": "kimi-k3",
  "reviewer_kind": "external-cli",
  "invocation": "/Users/seandonahoe/.kimi-code/bin/kimi -p <brief> --output-format text",
  "reachable": true,
  "schema_validated": true,
  "checks": {
    "all_severity": "PASS",
    "evidence_integrity": "PASS",
    "integration_authority": "PASS"
  },
  "deferred": ["native_macos", "native_windows"],
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 0, "low": 0 },
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "kimi_watchitems_carried": "PASS",
  "evidence": [
    {
      "command": "Guardrail A: traced workspace_capacity -> available_workspace_bytes().await?; three fail-closed exits (capture_error/non-zero-status/parse -> DispatchAdmission), no unwrap_or/u64::MAX/catch-to-unlimited; dunce::simplified round-trips or returns the verbatim original so it cannot manufacture a wrong root",
      "result": "PASS"
    },
    {
      "command": "Guardrail B: argv discipline kept (one entry -> one token, not format!-interpolated); quote_cmd_payload wraps the existing entry in one outer pair (inner verbatim) as cmd /s requires; scope narrow (resolved cmd.exe + first exact /c|/k, flag_idx+1==len yields never-matching Some); no change to is_unc_or_device_path/token/Job/ACL; exit /b 0 present verbatim",
      "result": "PASS"
    },
    {
      "command": "r7/r3/nextest/evidence-integrity/watch-items: tag test-generated (safe -like interpolation); reap assertion judged preserved and CIM-visibility treated as Windows-runtime deferred; nextest cfg(windows) override inert on Linux (11509/0/48 consistent); watch-items ride the native_windows deferral into 20-46 without being closed or weakened [NOTE: Kimi treated the active_process_cap post-close reap check as belt-and-braces/runtime-deferred rather than a static code defect]",
      "result": "PASS"
    }
  ],
  "disposition": "PASS"
}
```

## Artifact 4 — `wayland-core.phase20-independent-review.v1` (internal Claude non-author adversarial)

```json
{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "3f839309574d6741eed416cd3820f56447f74eba",
  "source_tree": "3092475bb4102d010b6ff5f6c9d8080cb4f51928",
  "source_executor_id": "wayland-f20-native-repair-builder",
  "reviewer_id": "wayland-f20-45-claude-adversarial",
  "reviewer_kind": "internal-claude-adversarial",
  "checks": {
    "all_severity": "FAIL",
    "evidence_integrity": "PASS",
    "integration_authority": "PASS"
  },
  "deferred": ["native_macos", "native_windows"],
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 1, "low": 1 },
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "kimi_watchitems_carried": "PASS",
  "evidence": [
    {
      "command": "SHA/tree bind: git rev-parse 3f839309^{tree} == 3092475bb4102d010b6ff5f6c9d8080cb4f51928 (verified); 3f839309 show --stat = Cargo.lock | 1 + (the dunce edge seal); the 20-43 source delta daf27337..3f839309 is exactly the 8 declared files + Cargo.lock + nextest.toml",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "MEDIUM (non-deferred, real, impact-mitigated): hard_process_containment_windows.rs active_process_cap_is_enforced post-close reap assertion wait_until(tagged_choice_descendant_count(&tag)==0) is vacuously satisfied. The fan-out idlers are BARE choice.exe (choice rejects an injected tag) so the helper scopes by the tagged parent cmd ParentProcessId; by the post-close check run.await has returned -> job closed -> tagged parent cmd dead -> CIM cmd.exe-with-tag empty -> $parents empty -> count structurally 0 regardless of a leaked/orphaned choice. The prior host-wide image_count(choice.exe)<=baseline would have caught a leaked survivor. Contrast job_close_reaps_detached_descendant_with_no_residue whose grandchild carries rem {tag} on its OWN cmdline, so tagged_cmd_count survives parent death (robust). The helper doc 'without weakening any containment assertion' + 20-43-SUMMARY 'Every containment assertion is preserved verbatim' are inaccurate for this assertion. Static code fact, not a Windows-runtime question -> non-deferred. Impact-mitigated: reap-after-close property robustly proven by the sibling test",
      "exit_code": 0,
      "result": "FAIL"
    },
    {
      "command": "LOW (non-deferred, real, low reachability): command.rs resolved_program_is_cmd uses to_ascii_lowercase().ends_with(\"cmd.exe\") which suffix-matches notcmd.exe/foocmd.exe -> would route its /c|/k payload through quote_cmd_payload instead of quote_arg. Bounded reachability (resolver pins bare cmd to System32\\cmd.exe; only cmd runs under Low-IL) so NOT an isolation-boundary regression, but an imprecise classifier; should compare the final path component == cmd.exe",
      "exit_code": 0,
      "result": "FAIL"
    },
    {
      "command": "Guardrail A PASS: worktree_manager.rs workspace_capacity does available_workspace_bytes().await?; #[cfg(windows)] probe returns SwarmError::DispatchAdmission on spawn/capture failure, non-zero status, and parse failure; required = MAX_TRANSACTION_WORKSPACE_BYTES.min(...).checked_add(WORKSPACE_SAFETY_MARGIN_BYTES); grep u64::MAX|unwrap_or|unlimited in the probe path = none. env-var transport (command.env WCORE_SWARM_PROBE_ROOT) changes delivery only; dunce::simplified no-op on unix. No catch-to-unlimited",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "Guardrail B PASS (isolation): git diff daf27337..3f839309 -- crates/wcore-sandbox grep is_unc_or_device_path|JOB_OBJECT_LIMIT|SECURITY_MANDATORY_LOW|restricted token|SetNamedSecurityInfo|icacls|acl_lease|CreateAppContainer = no boundary primitive touched (only a comment line matches 'Low-integrity'). quote_cmd_payload is applied only to the entry after the first /c|/k when resolved lpApplicationName ends_with cmd.exe; every other argv keeps quote_arg; payload stays one caller-supplied argv entry (argv discipline). 20-36 exit /b 0 present verbatim in all containment scripts",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "Kimi watch-items carried: 20-46-PLAN.md lines 24/61/83 carry git-ops-over-de-verbatimized-swarm_root, tagged_cmd_count CIM visibility under NetworkService, bash-worker-under-AppContainer + no-Windows-Docker-fallback (dispatch.rs), worker test-exe DLL-load under Low-IL as explicit next-layer 20-46 checks (plus the extra dispatch.rs:604 canonicalize watch-item). Not silently dropped",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "Evidence integrity PASS (candidate): 20-44-SUMMARY records --locked --workspace --all-features exit 0 + aggregate nextest run 32f1b4ba-ed6d-4d14-853f-f831d7798731 = 11509 passed / 0 failed / 48 skipped against tree 3092475b, and the seal adds exactly the one-line Cargo.lock dunce edge at 3f839309. Codex's evidence-integrity FAIL derives from reading only 20-43-SUMMARY (env-blocked 11508/1/48, source_sha 92cac8bb) and is refuted; not carried as a real candidate defect",
      "exit_code": 0,
      "result": "PASS"
    }
  ],
  "disposition": "BLOCK"
}
```

---

## Routing

BLOCK → a **further repaired successor** must (1) fix the `active_process_cap_is_enforced` post-close reap
assertion so a leaked/orphaned bare-`choice` idler is still detectable (capture the fan-out PIDs / parent
`ProcessId` before job close and re-check by fixed PID, or keep a tag-independent host-wide safety net), and
(2) tighten `resolved_program_is_cmd` to match the exact `cmd.exe` filename component rather than a suffix.
Then re-seal (Cargo.lock + `--locked` build + aggregate) and re-run this four-way pre-native cross-audit to a
zero-finding all-four PASS before 20-46 is authorized. All prior native-proof authorizations remain spent/void.
