---
phase: 20A-native-windows-macos-uat
plan: "04"
subsystem: infra
tags: [windows, macos, native-uat, appcontainer, candidate-seal, dispatch-gate, sean-gate]
status: complete
termination_state: 1
termination_state_history: "4 (blocked upstream, 2026-07-25 at seal 50cf00b3) -> 1 (complete, 2026-07-26 at seal 9821ef76 after Sean-authorized dispatch 30184651330). Sections 1-11 record state 4 verbatim as history; section 13 is authoritative."

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
  duration: "~50 min (initial) + closeout"
  completed: 2026-07-26
  targets_green: 14
  targets_red: 0
  requirements_completed: 11
  requirements_open: 4
  sealed_sha: 9821ef7603ac1e687b600cda591af1657c883484
  sealed_tree: 0a1267a990f3b512782916b6ed26501d0db39222
  dispatched_run: 30184651330
---

# Phase 20A Plan 04: Native Proof Candidate Seal Summary

**FINAL (2026-07-26):** dispatch `30184651330` fired on Sean's authorization against sealed SHA `9821ef76…` and returned `completed/success`. `F20_NATIVE_WINDOWS_ACCEPTANCE=PASS` (6/6 targets) and `F20_NATIVE_MACOS_ACCEPTANCE=PASS` (8/8 targets), same commit/tree/nonce. All three Phase 20A Success Criteria met; **Phase 20A is COMPLETE.** 11 of 15 `REQ-native-*` requirements complete, 4 left explicitly open. **See §13 — it supersedes §1, §8, §9 and §12.4.**

*Everything below until §12 was written at the earlier seal `50cf00b3` and is retained as history.* Six-target native Windows proof measured end to end on SEANDESKTOP at that seal: **4 GREEN, 2 RED**. Both reds classified to previously-recorded classes; neither was NEW. The plan terminated at that point in **state 4 (blocked upstream)** — no dispatch fired, no requirement completed, no authorization spent.

---

## 1. Termination state — SUPERSEDED BY §13 (kept as history)

> **Superseded 2026-07-26.** This was the state at seal `50cf00b3`. The dispatch
> was later authorized and fired at seal `9821ef76` (run `30184651330`, green on
> both platforms). Effective terminal state is **1 — complete**. Read §13.

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

> **CLOSED 2026-07-26 (20A-05).** Runner id 27 `f20-macos-ephemeral-1d053640` is registered, online and idle, carrying `f20-native-macos` + `f20-ephemeral` + `f20-no-ambient-secrets` + `f20-image-1d053640…`. See §9.

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

> **CLOSED 2026-07-26 (20A-05).** Annotated tag `f20a-candidate-50cf00b3` pins the seal immutably, and `EXPECTED_COMMIT` is no longer `${{ github.sha }}` — it is the explicit `f20_expected_sha` dispatch input, asserted against the real checkout. See §9.

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

> **MEASURED 2026-07-26 (20A-05), natively on the registered macOS runner host.**
> The source tree at the seal is byte-identical to the branch tip outside
> `.planning/` (`git diff --quiet 50cf00b3 HEAD -- crates scripts .github` → 0),
> so this measurement is valid for `50cf00b3`.
>
> 1. **The recorded blocker is STALE.** `cargo clippy --workspace --all-targets -- -D warnings`
>    (the exact `just lint` body) exits **101**, but **not** in `process_tree.rs` —
>    it aborts on a single `-D dead-code` promotion at
>    `crates/wcore-swarm/src/worktree_cleanup.rs:349` (`new_with_git_script_and_limits`
>    is never used on this target). `process_tree.rs` produces **zero** diagnostics.
>    Because clippy stops at the first failing crate, additional downstream lints
>    may remain unobserved.
> 2. **The clippy abort does not gate the proof.** `scripts/f20-native-macos-proof.sh`
>    never invokes clippy; it runs `cargo nextest run`. `-D warnings` is a lint
>    gate, not a compile failure.
> 3. **The macOS workspace DOES build.** All three compile groups backing the eight
>    macOS targets succeed: `-p wcore-sandbox --features live-docker` (exit 0),
>    `-p wcore-swarm --features wcore-sandbox/live-docker` (exit 0),
>    `-p wcore-agent --test transactional_delegated_mutation_test` (exit 0).
> 4. **All eight selectors resolve.** `cargo nextest list --run-ignored all --message-format json`
>    reports exactly one `filter-match: matches` test for targets 1–7 and the
>    intended 9-test binary for target 8, so `--no-tests=fail` cannot fire, and
>    the anti-drift OS gate resolves `macos-retained-directory` →
>    `live_integrity_macos` and `macos-process-tree` → `hard_process_containment_macos`
>    as designed.
>
> **REQ-native-r9's compile/harness half is therefore ANSWERED: green.** What
> remains unmeasured is runtime pass/fail of the live sandbox-exec, process-tree,
> and Docker targets — only an actual run settles that.

### Recorded, LOW: a stale workflow comment contradicts REQ-native-r11

`.github/workflows/nightly-windows-soak.yml:25` states candidate mode runs *"hosted windows-2022 plus the externally pinned ephemeral macOS runner."* The actual `f20-windows-candidate` job (`:299-303`) is `runs-on: [self-hosted, Windows, X64, msvc]`, with the correct rationale at `:290-298`. **REQ-native-r11 is satisfied in code**; only the header comment is stale. Documentation defect, non-blocking, not fixed here (this plan modifies no `.github/` file).

---

## 8. Requirement completion status — SUPERSEDED BY §13.6 (kept as history)

> **Superseded 2026-07-26.** These were the dispositions when no run had been
> dispatched. Run `30184651330` now completes 11 of the 15 (r1, r3, r4, r5, r6,
> r7, r9, r10, r11, r14, r15) and leaves 4 explicitly open (r2, r8, r12, r13).
> The authoritative table is §13.6, and the per-requirement evidence lives in
> `.planning/REQUIREMENTS.md`.

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

## 9. The fully-formed but UNFIRED dispatch command — SUPERSEDED BY §13.1 (kept as history)

> **Superseded 2026-07-26.** This §9 command targeted the now-stale seal
> `50cf00b3` and needed a two-ref split. §12.4 replaced it with a one-tag form at
> `9821ef76`, and **§13.1 records that command as actually FIRED** — run
> `30184651330`, green. Nothing in §9 is live.

Recorded here **for Sean's reading only**. This is **not** a prepared tuple and this command **must not be run** until **B1** is closed.

> **UPDATED 2026-07-26 (20A-05 infrastructure repair).** Every input below is now
> **fully resolvable** — B2 and B3 are closed. **B1 is NOT closed**: 2 of the 6
> Windows targets were red at the local go/no-go (§7), so a dispatch fired today
> would still burn an authorization to re-observe two already-classified reds.
> The command is recorded in its resolvable form so the remaining gate is
> unambiguously B1, and nothing else.

```bash
# NOT AUTHORIZED. NOT PREPARED. DO NOT RUN — B1 (2/6 Windows targets red) is still open.
gh auth switch --user FerroxLabs

gh workflow run nightly-windows-soak.yml \
  -R FerroxLabs/wayland-core \
  --ref f20a-harness-39d30e55 \
  -f f20_candidate=true \
  -f f20_expected_sha=50cf00b327891d218b910b216720b604a97c1dc5 \
  -f f20_request_nonce=ee5abe5c631da42945ba002da1e771c4b7ee009ffda84ce35868e33f80a6f715 \
  -f f20_macos_runner_label=f20-image-1d05364078523334605249687228ffec79964b7ecf731d7c9512b40e67fd1a64
```

### The two refs, and why `--ref` is NOT the candidate tag

A tag alone does **not** close B3, and this is the single most important
correction in this update.

**GitHub always reads the workflow DEFINITION from the ref passed to
`workflow_dispatch`.** Measured: the workflow as sealed at `50cf00b3` declares
only `f20_candidate`, `f20_macos_runner_label`, `f20_request_nonce` —

```
git show f20a-candidate-50cf00b3:.github/workflows/nightly-windows-soak.yml | grep 'f20_.*:'
  28:      f20_candidate:
  33:      f20_macos_runner_label:
  38:      f20_request_nonce:
```

