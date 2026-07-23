# BRIEF — Phase 20 Native-UAT Repair (Windows + macOS native proof machinery)

> **Document type:** PRD / repair brief — a Ferrox Factory *source document* intended to be ingested
> (`/ferrox-ingest-docs`), researched, brainstormed, and planned into structured phases. It is NOT a
> hand-executed task list. Everything below is evidence-backed from a live hardware investigation; the
> job of the Ferrox flow is to turn it into a disciplined, reviewed, gated plan.
>
> **Scope owner:** core lane (area:core). **Milestone:** v1.0 / Phase 20 (transactional-delegated-mutation).
> **Status:** DISCOVERY COMPLETE — awaiting structured planning. **Date:** 2026-07-23.
> **Precedence:** this brief captures newly-verified reality; where it conflicts with prior 20-08/20-16
> assumptions about native validation, this brief wins (those assumptions deferred native and were never tested).

---

## 1. Executive summary

Phase 20's **native-UAT machinery (20-08 scaffolding + `20-18` native proof) was never actually run on
real Windows or macOS hardware.** The 20-16 independent review that "accepted" 20-08 **explicitly deferred
`native_windows` / `native_macos`** — so nothing certified that the Windows AppContainer sandbox or the
native proof harness worked. The 20-18 native UAT — which exists precisely to catch this — went **RED**, and
a live investigation on real Windows 11 hardware found the native path is broken in several distinct ways.

The good news: the **security boundary itself is now understood and the two core sandbox defects are fixed
and proven on real hardware.** The remaining failures are in the **never-run test/proof harness**, not the
sandbox. This brief consolidates the complete, verified defect inventory and the repair scope so the rebuild
can proceed as a disciplined Ferrox phase (repaired 20-08 successor → fresh 20-16 review → 20-17 → 20-18 →
Sean authorize), rather than ad-hoc fixes.

**One-line ask of Ferrox:** ingest this, research the open questions (§7), brainstorm the harness redesign
(§6.C), and produce a phased plan that repairs the native path with full build discipline and gate coverage.

---

## 2. Background — how we got here (context, not blame)

- Phase 20 delivered a transactional delegated-mutation lifecycle (plans 20-01 … 20-16 complete; 20-17/20-18
  pending). Candidate commit `6937ef6` (working commit `be84bd2`) is **Linux-green** (`nextest --profile ci`
  = 11509/0) and **macOS 8/8** on the prior proof run.
- 20-16 (fresh independent review of the 20-08 product) returned **CLEAN but with `native_macos` and
  `native_windows` DEFERRED**. The native surface was therefore never validated by review — by design, the
  native UAT (20-18) is the gate that validates it.
- The `wcore-sandbox` Windows AppContainer implementation (`crates/wcore-sandbox/src/backends/appcontainer/`,
  ~4,700 lines: `acl_lease/` + `windows_impl/`) exists **only on the Phase-20 branch, not on `origin/main`**
  (`ea3bb1c`). It had **never compiled or run on real Windows** before this investigation.
- A live investigation used a real **Windows 11 Pro client** (AppContainer-capable) reachable over Tailscale
  as the dev/test host. That surfaced the defects below. (The investigation itself drifted from Ferrox
  discipline — that drift is the reason for this consolidation brief.)

---

## 3. Problem statement

The Phase 20 **native proof cannot pass as it stands** because:
1. The Windows AppContainer sandbox had real runtime defects preventing it from functioning at all (now
   root-caused; core fixes proven).
2. `wcore-agent` does not compile on Windows (a bad import).
3. Several native acceptance tests are broken (wrong exit-code assumptions, non-portable filesystem ops).
4. The Windows proof **harness itself is mis-authored** — two of its six "Windows" acceptance targets are
   wired to **Linux-only Bubblewrap tests** that can never pass on Windows.
5. The same "aspirational native machinery" pattern previously bit the macOS proof (per project memory), so
   the macOS harness must be re-examined too, not assumed good.

Until these are repaired under review, 20-18 cannot go green and Phase 20 cannot close.

---

## 4. Verified findings — the defect inventory (evidence-backed)

Each item below was observed first-hand on real Windows 11 hardware. Severity is for planning/ordering.

