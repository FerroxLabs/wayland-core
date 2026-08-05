---
phase: 20-transactional-delegated-mutation
plan: "45"
subsystem: infra
tags: [native-repair, cross-audit, pre-native-gate, four-way-panel, codex-sol, gemini-pro, kimi-k3, claude-adversarial, fail-closed, all-must-pass, third-repaired-successor, block, containment-assertion, dispatch-admission, cmd-quoting]

# Dependency graph
requires:
  - phase: "20-44"
    provides: "SEALED third-repaired-successor source_sha 3f839309574d6741eed416cd3820f56447f74eba (tree 3092475bb4102d010b6ff5f6c9d8080cb4f51928) — Cargo.lock dunce-edge resync + green --locked build + aggregate 11509/0/48 (run 32f1b4ba). The exact SHA every downstream gate binds to."
provides:
  - "Four per-reviewer schema-validated pre-native cross-audits of the sealed successor 3f839309: Codex Sol f20-native-crossaudit.codex-sol (BLOCK), Gemini Pro f20-native-crossaudit.gemini-pro (PASS), Kimi K3 f20-native-crossaudit.kimi-k3 (PASS), internal Claude adversarial wayland-core.phase20-independent-review.v1 (BLOCK) — recorded in 20-45-CROSS-AUDIT.md with raw outputs preserved under 20-45-raw/."
  - "GATE DISPOSITION: BLOCK — does NOT admit 20-46. Two of four auditors raise a real, non-deferred finding (the active_process_cap_is_enforced post-close reap assertion is vacuous once the tagged parent cmd dies; + the resolved_program_is_cmd ends_with imprecision). All three external auditors were reachable and schema-valid, so this is a finding-based BLOCK, not an incomplete-panel BLOCK."
  - "Both isolation guardrails PASS across all four legs: (A) DispatchAdmission budget preserved — no catch-to-unlimited; (B) #B is a quoting-layer change only — is_unc_or_device_path / Low-IL token / Job-Object limits / ACL lease untouched, argv discipline kept, 20-36 exit /b 0 intact. Kimi watch-items confirmed carried into 20-46-PLAN (lines 24/61/83)."