So `--ref f20a-candidate-50cf00b3` would run the **tautological** pre-fix
workflow and would reject `-f f20_expected_sha=…` outright as an unexpected
input. Dispatching from a ref that *does* carry the fix would instead check out
that ref's tip rather than the candidate. Both halves are needed, and they must
come from **two different refs**:

| Role | Ref | Resolves to | Supplies |
|------|-----|-------------|----------|
| Dispatch harness | `f20a-harness-39d30e55` | `39d30e559276126cb7a8f92ed743ed93bad0679d` | the **workflow definition** (fixed, non-tautological) |
| Candidate seal | `f20a-candidate-50cf00b3` | `50cf00b327891d218b910b216720b604a97c1dc5` | the **tree that is actually proven**, via `f20_expected_sha` |

`crates/` and `scripts/` at `39d30e55` are **byte-identical** to the sealed
candidate (`git diff --quiet 50cf00b3 39d30e55 -- crates scripts` → 0); only
`.github/` and `.planning/` differ. So the harness ref changes the workflow
plumbing and nothing that is under proof.

Both are annotated tags, pushed as tags only, verified on the remote:

```
refs/tags/f20a-candidate-50cf00b3^{}  -> 50cf00b327891d218b910b216720b604a97c1dc5   (tree dc0a5c0c…)
refs/tags/f20a-harness-39d30e55^{}    -> 39d30e559276126cb7a8f92ed743ed93bad0679d
```

The candidate tag remains load-bearing even though it is not the `--ref`: it is
the only remote ref keeping `50cf00b3` reachable, which is what makes
`actions/checkout` able to fetch that raw SHA at all. Previously the seal lived
only in a local `refs/f20a/candidate`. Demonstrated live: pushing two further
commits advanced the branch to `39d30e55` while
`gh api …/commits/f20a-candidate-50cf00b3` still returned `50cf00b3…` — the
exact drift B3 described, now inert.

**`f20_expected_sha` (new input — closes the tautology B3 also exposed).**
Both candidate jobs previously set `EXPECTED_COMMIT: ${{ github.sha }}` and then
asserted `HEAD == EXPECTED_COMMIT`. That compared the checkout against whatever
the dispatch ref resolved to, so it could **never** detect that the wrong
candidate had been proven. Both jobs now take the authorized SHA as an explicit
dispatch input, assert it against the real checkout in a dedicated
`Assert checkout is the authorized candidate` step that runs **before** any
toolchain or proof work, and pass it through as `EXPECTED_COMMIT`. A malformed,
empty, or mismatched value fails the job closed. The existing nonce mechanism is
untouched.

Both candidate jobs additionally pin `actions/checkout` to
`ref: ${{ github.event.inputs.f20_expected_sha }}`, so the dispatch ref supplies
only the workflow and never the proven tree. The scheduled `windows-soak` and
`windows-live-acceptance` jobs keep their default checkouts.

The bash assertion was executed verbatim (extracted from the shipped YAML) with
four inputs — it exits **1** on a wrong SHA (the real drift case: authorized
`50cf00b3…` vs checkout `d398fa9a…`), **1** on empty, **1** on uppercase hex, and
**0** only on an exact match. The PowerShell mirror could not be executed (no
`pwsh` on the macOS host); its `throw`-fails-the-job mechanism is the same one
the pre-existing adjacent step at `:398` already relies on.

**`f20_macos_runner_label` (was B2 — no macOS runner existed).** This Mac
(`Darwin arm64`, macOS 26.3 build 25D125) is registered as runner **id 27**,
`f20-macos-ephemeral-1d053640`, `status: online`, `busy: false`, `ephemeral: true`
(per `.runner`), installed as a LaunchAgent service. Labels:

```
self-hosted, macOS, ARM64,
f20-native-macos, f20-ephemeral, f20-no-ambient-secrets,
f20-image-1d05364078523334605249687228ffec79964b7ecf731d7c9512b40e67fd1a64
```

It is the **only** runner carrying that label set, satisfying the 20-17 preflight
rule that admits exactly one qualifying runner. The image label is
`sha256` over a deterministic host-identity manifest
(`os / build / arch / runner / rustc / cargo / nextest / docker / git`), recorded
in the 20A-05 report.

The nonce `ee5abe5c631da42945ba002da1e771c4b7ee009ffda84ce35868e33f80a6f715` is
`sha256` of the sealed SHA (verified: `printf '%s' 50cf00b3… | shasum -a 256`) —
deterministic and candidate-bound, so it cannot be reused for a different
candidate.

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

---

# 12. FINAL CONSOLIDATION — 2026-07-26 (supersedes §6, §7/B1, §9)

Everything in §1–§11 above was measured at the **now-stale** seal `50cf00b3`.
This section is the authoritative record. Where it conflicts with anything
above, **this section wins**.

## 12.1 The final tip

| Field | Value |
|-------|-------|
| Branch | `plan/f20-unified-audit-repair` |
| **Final SHA** | `9821ef7603ac1e687b600cda591af1657c883484` |
| **Final tree** | `0a1267a990f3b512782916b6ed26501d0db39222` |
| **Nonce** | `96c91107636c4eaca9130969369b2309ee6dd6582cc4e9e1a7a45e0fb8ec92cf` |
| Nonce derivation | `printf '%s' 9821ef7603ac1e687b600cda591af1657c883484 \| shasum -a 256` |
| Pinned ref | `refs/f20a/candidate` → `9821ef76…` (was `50cf00b3…`) |
| **Seal tag** | `f20a-candidate-9821ef76` → `48601e469b3a3fca524811cc37e0a6ce6841e457` → `9821ef76…` |

Tip confirmed identical on all four places, tree clean everywhere:

| Where | Path | HEAD | `git status --porcelain` |
|-------|------|------|--------------------------|
| Mac | `/Users/seandonahoe/dev/waylandcore-ferrox` | `9821ef76…` | only untracked `.planning/` churn (`TEST-AUDIT.md`, `config.json.pre-loopfix.bak`, `debug/`) |
| `gh` remote | `FerroxLabs/wayland-core` | `9821ef76…` | — (fetched via `gh`, never `origin`) |
| Hetzner | `hetzner-dsm:/root/wayland` | `9821ef76…` | **empty** |
| Windows | `SeanD@seandesktop:C:\ferrox-win` | `9821ef76…` | **empty**, incl. `--untracked-files=all` |

## 12.2 Six-target proof RE-RUN at the final tip — 6/6 PASS

Re-run was mandatory: the prior 6/6 was at `f4803e1e`/`f09a53c`, and five
`session_journal` commits landed afterwards touching `crates/wcore-agent`,
which `windows-f20-lifecycle` exercises.

Run on SEANDESKTOP on a **verified-idle box** (0 `cargo`/`rustc`/`link`
processes, CPU 3%). The in-progress CI run `30182036233` was holding the
self-hosted Windows runner with `Build (x86_64-pc-windows-msvc)` and
`Build (aarch64-pc-windows-msvc)`; the proof was **not** started until both
jobs completed and the runner went idle. No workflow was dispatched.

```
WAYLAND_F20_NATIVE_ACCEPTANCE=1
scripts/f20-native-windows-proof.ps1
  -ExpectedCommit 9821ef7603ac1e687b600cda591af1657c883484
  -ExpectedTree   0a1267a990f3b512782916b6ed26501d0db39222
  -Nonce          96c91107636c4eaca9130969369b2309ee6dd6582cc4e9e1a7a45e0fb8ec92cf
```

| # | Target | Result | Detail |
|---|--------|--------|--------|
| 1 | `windows-retained-handle` | **PASS** | 1 test run: 1 passed, 11 skipped |
| 2 | `windows-appcontainer-acl` | **PASS** | 1 test run: 1 passed, 11 skipped |
| 3 | `windows-job-object` | **PASS** | 4 tests run: 4 passed, 2 skipped |
| 4 | `windows-public-dispatch` | **PASS** | 10 tests run: **10 passed**, 0 skipped |
| 5 | `windows-hard-process-containment` | **PASS** | 1 test run: 1 passed, 5 skipped |
| 6 | `windows-f20-lifecycle` | **PASS** | 9 tests run: 9 passed, 0 skipped |

