---
phase: 20A-native-windows-macos-uat
plan: "01"
subsystem: testing
tags: [windows, macos, appcontainer, ci, nextest, native-uat, acl, job-object, cross-platform]

requires:
  - phase: 20-f20-native-uat
    provides: "the six-target native Windows proof script, its wrong-OS anti-drift guard, and the verifyNativeLog fail-closed marker verification whose target array this plan had to leave byte-identical"
provides:
  - "The compile verdict for the 155 Windows-only test bodies at one exact SHA, from the Windows compiler's own output on real hardware"
  - "An affirmative per-crate GREEN verdict for wcore-sandbox (105 of the 155 Windows-only tests) and wcore-agent"
  - "Four previously invisible Windows compile defects, enumerated with --keep-going, one of them repaired and three escalated"
  - "wcore-sandbox added to the recurring Windows soak — 105 Windows-only tests get a recurring execution path for the first time"
  - "The twelve live_fs_acl ACL tests and the orphaned native_containment_gate_marker wired into a non-proof runner at FILE level"
  - "A CI trigger that fires on this working branch — the branch's first CI run ever"
  - "A severity-classified finding register with every item routed"
affects: [20A-02, 20A-03, 20A-04]

tech-stack:
  added: []
  patterns:
    - "File-level ignored-set selection (--test <file> --run-ignored all --no-tests=fail) instead of name enumeration, so a later-added test cannot silently fall out of a runner"
    - "Trap-safe env assignment with a byte-exact proof-of-effect before any run that depends on it"
    - "Capability-routed runners: a live surface goes to the host that actually has the capability, rather than producing a permanent environmental red on a host that does not"

key-files:
  created:
    - .planning/phases/20A-native-windows-macos-uat/20A-01-BASELINE.md
    - .planning/BACKLOG.md
  modified:
    - crates/wcore-tools/tests/bash_sandbox_routing_test.rs
    - scripts/wayland-e2e-windows-soak.ps1
    - .github/workflows/ci.yml
    - .github/workflows/nightly-windows-soak.yml

key-decisions:
  - "Pinned the measurement SHA at 2a9d47ff (probe SHA b334d917 + the one sanctioned compile repair) so every downstream measurement is bound to a tree that actually builds"
  - "Gated the ungated unix-only test with #[cfg(unix)] rather than #[cfg(target_os = \"linux\")]: unix is the minimum that resolves E0433 and it leaves the test running on macOS, so a macOS result stays reportable instead of being gated away"
  - "Rejected workflow_dispatch for Wiring A — GitHub only exposes that trigger for workflows already on the default branch, and ci.yml on main has none, so it cannot fire against this ref"
  - "Routed Wiring C to the self-hosted msvc box rather than windows-2022, because the hosted server SKU reports AppContainerBackend::is_available() == false and would produce a guaranteed environmental red every night"
  - "Did NOT repair F-03 (unsafe impl Sync for WindowsJob): it is a soundness assertion in production src/, not a mechanical fix in the failing file, so the plan terminated compile-blocked exactly as its termination criterion requires"

patterns-established:
  - "Enumerate compile failures with --keep-going before concluding: cargo stops scheduling after the first error, and the first error here was masking three more"
  - "Prove a per-crate compile verdict affirmatively (exit 0 on a targeted build), never by absence from a failure list"

requirements-completed: []

