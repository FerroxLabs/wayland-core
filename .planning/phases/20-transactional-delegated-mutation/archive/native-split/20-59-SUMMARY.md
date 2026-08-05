---
phase: 20-transactional-delegated-mutation
plan: "59"
subsystem: infra
tags: [native-repair, pre-native-cross-audit, four-way-panel, fail-closed, reap-observer-false-green, powershell-layer-fail-open, erroraction-stop, finding-based-block, honest-deferral]

# Dependency graph
requires:
  - phase: "20-58"
    provides: "SEALED further-repaired-successor candidate source_sha 8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e (tree c1fe79fe4a6d68a536078be4887343a82b5fce38): the 20-57 fail-closed hardening of all three host-side reap-observers, zero-lock-delta confirmed, Linux floor 11509/0/48. The exact SHA every downstream gate binds to."
provides:
  - "FOUR per-reviewer schema-validated pre-native cross-audits over the re-sealed successor 8a1d2d84 (tree c1fe79fe): Codex 5.6 Sol f20-native-crossaudit.codex-sol (BLOCK, 1 MEDIUM + 1 LOW), Gemini 3.1 Pro f20-native-crossaudit.gemini-pro (PASS, 0 findings), Kimi K3 f20-native-crossaudit.kimi-k3 (BLOCK, 1 LOW), and the internal Claude adversarial wayland-core.phase20-independent-review.v1 (BLOCK, 1 LOW). All three external CLIs reachable, exit 0, schema-valid — a FINDING-based BLOCK, not an incomplete-panel BLOCK. Raw outputs preserved under 20-59-raw/."
  - "GATE DISPOSITION: BLOCK — does NOT admit the rebound 20-53 native gate. Three of four auditors (Codex + Kimi + Claude adversarial) independently raise the SAME real, non-deferred, statically-provable finding: a residual PowerShell-LAYER fail-open. The 20-57 fix hardened the RUST layer (out.status.success() + panicking parse, no unwrap_or(0), no filter_map) but the three -Command CIM scripts set NO -ErrorAction Stop / $ErrorActionPreference='Stop' / trap, so a non-terminating Get-CimInstance error leaves powershell.exe exiting 0 with @(...).Count == '0' (or an empty PID token stream) — out.status.success() is TRUE, the panicking parse succeeds on '0'/empty, and wait_until(observer==0) is satisfied WITHOUT evidence. This is the exact 20-52 fail-open class moved one layer down (Rust -> PowerShell). Gemini missed it (Rust-layer-only PASS); Kimi — which missed the 20-52 Rust-layer instance — independently caught this deeper one and converged with Codex on the exact -ErrorAction Stop fix."
  - "Routing for the further-repaired successor: add -ErrorAction Stop to each Get-CimInstance (and/or $ErrorActionPreference='Stop' atop each -Command script) so any CIM error becomes terminating => powershell.exe exits non-zero => caught by the existing assert!(out.status.success()); optionally assert out.stderr empty on the success path. Apply to all three observers; preserve the Rust-layer hardening, every assertion, the empty-set short-circuit, the sibling tests, the Finding-2 cmd classifier, and both #A/#B guardrails verbatim. Then re-seal and re-run this four-way pre-native cross-audit to a zero-finding ALL-FOUR PASS before 20-53 is authorized."
