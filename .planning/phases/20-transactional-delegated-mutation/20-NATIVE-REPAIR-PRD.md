# Native-Repair Successor — Acceptance Spec (PRD)

**Phase:** 20 (Transactional Delegated Mutation) — native-path closure
**Type:** Repaired 20-08 successor (ADDITIVE — does NOT replan/renumber accepted 20-01…20-16)
**Candidate base:** `6937ef6` (tree `6db6fc85`) / working commit `be84bd2` on `plan/f20-unified-audit-repair`
**Source of truth:** `.planning/inbox/native-uat-repair-BRIEF.md` (defect inventory) +
`.planning/intel/inbox-2026-07-23/AUDIT-2026-07-23.md` (audit addenda) +
`.planning/intel/inbox-2026-07-23/requirements.md` (R1–R12 full text).

---

## 1. Why this plan exists (corrected provenance — DO NOT re-overstate)

The recorded, tuple-authorized **20-18 ran RED on 2026-07-22 against PRE-repair `5e665ec`**
(hosted `windows-2022` + ephemeral macOS). Failures were:
- **Windows:** `wcore-sandbox` lib does **not compile** (windows_impl module-path debt —
  E0425 `reserve_output`/`probe_single_flight`/`BUFFERED_OUTPUT_LIMIT_BYTES`, E0423 private-field ctors).
- **macOS:** proof references a Windows-only test (`live_integrity.rs` is `#![cfg(windows)]` → 0 tests → `--no-tests=fail`).

`6937ef6` is the repaired successor that CLAIMS to fix those (added macOS tests + windows_impl edits);
20-16 reviewed it CLEAN **with native deferred by design**; 20-17 persisted a still-**pending** tuple.
**No 20-18 has run against `6937ef6`.** A separate 2026-07-23 **off-plan diagnostic** on real Windows 11
(which drifted from Ferrox discipline) then found a **deeper AppContainer read/token boundary issue**.
Neither Windows nor macOS native is proven on hardware for `6937ef6`.

**This plan's job:** produce ONE repaired candidate SHA that passes the FULL native path under discipline,
then re-run the review/prep/UAT gates against it.

## 2. Locked decisions (scope fence — OFF LIMITS to re-litigate)

- **D1.** Do NOT replan, renumber, or re-execute accepted plans 20-01…20-16. This is additive successor work.
- **D2.** First action is a HARD hygiene gate: **reset the working tree to pristine `be84bd2`**, discarding
  the uncommitted boundary-breaking diagnostic in `windows_impl/process.rs` (the `current_token` full-token
  swap + `DISABLE_MAX_PRIVILEGE=0` + WCORE_DIAG_TOKEN dump). The real fix is written FRESH, never salvaged
  from the diagnostic edit. (`storage.rs` `.write(true)` is a clean spike — re-derive it deliberately too.)
- **D3.** Every review gate emits a **schema-validated review artifact per claimed reviewer**. No prose-only
  "reviews" count toward a PASS (closes the 20-08/20-16 attestation gap where 4 claimed reviews had no artifact).
- **D4.** Sandbox isolation MUST be preserved: the "drop deny-only SIDs" fix restores reads AND keeps the
  AppContainer boundary; prove a genuine DENY still blocks and a normal-SID-only grant is still denied.
- **D5.** NO cargo/clippy/nextest on the Mac. Authoritative Rust runs on Hetzner (Linux) + the self-hosted
  msvc Windows runner + the ephemeral macOS runner. `cargo fmt` / node tooling on Mac is OK.
- **D6.** Sean-only gates (never infer): native-proof dispatch, and the exact-tuple 20-18 `authorize <digest>`
  (prior `cb6f06bd…` spent/void; a new candidate SHA + any RED both require FRESH authorization).
- **D7.** Never claim native proof from cross-compilation or source inspection — only real-hardware runs.

## 3. Acceptance requirements (must all pass on ONE exact repaired candidate SHA)

Native fixes (from the BRIEF §6):
- **R1** `AppContainerBackend::is_available()` true on real Windows (`storage.rs` `.write(true)`).
- **R2** sandboxed read of granted files / deny ungranted / grants revoked / isolation preserved (drop deny-only `SidsToDisable`).
- **R3** `wcore-agent` compiles on `x86_64-pc-windows-msvc` (`READ_CONTROL`/`WRITE_DAC` from `Win32::Storage::FileSystem`).
- **R4** no Linux/macOS regression (Linux `nextest --profile ci` 11509/0; macOS proof green).
- **R5** `type_and_hold` asserts on granted-read success, not `choice.exe` exit index (stdin-free hold).
- **R6** `dispatch_smoke` Windows-portable (no `fs::rename` of an open dir).
- **R7** `windows-job-object` + `windows-hard-process-containment` targets map to REAL Windows Job-Object
  containment tests (must be authored — none exist): KILL_ON_JOB_CLOSE, active-process cap, breakaway denial,
  exit-code fidelity, descendant reaping with no residue.
- **R8** structural guard so a native proof target cannot map to a wrong-OS test.
- **R9** re-validate the macOS proof harness on real macOS (8 targets real + green; fix aspirational mappings).
- **R10** regenerate + commit `Cargo.lock`.
- **R11** Windows proof leg runs on an AppContainer-capable self-hosted msvc runner (not hosted `windows-2022`).
- **R12** repaired candidate passes: build → cross-audit → Hetzner aggregate → native proof (Win+mac) →
  fresh 20-16 (native NOT deferred) → 20-17 re-prep → Sean-authorized 20-18.

Audit addenda (from AUDIT-2026-07-23):
- **R13** every review gate emits a schema-validated artifact per claimed reviewer (see D3).
- **R14** re-prove on hardware the wcore-**sandbox** Windows COMPILE fix (windows_impl module-path debt) and
  the macOS test-mapping fix — the ACTUAL recorded 20-18 failures — not only the 07-23 AppContainer findings.
- **R15** reset tainted tree to pristine `be84bd2` before any build; write the real `process.rs` fix fresh (see D2).

## 4. Out of scope
- Phases 21–30 and the program controls (separate admission boundaries).
- Any change to accepted 20-01…20-16 source or their summaries.
- Product features beyond closing the native path.

## 5. Definition of done
One exact repaired candidate SHA with: Linux 11509/0 (Hetzner) + Windows native green (self-hosted msvc,
real AppContainer + Job-Object containment) + macOS native green (ephemeral runner) + a FRESH 20-16
independent review (native NOT deferred, schema-validated per reviewer) CLEAN + 20-17 re-prep + Sean-authorized
20-18 dual-platform green + aggregate Hetzner. Then Phase 20 requirements complete.