coverage:
  - id: D1
    description: "Compile verdict for the 155 Windows-only test bodies at the pinned SHA, per crate, from the Windows compiler's own output"
    requirement: REQ-native-r3
    verification:
      - kind: integration
        ref: "SEANDESKTOP C:\\ferrox-win @2a9d47ff — cargo build --locked --workspace --all-targets --keep-going; cargo build --locked -p wcore-sandbox --all-targets (exit 0); cargo build --locked -p wcore-agent --all-targets (exit 0)"
        status: pass
    human_judgment: false
  - id: D2
    description: "wcore-agent's recorded 2026-07-22 Windows COMPILE defect re-proven fixed on real hardware rather than asserted from source"
    requirement: REQ-native-r14
    verification:
      - kind: integration
        ref: "SEANDESKTOP @2a9d47ff — cargo build --locked -p wcore-agent --all-targets -> AGENT_EXIT=0, no errors"
        status: pass
    human_judgment: false
  - id: D3
    description: "Pristine tree and --locked consistency established before any measurement"
    requirement: REQ-native-r15
    verification:
      - kind: integration
        ref: "SEANDESKTOP — git status --porcelain --untracked-files=all empty after removing the one recorded artifact; cargo build --locked raised no lockfile inconsistency"
        status: pass
    human_judgment: false
  - id: D4
    description: "wcore-sandbox added to the recurring Windows soak with all five original crates preserved"
    requirement: REQ-native-r1
    verification:
      - kind: other
        ref: "/usr/bin/grep -cF 'wcore-sandbox' scripts/wayland-e2e-windows-soak.ps1 -> 7; five-crate preservation loop -> all >= 1"
        status: pass
    human_judgment: false
  - id: D5
    description: "All twelve live_fs_acl ACL tests and the orphaned native_containment_gate_marker selected by a non-proof runner, with the proof script byte-identical"
    requirement: REQ-native-r2
    verification:
      - kind: other
        ref: "/usr/bin/git diff --exit-code -- scripts/f20-native-windows-proof.ps1 -> ZERO DIFF; grep -cF live_fs_acl -> 7; grep -cF run-ignored -> 2; grep -cF hard_process_containment_windows -> 6; twelve-name source check -> 12"
        status: pass
    human_judgment: false
  - id: D6
    description: "The newly wired PHASE L surface actually executes and its per-test outcome is recorded"
    requirement: REQ-native-r5
    verification: []
    human_judgment: true
    rationale: "NOT EXECUTED. The plan reached its compile-blocked termination state at Task 1, and Task 1's action mandates STOP + escalate. The wiring is landed and source-gate-proven; its first hardware execution belongs to whichever plan resumes after the F-03 decision."
  - id: D7
    description: "CI fires on this working branch so the macOS and self-hosted Windows legs compile this tree"
    requirement: REQ-native-r7
    verification:
      - kind: integration
        ref: "gh run list -R FerroxLabs/wayland-core --branch plan/f20-unified-audit-repair -> CI [30151510189] @95552f64 (previously [])"
        status: pass
    human_judgment: false
  - id: D10
    description: "Compile verdict for the 23 macOS-only test bodies"
    requirement: REQ-native-r3
    verification:
      - kind: integration
        ref: "CI 30151510189 job CI (macos-latest) step 11 Clippy (warnings = errors) -> FAILURE on 2 -D warnings lints in crates/wcore-sandbox/src/backends/process_tree.rs; test targets never type-checked"
        status: fail
    human_judgment: true
    rationale: "NOT ANSWERED. No genuine macOS compile error exists — only two lints — but -D warnings aborted the wcore-sandbox lib unit before clippy reached the test targets where all 23 macOS-only tests live. Two trivial lint fixes unlock a definitive verdict; M5 round-trip budget is untouched at zero used."
  - id: D11
    description: "F-09 — the Windows CI leg fails at Clippy before reaching Run tests, an 11-lint surface strictly larger than the local cargo build probe found"
    verification:
      - kind: integration
        ref: "CI 30151510189 job CI (Array) step 11 -> FAILURE; steps 12-16 skipped"
        status: fail
    human_judgment: true
    rationale: "Recorded and routed to 20A-02, not fixed — this plan does not repair behaviour or hygiene, and it had already reached its compile-blocked terminal state."
  - id: D8
    description: "Four-suite re-measured Windows baseline with every failure named and bucketed"
    requirement: REQ-native-r4
    verification: []
    human_judgment: true
    rationale: "NOT PERFORMED — plan terminated compile-blocked at Task 1 before the re-measure cycle. The prediction is carried forward in 20A-01-BASELINE.md §6 and is explicitly still a prediction."
  - id: D9
    description: "F-03 disposition — whether WindowsJob's Win32 Job Object HANDLE is safe to share across threads"
    verification: []
    human_judgment: true
    rationale: "Requires an unsafe soundness judgement about Win32 handle semantics in production source. Sean decision, escalated."

duration: 95min
completed: 2026-07-25
status: complete
---

# Phase 20A Plan 01: Native Windows/macOS Baseline Summary

**The 105 Windows-only `wcore-sandbox` tests DO compile — but proving it uncovered six previously invisible native defects (four Windows compile errors plus two `-D warnings` gates that stop CI before it ever reaches the tests), one repaired and five escalated, and the plan terminated COMPILE-BLOCKED on an `unsafe impl Sync` soundness decision only Sean can make.**

## Termination state: **2 — COMPILE-BLOCKED** (of the plan's three defined states)

## Performance

