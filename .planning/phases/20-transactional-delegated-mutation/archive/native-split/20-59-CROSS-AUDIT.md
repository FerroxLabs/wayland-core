# 20-59 Pre-native FOUR-WAY cross-audit — re-sealed further-repaired-successor `8a1d2d84`

Gate: pre-native attestation before the scarce Sean-gated native run (rebound 20-53) is re-spent.
Panel: **Codex 5.6 Sol + Gemini 3.1 Pro + Kimi K3** (three external CLIs, run in PARALLEL)
**+ internal Claude non-author adversarial reviewer** (prompted to REFUTE, default-refuted-if-uncertain).

- **sealed source_sha:** `8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e`
- **sealed source_tree:** `c1fe79fe4a6d68a536078be4887343a82b5fce38` (verified `git rev-parse 8a1d2d84^{tree}` == this)
- **predecessor (20-52-BLOCKED, sealed):** `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2` (tree `ac76c87b318ee4ba8c34927dea23e40e63fd0776`)
- **review base tuple:** `source_sha` over `20-58-SUMMARY.md`; delta prosecuted = the 20-57 observer-hardening over `f0dd5b6d` (the fail-closed hardening of all three host-side reap-observers)
- **repair delta (source):** exactly ONE Windows-only file — `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` (+53/−10). No `Cargo.toml`/`Cargo.lock`/workflow/`command.rs`/`worktree_manager.rs` change (verified: `git diff --name-only f0dd5b6d 8a1d2d84 | grep -v planning` == that one file; direct-parent `c902b2e5..8a1d2d84` == that one file).
- **branch:** `plan/f20-unified-audit-repair` (isolated STANDALONE checkout `/Users/seandonahoe/dev/waylandcore-ferrox`; `.git` is a directory; all git ops via `/usr/bin/git`)
- **deferred (only):** `native_macos`, `native_windows` (proven at the rebound 20-53)
- **raw outputs preserved:** `.planning/phases/20-transactional-delegated-mutation/20-59-raw/{codex-sol,gemini-pro,kimi-k3,claude-adversarial}.raw.txt` + shared `audit-context.shared.txt`

## External CLI invocations (verbatim)

| Auditor | Invocation | Exit | Reachable | Schema-valid |
|---|---|---|---|---|
| Codex 5.6 Sol | `codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "<prompt>"` (brief on stdin) | 0 | YES | YES |
| Gemini 3.1 Pro | `GEMINI_CLI_TRUST_WORKSPACE=true gemini -p "<prompt>" -m gemini-3.1-pro-preview -o text --approval-mode plan --skip-trust` | 0 | YES | YES |
| Kimi K3 | `/Users/seandonahoe/.kimi-code/bin/kimi -p "<prompt>" --output-format text` (absolute path, brief in `-p`) | 0 | YES | YES |

All three external auditors were reachable and returned schema-validatable JSON keyed to the exact re-sealed
`source_sha 8a1d2d84` with their assigned schema keys, so the fail-closed "external auditor unreachable/invalid"
path was **NOT** triggered. This is a **finding-based BLOCK**, not an incomplete-panel BLOCK.

## Per-auditor disposition

| # | Auditor id | Disposition | b/c/h/m/l | Obs-1 tagged_cmd_count | Obs-2 descendant_pids | Obs-3 surviving | Assertions preserved | Finding-2 classifier | #A budget | #B quoting |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | Codex 5.6 Sol (`f20-native-crossaudit.codex-sol`) | **BLOCK** | 0/0/0/1/1 | **FAIL** | **FAIL** | **FAIL** | PASS | PASS | PASS | PASS |
| 2 | Gemini 3.1 Pro (`f20-native-crossaudit.gemini-pro`) | PASS | 0/0/0/0/0 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| 3 | Kimi K3 (`f20-native-crossaudit.kimi-k3`) | **BLOCK** | 0/0/0/0/1 | PASS¹ | PASS¹ | PASS¹ | PASS | PASS | PASS | PASS |
| 4 | Claude adversarial (`wayland-core.phase20-independent-review.v1`) | **BLOCK** | 0/0/0/0/1 | **FAIL** | **FAIL** | **FAIL** | PASS | PASS | PASS | PASS |

