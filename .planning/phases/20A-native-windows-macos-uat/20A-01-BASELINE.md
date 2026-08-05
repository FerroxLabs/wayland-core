# 20A-01 BASELINE — the measured native Windows/macOS baseline for Phase 20A

Every table below is bound to one exact SHA. Nothing here is inherited, predicted
or inferred; each row states the machine it was measured on and the exact command
that produced it.

This document MEASURES and RECORDS. It repairs nothing behavioural. Findings are
classified and routed, never fixed here.

---

## 1. The pinned SHA

| Field | Value |
|---|---|
| Work branch | `plan/f20-unified-audit-repair` |
| **Probe SHA** (where the compile question was first asked) | `b334d91701ce97d7a031d215430df27a1951489b` |
| Probe SHA tree | `591ebbf7d6ff43ac92d9274d2f34abd69de62a4d` |
| **pinned SHA** (the measurement SHA — probe SHA + the one sanctioned compile repair) | `2a9d47ff` (`fix(tools): cfg-gate the unix-only delegated-mutation live test`) |
| Phase 20 close | `01a5b0ae` (green, CLOSED — untouched by this phase) |
| Phase base | `70ccd708` |

`git diff --stat 70ccd708 b334d917 -- crates/ .github/ scripts/ justfile .config/` is
**empty** — the two 20A planning commits touch `.planning/` only, so the probe SHA's
code tree is byte-identical to the phase base's. The compile verdict below therefore
holds for the phase base as well.

### 1.1 The Windows host's ACTUAL prior SHA — the record discrepancy, resolved

The plan flagged that `.planning/TEST-AUDIT.md` (§1.1) records `C:\ferrox-win` at
`ce9a11a6` while this session's measurements were taken at `c39f7254`, and instructed
the executor to record what the box prints rather than assume either value.

**The box printed `ce9a11a6a8f62b7214f443d1a6a174a3af1c48fb`** — `docs(20-75): record
the native Windows closeout and its two blockers`. TEST-AUDIT was right about the box.

The two records do **not** actually conflict on substance:

```
$ /usr/bin/git merge-base --is-ancestor c39f7254 ce9a11a6   -> YES
$ /usr/bin/git diff --stat c39f7254 ce9a11a6 -- crates/ .github/ scripts/ justfile .config/
  (empty)
```

`c39f7254` (`style(swarm): drop the needless return in the reparse predicate`) and
`ce9a11a6` have **identical code trees**. The measurements attributed to `c39f7254`
were therefore taken on exactly the tree the box was standing on. The apparent
disagreement was a labelling difference, not a measurement hazard.

Correspondingly, the plan attributed the `.config/nextest.toml` + five-fixture delta to
`c39f7254 → 70ccd708`. Measured, it is the `ce9a11a6 → b334d917` delta, and it is
exactly what the plan predicted:

```
$ /usr/bin/git diff --stat ce9a11a6 b334d917 -- crates/ .github/ scripts/ justfile .config/
 .config/nextest.toml                                          | 32 ++++++++++
 .../v1/adversarial/events/fixture-mismatch.jsonl              |  2 +-
 .../v1/adversarial/events/schema-mismatch.jsonl               |  2 +-
 .../v1/adversarial/events/version-mismatch.jsonl              |  2 +-
 .../contracts/desktop/v1/events/ready.json                    |  2 +-
 .../contracts/desktop/v1/manifest.json                        |  2 +-
 6 files changed, 37 insertions(+), 5 deletions(-)
```

**No `crates/**/*.rs` change.** The prediction's premise holds.

### 1.2 Pristine-tree confirmation (REQ-native-r15)

The box was **NOT** pristine when found. Recorded before it was touched:

```
$ cmd /c "git status --porcelain --untracked-files=all"
?? crates/wcore-swarm/.swarm-status.json
```

One untracked file — a `wcore-swarm` run artifact left behind by a prior measurement.
It was removed **by exact path** (`Remove-Item -Force`, never `git clean`, which in a
worktree deletes branch-committed files), and the tree re-checked:

```
$ cmd /c "git status --porcelain --untracked-files=all"
(empty)
```

The box was then fetched and detached onto the probe SHA:

```
$ cmd /c "git fetch origin plan/f20-unified-audit-repair"   -> b334d917...
$ cmd /c "git checkout --detach b334d91701ce97d7a031d215430df27a1951489b"
$ cmd /c "git rev-parse HEAD"        -> b334d91701ce97d7a031d215430df27a1951489b
$ cmd /c "git rev-parse HEAD^^{tree}"-> 591ebbf7d6ff43ac92d9274d2f34abd69de62a4d
$ cmd /c "git status --porcelain --untracked-files=all"  -> (empty)
```

Two mechanical notes for anyone repeating this, both of which cost a round-trip here:

- `git fetch --all --prune` on the box printed nothing and did **not** bring the branch
  down; `git fetch origin plan/f20-unified-audit-repair` did. Fetch the branch by name.
- Inside `cmd /c`, `^` is the escape character, so `HEAD^{tree}` arrives at git as
  `HEAD{tree}` and fails. Use `HEAD^^{tree}`.

---

## 2. COMPILE VERDICT — the unverified precondition, now settled