affects: ["20-46", "20-47", "20-48", "20-49"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Four-independent-auditor pre-native gate (3 external CLIs run in parallel + internal Claude adversarial), ALL-FOUR-MUST-PASS, fail-closed on any unreachable/schema-invalid external leg. A 2-PASS/2-BLOCK split still BLOCKS — a four-way panel catches a shared blind spot (Gemini + Kimi both missed the reap-assertion degradation Codex + Claude caught)."
    - "Distinguish a static code-logic defect (the vacuous post-close reap assertion, provable by reading the test) from a Windows-runtime property (CIM visibility) — the former is non-deferred and blocks; the latter rides the native_windows deferral."

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-45-CROSS-AUDIT.md
    - .planning/phases/20-transactional-delegated-mutation/20-45-SUMMARY.md
    - .planning/phases/20-transactional-delegated-mutation/20-45-raw/codex-sol.raw.txt
    - .planning/phases/20-transactional-delegated-mutation/20-45-raw/gemini-pro.raw.txt
    - .planning/phases/20-transactional-delegated-mutation/20-45-raw/kimi-k3.raw.txt
    - .planning/phases/20-transactional-delegated-mutation/20-45-raw/audit-context.shared.txt
  modified: []

key-decisions:
  - "GATE = BLOCK. The blocking defect is NOT rationalized away: active_process_cap_is_enforced fans out BARE choice.exe idlers (choice rejects an injected tag), so tagged_choice_descendant_count scopes them by the tagged parent cmd's ParentProcessId. By the post-close check run.await has returned -> the AppContainer job closed -> the tagged parent cmd is dead -> the CIM cmd.exe-with-tag query is empty -> the count is structurally 0 regardless of a leaked/orphaned choice. The prior host-wide image_count(choice.exe)<=baseline would have caught a leaked survivor; the new scoping cannot. This contradicts the helper doc ('without weakening any containment assertion') and 20-43-SUMMARY ('Every containment assertion is preserved verbatim'). Codex rated HIGH; honestly re-rated MEDIUM here because the reap-after-close PROPERTY is robustly proven by the sibling job_close_reaps_detached_descendant_with_no_residue (its grandchild carries rem {tag} on its OWN cmdline, so tagged_cmd_count survives parent death)."
  - "Second finding carried (LOW): resolved_program_is_cmd uses ends_with(\"cmd.exe\"), which suffix-matches notcmd.exe/foocmd.exe. Bounded reachability (resolver pins bare cmd to System32\\cmd.exe; only cmd runs under Low-IL) so NOT an isolation-boundary regression, but an imprecise classifier — should compare the final path component == cmd.exe."
  - "Codex's third finding (evidence-integrity FAIL) is REFUTED with counter-evidence and NOT carried as a real candidate defect: Codex read only 20-43-SUMMARY (env-disk-pressured 11508/1/48, source_sha 92cac8bb) and did not reconcile with 20-44-SUMMARY, which re-ran clean at 11509/0/48 (run 32f1b4ba) against tree 3092475b, proved --locked exit 0, and sealed the candidate at 3f839309 with exactly the one-line Cargo.lock dunce edge. Recorded as an auditor observation."
  - "Gemini required GEMINI_CLI_TRUST_WORKSPACE=true + --skip-trust to run in the untrusted checkout dir (first attempt exit 55). Kimi's -p mode does not ingest piped stdin — the shared brief had to be passed as the -p argument (first attempt reconstructed context from the repo, then was killed by the 10-min parallel wait; re-run standalone completed exit 0). Codex read the brief on stdin cleanly on the first attempt."

patterns-established:
  - "Preserve every auditor's raw output on disk (20-45-raw/) alongside the parsed keyed artifacts — no prose-only vote counts toward PASS (R13)."

requirements-completed: []  # NO Phase-20 requirement is claimed here. This gate implements REQ-native-r12/r13 as a gate; a BLOCK completes no requirement.

# Metrics
duration: ~40min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 45: Four-way pre-native cross-audit of the sealed third-repaired-successor (3f839309) — GATE BLOCK Summary

**A FOUR-WAY independent panel — Codex 5.6 Sol + Gemini 3.1 Pro + Kimi K3 (three external CLIs run in parallel) plus an internal Claude non-author adversarial reviewer — prosecuted the exact sealed third-repaired-successor `3f839309` (tree `3092475b`) across the 20-43 repair delta at every non-deferred severity. All three external auditors were reachable and returned schema-valid JSON (no fail-closed-on-unreachable). Result: 2 PASS (Gemini, Kimi) / 2 BLOCK (Codex, Claude). Under ALL-FOUR-MUST-PASS the gate BLOCKS and routes to a further repaired successor — it does NOT admit the scarce Sean-gated native run (20-46). The blocking defect is a statically-provable test-assertion degradation, not a Windows-runtime question and not a hallucination.**

## Sealed candidate under audit

- **source_sha:** `3f839309574d6741eed416cd3820f56447f74eba`
- **source_tree:** `3092475bb4102d010b6ff5f6c9d8080cb4f51928` (verified `3f839309^{tree}` == this)
- **review base:** `source_sha` tuple over `20-44-SUMMARY.md`; delta prosecuted = the 20-43 repair over `daf27337` (#A wcore-swarm de-verbatimize + env-transport probe; #B wcore-sandbox cmd `/c|/k` payload quoting; Job-Object counting-scope; lib-test compile-debt imports; belt-and-braces nextest group).
- **branch:** `plan/f20-unified-audit-repair` (isolated checkout `/Users/seandonahoe/dev/waylandcore-ferrox`; `.git` is a directory; all git ops via `/usr/bin/git`).

## Panel result

| Auditor | Kind | Artifact key | Reachable | Schema-valid | Findings (b/c/h/m/l) | Disposition |
|---|---|---|---|---|---|---|
| Codex 5.6 Sol | external CLI | `f20-native-crossaudit.codex-sol` | YES | YES | 0/0/1/2/0 | **BLOCK** |
| Gemini 3.1 Pro | external CLI | `f20-native-crossaudit.gemini-pro` | YES | YES | 0/0/0/0/0 | PASS |
| Kimi K3 | external CLI | `f20-native-crossaudit.kimi-k3` | YES | YES | 0/0/0/0/0 | PASS |
| Claude adversarial | internal non-author | `wayland-core.phase20-independent-review.v1` | n/a | YES | 0/0/0/1/1 | **BLOCK** |

### External CLI invocations (verbatim)
- Codex: `codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "<prompt>"` (shared brief on stdin) — exit 0.
- Gemini: `GEMINI_CLI_TRUST_WORKSPACE=true gemini --skip-trust -m gemini-3.1-pro-preview -o text -p "<brief>"` — exit 0.
- Kimi: `/Users/seandonahoe/.kimi-code/bin/kimi -p "<brief>" --output-format text` (absolute path) — exit 0.

Raw outputs preserved: `.planning/phases/20-transactional-delegated-mutation/20-45-raw/{codex-sol,gemini-pro,kimi-k3}.raw.txt` + shared `audit-context.shared.txt`.

## GATE DISPOSITION: BLOCK — does NOT admit 20-46

Two of four auditors raise real, non-deferred findings. Per the plan's ALL-FOUR-MUST-PASS rule, any single non-deferred finding from any one auditor stops the sequence and routes to a further repaired successor. The Gemini + Kimi zero-finding PASSes both **missed** the reap-assertion degradation — precisely the shared blind spot a four-independent panel exists to catch.

### Blocking finding (carried, real)
1. **[MEDIUM — impact-mitigated] `active_process_cap_is_enforced` post-close reap assertion is vacuous.** The fan-out idlers are bare `choice.exe` (choice rejects an injected tag), so `tagged_choice_descendant_count(&tag)` scopes by the tagged parent cmd's `ParentProcessId`. By the post-close check, `run.await` has returned → the job closed → the tagged parent cmd is dead → the CIM `cmd.exe`-with-tag query is empty → the count is structurally 0 regardless of a leaked/orphaned `choice`. `wait_until(|| tagged_choice_descendant_count(&tag) == 0, ...)` passes on the first poll, vacuously. The prior host-wide `image_count("choice.exe") <= baseline` would have caught a leaked survivor. The helper doc and 20-43-SUMMARY's "Every containment assertion is preserved verbatim" are inaccurate for this assertion. **Impact-mitigated** — the reap-after-close *property* is robustly proven by the sibling `job_close_reaps_detached_descendant_with_no_residue` (grandchild carries `rem {tag}` on its OWN cmdline, so `tagged_cmd_count` survives parent death) — but the specific assertion is degraded. Codex rated HIGH; re-rated MEDIUM here.
2. **[LOW — low reachability] `resolved_program_is_cmd` uses `ends_with("cmd.exe")`.** Suffix-matches `notcmd.exe`/`foocmd.exe`; bounded reachability (resolver pins bare `cmd`; only `cmd` runs under Low-IL), not an isolation regression, but an imprecise classifier — should compare the final path component `== "cmd.exe"`.

### Refuted (NOT carried)
- **Codex evidence-integrity FAIL:** derived from reading only `20-43-SUMMARY.md` (env-blocked `11508/1/48`, source_sha `92cac8bb`) without reconciling `20-44-SUMMARY.md` (clean `11509/0/48`, run `32f1b4ba`, tree `3092475b`, `--locked` exit 0, sealed at `3f839309`). Refuted with counter-evidence; recorded as an auditor observation only.

## Isolation guardrails (all four legs concur PASS)

- **Guardrail A — DispatchAdmission budget preserved:** PASS (4/4). Probe returns `Result<u64>`; spawn/capture failure, non-zero status, and parse failure all return `SwarmError::DispatchAdmission` (fail-closed). No `u64::MAX`, no `unwrap_or`, no catch-to-unlimited. `required = MAX_TRANSACTION_WORKSPACE_BYTES.min(...).checked_add(WORKSPACE_SAFETY_MARGIN_BYTES)`. `WCORE_SWARM_PROBE_ROOT` env transport changes delivery only; `dunce::simplified` no-op on unix.
- **Guardrail B — #B quoting-layer only:** PASS on isolation (4/4). The delta touches no `is_unc_or_device_path`, no Low-IL restricted token, no Job-Object limits, no ACL lease; argv discipline kept (payload is one caller-supplied argv entry, not `format!`-interpolated); 20-36 `exit /b 0` intact. Only sub-finding is the LOW classifier imprecision above.
- **Kimi watch-items carried into 20-46:** PASS (4/4). `20-46-PLAN.md` lines 24/61/83 carry git-ops-over-de-verbatimized-swarm_root, `tagged_cmd_count` CIM visibility under NetworkService, bash-worker-under-AppContainer + no-Windows-Docker-fallback, and worker test-exe DLL-load under Low-IL as explicit next-layer checks (plus the extra `dispatch.rs:604` canonicalize watch-item).

## Deviations from Plan

**1. [Rule 3 — Blocking, tooling] Gemini directory-trust + Kimi stdin transport.** Gemini's first parallel run exited 55 ("not running in a trusted directory"); re-run with `GEMINI_CLI_TRUST_WORKSPACE=true --skip-trust` → exit 0. Kimi's `-p` mode does not ingest piped stdin (first run reconstructed context from the repo, then was killed by the 10-min parallel wait); re-run standalone with the full brief in the `-p` argument → exit 0. No source/test/workflow change. All three external legs ultimately reachable + schema-valid, so fail-closed-on-unreachable was NOT triggered.

**2. [Judgment — not a code deviation] Honest severity re-rating.** Codex's HIGH (reap assertion) re-rated MEDIUM given robust sibling coverage; Codex's MEDIUM (ends_with) re-rated LOW given bounded reachability; Codex's evidence-integrity MEDIUM refuted with counter-evidence and not carried. The gate BLOCKS regardless of the re-rating — any non-deferred finding suffices.

No scope creep — only the cross-audit artifact, its raw-output sidecars, this summary, and (in the metadata commit) the standard planning docs were written. No source/test/workflow file touched.

## Routing to a further repaired successor

The next repair must: (1) fix `active_process_cap_is_enforced` so a leaked/orphaned bare-`choice` idler remains detectable after job close (capture the fan-out choice PIDs / parent `ProcessId` before close and re-check by fixed PID, or keep a tag-independent host-wide safety net); (2) tighten `resolved_program_is_cmd` to an exact `cmd.exe` filename-component match. Then re-seal (Cargo.lock + `--locked` build + aggregate) and re-run this four-way pre-native cross-audit to a zero-finding all-four PASS before 20-46 is authorized. All prior native-proof authorizations remain spent/void.

## Self-Check: PASSED

- `20-45-CROSS-AUDIT.md` present; the four artifact keys (`f20-native-crossaudit.codex-sol`/`.gemini-pro`/`.kimi-k3` + `wayland-core.phase20-independent-review.v1`), the sealed `source_sha` `3f839309`, and `deferred` all grep-match.
- Raw outputs preserved under `20-45-raw/` (codex 1.6K, gemini 433B, kimi 5.3K + shared context 29.9K).
- Sealed SHA verified: `git rev-parse 3f839309^{tree}` == `3092475bb4102d010b6ff5f6c9d8080cb4f51928`.
- No source/test/workflow file modified (cross-audit writes only planning artifacts).
- No Phase 20 requirement claimed (a BLOCK gate completes none).

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