¹ Kimi marked the three observer fields PASS at the Rust-mechanism level (each *does* implement status-assert + panicking parse) but carried the cross-cutting exit-0/stderr fail-open as a separate LOW `evidence` item with `result: FAIL`, forcing `all_severity: FAIL` and `disposition: BLOCK` under the "PASS only if zero findings at any severity" rule. Codex and the Claude leg map the same defect onto the three observer fields directly (the fail-open is a property OF those observers), hence FAIL there. The substantive finding is identical across all three blocking legs.

## GATE DISPOSITION: **BLOCK — does NOT admit the rebound 20-53**

Three of four auditors (Codex 5.6 Sol + Kimi K3 + internal Claude adversarial) raise the **same real, non-deferred,
statically-provable finding** against the 20-57 observer-hardening fix. Under the plan's ALL-FOUR-MUST-PASS rule, any
single non-deferred finding from any one auditor stops the sequence and routes to a **further repaired successor**.
Gemini returned a zero-finding PASS but **missed** the PowerShell-layer fail-open — the same class of shared blind spot a
four-independent panel exists to catch. Notably Kimi — which MISSED the 20-52 Rust-layer fail-open — **independently
caught** this deeper PowerShell-layer instance and converged with Codex on the exact one-line fix.

**The finding is NOT rationalized away, and it is NOT a refutable false positive.** It is a statically-provable code
fact verified against the sealed tree:

### FINDING (Codex MEDIUM / Kimi LOW / Claude LOW — real, non-deferred): residual PowerShell-LAYER fail-open in all three reap-observers

`crates/wcore-sandbox/tests/hard_process_containment_windows.rs`. The 20-57 fix correctly closes the **Rust-layer**
fail-open the 20-52 panel blocked on: each observer now `assert!(out.status.success())` after `.output()` and parses
with a panicking fallback (no `parse().unwrap_or(0) => 0`, no `filter_map(...ok())` swallow). That half is sound (all
four legs concur; grep-verified: `out.status.success()` ×3, `unwrap_or_else(|err| panic!)` ×3, `filter_map` absent, sole
`unwrap_or(0)` is the benign `unique_tag` nanos).

**But the fix hardens only the Rust layer, not the PowerShell layer.** The three `-Command` scripts run
`@(Get-CimInstance Win32_Process ...).Count` (and the PID-list variant) with **NO `-ErrorAction Stop`, NO
`$ErrorActionPreference='Stop'`, NO `trap`** (grep over the sealed file: none present). Under Windows PowerShell, a
`Get-CimInstance` failure is a **non-terminating** error by default: the pipeline continues, `@(<failed pipeline>)`
yields an empty array so `.Count` prints `"0"` (and the PID query prints no tokens), and `powershell.exe -Command`
**exits 0** (no terminating exception; in WinPS 5.x `@(...)` resets `$?` to `True`). Therefore on a non-terminating
CIM failure:

- `out.status.success()` == **TRUE** (exit 0) → the new assert does not fire,
- stdout == `"0"` (Count observers) or `""` (PID observer) → the panicking parse succeeds on `"0"`/empty,
- so `wait_until(|| surviving_captured_choice_pids(&captured_pids) == 0, 30, ...)` (and the sibling `tagged_cmd_count`
  / `tagged_choice_descendant_pids` reap checks) is satisfied on the first poll and the reap property is reported
  proven **without evidence**.

This is the **exact 20-52 fail-open class moved one layer down** (Rust → PowerShell). The 20-57 doc comment — "a
post-close query failure therefore cannot satisfy a reap `wait_until(... == 0)` without evidence" — **overclaims**: it
holds for a non-zero exit or unparseable stdout, but NOT for the exit-0-with-error-record mode.

