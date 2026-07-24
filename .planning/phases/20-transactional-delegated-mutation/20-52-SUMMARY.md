---
phase: 20-transactional-delegated-mutation
plan: "52"
subsystem: infra
tags: [native-repair, pre-native-gate, four-way-panel, cross-audit, fail-closed, all-must-pass, finding-based-block, reap-observer-false-green, unwrap-or-zero, further-repair-routing, honest-deferral]

# Dependency graph
requires:
  - phase: "20-51"
    provides: "SEALED further-repaired-successor candidate source_sha f0dd5b6d312af616f268f96f34c3bc9fc962c4d2 (tree ac76c87b318ee4ba8c34927dea23e40e63fd0776) — zero-lock-delta confirmed, Linux no-regression floor 11509/0/48."
provides:
  - "FOUR per-reviewer schema-validated pre-native cross-audit artifacts over the exact re-sealed successor f0dd5b6d (tree ac76c87b): Codex Sol f20-native-crossaudit.codex-sol (BLOCK), Gemini Pro f20-native-crossaudit.gemini-pro (PASS), Kimi K3 f20-native-crossaudit.kimi-k3 (PASS), internal Claude adversarial wayland-core.phase20-independent-review.v1 (BLOCK). Raw outputs preserved in 20-52-raw/."
  - "GATE DISPOSITION: BLOCK — does NOT admit 20-53. Two of four auditors (Codex 5.6 Sol + internal Claude adversarial) raise a real, statically-provable, non-deferred finding against the 20-50 Finding-1 fix: the new post-close reap OBSERVER surviving_captured_choice_pids ends in parse().unwrap_or(0) with no out.status.success() check, so a post-close CIM/PowerShell query failure returns 0 and satisfies the reap assertion WITHOUT evidence (residual false-green on the exact fixed property). The new !captured_pids.is_empty() guard covers the alive capture phase only. Under ALL-FOUR-MUST-PASS this blocks and routes to a further repaired successor."
  - "CONFIRMED SOUND (all four legs concur): Finding 2 cmd exact-final-component classifier (file_name()==cmd.exe, quoting fns byte-identical); Guardrail A DispatchAdmission budget (worktree_manager.rs untouched); Guardrail B quoting-layer-only (is_unc_or_device_path/Low-IL/Job/ACL/argv/exit-b-0 untouched); the STRUCTURAL half of Finding 1 (captured-PID reap is non-vacuous and does not reintroduce the host-wide-image-count flake); windows_impl/tests.rs byte-identical to predecessor (Kimi's 19 lib-test cross-check errors are pre-existing native_windows-deferred feature-unification artifacts, out of delta scope)."