Final marker emitted exactly once, after all six:

```
F20_NATIVE_WINDOWS_ACCEPTANCE=PASS commit=9821ef7603ac1e687b600cda591af1657c883484 tree=0a1267a990f3b512782916b6ed26501d0db39222 nonce=96c91107636c4eaca9130969369b2309ee6dd6582cc4e9e1a7a45e0fb8ec92cf
PROOF_EXIT=0
```

Every one of the six `F20_NATIVE_TARGET=PASS` lines carries
`commit=9821ef76… tree=0a1267a9… nonce=96c91107…`, so the log is bound to this
exact candidate and this exact request.

**§7/B1 is CLOSED.** Targets 4 and 6, red at `50cf00b3`, are green at `9821ef76`.

### `$targets` byte-identity, re-proved at the final tip

```
git diff --exit-code -- scripts/f20-native-windows-proof.ps1   → exit 0 (no diff)
git show f20a-candidate-9821ef76:scripts/f20-native-windows-proof.ps1 | diff - scripts/…  → IDENTICAL
```

The `$targets` array is byte-identical to the sealed version; no target was
added, removed, reordered, or reselected.

## 12.3 The seal — ONE tag, both roles

`f20a-candidate-50cf00b3` and `f20a-harness-39d30e55` are **stale** and
superseded.

§9 needed two refs only because the workflow fix post-dated the seal. At
`9821ef76` the fix and the candidate source are **in the same tree**, so one
tag serves both roles. Verified directly from the tag object, not the worktree:

```
git show f20a-candidate-9821ef76:.github/workflows/nightly-windows-soak.yml
  f20_candidate:        (line 29)
  f20_expected_sha:     (line 43)   ← the fix
  f20_macos_runner_label: (line 48)
  f20_request_nonce:    (line 53)
  "Assert checkout is the authorized candidate"          × 2  (both candidate jobs)
  ref: ${{ github.event.inputs.f20_expected_sha }}       × 2  (both checkouts pinned)
```

So `--ref f20a-candidate-9821ef76` supplies the **fixed, non-tautological
workflow definition** AND resolves to the **exact proven tree**. The two-ref
split described in §9 no longer applies.

Pushed to `gh` (tag only; no branch force, no merge, no PR):

```
git push gh refs/tags/f20a-candidate-9821ef76
 * [new tag]  f20a-candidate-9821ef76 -> f20a-candidate-9821ef76

git ls-remote --tags gh 'refs/tags/f20a*'
48601e469b3a3fca524811cc37e0a6ce6841e457  refs/tags/f20a-candidate-9821ef76
9821ef7603ac1e687b600cda591af1657c883484  refs/tags/f20a-candidate-9821ef76^{}
```

## 12.4 THE UNFIRED DISPATCH COMMAND — SUPERSEDED BY §13.1: IT WAS FIRED

> **Superseded 2026-07-26.** The command below was subsequently authorized by
> Sean and run **verbatim, once** → run `30184651330`, `completed/success`, both
> acceptance markers PASS. The "has NOT been run" statement was true when
> written and is false now. See §13.1.

Every input below is fully resolvable and verified. **B1, B2 and B3 are all
closed.** This command has **NOT** been run.

```bash
# UNFIRED. Sean's authorization only. Firing this burns one authorization.
gh auth switch --user FerroxLabs

gh workflow run nightly-windows-soak.yml \
  -R FerroxLabs/wayland-core \
  --ref f20a-candidate-9821ef76 \
  -f f20_candidate=true \
  -f f20_expected_sha=9821ef7603ac1e687b600cda591af1657c883484 \
  -f f20_request_nonce=96c91107636c4eaca9130969369b2309ee6dd6582cc4e9e1a7a45e0fb8ec92cf \
  -f f20_macos_runner_label=f20-image-1d05364078523334605249687228ffec79964b7ecf731d7c9512b40e67fd1a64
```

Resolvability of each input, verified:

| Input | Value | Verified |
|-------|-------|----------|
| `--ref` | `f20a-candidate-9821ef76` | tag exists on `gh`, dereferences to `9821ef76…`, and carries the fixed workflow |
| `f20_expected_sha` | `9821ef76…` | 40 lowercase hex; equals the tag's commit, so the pinned checkout satisfies both assert steps |
| `f20_request_nonce` | `96c91107…` | 64 lowercase hex; `sha256(final-sha)`; matches the proof script's `^[0-9a-f]{32,64}$` |
| `f20_macos_runner_label` | `f20-image-1d053640…1a64` | runner id **27**, `f20-macos-ephemeral-1d053640`, macOS/ARM64, **status online, busy false** |

## 12.5 Four-suite Windows baseline at the final tip (SEANDESKTOP)

`WAYLAND_SANDBOX_LIVE_WINDOWS=1`, box idle (CPU 6%, no compilers), tree clean,
`HEAD` = `9821ef76…` before and after. Actual numbers:

| # | Command | Result |
|---|---------|--------|
| 1 | `cargo nextest run -p wcore-sandbox --no-fail-fast` | **136 run: 131 passed, 5 failed, 45 skipped** (exit 100) |
| 2 | `cargo nextest run -p wcore-swarm --no-fail-fast` | **91 run: 84 passed, 7 failed, 6 skipped** (exit 100) |
| 3 | `cargo nextest run -p wcore-agent --test transactional_delegated_mutation_test --run-ignored all --no-fail-fast` | **9 run: 9 passed, 0 skipped** (exit 0) |
| 4 | `cargo nextest run -p wcore-swarm --test dispatch_smoke --no-fail-fast` | **7 run: 3 passed, 4 failed, 3 skipped** (exit 100) |

Suite 1 failures (all `wcore-sandbox::live_integrity`):
`live_cmd_runs_when_allowlist_has_missing_path`,
`live_cmd_builtin_runs_under_hardened_sandbox`,
`live_lsa_dependent_tool_fails_under_hardened_sandbox`,
`live_future_drop_reaps_descendant_job_tree`,
`live_runaway_command_is_bounded_by_timeout`.

Suite 2 failures: `dispatch_smoke::malformed_heartbeat_fails_closed_and_preserves_bounded_diagnostic`,
`dispatch_smoke::public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state`,
`dispatch_smoke::required_live_windows_public_dispatch_refuses_bash_worker_and_preserves_parent_and_sibling_state`,
`dispatch_smoke::dispatches_4_noop_workers_in_parallel`,
`worker_runtime_limits::timeout_releases_workspace_and_capacity_before_return`,
`worker_runtime_limits::multi_worker_output_exhaustion_fails_without_retaining_buffers`,
`swarm_worker_failure_reporting_e2e::swarm_reports_failed_worker_status_and_succeeding_workers_complete`.

Suite 4 failures: the four `dispatch_smoke` tests above.

### Why suite-mode reds do NOT contradict the 6/6 proof — measured, not asserted

`dispatch_smoke::public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state`
**passes in the proof and fails in the plain suite.** The mechanism was measured:

- The proof invokes the target with `--run-ignored all … --nocapture`. `--nocapture`
  makes nextest run **serially**; the transcript shows strictly sequential
  `START (1/10) → PASS (1/10) → START (2/10) → …` with no interleaving, and
  **10 tests run: 10 passed**.
- The plain suite has no `--nocapture`, so nextest runs tests **in parallel**.
  These tests spawn real sandboxed worker subprocesses; under concurrent spawns
  the backend probe degrades and `admit_delegated_backend`
  (`crates/wcore-swarm/src/dispatch.rs:33`) rejects with the exact text:

  ```
  status: Failed("sandbox backend fail_closed cannot enforce delegated read denial")
  ```

  i.e. `registry.backend_name()` == `fail_closed` — the hardened backend was not
  selected, so the test correctly refuses to proceed. This is the same
  contention sensitivity already flagged for this host, now with its precise
  cause recorded, and it is **not** a regression from the `session_journal`
  commits.

**The acceptance gate is the six-target proof, not the plain suites.** No source,
test, `scripts/f20-native-windows-proof.ps1`, or `crates/wcore-swarm/src/dispatch.rs`
was modified to obtain these numbers.

