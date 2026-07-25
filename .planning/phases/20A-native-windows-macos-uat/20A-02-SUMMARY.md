---
phase: 20A-native-windows-macos-uat
plan: "02"
subsystem: infra
tags: [windows, appcontainer, sandbox, retained-authority, anti-swap, share-mode, ntfs, delegated-dispatch, swarm]

requires:
  - phase: 20A-native-windows-macos-uat
    provides: "the measured four-suite baseline (135/135/0/45 · 90/83/7 · 9/5/4 · 7/3/4/3) and the attribution of all 15 failures to the delegated-backend admission refusal"
provides:
  - "The Windows AppContainer backend now binds a delegated child's working directory to the RETAINED workspace authority, under an OS-enforced pin on that directory's name"
  - "A scoped process-lifetime name lease — the pin lives inside one bound execution, so no existing open changes its share mode and no existing test changes"
  - "The disproof of the prior finding that the pin cannot be scoped (R2) and the withdrawal of the six-test security trade (R1)"
  - "An anti-swap regression proof that constructs the substitution and asserts the child operated on the retained object"
  - "The delegated-dispatch admission on Windows stops refusing: 6 of the 15 blocked tests pass, and the remaining 9 now fail for three named causes that the blocker was masking"
affects: [20A-03, 20A-04]

tech-stack:
  added: []
  patterns:
    - "Process-lifetime NAME LEASE: pin a retained directory's name for the duration of one bound execution by reopening it handle-relatively with a share mode that omits FILE_SHARE_DELETE"
    - "Pin-then-prove-then-bind ordering: acquire the pin BEFORE the pathname is used, re-prove the display path against the retained object AFTER the pin exists, and only then bind by path — leaving no residual window"
    - "Scope a share-mode narrowing to the consuming call site rather than to the opener, when the opener is shared by call sites with incompatible access needs"

key-files:
  created: []
  modified:
    - crates/wcore-sandbox/src/directory_authority_windows.rs
    - crates/wcore-sandbox/src/directory_authority.rs
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
    - crates/wcore-sandbox/src/backends/appcontainer/windows_impl/tests.rs
    - .planning/phases/20A-native-windows-macos-uat/20A-02-BIND-MECHANISM.md

key-decisions:
  - "Scoped Mechanism A to a bind-site name lease rather than narrowing an open_* function, because the narrowed-opener form was MEASURED to break RetainedWorkspaceAuthority::new with err=32"
  - "Did not override binds_workspace_authority independently — the trait derives it from binds_cwd_authority, and a second answer is how the two drift apart"
  - "Left crates/wcore-swarm/src/dispatch.rs byte-identical and gate-checked so; the admission predicate is correct and the defect was that the backend could not bind"
  - "Left bind_command_cwd's Windows PolicyNotSupported stub and its test untouched — it is a separate unused-on-Windows primitive, and changing its message would have modified an existing test"
  - "Ran a throwaway no-pin diagnosis to exonerate the pin for a residual failure rather than reasoning from the error code alone"

patterns-established:
  - "Anti-swap regression proof shape: assert the child's artifact is reachable THROUGH THE RETAINED HANDLE, never assert an OS error code, and prove the production entry point refuses when the pin cannot be established"
  - "Diagnose-by-subtraction: when a newly-reached failure appears behind a fix, temporarily remove the fix's mechanism and re-measure, rather than attributing by error code"

status: complete
---

# Phase 20A Plan 02: AppContainer Retained-Workspace-Authority Bind Summary

The Windows AppContainer backend now binds a delegated child's working directory to the retained
workspace authority under an OS-enforced pin on that directory's NAME, scoped to one bound
execution — so the anti-swap guarantee holds, no existing open changes its share mode, and the
six-test security trade Sean was about to be asked to authorize turned out to be unnecessary.

**Termination state: 1 — Complete.**

---

## 1. Fix commits and tree