### 4.A — Windows sandbox: `is_available()` false everywhere  *(CORE — root-caused, FIX PROVEN)*
- **Symptom:** `AppContainerBackend::is_available()` returned false on *every* Windows environment (hosted
  runner *and* real client), so the sandbox refused to run at all. (This — not "AppContainer is unavailable
  on hosted windows-2022" — is the true reason the earlier hosted run failed at 0.02s.)
- **Root cause:** `crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs`
  `create_new_nofollow()` builds `OpenOptions` with `.access_mode(GENERIC_READ|GENERIC_WRITE)` +
  `.create_new(true)` but **no `.write(true)`**. On Windows, std's `get_creation_mode` validates the
  high-level write/append flags independently of `access_mode`; `create_new` without `.write(true)` →
  `InvalidInput` ("creating or truncating a file requires write or append access") → the ACL-lease file is
  never creatable → the probe fails.
- **Fix (proven):** add `.write(true)` to that `OpenOptions` chain. Verified: probe returns exit 0,
  `is_available()` → true on real hardware.

### 4.B — Windows sandbox: sandboxed processes cannot read ANY file  *(CORE — root-caused, FIX PROVEN)*
- **Symptom:** after 4.A, a sandboxed process could *run* (`cmd /c echo` → exit 0, output captured) but could
  **read no file at all** — not `fs_read_allow`-granted files, not files manually granted to ALL APPLICATION
  PACKAGES on the full ancestor chain, not even world-readable `C:\Windows\win.ini`.
- **Root cause:** `windows_impl/process.rs` `execute_blocking()` builds the sandbox token via
  `CreateRestrictedToken` marking `BUILTIN\Administrators` / `BUILTIN\Users` / `Authenticated Users` as
  **deny-only** (`SidsToDisable`). That deny-only marking **breaks the AppContainer package-SID grant path** —
  the child then has no usable enabled SID for any file's DACL. (Confirmed by a controlled matrix: identical
  files read exit 0 with deny-only OFF, exit 1 with deny-only ON.)
- **Isolation is preserved:** the AppContainer access model *intrinsically* ignores normal SIDs
  (Everyone/Users) for granting — proven: a file granted only to `Everyone` is still DENIED to the sandbox
  with deny-only OFF. So the deny-only marking is **redundant**, and removing it restores reads **without
  weakening the sandbox**.
- **Fix (proven):** drop the `SidsToDisable` (pass 0/null in `CreateRestrictedToken`) and remove the now-dead
  SID-allocation block. Verified: the core acceptance test `granted_path_is_readable_then_revoked` (read +
  grant + revoke) goes **green** on real hardware.
- **Discarded hypotheses (do not re-litigate):** `CreateAppContainerToken`/explicit LowBox rewrite (neither
  necessary nor sufficient — spike-tested); `S-1-15-2-1` missing from TokenGroups (red herring — real
  AppContainers lack it there too; the package SID lives in `TokenAppContainerSid`); `DISABLE_MAX_PRIVILEGE` /
  bypass-traverse; restricted-vs-normal base token; the box/AV.

### 4.C — `wcore-agent` does not compile on Windows  *(REAL bug — fix identified)*
- **Symptom:** `error[E0432]: unresolved imports windows_sys::Win32::Security::READ_CONTROL, WRITE_DAC`.
- **Root cause:** `crates/wcore-agent/src/session_journal/snapshot.rs:654` imports `READ_CONTROL`/`WRITE_DAC`
  from `Win32::Security`; in `windows-sys 0.59` they live in `Win32::Storage::FileSystem` (the crate already
  enables that feature).
- **Fix:** correct the import module. (Blocks the `windows-f20-lifecycle` target.)

### 4.D — `type_and_hold` test uses a wrong exit-code assumption  *(TEST bug — not a sandbox defect)*
- **Symptom:** `live_fs_acl.rs::one_execution_grant_never_leaks_to_another_identity` fails at the
  `assert_eq!(A.exit_code, 0)`.
- **Root cause:** the helper holds the process alive with `choice.exe /T 3 /D Y`; `choice.exe` returns the
  1-based index of the selected option, so default `Y` → **exit 1**, never 0 (verified on hardware). The
  sandbox behavior is actually correct (B *was* denied; A *did* read). The assertion is impossible as written.
- **Fix:** replace the hold mechanism so the command's exit reflects the granted read's success (0), while
  keeping a stdin-free delay (note: `timeout.exe` fails under redirected stdin — that's presumably why
  `choice` was chosen; the fix must preserve a no-interactive-stdin hold).

### 4.E — `dispatch_smoke` test uses a non-portable filesystem op  *(TEST bug)*
- **Symptom:** `wcore-swarm::dispatch_smoke::dispatch_rejects_different_head_repository_replacement` panics at
  `std::fs::rename(...).unwrap()` → `Os { code: 5, PermissionDenied }`.
