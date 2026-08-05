---
phase: 20-transactional-delegated-mutation
plan: "75"
type: execute
status: complete-with-reported-blockers
source_sha: c39f72549e524ca896d144fd79db13dd67001533
baseline_sha: 2353aa9d504be93f2806ad0e00eee6827b9e7f3e
proof_host: SEANDESKTOP (C:\ferrox-win)
requirements:
  - REQ-native-r4
  - REQ-native-r6
  - REQ-native-r12
requirements_completed: []
---

# Phase 20 Plan 75: Native Windows Closeout Summary

Repaired four Windows production defects in the retained-handle core — a rename
primitive that had never worked, a cleanup access mask that could not serve both
child kinds, a read-only-handle flush, and a journal delete without DELETE — plus
the landing-path chdir defect and both live-process quoting defects. Took
`wcore-sandbox` from 6 failures to **ZERO across two consecutive runs**. Did NOT
reach the plan's zero-failure target on `wcore-swarm` or 9/9 on the transactional
suite: both are blocked by newly-surfaced, independently-measured defects that
lie outside this plan's four workstreams and require design decisions.

**A reported red, with evidence, in preference to an engineered green.**

## Re-measured baseline at `2353aa9d`

| Suite | Measured | Plan asserted |
|---|---|---|
| `wcore-sandbox` | 132 run / 126 passed / 6 failed / 45 skipped | 133/127/6/45 |
| `wcore-swarm` | 88 / 79 / 9 / 6 | identical |
| `transactional_delegated_mutation_test --run-ignored all` | 9 / 3 / 6 | identical |
| `dispatch_smoke` | 7 / 3 / 4 / 3 | identical |

Zero stack-overflow aborts — 20-74's stack repair has not regressed. The
`wcore-sandbox` run/passed counts differ from the plan's by exactly 1; the
FAILURE set and skip count are identical, so the delta is not material and every
failing name bucketed cleanly.

Bucketing (all measured, none assumed):
- **A (err-87)** — `concurrent_atomic_write_exposes_only_whole_old_or_new_payloads`,
  `windows_handle_relative_rename_stays_bound_to_target_parent`,
  `crash_after_descendant_removal_recovers_original_before_reads`,
  `mid_import_failure_rolls_back_to_original_tree`. The plan predicted
  `relative_creation_stays_bound_to_renamed_parent_object`; the two archive tests
  are the real members.
- **D (live process)** — `required_live_job_teardown_precedes_workspace_cleanup`
  and, newly named from the run, `backends::no_sandbox::tests::dropping_stream_reaps_windows_job_descendant`.
- **B (os-5)** — all 9 `wcore-swarm` failures.
- **C (chdir)** — all 6 agent failures, `git ["rev-parse","--is-inside-work-tree"]`
  during **parent landing**.

## Dependency confirmation

`NtSetInformationFile` (ntdll link, gated `Win32_System_IO` — enabled),
`FileRenameInformation = 10i32`, and `FILE_RENAME_INFORMATION { Anonymous,
RootDirectory, FileNameLength, FileName }` are all present in the pinned
`windows-sys 0.59.0` under `Wdk::Storage::FileSystem` with the crate's existing
features. Field order exactly as the plan stated. **No fallback needed**; no
dependency, feature or `Cargo.toml` change was made.

## Measurement protocol

`cmd` trailing-space trap demonstrated verbatim (delayed expansion, script file
to avoid ssh quote mangling):

```
UNQUOTED=[8388608 ]     <- set VAR=x && ...   trailing space
QUOTED=[8388608]        <- set "VAR=x" && ... flush
```

`RUST_BACKTRACE` was set via PowerShell `$env:` and proven effective by observing
actual backtraces. `/usr/bin/grep` used for every gate.

## Workstream A — the rename primitive (commit `66f1abaa`)

Replaced `SetFileInformationByHandle`/`FileRenameInfo` with
`NtSetInformationFile`. The destination remains named ONLY by the retained parent
handle; the `RootDirectory = NULL` + full-pathname form is recorded as FORBIDDEN
at the call with the anti-swap property it would destroy.

