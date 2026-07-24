---
phase: 20-transactional-delegated-mutation
plan: "57"
subsystem: infra
tags: [native-repair, fail-closed, reap-observer-false-green, unwrap-or-zero, comprehensive-single-class, windows-job-object, honest-deferral, further-repair-successor]

# Dependency graph
requires:
  - phase: "20-52"
    provides: "FOUR-way pre-native panel BLOCK against the sealed successor f0dd5b6d (tree ac76c87b): the residual reap-OBSERVER false-green — surviving_captured_choice_pids (and sibling tagged_cmd_count) end in parse().unwrap_or(0) with no out.status.success() check, so a post-close CIM/PowerShell query failure returns 0 and satisfies wait_until(...==0) without evidence. Everything else (Finding-2 cmd classifier, guardrails #A/#B, structural reap, sibling tests) confirmed SOUND by all four legs."
provides:
  - "Further-repaired successor delta 8a1d2d84 (tree c1fe79fe) over the 20-52-blocked f0dd5b6d (tree ac76c87b): ALL THREE host-side query observers in crates/wcore-sandbox/tests/hard_process_containment_windows.rs hardened to FAIL CLOSED in one comprehensive pass. tagged_cmd_count, tagged_choice_descendant_pids, and surviving_captured_choice_pids each assert out.status.success() after .output() and panic on non-success exit / unparseable output instead of mapping the failure to 0 (or a silently-empty Vec). A post-close query failure can no longer satisfy a reap wait_until(...==0) without evidence."
  - "Single-class, comprehensive close: the two observers the finding named PLUS the third of the same class (tagged_choice_descendant_pids, whose filter_map(...ok()) silently swallowed a query error) fixed together, so a re-audit cannot surface a third instance. Full-file sweep confirms the only remaining .unwrap_or(0) is the benign unique_tag SystemTime-nanos fallback (not a query observer) and reap_stray_choice stays intentional best-effort cleanup — both out of scope and unchanged."
  - "Scope integrity: only the three shared helper BODIES (+ their doc comments) change; no test body and no assertion altered. The captured-PID reap logic, assert!(!captured_pids.is_empty()), the peak/cap/below-attempts/clean-exit assertions, the if pids.is_empty() { return 0; } short-circuit, and the sibling job_close_reaps_detached_descendant_with_no_residue / breakaway_is_denied / qualified_hard_containment_backend_preflight tests are preserved verbatim (grep-confirmed). command.rs, the #A/#B guardrails, and every file outside the one named file are untouched."