- **Root cause:** the test renames a repo directory that still has open handles; Windows cannot rename a
  directory with open handles (Unix-rename assumption). Unrelated to the sandbox.
- **Fix:** make the test portable (drop handles before rename / restructure), or narrow the Windows target to
  the acceptance-relevant test rather than the whole binary.

### 4.F — Windows proof harness wired to LINUX tests  *(HARNESS defect — the systemic one)*
- **Symptom:** `windows-job-object` and `windows-hard-process-containment` targets fail/skip on Windows.
- **Root cause:** `scripts/f20-native-windows-proof.ps1` maps those two Windows targets to tests in
  `crates/wcore-sandbox/tests/hard_process_containment.rs`, which is **entirely Bubblewrap (Linux-only)** —
  no Windows backend anywhere. They can never pass on Windows.
- **Fix:** repoint these targets to **real Windows containment tests (Job Object)** — which **do not exist
  yet** and must be authored (the Windows sandbox uses Job Objects: `KILL_ON_JOB_CLOSE`, active-process cap,
  breakaway denial, UI restrictions — all present in `windows_impl/process.rs`, but untested).

### 4.G — Systemic: native machinery is "aspirational"  *(pattern to design against)*
- Both the Windows proof harness (this brief) and the macOS proof harness (project memory: macOS proof
  referenced a Windows-only test; several targets needed authoring) were written **without ever running on
  the target OS**. The repair must therefore **re-validate the macOS harness too**, not assume it green, and
  should add a guard so native targets can't silently map to the wrong OS again.

---

## 5. What is already fixed vs. what remains

| Area | State |
|---|---|
| 4.A `storage.rs .write(true)` | Fix proven on hardware — **spike only, uncommitted** |
| 4.B drop deny-only SIDs | Fix proven on hardware (core read/grant/revoke green) — **spike only, uncommitted** |
| 4.C `wcore-agent` import | Fix identified (1-line) — **not built/committed** |
| 4.D `type_and_hold` | Diagnosed — fix designed, not written |
| 4.E `dispatch_smoke` | Diagnosed — fix approach known, not written |
| 4.F Windows containment tests | Diagnosed — **tests must be authored** |
| 4.G macOS harness re-validate | Not started |
| Cargo.lock | **STALE** at `be84bd2` (build adds `cap-std`/`serde_json`/`tar` to wcore-sandbox; `fd-lock`/`libc`/`sha2`/`windows-sys` to wcore-swarm) → must regenerate + commit |

> **Discipline note:** none of the "proven" fixes are committed on the candidate branch. They are spikes in a
> throwaway Windows worktree, to be re-derived cleanly under the plan. The local `waylandcore-ferrox` working
> tree has diagnostic edits in `windows_impl/process.rs` that must be **reset to pristine `be84bd2`** before
> any real candidate; only the intent of 4.A/4.B carries forward.

---

## 6. Repair scope (requirements the plan must satisfy)

**A. Sandbox correctness (code).**
- R1: `is_available()` returns true on a real AppContainer-capable Windows host (fix 4.A).
- R2: a sandboxed process can read `fs_read_allow`-granted files and is denied ungranted files, with grants
  revoked after the run (fix 4.B), **with isolation preserved** (a genuine DENY ace still blocks; a file
  granted only to normal SIDs is still denied).
- R3: `wcore-agent` compiles on `x86_64-pc-windows-msvc` (fix 4.C).
- R4: no regression to Linux (`nextest --profile ci` stays 11509/0) or macOS (8/8).

**B. Test correctness.**
- R5: `type_and_hold` asserts on the granted read's success, not `choice.exe`'s index (fix 4.D).
- R6: `dispatch_smoke` Windows-portable or the target narrowed (fix 4.E).

**C. Proof-harness rebuild (design + author).**
- R7: `windows-job-object` and `windows-hard-process-containment` targets map to **real Windows Job-Object
  containment tests** that exercise the actual Windows mechanism (author new tests). [needs research/brainstorm]
- R8: a structural guard so a Windows proof target cannot map to a non-Windows test (and vice-versa for
  macOS) — e.g. per-target `#[cfg(target_os=...)]` assertions or a harness lint.
- R9: re-validate the **macOS** proof harness against real macOS (re-confirm the 8 targets are real + green;
  fix any aspirational mappings the same way).