The audit's top "could not determine" item: nobody had confirmed that the 155
Windows-only and 23 macOS-only test bodies COMPILE. This is the precondition for every
claim about those 178 tests, and it is the exact failure mode that hid 133
`wcore-sandbox` tests for two weeks.

### 2.1 Windows — measured on SEANDESKTOP (`C:\ferrox-win`), real hardware

Command (test targets included — a check that omits them proves nothing about them):

```
cargo build --locked --workspace --all-targets
```

At the probe SHA `b334d917`: **FAILED — exit 101.**

`--locked` itself was satisfied (no lockfile-inconsistency error; the failure is a
type error in a test body, not a `Cargo.lock` refusal). REQ-native-r10 is met.

```
error[E0433]: cannot find `unix` in `os`
   --> crates\wcore-tools\tests\bash_sandbox_routing_test.rs:377:18
    |
377 |     use std::os::unix::fs::symlink;
    |                  ^^^^ could not find `unix` in `os`
    |
note: found an item that was configured out
   --> library\std\src\os\mod.rs:29:4
note: found an item that was configured out
   --> library\std\src\os\mod.rs:84:40

error: could not compile `wcore-tools` (test "bash_sandbox_routing_test") due to 1 previous error
```

**F-01 — see §4.** One error, one crate, one file.

`cargo build` stops scheduling new units after the first failure, so this ONE error
was masking others. After the F-01 repair the build was re-run at the pinned SHA
`2a9d47ff`, and then re-run again with `--keep-going` to enumerate EVERY failing unit
rather than discovering them one round-trip at a time.

#### 2.1.1 The complete Windows failure set at `2a9d47ff`

```
cargo build --locked --workspace --all-targets --keep-going    -> exit 101
```

Exactly **two** compilation units still fail. Every other unit in the workspace built.

| # | Unit | Errors | ID |
|---|---|---|---|
| 1 | `wcore-eval-scenarios` (test `runner_contracts`) | 2× E0432 + 1× **E0277** | F-02, **F-03** |
| 2 | `wcore-skills` (lib test) | 1× E0432 | F-04 |

#### 2.1.2 Per-crate verdict for the two crates Task 1 names — BOTH GREEN

Proven affirmatively, not by absence from the failure list:

```
$ cargo build --locked -p wcore-sandbox --all-targets   -> == SANDBOX_EXIT=0 ==  (no errors)
$ cargo build --locked -p wcore-agent   --all-targets   -> == AGENT_EXIT=0 ==    (no errors)
```

| Crate | Windows-only tests it carries | Compile verdict at `2a9d47ff` |
|---|---|---|
| **`wcore-sandbox`** | **105 of the 155** — the appcontainer `windows_impl` module (41), the retained-handle module mounted by path from `directory_authority.rs` (22), `live_fs_acl.rs` (12), the `acl_lease` modules (16), `hard_process_containment_windows.rs` (6), `live_integrity.rs` (5) | **COMPILES — exit 0** |
| **`wcore-agent`** (REQ-native-r3, r14) | the `session_journal` and related Windows-only bodies | **COMPILES — exit 0** |

**This is the headline positive result of the plan.** The audit's top "could not
determine" item asked whether the 155 Windows-only bodies compile. For the 105 that
matter most — every retained-handle security proof, every ACL boundary test, the whole
Job-Object containment surface — the answer is **YES**, from the Windows compiler's own
output, at an exact SHA, on real hardware. The recorded 2026-07-22 `wcore-agent` Windows
COMPILE defect (REQ-native-r14) is **re-proven fixed on hardware**, not asserted from
source.

The four defects below are in three OTHER crates and none of them is in
`wcore-sandbox` or `wcore-agent`.

### 2.2 macOS — obtained from CI (the Mac cannot compile this workspace)

CI run `30151510189` (head `95552f64`), the branch's first run ever. See §5.1 for the
outcome and §5.1.2 for what could NOT be obtained.

### 2.3 F-09 — the Windows CI leg cannot even reach the test step: Clippy fails first

Independent confirmation of F-05, from the CI side, on a fresh checkout of the
self-hosted box — and it found something the local `cargo build` probe could not.

`CI (Array)` (the `[self-hosted, Windows, X64, msvc]` leg) **FAILED at step 11,
`Clippy (warnings = errors)`** = `cargo clippy --workspace --all-targets -- -D warnings`.
Steps 12-16 — including `Run tests (nextest CI profile)` — were **skipped**.

```
error: could not compile `wcore-eval-scenarios` (lib)      due to 7 previous errors
error: could not compile `wcore-swarm` (lib)               due to 1 previous error
error: could not compile `wcore-swarm` (lib test)          due to 1 previous error
error: could not compile `wcore-tools` (lib)               due to 1 previous error
error: could not compile `wcore-tools` (lib test)          due to 1 previous error
error: Recipe `lint` failed on line 76 with exit code 1
```

The eleven underlying lints, all in `#[cfg(windows)]`-gated code and therefore invisible
to the Linux-only green suite:

| Lint | Site |
|---|---|
| `unused variable: executable` | `wcore-eval-scenarios/src/process_tree.rs:152` |
| `unused variable: cwd` | `wcore-eval-scenarios/src/process_tree.rs:173` |
| `function authoritative_required is never used` | `wcore-eval-scenarios/src/process_tree.rs:458` |
| `this if statement can be collapsed` | `wcore-eval-scenarios/src/process_tree.rs:626` |
| `unneeded return statement` | `wcore-eval-scenarios/src/child_env.rs:209` |
| `call to std::mem::drop with a value that does not implement Drop` ×2 | `wcore-eval-scenarios/src/runner.rs:370, :393` |
| `method rename_into is never used` | `wcore-swarm/src/worktree_security.rs:105` |
| `unneeded return statement` | `wcore-tools/src/vision_tools.rs:272` |

