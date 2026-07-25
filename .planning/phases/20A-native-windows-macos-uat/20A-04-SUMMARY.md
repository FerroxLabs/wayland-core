---
phase: 20A-native-windows-macos-uat
plan: "04"
subsystem: infra
tags: [windows, macos, native-uat, appcontainer, candidate-seal, dispatch-gate, sean-gate]
status: complete
termination_state: 4

requires:
  - phase: 20A-native-windows-macos-uat
    provides: "20A-02 termination state 1 (the AppContainer retained-workspace-authority bind shipped at c252d01d) and 20A-03 termination state 2 (REFUTED-NO-DEFECT)"
provides:
  - "The measured six-target native Windows proof result at the sealed SHA: 4 GREEN, 2 RED, both reds classifying to previously-recorded classes and neither NEW"
  - "The sealed candidate SHA 50cf00b3 pinned to refs/f20a/candidate, with per-host pristine confirmation on Mac, Hetzner and SEANDESKTOP"
  - "Per-requirement code-presence findings at the sealed SHA for REQ-native-r1, r2, r5 and r7 — all four PRESENT with evidence"
  - "Three independent, previously-unrecorded blockers on the dispatch itself: the failed go/no-go, an unresolvable macOS runner label, and a dispatch ref that cannot bind to the sealed SHA"
  - "The fully-formed but UNFIRED dispatch command, recorded for Sean as reporting only — not as a prepared tuple"
affects: [20A-05-or-successor]

tech-stack:
  added: []
  patterns: []

key-files:
  created:
    - .planning/phases/20A-native-windows-macos-uat/20A-04-SUMMARY.md
  modified: []

key-decisions:
  - "Terminated in state 4 (blocked upstream) rather than sealing for dispatch: the plan's own mandatory local six-target dry-run went RED on 2 of 6, which the plan names as the go/no-go precondition"
  - "Did NOT create 20A-04-CANDIDATE.md: the plan's termination state 4 says to stop before preparing the tuple when the dry-run fails after sealing, and creating that artifact IS the act of preparing it"
  - "Ran targets 5 and 6 individually with the script's byte-identical selectors after the script threw at target 4, because the script fails closed on the first red and would otherwise have left a third of the six unmeasured"
  - "Did not fix, re-gate, ignore or re-time any red, and did not touch $targets — proven byte-identical to the sealed tree by blob hash"

metrics:
  duration: "~50 min"
  completed: 2026-07-25
  targets_green: 4
  targets_red: 2
  requirements_completed: 0
---

# Phase 20A Plan 04: Native Proof Candidate Seal Summary

Six-target native Windows proof measured end to end on SEANDESKTOP at the sealed SHA: **4 GREEN, 2 RED**. Both reds classify to previously-recorded classes; neither is NEW. Plan terminates in **state 4 (blocked upstream)**. No dispatch fired, no requirement completed, no authorization spent.

---

## 1. Termination state

**State 4 — Blocked upstream. Do not seal for dispatch, do not spend the authorization.**

The plan names one go/no-go precondition (Task 1, `<action>`): *"ALL SIX TARGETS GREEN LOCALLY IS THE GO/NO-GO PRECONDITION. Anything less — a red target, a missing target marker, a marker out of order, an absent final acceptance marker, or a live-acceptance flag that did not take effect — is termination state 4."*

Two of the six went red. That precondition failed on its own terms. Two further independent blockers (§7) were found that would each independently prevent a dispatch from satisfying Success Criterion 2 or 3.

---

## 2. The upstream go/no-go (answered first, and it PASSED)

The plan's blocking condition was whether 20A-02 escalated instead of shipping the AppContainer bind.

| Plan | Termination state | Consequence |
|------|-------------------|-------------|
| 20A-02 | **1 — Complete** (`20A-02-SUMMARY.md:55`) | The bind SHIPPED. Candidate is worth measuring. |
| 20A-03 | 2 — REFUTED-NO-DEFECT (`20A-03-SUMMARY.md:29`) | No production file changed. Nothing invalidated. |

So the upstream gate did **not** fire. The plan correctly proceeded to seal and measure. It is the *local dry-run* — the plan's real go/no-go — that failed.

---

## 3. The seal