**D. Candidate hygiene + gate.**
- R10: regenerate + commit `Cargo.lock`.
- R11: the Windows proof leg runs on an **AppContainer-capable self-hosted runner** (client SKU), not hosted
  `windows-2022` — the workflow `runs-on` must change accordingly (self-hosted msvc labels).
- R12: the repaired candidate goes through the **full gate sequence**: build → cross-audit → Hetzner
  aggregate → native proof (Windows + macOS) → **fresh 20-16 independent review** → 20-17 re-prep → 20-18
  Sean-authorized native UAT.

---

## 7. Open questions for Ferrox research / brainstorm

1. **Windows Job-Object containment tests (R7):** what is the correct acceptance surface for Windows hard
   containment? (Descendant reaping on job close, active-process cap, breakaway denial, exit-code fidelity on
   zero/non-zero terminal paths — mirror the intent of the Linux bwrap tests but via Windows Job Objects.)
   How should a "detached descendant is reaped with no residue" test be written on Windows?
2. **`type_and_hold` hold primitive (R5):** best stdin-free, exit-0 delay usable inside an AppContainer
   (candidates: a tiny purpose-built sleeper, `ping -n` to loopback — needs network cap?, a cmd busy-wait).
3. **macOS harness (R9):** which of the 8 macOS targets are real vs aspirational; what needs authoring.
4. **Harness anti-drift guard (R8):** cleanest enforcement that native targets match their OS.
5. **Review scope (R12):** does dropping the deny-only SIDs (a sandbox-hardening change) require any
   additional adversarial security review beyond standard 20-16, given it touches the isolation boundary?
   (Evidence says isolation is intrinsic to the AppContainer, but this should be adversarially re-confirmed.)

---

## 8. Constraints, non-goals, invariants

- **Security boundary:** the sandbox is a security boundary. Every change to token construction must preserve
  isolation and be adversarially reviewed. Do NOT weaken the sandbox to pass a test.
- **Cross-platform discipline:** all platform-specific behavior stays behind the central `wcore_config::shell`
  / `cfg(...)` conventions; no scattered platform detection (per AGENTS.md).
- **No drift outside plan:** all fixes land as reviewed, gated commits on the candidate branch — no ad-hoc
  hardware edits carried into the candidate. Spikes are re-derived under the plan.
- **Non-goals:** no refactor of the AppContainer implementation beyond what R1–R11 require; no new sandbox
  features; no changes to already-accepted 20-01…20-16 source outside the native surface without re-review.
- **Infra facts (reference):** an AppContainer-capable Windows 11 client with online self-hosted
  `wcore-core` msvc runners is available (labels `self-hosted, Windows, X64, msvc`); the macOS native leg
  runs on an ephemeral Scaleway Apple-silicon runner and REQUIRES `DOCKER_HOST` exported into the runner
  process env (colima socket).

---

## 9. Acceptance criteria (Phase 20 native path "done")

- All Windows native proof targets green on an AppContainer-capable self-hosted runner (real reads + real
  containment), all macOS native targets green, Linux aggregate unchanged (11509/0).
- Repaired candidate passes a fresh independent 20-16 review (no deferred native), 20-17 re-prep, and a
  Sean-authorized 20-18 native UAT.
- `Cargo.lock` consistent; no uncommitted spikes; harness anti-drift guard in place.

---

## 10. Reference appendix (evidence pointers)

- Candidate: `6937ef6` (tree `6db6fc85`); working commit `be84bd2` (branch `plan/f20-unified-audit-repair`).
- `origin/main` = `ea3bb1c` (does NOT contain the AppContainer implementation → never validated).
- Key source: `crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs`
  (`create_new_nofollow`, ~L410); `.../windows_impl/process.rs` (`execute_blocking`, `CreateRestrictedToken`
  ~L367, `SidsToDisable` block ~L330-355); `crates/wcore-agent/src/session_journal/snapshot.rs:654`.
- Tests: `crates/wcore-sandbox/tests/live_fs_acl.rs` (`type_and_hold` L118; the 9 native cases);
  `crates/wcore-sandbox/tests/hard_process_containment.rs` (**all Bubblewrap/Linux**);
  `crates/wcore-swarm/tests/dispatch_smoke.rs:289`.
- Harness: `scripts/f20-native-windows-proof.ps1` (6 target→test map, 2 wired to Linux tests);
  `scripts/f20-native-macos-proof.sh`.
- Proof matrix evidence (deny-only ON vs OFF; `choice.exe` exit 1; token IsAppContainer=1 with no
  `S-1-15-2-1` in groups) captured during the 2026-07-23 investigation.