| | |
|---|---|
| Base SHA | `3e3e6903` (`plan/f20-unified-audit-repair`) |
| Bind commit | `2b44add4` — `fix(sandbox): bind the AppContainer child cwd to the retained authority` |
| Clippy repair | `c252d01d` — `style(sandbox): collapse the retained-cwd guard into a let-chain` |
| Sealed SHA for every measurement below | **`c252d01d3c885ed97ec0eff9b04280f2e5756672`** |

Pushed to `gh` (`FerroxLabs/wayland-core`) on `plan/f20-unified-audit-repair`. No push to main,
no merge, no PR, no tag, no release, no issue closure.

---

## 2. The retained authority's actual open — source and probe agree

`crates/wcore-sandbox/src/directory_authority_windows.rs`:

| Opener | Access mask | Share mode | Create options |
|---|---|---|---|
| `open_directory` (mutating) | `GENERIC_READ \| GENERIC_WRITE \| DELETE` | `FILE_SHARE_READ \| FILE_SHARE_WRITE \| FILE_SHARE_DELETE` | `BACKUP_SEMANTICS \| OPEN_REPARSE_POINT` |
| `open_directory_observational` | `GENERIC_READ` | `FILE_SHARE_READ \| FILE_SHARE_WRITE \| FILE_SHARE_DELETE` | `BACKUP_SEMANTICS \| OPEN_REPARSE_POINT` |

**The authority the bind actually receives is the OBSERVATIONAL one.** The delegated checkout is
opened `DirectoryAuthority::open_observational` at `wcore-swarm/src/worktree_manager.rs:960`,
because a retained `DELETE` right denies `SetCurrentDirectory` system-wide and broke every
checkout-scoped git invocation. That distinction is what the earlier evaluation missed, and it is
what makes the scoped pin possible.

---

## 3. The mechanism — measured, then implemented

### 3.1 What the earlier pass concluded, and why it was wrong

`20A-02-BIND-MECHANISM.md` §2.7 recorded "the pin CANNOT be scoped", from a probe that acquired a
second handle against the AS-SHIPPED **mutating** authority (which holds `DELETE`). A share-
arbitrated lease cannot open alongside a `DELETE`-bearing handle whose grant the lease's share
mode refuses — so the probe was refused, correctly, for the shape it tested. **It was never run
against the observational authority the bind receives.** That was the untested option.

### 3.2 The two scoped forms, measured on SEANDESKTOP

A Rust probe compiled into `wcore-sandbox` on the box, calling the real production functions —
not PowerShell, whose `0x80000000` `UInt32` parse already produced one false result in this plan.
Full verbatim output is inline in `20A-02-BIND-MECHANISM.md` §7.

**Form 1 — a narrowed variant open (`open_observational_pinned`, share `READ|WRITE`) used ONLY
for the workspace authority. DOES NOT QUALIFY.**

| Operation | Result |
|---|---|
| external rename of the retained NAME | REFUSED `err=32` — pin holds |
| external unlink of the retained NAME | REFUSED `err=32` — pin holds |
| `CreateProcess(cwd = display_path)` | OK, exit 0; child's write visible through the RETAINED handle |
| **`RetainedWorkspaceAuthority::new`** | **FAILED `err=32`** |

Its identity re-proof calls `owner.open_child_directory(child_name)`, whose
`RelativeIntent::Mutate` arm requests `DELETE` (`directory_authority_windows.rs:805-807`). The
narrowed workspace handle's share mode refuses that open, so the constructor breaks outright.

**Form 2 — a process-lifetime NAME LEASE acquired at the bind site. QUALIFIES, and is what
shipped.**

The same Mechanism-A pin at a strictly smaller scope: a second handle on the retained object,
opened HANDLE-RELATIVELY (`RootDirectory` = the retained handle, empty `ObjectName`, so no
pathname is resolved), access `GENERIC_READ | SYNCHRONIZE`, share `FILE_SHARE_READ |
FILE_SHARE_WRITE`. This is the mechanism `bind_command_cwd`'s pre-existing error message already
named verbatim ("without a process-lifetime name lease").