**Severity split, stated carefully.** Individually these are MEDIUM-and-below hygiene
lints — no correctness bug among them. Their *aggregate effect* is HIGH and is the
finding that matters: under `-D warnings` they make the Windows CI leg fail at lint,
which means it can never reach `Run tests`, which means **the F-01..F-04 compile errors
would never even be discovered by CI**. Two independent gates, both closed, both silent.

**This is a strictly larger failure surface than the local probe found.** `cargo build
--workspace --all-targets` on the box passes for `wcore-swarm` and `wcore-tools`; clippy
with `-D warnings` does not. Anyone repairing the Windows leg must satisfy BOTH gates —
fixing F-01..F-04 alone leaves the leg red at step 11.

Route: 20A-02, together with F-02/F-03/F-04. **Not fixed here** — this plan does not
repair behaviour or hygiene, and F-09 is not what terminated it (F-03 is).

---

## 3. Severity-classified finding register

Every finding carries a severity and a route. Per the amended phase rules: CRITICAL and
HIGH must be fixed or disproved; MEDIUM and below go to `.planning/BACKLOG.md` and do
not block.

| ID | Finding | Severity | Bucket | Route |
|---|---|---|---|---|
| F-01 | `wcore-tools` `bash_sandbox_routing_test` fails to compile on Windows — E0433, ungated `std::os::unix::fs::symlink` | HIGH | NEW | **FIXED IN THIS PLAN** — sanctioned mechanical cfg-gate repair (§4) |
| F-02 | `wcore-eval-scenarios` `runner_contracts` — E0432 ×2, `SYNCHRONIZE` imported from `Win32::System::Threading` (it lives in `Win32::Foundation`) | HIGH | NEW | **NOT FIXED HERE** — mechanically repairable, but it shares a unit with F-03, which is not. Escalated with F-03. |
| **F-03** | `wcore-eval-scenarios` `runner_contracts` — **E0277, `ProcessTree` is not `Sync` on Windows because `WindowsJob(HANDLE)` has `unsafe impl Send` but no `Sync`** | **HIGH** | **NEW** | **PLAN-TERMINATING — escalated to Sean (§4.3).** Repair is an `unsafe` soundness assertion in production `src/`, not a mechanical fix in the failing file. |
| F-04 | `wcore-skills` (lib test) — E0432, `ACCESS_ALLOWED_ACE_TYPE` no longer in `windows_sys::Win32::Security` | HIGH | NEW | **NOT FIXED HERE** — mechanically repairable, but out of this plan's scope fence and gated behind the same escalation. Route: 20A-02 or a Sean decision. |
| F-05 | The Windows leg of `ci.yml` could not have produced a test result on ANY recent tree: `cargo nextest run --workspace` fails at BUILD on Windows because of F-01..F-04, all of which are pre-existing | HIGH | NEW | Escalated with F-03 — it is the same root cause seen from the CI side. |
| F-09 | **The Windows CI leg fails at `Clippy (warnings = errors)` (step 11) and never reaches `Run tests`** — 11 `-D warnings` lints across `wcore-eval-scenarios`, `wcore-swarm`, `wcore-tools`, all in `cfg(windows)` code | HIGH (aggregate; each lint alone is LOW) | NEW | 20A-02. Independently confirms F-05 and is a STRICTLY LARGER surface than the local `cargo build` probe found (§2.3) |
| F-10 | **The macOS CI leg fails at the same step on 2 lints in `wcore-sandbox/src/backends/process_tree.rs`, so the 23 macOS-only TEST bodies were never type-checked** — `--all-targets` requested, not reached | HIGH (blocks the verdict; the lints themselves are LOW and there is NO real macOS compile error) | NEW | 20A-02 — a two-lint fix unlocks a definitive macOS verdict (§5.1.3) |
| F-11 | `CI (macos-latest)`, `CI (linux-containerized)` and `Build (aarch64-pc-windows-msvc)` were cancelled mid-flight on the first run with no second run, no concurrency trigger and `fail-fast: false`. Cause not established. | LOW | NEW | BACKLOG — recorded as an open question, not guessed at. Resolved for macOS by re-running the job. |
| F-06 | `C:\ferrox-win` was not pristine when found (`?? crates/wcore-swarm/.swarm-status.json`, a `wcore-swarm` run artifact) | LOW | NEW | BACKLOG — recorded and restored before any measurement (§1.2). Non-blocking. |
| F-07 | The `c39f7254` vs `ce9a11a6` "disagreement" in the record is a labelling artefact, not a measurement hazard — the two SHAs have identical code trees | INFO | NEW | Resolved in §1.1. No action. |
| M6 | Self-hosted CI contention — Wiring A makes `ci.yml` fire on this branch and its Windows leg runs on the SAME box 20A-02/03/04 measure on. Nothing serializes them. | MEDIUM | NEW | BACKLOG, explicitly non-blocking. First hypothesis for any inexplicable measurement. |

---