**Repairing os-87 unmasked a second production defect, measured here.** The
CLASSIC rename class cannot replace a destination another handle has open, even
one opened with `FILE_SHARE_DELETE`. Measured: replace over an *unopened*
destination succeeds; over a destination a concurrent reader holds open it fails
with os error 5, deterministically (10/10 in isolation). The polled heartbeat
mirror publishes exactly such a file. Fixed with `FileRenameInformationEx` +
`FILE_RENAME_POSIX_SEMANTICS` — matching unix `renameat` semantics — falling back
to the classic class exactly as the sibling `mark_open_object_for_delete` falls
back from `FileDispositionInfoEx`. **`RootDirectory` is identical under both
classes, so the anti-swap guarantee is byte-for-byte unchanged.**

STATUS_PENDING refused explicitly. Buffer sizing and `usize` alignment preserved.

**Anti-swap proof:** `windows_handle_relative_rename_stays_bound_to_target_parent`
passes UNMODIFIED. A new test,
`windows_handle_relative_file_publish_stays_bound_to_target_parent`, proves the
same held-handle property for the FILE publish path the production status mirror
uses (target pathname renamed away, decoy recreated, publish lands in the MOVED
object).

**PRODUCTION DURABILITY DEFECT.** `atomic_write_child` was non-functional on
Windows for the life of the port, silently disabling the heartbeat status mirror
(`dispatch.rs::mirror_heartbeat`), the swarm directory-rename API and the archive
rollback path. Nothing caught it because `heartbeat_test.rs`'s only status-file
probe is reachable solely from a `#[cfg(unix)]` test. New, un-gated
`heartbeat_mirror_publishes_through_a_retained_root_authority` drives the exact
production shape and the REPEATED publish the polled mirror performs. **This is a
production durability defect, not test debt.**

`rename_into` in `worktree_security.rs` PRESERVED with an in-source note.

## Workstream B — precise cleanup kind (commit `d3cc5b21`)

`FileAttributes` carried out of `parse_directory_entries` (offset 56, strictly
below `FileName` at 104 — the existing `remaining < header` guard already proves
the read in bounds; that argument is recorded in-source). No guard relaxed,
reordered or removed. `child_names` became the name-only projection of a single
enumeration loop.

Rights table after: `(File, Mutate) -> FILE_GENERIC_READ | DELETE | SYNCHRONIZE`
(unchanged in value, absence of the write bit now load-bearing);
`(Directory, Create|Mutate)` unchanged; the unknown-kind arm eliminated. Kernel
now enforces the child type at open via `FILE_DIRECTORY_FILE` /
`FILE_NON_DIRECTORY_FILE` — additive; the post-open type check, identity read and
reparse refusal all survive and all symlink/reparse refusal tests still pass.

Two new Windows-only regressions:
`windows_destructive_removal_succeeds_through_a_read_only_file_child` (the case
the original diagnosis missed) and `..._through_a_directory_child`. Parser fixture
now writes the attribute field explicitly; all pre-existing parser proofs pass.

Nothing observable changed on unix: portable `child_names` signature untouched,
unix arms untouched, `wcore-swarm` gained no `#[cfg]`.

### Two further production defects, unmasked by A and root-caused rather than filed as pre-existing

1. **`set_child_mode` flushed a read-only handle** (commit `9adb3aad`).
   `open_child_file` opens `ReadOnly` (no write bit) and `sync()` →
   `FlushFileBuffers` is refused with os error 5. Off unix the function applies
   nothing (`file_mode` fabricates a constant `0o600`) and the bytes already
   reached disk via `atomic_write_child`. Restricted to unix. This is the crate's
   only `sync()` on a read-only-opened regular file.
2. **The import journal was deleted through a handle without DELETE**
   (commit `255e88db`). Unix unlinks via the PARENT descriptor; Windows sets a
   disposition on the FILE's handle. `recover_pending_import` could never consume
   its journal on Windows, so every later import refused with "already active or
   requires exclusive recovery" — **crash recovery could not complete**. Added
   `open_child_file_for_removal` (read + delete, still no write bit) and made
   `remove_child_file`'s access requirement explicit.