> Method note: an initial measurement added `--run-ignored all` to suites 1, 2
> and 4 as well. That is **wrong** and its numbers are discarded — it force-runs
> `*_fixture` helper tests (`standalone_authority_fixture`,
> `flood_worker_fixture`, `capacity_registration_fixture`, …) that exist only to
> be spawned as child processes by their parent test, and which fail by design
> when executed directly (visible as ~0.006 s failures). The table above is the
> corrected run using exactly the specified commands.

## 12.6 Linux aggregate at the final tip (Hetzner)

Host pinned and verified: `HEAD` `9821ef76…` / tree `0a1267a9…` **before and
after** the run, `git status --porcelain --untracked-files=all` empty. Fetched
with `git fetch origin plan/f20-unified-audit-repair` (the pinned refspec would
otherwise miss this branch). Load average 5.77 on 96 cores at start.

```
cargo build --locked --workspace --all-features     → BUILD_EXIT=0
cargo nextest run --profile ci --no-fail-fast       → NEXTEST_EXIT=0
Summary [187.312s] 11520 tests run: 11520 passed (1 slow, 1 flaky), 48 skipped
```

**11520 run, 11520 passed.** This is **better** than the expected baseline: the
3 previously non-passing tests did not reproduce. The two 60 s timeouts were
contention artefacts — the earlier measurement ran at load average ~78, this one
at ~5.8 — and the load-flake
(`wcore-cli::deterministic_openai_loop::packaged_core_cancels_an_active_stream`)
retried green (`FLAKY 3/3`).

## 12.7 Known-red items that are NOT proof targets and do NOT gate acceptance

| Item | Status at final tip | Note |
|------|---------------------|------|
| `wcore-sandbox::live_integrity::live_future_drop_reaps_descendant_job_tree` | **RED** (reproduced) | Windows; deterministic. Escalated: every remaining fix changes what the sandbox permits. Not a proof target. |
| `wcore-agent` `snapshot.rs::windows_private_dacl_accepts_restrictive_deny_ace` | RED | `WRITE_DAC` reopen error 5. Fails identically at the parent commit. Unit tests in `crates/wcore-agent/src/session_journal/snapshot.rs` — **not** reached by any of the four suites (hence absent from §12.5). |
| `wcore-agent` `snapshot.rs::windows_private_dacl_rejects_null_empty_and_broad_allow` | RED | as above |
| `wcore-swarm::worker_runtime_limits::multi_worker_output_exhaustion_fails_without_retaining_buffers` | **RED** (reproduced) | ~35 s against a 20 s budget. The timeout is deliberately **NOT** raised. |
| The 3 pre-existing Linux non-passing tests | **NOT REPRODUCED** | Full Linux aggregate was 11520/11520 on a quiet box (§12.6). |
| Additional `live_integrity` reds (4) + `wcore-swarm` parallel-mode reds (6) | RED in plain suite mode only | Cause measured in §12.5: `fail_closed` backend under parallel sandboxed spawns. Green under the proof's serial execution. |

### Known caveat in the evidence chain — the `f20-no-ambient-secrets` label is NOT accurate

Runner id 27 (`f20-macos-ephemeral-1d053640`) advertises the label
`f20-no-ambient-secrets`. **On this host that label is inaccurate and must not
be relied on.** The runner executes as Sean's own user, with reach over
`~/.ssh`, `~/.aws`, and an unlocked login keychain. What *is* true is that **no
GitHub secrets are exposed** to the candidate jobs. The label overstates the
isolation; record it as a known caveat, not as a proven property.

## 12.8 Bounds honoured

- No Rust source, no test, no `scripts/f20-native-windows-proof.ps1`, no
  `crates/wcore-swarm/src/dispatch.rs` modified — `$targets` byte-identity
  proved twice (§12.2).
- No failing test fixed; every red reported as-is.
- **No `gh workflow run` fired.** No main push, no merge, no PR, no release, no
  issue closure. Only the branch (already present) and the new **tag** touched `gh`.
- `origin` in `waylandcore-ferrox` (a stale local worktree) was never fetched or
  reset against; all remote operations used `gh` explicitly.
- No `AGENTS.md` or `.ijfw` churn staged.

## Self-Check (§12): PASSED

- Final SHA/tree/nonce — computed and quoted verbatim; nonce re-derivable via the printed command.
- Six-target result — all six `F20_NATIVE_TARGET=PASS` lines plus the single terminal `F20_NATIVE_WINDOWS_ACCEPTANCE=PASS` and `PROOF_EXIT=0` quoted verbatim from the SEANDESKTOP transcript.
- Tag — `git push` output and `git ls-remote --tags gh` quoted verbatim; dual-role verified by reading the workflow **out of the tag object**.
- `$targets` — `git diff --exit-code` exit 0 and a tag-vs-worktree `diff` quoted verbatim.
- Four-suite baseline — nextest `Summary` lines and per-test failure names quoted verbatim; the discarded first attempt disclosed rather than hidden.
- Linux aggregate — `Summary` line and both exit codes quoted verbatim; HEAD pinned and re-verified after the run.
- Runner label — `gh api .../actions/runners` JSON read directly.

---

# 13. DISPATCH FIRED — Phase 20A CLOSED — 2026-07-26

> **This section supersedes §1 (termination state 4), §8 (zero requirements
> completed), §9 (the unfired command) and §12.4 (the unfired command).** Those
> sections are kept verbatim as history: they were true when written, and the
> record of *why* the dispatch was withheld for three candidates is worth more
> than a tidy file. Where §13 conflicts with anything above it, **§13 wins.**
>
> The plan's frontmatter still reads `termination_state: 4`. That was the state
> the plan terminated in. The *phase* did not end there — Sean authorized the
> dispatch afterwards and it was fired on his instruction. **Effective terminal
> state: 1 — complete.**

## 13.1 The fired dispatch

The command recorded as UNFIRED in §12.4 was run verbatim, once, on Sean's
explicit authorization.

| Field | Value |
|-------|-------|
| Workflow | `nightly-windows-soak` |
| **Run id** | **`30184651330`** |
| URL | `https://github.com/FerroxLabs/wayland-core/actions/runs/30184651330` |
| Event | `workflow_dispatch` |
| `--ref` | `f20a-candidate-9821ef76` (annotated tag → `48601e46…` → commit `9821ef76…`) |
| Status / conclusion | `completed` / **`success`** |
| headSha | `9821ef7603ac1e687b600cda591af1657c883484` |
| Started → finished | `2026-07-26T02:30:03Z` → `2026-07-26T02:48:08Z` (18m 05s) |
| `f20_expected_sha` | `9821ef7603ac1e687b600cda591af1657c883484` |
| `f20_request_nonce` | `96c91107636c4eaca9130969369b2309ee6dd6582cc4e9e1a7a45e0fb8ec92cf` |
| `f20_macos_runner_label` | `f20-image-1d05364078523334605249687228ffec79964b7ecf731d7c9512b40e67fd1a64` |

Independently re-verified at closeout via `gh` (account `FerroxLabs`), not
carried forward from the dispatching session:

```
gh api repos/FerroxLabs/wayland-core/git/refs/tags/f20a-candidate-9821ef76
  → tag object 48601e469b3a3fca524811cc37e0a6ce6841e457
gh api repos/FerroxLabs/wayland-core/git/tags/48601e46…
  → object.sha 9821ef7603ac1e687b600cda591af1657c883484   (type: commit)
  → message declares Tree 0a1267a9…  Nonce 96c91107…
git rev-parse refs/f20a/candidate
  → 9821ef7603ac1e687b600cda591af1657c883484
git cat-file -p 9821ef76… | head -1
  → tree 0a1267a990f3b512782916b6ed26501d0db39222
```

Sealed SHA, `refs/f20a/candidate`, the tag object and the run's `headSha` all
agree. The tree in the commit object matches the tree in the tag message and the
tree in every marker.

> `refs/f20a/candidate` exists **locally only** — `gh api …/git/matching-refs/f20a`
> returns `[]`. That is by design (the tag is the remote-durable seal), and it is
> recorded here so nobody looks for a remote `refs/f20a/*` and concludes it was
> deleted.