| Field | Value |
|-------|-------|
| Sealed SHA | `50cf00b327891d218b910b216720b604a97c1dc5` |
| Sealed tree | `dc0a5c0c346477a080c868f07566e2fad923dd29` |
| Pinned ref | `refs/f20a/candidate` → `50cf00b327891d218b910b216720b604a97c1dc5` |
| Branch | `plan/f20-unified-audit-repair` |
| Local tip at seal time | `50cf00b3` (identical to seal) |
| Remote tip at seal time | `50cf00b3` (identical to seal) |

Pinned with `git update-ref refs/f20a/candidate "$(git rev-parse HEAD)"` per the plan's first Task 1 gate. Every measurement below reads the pinned value, never live `HEAD`.

### Per-host pristine confirmation (REQ-native-r15)

| Host | Path | HEAD | Tree | `git status --porcelain` |
|------|------|------|------|--------------------------|
| Mac (`waylandcore-ferrox`) | `/Users/seandonahoe/dev/waylandcore-ferrox` | `50cf00b3` | `dc0a5c0c` | clean across `crates/ scripts/ .github/` (only `.planning/` churn, all untracked or planning-only) |
| Hetzner (`hetzner-dsm`) | `/root/wayland` | `50cf00b327891d218b910b216720b604a97c1dc5` | `dc0a5c0c346477a080c868f07566e2fad923dd29` | **empty** |
| Windows (`SEANDESKTOP`) | `C:\ferrox-win` | `50cf00b327891d218b910b216720b604a97c1dc5` | `dc0a5c0c346477a080c868f07566e2fad923dd29` | **empty**, including `--untracked-files=all` (`STATUS_LEN=0`) |

Both remote hosts were moved from `c252d01d` with an **explicit** `git fetch origin plan/f20-unified-audit-repair` — both hosts' fetch refspecs are pinned to an unrelated branch and `git fetch --all` silently misses this one.

`cargo fmt --all -- --check` — **CLEAN** (run on the Mac; it is the only working cargo command there, and it fails on the Windows box with os error 206).

---

## 4. The exact-count gate on `$targets` (PASSED)

The plan requires proving the **absence of a seventh** entry, not merely the presence of six.

```
test "$(/usr/bin/grep -c "^    @{ id = '" scripts/f20-native-windows-proof.ps1)" = "6" && \
/usr/bin/grep -o "id = '[a-z0-9-]*'" scripts/f20-native-windows-proof.ps1 \
  | /usr/bin/sed "s/id = //;s/'//g" | tr '\n' ',' \
  | /usr/bin/grep -qxF 'windows-retained-handle,windows-appcontainer-acl,windows-job-object,windows-public-dispatch,windows-hard-process-containment,windows-f20-lifecycle,'
```

**Output:** `PASS: target array is EXACTLY the canonical six in canonical order` (exit 0).

Raw array, `scripts/f20-native-windows-proof.ps1:82-89` — six entries, canonical order, no seventh.

Canonical-map cross-check against `scripts/f20-native-uat-proof.mjs`:
- `six windows targets present in canonical map`
- `eight macos targets present`

### `$targets` byte-identity proof

```
git diff --exit-code -- scripts/ crates/ .github/   → exit 0 (CLEAN)
worktree blob: 5cc3bde7a41b3b662bc054864948d3a78e33786a
sealed   blob: 5cc3bde7a41b3b662bc054864948d3a78e33786a   (refs/f20a/candidate:scripts/f20-native-windows-proof.ps1)
```

The proof script is **blob-identical** to the sealed tree. Nothing in `crates/`, `scripts/` or `.github/` was modified by this plan.

---

## 5. Per-requirement code presence at the sealed SHA (M4)