## Workstream C — the landing chdir defect (commit `ba32567c`)

**Live handle enumeration, not inference.** Sysinternals `handle.exe` is NOT
installed on the box (checked and reported), so an `NtQueryObject`-based
own-process enumeration probe was written, run in the REAL landing flow at the
moment of failure, and then reverted.

```
PROBE[C BEFORE DirectoryAuthority::open] total handles on target: 0
PROBE[C AFTER  DirectoryAuthority::open] handle 0x318 granted=0x0013019f
  [DELETE|READ_CONTROL|SYNCHRONIZE|FILE_READ_DATA|FILE_WRITE_DATA|
   FILE_APPEND_DATA|FILE_READ_EA|FILE_WRITE_EA|FILE_READ_ATTRIBUTES|
   FILE_WRITE_ATTRIBUTES]
PROBE[C AFTER  DirectoryAuthority::open] total handles on target: 1
PROBE[C bind_integration_checkout pre-git] total handles on target: 1  (same handle)
```

Zero before, exactly one after, same single handle at the failing git call.
`bind_integration_checkout`'s `DirectoryAuthority::open(&root)` is therefore the
SOLE handle, and **every other swept candidate was ruled out BY MEASUREMENT** —
`RetainedWorkspaceAuthority::new`/`::validate`, the objects-dir authorities, the
transaction-root/swarm/control/quarantine authorities, the recursive
`open_child_directory` walks, every `to_sandbox()`/`try_clone_handle()` loan and
the agent-side spawner paths.

**Consumer classification:** `IntegrationCheckout::authority` has exactly ONE
consumer in the crate — `assert_parent_unchanged`'s `validate_path`. Identity
witness only. Branch (i) taken: `open_observational`. **Anti-swap unchanged** —
the handle is acquired at the same point and held CONTINUOUSLY; no
release/re-acquire window. Stale field doc corrected. 20-74's two markers
untouched. Regression test
`landing_git_chdir_succeeds_while_the_integration_checkout_authority_is_alive`
added as the third member of the 20-72/20-74 family.

**The chdir class is fully eliminated.**

## Workstream D — live-process quoting (commits `6197b324`, `482214a0`)

Both named and root-caused at what each shell layer actually received.

`required_live_job_teardown_precedes_workspace_cleanup`: embedded an absolute
path in an already-quoted argument with doubled quotes and passed it through
`Command::arg`, which layers std's `CommandLineToArgvW` quoting on top of the cmd
escaping. Repaired by REMOVING the nesting — child cwd set to the temp directory,
bare relative marker name, line handed to cmd via `raw_arg` — and by adopting the
sibling file's measured idiom (`start ""`, `/b`, `/d`, `/s`, bare `for /L`
builtin rather than an external exe). All assertions unchanged.

`dropping_stream_reaps_windows_job_descendant`: the nested line carried two
quoted absolute paths whose inner quotes std escaped as `\"`, so the inner shell
never launched. Repaired with `cwd` + bare relative names; **two process levels
retained** because the reaped descendant IS the inner shell.

Four err-87 tests confirmed individually by name (not by aggregate).

## Workstream E — lint gate (commits `31604799`, `54efdc63`, `b84394fc`, `c39f7254`)

`wcore-sandbox` and the named `wcore-swarm` findings are clean. Every fix at its
cause; **no blanket suppression added, and no narrowly-scoped suppression added
either — the count is zero.**

- `RelativeKind::Any` REMOVED. Workstream B deleted its only constructor, so
  clippy correctly reported it dead. Deletion is strictly stronger than the
  runtime refusal the plan specified: a future caller must now add it back and
  answer the rights question at COMPILE time. **Deviation from the plan's letter,
  taken to serve its intent.**
- Two unreachable platform arms and two needless returns converted to the
  block-expression form the same files already use.
- Two parser modulo guards took `is_multiple_of`; both are "not divisible" checks
  and both keep their negation exactly (`a % b != 0` -> `!a.is_multiple_of(b)`).