## 13.2 Jobs — what ran, what was skipped

| Job | id | Conclusion |
|-----|----|-----------|
| F20 native macOS candidate (ephemeral) | `89747992986` | **success** |
| F20 native Windows candidate (self-hosted msvc) | `89747993276` | **success** |
| Windows soak (windows-2022) | `89747993117` | skipped — `if: inputs.f20_candidate != 'true'` |
| Windows live-acceptance ignored set (self-hosted msvc) | `89747993309` | skipped — `if: inputs.f20_candidate != 'true'` |

Both skips are by construction in candidate mode, not failures. **The second
skip has a consequence** — see REQ-native-r2 in §13.6.

**Runners actually used** (from the job logs, not from labels):

```
Windows: Runner name 'ferrox-win-msvc'              Machine name 'SEANDESKTOP'
macOS:   Runner name 'f20-macos-ephemeral-1d053640' Machine name 'Seans-MacBook-Pro'
```

Both jobs ran the `Assert checkout is the authorized candidate` step with
`F20_EXPECTED_SHA: 9821ef7603ac1e687b600cda591af1657c883484` **before** any
toolchain install or proof work, and both pinned `actions/checkout` to that SHA.
The tautology B3 identified is closed in the fired run, not merely in the YAML.

## 13.3 Acceptance markers — both PASS, same commit/tree/nonce

```
F20_NATIVE_WINDOWS_ACCEPTANCE=PASS commit=9821ef7603ac1e687b600cda591af1657c883484 tree=0a1267a990f3b512782916b6ed26501d0db39222 nonce=96c91107636c4eaca9130969369b2309ee6dd6582cc4e9e1a7a45e0fb8ec92cf
F20_NATIVE_MACOS_ACCEPTANCE=PASS   commit=9821ef7603ac1e687b600cda591af1657c883484 tree=0a1267a990f3b512782916b6ed26501d0db39222 nonce=96c91107636c4eaca9130969369b2309ee6dd6582cc4e9e1a7a45e0fb8ec92cf
```

Windows marker at `02:34:04Z` (job log line 1446), macOS marker at `02:47:56Z`
(job log line 1085). Each of the fourteen `F20_NATIVE_TARGET=PASS` lines carries
the same three bindings, so no target's evidence can have come from a different
tree or a different request.

## 13.4 Per-target results — 6 Windows + 8 macOS, all PASS

### Windows (job `89747993276`, `ferrox-win-msvc` / SEANDESKTOP)

| # | Target | Result | Tests | Named tests that passed |
|---|--------|--------|-------|--------------------------|
| 1 | `windows-retained-handle` | **PASS** | 1 run / 1 passed / 11 skipped (2.044s) | `live_fs_acl::one_execution_grant_never_leaks_to_another_identity` |
| 2 | `windows-appcontainer-acl` | **PASS** | 1 run / 1 passed / 11 skipped (2.056s) | `live_fs_acl::granted_path_is_readable_then_revoked` |
| 3 | `windows-job-object` | **PASS** | 4 run / 4 passed / 2 skipped (10.230s) | `active_process_cap_is_enforced`, `breakaway_is_denied`, `contained_detached_child_exit`, `job_close_reaps_detached_descendant_with_no_residue` |
| 4 | `windows-public-dispatch` | **PASS** | 10 run / **10 passed** / 0 skipped (11.983s) | `dispatch_rejects_different_head_repository_replacement`, `dispatch_rejects_same_head_repository_replacement`, `dispatches_4_noop_workers_in_parallel`, `malformed_heartbeat_fails_closed_and_preserves_bounded_diagnostic`, `malformed_heartbeat_fixture`, `public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state`, `repository_replaced_at_same_pathname_is_refused_by_retained_authority`, `repository_replacement_must_not_execute`, `required_live_windows_public_dispatch_refuses_bash_worker_and_preserves_parent_and_sibling_state`, `standalone_authority_fixture` |
| 5 | `windows-hard-process-containment` | **PASS** | 1 run / 1 passed / 5 skipped (4.718s) | `qualified_hard_containment_backend_preflight` |
| 6 | `windows-f20-lifecycle` | **PASS** | 9 run / **9 passed** / 0 skipped (26.251s) | all nine `transactional_delegated_mutation_test` cases incl. `happy_path_open_accept_land_receipt_then_rollback` and `restart_replays_landed_state_from_disk` |

Targets 4 and 6 — the two that were RED at `50cf00b3` (§6.2, §6.3) — are green
here, on a runner, in a dispatched run. §7/B1 is closed by measurement, twice
now (locally in §12.2, and in CI here).

### macOS (job `89747992986`, `f20-macos-ephemeral-1d053640` / Seans-MacBook-Pro)

| # | Target | Result | Test that ran |
|---|--------|--------|---------------|
| 1 | `macos-retained-directory` | **PASS** (0.065s) | `wcore-sandbox::live_integrity_macos required_live_macos_retained_directory_confines_writes` |
| 2 | `macos-process-tree` | **PASS** (0.132s) | `wcore-sandbox::hard_process_containment_macos required_live_macos_process_tree_contains_descendants` |
| 3 | `macos-docker-reject-path-replacement` | **PASS** (0.019s) | `wcore-sandbox::docker_smoke docker_rejects_allow_hosts_policy` |
| 4 | `macos-docker-roundtrip-delete` | **PASS** (0.232s) | `wcore-sandbox::docker_smoke docker_runs_hello_world` |
| 5 | `macos-public-dispatch` | **PASS** (0.023s) | `wcore-swarm dispatch::tests::sandbox_exec_is_refused_before_descendant_escape_can_spawn` |
| 6 | `macos-docker-cancellation` | **PASS** (0.206s) | `wcore-sandbox::docker_smoke docker_returns_enforced_resource_limits` |
| 7 | `macos-docker-budget` | **PASS** (0.227s) | `wcore-swarm::workspace_authority required_live_macos_docker_rejects_over_budget_result` |
| 8 | `macos-f20-lifecycle` | **PASS** | `wcore-agent::transactional_delegated_mutation_test` — 9 run / 9 passed |

Every macOS target resolves to a real, named, OS-appropriate test. Target 1 in
particular resolves to `live_integrity_macos`, **not** the Windows-only
retained-handle test it once pointed at — the r14 mapping fix, proven by
execution.

## 13.5 Phase 20A Success Criteria — all three met

| # | Criterion | Met by |
|---|-----------|--------|
| 1 | Six-target native Windows proof passes on the certified self-hosted runner against one exact sealed candidate | 6/6 PASS on `ferrox-win-msvc`, every marker bound to `9821ef76`/`0a1267a9`/`96c91107` |
| 2 | The macOS leg passes against that same exact candidate | 8/8 PASS, identical commit/tree/nonce triple |
| 3 | Native evidence is bound to one newly dispatched, Sean-authorized run — never inferred from source, cross-compilation, Linux proof, or a reused run | Run `30184651330`, `workflow_dispatch`, created 2026-07-26T02:30:03Z, fired once on Sean's explicit instruction. Not a re-read of a prior run; the previous soak run was `30149496548` (scheduled, headSha `61b79c4f`). |

**Phase 20A is COMPLETE.**

## 13.6 Requirement dispositions bound to run `30184651330`

Full per-requirement evidence is written into `.planning/REQUIREMENTS.md`. Tally
and reasoning:

**COMPLETE — 11:**