| Operation | Result |
|---|---|
| lease opens against the observational checkout authority | **OPENED** |
| external rename of the bound NAME, lease held | **REFUSED `err=32`** |
| external unlink of the bound NAME, lease held | **REFUSED `err=32`** |
| second substitution attempt, lease held | **REFUSED `err=32`** |
| `RetainedWorkspaceAuthority::new`, before the lease | **OK** — the production ordering |
| `DirectoryAuthority::validate_path` under the lease | **OK** |
| `CreateProcess(cwd = display_path)` under the lease | **OK**, exit 0 |
| child's artifact read back THROUGH the retained handle | `["proof.txt"]` |
| after the lease drops: external rename | succeeds again — pin scoped to the execution |
| after the lease drops: `remove_descendants` | **OK** — destructive cleanup unaffected |

`RetainedWorkspaceAuthority::validate()` is refused under the lease, for the same `DELETE`-request
reason. It is NOT on the bound path: `SandboxRegistry::execute_with_workspace_authority` calls it
at `lib.rs:330`, before the backend receives the workspace, and the native backend never
re-invokes `reauthorize`. The two things that run DURING execution — `mirror_heartbeat` and the
`WorkspaceMonitor` scan — use `validate_execution_authority` (path-metadata `validate_path`,
measured OK), relative child reads, and read-only enumeration. None requests `DELETE` on the
pinned object.

### 3.3 The three decision-rule measurements

| | Question | Result |
|---|---|---|
| **(a)** | Is the NAME pin OS-enforced on the workspace with every other open unchanged? | **YES** — rename `err=32`, unlink `err=32`; the child provably lands in the retained object |
| **(b)** | Do all 6 previously-broken tests pass? | **YES**, all 6 by name |
| **(c)** | Does the `wcore-sandbox` suite return to baseline? | **YES** — 136/136/0/45 (baseline 135/135/0/45, +1 = the new anti-swap regression test) |

```
PASS authority_boundary_tests::buffered_authority_rejects_same_path_replacement_before_backend
PASS authority_boundary_tests::streaming_authority_rejects_same_path_replacement_before_backend
PASS directory_authority::tests::retained_parent_routes_children_after_path_replacement
PASS directory_authority::tests::windows_handle_relative_rename_stays_bound_to_target_parent
PASS directory_authority::tests::windows_handle_relative_file_publish_stays_bound_to_target_parent
PASS directory_authority::tests::windows_command_cwd_stays_bound_to_renamed_directory_object
PASS backends::appcontainer::windows_impl::tests::windows_retained_cwd_bind_survives_a_pathname_substitution
```

**No test was modified, weakened, re-gated, `#[ignore]`d, `#[allow]`ed or deleted.** The two
guards on the `RootDirectory = NULL` pathname-form rename defect stay exactly as they were and
keep passing. R1 (the six-test trade) is **withdrawn**; R2 ("the pin cannot be scoped") is
**disproved**. R3 stands: every rule was measured on this box's default NTFS volume.

---

## 4. The implementation

### 4.1 `directory_authority_windows.rs` — `acquire_name_lease`

Opens the retained object handle-relatively with `GENERIC_READ | SYNCHRONIZE` and share
`FILE_SHARE_READ | FILE_SHARE_WRITE`. The omission of `FILE_SHARE_DELETE` IS the pin: renaming or
unlinking an object requires opening it with `DELETE`, and Windows share arbitration refuses a new
open whose desired access is not permitted by every already-open handle's share mode.

`GENERIC_READ` is deliberate rather than incidental — an attributes-only open requests none of
read/write/delete, so it neither is checked against nor contributes to share arbitration, and was
MEASURED to deliver no pin at all. The measured basis is documented at the function.

It fails closed by construction: a `DELETE`-bearing authority cannot be pinned, because the
lease's share mode would have to permit the `DELETE` that handle was already granted.

### 4.2 `directory_authority.rs` — `DirectoryNameLease` and `acquire_name_lease`