- **Not refutable:** no executable counter-evidence can show the observers fail closed on a non-terminating CIM error —
  the documented PowerShell semantics demonstrate exit 0 + `"0"`/empty output, and the file is `#![cfg(windows)]` so no
  macOS/Linux run can prove otherwise. The Claude leg (prompted to REFUTE, default-refuted-if-uncertain) therefore
  **concurs**, and the initial Rust-layer-only read (which would have PASSed, mirroring Gemini) is corrected by the
  deeper prosecution.
- **Concrete trigger, not purely theoretical:** the carried Kimi watch-item "`tagged_cmd_count` CIM visibility under
  NetworkService" flags that the CIM query's behavior under a restricted token is a live 20-53 question; access/provider
  errors under such a token are precisely the non-terminating class that leaves exit 0.
- **Severity:** Codex MEDIUM; Kimi + Claude LOW (the fix is strictly better than the predecessor's Rust-layer
  fail-open, and the false-green requires an actual partial CIM failure that exits 0). Either way it is a real,
  non-deferred finding, so under ALL-FOUR-MUST-PASS it BLOCKS.
- **Fix for the further-repaired successor:** add `-ErrorAction Stop` to each `Get-CimInstance` (any CIM error becomes a
  terminating error → `powershell.exe` exits non-zero → caught by the existing `assert!(out.status.success())`), and/or
  set `$ErrorActionPreference='Stop'` at the top of each script, and/or assert `out.stderr` is empty on the success path.
  Touch ONLY the three observer script bodies; preserve every assertion, the empty-set short-circuit, and the sibling
  tests verbatim, as the 20-57 fix already did for the Rust layer.

### Codex LOW (recorded as a refuted/clarified evidence nit, NOT a candidate defect): "one-file delta" phrasing

Codex flags that `git diff f0dd5b6d..8a1d2d84` lists 16 paths (15 `.planning/` artifacts + the Windows test), not one
file, so the brief's "ONE file" is imprecise; only the direct-parent `c902b2e5..8a1d2d84` is one file at +53/−10.
**Partially refuted:** the SOURCE repair delta restricted to non-planning paths IS exactly one file
(`git diff --name-only f0dd5b6d 8a1d2d84 | grep -v planning` == the one test file; direct-parent delta == the same one
file). Both Codex and Kimi concur the *code* delta is one file; the 16-path count is the interleaved planning-doc
commits between the seal and the branch tip. This is an audit-brief phrasing clarification, not a defect in the
candidate, and does not affect the disposition — the MEDIUM/LOW PowerShell-layer finding already blocks.

**Guardrail verdicts (all four legs concur — PASS):**
- **Rust-layer observer hardening:** PASS (4/4). `out.status.success()` ×3, panicking parse ×3, `filter_map` absent,
  sole remaining `unwrap_or(0)` is the benign `unique_tag` nanos, `reap_stray_choice` best-effort unchanged. (The
  BLOCK is the PowerShell layer BELOW this, not a regression in the Rust layer.)
- **Assertions preserved verbatim:** PASS (4/4). `if pids.is_empty(){return 0;}` (L221), `peak > 0` (L455),
  `peak <= SANDBOX_ACTIVE_PROCESS_LIMIT` (L459, =512), `peak < attempts` (L464), `!captured_pids.is_empty()` (L469),
  exit-0 asserts, all `reap_stray_choice` trailers, and the sibling
  `job_close_reaps_detached_descendant_with_no_residue` / `breakaway_is_denied` /
  `qualified_hard_containment_backend_preflight` tests — no assertion weakened under cover of "hardening".
- **Full-file sweep / no THIRD fail-open observer of the same class:** PASS (4/4) at the Rust-mechanism level. The
  PowerShell-layer defect is a cross-cutting refinement of the SAME three observers, not a fourth observer.
- **Finding-2 (`command.rs` cmd exact-final-component classifier):** PASS (4/4). Untouched by absence of change in the
  one-file test delta; still holds.
- **Guardrail A (DispatchAdmission budget, `worktree_manager.rs`):** PASS (4/4). Untouched by absence of change.
- **Guardrail B (quoting-layer only, boundary primitives):** PASS (4/4). Untouched by absence of change.
- **Delta scope / evidence integrity:** PASS (Gemini/Kimi/Claude) — `8a1d2d84^{tree}` == `c1fe79fe`; source delta is
  exactly the one Windows-only test file; no manifest/lock/workflow/`command.rs`/`worktree_manager.rs` change. Codex
  marked `evidence_integrity: FAIL` solely on the "16-path" brief-phrasing nit above (partially refuted; not a
  candidate defect).