| Req | Proving evidence (not "present at the SHA" — actually exercised) |
|-----|------------------------------------------------------------------|
| r1 | `require_live_acceptance()`/`require_live_windows()` **hard-assert** `AppContainerBackend::new().is_available()`. Four targets (1, 2, 3, 5) run through those guards and PASS. A false `is_available()` fails the assert. |
| r3 | Target 6 compiled and executed `-p wcore-agent --test transactional_delegated_mutation_test` on msvc, 9/9. E0432 cannot survive a linked, executed binary. |
| r4 | macOS 8/8 in this run. Linux half from §12.6 (`--locked` build exit 0; 11520/11520) — **not** part of this run, and no compressed log retained. Disclosed, not glossed. |
| r5 | The acceptance's exact test, `one_execution_grant_never_leaks_to_another_identity`, PASS 2.043s. |
| r6 | The acceptance's exact test, `dispatch_rejects_different_head_repository_replacement`, PASS 0.121s — no Os code 5. |
| r7 | Target 3's four Job-Object tests cover the four named mechanisms (cap, breakaway, exit fidelity, KILL_ON_JOB_CLOSE reaping with no residue); target 5's preflight asserts `owns_descendants_hard` / `enforces_read_deny` / `blocks_powershell` and drives a live contained command. |
| r9 | All 8 macOS targets real and green; each named above. |
| r10 | `Cargo.lock` committed in the sealed tree (blob `60bcfb50…`, `+9` vs `be84bd2`, named deps resolve); `--locked` build exit 0 on Hetzner. This run does not use `--locked` — noted. |
| r11 | `Runner name: 'ferrox-win-msvc'`, `Machine name: 'SEANDESKTOP'`; the hosted `windows-2022` job was skipped. |
| r14 | Windows compile half: `windows_impl` tree executed real tests on msvc. macOS half: `macos-retained-directory` → `live_integrity_macos::required_live_macos_retained_directory_confines_writes` PASS. |
| r15 | `be84bd2` is a verified ancestor of `0e8e6c1d`, the single fresh 3-file commit landing r1+r2+r3. At the seal, `process.rs` is `CreateRestrictedToken(…, DISABLE_MAX_PRIVILEGE, 0, null, …)` — no token swap. Both jobs re-asserted `HEAD == 9821ef76…` in-run. |

**OPEN — 4.** None of these is a red test. Each is a path that was never
exercised, and saying so is the point.

> **Superseded in part by §13.10 (post-seal sweep, 2026-07-26): r2 and r8 are
> now CLOSED on hardware evidence; r12 and r13 remain open exactly as written
> below.** The r2/r8 rows are kept verbatim as the seal-time record — they state
> correctly what was and was not proven by run `30184651330`.

| Req | Why it stays open |
|-----|-------------------|
| **r2** | 1 of 3 acceptance clauses proven. `granted_path_is_readable_then_revoked` PASS (grant ACE present-during, exit 0 + MARKER, ACE absent-after) and `one_execution_grant_never_leaks_to_another_identity` PASS. But the acceptance also requires "a genuine DENY ace still blocks" (`deny_ace_still_blocks_granted_read`) and "a file granted only to normal SIDs is still denied" (`normal_sid_only_grant_is_denied`). Neither is in the six targets. Their only runner is the `windows-live-acceptance` job, gated `if: inputs.f20_candidate != 'true'` (`nightly-windows-soak.yml:239`) — **skipped by construction in candidate mode**. 20A-01 wired these ten orphaned ACL tests into exactly that job, so they still have no observed green at the sealed SHA. |
| **r8** | The wrong-OS anti-drift guard RAN — `Assert-TargetOsGate` before every Windows target, the shell mirror before every macOS target — and admitted all six OS-specific targets without firing. But the acceptance is that a target mapped to the **wrong** OS is *rejected*, and that direction has never been demonstrated. `scripts/f20-native-uat-proof.test.mjs` (34 cases) covers marker parsing/ordering/nonce/publication; it has no wrong-OS case. Admitting correct mappings is not proof of rejection. |
| **r12** | Native leg closed (build + Hetzner aggregate + Win/mac proof, one SHA, one authorized run). Not closed: there is **no fresh 20-16 at `9821ef76`**. The only 20-16 is bound to `6937ef61…` / tree `6db6fc85` and its own key-decisions name `native_macos` and `native_windows` as "the only deferred checks" — exactly the deferral r12 forbids. `20-17` is likewise bound to that older SHA. None of these clauses is in Phase 20A's Success Criteria, so the phase closes; r12 does not. |
| **r13** | No review gate ran against `9821ef76`, so no artifact exists for this candidate. The pre-existing gap is also still measurable: `20-16-SUMMARY.md` claims **two** reviewers (`wayland-f20-16-repair-review`, `wayland-f20-16-adversarial-confirmer`); its sole artifact `20-08-INDEPENDENT-REVIEW.md` (schema `wayland-core.phase20-independent-review.v1`) carries **one** `reviewer_id`. Recording a PASS here is what r13 exists to forbid. |

## 13.7 KNOWN-RED, NON-GATING — recorded so nobody rediscovers them

None of the following is a proof target. None gates acceptance. All are recorded
as-is; **nothing was fixed, ignored, re-gated, re-timed or deleted to reach the
green above.**

1. **`wcore-sandbox::live_integrity::live_future_drop_reaps_descendant_job_tree`** — Windows,
   **deterministic** (reproduces every time, not flaky). Escalated rather than
   fixed because every candidate fix changes *what the sandbox permits* — i.e.
   the remedy is a security-semantics decision, not a bug fix. Not a proof
   target.

2. **`wcore-agent` `session_journal/snapshot.rs::windows_private_dacl_accepts_restrictive_deny_ace`**
   and **`::windows_private_dacl_rejects_null_empty_and_broad_allow`** — both RED
   on `WRITE_DAC` reopen **error 5**. They fail **identically at the parent
   commit**, so they are not a regression from this candidate. They are *unit*
   tests inside `snapshot.rs` and are **not reached by any of the four suites**
   in §12.5 — which is why they do not appear in those numbers. Their absence
   from the suite tables is not an oversight; it is the reason this note exists.

3. **`wcore-swarm::worker_runtime_limits::multi_worker_output_exhaustion_fails_without_retaining_buffers`**
   — takes **~35s against a 20s budget**. The timeout was **deliberately NOT
   raised.** Raising it is the exact "engineered green" this phase refuses.

4. **`required_live_windows_public_dispatch_bash_confines_parent_and_descendants`**
   — **bash cannot run under AppContainer at all.** msys/bash requires
   `\BaseNamedObjects`, which AppContainer confines *by construction*. There is
   no fix that keeps both properties. The test was therefore rewritten to assert
   the **real fail-closed contract** — it now ships as
   `required_live_windows_public_dispatch_refuses_bash_worker_and_preserves_parent_and_sibling_state`
   and PASSES in this run (target 4, test 9/10). Recorded here because the old
   name still appears in older documents and reads like an unexplained
   disappearance.

5. **Parallel-load degradation of `admit_delegated_backend`.** Under concurrent
   sandboxed spawns the backend probe degrades and
   `admit_delegated_backend` (`crates/wcore-swarm/src/dispatch.rs:33`) rejects
   with `sandbox backend fail_closed cannot enforce delegated read denial` —
   `registry.backend_name()` resolves to `fail_closed`. **The proof passes
   because `--nocapture` forces nextest to run serially**, which the transcript
   confirms (strict `START (n/10) → PASS (n/10)` with no interleaving). This is
   **fail-closed, therefore safe** — it refuses rather than proceeding
   unsandboxed. But it is honest to say: **concurrent dispatch degrades under
   load**, and the green depends on serialisation. Cause measured, not asserted
   (§12.5).

Also still open and non-gating: the four additional plain-suite `live_integrity`
reds and six `wcore-swarm` parallel-mode reds (same cause as item 5); the 30
Windows failures F-09 unmasked (18 in `session_journal`), un-baselined.

## 13.8 FOLLOW-UPS for the next candidate — recorded, deliberately NOT fixed here

Both are real defects. Neither is fixed in this closeout, because this closeout
changes nothing outside `.planning/`.

**F1 — the proof script's unconditional `docker pull`.**
`scripts/f20-native-macos-proof.sh:134` runs `docker pull "$image"` with no
prior `docker image inspect` check. On a host with the image already present
this is a pointless network round-trip that also touches the credential store —
and it is **why the macOS leg failed four times on a locked keychain** before
the run that finally went green. Fix: probe `docker image inspect "$image"`
first and only `docker pull` on a miss. Low risk, high annoyance-reduction.