- **Duration:** ~95 min
- **Tasks:** 2 of 3 executed; Task 3 not started (terminal state reached in Task 1)
- **Files modified:** 4 source/config + 2 planning documents

## Accomplishments

- **Settled the audit's top "could not determine" item.** `wcore-sandbox` (105 of the 155 Windows-only tests — every retained-handle security proof, every ACL boundary test, the whole Job-Object surface) and `wcore-agent` both **compile clean on Windows**, proven affirmatively with targeted `--all-targets` builds returning exit 0, not inferred from absence in a failure list.
- **Found four Windows compile defects that a Linux-only green suite structurally cannot see**, all pre-existing (`git diff ce9a11a6 b334d917 -- crates/` is empty). The first was masking the other three — `cargo build` stops scheduling after an error, so `--keep-going` was needed to enumerate the real set.
- **Established that the Windows leg of `ci.yml` could not have produced a test result on this tree at all** (F-05): `cargo nextest run --workspace` builds first, and the build fails. Any "Windows CI green" claim for this branch would have been vacuous.
- **Fired CI on this branch for the first time ever.** `gh run list --branch plan/f20-unified-audit-repair` previously returned `[]`; it now returns run `30151510189`.
- **Closed the two CRITICAL wiring threats.** 105 Windows-only tests got a recurring execution path; the ten orphaned ACL tests and the orphaned containment gate marker got a runner — without touching the six-target proof script.
- **Resolved the SHA record discrepancy** the plan flagged: `c39f7254` and `ce9a11a6` have identical code trees, so the two records never actually disagreed.

## Task Commits

1. **Task 1: compile verdict + the one sanctioned repair** — `2a9d47ff` (fix)
2. **Task 2: the three wirings** — `95552f64` (ci)
3. **Task 3: re-measure** — NOT EXECUTED (terminal state reached in Task 1)

**Plan metadata:** see final commit (BASELINE + BACKLOG + SUMMARY)

## The SHA the box actually printed

The plan required reading, not assuming. **The box printed `ce9a11a6a8f62b7214f443d1a6a174a3af1c48fb`** — TEST-AUDIT was right. It was **not pristine**: one untracked `crates/wcore-swarm/.swarm-status.json`, recorded then removed by exact path (never `git clean`) before anything was measured.

The apparent `c39f7254` vs `ce9a11a6` conflict dissolves under measurement: `merge-base --is-ancestor` says YES and `git diff --stat c39f7254 ce9a11a6 -- crates/ .github/ scripts/ justfile .config/` is **empty**. Identical code trees. The plan's predicted `.config/nextest.toml` +32 lines and five desktop contract fixtures delta is real and lands on `ce9a11a6 → b334d917`, with **no `crates/**/*.rs` change**.

| | SHA |
|---|---|
| Box prior SHA (read, not assumed) | `ce9a11a6` |
| Probe SHA | `b334d917` (tree `591ebbf7`) |
| **Pinned measurement SHA** | **`2a9d47ff`** |

## Compile verdict

| Crate | Verdict on Windows @`2a9d47ff` |
|---|---|
| **`wcore-sandbox`** (105 of the 155) | **COMPILES — exit 0** |
| **`wcore-agent`** (REQ-native-r3/r14) | **COMPILES — exit 0** |
| `wcore-tools` (test `bash_sandbox_routing_test`) | was BROKEN — **repaired here** (F-01) |
| `wcore-eval-scenarios` (test `runner_contracts`) | **BROKEN** — F-02, **F-03** |
| `wcore-skills` (lib test) | **BROKEN** — F-04 |

**macOS: NOT ANSWERED — and this is the one Task 1 deliverable this plan did not land.**
CI run `30151510189`'s macOS leg failed at `Clippy (warnings = errors)` on **two `-D warnings`
lints** (`dead-code` on `signal`, `clippy::needless-return`) in
`crates/wcore-sandbox/src/backends/process_tree.rs`, aborting the `wcore-sandbox` lib unit
before clippy reached that crate's test targets — which is where all 23 macOS-only tests
live. `--all-targets` was requested; it was not reached.

There is **no genuine macOS compile error anywhere** — only two lints — so macOS is in a
much better position than Windows. But "no error seen" is not "verified", and this plan
does not claim the latter. Two trivial lint fixes unlock a definitive verdict; the M5 cap
of two compile-repair round-trips is **untouched at zero used** (the one macOS re-run was a
retry of a cancelled job, not a repair round-trip). See BASELINE §5.1.3.

## The CI run found something the local probe could not