- **Kimi watch-items carried into 20-53:** PASS. `20-53-PLAN.md` carries the CIM-visibility-under-NetworkService item
  (directly relevant to this finding), `dispatch.rs:604` canonicalize, bash-worker-under-AppContainer + no-Windows-
  Docker-fallback, worker test-exe DLL-load under Low-IL, and the fail-closed post-close PID-reuse race — these ride
  the block into the re-repair, now EXPANDED to include the PowerShell-layer `-ErrorAction Stop` hardening.

---

## Artifact 1 — `f20-native-crossaudit.codex-sol` (Codex 5.6 Sol, external)

```json
{
  "schema": "f20-native-crossaudit.codex-sol",
  "source_sha": "8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e",
  "source_tree": "c1fe79fe4a6d68a536078be4887343a82b5fce38",
  "reviewer_id": "codex-5.6-sol",
  "reviewer_kind": "external-cli",
  "deferred": ["native_macos", "native_windows"],
  "checks": { "all_severity": "FAIL", "evidence_integrity": "FAIL", "integration_authority": "PASS" },
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 1, "low": 1 },
  "observer_1_tagged_cmd_count_failclosed": "FAIL",
  "observer_2_tagged_choice_descendant_pids_failclosed": "FAIL",
  "observer_3_surviving_captured_choice_pids_failclosed": "FAIL",
  "assertions_preserved_verbatim": "PASS",
  "fullfile_sweep_no_third_failopen": "FAIL",
  "finding_2_cmd_classifier_holds": "PASS",
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "evidence": [
    { "claim": "MEDIUM: all three observers still fail open for non-terminating PowerShell/CIM errors. Their scripts omit $ErrorActionPreference='Stop' and -ErrorAction Stop. Windows PowerShell defaults non-terminating errors to Continue, and before PowerShell 7 an @(...) expression resets $? to True; powershell.exe -Command can therefore exit 0 after the failed CIM operation. The two .Count expressions then emit parseable 0, while the PID-list expression emits no tokens and becomes an empty Vec. See Microsoft ErrorActionPreference and Windows PowerShell 5.1 automatic-variable documentation.", "severity": "medium", "result": "FAIL" },
    { "claim": "tagged_cmd_count and surviving_captured_choice_pids can still satisfy wait_until(... == 0) after a non-terminating Get-CimInstance failure because @(...).Count produces 0 and the Rust-side status and parse checks both pass.", "severity": "medium", "result": "FAIL" },
    { "claim": "tagged_choice_descendant_pids can still map a non-terminating CIM failure to an apparently legitimate empty Vec. A failure after an earlier valid peak capture need not invalidate the active-process-cap test.", "severity": "medium", "result": "FAIL" },
    { "claim": "The asserted predecessor-to-source one-file delta is false: git diff f0dd5b6d 8a1d2d84 contains 16 paths, including 15 planning artifacts and the Windows test. Only the direct-parent delta c902b2e5..8a1d2d84 is one file at +53/-10.", "severity": "low", "result": "FAIL" },
    { "claim": "The sealed commit resolves to tree c1fe79fe4a6d68a536078be4887343a82b5fce38 and is an ancestor of plan/f20-unified-audit-repair. Cargo manifests, lockfile, workflows, command.rs, and worktree_manager.rs are unchanged across the stated predecessor range.", "severity": "none", "result": "PASS" },
    { "claim": "The observer-only source diff preserves the existing test assertions, captured-PID logic, empty-PID short-circuit, sibling tests, and best-effort cleanup. The only remaining unwrap_or(0) is unique_tag's SystemTime fallback; filter_map is absent.", "severity": "none", "result": "PASS" },
    { "claim": "resolved_program_is_cmd still compares the exact final path component to cmd.exe, DispatchAdmission capacity enforcement remains present, and the cmd payload change remains confined to the quoting layer.", "severity": "none", "result": "PASS" },
    { "claim": "The stricter Rust parsing does not itself reject legitimate zero .Count results or legitimate empty PID lists; the defect is the missing PowerShell error escalation before those valid-looking results are produced.", "severity": "none", "result": "PASS" }
  ],
  "disposition": "BLOCK"
}
```

