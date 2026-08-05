# 20-52 Pre-native FOUR-WAY cross-audit — re-sealed further-repaired-successor `f0dd5b6d`

Gate: pre-native attestation before the scarce Sean-gated native run (20-53) is re-spent.
Panel: **Codex 5.6 Sol + Gemini 3.1 Pro + Kimi K3** (three external CLIs, run in parallel)
**+ internal Claude non-author adversarial reviewer** (prompted to REFUTE, default-refuted-if-uncertain).

- **sealed source_sha:** `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2`
- **sealed source_tree:** `ac76c87b318ee4ba8c34927dea23e40e63fd0776` (verified `f0dd5b6d^{tree}` == this)
- **predecessor (sealed-but-RED, 20-45 BLOCK):** `3f839309574d6741eed416cd3820f56447f74eba` (tree `3092475bb4102d010b6ff5f6c9d8080cb4f51928`)
- **review base tuple:** `source_sha` over `20-51-SUMMARY.md`; delta prosecuted = the 20-50 repair over `3f839309` (the two 20-45 finding-fixes)
- **repair delta (source):** exactly two Windows-only files — `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` (+88/−18) and `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs` (+12/−4). No `Cargo.toml`/`Cargo.lock`/workflow/`worktree_manager.rs` change.
- **branch:** `plan/f20-unified-audit-repair` (isolated STANDALONE checkout `/Users/seandonahoe/dev/waylandcore-ferrox`; `.git` is a directory; all git ops via `/usr/bin/git`)
- **deferred (only):** `native_macos`, `native_windows` (proven at 20-53)
- **raw outputs preserved:** `.planning/phases/20-transactional-delegated-mutation/20-52-raw/{codex-sol,gemini-pro,kimi-k3}.raw.txt` + shared `audit-context.shared.txt`

## External CLI invocations (verbatim)

| Auditor | Invocation | Exit | Reachable | Schema-valid |
|---|---|---|---|---|
| Codex 5.6 Sol | `codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "<prompt>"` | 0 | YES | YES |
| Gemini 3.1 Pro | `GEMINI_CLI_TRUST_WORKSPACE=true gemini -p "<prompt>" -m gemini-3.1-pro-preview -o text --approval-mode plan --skip-trust` | 0 | YES | YES |
| Kimi K3 | `/Users/seandonahoe/.kimi-code/bin/kimi -p "<prompt>" --output-format text` (absolute path, brief in `-p`) | 0 | YES | YES |

All three external auditors were reachable and returned schema-validatable JSON keyed to the exact re-sealed
`source_sha f0dd5b6d` with their assigned schema keys, so the fail-closed "external auditor unreachable/invalid"
path was **NOT** triggered. This is a **finding-based BLOCK**, not an incomplete-panel BLOCK.

## Per-auditor disposition

| # | Auditor id | Disposition | b/c/h/m/l | Finding 1 (reap non-vacuous) | Finding 2 (cmd exact) | Guardrail A | Guardrail B |
|---|---|---|---|---|---|---|---|
| 1 | Codex 5.6 Sol (`f20-native-crossaudit.codex-sol`) | **BLOCK** | 0/0/0/1/1 | **FAIL** (residual observer false-green) | PASS | PASS | PASS |
| 2 | Gemini 3.1 Pro (`f20-native-crossaudit.gemini-pro`) | PASS | 0/0/0/0/0 | PASS | PASS | PASS | PASS |
| 3 | Kimi K3 (`f20-native-crossaudit.kimi-k3`) | PASS | 0/0/0/0/0 | PASS | PASS | PASS | PASS |
| 4 | Claude adversarial (`wayland-core.phase20-independent-review.v1`) | **BLOCK** | 0/0/0/0/1 | **FAIL** (concur: observer false-green) | PASS | PASS | PASS |

## GATE DISPOSITION: **BLOCK — does NOT admit 20-53**

