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
| **pinned SHA** (the measurement SHA — probe SHA + the one sanctioned compile repair) | recorded in §4 once the repair commit exists |
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

### 2.2 macOS — obtained from CI (the Mac cannot compile this workspace)

Recorded in §5 (Wiring A), which is the first machine able to answer it.

---

## 3. Severity-classified finding register

Every finding carries a severity and a route. Per the amended phase rules: CRITICAL and
HIGH must be fixed or disproved; MEDIUM and below go to `.planning/BACKLOG.md` and do
not block.

| ID | Finding | Severity | Bucket | Route |
|---|---|---|---|---|
| F-01 | `bash_sandbox_routing_test.rs` fails to compile on Windows (E0433) | HIGH | NEW | FIXED IN THIS PLAN — sanctioned mechanical cfg-gate repair (§4) |

(Further rows are appended by Tasks 2 and 3.)

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

---

## 5. Wiring (Task 2)

_Appended by Task 2._

---

## 6. Re-measured four-suite baseline (Task 3)

_Appended by Task 3._