## Artifact 2 — `f20-native-crossaudit.gemini-pro` (Gemini 3.1 Pro, external)

```json
{
  "schema": "f20-native-crossaudit.gemini-pro",
  "source_sha": "8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e",
  "source_tree": "c1fe79fe4a6d68a536078be4887343a82b5fce38",
  "reviewer_id": "gemini-3.1-pro",
  "reviewer_kind": "external-cli",
  "deferred": ["native_macos", "native_windows"],
  "checks": { "all_severity": "PASS", "evidence_integrity": "PASS", "integration_authority": "PASS" },
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 0, "low": 0 },
  "observer_1_tagged_cmd_count_failclosed": "PASS",
  "observer_2_tagged_choice_descendant_pids_failclosed": "PASS",
  "observer_3_surviving_captured_choice_pids_failclosed": "PASS",
  "assertions_preserved_verbatim": "PASS",
  "fullfile_sweep_no_third_failopen": "PASS",
  "finding_2_cmd_classifier_holds": "PASS",
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "evidence": [
    { "claim": "All three query observers assert process success and panic on unparseable outputs, closing the fail-open vulnerability without introducing residual blind spots.", "severity": "none", "result": "PASS" },
    { "claim": "Valid zero-results paths (e.g., empty stdout yielding an empty iterator for split_whitespace()) do not panic and correctly return empty collections, confirming no brittle zero-state defects were introduced.", "severity": "none", "result": "PASS" },
    { "claim": "No external files were modified, strictly preserving the finding-2 cmd classifier and isolation guardrails.", "severity": "none", "result": "PASS" }
  ],
  "disposition": "PASS",
  "panel_note": "Gemini did NOT surface the residual PowerShell-layer fail-open (non-terminating CIM error → exit 0 → parseable 0/empty) that Codex + Kimi + the internal Claude leg caught. Its 'closing the fail-open vulnerability without introducing residual blind spots' reasons only about the Rust layer — the panel blind-spot the four-way panel exists to catch."
}
```

## Artifact 3 — `f20-native-crossaudit.kimi-k3` (Kimi K3, external)