## 4. F-01 — the compile defect, and the one repair this plan is permitted to make

**Diagnostic:** §2.1.

**Root cause.** `crates/wcore-tools/tests/bash_sandbox_routing_test.rs` declares
`delegated_mutation_required_live_sandbox_confines_parent_and_descendants` with **no
cfg gate**, while its body opens with `use std::os::unix::fs::symlink;`. `std::os::unix`
does not exist on Windows. The file's two sibling live-sandbox tests are correctly
gated — `#[cfg(unix)]` at :261 and `#[cfg(target_os = "linux")]` at :299, both of which
also use `std::os::unix::fs::symlink` — so this is a single omitted attribute, not a
design problem.

**Blast radius, and why this is HIGH rather than MEDIUM.** A test-binary compile error
is not confined to its own test. It takes down the **entire**
`wcore-tools::bash_sandbox_routing_test` binary — all 19 tests in the file — on Windows,
and it fails `cargo nextest run --workspace` at the BUILD step, meaning the Windows leg
of `ci.yml` could not have produced a test result on this tree at all. The nine-defect
Linux-only green suite could never have shown this: on Linux the file compiles.

**Repair (Task 1's sanctioned scope: "a mechanical module-path, import or cfg-gate
defect confined to the failing file").** Added `#[cfg(unix)]` to the one ungated test.
Single attribute, single file, no design change, no API change.

**The gate choice is deliberate, and it is the conservative one.** `#[cfg(unix)]`, not
`#[cfg(target_os = "linux")]`. The test's own doc comment says "Required Linux live
acceptance (runs on the Hetzner gate)" and it asserts
`wcore_tools::bash::platform_enforces_read_deny()`, so `target_os = "linux"` would
arguably match its intent more tightly — and that is exactly why it was rejected.
Narrowing to `linux` would ALSO remove the test from macOS, where it runs today and
where macOS CI has never executed this tree. That would silently gate away a macOS
result before anyone has seen one. `#[cfg(unix)]` is the **minimum** that resolves
E0433, and it leaves every platform's current behaviour unchanged except Windows, which
goes from "cannot compile" to "correctly excluded". If this test goes red on the macOS
leg, that is a finding this plan reports — not one it pre-emptively hides.

Nothing was `#[ignore]`d, `#[allow]`ed, weakened or deleted.

### 4.1 F-02 — `SYNCHRONIZE` imported from the wrong module

```
error[E0432]: unresolved import `windows_sys::Win32::System::Threading::SYNCHRONIZE`
   --> crates\wcore-eval-scenarios\tests\runner_contracts.rs:133:62
133 |     use windows_sys::Win32::System::Threading::{OpenProcess, SYNCHRONIZE, WaitForSingleObject};
    |                                                              ^^^^^^^^^^^ no `SYNCHRONIZE` in `Win32::System::Threading`

error[E0432]: unresolved import `windows_sys::Win32::System::Threading::SYNCHRONIZE`
   --> crates\wcore-eval-scenarios\tests\runner_contracts.rs:178:45
178 |             OpenProcess, PROCESS_TERMINATE, SYNCHRONIZE, TerminateProcess, WaitForSingleObject,
    |                                             ^^^^^^^^^^^ no `SYNCHRONIZE` in `Win32::System::Threading`
```

`windows-sys` exposes `SYNCHRONIZE` from `Win32::Foundation` — the same module the file
already imports `CloseHandle` and `WAIT_OBJECT_0` from two lines above. This is a
one-line import-path move in the failing file, i.e. mechanically repairable within
Task 1's sanctioned scope. It is **not** repaired here only because it shares a
compilation unit with F-03, so fixing it in isolation cannot make that unit build and
would leave the tree in a half-repaired state across an escalation boundary.

### 4.2 F-04 — `ACCESS_ALLOWED_ACE_TYPE` no longer in `Win32::Security`

```
error[E0432]: unresolved import `windows_sys::Win32::Security::ACCESS_ALLOWED_ACE_TYPE`
   --> crates\wcore-skills\src\bundled\bundled_tests.rs:607:29
607 |         ACCESS_ALLOWED_ACE, ACCESS_ALLOWED_ACE_TYPE, ACL, DACL_SECURITY_INFORMATION, EqualSid,
    |                             ^^^^^^^^^^^^^^^^^^^^^^^ no `ACCESS_ALLOWED_ACE_TYPE` in `Win32::Security`
```

Note that rustc's `help` suggestion here is **wrong** — it proposes replacing the
constant with a duplicate of `ACCESS_ALLOWED_ACE`, which would not compile. Anyone
repairing this should resolve the constant's real location in the pinned `windows-sys`
version rather than take the suggestion. This is a `windows-sys` API-drift defect, and
it kills the entire `wcore-skills` **lib test** binary on Windows — including the seven
Windows-only tests in `bundled_tests.rs` and every other unit test in that crate.

Not repaired here: `crates/wcore-skills/src/bundled/bundled_tests.rs` is not the file
that Task 1's compile probe was scoped to, and the plan's scope fence limits code repair
to "a compile error in a Windows-only or macOS-only test body" reachable by a mechanical
single-file fix. This one is mechanical, but landing it would not unblock the build
(F-03 still fails), so it is escalated as part of the same package rather than applied
piecemeal.

### 4.3 F-03 — THE PLAN-TERMINATING FINDING

```
error[E0277]: `*mut c_void` cannot be shared between threads safely
    --> crates\wcore-eval-scenarios\tests\runner_contracts.rs:707:16
     |
 707 |       let task = tokio::spawn(async move {
     |  ________________^ `*mut c_void` cannot be shared between threads safely
     |
     = help: within `ProcessTree`, the trait `Sync` is not implemented for `*mut c_void`
note: required because it appears within the type `process_tree::windows::WindowsJob`
    --> crates\wcore-eval-scenarios\src\process_tree.rs:491:23
 491 |     pub(super) struct WindowsJob(HANDLE);
note: required because it appears within the type `process_tree::Backend`      (src/process_tree.rs:40)
note: required because it appears within the type `process_tree::ProcessTree`  (src/process_tree.rs:31)
     = note: required for `&ProcessTree` to implement `Send`
note: required because it's used within this `async` fn body
    --> crates\wcore-eval-scenarios\src\process_tree.rs:342:69   (reap_child)
note: required because it's used within this `async` fn body
    --> crates\wcore-eval-scenarios\src\process_tree.rs:217:83   (terminate)
note: required because it's used within this `async` fn body
    --> crates\wcore-eval-scenarios\src\runner.rs:611:84         (run_session_body)
note: required because it's used within this `async` fn body
    --> crates\wcore-eval-scenarios\src\runner.rs:565:88         (run_prepared_session)
note: required because it's used within this `async` fn body
    --> crates\wcore-eval-scenarios\src\runner.rs:486:37
note: required because it's used within this `async` fn body
    --> crates\wcore-eval-scenarios\src\runner.rs:416:37
note: required by a bound in `tokio::spawn`   (F: Future + Send + 'static)
```

**What it means.** `crates/wcore-eval-scenarios/src/process_tree.rs:491` declares
`pub(super) struct WindowsJob(HANDLE)` where `HANDLE = *mut c_void`, and at :494 asserts
only half of what is needed:

```rust
#[derive(Debug)]
pub(super) struct WindowsJob(HANDLE);

// SAFETY: this wrapper uniquely owns a process-wide kernel handle.
unsafe impl Send for WindowsJob {}
```

`Send` is implemented; **`Sync` is not**. `ProcessTree::reap_child(&self, …)` is an
`async fn` taking `&self`, so its future holds `&ProcessTree` across an `await`, and
`&T: Send` requires `T: Sync`. That requirement propagates up through `terminate`,
`run_session_body`, `run_prepared_session` and two more `async fn` bodies in
`src/runner.rs`, and finally collides with `tokio::spawn`'s `F: Future + Send` bound at
`tests/runner_contracts.rs:707`.

**Why this terminates the plan, precisely against the stated criterion.** Termination
state 2 fires when "a Windows-only or macOS-only body fails to compile and the repair is
NOT a mechanical module-path/import/cfg fix confined to the failing file." Every clause
is satisfied:

1. **Not confined to the failing file.** The failing file is
   `tests/runner_contracts.rs`. The repair site is `src/process_tree.rs:491` — a
   different file, and **production source**, which this plan's scope fence explicitly
   excludes ("This plan adds no production code beyond a possible mechanical compile
   fix").
2. **Not a module-path, import or cfg fix.** The repair is `unsafe impl Sync for
   WindowsJob {}` — a new **soundness assertion** that a Win32 Job Object `HANDLE` may
   be shared by reference across threads concurrently. That is a correctness judgement
   about Win32 handle semantics under concurrent `AssignProcessToJobObject` /
   `QueryInformationJobObject` / `TerminateJobObject`, not a mechanical edit. The
   existing `Send` impl was deliberately scoped to *unique ownership* ("this wrapper
   **uniquely** owns a process-wide kernel handle"), which is exactly the weaker claim;
   widening it to `Sync` contradicts that comment's reasoning and needs an author.
3. **The alternatives are worse and are all redesigns.** Restructuring
   `runner_contracts.rs:707` to avoid `tokio::spawn`, or changing `reap_child`/
   `terminate` to take `&mut self`, or changing `Backend`'s representation — each is a
   design change to production async plumbing spanning six `async fn` bodies.

The plan says: *"Do not redesign anything to make it compile."* So it was not redesigned.

**ESCALATED TO SEAN.** Decision required: whether `WindowsJob`'s handle is genuinely
safe to share across threads (`unsafe impl Sync`, with a SAFETY note that supersedes the
current unique-ownership comment), or whether the ownership model should be tightened
instead so the `Sync` requirement never arises. Route the outcome to 20A-02 together
with F-02 and F-04, which live in the same two compilation units.

**Pre-existing, not a regression.** `git diff --stat ce9a11a6 b334d917 -- crates/` is
empty, so none of F-01..F-04 was introduced by the Phase 20 repair commits or by the 20A
planning commits. They are structural, exactly like the AppContainer bind blocker
(blocker A), and they have been invisible for the same reason: the green suite is
Linux-only and the Windows CI leg had never run this tree.

**F-05 is the same fact seen from CI.** Because `cargo nextest run --workspace` builds
before it runs, F-01..F-04 mean the Windows leg of `ci.yml` fails at the BUILD step. It
therefore cannot have produced a Windows test result on this tree — and, since the
defects are pre-existing, very likely not on recent trees either. Any claim of "Windows
CI green" for this branch would have been vacuous.

---

## 5. Wiring (Task 2)

Landed in `95552f64`. Three wirings; the six-target native proof script is untouched.

### 5.1 WIRING A — CI now fires on this branch

`ci.yml` triggered only on `pull_request → main` / `push → main`. Added
`plan/f20-unified-audit-repair` to `push.branches`. The job matrix, runner labels,
concurrency group and every test command are unchanged.

**`workflow_dispatch` was tried first and rejected on a hard technical ground, not
taste.** GitHub only exposes `workflow_dispatch` for a workflow already present on the
**default branch**; `ci.yml` on `main` has no such trigger, so a dispatch against this
ref cannot fire, and pushing the trigger to `main` is forbidden without Sean. Naming the
branch in `push` is the only route that actually runs.

**PROVEN:**

```
$ gh run list -R FerroxLabs/wayland-core --branch plan/f20-unified-audit-repair --limit 5
Workflow Runs
  CI  [30151510189]      <- head sha 95552f64
```

Before this change the same command returned `[]`. This is the branch's **first CI run
ever**. Jobs dispatched: `CI (macos-latest)`, `CI (Array)` (the self-hosted Windows
leg), `CI (linux-containerized)`, the six `Build (<target>)` legs, the eval gate and the
browser-live job.

Per the plan: this is normal CI, **not** the Sean-gated native proof dispatch. That is a
different workflow (`nightly-windows-soak.yml` with `f20_candidate=true` and a nonce)
and it belongs to 20A-04. Nothing here dispatches it.

#### 5.1.1 Run outcome — a REPORTED RED, which is the wiring working

| Job | Conclusion |
|---|---|
| `CI (Array)` — self-hosted Windows | **failure** (Clippy, step 11 — **F-09**, §2.3) |
| `CI (macos-latest)` | cancelled at step 8 — see §5.1.2 |
| `CI (linux-containerized)` | cancelled |
| `Build (x86_64-pc-windows-msvc)` | success |
| `Build (aarch64-pc-windows-msvc)` | cancelled |
| `Build (x86_64-apple-darwin)` / `Build (aarch64-apple-darwin)` | success |
| `Build (x86_64-unknown-linux-gnu)` / `(aarch64-unknown-linux-gnu)` | success |
| `Eval acceptance gate (Linux, containerized)` | success |
| `Browser live e2e (chromium)` | success |
| **Run** | **failure** |

The Windows red is the point. Wiring A's entire purpose was to let this branch fail
visibly rather than pass invisibly, and on its first run it immediately surfaced a defect
class (F-09) that the local compile probe could not see.

Note that the six `Build (<target>)` legs are **not** evidence about platform-only tests:
they run `cargo build --release -p wcore-cli`, with no `--all-targets`, so they never
type-check a test body. `Build (x86_64-apple-darwin)` succeeding says nothing about the
23 macOS-only tests. Only the `ci` matrix's Clippy step
(`cargo clippy --workspace --all-targets`) does.

#### 5.1.2 WHAT COULD NOT BE OBTAINED — the macOS-only compile verdict

**The 23 macOS-only test bodies remain UNVERIFIED. This is the one Task 1 deliverable
this plan did not land, and it is reported rather than papered over.**

`CI (macos-latest)` was **cancelled at step 8** ("Pre-build wcore-cli release binary"),
so steps 9-16 — including the Clippy step that is the only thing in the whole workflow
that type-checks macOS-only test bodies — were **skipped**:

```
 7. Pre-build tool_token_bench                 : completed success
 8. Pre-build wcore-cli release binary         : completed cancelled   <-- stopped here
10. Check formatting                           : completed skipped
11. Clippy (warnings = errors)                 : completed skipped     <-- the answer lives here
12. Run tests (nextest CI profile)             : completed skipped
```

Cause not established. It is **not** concurrency `cancel-in-progress` — `gh run list` for
this branch shows exactly ONE run (`30151510189`), and no second push occurred. It is
also not matrix fail-fast, which is explicitly `fail-fast: false` on the `ci` job. Three
jobs were cancelled mid-flight (`macos-latest`, `linux-containerized`,
`Build (aarch64-pc-windows-msvc)`) while `CI (Array)` failed, which is consistent with a
run-level cancellation of unknown origin. Recorded as an open question, not guessed at.

The macOS job was re-run (`gh run rerun --job 89662563384`). This is a **retry of a
cancelled job, not a compile-repair round-trip** — the plan's cap of two compile-repair
round-trips (M5) is untouched at **zero used**.

#### 5.1.3 The macOS re-run — F-10, and the honest verdict

The re-run cleared step 8 and reached step 11. **`Clippy (warnings = errors)` FAILED**,
and steps 12-16 were skipped again.

```
   Checking wcore-sandbox v0.12.25 (/Users/runner/work/wayland-core/wayland-core/crates/wcore-sandbox)
error: method `signal` is never used
   --> crates/wcore-sandbox/src/backends/process_tree.rs:403:8
    = note: `-D dead-code` implied by `-D warnings`
error: unneeded `return` statement
   --> crates/wcore-sandbox/src/backends/process_tree.rs:225:9
    = note: `-D clippy::needless-return` implied by `-D warnings`
error: could not compile `wcore-sandbox` (lib) due to 2 previous errors
error: Recipe `lint` failed on line 76 with exit code 101
```

**Read this precisely, because the good news and the bad news are easy to confuse.**

**The good news.** There is **no genuine compile error anywhere on macOS.** Both
diagnostics are `-D warnings` lints — one `dead-code`, one `clippy::needless-return` — in
macOS-gated code in `wcore-sandbox`'s **lib**. macOS is in a far better position than
Windows, which has four real type errors (F-01..F-04).

**The bad news, and the honest verdict.** **The 23 macOS-only TEST bodies remain
UNVERIFIED.** `-D warnings` aborts the `wcore-sandbox` lib unit, so clippy never proceeds
to that crate's test targets — and every macOS-only test lives exactly there
(`tests/live_integrity_macos.rs`, `tests/hard_process_containment_macos.rs`, and the
`cfg(target_os = "macos")` unit tests). `--all-targets` was requested; it was not
reached. **This plan therefore does NOT close the macOS half of the audit's compile
question, and does not claim to.**

**Why it was not simply fixed.** The two lints are in **production** source
(`crates/wcore-sandbox/src/backends/process_tree.rs`), not in a test body. That is outside
this plan's scope fence ("The ONLY code repair in scope is a compile error in a
Windows-only or macOS-only test body"), and they are not compile errors at all. The plan
was already in its compile-blocked terminal state when this landed. Fixing them would
have been repair-after-termination.

**Actionable for whoever resumes — this is a cheap unlock.** Two trivial lint fixes in one
file are all that stand between the project and a definitive macOS-only compile verdict.
Fix them, re-run `CI (macos-latest)`, and read step 11. The plan's M5 cap of two
compile-repair round-trips is **untouched at zero used** — the one re-run performed here
was a retry of a cancelled job, not a repair round-trip, so a resumer has the full budget.

Route: 20A-02, with F-09 (same class, same `-D warnings` mechanism, different platform).

### 5.2 WIRING B — `wcore-sandbox` joins the recurring Windows soak

`scripts/wayland-e2e-windows-soak.ps1` PHASE G now selects six crates. The phase's own
description and count were corrected 5 → 6 so the script no longer describes itself
falsely.

**All five original crates preserved** — gate output:

```
$ for c in wcore-cron wcore-config wcore-providers wcore-tools wcore-swarm; do
    printf "%s=%s " "$c" "$(/usr/bin/grep -cF "$c" scripts/wayland-e2e-windows-soak.ps1)"; done
wcore-cron=1 wcore-config=1 wcore-providers=3 wcore-tools=1 wcore-swarm=1
$ /usr/bin/grep -cF 'wcore-sandbox' scripts/wayland-e2e-windows-soak.ps1
7
```

This is the single highest-leverage line in the plan: 105 Windows-only tests, including
every retained-handle security proof, get a recurring execution path for the first time.

### 5.3 WIRING C — the ten orphaned ACL tests and the orphaned containment marker

New **PHASE L** in the same soak script runs the **whole ignored set** of both files:

```powershell
cargo nextest run --run-ignored all --no-tests=fail --no-fail-fast -p wcore-sandbox --test live_fs_acl --nocapture
cargo nextest run --run-ignored all --no-tests=fail --no-fail-fast -p wcore-sandbox --test hard_process_containment_windows --nocapture
```

Three deliberate properties:

- **File-level selection, never name enumeration.** A hand-enumerated selector is
  exactly how ten of the twelve `live_fs_acl` tests fell out in the first place; a
  file-level selector cannot silently lose a test added later.
- **`--no-tests=fail`** — an empty selector fails closed, matching the discipline
  `f20-native-windows-proof.ps1` already applies.
- **The live-acceptance flag is set in the trap-safe form and PROVEN.** The phase does
  `$env:WAYLAND_SANDBOX_LIVE_WINDOWS = '1'`, echoes it back delimited with its length,
  and hard-fails on `-cne '1'`. The `cmd` form `set VAR=value && …` appends a trailing
  space that Rust reads verbatim; `require_live_acceptance()` in `live_fs_acl.rs:28`
  asserts `== "1"`, so the trailing-space form yields a vacuous or wrongly-attributed
  run. Mitigates T-20A-01-04.

`native_containment_gate_marker` — the audit's "FIX or DELETE" item #5 — is resolved as
**FIX**: it is now selected by the `hard_process_containment_windows` file-level
selector. It was not deleted.

**Runner: the self-hosted `windows-live-acceptance` job, NOT `windows-2022`.** The
hosted image is a server SKU that reports `AppContainerBackend::is_available() == false`
— documented in this very workflow as the reason `f20-windows-candidate` was moved to
self-hosted. Running the live set there would produce a guaranteed environmental red
every night, which trains readers to ignore the runner; that is not a "reported red", it
is a capability mismatch. The job is `if: github.event.inputs.f20_candidate != 'true'`,
so a Sean-authorized candidate dispatch still runs ONLY the two exact F20 native jobs.

When the opt-in is absent, PHASE L prints `PHASE L — SKIPPED (not run, NOT passed)` and
names why. It is never counted as green. An unrecognised value of
`WAYLAND_SOAK_LIVE_ACCEPTANCE` is a hard error, so a typo cannot silently degrade the
gate into a skip.

Gate output:

```
$ /usr/bin/grep -cF 'live_fs_acl' scripts/wayland-e2e-windows-soak.ps1                    -> 7
$ /usr/bin/grep -cF 'run-ignored' scripts/wayland-e2e-windows-soak.ps1                    -> 2
$ /usr/bin/grep -cF 'hard_process_containment_windows' scripts/wayland-e2e-windows-soak.ps1 -> 6
```

### 5.4 THE INVARIANT THIS PLAN WAS MOST AT RISK OF BREAKING — held

```
$ /usr/bin/git diff --exit-code -- scripts/f20-native-windows-proof.ps1
ZERO DIFF: proof script target array is 20A-04's invariant and is unmodified
```

`verifyNativeLog` (`scripts/f20-native-uat-proof.mjs:406-453`) fails closed on any
target marker outside the canonical six (`foreign target marker`), on any repeat
(`duplicate target marker`), and on any reordering (`target markers out of order`).
Adding entries to `$targets` would make the six-target proof fail closed on its own new
markers; widening the `-E` filter on `windows-retained-handle` or
`windows-appcontainer-acl` would load two CERTIFIED-GREEN targets with ten
never-executed tests. Either route makes Phase 20A Success Criterion 1 unreachable.
Wiring C therefore landed entirely off that file.

Anti-drift guard intact, no target reclassified (REQ-native-r8):

```
$ /usr/bin/grep -c "os = 'windows'" scripts/f20-native-windows-proof.ps1   -> 4
```

Four OS-specific Windows targets, unreduced — and trivially so, since the file has a
zero diff.

All twelve `live_fs_acl` test names still present in source (nothing deleted or renamed):

```
$ (twelve-name loop) -> count=12 (expect 12)
```

### 5.5 The modified soak script PARSES on real Windows

The Mac has no `pwsh`, so the edited PowerShell was proven on the box:

```
ORIG_ERRORS=0    (scripts/wayland-e2e-windows-soak.ps1 @2a9d47ff, i.e. pre-Wiring)
NEW_ERRORS=0     (scripts/wayland-e2e-windows-soak.ps1 @95552f64, i.e. post-Wiring B+C)
```

under `pwsh` (PowerShell 7) — the interpreter `nightly-windows-soak.yml` actually invokes
(`shell: pwsh`, `pwsh scripts/wayland-e2e-windows-soak.ps1`).

Under Windows PowerShell **5.1** the same check returns 7 errors for the ORIGINAL file and
8 for the modified one, in code this plan never touched (PHASE H's `cargo mutants --list`).
That is a UTF-8 decode artifact of the script's non-ASCII banner/status glyphs, pre-existing
and unreachable on the real runner. Logged as BACKLOG **F-08 · LOW · non-blocking**, and
worth stating plainly: the raw "8 errors" number is NOT evidence of a defect introduced here
— the correct comparison is 0 vs 0 on the interpreter that runs it.

### 5.6 What was NOT run, and why

PHASE L was **not** executed on hardware in this plan. The plan reached its
compile-blocked termination state (§4.3) at Task 1, and Task 1's action is explicit:
*"STOP … Write the diagnostics into the baseline document, write the SUMMARY, and
escalate to Sean."* Executing the newly wired surface and re-measuring the four suites
would be continuing past a defined terminal state, which the termination criterion
forbids ("Under no circumstances does this plan spawn additional plans, extend its own
task list, or start a second measure/fix cycle").

The wiring is landed, gate-proven at source level, and ready. The first execution of the
newly selected tests belongs to whichever plan resumes after the F-03 decision.

---

## 6. Re-measured four-suite baseline (Task 3)

**NOT PERFORMED — the plan terminated compile-blocked at Task 1 (§4.3).**

The prediction carried forward for whoever resumes, restated here so it is not lost, and
still explicitly a PREDICTION and not a result. Measured at `c39f7254`, whose code tree
is identical to `ce9a11a6`, and whose delta to the pinned SHA is `.config/nextest.toml`
timeout overrides plus five desktop contract fixtures with **no `crates/**/*.rs`
change** (verified in §1.1):

| Suite | run / passed / failed / skipped | Attribution |
|---|---|---|
| `wcore-sandbox` | 135 / 135 / 0 / 45 | green twice consecutively |
| `wcore-swarm` | 90 / 83 / 7 | all 7 → blocker A |
| `wcore-agent --test transactional_delegated_mutation_test --run-ignored all` | 9 / 5 / 4 | all 4 → blocker A |
| `wcore-swarm --test dispatch_smoke` | 7 / 3 / 4 (3 skipped) | all 4 → blocker A |

Blocker A remains the delegated-backend admission refusal `sandbox backend appcontainer
cannot bind retained delegated workspace authority` (`crates/wcore-swarm/src/dispatch.rs:52-57`),
reached because `AppContainerBackend` overrides neither `binds_cwd_authority` nor
`execute_with_cwd_authority` and both keep their fail-closed trait defaults
(`crates/wcore-sandbox/src/backends/mod.rs:299-350`). Pre-existing and structural.
**Route: 20A-02. Untouched by this plan.**

Note for the resumer: `wcore-sandbox` and `wcore-agent` both compile clean at the pinned
SHA (§2.1.2), so these four suites are measurable as soon as the F-03 decision unblocks
the plan — the compile blocker is in `wcore-eval-scenarios` and `wcore-skills`, neither
of which any of the four suites depends on for its own build.