affects: ["20-60", "20-53", "20-54", "20-55", "20-56"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "A reap/containment observer is not fail-closed until BOTH layers fail closed: the Rust layer (assert out.status.success() + panicking parse) AND the query-script layer (the CIM/PowerShell query must escalate its own errors to a non-zero process exit via -ErrorAction Stop / $ErrorActionPreference='Stop'). Hardening only the Rust layer leaves the exit-0-with-non-terminating-error-record mode fail-open: @(<failed Get-CimInstance>).Count prints '0' and powershell.exe still exits 0, so the Rust status/parse checks both pass and wait_until(==0) is satisfied without evidence."
    - "The layered-finding lineage confirms the pre-native four-way gate re-prosecutes the SAME finding surface one layer deeper after each fix: 20-45 structural reap vacuity -> 20-50 fix -> 20-52 Rust-layer observer fail-open -> 20-57 fix -> 20-59 PowerShell-layer fail-open. A four-INDEPENDENT-auditor panel raises the cost of a shared blind spot: the auditor that missed the prior layer (Kimi at 20-52) caught the next one."

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-59-CROSS-AUDIT.md
    - .planning/phases/20-transactional-delegated-mutation/20-59-SUMMARY.md
    - .planning/phases/20-transactional-delegated-mutation/20-59-raw/audit-context.shared.txt
    - .planning/phases/20-transactional-delegated-mutation/20-59-raw/codex-sol.raw.txt
    - .planning/phases/20-transactional-delegated-mutation/20-59-raw/gemini-pro.raw.txt
    - .planning/phases/20-transactional-delegated-mutation/20-59-raw/kimi-k3.raw.txt
    - .planning/phases/20-transactional-delegated-mutation/20-59-raw/claude-adversarial.raw.txt
  modified: []

key-decisions:
  - "FINDING-based BLOCK, not a false-green PASS and not an incomplete-panel BLOCK. All three external CLIs (Codex 5.6 Sol, Gemini 3.1 Pro, Kimi K3) were reachable, exit 0, and returned schema-valid JSON keyed to the exact re-sealed source_sha 8a1d2d84, so the fail-closed unreachable/partial-panel path was NOT triggered. Under ALL-FOUR-MUST-PASS, the finding from Codex + Kimi + the Claude leg blocks."
  - "The finding is real, statically-provable, and NOT refutable: verified against the sealed tree that the three -Command scripts contain no -ErrorAction Stop / $ErrorActionPreference='Stop' / trap (grep: none), and Windows PowerShell leaves non-terminating Get-CimInstance errors as exit-0 with @(...).Count=='0'. No executable counter-evidence of fail-closed behavior is possible (the file is #![cfg(windows)]; macOS/Linux cannot run it; the documented semantics show fail-open). The Claude leg, prompted to REFUTE (default-refuted-if-uncertain), therefore CONCURS rather than refuting — the initial Rust-layer-only read (which would have PASSed, mirroring Gemini) is corrected by the deeper prosecution."
  - "Codex LOW 'one-file delta' evidence nit PARTIALLY REFUTED: git diff f0dd5b6d..8a1d2d84 lists 16 paths because planning-doc commits interleave, but the SOURCE delta restricted to non-planning is exactly one file (verified: git diff --name-only f0dd5b6d 8a1d2d84 | grep -v planning == the one test file; direct-parent c902b2e5..8a1d2d84 == same one file). Both Codex and Kimi concur the CODE delta is one file. A brief-phrasing clarification, not a candidate defect; does not affect the disposition (the PowerShell-layer finding already blocks)."
  - "The Rust-layer 20-57 hardening is genuinely sound and preserved (4/4 legs): out.status.success() x3, panicking parse x3, filter_map absent, sole unwrap_or(0) the benign unique_tag nanos, all assertions + empty-set short-circuit + three sibling tests verbatim, command.rs / #A / #B untouched. The BLOCK is the PowerShell layer BELOW this, not a regression in the Rust layer."
  - "Kimi watch-items carried into 20-53 (verified): CIM-visibility-under-NetworkService (directly relevant — a restricted-token access/provider error is exactly the non-terminating class that exits 0), dispatch.rs:604 canonicalize, bash-worker-under-AppContainer + no-Windows-Docker-fallback, worker test-exe DLL-load under Low-IL, and the fail-closed post-close PID-reuse race. These ride the block into the re-repair, now EXPANDED to include the PowerShell-layer -ErrorAction Stop hardening."
  - "requirements-completed is [] — NO Phase-20 requirement is claimed. REQ-native-r12 (pre-native gate) and REQ-native-r13 (per-reviewer schema-validated artifacts) are the surfaces this plan writes toward; the gate ran and produced four artifacts, but its disposition is BLOCK, so no requirement is completed and the native run is not authorized. A further-repaired successor + re-seal + re-audit is required."

patterns-established:
  - "Fail-closed is a two-layer property for any test observer that shells out to a query tool: harden the host language layer (check process exit + panicking parse) AND make the query tool escalate its own errors to a non-zero exit (-ErrorAction Stop / set -e / equivalent). Hardening only the host layer leaves the query-exits-0-on-partial-failure mode fail-open."

requirements-completed: []  # NO Phase-20 requirement claimed. Gate disposition is BLOCK; native run not authorized.

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "Four per-reviewer schema-validated pre-native cross-audits recorded over the exact re-sealed successor 8a1d2d84 (tree c1fe79fe): Codex Sol f20-native-crossaudit.codex-sol, Gemini Pro f20-native-crossaudit.gemini-pro, Kimi K3 f20-native-crossaudit.kimi-k3, internal Claude adversarial wayland-core.phase20-independent-review.v1. All three external CLIs reachable/exit-0/schema-valid; raw outputs preserved under 20-59-raw/ (REQ-native-r13)."
    requirement: "REQ-native-r13"
    verification:
      - kind: integration
        ref: "codex exec (exit 0), GEMINI_CLI_TRUST_WORKSPACE=true gemini --skip-trust (exit 0), /Users/seandonahoe/.kimi-code/bin/kimi -p (exit 0); four keyed JSON artifacts + four raw sidecars on disk"
        status: pass
    human_judgment: false
  - id: D2
    description: "The pre-native gate adversarially prosecuted the 20-57 observer-hardening delta at every non-deferred severity and returned BLOCK: three of four auditors (Codex MEDIUM+LOW, Kimi LOW, Claude LOW) raised the residual PowerShell-layer fail-open; native_macos/native_windows the only deferred checks (REQ-native-r12)."
    requirement: "REQ-native-r12"
    verification:
      - kind: unit
        ref: "grep over sealed tree: no -ErrorAction Stop / $ErrorActionPreference / trap in the three observer scripts; non-terminating Get-CimInstance error => powershell exit 0 + @().Count=='0' => wait_until(==0) satisfied without evidence"
        status: pass
      - kind: manual_procedural
        ref: "Windows real-hardware fail-open behavior of the CIM query under a restricted token is native_windows-deferred to 20-53; the finding is a static-semantics claim, not a runtime claim"
        status: unknown
    human_judgment: true
    rationale: "The gate ran and produced a disposition (BLOCK); the finding is a statically-provable PowerShell-semantics fact, but its concrete runtime trigger (CIM partial-failure under NetworkService) is native_windows-deferred. The disposition itself does not complete a requirement — it routes to a further-repaired successor."

# Metrics
duration: ~30min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 59: Four-way pre-native cross-audit of the re-sealed successor 8a1d2d84 — FINDING-based BLOCK (PowerShell-layer fail-open) Summary

**The FOUR-WAY pre-native panel — Codex 5.6 Sol + Gemini 3.1 Pro + Kimi K3 (three external CLIs run in parallel) + an internal Claude non-author adversarial reviewer — cross-audited the exact re-sealed further-repaired successor `source_sha 8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e` / tree `c1fe79fe` at every non-deferred severity across the 20-57 observer-hardening delta, and returned BLOCK. Three of four auditors (Codex MEDIUM+LOW, Kimi LOW, internal Claude LOW) independently raised the SAME real, non-deferred, statically-provable finding: a residual PowerShell-LAYER fail-open. The 20-57 fix genuinely closed the RUST-layer fail-open (each observer now asserts `out.status.success()` and parses with a panicking fallback; no `unwrap_or(0)`, no `filter_map(...ok())`), but the three `-Command` CIM scripts set NO `-ErrorAction Stop` / `$ErrorActionPreference='Stop'` / `trap`, so a non-terminating `Get-CimInstance` error leaves `powershell.exe` exiting 0 with `@(...).Count == '0'` (or an empty PID token stream) — `out.status.success()` is TRUE, the panicking parse succeeds on `'0'`/empty, and `wait_until(observer == 0)` is satisfied WITHOUT evidence. This is the exact 20-52 fail-open class moved one layer down (Rust → PowerShell). Gemini missed it (Rust-layer-only PASS); Kimi — which missed the 20-52 Rust-layer instance — independently caught this deeper one and converged with Codex on the exact `-ErrorAction Stop` fix. All three external CLIs were reachable, exit 0, and schema-valid keyed to `8a1d2d84`, so this is a FINDING-based BLOCK, not an incomplete-panel BLOCK. Each auditor left its own schema-validated artifact; raw outputs are preserved under `20-59-raw/`. No source/test/workflow file was modified; NO Phase-20 requirement is claimed. The rebound 20-53 native gate is NOT admitted; a further-repaired successor must harden the PowerShell layer, re-seal, and re-audit to a zero-finding ALL-FOUR PASS.**

## Audited candidate identity

| Field | Value |
|-------|-------|
| Re-sealed successor (`source_sha`) | `8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e` |
| Re-sealed successor tree | `c1fe79fe4a6d68a536078be4887343a82b5fce38` (verified `8a1d2d84^{tree}` == this) |
| Predecessor (20-52-BLOCKED, sealed) | `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2` (tree `ac76c87b318ee4ba8c34927dea23e40e63fd0776`) |
| Branch | `plan/f20-unified-audit-repair` (standalone checkout `/Users/seandonahoe/dev/waylandcore-ferrox`, `.git` = directory, all git ops `/usr/bin/git`) |
| Source repair delta prosecuted | `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` (+53/−10, one `#![cfg(windows)]` file) |
| Deferred (only) | `native_macos`, `native_windows` (proven at the rebound 20-53) |

## Four reviewer identities and artifacts

| # | Reviewer | Kind | Schema key | Schema-validated | Disposition | b/c/h/m/l |
|---|----------|------|-----------|-----------------|-------------|-----------|
| 1 | Codex 5.6 Sol | external-cli | `f20-native-crossaudit.codex-sol` | YES | **BLOCK** | 0/0/0/1/1 |
| 2 | Gemini 3.1 Pro | external-cli | `f20-native-crossaudit.gemini-pro` | YES | PASS | 0/0/0/0/0 |
| 3 | Kimi K3 | external-cli | `f20-native-crossaudit.kimi-k3` | YES | **BLOCK** | 0/0/0/0/1 |
| 4 | internal Claude adversarial | internal-claude | `wayland-core.phase20-independent-review.v1` | YES | **BLOCK** | 0/0/0/0/1 |

## External CLI invocations (verbatim)

| Auditor | Invocation | Exit | Reachable |
|---|---|---|---|
| Codex 5.6 Sol | `codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "<prompt>"` (brief on stdin) | 0 | YES |
| Gemini 3.1 Pro | `GEMINI_CLI_TRUST_WORKSPACE=true gemini -p "<prompt>" -m gemini-3.1-pro-preview -o text --approval-mode plan --skip-trust` | 0 | YES |
| Kimi K3 | `/Users/seandonahoe/.kimi-code/bin/kimi -p "<prompt>" --output-format text` (absolute path, brief in `-p`) | 0 | YES |

## The finding (real, non-deferred, non-refutable)

**Residual PowerShell-layer fail-open in all three reap observers.** The 20-57 fix hardened the Rust layer only. The
three `-Command` CIM query scripts run `@(Get-CimInstance Win32_Process ...).Count` (and the PID-list variant) with no
error-action escalation. Under Windows PowerShell a `Get-CimInstance` failure is a non-terminating error by default:
the pipeline continues, `@(<failed pipeline>)` is an empty array so `.Count` prints `"0"` (the PID query prints no
tokens), and `powershell.exe -Command` exits 0 (in WinPS 5.x `@(...)` resets `$?` to `True`). So:

- `out.status.success()` == TRUE → the new assert does not fire;
- stdout `"0"`/empty → the panicking parse succeeds;
- `wait_until(|| observer == 0, 30)` is satisfied on the first poll → the reap property is reported proven with no
  evidence.

Verified statically against the sealed tree: `grep -E 'ErrorAction|ErrorActionPreference|-Stop|trap '` over the three
observers returns nothing. The 20-57 doc comment ("a post-close query failure therefore cannot satisfy a reap
`wait_until(... == 0)` without evidence") overclaims — it holds for a non-zero exit or unparseable stdout, but not for
the exit-0-with-error-record mode. Concrete trigger: the carried "`tagged_cmd_count` CIM visibility under
NetworkService" watch-item is precisely a restricted-token access/provider error, the non-terminating class that exits 0.

**Fix for the successor:** add `-ErrorAction Stop` to each `Get-CimInstance` (and/or `$ErrorActionPreference='Stop'`
atop each `-Command` script) so any CIM error becomes terminating → `powershell.exe` exits non-zero → caught by the
existing `assert!(out.status.success())`; optionally assert `out.stderr` empty on the success path. Apply to all three
observers; preserve the Rust-layer hardening, every assertion, the empty-set short-circuit, the sibling tests, the
Finding-2 cmd classifier, and both #A/#B guardrails verbatim.

## Confirmed sound (4/4 legs) — preserved, not the source of the block

- Rust-layer hardening: `out.status.success()` ×3, panicking parse ×3, `filter_map` absent, sole `unwrap_or(0)` the
  benign `unique_tag` nanos, `reap_stray_choice` best-effort unchanged.
- Assertions preserved verbatim: `if pids.is_empty(){return 0;}`, `peak > 0`, `peak <= SANDBOX_ACTIVE_PROCESS_LIMIT`,
  `peak < attempts`, `!captured_pids.is_empty()`, exit-0 asserts; siblings
  `job_close_reaps_detached_descendant_with_no_residue` / `breakaway_is_denied` /
  `qualified_hard_containment_backend_preflight` untouched.
- Finding-2 `command.rs` cmd classifier, Guardrail #A (`worktree_manager.rs` DispatchAdmission budget), Guardrail #B
  (quoting-layer-only) hold by absence of change — none are in the one-file test delta.

## Refuted / clarified

- **Codex LOW "one-file delta" evidence nit — partially refuted.** `git diff f0dd5b6d..8a1d2d84` lists 16 paths because
  planning-doc commits interleave; the SOURCE delta restricted to non-planning is exactly one file (verified:
  `git diff --name-only f0dd5b6d 8a1d2d84 | grep -v planning` == the one test file; direct-parent `c902b2e5..8a1d2d84`
  == same one file). Both Codex and Kimi concur the code delta is one file. A brief-phrasing clarification, not a
  candidate defect; does not affect the disposition.

## GATE DISPOSITION: BLOCK — does NOT admit the rebound 20-53

Under ALL-FOUR-MUST-PASS, any single non-deferred finding from any one auditor blocks. Codex + Kimi + the internal
Claude leg raise the same real PowerShell-layer fail-open; Gemini's zero-finding PASS missed it. The scarce Sean-gated
native run is NOT re-spent on this candidate. Routing: a further-repaired successor hardens the PowerShell layer
(`-ErrorAction Stop`), re-seals (zero-lock-delta + `--locked` build + Linux aggregate floor), and re-runs this four-way
pre-native cross-audit to a zero-finding ALL-FOUR PASS before 20-53 is authorized.

## Raw-output sidecar

`.planning/phases/20-transactional-delegated-mutation/20-59-raw/`: `audit-context.shared.txt` (the shared brief),
`codex-sol.raw.txt`, `gemini-pro.raw.txt`, `kimi-k3.raw.txt`, `claude-adversarial.raw.txt`.

## Deviations from Plan

None — plan executed exactly as written (a four-way pre-native cross-audit that writes only the cross-audit artifact +
summary + raw sidecar). The plan's expected happy-path was an ALL-FOUR PASS admitting 20-53; the honest outcome is a
finding-based BLOCK, which the plan's disposition rules explicitly provide for (any single finding → block → further
repaired successor). No source/test/workflow file was modified. The pre-existing untouched `AGENTS.md` ijfw-memory
modification in the working tree was left exactly as-is (never staged), per the task instruction.

## Self-Check: PASSED

- `.planning/phases/20-transactional-delegated-mutation/20-59-CROSS-AUDIT.md` — created (four keyed artifacts).
- `.planning/phases/20-transactional-delegated-mutation/20-59-SUMMARY.md` — created.
- `.planning/phases/20-transactional-delegated-mutation/20-59-raw/` — five files preserved (shared brief + four raw).
- Re-sealed `source_sha 8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e` / tree `c1fe79fe4a6d68a536078be4887343a82b5fce38`
  verified (`git rev-parse 8a1d2d84^{tree}`).
- All three external CLIs reachable, exit 0, schema-valid keyed to `8a1d2d84`; finding-based BLOCK (not incomplete-panel).

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