An RAII guard whose LIFETIME is the pin. Deliberately **not** a `DirectoryHandleLoan`: a loan
records that a DESCENDANT holds a duplicate of the retained handle and makes terminal cleanup
refuse; this lease is held by the PARENT for one bound execution and never handed to the child,
so counting it as a loan would misreport who holds the workspace.

### 4.3 `appcontainer/windows_impl/process.rs` — `bind_retained_cwd` and the trait overrides

`bind_retained_cwd` is the single place the binding is established, in a non-rearrangeable order:

1. **Pin the name first** — before any pathname reaches `CreateProcess`.
2. **Prove the pin landed on the right name** — `validate_path` re-proves the display path still
   resolves to exactly the retained object, AFTER the pin exists. This closes the one window that
   would otherwise exist: the lease pins whatever name the object currently carries, so an object
   already renamed away would leave the display path naming a decoy.
3. **Only then bind by path.** Everything after step 2 is inside the pin, so there is **no
   residual window** — a substitution between the proof and the child's first filesystem
   operation is refused by the OS.

`execute_with_cwd_authority` holds the lease across the whole execution and refuses on any
failure; there is no unbound-spawn fallback. `binds_cwd_authority` answers true only because that
binding is real. **`binds_workspace_authority` is NOT overridden** — the trait derives it.

### 4.4 The anti-swap regression proof

`windows_retained_cwd_bind_survives_a_pathname_substitution`, in the Windows AppContainer test
module. It **always runs** — it spawns an ordinary child rather than an AppContainer one, so it is
never skipped by the live-acceptance environment gate and can never report a vacuous green
(threat T-20A-02-06).

It constructs the substitution the guarantee exists to defeat — rename the bound name away, and
unlink it so a decoy could be recreated at it — and asserts the INVARIANT: the child's artifact is
reachable **through the retained handle**, and absent from the decoy. It asserts no error code, no
error kind, no numeric OS status (threat T-20A-02-07).

It fails if the binding is downgraded, two independent ways:
- with the bind held, the substitution succeeds and the child's artifact is then absent from the
  retained object;
- part two asserts that `execute_with_cwd_authority` REFUSES an authority whose name cannot be
  pinned (a `DELETE`-bearing one) and that no child ran against the unpinned pathname — so if the
  production entry point ever stops establishing the pin, that call stops failing.

---

## 5. Per-suite delta against the measured 20A-01 baseline

All at the sealed SHA `c252d01d` on `SEANDESKTOP`.

| Suite | 20A-01 baseline | Now | Delta |
|---|---|---|---|
| `wcore-sandbox` | 135 / 135 / 0 / 45 | **136 / 136 / 0 / 45** | +1 test (the anti-swap proof), still zero failures |
| `wcore-swarm` | 90 / 83 / 7 | **90 / 87 / 3** | **4 cleared** |
| `wcore-agent --test transactional_delegated_mutation_test --run-ignored all` | 9 / 5 / 4 | **9 / 5 / 4** | count unchanged, **cause changed** |
| `wcore-swarm --test dispatch_smoke` | 7 / 3 / 4 (3 skipped) | **7 / 5 / 2 (3 skipped)** | **2 cleared** |

Workspace clippy with all targets: **clean, zero warnings.**

**Six of the fifteen now pass. Not one of the nine still failing is the admission refusal** —
`sandbox backend appcontainer cannot bind retained delegated workspace authority` no longer
appears anywhere in any of the four suites. Every residual is a distinct defect the blocker was
masking, named and diagnosed below.

### 5.1 Residual failure 1 — `parent integration checkout is dirty: M README.md` (4 tests)

`wcore-agent::transactional_delegated_mutation_test`:
`happy_path_open_accept_land_receipt_then_rollback`,
`land_selected_winner_drives_production_chain_to_landed`,
`restart_replays_landed_state_from_disk`,
`multi_candidate_only_winner_lands_loser_is_cleaned`.

All four now reach the landing chain and fail with one identical message:

```
Primitive("worktree io: parent landing: parent integration checkout is dirty: M README.md")
```