affects: ["20-53", "20-54", "20-55", "20-56"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "A reap/containment assertion whose host-side observer maps a failed query to a passing value (parse().unwrap_or(0) with no status.success() check) has a statically-provable false-green path even after the STRUCTURAL vacuity is fixed — the observer must fail CLOSED (assert query success, panic on unparseable output) for the assertion to be sound."
    - "Four-independent-auditor panel value re-demonstrated: Codex + Claude caught the residual observer false-green that Gemini + Kimi both missed (same blind-spot shape as 20-45), and the ALL-FOUR-MUST-PASS + fail-closed rule converts a single real finding into a BLOCK rather than a 2-of-4 majority PASS."

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-52-CROSS-AUDIT.md
    - .planning/phases/20-transactional-delegated-mutation/20-52-SUMMARY.md
    - .planning/phases/20-transactional-delegated-mutation/20-52-raw/audit-context.shared.txt
    - .planning/phases/20-transactional-delegated-mutation/20-52-raw/codex-sol.raw.txt
    - .planning/phases/20-transactional-delegated-mutation/20-52-raw/gemini-pro.raw.txt
    - .planning/phases/20-transactional-delegated-mutation/20-52-raw/kimi-k3.raw.txt
  modified: []

key-decisions:
  - "GATE = BLOCK on a REAL finding, not rationalized away and not a refutable false positive. Codex 5.6 Sol's MEDIUM (Claude leg concurs at LOW) is a statically-provable code fact verified against the sealed tree: surviving_captured_choice_pids in hard_process_containment_windows.rs ends in String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0) with NO out.status.success() check (.output().expect only catches spawn failure). On a post-close CIM/PowerShell query failure (empty/malformed stdout) it returns 0, satisfying wait_until(...==0) on the first poll — the reap property is reported proven without evidence. The 20-50 fix closes the GUARANTEED structural vacuity the 20-45 panel blocked on, but leaves this NARROWER false-green on the same fixed property; the identical pattern in the sibling tagged_cmd_count observer weakens the 20-45 robust-sibling mitigation. It is NOT refutable (unwrap_or(0) demonstrably returns 0 on empty stdout), so the Claude leg concurred rather than refuting."
  - "Codex's LOW PID-reuse race is recorded as a fail-closed 20-53 watch-item, not an independent blocker — recycling a captured PID to a concurrent choice.exe can only make the test FAIL (never false-green) and is low-probability; the successor should prefer creation-time/handle identity if convenient."
  - "This is a FINDING-based BLOCK, not an incomplete-panel/fail-closed-unreachable BLOCK: all three external CLIs (Codex Sol, Gemini Pro, Kimi K3) were reachable, exited 0, and returned schema-valid JSON keyed to the exact re-sealed source_sha f0dd5b6d with their assigned schema keys. Each of the four auditors emitted its own on-disk schema-validated artifact; no prose-only vote was counted. Raw outputs preserved in 20-52-raw/."
  - "The Finding-2 cmd classifier, both isolation guardrails (#A/#B), and every other containment assertion (exit_code==0, peak>0, peak<=512, peak<attempts, the sibling job_close_reaps_detached_descendant_with_no_residue) are confirmed SOUND by all four legs and must be preserved verbatim by the further-repaired successor. Only the reap-observer error handling needs hardening."
  - "requirements-completed is [] — NO Phase-20 requirement is claimed. REQ-native-r12/r13 are the pre-native gate this plan implements; because the gate BLOCKS, it neither completes a requirement nor admits the 20-53 native re-dispatch. No source/test/workflow file was modified; this plan wrote only the cross-audit artifact, its raw sidecar, and this summary."

patterns-established:
  - "The pre-native four-way gate re-runs after every finding-fix and re-prosecutes the SAME finding surface at a deeper layer: closing the structural vacuity surfaced the residual observer-error-handling false-green underneath it. Fixing a containment assertion is not done until its host-side observer also fails closed."

requirements-completed: []  # NO Phase-20 requirement claimed. Gate BLOCKED; 20-53 native re-dispatch NOT admitted.

# Coverage metadata (#1602)
coverage:
  - id: G1
    description: "Four per-reviewer schema-validated pre-native cross-audit artifacts over the exact re-sealed successor f0dd5b6d/ac76c87b, each auditor emitting its own keyed artifact with raw output preserved (REQ-native-r13)."
    requirement: "REQ-native-r13"
    verification:
      - kind: integration
        ref: "20-52-CROSS-AUDIT.md — codex-sol/gemini-pro/kimi-k3/phase20-independent-review artifacts + 20-52-raw/ sidecars"
        status: pass
    human_judgment: false
  - id: G2
    description: "Pre-native gate disposition determined by the all-four-must-pass + fail-closed rule: Codex Sol BLOCK (real reap-observer false-green) => GATE BLOCK, does not admit the Sean-gated native re-dispatch (REQ-native-r12)."
    requirement: "REQ-native-r12"
    verification:
      - kind: integration
        ref: "GATE DISPOSITION: BLOCK — Codex 5.6 Sol + internal Claude adversarial findings; 20-53 not admitted"
        status: pass
    human_judgment: true

# Metrics
duration: ~40min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 52: Four-way pre-native cross-audit of the re-sealed further-repaired successor — GATE BLOCK Summary

**The FOUR-WAY pre-native panel (Codex 5.6 Sol + Gemini 3.1 Pro + Kimi K3 external, run in parallel, plus the internal Claude non-author adversarial leg) cross-audited the exact re-sealed successor `f0dd5b6d` (tree `ac76c87b`) across the 20-50 repair delta. Each auditor emitted its own schema-validated artifact; all three externals were reachable, exited 0, and returned schema-valid JSON, so the fail-closed unreachable path was NOT triggered. GATE DISPOSITION = BLOCK: Codex 5.6 Sol (MEDIUM) and the internal Claude adversarial leg (LOW, concurring) raise a real, statically-provable, non-deferred finding — the new post-close reap OBSERVER `surviving_captured_choice_pids` ends in `parse().unwrap_or(0)` with no `out.status.success()` check, so a post-close CIM/PowerShell query failure returns 0 and satisfies the reap assertion WITHOUT evidence (a residual false-green on the exact property the 20-50 Finding-1 fix was meant to make sound). Gemini and Kimi returned zero-finding PASS but both MISSED it — the same blind-spot shape the four-way panel exists to catch (as at 20-45). Under ALL-FOUR-MUST-PASS this BLOCKS and routes to a further repaired successor; the scarce Sean-gated native run (20-53) is NOT re-spent. The Finding-2 cmd classifier, both isolation guardrails (#A/#B), the STRUCTURAL half of Finding 1, and every other containment assertion are confirmed SOUND by all four legs. No source/test/workflow change; no requirement claimed.**

## Re-sealed candidate identity (audited)

- **source_sha (SEALED, audited):** `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2`
- **source_tree (SEALED, audited):** `ac76c87b318ee4ba8c34927dea23e40e63fd0776` (verified `f0dd5b6d^{tree}` == `ac76c87b`)
- **predecessor (sealed-but-RED, 20-45 BLOCK):** `3f839309574d6741eed416cd3820f56447f74eba`
- **repair delta (source):** exactly two Windows-only files — `hard_process_containment_windows.rs` (+88/−18) and `windows_impl/command.rs` (+12/−4). No `Cargo.toml`/`Cargo.lock`/workflow/`worktree_manager.rs` change. `windows_impl/tests.rs` byte-identical to the predecessor.
- **branch:** `plan/f20-unified-audit-repair` (isolated STANDALONE checkout `/Users/seandonahoe/dev/waylandcore-ferrox`; all git ops via `/usr/bin/git`).

## Four reviewer identities, invocations, and schema-validation status

| # | Reviewer | Kind | Invocation | Exit | Reachable | Schema-valid | Artifact key | Disposition |
|---|---|---|---|---|---|---|---|---|
| 1 | Codex 5.6 Sol | external-cli | `codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check` | 0 | YES | YES | `f20-native-crossaudit.codex-sol` | **BLOCK** (m1/l1) |
| 2 | Gemini 3.1 Pro | external-cli | `GEMINI_CLI_TRUST_WORKSPACE=true gemini -p … -m gemini-3.1-pro-preview -o text --approval-mode plan --skip-trust` | 0 | YES | YES | `f20-native-crossaudit.gemini-pro` | PASS |
| 3 | Kimi K3 | external-cli | `/Users/seandonahoe/.kimi-code/bin/kimi -p … --output-format text` | 0 | YES | YES | `f20-native-crossaudit.kimi-k3` | PASS |
| 4 | Claude adversarial | internal-claude | non-author subagent, prompted to REFUTE (default-refuted-if-uncertain) | — | YES | YES | `wayland-core.phase20-independent-review.v1` | **BLOCK** (l1) |

Raw-output sidecar identity: `.planning/phases/20-transactional-delegated-mutation/20-52-raw/` — `codex-sol.raw.txt` (224.9K), `gemini-pro.raw.txt` (19.7K), `kimi-k3.raw.txt` (71.9K), `audit-context.shared.txt` (15.8K shared brief + repair delta).

## The blocking finding (not rationalized, not refutable)

**Residual reap-observer false-green — `surviving_captured_choice_pids` (Codex MEDIUM / Claude LOW).**
`crates/wcore-sandbox/tests/hard_process_containment_windows.rs`. The 20-50 fix correctly closes the GUARANTEED
**structural** vacuity the 20-45 panel blocked on — the post-close reap check now filters `choice.exe` by the fixed
captured ProcessId set, so a leaked survivor is counted and a concurrent target's `choice.exe` is excluded (all four
legs concur this half is sound). BUT the new observer helper ends in
`String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)` with **no `out.status.success()` check**
(`.output().expect(...)` only catches a spawn failure). On any post-close CIM/PowerShell query failure — empty or
malformed stdout — it returns `0`, so `wait_until(|| surviving_captured_choice_pids(&captured_pids) == 0, 30, ...)`
is satisfied on the first poll and the reap property is reported proven **without evidence**. The new
`assert!(!captured_pids.is_empty())` guards the ALIVE capture phase only, not the post-close OBSERVER. The identical
`parse().unwrap_or(0)` pattern in the sibling `tagged_cmd_count` reap observer weakens the 20-45 robust-sibling
mitigation. It is not refutable (`unwrap_or(0)` demonstrably returns 0 on empty stdout), so the Claude leg concurred
rather than refuting.

**Fix for the further-repaired successor:** harden the post-close observer to FAIL CLOSED — assert
`out.status.success()` and treat empty/unparseable stdout as a test failure (panic) instead of `unwrap_or(0) => 0`,
in `surviving_captured_choice_pids` and consistently in the sibling `tagged_cmd_count` observer.

**Fail-closed watch-item (Codex LOW, not an independent blocker):** the post-close PID-reuse race can only produce a
false FAIL (fail-closed), so it is carried as a 20-53-observable watch-item; the successor should prefer
creation-time/handle identity if convenient.

## Confirmed sound (all four legs concur — preserve verbatim)

- **Finding 2 (cmd exact-final-component):** `resolved_program_is_cmd` = `Path::file_name() == "cmd.exe"`; accepts
  `System32\cmd.exe`, rejects `notcmd.exe`/`foocmd.exe`; new match set is a strict subset of the old `ends_with` set,
  so no genuine `cmd.exe` is newly misclassified. `quote_cmd_payload`/`quote_arg`/`classify_bare_shell`/
  `resolve_program`/`is_unc_or_device_path` byte-identical; single call site unchanged.
- **Guardrail A (DispatchAdmission budget):** delta does not touch `worktree_manager.rs` — preserved by absence of change.
- **Guardrail B (quoting-layer only):** `is_unc_or_device_path`, Low-IL restricted token, Job-Object limits, ACL
  lease untouched; argv discipline kept; 20-36 `exit /b 0` intact.
- **Finding 1 structural half:** captured-PID reap is non-vacuous and does not reintroduce the host-wide-image-count flake.
- **Preserved assertions:** exit_code==0, peak>0, peak<=512, peak<attempts, `reap_stray_choice`, sibling
  `job_close_reaps_detached_descendant_with_no_residue` + `tagged_cmd_count` — all verbatim.
- **Delta scope:** `windows_impl/tests.rs` is byte-identical to the predecessor; Kimi's 19 lib-test cross-check errors
  are pre-existing host-dependent `windows-sys` feature-unification artifacts (identical at `3f839309`), out of delta
  scope and part of the deferred `native_windows` compile check.

## Kimi watch-items carried into 20-53

`20-53-PLAN.md` carries all of: git-ops-over-de-verbatimized-swarm_root; `tagged_cmd_count` CIM visibility under
NetworkService; bash-worker-under-AppContainer + no-Windows-Docker-fallback (`dispatch.rs`); worker test-exe DLL-load
under Low-IL; `dispatch.rs:604` canonicalize — plus the two 20-50 fix-proof items (Finding 1 non-vacuous captured-PID
reap; Finding 2 granted-read exit-0). On the further repair, the successor's fix-proof set expands to include the
hardened fail-closed reap observer.

## Disposition

**GATE: BLOCK — does NOT admit 20-53.** ALL FOUR did NOT pass: Codex 5.6 Sol + the internal Claude adversarial leg
returned BLOCK on a real, non-deferred, non-refutable finding. Route to a further repaired successor that hardens the
reap observer to fail closed; then re-seal and re-run this four-way pre-native cross-audit to a zero-finding ALL-FOUR
PASS before the Sean-gated native re-dispatch (20-53) is authorized. All prior native-proof authorizations remain
spent/void; the scarce native run is NOT re-spent on this candidate. `native_macos`/`native_windows` remain the only
deferred checks.

## Task Commits

This plan modifies no repository source/test/workflow file. It writes only the cross-audit artifact, its raw sidecar,
and this summary.

1. **Task 1: Four-way pre-native cross-audit panel** — no code commit (artifact + raw sidecars + summary only).

## Files Created/Modified

- `.planning/phases/20-transactional-delegated-mutation/20-52-CROSS-AUDIT.md` (created)
- `.planning/phases/20-transactional-delegated-mutation/20-52-raw/{audit-context.shared.txt,codex-sol.raw.txt,gemini-pro.raw.txt,kimi-k3.raw.txt}` (created)
- `.planning/phases/20-transactional-delegated-mutation/20-52-SUMMARY.md` (this summary, created)

No repository source/test/workflow file touched.

## Deviations from Plan

None — plan executed exactly as written. The panel returned a finding-based BLOCK (the plan's explicit
all-four-must-pass / route-to-further-repair path), not an incomplete-panel block. The residual finding surfaced by
the panel is a deeper layer of the same Finding-1 surface (observer error handling under the now-closed structural
vacuity), recorded honestly rather than rationalized.

## Issues Encountered

None. All three external CLIs were reachable and returned schema-valid JSON on the first invocation (Gemini needed
`GEMINI_CLI_TRUST_WORKSPACE=true --skip-trust`; Kimi needed the absolute path with the brief in `-p`, per the 20-45
quirks). Kimi ran an msvc cross-check as part of its analysis and correctly classified the 19 lib-test errors as
pre-existing out-of-scope artifacts. No Cargo ran on the Mac from this plan (Kimi's own tool use aside).

## Next Phase Readiness

- 20-53 is **NOT** admitted. A further-repaired successor (hardened fail-closed reap observer) must be produced,
  re-sealed, and re-audited to a zero-finding ALL-FOUR PASS first.

## Self-Check: PASSED

- `20-52-CROSS-AUDIT.md` exists; contains all four artifact keys (`f20-native-crossaudit.codex-sol`, `.gemini-pro`,
  `.kimi-k3`, `wayland-core.phase20-independent-review.v1`), both findings, the four reviewer identities, `source_sha`,
  and the deferred native checks (49 keyword hits).
- Four raw sidecars preserved in `20-52-raw/`.
- Sealed `source_sha` `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2`, tree `ac76c87b318ee4ba8c34927dea23e40e63fd0776`
  (verified `f0dd5b6d^{tree}`).
- All three external auditors reachable, exit 0, schema-valid → finding-based BLOCK (not fail-closed-unreachable).
- No Phase 20 requirement claimed (`requirements-completed: []`).
- No repository source/test/workflow file modified by this plan.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