**F2 — the `f20-no-ambient-secrets` runner label is inaccurate on this host.**
Runner id 27 (`f20-macos-ephemeral-1d053640`) advertises
`f20-no-ambient-secrets`, but the job log shows `Machine name: 'Seans-MacBook-Pro'`
and the runner executes **as Sean's own user**, with reach over `~/.ssh`,
`~/.aws`, and an unlocked login keychain. What *is* true: no GitHub Actions
secrets are exposed to the candidate jobs. What is **not** true: the absence of
ambient credentials. A label that overstates isolation is worse than no label,
because downstream gates may key on it. **The real fix is a dedicated macOS
runner account** (separate user, no keychain, no `~/.ssh`, no `~/.aws`) — not a
relabel. Until then, treat that label as aspirational and cite this note.

## 13.9 Bounds honoured by this closeout

- Files changed: **only** under `.planning/` —
  `REQUIREMENTS.md`, `ROADMAP.md`,
  `phases/20A-native-windows-macos-uat/20A-04-SUMMARY.md`,
  `phases/20A-native-windows-macos-uat/.continue-here.md`.
  Nothing in `crates/`, `scripts/`, or `.github/`.
- **No workflow dispatched by this closeout.** Run `30184651330` was fired
  earlier on Sean's instruction; this section records it. No push to main, no
  merge, no PR, no new tag, no release, no issue closure.
- `origin` in `waylandcore-ferrox` (a stale local worktree) was never fetched or
  reset against. Every remote read used `gh` explicitly, under
  `gh auth switch --user FerroxLabs`.
- No `AGENTS.md` or `.ijfw` churn staged. No `Co-Authored-By`.
- Nothing was recorded as green that could not be re-derived from the run at
  closeout time; four requirements were left **open** rather than ticked.

## Self-Check (§13): PASSED

Every claim in §13 was re-derived at closeout from primary sources, not copied
from the dispatching session:

- Run metadata (`conclusion`, `event`, `headSha`, timestamps, job ids/conclusions)
  — `gh run view 30184651330 --json …`, read directly.
- Both acceptance markers and all fourteen per-target markers — pulled from
  `gh api repos/FerroxLabs/wayland-core/actions/jobs/{89747993276,89747992986}/logs`
  and grepped verbatim.
- Per-test PASS names and timings — extracted from the same two job logs.
- Runner identity — `Runner name` / `Machine name` lines from the job logs, not
  from the runner registry.
- Seal chain (tag → tag object → commit → tree) — `gh api …/git/refs/tags/…`,
  `gh api …/git/tags/…`, `git cat-file -p`, `git rev-parse refs/f20a/candidate`.
- Target→test mappings — read out of `scripts/f20-native-windows-proof.ps1:83-88`
  and `scripts/f20-native-macos-proof.sh:286-302`.
- The r1 assertion chain — read out of `live_fs_acl.rs:29-38` and
  `hard_process_containment_windows.rs:64-74`.
- The r2 skip cause — read out of `nightly-windows-soak.yml:238-282`.
- The r12/r13 gaps — read out of `20-16-SUMMARY.md` frontmatter and
  `20-08-INDEPENDENT-REVIEW.md`.
- The r15 lineage — `git merge-base --is-ancestor be84bd2 0e8e6c1d` → yes;
  `git show --stat 0e8e6c1d`.

---

## 13.10 POST-SEAL SWEEP (2026-07-26) — r2 and r8 CLOSED, two harness defects found

Bounded sweep of the two cheap open requirements. Nothing about the seal,
the tag, or run `30184651330` changes; this section only adds evidence.

**Product identity to the seal.** Every observation below was made at commit
`2cc1a285ffd3f3b0fb41b177bd9a1317654cb350`, not literally `9821ef76`.
`git diff 9821ef76 2cc1a285 -- crates/ Cargo.lock Cargo.toml` is **empty** —
the product tree under test is byte-identical to the sealed tree. Everything
that differs is CI/harness: `.github/workflows/nightly-windows-soak.yml`,
`scripts/wayland-e2e-windows-soak.ps1`, `scripts/f20-native-uat-proof.mjs`,
`scripts/f20-native-uat-proof.test.mjs` (plus an unrelated `ci.yml` change
from a concurrent lane).

### 13.10.1 The `$targets` invariant — re-proved, untouched

```
git diff --exit-code -- scripts/f20-native-windows-proof.ps1   ->  exit 0, zero output
```

`scripts/f20-native-windows-proof.ps1` was never edited. No target was added to
it, so `verifyNativeLog`'s fail-closed set of six canonical markers, their order
and their uniqueness are all unchanged. The two ACL tests were wired through a
**different** job, which emits no `F20_NATIVE_*` marker at all.

### 13.10.2 r2 — the two DENY tests, PASS on real Windows

Trap first, because it nearly produced a false RED: **these tests cannot be
observed over SSH.** A non-interactive session-0 SSH logon to `SEANDESKTOP`
(`whoami=seand`, `SessionId=0`, `UserInteractive=False`) reports
`AppContainerBackend::is_available() == false`, so every test in the file panics
at `live_fs_acl.rs:34` regardless of correctness. Established by control, not
assumed: the CI-certified-green `granted_path_is_readable_then_revoked` fails
identically over SSH at the sealed SHA. Only the runner service is a valid
environment.

Observed through the runner, run **`30186873948`**, job **`89753061944`**,
`Runner name: 'ferrox-win-msvc'`, `Machine name: 'SEANDESKTOP'`,
job conclusion **`success`**:

```
· WAYLAND_SANDBOX_LIVE_WINDOWS=[1] (len=1)
✓ live-acceptance flag proven effective (exactly '1', no trailing space)
        PASS [   0.210s] ( 3/12) wcore-sandbox::live_fs_acl deny_ace_still_blocks_granted_read
        PASS [   0.235s] ( 6/12) wcore-sandbox::live_fs_acl normal_sid_only_grant_is_denied
     Summary [  15.282s] 12 tests run: 12 passed, 0 skipped
     Summary [  15.157s] 6 tests run: 6 passed, 0 skipped
✓ live-acceptance suite live_fs_acl passed
✓ live-acceptance suite hard_process_containment_windows passed
✓ PHASE L complete (live_fs_acl + hard_process_containment_windows ignored sets)
═══ WINDOWS LIVE-ACCEPTANCE SOAK: PASS ═══
```

Reproduced one run earlier (`30186743564`, job `89752739969`): the same two tests
PASS at 0.192s / 0.192s, `12 tests run: 12 passed (1 flaky)`. **The flake is
recorded, not swept:** `concurrent_allow_and_deny_identities_do_not_interfere`
FAILED try 1 (`ordinary allow identity must retain access`, 0.250s) and PASSED
try 2 under nextest retry. It is green in `30186873948` without a retry. Not one
of the two r2 tests, not a proof target, and nothing was re-timed or re-gated for
it — it is logged here so nobody rediscovers it.

### 13.10.3 r2 — the wiring, repaired

`windows-live-acceptance` was gated `if: github.event.inputs.f20_candidate != 'true'`,
making it the sole runner of these two tests *and* excluding it from the only
dispatch mode that proves a candidate. It now runs in both modes:

- **candidate:** `needs: f20-windows-candidate` and
  `if: !cancelled() && (inputs.f20_candidate != 'true' || needs.f20-windows-candidate.result == 'success')`.
  `needs:` is load-bearing — both jobs target the SAME self-hosted box, and
  concurrent compile load has corrupted a proof run before, so this job must
  never run alongside the six-target proof. Its checkout is pinned to
  `f20_expected_sha` and it re-asserts `HEAD` against it, so its ACL evidence
  binds to the tree the proof certifies.
- **non-candidate:** `f20-windows-candidate` is skipped, `!cancelled()` admits
  this job, and an empty `f20_expected_sha` is what `actions/checkout` already
  treats as "not supplied" — pre-existing behaviour.

Verified in run `30186873948`: `F20 native Windows candidate` **skipped**,
`Windows live-acceptance` **success**. **Caveat, not glossed:** the candidate-mode
branch is verified by review plus a real non-candidate dispatch; no candidate
dispatch has been fired since, so that branch has not itself executed.

### 13.10.4 DEFECT FOUND — the soak harness could never report success

The first dispatch ran every test green and still failed:

```
✗ live-acceptance suite live_fs_acl failed with exit code <the entire compile+test log> 0
✗ PHASE L failed: live_fs_acl, hard_process_containment_windows
```