Accompanied throughout by `warning: in the working copy of 'README.md', LF will be replaced by
CRLF the next time Git touches it`. **This is the checkout-dirty / EOL reconciliation defect,
which 20A-03 owns by the plan's own scope fence.** Not touched here. The count is unchanged from
baseline but the cause is not: they have progressed from "refused at admission" to "blocked by the
20A-03 defect".

### 5.2 Residual failure 2 — `bash` is unsupported under the AppContainer sandbox (1 test)

`wcore-swarm::dispatch_smoke::required_live_windows_public_dispatch_bash_confines_parent_and_descendants`.
The worker argv is `bash`, and the backend refuses it structurally:

```
argv[0] "bash": this shell is not supported under the Windows AppContainer sandbox.
git-bash requires msys-2.0.dll from C:\Program Files\Git, and even static busybox-w32
links network/auth/UI DLLs (Secur32, WS2_32, bcrypt, USER32) that cannot load under the
sandbox's Low-integrity restricted token (STATUS_DLL_NOT_FOUND 0xC0000135).
```

That refusal is pre-existing, documented and intentional. The test never reaches a spawn, so the
cwd binding is not involved. It is a shell-support question, not a bind question, and is a
separate front from this plan.

### 5.3 Residual failure 3 — Low-IL `canonicalize` denial in the child (1 test)

`wcore-swarm::dispatch_smoke::public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state`.
The worker fixture now RUNS and panics inside the child:

```
dispatch_smoke.rs:505:68: Os { code: 5, kind: PermissionDenied, message: "Access is denied." }
```

Line 505 is `std::env::current_dir().unwrap().canonicalize().unwrap()`.

**The pin was exonerated by measurement, not by reasoning.** A throwaway diagnosis patch released
the lease immediately before the spawn and the failure was byte-identical — same file, same line,
same `code: 5`. Share-arbitration refusals in every measurement in this plan are `err=32`
(`ERROR_SHARING_VIOLATION`); `code: 5` is `ERROR_ACCESS_DENIED` from the AppContainer's
Low-integrity restricted token. The diagnosis patch was reverted; the box's tree is clean.

### 5.4 Residual failure 4 — 20-second wall-clock budget exceeded (1 test)

`wcore-swarm::worker_runtime_limits::multi_worker_output_exhaustion_fails_without_retaining_buffers`
asserts `started.elapsed() < Duration::from_secs(20)` around a five-worker dispatch. At baseline
that dispatch was refused at admission and returned instantly; it now really executes five
AppContainer children, and the run took ~35 s. **The timeout was NOT raised** — doing so is
explicitly forbidden and would convert a real Windows performance red into a fake green. It is an
honest red on the newly-reached execution path.

### 5.5 Explicitly NOT investigated

The 30 Windows failures F-09 unmasked — in particular the 18 `session_journal` ones
(`session_journal_test` 10, `session_journal_compaction_test` 6,
`session_journal_crash_matrix_test` 2) — are recorded and untouched. They had never run on Windows
because clippy blocked the CI test step; they are a new, un-baselined front and are out of scope
for this plan. They were not investigated and not fixed.

---

## 6. REQ-native-r6 portability clause — already satisfied at this SHA

REQ-native-r6 reads "`dispatch_smoke` Windows-portable (no `fs::rename` of open dir)". Every
`fs::rename` in `crates/wcore-swarm/tests/dispatch_smoke.rs` was inspected:

| Site | Disposition |
|---|---|
| `:301`, `:327` | already inside `if cfg!(windows)` arms that call `assert_rename_refused_by_open_descendant` — they assert the OS refusal rather than performing the rename |
| `:363` (`replace_repo_container`) | documented UNIX-ONLY, reached only from the non-Windows arm |
| `:386` (`assert_rename_refused_by_open_descendant`) | asserts the refusal; it is the Windows arm |
| `:442` | renames the repository OBJECT itself under a topology with NO descendant handle held. Permitted on Windows by the measured rename rule, and the test asserts it succeeds |