- Four unsafe operations in the nested `unsafe fn` given explicit blocks with
  SAFETY comments; the fn's pointer precondition documented.
- Dead-code findings investigated ACROSS CONFIGURATIONS and gated, never deleted:
  `set_ambient_git_env` -> `cfg(all(test, target_os = "linux"))` (all four callers
  in `worktree_tests/linux.rs`); the `Arc` import in `worker_runtime_limits.rs` ->
  `cfg(any(linux, macos))` (**the plan's gate expected deletion; deleting would
  break both other platforms**); `export_tar_bounded` and
  `replace_from_tar_bounded` -> `cfg(feature = "live-docker")` (both call sites
  are inside `live-docker` regions).
- The Linux gating could NOT be compile-verified: neither the Mac nor the Windows
  box builds Linux. The gates mirror their callers' cfg exactly — a mechanical
  correspondence, stated as such and not as a compile proof.

## Final measurement on `c39f7254` (SEANDESKTOP)

| Suite | Baseline | Final | Delta |
|---|---|---|---|
| `wcore-sandbox` | 132/126/**6**/45 | 135/**135**/**0**/45 (x2 consecutive) | **-6, ZERO** |
| `wcore-swarm` | 88/79/**9**/6 | 90/83/**7**/6 | -2 |
| transactional suite | 9/3/**6** | 9/**5**/**4** | -2 |
| `dispatch_smoke` | 7/3/**4**/3 | 7/3/**4**/3 | 0 |

`cargo check -p wcore-sandbox -p wcore-swarm -p wcore-agent --all-targets`: clean.
`cargo fmt --all -- --check` on the MAC: clean (cannot run on the box — os error
206 on this 54-crate workspace; the justfile carries the `[windows]` override).

## BLOCKERS — why the zero-failure target was not reached

### 1. AppContainer cannot bind a retained workspace authority (7 `wcore-swarm`, 4 `dispatch_smoke`)

All remaining swarm failures now share ONE error:
`sandbox backend appcontainer cannot bind retained delegated workspace authority`.

**This is pre-existing and structural, not caused by any change here.** Verified:
`git diff 2353aa9d..HEAD -- crates/wcore-sandbox/src/backends/ crates/wcore-swarm/src/dispatch.rs`
is EMPTY. At `2353aa9d` the Windows AppContainer backend already overrode only
`owns_descendants_hard` and `enforces_read_deny`; it has never overridden
`binds_cwd_authority`, and it does not implement `execute_with_cwd_authority`
(the default fails closed with `PolicyNotSupported`). So
`admit_delegated_backend` has ALWAYS refused it.

**The plan's assertion that all 9 swarm failures share ONE root cause (workstream
B) is falsified by measurement.** They shared a MASKING cause: the os-5 cleanup
failure was the reported worker status. With cleanup repaired, the true admission
refusal surfaces underneath.

Closing this requires implementing retained-cwd/workspace-authority binding in
the Windows AppContainer backend — a new backend capability (Win32
`CreateProcess` takes `lpCurrentDirectory` as a pathname, not a handle, so this
needs the name-lease machinery). That is an architectural change outside all four
of this plan's workstreams. **Not attempted. Escalated.**

### 2. The landing reads every normally-created Windows checkout as dirty (4 agent tests)

Remaining agent error: `parent integration checkout is dirty: M README.md`.

Measured, single-variable, on the box:

```
source worktree CR count:                                    0
cloned worktree CR count (ambient autocrlf=true):            1
clone status AMBIENT (autocrlf=true):                        (clean)
clone status UNDER LANDING SCRUB (autocrlf=false):            M README.md
```

Git for Windows ships `core.autocrlf=true` in the SYSTEM gitconfig
(`C:/Program Files/Git/etc/gitconfig`, confirmed on the box). Any normally-created
Windows checkout therefore has a CRLF working tree against an LF index. The
landing deliberately scrubs ambient configuration (`GIT_CONFIG_NOSYSTEM=1`, empty
system/global config, `-c core.autocrlf=false`) — a correct security posture — and
consequently reads every text file as modified and refuses to land.

