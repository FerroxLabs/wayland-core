# Phase 20 — Native-Path Closure CONTEXT

**Source:** PRD Express Path (`20-NATIVE-REPAIR-PRD.md`) — no discuss-phase; scope was fixed by the
2026-07-23 whole-program audit (9 agents). This CONTEXT governs ONLY the additive native-repair successor;
accepted plans 20-01…20-16 are untouched.

<domain>
Close the Phase-20 native path: produce ONE repaired candidate (successor of `6937ef6`) that passes real
Windows + macOS native acceptance under Ferrox discipline, then re-run the 20-16 review / 20-17 prep / 20-18
UAT gates against it. The transactional delegated-mutation lifecycle itself is built and Linux-green; only the
native proof + a set of real defects (compile, sandbox boundary, test/harness bugs) remain.
</domain>

<decisions>
1. ADDITIVE only — never replan/renumber/re-execute accepted 20-01…20-16. (confirmed at exit)
2. HARD first step: reset working tree to pristine `be84bd2`; discard the uncommitted boundary-breaking
   `process.rs` diagnostic (full-token `current_token` swap + `DISABLE_MAX_PRIVILEGE=0` + WCORE_DIAG_TOKEN
   dump); write the real fixes fresh. (confirmed at exit)
3. Every review gate emits a schema-validated artifact PER claimed reviewer — no prose-only reviews count
   toward PASS. (confirmed at exit)
4. Sandbox isolation is preserved by the fix: dropping deny-only SIDs restores reads AND keeps the AppContainer
   boundary; prove a genuine DENY still blocks and a normal-SID-only grant is still denied. (confirmed at exit)
5. NO cargo/clippy/nextest on the Mac — Hetzner + self-hosted msvc + ephemeral macOS runners only; fmt/node OK.
   (confirmed at exit)
   AMENDMENT (2026-07-24, after 20-25 native RED): ONE narrow carve-out — a compile-only
   `cargo check -p wcore-sandbox --features live-docker --tests` on the Mac (type-check only, no test
   run, no Docker) is sanctioned as a pre-dispatch gate in 20-29/20-32 to catch macOS-only compile
   errors BEFORE spending a scarce Sean-authorized native run (20-25 burned a run on exactly such an
   E0599). Fail-safe: if the check cannot run on the Mac, the gate blocks rather than emitting a false
   green. No other Mac cargo build/clippy/nextest is permitted.
6. Sean-only: native-proof dispatch + exact-tuple 20-18 authorization (prior digest spent/void). (confirmed at exit)
7. Never claim native proof from cross-compilation or source inspection — real-hardware runs only. (confirmed at exit)
</decisions>

<canonical_refs>
- `.planning/inbox/native-uat-repair-BRIEF.md` — verified defect inventory + R1–R12 acceptance.
- `.planning/intel/inbox-2026-07-23/AUDIT-2026-07-23.md` — audit addenda r13–r15, provenance correction.
- `.planning/intel/inbox-2026-07-23/requirements.md` — R1–R12 full text + acceptance.
- `.planning/phases/20-transactional-delegated-mutation/20-18-SUMMARY.md` — the actual recorded 20-18 RED (5e665ec).
- `.planning/phases/20-transactional-delegated-mutation/20-16-SUMMARY.md` / `20-08-INDEPENDENT-REVIEW.md` — the one attested review of 6937ef6.
- Defect sites: `crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs`,
  `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs`,
  `crates/wcore-agent/src/session_journal/snapshot.rs:654`,
  `crates/wcore-sandbox/tests/{live_fs_acl.rs,hard_process_containment.rs}`,
  `crates/wcore-swarm/tests/dispatch_smoke.rs:289`,
  `scripts/f20-native-{windows-proof.ps1,macos-proof.sh,uat-proof.mjs}`.
</canonical_refs>

<scope_fence>
IN: reset-to-pristine hygiene; the real sandbox fixes (storage `.write(true)`; drop deny-only SIDs in
`process.rs`); `wcore-agent` snapshot.rs import; wcore-sandbox windows_impl COMPILE debt; test bugs
(`live_fs_acl` choice.exe, `dispatch_smoke` rename); author real Windows Job-Object containment tests +
repoint the 2 harness targets; wrong-OS-mapping guard; re-validate macOS harness; regenerate Cargo.lock;
self-hosted-runner runs-on; the verifier→request-writer gap in `f20-native-uat-proof.mjs`; then the full
gate sequence culminating in fresh 20-16 (schema-attested) → 20-17 → Sean-authorized 20-18.
OUT: phases 21–30; program controls; accepted 20-01…20-16 source/summaries; any product feature.
</scope_fence>

<success_criteria>
One exact repaired candidate SHA green on: Linux `nextest --profile ci` 11509/0 (Hetzner); Windows native
(self-hosted msvc — real AppContainer read/grant/revoke/isolation + real Job-Object containment); macOS native
(ephemeral runner, 8/8 real); FRESH 20-16 independent review (native NOT deferred; schema-validated per
reviewer) CLEAN; 20-17 re-prep; Sean-authorized 20-18 dual-platform green + aggregate Hetzner → Phase 20
requirements complete.
</success_criteria>

<specifics>
The planner must treat R1–R15 as the acceptance set and structure ADDITIVE plans (next free number is 20-19)
that: (a) do hygiene reset first, (b) construct + focused-prove the fixes, (c) author the missing real Windows
Job-Object tests + repoint harness targets + the wrong-OS guard, (d) re-validate macOS, (e) regenerate
Cargo.lock + change runs-on, then (f) route the repaired candidate through build → cross-audit → Hetzner
aggregate → native proof both OS → FRESH 20-16 (schema-attested, native NOT deferred) → 20-17 re-prep →
Sean-authorized 20-18. Focused proof only on Hetzner/self-hosted; NEVER on the Mac.
</specifics>