**No non-portable construction remains.** The portability clause was already closed at this SHA,
so nothing was changed — inventing a change to justify the requirement is explicitly forbidden.
This bind does not affect it: no `open_*` share mode changed, so the `:442` rename still succeeds.

---

## 7. Linux non-regression (REQ-native-r4)

`cargo build --locked --workspace --all-features` plus `cargo nextest run --profile ci
--no-fail-fast` on `hetzner-dsm` at `/root/wayland`, detached at the same sealed SHA
`c252d01d3c885ed97ec0eff9b04280f2e5756672`.

```
c252d01d3c885ed97ec0eff9b04280f2e5756672
NEXTEST_EXIT=0
     Summary [ 190.922s] 11519 tests run: 11519 passed (1 slow, 2 flaky), 48 skipped
```

`cargo build --locked --workspace --all-features` finished clean. **11519 / 11519 passed, 0
failed, 48 skipped — no Linux regression.** That matches the `Linux 11509/0` figure REQ-native-r4
carries, plus the ten tests added on this branch since. Two tests were flaky-on-retry
(`worktree::tests::linux::status_output_cap_kills_git_descendant`,
`deterministic_openai_loop::packaged_core_cancels_an_active_stream`) and both are pre-existing
Linux flakes unrelated to this change — neither touches the sandbox authority surface.

The Windows binding is entirely inside `#[cfg(windows)]` surface: `acquire_name_lease` lives in
`directory_authority_windows.rs`, `DirectoryNameLease` and `DirectoryAuthority::
acquire_name_lease` are `#[cfg(windows)]`, and the AppContainer backend is a Windows-only module.
No Linux or macOS code path was touched.

---

## 8. Gate checks

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | clean |
| `crates/wcore-swarm/src/dispatch.rs` diff | **empty** across both commits — untouched |
| `cfg(windows)` count in `dispatch.rs` | unchanged from the pre-task tree |
| `wcore-swarm` platform conditionals added | none |
| `scripts/f20-native-windows-proof.ps1` `git diff --exit-code` | **no diff — `$targets` byte-identical** |
| `binds_cwd_authority` present in `process.rs` | yes |
| `execute_with_cwd_authority` present in `process.rs` | yes |
| `binds_workspace_authority` overridden | **no** — derives from the trait |
| probes reaching production code | none; both hosts' trees clean after every probe |
| `AGENTS.md` / `.ijfw` staged | never |
| `Co-Authored-By` trailers | none |

Repair iterations used: **1 of 2** (the `clippy::collapsible_if` fix in `c252d01d`).

---

## 9. Recorded unknowns

- **NTFS-local.** Every share-arbitration and rename rule here was measured on SEANDESKTOP's
  default NTFS volume. ReFS, FAT and SMB workspaces are unproven, and a filesystem that does not
  implement Windows share arbitration would not deliver the pin.
- **AppContainer handle-inheritance and integrity behaviour across Windows builds** is unmeasured
  beyond this box. §5.3's `code: 5` denial is one instance of that surface.
- **Reachability.** There is no residual window to be reached — the pin is established before the
  pathname is used and the display path is re-proven after the pin exists — but the pre-bind
  interval, between `open_observational` at workspace materialization and the lease at execution,
  is covered by `validate_path` rather than by the OS. A substitution in that interval is
  DETECTED and fails closed; it is not prevented.
- **`bind_command_cwd`'s Windows stub is now understated.** It still returns
  `PolicyNotSupported("... without a process-lifetime name lease")`, and the lease now exists. It
  was left alone deliberately: it is a separate primitive with no Windows consumer, and changing
  its message would have modified the existing test at `directory_authority_windows_tests.rs:627`.

---

## 10. Termination state

**State 1 — Complete.** A mechanism was measured, implemented once, and proven on hardware to bind
the retained authority with the anti-swap property intact and no residual window, with the delta
stated against the 20A-01 baseline. No requirement is marked complete; closure is claimed by the
downstream native proof under 20A-04.