```json
{
  "schema": "f20-native-crossaudit.kimi-k3",
  "source_sha": "8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e",
  "source_tree": "c1fe79fe4a6d68a536078be4887343a82b5fce38",
  "reviewer_id": "kimi-k3",
  "reviewer_kind": "external-cli",
  "deferred": ["native_macos", "native_windows"],
  "checks": { "all_severity": "FAIL", "evidence_integrity": "PASS", "integration_authority": "PASS" },
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 0, "low": 1 },
  "observer_1_tagged_cmd_count_failclosed": "PASS",
  "observer_2_tagged_choice_descendant_pids_failclosed": "PASS",
  "observer_3_surviving_captured_choice_pids_failclosed": "PASS",
  "assertions_preserved_verbatim": "PASS",
  "fullfile_sweep_no_third_failopen": "PASS",
  "finding_2_cmd_classifier_holds": "PASS",
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "evidence": [
    { "claim": "Tree binding verified: git rev-parse 8a1d2d84^{tree} == c1fe79fe4a6d68a536078be4887343a82b5fce38; both 8a1d2d84 and f0dd5b6d exist as commits in this checkout.", "severity": "none", "result": "PASS" },
    { "claim": "Code delta is exactly one file (crates/wcore-sandbox/tests/hard_process_containment_windows.rs, +53/-10) with no Cargo.toml/Cargo.lock/.github/command.rs/worktree_manager.rs change. NOTE: the full name-only diff also carries 15 .planning/ markdown artifacts — audit-trail docs, not code; the brief's 'ONE file' is accurate only for the code delta.", "severity": "none", "result": "PASS" },
    { "claim": "Observer 1 tagged_cmd_count (L114-138): asserts out.status.success() with stderr in the panic message (L127-132), then panicking parse via unwrap_or_else (L135-137). No unwrap_or(0) residual. A failed or unparseable .Count query now hard-fails instead of feeding wait_until(==0) at L373/L532/L601.", "severity": "none", "result": "PASS" },
    { "claim": "Observer 2 tagged_choice_descendant_pids (L168-200): status assert (L185-190) plus per-token panicking parse replacing filter_map(...ok()) (L194-198); grep confirms zero filter_map remaining in file. Legitimate empty stdout on success exit still yields empty Vec via split_whitespace — the valid zero-descendants case is preserved, and downstream guards (peak > 0 L455, pids.len() >= cap/2 L432, !captured_pids.is_empty() L469) prevent a vacuous capture from proving anything.", "severity": "none", "result": "PASS" },
    { "claim": "Observer 3 surviving_captured_choice_pids (L220-253): if pids.is_empty() { return 0; } short-circuit preserved verbatim (L221); past it, status assert (L242-247) + panicking parse (L249-252). A post-close query failure can no longer satisfy wait_until(... == 0) at L481.", "severity": "none", "result": "PASS" },
    { "claim": "Assertions preserved verbatim: peak > 0 (L455), peak <= SANDBOX_ACTIVE_PROCESS_LIMIT=512 (L459, const L53), peak < attempts (L464, attempts=cap+32 L397), !captured_pids.is_empty() (L469), exit-0 assert (L448-451), all reap_stray_choice trailers, and sibling tests job_close_reaps_detached_descendant_with_no_residue (L339), breakaway_is_denied (L499), qualified_hard_containment_backend_preflight (L551) — diff hunks touch only the three observer bodies and their doc comments; nothing weakened under cover of hardening.", "severity": "none", "result": "PASS" },
    { "claim": "Full-file sweep: exactly 3x out.status.success() (L128/186/243), exactly 3x panicking parse (L135/195/250), only 4 .output() call sites — the fourth is reap_stray_choice's `let _ = taskkill` (L278-281), intentional best-effort cleanup unchanged. Sole remaining unwrap_or(0) is L98 in unique_tag (UNIX_EPOCH nanos fallback); tag uniqueness is independently carried by process id + AtomicU64 counter + label, so the fallback is benign. No third fail-open observer of the 20-52 class exists.", "severity": "none", "result": "PASS" },
    { "claim": "Guardrails hold by absence of change: name-only diff contains no command.rs (Finding-2 resolved_program_is_cmd classifier), no worktree_manager.rs (#A DispatchAdmission budget), no quoting-layer (#B) files, no workflow files. This one-file test delta cannot have altered them.", "severity": "none", "result": "PASS" },
    { "claim": "No new wrong-panic defect: @(...).Count emits a bare integer on any success exit (0 for an empty set), so the panicking parse cannot fire on a legitimate zero; observer 2's empty-success case yields an empty Vec by construction; observer 3's legitimate empty-set case returns 0 before any query is issued.", "severity": "none", "result": "PASS" },
    { "claim": "RESIDUAL (low): on the success path none of the three observers inspects stderr. A CIM query that emits a NON-terminating error record (stderr) while powershell -Command still exits 0 yields stdout '0' (Count observers) or an empty token stream (PID observer) — both parse cleanly, so wait_until(observer==0) at L373/L481/L532/L601 can still be satisfied without evidence. This is a narrower survivor of the exact 20-52 class ('broken query read as 0 survivors'): the hard failure modes (spawn failure, non-zero exit, empty/malformed stdout) are closed, but the exit-0-with-error-record mode is not. The downstream peak/capture asserts protect observer 2's capture path but NOT the .Count reap checks. Trigger requires WMI/CIM to partially fail with exit 0 — not runtime-verifiable here (native_windows deferred) — hence low, not higher. Trivial fix: add -ErrorAction Stop to each Get-CimInstance (any error becomes terminating => exit 1 => caught by the new assert), or assert stderr is empty on the success path.", "severity": "low", "result": "FAIL" }
  ],
  "disposition": "BLOCK",
  "panel_note": "Kimi — which MISSED the 20-52 Rust-layer fail-open — independently CAUGHT this deeper PowerShell-layer instance and converged with Codex on the exact -ErrorAction Stop fix."
}
```