Two of four auditors (Codex 5.6 Sol + internal Claude adversarial) raise a **real, non-deferred, statically-provable
finding** against the 20-50 Finding-1 fix. Under the plan's ALL-FOUR-MUST-PASS rule, any single non-deferred finding
from any one auditor stops the sequence and routes to a **further repaired successor**. Gemini and Kimi returned a
zero-finding PASS but both **missed** the residual reap-observer false-green — the exact class of shared blind spot a
four-independent panel exists to catch (mirroring the 20-45 pattern, where Codex + Claude caught the vacuity that
Gemini + Kimi missed).

**The finding is NOT rationalized away, and it is NOT a refutable false positive.** It is a statically-provable code
fact (verified against the sealed tree, not a Windows-runtime question):

### FINDING (Codex MEDIUM / Claude LOW — real, non-deferred): residual reap-observer false-green in `surviving_captured_choice_pids`

`crates/wcore-sandbox/tests/hard_process_containment_windows.rs`. The 20-50 fix correctly closes the **structural**
vacuity the 20-45 panel blocked on: the post-close reap check now filters `choice.exe` by the **fixed captured
ProcessId set** (`surviving_captured_choice_pids(&captured_pids)`), so a leaked/orphaned captured survivor keeps its
PID and is counted, while a concurrent `live_fs_acl` `choice.exe` carries a non-captured PID and is excluded (no
host-wide-image-count flake). That half is sound (all four legs concur).

**But the new observer helper ends in `String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)` with NO
`out.status.success()` check** (`.output().expect(...)` only catches a spawn failure, not a query failure). On any
post-close CIM/PowerShell query failure — empty or malformed stdout — `parse().unwrap_or(0)` yields `0`, so
`wait_until(|| surviving_captured_choice_pids(&captured_pids) == 0, 30, ...)` is satisfied on the first poll and the
reap property is reported proven **without evidence**. The new belt-and-braces `assert!(!captured_pids.is_empty())`
guards the ALIVE capture phase only; it does **not** guard the post-close OBSERVER query. So the fix replaces a
GUARANTEED structural vacuity with a NARROWER but still statically-provable false-green path on the exact property it
was meant to make sound. The same `parse().unwrap_or(0)` pattern is shared by the sibling reap test's
`tagged_cmd_count` observer (used by `job_close_reaps_detached_descendant_with_no_residue`), so the 20-45
"impact-mitigated by the robust sibling" argument is itself weakened — both reap observers fail OPEN on a query error.

- **Not refutable:** no executable counter-evidence can show `unwrap_or(0)` fails closed — it demonstrably returns
  `0` on empty/malformed stdout. This is why the Claude leg does not refute it and instead concurs.
- **Severity:** Codex rates MEDIUM; the Claude leg rates LOW (the fix is strictly better than the predecessor's
  guaranteed vacuity, and the false-green requires an actual post-close query-infra failure). Either way it is a
  real, non-deferred finding, so under ALL-FOUR-MUST-PASS it BLOCKS.
- **Fix for the further-repaired successor:** harden the post-close observer to FAIL CLOSED — assert
  `out.status.success()` and treat empty/unparseable stdout as a test failure (panic) rather than `unwrap_or(0) => 0`.
  Apply the same hardening consistently to the sibling `tagged_cmd_count` reap observer so both reap checks are sound.

### Codex LOW (recorded as a fail-closed watch-item, not an independent blocker): post-close PID-reuse race

Codex notes fixed-PID-plus-image is not a stable identity after job close: Windows could recycle a captured PID to a
concurrent `choice.exe` before the survivor query, producing a false **FAIL**. The Claude leg and Kimi both judge
this **fail-closed** (it can only make the test fail, never false-green) and low-probability (a new `choice.exe` must
draw one of the specific recycled PIDs within the 30s window). It is recorded as a **20-53-observable watch-item**,
not an independent blocking finding — but the successor SHOULD prefer creation-time/handle identity if convenient.