affects: ["20-58", "20-59", "20-53", "20-54", "20-55", "20-56"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "A reap/containment assertion is not sound until its host-side query observer FAILS CLOSED: after .output(), assert!(out.status.success(), ...) (include status.code() + stderr) and parse with a panicking fallback, so a failed CIM/PowerShell query panics the test rather than being read as a passing count / empty list. Fixing the STRUCTURAL vacuity (20-50) and the alive-phase !captured_pids.is_empty() guard is necessary but not sufficient — the post-close observer error path is the residual false-green."
    - "Close the fail-open CLASS in one pass, not just the named instances: harden every observer sharing the shape (parse().unwrap_or(0) and filter_map(...ok()) both map a query error to a passing value) and run a full-file sweep, so a re-audit cannot find a third instance of the same class."

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-57-SUMMARY.md
  modified:
    - crates/wcore-sandbox/tests/hard_process_containment_windows.rs

key-decisions:
  - "Comprehensive one-pass fix of ALL THREE observers, not only the two Codex/Claude named. tagged_choice_descendant_pids ended in split_whitespace().filter_map(|s| s.parse::<u32>().ok()).collect(), which silently swallowed a non-success exit or malformed token and returned an empty Vec — the same fail-open class. It now asserts out.status.success() and parses each token with a panicking parse (unwrap_or_else(|err| panic!(...))); a LEGITIMATE empty result on a success exit still yields an empty Vec because split_whitespace() over empty stdout produces no tokens, preserving the valid zero-descendants case."
  - "Fix vs scope-change strictly separated: only HOW a query error is handled changed (fail closed), never WHAT is asserted. Every assertion, the captured-PID reap logic, the empty-set short-circuit, and the three sibling tests are byte-preserved. Guardrails #A/#B, command.rs, the AppContainer implementation, and the unique_tag nanos / reap_stray_choice best-effort trailers are untouched."
  - "HONEST deferral (CONTEXT D5/D7): hard_process_containment_windows.rs is #![cfg(windows)] — NEITHER the Mac NOR Hetzner Linux compiles a line of the observer fix. The sanctioned macOS cargo check (green) and the Hetzner clippy + aggregate (11509/0/48) are a NO-REGRESSION FLOOR only, NOT proof of the fix. The observer fix's compile AND real-hardware fail-closed behavior are proven ONLY at the rebound 20-53 native msvc re-dispatch. No Windows fix is claimed verified pre-dispatch."
  - "Codex LOW post-close PID-reuse race carried as a 20-53 fail-closed native watch-item (recycling a captured PID to a concurrent choice.exe within the 30s window can only make the test FAIL, never false-green), NOT expanded in scope here."
  - "Chain decision: AUTHOR delta plans 20-57 (this fix) -> 20-58 (re-seal) -> 20-59 (four-way pre-native cross-audit), then REBIND the pending unexecuted 20-53..20-56 onto the 20-58 seal (depends_on chains from 20-59; symbolic seal 20-51 -> 20-58; pre-native cross-audit 20-52 -> 20-59). Do NOT mutate the executed/sealed records 20-43/44/45/50/51/52."
  - "requirements-completed is [] — NO Phase-20 requirement is claimed. REQ-native-r7 (observer integrity) and REQ-native-r4 (Linux no-regression floor) are the surfaces this delta writes toward, but the fix is Windows-only construction-only; its behavior is proven only at the rebound 20-53, so no requirement is completed and no native/aggregate-seal claim is made."

patterns-established:
  - "The pre-native four-way gate re-prosecutes the SAME finding surface at a deeper layer after each fix: 20-45 blocked on structural reap vacuity, 20-50 closed it, 20-52 surfaced the residual observer-error-handling false-green underneath, and 20-57 closes the whole observer class. A containment assertion is done only when its host-side observer also fails closed."

requirements-completed: []  # NO Phase-20 requirement claimed. Windows-only construction-only fix; behavior proven at the rebound 20-53.

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "All three host-side query observers (tagged_cmd_count, tagged_choice_descendant_pids, surviving_captured_choice_pids) fail CLOSED on a CIM/PowerShell query failure — each asserts out.status.success() and panics on non-success/unparseable output — so a post-close query failure can no longer satisfy a reap wait_until(...==0) without evidence (REQ-native-r7). Windows compile + fail-closed behavior proven ONLY at the rebound 20-53."
    requirement: "REQ-native-r7"
    verification:
      - kind: unit
        ref: "grep: out.status.success() x3 (L128/L186/L243); unwrap_or_else(|err| panic!) x3 (L135/L195/L250); no filter_map(...ok()); only remaining unwrap_or(0) is unique_tag nanos (L98)"
        status: pass
      - kind: manual_procedural
        ref: "#![cfg(windows)] compile + real-hardware fail-closed behavior deferred to rebound 20-53 native msvc re-dispatch"
        status: unknown
    human_judgment: true
    rationale: "The observer fix is #![cfg(windows)] and is NOT compiled or executed by any pre-dispatch Mac/Linux gate; only the self-hosted msvc runner at 20-53 can prove its compile and fail-closed behavior. Pre-dispatch gates are a no-regression floor, not proof."
  - id: D2
    description: "Linux/macOS no-regression floor held: sanctioned macOS cargo check -p wcore-sandbox --features live-docker --tests green (exit 0), Hetzner clippy -D warnings clean (exit 0), Hetzner aggregate nextest --profile ci --no-fail-fast exactly 11509/0/48 (Windows-only change => Linux unchanged) (REQ-native-r4)."
    requirement: "REQ-native-r4"
    verification:
      - kind: integration
        ref: "macOS cargo check exit 0; f20-57-clippy exit 0; f20-57-aggregate: 11509 tests run: 11509 passed (2 flaky), 48 skipped = 11509/0/48"
        status: pass
    human_judgment: false

# Metrics
duration: ~25min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 57: Fail-closed hardening of ALL THREE host-side reap-observers — closes the 20-52 residual false-green Summary

**The 20-52 FOUR-WAY pre-native panel (Codex 5.6 Sol MEDIUM + internal Claude adversarial LOW, concurring; Gemini + Kimi missed it) returned BLOCK on the re-sealed successor `f0dd5b6d` (tree `ac76c87b`) for exactly ONE real, statically-provable, non-refutable finding: the residual reap-OBSERVER false-green. The 20-50 fix closed the STRUCTURAL reap vacuity, but the post-close observer still FAILED OPEN — `surviving_captured_choice_pids` ended in `parse().unwrap_or(0)` with no `out.status.success()` check, so a post-close CIM/PowerShell query FAILURE returned 0, satisfied `wait_until(... == 0)` on the first poll, and reported the reap property proven with no evidence. This delta (`8a1d2d84`, tree `c1fe79fe`) hardens EVERY host-side query observer in `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` to FAIL CLOSED in one comprehensive pass — not only the two the finding named, but all three (`tagged_cmd_count`, `tagged_choice_descendant_pids`, `surviving_captured_choice_pids`), plus a full-file sweep — so a re-audit cannot surface a third instance of the class. Only the three shared helper bodies (+ doc comments) change; every assertion, the captured-PID reap logic, the empty-set short-circuit, and the sibling tests are preserved verbatim. The file is `#![cfg(windows)]`: its compile AND fail-closed behavior are proven ONLY at the rebound 20-53 native msvc re-dispatch — the Mac/Hetzner gates here are a no-regression floor, not proof. No requirement is completed.**

## New candidate identity

| Field | Value |
|-------|-------|
| Successor commit (`source_sha`) | `8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e` |
| Successor tree | `c1fe79fe4a6d68a536078be4887343a82b5fce38` |
| Predecessor (20-52-blocked, sealed) | `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2` (tree `ac76c87b318ee4ba8c34927dea23e40e63fd0776`) |
| Branch | `plan/f20-unified-audit-repair` (standalone checkout `/Users/seandonahoe/dev/waylandcore-ferrox`, `.git` = directory) |
| Changed paths | `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` (1 file, +53 / -10) |

## Byte-identity check vs the sealed base (no source drift)

Before editing, `git diff f0dd5b6d -- crates/wcore-sandbox/tests/hard_process_containment_windows.rs` was **empty** — the touched file was byte-identical to the sealed `f0dd5b6d` candidate (only planning-doc commits sat between the seal and the branch tip), so the fix is authored cleanly on top of the audited source with no intervening source drift. After the fix, `git diff --stat f0dd5b6d HEAD` shows the single expected delta (1 file changed, 53 insertions, 10 deletions).

## The fix — all three observers fail CLOSED (one pass)

Each observer now, after `.output()`:

1. **`tagged_cmd_count`** — `assert!(out.status.success(), ...)` (message includes `out.status.code()` + stderr), then the trailing `.parse().unwrap_or(0)` replaced with `.unwrap_or_else(|err| panic!(...))`. The CIM `@(...).Count` always prints a number on a success exit, so empty/unparseable stdout on "success" is anomalous and panics — never read as 0.
2. **`surviving_captured_choice_pids`** — the legitimate `if pids.is_empty() { return 0; }` short-circuit kept VERBATIM; past it, the same `assert!(out.status.success(), ...)` + panicking parse applied to the post-query `.Count`.
3. **`tagged_choice_descendant_pids`** — `assert!(out.status.success(), ...)`, then the PID list parsed with a panicking per-token parse (`.map(|s| s.parse::<u32>().unwrap_or_else(|err| panic!(...)))`) instead of `filter_map(...ok())`, so a malformed token fails closed. A LEGITIMATE empty result on a success exit still yields an empty `Vec` (`split_whitespace()` over empty stdout produces no tokens), preserving the valid zero-descendants case.

Each observer's doc comment now documents the fail-closed contract without writing the literal zero-default token.

**Full-file sweep:** the only remaining `.unwrap_or(0)` is the benign `unique_tag` SystemTime-nanos fallback (L98 — seeds a tag suffix, NOT a query observer, left intentionally); `reap_stray_choice` stays intentional best-effort cleanup (`let _ = ...output()`, left intentionally). No other observer, `wait_until` predicate, or assertion can pass on a query error.

## Preserved verbatim (20-52 panel confirmed sound)

`if pids.is_empty()` short-circuit (L221), `!captured_pids.is_empty()` (L469), `peak > 0` (L455), `peak <= SANDBOX_ACTIVE_PROCESS_LIMIT` (L459), `peak < attempts` (L464), the captured-PID two-phase reap logic, exit-0 assertions, the `reap_stray_choice()` trailers, and the sibling `job_close_reaps_detached_descendant_with_no_residue` / `breakaway_is_denied` / `qualified_hard_containment_backend_preflight` tests. `command.rs`, guardrails #A/#B, and every file outside the one named file are untouched.

## Gate receipts

Construction gate (Mac): scope-ok (base `c902b2e5`, generation `g-5a8dc837...`, paths=1) + `vx rustfmt --edition 2024 --check` clean + grep sweeps (below) as the provisional task commit `8a1d2d84`. After the commit and with a clean tree:

| Gate | Command | Result |
|------|---------|--------|
| Sanctioned macOS compile | `cargo check -p wcore-sandbox --features live-docker --tests` | **exit 0** (0 errors; lone warning is a pre-existing macOS-cfg dead-code note in `process_tree.rs`, unrelated — the `#![cfg(windows)]` test compiles to nothing on Mac) |
| Hetzner clippy | `remote-cargo.sh f20-57-clippy ... clippy -p wcore-sandbox --all-targets --all-features -- -D warnings` | **exit 0**, clean (run ID `f20-57-clippy`) |
| Hetzner aggregate | `remote-cargo.sh f20-57-aggregate ... nextest run --workspace --profile ci --no-fail-fast` | **`11509 tests run: 11509 passed (2 flaky), 48 skipped` = 11509/0/48** (run ID `f20-57-aggregate`) — matches the floor exactly; the 2 flaky passed on nextest retry, 0 failed |

Grep confirmations on the committed file:
- `out.status.success()` — 3 occurrences (L128, L186, L243), one per observer.
- `unwrap_or_else(|err| panic!` — 3 occurrences (L135, L195, L250), one panicking parse per observer.
- `filter_map(|s| s.parse` — none remaining.
- `unwrap_or(0)` — 1 occurrence only (L98, benign `unique_tag` nanos).
- Preserved assertions grep-confirmed present (L221 / L455 / L459 / L464 / L469).

The macOS `cargo check` re-confirms the macOS-compiled `wcore-sandbox` surface still compiles — it does NOT compile the `#![cfg(windows)]` containment test. The Hetzner clippy + aggregate are a NO-REGRESSION FLOOR: because the fix is Windows-only, the Linux aggregate MUST be exactly 11509/0/48 (it is), and the aggregate ran with the unchanged lock (20-57 changes no manifest).

## Explicit deferrals (honesty gate)

- **ALL Windows compile + real-hardware behavior of the fail-closed observers** is deferred to the rebound **20-53** native msvc re-dispatch. NEITHER the Mac NOR Hetzner Linux compiles a line of the change; no pre-dispatch gate proves it. No Windows fix is claimed verified here.
- **Codex LOW post-close PID-reuse race** carried as a 20-53 fail-closed native watch-item (can only false-FAIL, never false-green), NOT expanded in scope.

## Chain decision

AUTHOR delta plans **20-57 (this fix) → 20-58 (re-seal) → 20-59 (four-way pre-native cross-audit)**, then REBIND the pending unexecuted **20-53…20-56** onto the 20-58 seal (`depends_on` chains from 20-59; symbolic seal 20-51 → 20-58; pre-native cross-audit 20-52 → 20-59). Do NOT mutate the executed/sealed records **20-43/44/45/50/51/52**. Net flow of the further-repaired candidate: 20-57 → 20-58 → 20-59 → (rebound) 20-53 → 20-54 → 20-55 → 20-56.

## Deviations from Plan

None — plan executed exactly as written. The pre-existing untouched `AGENTS.md` ijfw-memory modification was non-destructively restored (`git checkout -- AGENTS.md`) once, solely to give the scope-base capture a clean tree, exactly as prior executors did; it was never staged and is not part of any commit.

## Self-Check: PASSED

- `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` — modified, committed in `8a1d2d84`.
- `.planning/phases/20-transactional-delegated-mutation/20-57-SUMMARY.md` — created.
- Commit `8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e` exists on `plan/f20-unified-audit-repair` (tree `c1fe79fe`).