## Artifact 4 — `wayland-core.phase20-independent-review.v1` (internal Claude non-author adversarial)

```json
{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e",
  "source_tree": "c1fe79fe4a6d68a536078be4887343a82b5fce38",
  "source_executor_id": "wayland-f20-native-repair-builder",
  "reviewer_id": "wayland-f20-59-claude-adversarial",
  "reviewer_kind": "internal-claude-adversarial",
  "checks": { "all_severity": "FAIL", "evidence_integrity": "PASS", "integration_authority": "PASS" },
  "deferred": ["native_macos", "native_windows"],
  "findings": { "blocker": 0, "critical": 0, "high": 0, "medium": 0, "low": 1 },
  "observer_1_tagged_cmd_count_failclosed": "FAIL",
  "observer_2_tagged_choice_descendant_pids_failclosed": "FAIL",
  "observer_3_surviving_captured_choice_pids_failclosed": "FAIL",
  "assertions_preserved_verbatim": "PASS",
  "fullfile_sweep_no_third_failopen": "PASS",
  "finding_2_cmd_classifier_holds": "PASS",
  "guardrail_A_admission_budget": "PASS",
  "guardrail_B_quoting_layer_only": "PASS",
  "kimi_watchitems_carried": "PASS",
  "evidence": [
    { "command": "SHA/tree bind: git rev-parse 8a1d2d84^{tree} == c1fe79fe4a6d68a536078be4887343a82b5fce38 (verified); source delta (non-planning) git diff --name-only f0dd5b6d 8a1d2d84 | grep -v planning == exactly crates/wcore-sandbox/tests/hard_process_containment_windows.rs; direct-parent c902b2e5..8a1d2d84 == same one file (+53/-10); no Cargo.toml/Cargo.lock/workflow/command.rs/worktree_manager.rs change", "exit_code": 0, "result": "PASS" },
    { "command": "Rust-layer hardening CONFIRMED: out.status.success() x3 (L128/186/243); panicking parse unwrap_or_else(|err| panic!) x3 (L135/195/250); filter_map ABSENT; sole unwrap_or(0) is L98 unique_tag nanos (benign). The 20-52 Rust-layer fail-open is genuinely closed", "exit_code": 0, "result": "PASS" },
    { "command": "LOW (non-deferred, real, non-refutable): RESIDUAL PowerShell-LAYER fail-open. The three -Command scripts set NO -ErrorAction Stop / $ErrorActionPreference='Stop' / trap (grep over sealed file: none). A non-terminating Get-CimInstance error leaves powershell.exe exiting 0 (WinPS 5.x @(...) resets $? to True), @(...).Count prints '0' and the PID query prints no tokens. So out.status.success()==TRUE, the panicking parse succeeds on '0'/empty, and wait_until(observer==0,30) is satisfied without evidence — the exact 20-52 class moved Rust->PowerShell. The 20-57 doc comment ('a post-close query failure therefore cannot satisfy a reap wait_until(...==0) without evidence') overclaims. Not refutable (cannot run Windows; documented semantics show exit 0 + '0'/empty), so concurred not refuted. Rated LOW (strictly better than the predecessor Rust-layer fail-open; requires an actual exit-0 partial CIM failure) but real and non-deferred. Fix: add -ErrorAction Stop to each Get-CimInstance so any CIM error becomes terminating => non-zero exit => caught by the existing assert!(out.status.success())", "exit_code": 0, "result": "FAIL" },
    { "command": "Convergence: Codex 5.6 Sol (MEDIUM) and Kimi K3 (LOW) independently reached the same finding; Kimi named the same one-line -ErrorAction Stop fix. Gemini missed it (Rust-layer PASS) — the four-way panel blind-spot dynamic (cf. 20-52). Concrete trigger: the carried 'tagged_cmd_count CIM visibility under NetworkService' watch-item is precisely a restricted-token access/provider error, the non-terminating class that exits 0", "exit_code": 0, "result": "FAIL" },
    { "command": "Assertions preserved verbatim: if pids.is_empty(){return 0;} (L221), peak>0 (L455), peak<=SANDBOX_ACTIVE_PROCESS_LIMIT=512 (L459), peak<attempts (L464), !captured_pids.is_empty() (L469), exit-0 asserts, reap_stray_choice best-effort trailers; sibling job_close_reaps_detached_descendant_with_no_residue (L339)/breakaway_is_denied (L499)/qualified_hard_containment_backend_preflight (L551) untouched — nothing weakened under cover of hardening", "exit_code": 0, "result": "PASS" },
    { "command": "Finding-2 command.rs cmd classifier, Guardrail A (worktree_manager.rs DispatchAdmission budget), Guardrail B (quoting-layer-only / boundary primitives) hold by absence of change: none are in the one-file test delta", "exit_code": 0, "result": "PASS" },
    { "command": "Codex LOW 'one-file delta' evidence nit partially REFUTED: git diff f0dd5b6d..8a1d2d84 lists 16 paths because planning-doc commits interleave, but the SOURCE delta restricted to non-planning is exactly one file (verified); the candidate's code repair IS a clean one-file delta (Codex + Kimi concur). A brief-phrasing clarification, not a candidate defect; does not affect disposition", "exit_code": 0, "result": "PASS" },
    { "command": "Kimi watch-items carried into 20-53: 20-53-PLAN.md carries CIM-visibility-under-NetworkService (directly relevant to this finding), dispatch.rs:604 canonicalize, bash-worker-under-AppContainer + no-Windows-Docker-fallback, worker test-exe DLL-load under Low-IL, and the fail-closed post-close PID-reuse race — ride the block into the re-repair, now EXPANDED to include the PowerShell-layer -ErrorAction Stop hardening", "exit_code": 0, "result": "PASS" },
    { "command": "Panel reachability: all three external CLIs (Codex 5.6 Sol, Gemini 3.1 Pro, Kimi K3) reachable, exit 0, schema-valid JSON keyed to 8a1d2d84 — the fail-closed unreachable/partial-panel path was NOT triggered; this is a finding-based BLOCK", "exit_code": 0, "result": "PASS" }
  ],
  "disposition": "BLOCK"
}
```