**Guardrail verdicts (all four legs concur — PASS):**
- **Finding 2 (cmd exact-final-component):** PASS (4/4). `resolved_program_is_cmd` now decodes+lowercases then
  `Path::new(&decoded).file_name() == "cmd.exe"` — accepts `System32\cmd.exe` (Windows `Path` treats `\` and `/`
  as separators), rejects `notcmd.exe`/`foocmd.exe`; the new match set is a strict subset of the old `ends_with`
  set, so no genuine `cmd.exe` resolution is newly misclassified. `quote_cmd_payload`/`quote_arg`/
  `classify_bare_shell`/`resolve_program`/`is_unc_or_device_path` are byte-identical; the single call site
  (`windows_impl/process.rs`) is unchanged.
- **Guardrail A (DispatchAdmission budget preserved):** PASS (4/4). The delta does not touch `worktree_manager.rs`
  or any file outside the two named ones — the admission budget is preserved by absence of change.
- **Guardrail B (quoting-layer only, boundary primitives untouched):** PASS (4/4). No `is_unc_or_device_path`, no
  Low-IL restricted token, no Job-Object limits, no ACL lease touched; argv discipline kept; the 20-36 `exit /b 0`
  normalization is intact in all containment scripts.
- **Delta scope / evidence integrity:** PASS (4/4). `f0dd5b6d^{tree}` == `ac76c87b`; the non-planning delta against
  `3f839309` is exactly the two declared Windows-only files; `windows_impl/tests.rs` is **byte-identical** to the
  predecessor (Kimi's 19 lib-test cross-check errors are pre-existing host-dependent `windows-sys` feature-unification
  artifacts, identical at `3f839309`, out of delta scope, and are the deferred `native_windows` compile check).
- **Kimi watch-items carried into 20-53:** PASS. `20-53-PLAN.md` carries git-ops-over-de-verbatimized-swarm_root,
  `tagged_cmd_count` CIM visibility under NetworkService, bash-worker-under-AppContainer + no-Windows-Docker-fallback
  (`dispatch.rs`), worker test-exe DLL-load under Low-IL, and the `dispatch.rs:604` canonicalize item — plus the two
  20-50 fix-proof items (Finding 1 non-vacuous captured-PID reap; Finding 2 granted-read exit-0) — as explicit
  next-layer 20-53 checks. These ride the block into the re-repair (the successor's fix-proof items expand to include
  the hardened observer).

---

## Artifact 1 — `f20-native-crossaudit.codex-sol` (Codex 5.6 Sol, external)

```json
{
  "schema": "f20-native-crossaudit.codex-sol",
  "source_sha": "f0dd5b6d312af616f268f96f34c3bc9fc962c4d2",
  "source_tree": "ac76c87b318ee4ba8c34927dea23e40e63fd0776",
  "reviewer_id": "codex-5.6-sol",
  "reviewer_kind": "external-cli",
  "invocation": "codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check (brief on prompt)",
  "reachable": true,
  "schema_validated": true,
  "checks": { "all_severity": "FAIL", "evidence_integrity": "FAIL", "integration_authority": "FAIL" },
  "deferred": ["native_macos", "native_windows"],
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 1, "low": 1 },
  "finding_1_reap_nonvacuous": "FAIL",
  "finding_2_cmd_exact_component": "PASS",
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "evidence": [
    { "claim": "The sealed commit resolves to tree ac76c87b318ee4ba8c34927dea23e40e63fd0776; the non-planning delta against 3f839309 is exactly the two declared Windows-only files, with no Cargo.toml, workflow, worktree_manager.rs, process.rs, ACL-lease, or live_fs_acl change.", "severity": "none", "result": "PASS" },
    { "claim": "The captured-PID survivor query is syntactically well formed, scopes by the nonempty captured PID set intersected with choice.exe, and therefore detects a captured same-PID choice.exe survivor without counting ordinary concurrent choice.exe processes having different PIDs.", "severity": "none", "result": "PASS" },
    { "claim": "surviving_captured_choice_pids ignores PowerShell exit status and stderr, then converts empty or malformed stdout to zero with parse().unwrap_or(0); any CIM or PowerShell query failure therefore makes the post-close assertion pass immediately, preserving a statically provable false-green path. The nonempty captured_pids assertion does not guard the post-close observer.", "severity": "medium", "result": "FAIL" },
    { "claim": "Fixed PID plus image name is not a stable process identity after job close: Windows may reuse one of the captured PIDs for a concurrent choice.exe before the first survivor query, producing the host-concurrency false failure the fix claims to exclude. Capturing creation identity or retaining a process handle is required to eliminate this race.", "severity": "low", "result": "FAIL" },
    { "claim": "The peak-positive, peak-at-or-below-512, peak-below-attempts, and exit-code-zero assertions remain; job_close_reaps_detached_descendant_with_no_residue and its independently tagged descendant check are unchanged.", "severity": "none", "result": "PASS" },
    { "claim": "resolved_program_is_cmd now compares the lowercased exact Windows final path component to cmd.exe, accepting System32\\cmd.exe and rejecting notcmd.exe or foocmd.exe; quote_cmd_payload, quote_arg, classify_bare_shell, resolve_program, and is_unc_or_device_path are unchanged.", "severity": "none", "result": "PASS" },
    { "claim": "DispatchAdmission budget code is outside and byte-unchanged by this delta; the quoting change remains classifier-only, while Low-IL token setup, Job-Object limits, ACL leasing, argv construction, and exit /b 0 normalization are untouched.", "severity": "none", "result": "PASS" }
  ],
  "disposition": "BLOCK"
}
```

## Artifact 2 — `f20-native-crossaudit.gemini-pro` (Gemini 3.1 Pro, external)

```json
{
  "schema": "f20-native-crossaudit.gemini-pro",
  "source_sha": "f0dd5b6d312af616f268f96f34c3bc9fc962c4d2",
  "source_tree": "ac76c87b318ee4ba8c34927dea23e40e63fd0776",
  "reviewer_id": "gemini-3.1-pro-preview",
  "reviewer_kind": "external-cli",
  "invocation": "GEMINI_CLI_TRUST_WORKSPACE=true gemini -p <brief> -m gemini-3.1-pro-preview -o text --approval-mode plan --skip-trust",
  "reachable": true,
  "schema_validated": true,
  "checks": { "all_severity": "PASS", "evidence_integrity": "PASS", "integration_authority": "PASS" },
  "deferred": ["native_macos", "native_windows"],
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 0, "low": 0 },
  "finding_1_reap_nonvacuous": "PASS",
  "finding_2_cmd_exact_component": "PASS",
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "evidence": [
    { "claim": "Finding 1: Post-close reap check explicitly targets captured choice.exe PIDs, correctly eliminating the vacuous parent-scoped query while avoiding the host-wide flake.", "severity": "none", "result": "PASS" },
    { "claim": "Finding 1: New guard !captured_pids.is_empty() enforces test correctness and prevents stealth vacuity.", "severity": "none", "result": "PASS" },
    { "claim": "Finding 2: resolved_program_is_cmd uses exact final path component matching via file_name(), correctly rejecting spoofed files like notcmd.exe while preserving quoting layer isolation.", "severity": "none", "result": "PASS" },
    { "claim": "Guardrails: No other logic was modified; admission budgets and core isolation checks remain fully intact.", "severity": "none", "result": "PASS" }
  ],
  "disposition": "PASS",
  "panel_note": "Gemini did NOT surface the residual reap-observer parse().unwrap_or(0) false-green that Codex + the internal Claude leg caught (panel blind-spot coverage)."
}
```

## Artifact 3 — `f20-native-crossaudit.kimi-k3` (Kimi K3, external)

```json
{
  "schema": "f20-native-crossaudit.kimi-k3",
  "source_sha": "f0dd5b6d312af616f268f96f34c3bc9fc962c4d2",
  "source_tree": "ac76c87b318ee4ba8c34927dea23e40e63fd0776",
  "reviewer_id": "kimi-k3",
  "reviewer_kind": "external-cli",
  "invocation": "/Users/seandonahoe/.kimi-code/bin/kimi -p <brief> --output-format text",
  "reachable": true,
  "schema_validated": true,
  "checks": { "all_severity": "PASS", "evidence_integrity": "PASS", "integration_authority": "PASS" },
  "deferred": ["native_macos", "native_windows"],
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 0, "low": 0 },
  "finding_1_reap_nonvacuous": "PASS",
  "finding_2_cmd_exact_component": "PASS",
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "evidence": [
    { "claim": "Sealed identity verified against the repository: git log -1 f0dd5b6d reports tree ac76c87b318ee4ba8c34927dea23e40e63fd0776 exactly as sealed, on branch plan/f20-unified-audit-repair", "severity": "none", "result": "PASS" },
    { "claim": "Actual git diff 3f839309..f0dd5b6d for the two named files reproduces the diff quoted in the audit brief verbatim (191 lines, identical hunks); under crates/ ONLY command.rs and hard_process_containment_windows.rs changed (remaining delta entries are .planning/ process docs); zero changes to Cargo.toml, Cargo.lock, .github, justfile, vx.toml", "severity": "none", "result": "PASS" },
    { "claim": "Finding 1 non-vacuous: post-close check surviving_captured_choice_pids(&captured_pids) filters Win32_Process by image Name='choice.exe' intersected with the FIXED ProcessId set captured while the tagged parent was alive; a leaked/orphaned captured choice keeps its PID for its lifetime, so it is counted and fails wait_until(==0, 30s). Non-vacuity proven by construction", "severity": "none", "result": "PASS" },
    { "claim": "Finding 1 flake-freedom: the post-close filter matches ONLY this test's captured PIDs; a concurrent live_fs_acl choice.exe carries a non-captured PID and is excluded; no reversion to host-wide image_count baseline. PID-list PowerShell is digits+commas only (no injection), empty slice short-circuits before any filter is issued", "severity": "none", "result": "PASS" },
    { "claim": "Finding 1 guard and preserved assertions: assert!(!captured_pids.is_empty()) directly asserts the post-close precondition; peak>0, peak<=512, peak<attempts, exit_code==0, and the sibling job_close_reaps_detached_descendant_with_no_residue are all preserved verbatim per the byte-identical diff", "severity": "none", "result": "PASS" },
    { "claim": "Finding 1 adversarial residuals prosecuted and dismissed: (a) tail spawns after the final peak sample are not in captured_pids, but any real containment failure leaks the bulk captured set, and the predecessor was fully vacuous — strictly better; (b) PID-reuse false-fail requires a NEW choice.exe to draw one of ~256+ specific recycled PIDs within 30s — negligible and fail-safe direction; (c) early-break overshoot re-captures correctly; (d) tag interpolation is [A-Za-z0-9_] only", "severity": "none", "result": "PASS" },
    { "claim": "Finding 2 exact-final-component: decoded+lowercased path -> Path::file_name() == cmd.exe; System32\\cmd.exe and forward-slash variants match; notcmd.exe/foocmd.exe rejected; new match set is a strict subset of the old ends_with set so NO real cmd.exe is newly misclassified and isolation quoting cannot regress", "severity": "none", "result": "PASS" },
    { "claim": "Finding 2 blast radius: quote_cmd_payload, quote_arg, classify_bare_shell, resolve_program, is_unc_or_device_path byte-identical; the single call site (windows_impl/process.rs:620) unchanged; no existing test pins the old suffix behavior", "severity": "none", "result": "PASS" },
    { "claim": "Guardrail A: delta does not touch worktree_manager.rs or any file outside the two named ones — admission budget preserved by absence of change", "severity": "none", "result": "PASS" },
    { "claim": "Guardrail B: is_unc_or_device_path, Low-IL restricted-token path, Job-Object limit setup, and ACL lease untouched; argv discipline at CreateProcessAsUserW unchanged; 20-36 exit /b 0 retained verbatim", "severity": "none", "result": "PASS" },
    { "claim": "Construction compile evidence: cargo check --target x86_64-pc-windows-msvc -p wcore-sandbox succeeds for the lib and for --test hard_process_containment_windows (clean). A full --tests cross-check surfaces 19 lib-test errors confined to pre-existing files untouched by the delta (windows_impl/tests.rs etc.), identical at 3f839309, characteristic of host-dependent windows-sys feature unification when cross-checking from macOS; not introduced by this fix and out of delta scope", "severity": "none", "result": "PASS" },
    { "claim": "Integration authority: the repair answers exactly the two 20-45 panel findings, in exactly the two files those findings name, with no opportunistic edits, no workflow/manifest mutation; lineage 3f839309 -> f0dd5b6d confirmed in git", "severity": "none", "result": "PASS" }
  ],
  "disposition": "PASS",
  "panel_note": "Kimi prosecuted the PID-reuse and tail-spawn residuals and dismissed them, but did NOT surface the residual reap-observer parse().unwrap_or(0) false-green that Codex + the internal Claude leg caught."
}
```

## Artifact 4 — `wayland-core.phase20-independent-review.v1` (internal Claude non-author adversarial)

```json
{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "f0dd5b6d312af616f268f96f34c3bc9fc962c4d2",
  "source_tree": "ac76c87b318ee4ba8c34927dea23e40e63fd0776",
  "source_executor_id": "wayland-f20-native-repair-builder",
  "reviewer_id": "wayland-f20-52-claude-adversarial",
  "reviewer_kind": "internal-claude-adversarial",
  "checks": { "all_severity": "FAIL", "evidence_integrity": "PASS", "integration_authority": "PASS" },
  "deferred": ["native_macos", "native_windows"],
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 0, "low": 1 },
  "finding_1_reap_nonvacuous": "FAIL",
  "finding_2_cmd_exact_component": "PASS",
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "kimi_watchitems_carried": "PASS",
  "evidence": [
    { "command": "SHA/tree bind: git rev-parse f0dd5b6d^{tree} == ac76c87b318ee4ba8c34927dea23e40e63fd0776 (verified); git diff 3f839309..f0dd5b6d source delta = exactly command.rs (+12/-4) and hard_process_containment_windows.rs (+88/-18); windows_impl/tests.rs is byte-identical to the predecessor (git diff = 0 lines); no Cargo.toml/Cargo.lock/workflow/worktree_manager.rs change", "exit_code": 0, "result": "PASS" },
    { "command": "Finding 1 STRUCTURAL vacuity CLOSED (the 20-45 block): the post-close reap check is now surviving_captured_choice_pids(&captured_pids), filtering choice.exe by the FIXED captured ProcessId set (Where {$pids -contains $_.ProcessId}). A leaked/orphaned captured survivor keeps its PID and is counted; a concurrent live_fs_acl choice.exe carries a non-captured PID and is excluded (no host-wide-image-count flake). captured_pids is set together with peak so it is non-empty iff peak>0. This half is sound", "exit_code": 0, "result": "PASS" },
    { "command": "LOW (non-deferred, real, non-refutable): RESIDUAL reap-observer false-green. surviving_captured_choice_pids ends in String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0) with NO out.status.success() check (.output().expect only catches spawn failure). On a post-close CIM/PowerShell query failure (empty/malformed stdout) it returns 0, so wait_until(surviving_captured_choice_pids(&captured_pids)==0, 30) is satisfied on the first poll and the reap property is reported proven WITHOUT evidence. The new assert!(!captured_pids.is_empty()) guards the ALIVE capture phase only, not the post-close OBSERVER. The fix replaces a GUARANTEED structural vacuity with a narrower but still statically-provable false-green on the exact fixed property; the same parse().unwrap_or(0) pattern is shared by the sibling tagged_cmd_count reap observer, weakening the 20-45 robust-sibling mitigation. Not refutable (unwrap_or(0) demonstrably returns 0 on empty stdout) so concurred, not refuted. Rated LOW (strictly better than the predecessor; requires an actual query-infra failure) but real and non-deferred", "exit_code": 0, "result": "FAIL" },
    { "command": "Codex LOW PID-reuse race recorded as a fail-closed 20-53 watch-item, NOT an independent blocker: recycling a captured PID to a concurrent choice.exe within the 30s window can only make the test FAIL (fail-closed), never false-green; low probability. Successor should prefer creation-time/handle identity if convenient", "exit_code": 0, "result": "PASS" },
    { "command": "Finding 2 CLOSED: resolved_program_is_cmd = Path::new(&decoded.to_ascii_lowercase()).file_name().and_then(to_str).is_some_and(|n| n==cmd.exe). Accepts System32\\cmd.exe (Windows Path treats \\ and / as separators), rejects notcmd.exe/foocmd.exe; new match set is a strict subset of the old ends_with set so no genuine cmd.exe resolution is newly misclassified (a None file_name -> non-cmd -> quote_arg, safe for non-cmd). quote_cmd_payload/quote_arg/classify_bare_shell/resolve_program/is_unc_or_device_path byte-identical; single call site unchanged", "exit_code": 0, "result": "PASS" },
    { "command": "Preserved assertions verbatim: exit_code==0 (l406), peak>0 (l411), peak<=SANDBOX_ACTIVE_PROCESS_LIMIT=512 (l415), peak<attempts (l420), reap_stray_choice (l442); NEW assert!(!captured_pids.is_empty()) (l425); sibling job_close_reaps_detached_descendant_with_no_residue and tagged_cmd_count untouched", "exit_code": 0, "result": "PASS" },
    { "command": "Guardrail A PASS: the delta does not touch worktree_manager.rs at all; the DispatchAdmission capacity probe is byte-unchanged between 3f839309 and f0dd5b6d — admission budget preserved by absence of change", "exit_code": 0, "result": "PASS" },
    { "command": "Guardrail B PASS: command.rs change is only resolved_program_is_cmd + its doc; is_unc_or_device_path, Low-IL restricted token, Job-Object limits, ACL lease untouched; argv discipline kept (payload one caller-supplied argv entry); 20-36 exit /b 0 present verbatim in the containment scripts (untouched by the test diff)", "exit_code": 0, "result": "PASS" },
    { "command": "Kimi watch-items carried into 20-53: 20-53-PLAN.md carries git-ops-over-de-verbatimized-swarm_root, tagged_cmd_count CIM visibility under NetworkService, bash-worker-under-AppContainer + no-Windows-Docker-fallback (dispatch.rs), worker test-exe DLL-load under Low-IL, dispatch.rs:604 canonicalize, plus the two 20-50 fix-proof items (non-vacuous captured-PID reap; granted-read exit-0) as explicit next-layer checks. Not silently dropped", "exit_code": 0, "result": "PASS" },
    { "command": "Panel reachability: all three external CLIs (Codex 5.6 Sol, Gemini 3.1 Pro, Kimi K3) reachable, exit 0, schema-valid JSON keyed to f0dd5b6d — the fail-closed unreachable/partial-panel path was NOT triggered; this is a finding-based BLOCK", "exit_code": 0, "result": "PASS" }
  ],
  "disposition": "BLOCK"
}
```

---

## Routing

**BLOCK → a further repaired successor** must harden the post-close reap OBSERVER to fail closed: in
`surviving_captured_choice_pids` (and consistently in the sibling `tagged_cmd_count` reap observer), assert
`out.status.success()` and treat empty/unparseable stdout as a test failure (panic) instead of
`parse().unwrap_or(0) => 0`, so a post-close CIM/PowerShell query failure can no longer satisfy the reap assertion
without evidence. The successor SHOULD also consider creation-time/handle process identity to close the Codex LOW
PID-reuse race (currently fail-closed / 20-53-observable). Touch ONLY the file(s) the finding names; the Finding-2
cmd classifier, both isolation guardrails (#A/#B), and every other containment assertion are confirmed sound and must
be preserved verbatim. Then re-seal (confirm zero-lock-delta + `--locked` build + Linux aggregate floor) and re-run
this four-way pre-native cross-audit to a zero-finding ALL-FOUR PASS before 20-53 is authorized. All prior
native-proof authorizations remain spent/void; the scarce native run is NOT re-spent on this candidate.