while the same log carried `12 tests run: 12 passed` and `6 tests run: 6 passed`.

Root cause is the exit-code capture idiom, used at three sites (phases F, G, L):

```powershell
$exit = & { cargo … 2>&1 | Tee-Object -FilePath $log; $LASTEXITCODE }
```

`Tee-Object` passes every line through, so the block returns an **array** of all
output lines plus the exit code, and `if ($exit -ne 0)` is an array **filter**
whose non-empty result is always truthy. Measured on `SEANDESKTOP` pwsh 7.6.3
against a command that exits 0 with two output lines:

```
BROKEN_FORM: type=Object[] count=3 verdict=REPORTS_FAILURE
FIXED_FORM:  type=Int32 value=0  verdict=reports_success
FIXED_FORM_ON_REAL_FAILURE: value=3 verdict=REPORTS_FAILURE
```

Fixed by reading `$LASTEXITCODE` after the pipeline. The third line is the point:
the fix still fails closed on a real non-zero exit — a phase that could never
report success merely becomes able to. This affected phases F and G too, i.e. the
whole nightly Windows soak was structurally incapable of reporting green.

### 13.10.5 r8 — both PRODUCTION guards driven to rejection

The guards themselves were driven, extracted **verbatim**; neither proof script
was modified or executed.

**Windows** — `Assert-TargetOsGate` + `Get-TargetTestSource` AST-extracted from
`scripts/f20-native-windows-proof.ps1` in the sealed checkout
(`CHECKOUT_HEAD=9821ef7603ac1e687b600cda591af1657c883484`,
`SOURCE_SHA256=a79d2ed47c4a97f16051c12ef9941e1afb97c61f0afdc334a3c8be79e163bbc6`,
lines 101-111 and 113-162), pwsh 7.6.3 on `SEANDESKTOP`:

```
CONTROL  windows-appcontainer-acl -> live_fs_acl (os=windows): ADMITTED (no throw)
WRONG-OS windows-appcontainer-acl -> hard_process_containment_macos (os=windows): REJECTED ->
  anti-drift: target windows-appcontainer-acl (os=windows) selects a test source cfg-gated for macos: …\hard_process_containment_macos.rs
WRONG-OS windows-job-object -> hard_process_containment (Linux bwrap, os=windows): REJECTED ->
  anti-drift: target windows-job-object declares os=windows but its selected test source is not cfg-gated for windows (a wrong-OS or ungated test cannot prove windows containment): …\hard_process_containment.rs
```

**macOS** — `assert_target_os_gate` extracted verbatim from
`scripts/f20-native-macos-proof.sh` (`SOURCE_SHA256=267582272bc57b078b2f13a875485e80d1fd641b35a5bb5289000e4f7cbd5236`):

```
CONTROL  macos-process-tree -> hard_process_containment_macos (os=macos): ADMITTED (exit 0)
WRONG-OS macos-retained-directory -> live_fs_acl (Windows-only, os=macos): REJECTED (exit 1) ->
  anti-drift: macos target macos-retained-directory source is not cfg-gated for macos: …/live_fs_acl.rs
WRONG-OS macos-process-tree -> hard_process_containment_windows (os=macos): REJECTED (exit 1) ->
  anti-drift: macos target macos-process-tree source is not cfg-gated for macos: …/hard_process_containment_windows.rs
```

The second macOS case is the exact 07-22 failure, run backwards: the mapping that
once shipped is now refused before cargo starts.

### 13.10.6 r8 — the durable regression

`scripts/f20-native-uat-proof.test.mjs`: **34 → 41 cases**. The rule is expressed
once beside the canonical map it governs (`assertTargetOsGate` over
`WINDOWS_TARGET_SOURCES` / `MACOS_TARGET_SOURCES`, the same map both production
guards mirror), because off a native runner the guards' rejection path is
otherwise unreachable. Seven new cases: admission of all six real OS-specific
target sources (so the guard cannot pass by rejecting everything), wrong-OS in
both directions, an ungated source, a foreign gate alongside a correct one, a cfg
named only in prose, and an unknown `os`.

Non-vacuity proven by mutation, not asserted:

| Mutation | Result |
|---|---|
| positive gate forced true | `not ok 21, 22, 23, 25` — 4 fail |
| foreign-OS negative gate skipped | `not ok 24` — 1 fail |
| cfg filter widened to whole file text (prose counts) | `not ok 21, 25` — 2 fail |
| none (restored) | 41 pass, 0 fail |

### 13.10.7 FINDING recorded, deliberately NOT fixed — prose-satisfiable positive gate

The Windows rejection above fired on the **negative** gate, not the positive one.
`hard_process_containment_macos.rs:13` is a doc comment reading
``//! (`#![cfg(windows)]`): on other platforms the file compiles to zero tests.``
and its only real cfg attribute is `#![cfg(target_os = "macos")]` at line 17. The
PowerShell guard matches whole file text, so **prose satisfied its positive
`cfg(windows)` check**; only the foreign-`target_os` check caught the mapping.

Consequence: an **ungated** source whose comments mention `cfg(windows)` would
pass the positive gate, carry no foreign `target_os`, and be **admitted** as a
Windows OS-specific target. The bash mirror is immune — it filters to `#[cfg…]`
attribute lines first (`f20-native-macos-proof.sh:226`), and the new
`assertTargetOsGate` does the same.

Not fixed here: the fix lives in `scripts/f20-native-windows-proof.ps1`, which
this sweep is required to leave with a zero diff. Recorded as a follow-up for the
next candidate — adopt the attribute-line filter in `Assert-TargetOsGate`.

### 13.10.8 Bounds honoured by this sweep

- `scripts/f20-native-windows-proof.ps1`: **zero diff**, `$targets` byte-identical.
- `crates/wcore-swarm/src/dispatch.rs`: untouched. No `crates/` file changed at all.
- Nothing weakened to reach green: no assertion relaxed, no `#[ignore]`, no
  `#[allow]`, no re-gate, no deleted test, no raised timeout. The one behavioural
  change to a gate (`$LASTEXITCODE` capture) was proven to still fail closed on a
  real non-zero exit before it was committed.
- Two dispatches fired, both on `plan/f20-unified-audit-repair`, both cancelled
  after the job of interest completed. No push to main, no merge, no PR, no tag,
  no release, no issue closure. `origin` (stale local worktree) never used; all
  remote work via `gh` under `gh auth switch --user FerroxLabs`.
- `.planning/intel/` untouched (a concurrent lane owns it). No `AGENTS.md` or
  `.ijfw` churn staged. No `Co-Authored-By`.
- Repair iterations used: **1 of 2** (the soak exit-code fix, then re-proved).

### 13.10.9 Disposition

| Req | Was | Now |
|---|---|---|
| r2 | OPEN — 1 of 3 clauses | **COMPLETE** — 3 of 3, both DENY tests PASS on `SEANDESKTOP`, wiring repaired |
| r8 | OPEN — rejection never shown | **COMPLETE** — both production guards rejected with the specific error; 7 regression cases, mutation-proven |
| r12 | OPEN | OPEN — unchanged, out of scope for this sweep |
| r13 | OPEN | OPEN — unchanged, out of scope for this sweep |

## Self-Check (§13.10): PASSED

- Both ACL PASS lines and the suite summaries — read from
  `gh api …/actions/jobs/89753061944/logs`, ANSI-stripped, verbatim.
- Job conclusion and runner identity — `gh api …/runs/30186873948/jobs` and the
  `Runner name` / `Machine name` lines in the job log.
- Product-tree identity — `git diff 9821ef76 2cc1a285 -- crates/ Cargo.lock Cargo.toml`,
  empty output.
- `$targets` invariant — `git diff --exit-code -- scripts/f20-native-windows-proof.ps1`, exit 0.
- Guard rejections — captured stdout of harnesses that AST-extract / `sed`-extract
  the production functions; source SHA256 and extracted line numbers printed by
  the harnesses themselves.
- Mutation results — four `node --test` runs, pass/fail counts read from the TAP
  summary, file restored from a pre-mutation copy and re-run green.
- The SSH-unavailability trap — reproduced against the CI-certified-green control
  test before any conclusion was drawn.