---

## Routing

**BLOCK → a further repaired successor** must harden the reap-observer PowerShell scripts to FAIL CLOSED on a
non-terminating CIM error: add `-ErrorAction Stop` to each `Get-CimInstance` (and/or `$ErrorActionPreference='Stop'` at
the top of each `-Command` script), so any CIM failure becomes a terminating error → `powershell.exe` exits non-zero →
the existing `assert!(out.status.success())` fires; optionally also assert `out.stderr` is empty on the success path.
Apply to all three observers (`tagged_cmd_count`, `tagged_choice_descendant_pids`, `surviving_captured_choice_pids`).
Touch ONLY the three observer script bodies; the Rust-layer hardening, every assertion, the empty-set short-circuit, the
sibling tests, the Finding-2 cmd classifier, and both isolation guardrails (#A/#B) are confirmed sound and must be
preserved verbatim. Then re-seal (confirm zero-lock-delta + `--locked` build + Linux aggregate floor) and re-run this
four-way pre-native cross-audit to a zero-finding ALL-FOUR PASS before the rebound 20-53 is authorized. All prior
native-proof authorizations remain spent/void; the scarce native run is NOT re-spent on this candidate.

**Layered-finding lineage:** 20-45 blocked on structural reap vacuity → 20-50 closed it → 20-52 blocked on the
Rust-layer observer fail-open → 20-57 closed the Rust layer → **20-59 blocks on the PowerShell-layer fail-open
underneath**. Each fix closed one layer and the next-deeper fail-open surfaced — exactly the class of miss a four-way
panel exists to catch. Kimi, which missed the 20-52 Rust-layer instance, independently caught this deeper one.