`CI (Array)` (self-hosted Windows) also failed at **Clippy**, not at the test step — 11
`-D warnings` lints across `wcore-eval-scenarios`, `wcore-swarm` and `wcore-tools`, all in
`cfg(windows)` code (**F-09**). This matters more than it first looks:

- It is a **strictly larger** failure surface than the local `cargo build --workspace
  --all-targets` probe found — that probe passes for `wcore-swarm` and `wcore-tools`.
- It means the Windows leg fails at step 11 and **never reaches `Run tests`**, so CI would
  never have discovered F-01..F-04 either. Two independent gates, both closed, both silent.
- Anyone repairing the Windows leg must satisfy **both** gates; fixing F-01..F-04 alone
  leaves it red.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Windows compile break in `bash_sandbox_routing_test.rs` (F-01)**
- **Found during:** Task 1 (the compile probe — this is what the task exists to find)
- **Issue:** `delegated_mutation_required_live_sandbox_confines_parent_and_descendants` had no cfg gate while its body opens `use std::os::unix::fs::symlink;`. E0433 on Windows, taking the whole test binary — all 19 tests — down with it. The file's two sibling live-sandbox tests are already gated (`cfg(unix)` :261, `cfg(target_os = "linux")` :299).
- **Fix:** added `#[cfg(unix)]`. Single attribute, single file, explicitly within Task 1's sanctioned "mechanical cfg-gate defect confined to the failing file".
- **Verification:** re-ran `cargo build --locked --workspace --all-targets` on SEANDESKTOP — this error is gone.
- **Committed in:** `2a9d47ff`

### Judgement calls worth flagging

**Gate choice `#[cfg(unix)]`, not `#[cfg(target_os = "linux")]`.** The test's doc says "Required Linux live acceptance" and it asserts `platform_enforces_read_deny()`, so `linux` arguably matches intent better — which is exactly why it was rejected. Narrowing to `linux` would also remove it from macOS, where it runs today and where macOS CI has never executed this tree, silently gating away a macOS result nobody has seen. `unix` is the minimum that resolves E0433 and changes no platform's behaviour except Windows.

**Enumerating with `--keep-going`.** After repairing F-01 the build failed again on a different unit. Rather than discover defects one SSH round-trip at a time, the workspace was rebuilt with `--keep-going` to enumerate every failing unit at once. This is recording, not repairing, and it is what turned "one more error" into a complete, actionable escalation package.

**Not repairing F-02 and F-04 despite both being mechanical.** F-02 (`SYNCHRONIZE` imported from `Win32::System::Threading` instead of `Win32::Foundation`) shares a compilation unit with F-03, so fixing it cannot make that unit build. F-04 (`ACCESS_ALLOWED_ACE_TYPE`) is in a different crate and outside the scope fence. Landing either would leave the tree half-repaired across an escalation boundary for no gain.

**Nothing was weakened.** No assertion relaxed, no `#[ignore]`, no `#[allow]`, no timeout raised, no test deleted. `native_containment_gate_marker` — the audit's "FIX or DELETE" — was resolved as **FIX**.

**Total deviations:** 1 auto-fixed (Rule 1). **No scope creep**; three further defects were recorded and routed rather than fixed.

## THE ESCALATION — F-03, why this plan stopped

```
error[E0277]: `*mut c_void` cannot be shared between threads safely
    --> crates\wcore-eval-scenarios\tests\runner_contracts.rs:707:16
     = help: within `ProcessTree`, the trait `Sync` is not implemented for `*mut c_void`
note: required because it appears within `process_tree::windows::WindowsJob`
    --> crates\wcore-eval-scenarios\src\process_tree.rs:491:23
     = note: required for `&ProcessTree` to implement `Send`
note: required by a bound in `tokio::spawn`   (F: Future + Send + 'static)
```

`src/process_tree.rs:491` declares `pub(super) struct WindowsJob(HANDLE)` and asserts only half of what is needed:

```rust
// SAFETY: this wrapper uniquely owns a process-wide kernel handle.
unsafe impl Send for WindowsJob {}
```

`Send` yes, **`Sync` no**. `reap_child(&self, …)` is an `async fn` holding `&ProcessTree` across an `await`, and `&T: Send` requires `T: Sync`; that propagates through six `async fn` bodies in `process_tree.rs` and `runner.rs` and collides with `tokio::spawn`'s `Send` bound.