**This is a real production defect on Windows, not a test-fixture artifact:** it
would fire for any real user whose integration checkout was created by ordinary
Git for Windows. Reconciling the security scrub with eol-normalization semantics
is a design decision with security implications. **Not attempted. Escalated.**

### 3. Residual lint findings outside this plan's file list

`cargo clippy --workspace --all-targets -- -D warnings` is not yet clean. Remaining:

| Crate | Finding | Location |
|---|---|---|
| wcore-swarm | `rename_into` is never used | `worktree_security.rs:105` |
| wcore-tools | (1 finding) | `vision_tools.rs:272` |
| wcore-eval-scenarios | unused var `executable` | `process_tree.rs:152` |
| wcore-eval-scenarios | unused var `cwd` | `process_tree.rs:173` |
| wcore-eval-scenarios | `authoritative_required` never used | `process_tree.rs:458` |
| wcore-eval-scenarios | collapsible `if` | `process_tree.rs:626` |
| wcore-eval-scenarios | needless `return` | `child_env.rs:209` |
| wcore-eval-scenarios | `drop` of non-`Drop` value (x2) | `runner.rs:370,393` |

`wcore-tools` and `wcore-eval-scenarios` are NOT in this plan's `files_modified`
and are unrelated to the native Windows repair; clippy has additionally not yet
reached crates downstream of them, so the true remaining set is larger than this
table. The plan's E inventory was measured before these crates were reachable.

`rename_into` is the one the plan explicitly forbids deleting, gating away or
allowing. It still has no production caller. Per the plan's own instruction to
"say where a caller belongs rather than delete the callee": the repaired surface
is a handle-relative directory rename, whose natural swarm consumer is the
candidate-promotion / quarantine-promotion path in `worktree/parent.rs`, which
currently promotes by pathname. Wiring that is a behavioural change and was not
attempted here.

## Recorded unknowns

Every probe ran on NTFS under the box's temp directory on the system drive:
ReFS/FAT/SMB behaviour of the rename primitive and the disposition fallback is
untested. `FILE_RENAME_POSIX_SEMANTICS` requires Windows 10 1709+; the classic
fallback path is present but was not exercised on an older host. The
traverse-bypass privilege underpinning the observational profile is established
only for the tokens measured. The read-only-child rights impossibility was
measured for the file ATTRIBUTE specifically, not the full space of restrictive
ACLs a hostile checkout could carry.

## CI status

`gh api repos/FerroxLabs/wayland-core/branches/plan%2Ff20-unified-audit-repair`
returns **404 — branch not found**. `gh run list --branch plan/f20-unified-audit-repair`
returns **no runs**. No Windows CI has ever executed any of these commits. Work
commits were pushed only as `f20-native-uat-<sha>` refs to move them to the proof box.

## Prepared, UNFIRED native re-dispatch (Sean-only gate, PRD D6)

The closeout brief asks for the re-dispatch; PRD D6 reserves the trigger.
Resolved the only way that honours both — prepared and presented, NOT fired.

```bash
gh auth switch --user FerroxLabs
gh workflow run nightly-windows-soak.yml \
  --ref f20-native-uat-c39f72549e524ca896d144fd79db13dd67001533 \
  -f f20_candidate=true \
  -f f20_request_nonce=c39f72549e524ca896d144fd79db13dd67001533 \
  -f f20_macos_runner_label=""
```

The plan branch has never been pushed, so the ref is the pushed UAT branch that
carries the final commit.

Six targets in `scripts/f20-native-windows-proof.ps1`. Three are already green and
**must REMAIN green** as an acceptance condition — a re-dispatch that turns any
of them red is a regression this plan owns:
`windows-retained-handle`, `windows-appcontainer-acl`, `windows-job-object`.

**Firing it now would be expected to FAIL** on `windows-public-dispatch`
(`dispatch_smoke`, blocker 1) and `windows-f20-lifecycle`
(`transactional_delegated_mutation_test`, blockers 1 and 2). Recommend resolving
both escalated blockers before dispatch.

## No Phase 20 requirement marked complete.