| Req | Named fix | At sealed SHA | Evidence |
|-----|-----------|---------------|----------|
| **REQ-native-r1** | add `.write(true)` to `storage.rs` `create_new_nofollow` | **PRESENT** | `crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs:421` — `.write(true)` immediately above `.access_mode(GENERIC_READ \| GENERIC_WRITE)`, with the exact rationale in the comment at :413-420 (std's `get_creation_mode` rejects a `create_new` open lacking `.write`/`.append` before `CreateFileW` is reached, so the probe file is never created and `is_available()` returns false on every Windows host). Behaviourally corroborated: `windows-retained-handle` and `windows-appcontainer-acl` both went GREEN on real Windows — impossible if `is_available()` were false. |
| **REQ-native-r2** | drop deny-only `SidsToDisable` in `CreateRestrictedToken` | **PRESENT** | `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs:444-454` — `CreateRestrictedToken(token, DISABLE_MAX_PRIVILEGE, 0, ptr::null_mut(), 0, ptr::null(), 0, ptr::null(), &mut restricted_raw)`: `DisableSidCount=0`, `SidsToDisable=null`. Rationale recorded at :406-425 citing the 2026-07-23 hardware matrix. Behaviourally corroborated: `granted_path_is_readable_then_revoked` GREEN (a sandboxed read succeeding at all requires the deny-only marking to be off). |
| **REQ-native-r5** | `type_and_hold` asserts on granted-read success, not `choice.exe` exit index (stdin-free hold) | **PRESENT** | `crates/wcore-sandbox/tests/live_fs_acl.rs:140-146` — `type "<file>" && for /L %i in (1,1,N) do @rem`. The hold is a `cmd` **builtin** loop: no child image, no DLL, no stdin, no network, and no `choice.exe`. The assertion is on the read result: `assert_eq!(out.exit_code, 0, ...)` at :225-230 plus `assert!(stdout.contains(MARKER))` at :231-234. No exit-index assertion anywhere in the path. |
| **REQ-native-r7** | `windows-job-object` / `windows-hard-process-containment` map to REAL Windows Job-Object containment tests (must be authored) | **PRESENT** | `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` exists (43.1K), is `#![cfg(windows)]` at :32, and genuinely calls `CreateJobObjectW` (:682), `SetInformationJobObject` (:694), `AssignProcessToJobObject` (:708) and `QueryInformationJobObject` (:721). All five selected test names present. Both targets select **this** file, not the Bubblewrap/Linux-only `hard_process_containment.rs`. Behaviourally corroborated: both targets GREEN on real Windows. |

**No requirement is completed by this plan.** These are presence findings, recorded so that no requirement is later misattributed to a run that did not deliver it. Note that r1, r2, r5 and r7's fixes all **predate** this candidate.

---

## 6. THE SIX-TARGET RESULT — measured on SEANDESKTOP at the sealed SHA

### Live-acceptance flag: PROVEN effective (the trailing-space trap defeated)

The gating variable is `WAYLAND_F20_NATIVE_ACCEPTANCE` (the script itself sets `WAYLAND_SANDBOX_LIVE_WINDOWS` internally at `:61`). Set with `$env:WAYLAND_F20_NATIVE_ACCEPTANCE = '1'` and proven in **both** shell contexts before any target ran:

```
PS_FLAG=[1] LEN=1
CMD_FLAG=[1]
```

`LEN=1` is the load-bearing measurement: a trailing space would report `LEN=2`, Rust would silently ignore the value, every acceptance test would skip, and the run would be vacuously green. It did not. Corroborated downstream — every target reported real work (`1 test run: 1 passed, 11 skipped`, etc.), never `0 tests run`.

Script preconditions all satisfied: repository-root match, `wcore-sandbox` manifest present, `status --porcelain=v1 --untracked-files=all` empty, `HEAD` == expected commit, tree == expected tree.

Invocation (byte-identical selectors, nonce bound to the sealed SHA):
```
scripts\f20-native-windows-proof.ps1
  -ExpectedCommit 50cf00b327891d218b910b216720b604a97c1dc5
  -ExpectedTree   dc0a5c0c346477a080c868f07566e2fad923dd29
  -Nonce          ee5abe5c631da42945ba002da1e771c4b7ee009ffda84ce35868e33f80a6f715
```
Nonce = `sha256("50cf00b327891d218b910b216720b604a97c1dc5")` — deterministic, candidate-bound, idempotent.

### Per-target result

| # | Target | Result | Detail |
|---|--------|--------|--------|
| 1 | `windows-retained-handle` | **GREEN** | `1 test run: 1 passed, 11 skipped` in 1.977s. Marker emitted bound to commit+tree+nonce. |
| 2 | `windows-appcontainer-acl` | **GREEN** | `1 test run: 1 passed, 11 skipped` in 1.992s. Marker emitted. |
| 3 | `windows-job-object` | **GREEN** | `4 tests run: 4 passed, 2 skipped` in 10.406s — `active_process_cap_is_enforced`, `breakaway_is_denied`, `contained_detached_child_exit`, `job_close_reaps_detached_descendant_with_no_residue` all pass. Marker emitted. |
| 4 | `windows-public-dispatch` | **RED** | `6/10 tests run: 5 passed, 1 failed`. Script threw; **no marker**. |
| 5 | `windows-hard-process-containment` | **GREEN** | `1 test run: 1 passed, 5 skipped` in 4.337s (`qualified_hard_containment_backend_preflight`). Measured individually — see §6.1. |
| 6 | `windows-f20-lifecycle` | **RED** | `3/9 tests run: 2 passed, 1 failed`. Measured individually — see §6.1. |

**Score: 4 GREEN / 2 RED.** The final `F20_NATIVE_WINDOWS_ACCEPTANCE=PASS` marker was **never emitted** — correct behaviour, the script fails closed.

### 6.1 Why targets 5 and 6 were measured separately

`scripts/f20-native-windows-proof.ps1:170-172` throws on the first non-zero target exit, so the end-to-end run **stopped at target 4** and left targets 5 and 6 unmeasured. Leaving a third of the six unknown would not have answered the question this run exists to answer, so both were then run with the script's **byte-identical** selectors and the same environment the script establishes (`--run-ignored all --no-tests=fail <args> --nocapture`, with `WAYLAND_F20_NATIVE_ACCEPTANCE=1` and `WAYLAND_SANDBOX_LIVE_WINDOWS=1`). The script was not modified; the seal is intact (blob-hash proof in §4).

### 6.2 Target 4 — `windows-public-dispatch` — exact failure text

Failing test: `wcore-swarm::dispatch_smoke public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state`. Failed on both TRY 1 and TRY 2.

```
thread 'public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state' (33388)
panicked at crates\wcore-swarm\tests\dispatch_smoke.rs:94:5:
assertion `left == right` failed: WorkerHandle {
  worker_id: "4457e7ffc9824380be81e54f9d17ed5a-0",
  branch: "swarm/authority/4457e7ffc9824380be81e54f9d17ed5a-0",
  status: Failed("exit 101"),
  stdout: "\nrunning 1 test\ntest standalone_authority_fixture ... FAILED\n\nfailures:\n\nfailures:\n    standalone_authority_fixture\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.00s\n\n",
  stderr: "\nthread 'standalone_authority_fixture' (37104) panicked at crates\\wcore-swarm\\tests\\dispatch_smoke.rs:505:68:\ncalled `Result::unwrap()` on an `Err` value: Os { code: 5, kind: PermissionDenied, message: \"Access is denied.\" }\n...",
  duration: 1.2912345s }
  left: Failed("exit 101")
 right: Succeeded

native Windows target windows-public-dispatch failed with exit code 100
At C:\ferrox-win\scripts\f20-native-windows-proof.ps1:171 char:9
```

**Classification: KNOWN CLASS 3** — "a child panics at `dispatch_smoke.rs:505:68` on `canonicalize()` with `code: 5` (Low-IL restricted token; the name-lease pin was already exonerated by measurement)." Exact file, exact line, exact error code. **Not NEW.**

Note: the script does **not** pass `--no-fail-fast` to the per-target runs, so nextest cancelled after this failure and `4/10 tests were not run`. Known class 4 (the ~35 s test exceeding a 20 s wall-clock budget) therefore did not get a chance to surface in this run. That timeout was **not** raised.

### 6.3 Target 6 — `windows-f20-lifecycle` — exact failure text

Failing test: `wcore-agent::transactional_delegated_mutation_test happy_path_open_accept_land_receipt_then_rollback`. Failed on both TRY 1 and TRY 2.

```
thread 'happy_path_open_accept_land_receipt_then_rollback' (12668)
panicked at crates\wcore-agent\tests\transactional_delegated_mutation_test.rs:349:10:
authorized landing: Primitive("worktree io: parent landing: parent integration checkout is dirty: M README.md")
```

**Classification: KNOWN CLASS 1** — the test-harness EOL failure (`F-EOL-1`): `clone_integration` mints via an unscrubbed clone, so on Windows the freshly-minted parent integration checkout reports `M README.md` from LF→CRLF normalisation and the landing refuses a dirty tree. Directly corroborated by git's own warnings printed throughout the same log: `warning: in the working copy of 'README.md', LF will be replaced by CRLF the next time Git touches it`. 20A-03 closed this REFUTED-NO-DEFECT — it is a recorded finding, not a defect and not this plan's work. **Not NEW.**

The other 6 of 9 lifecycle tests did not run (fail-fast). The `\\?\` extended-length-path failure behind class 1 did not surface in this run because the EOL failure fires first.

### 6.4 Summary of classification

| Red target | Class | New? |
|------------|-------|------|
| `windows-public-dispatch` | Known class 3 — `dispatch_smoke.rs:505:68`, `code: 5` PermissionDenied under the Low-IL restricted token | No |
| `windows-f20-lifecycle` | Known class 1 — `F-EOL-1` test-harness EOL, `parent integration checkout is dirty: M README.md` | No |

**No NEW failure class was found.** Nothing was fixed, weakened, ignored, re-gated, re-timed or deleted.

### 6.5 A correction to the plan's own premise (record this)

The plan's `must_haves` and threat `T-20A-04-12` assert that *"`windows-f20-lifecycle` selects `-p wcore-agent --test transactional_delegated_mutation_test` with **NO ignored-set flag**, while every baseline and delta measurement runs that same test WITH the ignored set included — the measured set and the proven set are literally different sets."*

**That is false at the sealed SHA.** `scripts/f20-native-windows-proof.ps1:168` builds every target's command as:

```powershell
$nextestArgs = @('nextest', 'run', '--run-ignored', 'all', '--no-tests=fail') + $target.args + @('--nocapture')
```

`--run-ignored all` is applied **globally to all six targets**. The target's own `args` carry no ignored-set flag, but the effective selection includes the ignored set — the same set the upstream measurements use. The measured set and the proven set are the **same** set.

This does not weaken the plan's conclusion; it strengthens the case for having run the dry-run, because it means the known 4 lifecycle failures were always going to appear here. It is recorded so a successor plan does not act on the incorrect premise.

---

## 7. Three independent blockers on the dispatch

Any one of these alone is sufficient for state 4.

### B1 — The go/no-go precondition failed (the plan's own gate)

2 of 6 targets red. The plan: *"ALL SIX TARGETS GREEN LOCALLY IS THE GO/NO-GO PRECONDITION."* Both reds are previously classified, so no new work is implied — but the candidate is **not worth a Sean authorization** in this state, because a dispatched run would return red on exactly these two targets and spend the authorization for nothing.

### B2 — HIGH: the macOS runner label is UNRESOLVABLE, and no macOS runner exists

`f20-macos-candidate` (`.github/workflows/nightly-windows-soak.yml:348-353`) requires the label set:
```
self-hosted, f20-native-macos, f20-ephemeral, f20-no-ambient-secrets, ${{ inputs.f20_macos_runner_label }}
```

Measured registered runners on `FerroxLabs/wayland-core`:
```json
{"name":"ferrox-win-msvc","status":"online","busy":true, "labels":["self-hosted","Windows","X64","msvc"]}
{"name":"SEANDESKTOP",    "status":"online","busy":false,"labels":["self-hosted","Windows","X64","msvc"]}
```
(`GET /orgs/FerroxLabs/actions/runners` → 404; there is no org-level runner pool.)

**No runner carries `f20-native-macos`, `f20-ephemeral`, `f20-no-ambient-secrets`, or any `f20-image-<sha256>` label. No macOS runner is registered at all.** The label cannot be resolved to an actual value. Per the plan's Task 3: *"an unresolved label is a reason to stop, not a blank to leave empty and hope."* Success Criterion 2 (the macOS leg) **cannot be met by any dispatch fired today**, regardless of the Windows result. Standing up that ephemeral runner is Sean-only infrastructure.

### B3 — HIGH: the dispatch ref cannot bind to the sealed SHA

The candidate jobs bind evidence via `EXPECTED_COMMIT: ${{ github.sha }}` (`:331`, `:371`), and the proof script asserts `HEAD == ExpectedCommit`. That assertion is **self-consistent by construction** — it compares the checkout against whatever `github.sha` resolved to, so it can never detect that the wrong candidate was run. The only thing that actually binds the run to `50cf00b3` is the `--ref` passed to `workflow_dispatch`.

GitHub's `workflow_dispatch` `ref` accepts a **branch or tag name**, not an arbitrary commit SHA. At seal time `plan/f20-unified-audit-repair` pointed at `50cf00b3` on both the local and the remote (verified: remote `.object.sha` = `50cf00b327891d218b910b216720b604a97c1dc5`). **The moment any commit lands on that branch, `github.sha` stops being the sealed SHA, and a dispatch would silently prove a different candidate while every in-run assertion still passed.**

**This is now measured, not predicted.** Pushing this SUMMARY advanced the branch:

```
50cf00b3..ade88c9a  plan/f20-unified-audit-repair -> plan/f20-unified-audit-repair
remote .object.sha  = ade88c9aa4b8151d868b329bed515f16f228bf17
refs/f20a/candidate = 50cf00b327891d218b910b216720b604a97c1dc5
```

A dispatch with `--ref plan/f20-unified-audit-repair` fired right now would run against `ade88c9a`, **not** the sealed candidate — and would report a clean self-consistent PASS while proving the wrong tree. `refs/f20a/candidate` still holds the seal locally; nothing on the remote does.

This is threat `T-20A-04-13` (seal drift) realised at the dispatch boundary rather than at the local gate. `refs/f20a/candidate` protects the local record; it does **not** protect the dispatch. Binding a dispatch to `50cf00b3` requires a tag or a frozen branch pointing exactly at it — and creating a tag is itself a Sean gate.

### Also open: REQ-native-r9 is NOT ANSWERED

20A-01 left the macOS harness re-validation unresolved: CI run `30151510189`'s macOS leg failed at `Clippy (warnings = errors)` on two `-D warnings` lints in `crates/wcore-sandbox/src/backends/process_tree.rs`, aborting before clippy reached the test targets where all 23 macOS-only tests live. There is no genuine macOS compile error — only two lints — but the verdict is unobtained. Combined with B2, the macOS leg is blocked twice over.

### Recorded, LOW: a stale workflow comment contradicts REQ-native-r11

`.github/workflows/nightly-windows-soak.yml:25` states candidate mode runs *"hosted windows-2022 plus the externally pinned ephemeral macOS runner."* The actual `f20-windows-candidate` job (`:299-303`) is `runs-on: [self-hosted, Windows, X64, msvc]`, with the correct rationale at `:290-298`. **REQ-native-r11 is satisfied in code**; only the header comment is stale. Documentation defect, non-blocking, not fixed here (this plan modifies no `.github/` file).

---

## 8. Requirement completion status

**ZERO requirements completed.** Evidence was accepted from no dispatched run, because no run was dispatched.

| Req | Status | Disposition |
|-----|--------|-------------|
| REQ-native-r1 | **Incomplete** | Fix PRESENT at sealed SHA (§5) and behaviourally corroborated by targets 1-2 GREEN, but not proven by an authorized native run. Presence is not completion. |
| REQ-native-r2 | **Incomplete** | Fix PRESENT at sealed SHA (§5), corroborated by target 2 GREEN. Not proven by an authorized run. |
| REQ-native-r3 | **Incomplete** | Not evidenced by this plan. |
| REQ-native-r4 | **Incomplete** | Per-host pristine + seal evidence recorded (§3). Aggregate Linux proof not re-derived — see §10 deviation D3. |
| REQ-native-r5 | **Incomplete** | Fix PRESENT at sealed SHA (§5). Not proven by an authorized run. |
| REQ-native-r6 | **Incomplete** | Not evidenced by this plan. |
| REQ-native-r7 | **Incomplete** | Fix PRESENT at sealed SHA (§5), corroborated by targets 3 and 5 GREEN. Not proven by an authorized run. |
| REQ-native-r8 | **Incomplete** | Wrong-OS anti-drift guard verified present and unmodified (`:91-162`), blob-identical to seal. Verified, not proven. |
| REQ-native-r9 | **Incomplete — NOT ANSWERED** | macOS harness re-validation unobtained (§7). Blocked by the clippy `-D warnings` abort AND by the absence of any registered macOS runner. |
| REQ-native-r10 | **Incomplete** | Not evidenced by this plan. |
| REQ-native-r11 | **Incomplete** | Candidate Windows job confirmed `self-hosted / Windows / X64 / msvc` and AppContainer-capable in code (§7), matching the online idle `SEANDESKTOP` runner. Confirmed, not proven by a run. |
| REQ-native-r12 | **Incomplete** | No authorized run exists to bind evidence to. |
| REQ-native-r13 | **Incomplete** | No review round was run — the plan terminated at Task 1's go/no-go before Task 2. No review artifact is claimed, and none exists. Claiming a PASS here is precisely what r13 forbids. |
| REQ-native-r14 | **Incomplete** | Not evidenced by this plan. |
| REQ-native-r15 | **Incomplete** | Per-host pristine confirmed on all three hosts (§3). Confirmed, not proven by a run. |

`.planning/REQUIREMENTS.md` was **not modified** — there is nothing to check off.

---

## 9. The fully-formed but UNFIRED dispatch command

Recorded here **for Sean's reading only**. This is **not** a prepared tuple, this candidate is **not** dispatchable in its current state (§7), and this command **must not be run as written** until B1, B2 and B3 are each closed.

```bash
# NOT AUTHORIZED. NOT PREPARED. DO NOT RUN AGAINST THIS CANDIDATE.
gh auth switch --user FerroxLabs

gh workflow run nightly-windows-soak.yml \
  -R FerroxLabs/wayland-core \
  --ref <FROZEN-REF-POINTING-AT-50cf00b3>   \
  -f f20_candidate=true \
  -f f20_request_nonce=ee5abe5c631da42945ba002da1e771c4b7ee009ffda84ce35868e33f80a6f715 \
  -f f20_macos_runner_label=<UNRESOLVABLE — no macOS runner registered>
```

Two inputs are **unresolved and unresolvable today**:

- `--ref` — cannot be `50cf00b327891d218b910b216720b604a97c1dc5`; `workflow_dispatch` takes a branch or tag. `plan/f20-unified-audit-repair` **no longer points at the seal** — it is at `ade88c9a` since this SUMMARY was pushed (B3, measured). No remote ref currently resolves to the sealed candidate.
- `f20_macos_runner_label` — no `f20-image-<sha256>` runner exists (B2).

The nonce `ee5abe5c631da42945ba002da1e771c4b7ee009ffda84ce35868e33f80a6f715` is `sha256` of the sealed SHA — deterministic and candidate-bound, so it cannot be reused for a different candidate.

**Every prior authorization digest is spent and void.** No authorization was spent by this run. `gh workflow run` was never invoked; the most recent `nightly-windows-soak.yml` run remains `30149496548` (scheduled, `2026-07-25T07:32:44Z`, headSha `61b79c4f`), which predates this plan and is unrelated to it.

---

## 10. Deviations

| # | Deviation | Reason |
|---|-----------|--------|
| D1 | **Did not create `20A-04-CANDIDATE.md`.** | The plan's termination state 4 says: *"STOP before sealing (or, if the dry-run failed after sealing, before preparing the tuple)."* The dry-run failed after sealing, so the correct stop point is before preparing the tuple — and `20A-04-CANDIDATE.md` **is** the tuple artifact. Creating it would perform the act state 4 forbids and would risk reading as a go. The seal is instead pinned to `refs/f20a/candidate` (durable) and recorded here. |
| D2 | **Did not run Task 2 (reviews) or Task 3 (tuple preparation).** | Task 1's go/no-go failed. The plan terminates at that point and does not proceed. Running reviews against a candidate that cannot be dispatched would produce artifacts bound to a dead candidate. |
| D3 | **Did not re-run the aggregate Hetzner `cargo build --locked --workspace --all-features` + `cargo nextest run --profile ci --no-fail-fast`.** | Instructed as measured-and-not-to-be-re-derived: Linux non-regression at `50cf00b3` is **11519/11519 passed, 0 failed**, workspace clippy clean. Hetzner was still moved to the sealed SHA and confirmed pristine (§3) so the host is staged for a successor. Since the plan terminates before dispatch, a fresh aggregate count would not change the outcome. |
| D4 | **Ran targets 5 and 6 individually after the script threw at target 4.** | The script fails closed on the first red, which would have left `windows-hard-process-containment` and `windows-f20-lifecycle` unmeasured — a third of the deliverable. Selectors were byte-identical to the script's, the environment identical, and the script itself untouched (blob-hash proof, §4). |
| D5 | **Two plan `<verify>` blocks in Task 1 are not runnable as written.** | The Task 1 dry-run gate sets `$env:WAYLAND_SANDBOX_LIVE_WINDOWS='1'` and invokes `f20-native-windows-proof.ps1` **with no parameters**. The script requires `WAYLAND_F20_NATIVE_ACCEPTANCE=1` (it sets `WAYLAND_SANDBOX_LIVE_WINDOWS` itself at `:61`) and takes three **mandatory** parameters (`-ExpectedCommit`, `-ExpectedTree`, `-Nonce`). As written the gate would throw on the flag check, or block on a mandatory-parameter prompt. Corrected in execution; recorded so a successor plan fixes the gate text. Also, the Windows-host gate uses `HEAD^{tree}` inside `cmd /c`, where `^` is the escape character — it fails with `ambiguous argument 'HEAD{tree}'`. Use `git rev-parse HEAD:`. |
| D6 | **Two SSH transport attempts were needed to get the proof to run.** | Attempt 1 (`powershell -Command -` over stdin with backtick line-continuations) executed the preamble and then silently produced nothing. Attempt 2 (single-line invocation, transcript redirected to `C:\Users\SeanD\f20a-proof.log` — deliberately **outside** the repo, since the script rejects any non-empty `--untracked-files=all` status) ran clean. This consumed the 2-attempt budget for environment/setup, and the proof **did** run. |

**Nothing outside `.planning/` was modified.** No test weakened, no `#[ignore]`, no `#[allow]`, no timeout raised, no selector narrowed, no test deleted. No `gh workflow run`. No push to main, no merge, no PR, no tag, no release, no issue closure.

---

## 11. What a successor plan must close, in order

1. **B2 (Sean-only infra):** stand up and register the ephemeral macOS runner with labels `self-hosted`, `f20-native-macos`, `f20-ephemeral`, `f20-no-ambient-secrets`, `f20-image-<sha256>`. Until this exists, Success Criterion 2 is unreachable and no dispatch can close Phase 20A.
2. **B3 (Sean gate):** decide the frozen-ref mechanism that binds `github.sha` to the sealed candidate — a tag at the sealed SHA is the obvious answer and tag creation is a Sean gate. Separately, consider whether `EXPECTED_COMMIT: ${{ github.sha }}` should instead be a dispatch input compared against `github.sha`, so the binding is falsifiable rather than tautological.
3. **REQ-native-r9:** two lint fixes in `crates/wcore-sandbox/src/backends/process_tree.rs` unlock the macOS compile verdict. 20A-01 records the M5 round-trip budget as untouched at zero used.
4. **B1, red 1 (`windows-f20-lifecycle`):** the `F-EOL-1` harness defect — `clone_integration` mints via an unscrubbed clone, so the parent integration checkout is born dirty on Windows. Owned by whichever plan owns that harness; 20A-03 closed the *premise* REFUTED, not the harness.
5. **B1, red 2 (`windows-public-dispatch`):** `dispatch_smoke.rs:505:68` `canonicalize()` → `code: 5` under the Low-IL restricted token. The name-lease pin is already exonerated by measurement, so the cause lies elsewhere.
6. **Only then** re-seal a fresh candidate under a fresh plan and a **fresh** authorization. Every prior digest, including any implied by this document, is spent and void.

Also open, non-blocking: the 30 Windows failures F-09 unmasked (18 in `session_journal`) remain un-baselined and are explicitly outside this plan; and the ~35 s test against a 20 s budget (known class 4) did not surface here only because nextest cancelled early — it is unresolved, and its timeout was not raised.

---

## Self-Check

**Files claimed created:**
- `.planning/phases/20A-native-windows-macos-uat/20A-04-SUMMARY.md` — this file.

**Claims verified by direct measurement in this run:**
- Sealed SHA / tree / pinned ref — `git rev-parse` output quoted verbatim.
- Per-host pristine — `git status --porcelain` output quoted verbatim from all three hosts.
- Exact-count gate — command and its `PASS` output quoted verbatim.
- `$targets` byte-identity — `git diff --exit-code` exit 0 plus matching blob hashes quoted verbatim.
- Six-target result — nextest summaries and panic text quoted verbatim from the SEANDESKTOP transcript.
- Live-flag effectiveness — `PS_FLAG=[1] LEN=1` / `CMD_FLAG=[1]` quoted verbatim.
- Runner inventory — `gh api .../actions/runners` JSON quoted verbatim.
- r1/r2/r5/r7 presence — file paths and line numbers cited, each read directly.

**Commits:** none made by this run at the time of writing.

## Self-Check: PASSED