**Every clause of termination state 2 is satisfied:** the repair site (`src/process_tree.rs`) is **not the failing file** (`tests/runner_contracts.rs`), it is **production source** which the scope fence excludes, and `unsafe impl Sync` is a **soundness assertion** about concurrent Win32 Job Object handle access — not a module-path, import or cfg fix. The existing SAFETY comment scopes the claim to *unique ownership*, which is precisely the weaker claim; widening it to `Sync` contradicts that reasoning and needs an author. Every alternative (restructuring the `tokio::spawn`, `&mut self` on `reap_child`/`terminate`, changing `Backend`'s representation) is a redesign, and the plan says *"Do not redesign anything to make it compile."*

**Sean's decision:** is `WindowsJob`'s handle genuinely safe to share across threads (`unsafe impl Sync` + a SAFETY note superseding the unique-ownership comment), or should the ownership model be tightened so the `Sync` requirement never arises? Route the outcome to 20A-02 alongside F-02 and F-04.

## Issues Encountered

- `git fetch --all --prune` on the box silently brought nothing down; `git fetch origin <branch>` worked. Fetch the branch by name.
- Inside `cmd /c`, `^` is the escape character, so `HEAD^{tree}` reaches git as `HEAD{tree}`. Use `HEAD^^{tree}`.
- `findstr` quoting through the ssh → powershell → cmd chain mangles; capture raw output and filter on the Mac with `/usr/bin/grep`.
- `pwsh` is unavailable on the Mac, so the edited soak script was parse-proven on the box instead: **0 errors before, 0 errors after** under `pwsh` 7, the interpreter the workflow actually invokes. Under Windows PowerShell **5.1** the ORIGINAL file already yields 7 errors and the modified one 8 — a pre-existing UTF-8 decode artifact of the script's `═`/`✓`/`✗`/`·` glyphs, in code this plan never touched. Logged as BACKLOG F-08 (LOW). The raw "8 errors" is not evidence of a defect introduced here; 0 vs 0 on the real interpreter is the correct comparison.
- Both workflow YAMLs parse cleanly, with the expected triggers and job sets.
- `CI (macos-latest)`, `CI (linux-containerized)` and `Build (aarch64-pc-windows-msvc)` were cancelled mid-flight on the first run — not concurrency (one run only, no second push) and not fail-fast (`fail-fast: false`). Cause not established; recorded as F-11 (LOW) rather than guessed at. Re-running the macOS job cleared it.

## Next Phase Readiness

**BLOCKED on Sean for F-03.** Two cheap unlocks are waiting behind it:

1. **Two lint fixes** in `crates/wcore-sandbox/src/backends/process_tree.rs` (F-10) + one re-run of `CI (macos-latest)` gives a definitive macOS-only compile verdict. The M5 round-trip budget is untouched at zero used.
2. **F-09's 11 Windows lints** must be fixed alongside F-01..F-04, or the Windows leg stays red at step 11 regardless of the compile repairs.

Once decided:

- `wcore-sandbox` and `wcore-agent` compile clean, so 20A-02's and 20A-03's four-suite measurements are runnable immediately — the compile blocker is in `wcore-eval-scenarios`/`wcore-skills`, which none of the four suites builds against.
- The wiring is landed and source-gate-proven; PHASE L's first hardware run is the natural first act of the resuming plan.
- `scripts/f20-native-windows-proof.ps1` has a **zero diff**, so 20A-04's six-target invariant is intact.
- Blocker A (AppContainer retained-workspace-authority bind) is untouched and still 20A-02's.
- **BACKLOG M6 (self-hosted CI contention) is now live**: Wiring A makes `ci.yml` fire on this branch and its Windows leg shares the box with every downstream measurement. First hypothesis for any inexplicable result.
- `.planning/BACKLOG.md` flags that `crates/wcore-swarm/.swarm-status.json` is not gitignored and dirties the checkout on every `wcore-swarm` run — a plausible contributing cause of the checkout-dirty item routed to **20A-03**, worth checking there first.

---
*Phase: 20A-native-windows-macos-uat*
*Completed: 2026-07-25*

## Self-Check: PASSED

- Files created/modified: all 7 present on disk.
- Task commits `2a9d47ff`, `95552f64`: both present in `git log --all`.
- `cargo fmt --all -- --check`: clean.
- `scripts/f20-native-windows-proof.ps1`: ZERO DIFF across both task commits.
- All twelve `live_fs_acl` test names present in source: 12/12.
- `#[ignore]` / `#[allow]` added across all `crates/` changes: **0**.
