# F21-BWRAP-OVERLAP — 21-C3-01 repair

**Lane** `f21-bwrap-overlap` · branch `lane/f21-bwrap-overlap` · base `eaff921d` on
`plan/f20-unified-audit-repair`, merged forward to `3f261977` · **HEAD `a45ce175`**.

**Remit.** Fix 21-C3-01 — overlapping `fs_read_deny` entries abort bubblewrap, so a delegated
`IsolatedMutation` child cannot run any shell command on Linux. Decide where the fix belongs
and justify it. Prove containment is not weakened. Prove it at the shipped-binary level.
Check the other two backends. Report anything the unmasking reveals.

**Verdict: all five delivered.** The fix is in the renderer. Both halves are proven, on all
three backends, each behind an instrument control and each with its known-negatives shown to
fire. The unmasking flipped a Phase 21 corpus cell and the finding is below in §5 — it cuts
partly against the fix.

---

## 1. Where the fix went, and why the code — not the guess — put it there

`crates/wcore-sandbox/src/backends/bwrap.rs`: new `DenyMountKind` + `reduce_read_deny_mounts`.
Classify each `fs_read_deny` entry once (Directory / NonDirectory / Absent), drop entries
strictly nested under a **Directory** deny and exact repeats, then render. ~40 lines of logic.

The orchestrator's instinct was the renderer, and the renderer is right — but for reasons the
code establishes, not because it was suggested. Four, in ascending order of force:

1. **The defect is order-dependent.** Measured on `hetzner-dsm`, bubblewrap 0.9.0: the pair
   `[/p, /p/q]` exits 1 having run nothing, while `[/p/q, /p]` runs the shell and exits 0.
   `fs_read_deny` is a `Vec` expressing a SET; a set has no order. A renderer whose success
   depends on the order of a set is broken at the renderer.

2. **`spawner.rs`'s pair is semantically CORRECT, and deduplicating it there would open a
   hole.** `git_common_dir` is `<parent>/.git` only when the parent is an ordinary clone.
   When the parent is itself a **linked git worktree**, `--git-common-dir` resolves to the
   MAIN repository's `.git`, which is **outside** the parent workspace and must be denied
   separately. Whether the pair nests is a property of the parent's git layout. Dropping
   `git_common_dir` at the spawner would silently stop denying the main repo's object store
   for every worktree parent. The permanent test
   `disjoint_denies_are_left_untouched` pins exactly this case.

3. **There are at least three independent producers**, so a spawner-side fix repairs one
   caller and leaves the trap armed for the rest:
   - `wcore_agent::spawner` — `[parent, git_common_dir]` (`spawner.rs:1833`), also passed to
     `with_authority_write_deny` (`spawner.rs:2728`);
   - `WorkspacePolicy::secret_deny_paths_dynamic` (`workspace_policy.rs:561`) → `bash.rs:133`,
     whose secret walk returns a credentials directory and files inside it, and whose own test
     `bash/tests.rs:814-815` asserts the same `[parent, git_common]` pair reaches the manifest
     on the **ordinary Bash path** — so this was never only a delegated-child defect;
   - `wcore_swarm::dispatch` — `sandbox_read_denies` (`dispatch.rs:645`).

4. **The overlap is legal in the manifest contract; only bubblewrap cannot express it.**
   macOS emits independent SBPL predicates, Windows applies a protected DACL per object —
   both measured below, both fine. bubblewrap is the only mount-based backend. A constraint
   that exists in exactly one backend's rendering mechanics belongs in that backend, not
   pushed up into platform-neutral callers.

**Why dropping is safe, not merely convenient.** A deny nested under a *directory* deny is
redundant: the ancestor's empty read-only mask removes the descendant pathname from the
namespace entirely. Measured — with only the ancestor denied, reading `<parent>/.git/config`
fails and writing into `<parent>/.git/` fails `Directory nonexistent`, identical to denying
both. Nesting is decided component-wise (`Path::starts_with`), so `/p-backup` keeps its own
mount. An **Absent** ancestor mounts no mask and therefore subsumes nothing. A
**NonDirectory** entry subsumes nothing. Comparison is lexical, so a symlinked alias is not
recognised and both denies are emitted — the safe direction: a redundant mount that may
abort, never a silently discarded denial.

---

## 2. Both halves, at the shipped-binary level

**Half 1 — the shell runs. Half 2 — the parent and its `.git` are still refused.** Neither
alone is a pass: half 1 without half 2 is a hole in a sandbox; half 2 without half 1 is the
broken build that made three Phase 21 plans record a refusal that never happened.

**Base RED → head GREEN, through the production `execute` path** (`hetzner-dsm`, unproxied
`/root/.cargo/bin/cargo`). The committed live test was transplanted verbatim onto the unfixed
base `eaff921d` as a throwaway integration test:

```
BASE eaff921d: bwrap aborted on the overlapping deny pair (21-C3-01):
  stderr="bwrap: Can't mkdir /tmp/.tmpGQI7qq/.git: Read-only file system"
  test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

It reached arm 2, which means **arm 1 — the no-deny leak control — passed**: the probe could
read both secrets. The abort is not an artifact of a dead instrument.

```
HEAD (fixed): test result: ok. 89 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
              (base lib was 80 — the delta is exactly my 9 additions, all named and ok)
```

**And at the true shipped-binary level, using the phase's own corpus rather than my
instrument** — `cargo test -p wcore-cli --test child_authority_corpus`, the real
`wayland-core` binary in a headless PTY: **29 passed; 0 failed; 0 ignored; 0 measured; 0
filtered out**, reproduced twice (24.29 s and 24.13 s, independent runs). The corpus's own
verdict text for the `corpus_tool` linux/standalone/live cell:

> the delegated child's **SHELL RAN** — its stdout marker returned on the wire in a served
> provider request — and its write **still produced no effect** in the hermetic home. …
> ATTRIBUTED TO WORKSPACE CONTAINMENT, NOT TOOL AUTHORITY. The same shell command's write to
> a RELATIVE path — inside the child's own workspace, where containment has nothing to bind —
> **succeeded** and returned its marker on the wire.

That is both halves in one row, written by the phase's corpus, not by me: the child holds and
exercises Bash, and only the out-of-workspace destination is refused.

**The containment assertions can fail.** Known-negative harnesses (uncommitted, deleted after
each run) drive arm 2 with an injected deny list:

| injection | Linux | macOS 26.3 | Windows |
|---|---|---|---|
| `as_committed` `[parent, parent/.git]` | ran, no leak | ran, no leak | ran, no leak |
| `drop_ancestor` `[parent/.git]` | ran, **parent LEAKS**, `.git` denied | same | same |
| `no_deny` `[]` | ran, **both LEAK** | same | same |

`drop_ancestor` earns its place twice: it proves the parent-containment assertion *can* fire,
and `git_leak=false` in the same row proves the two assertions are **independent** — `.git` is
still enforced on its own, so the ancestor mask is not the only thing doing the work.

---

## 3. The other two backends — both MEASURED, neither inferred

| Backend | Mechanism | Overlapping pair | Evidence |
|---|---|---|---|
| Linux `bwrap` | `--ro-bind` **mount** per deny | **ABORTED** — the defect | fixed here |
| macOS `sandbox_exec` | independent SBPL `(deny file-read* (subpath …))`, last-match-wins | **TOLERATED** | `1 passed; 0 failed; 0 ignored; 104 filtered out` |
| Windows `AppContainer` | protected DACL **per object** (`apply_protected_deny`) | **TOLERATED** | `1 passed; 0 failed; 0 ignored; 12 filtered out` |

Both new tests are permanent, both build their policy with the *production* function
(`build_profile` / `canonical_intents` → `apply_intents`) and run the *production* `execute`,
and both use the exact `[parent, parent/.git]` pair `spawner.rs` builds.

- **macOS** — `sandbox_exec.rs::overlapping_read_deny_runs_shell_and_still_contains`. Run on
  the Mac under the LANE-BRIEF §0 **Darwin-behaviour exception** (single crate, single
  filtered test), **disclosed as required**: `sandbox-exec` is macOS-only and no permitted
  build host runs macOS, so this was otherwise unprovable. It also asserts both nested denies
  reach the profile as independent rules, so a future change that silently collapsed them
  would fail here.
- **Windows** — `live_fs_acl.rs::overlapping_directory_denies_run_the_command_and_still_contain`,
  **three** arms, because two questions are in play and must not conflate: arm 2 (single deny
  on the granted directory) isolates allow-then-deny ordering; arm 3 vs arm 2 isolates the
  nesting. It also asserts no AppContainer ace survives on **either** object of the nested
  pair. `NATIVE_ACCEPTANCE_CASES` bumped 11 → 12 so the binary's zero-execution gate stays
  honest. Verified at `WLSHA=a791979c`, my own branch HEAD, read back from the run.

**21-C3-06 is closed: there is no Windows analogue, and it is measured.** Only bubblewrap had
the defect, and only because it is the only mount-based backend.

---

## 4. Nothing was weakened

Unproxied `git grep` over `crates/`, `eaff921d` → `bcc134dd` (my work, pre-merge):

- `#[allow` **188 → 188**.
- `#[ignore` **223 → 224**. The `+1` is mine and is stated plainly: the new Windows case
  carries `#[ignore = "explicit native Windows AppContainer acceptance"]`, which is the
  established contract of that binary — all 11 pre-existing native cases carry it, and the
  `native_acceptance_gate_marker` test exists precisely to stop that pattern going vacuous.
  I bumped its constant 11 → 12 in the same commit, so the gate still fails on a run that
  cannot execute the cases. **No existing test was ignored, re-gated, deleted or loosened; no
  timeout was raised.**
- Instrument-alive control for those greps: the same query returns **11** `#[test]` in
  `bwrap.rs` at the same rev, so it is not returning numbers because it is broken.
- **Shared fence untouched:** `git diff $(git merge-base HEAD 3f261977) -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs` is **0 lines**.
- Footprint: **3 files, +599 / −24**, all under `crates/wcore-sandbox/`.

---

## 5. What the unmasking revealed — and it cuts against the fix, too

My fix removes **one of the four** mechanisms 21-C3-02 found were making Phase 21 `corpus_tool`
REFUSED readings come from tool calls that never executed. Reporting it loudly, as asked:

1. **The `corpus_tool` linux/standalone/live cell flipped NOT-EXPRESSIBLE → REFUSED**, and it
   is now a real measurement rather than an absence of effect. Before: *"the delegated child's
   shell never ran — `bwrap: Can't mkdir …/workspace/.git`"*. After: the shell runs, its marker
   returns on the wire, a relative-path write inside its own workspace succeeds, and only the
   out-of-workspace write is refused.

2. **`21-04-PHASE-VERDICT.md` was right about the mechanism and wrong that it had been
   measured.** The verdict attributed the tool REFUSED jointly to tool authority and workspace
   containment; 21-C3 §4 found neither among the four real causes. Post-fix, on this cell, the
   corpus attributes the refusal to **workspace containment** — one of the two the verdict
   named. The verdict's *claim* was sound; its *evidence* was a masked non-event.

3. **The tool dimension is still NOT proved enforced, and the corpus says so in the same
   row** — the refusal is containment, not tool authority. My fix closes only the first half of
   21-C3-04 (the child's shell can now run); the second half stands (a scripted probe still
   cannot learn the child's isolated checkout root, so `Write`/`Read`, which need an absolute
   path, cannot be targeted). **This fix does not make Criterion 3 met and I am not claiming
   it does.**

4. **Control that the flip is specific, not a general loosening:** `corpus_tool`
   host-protocol/live is *still* NOT-EXPRESSIBLE with cause *"the child's shell never ran"* —
   masked by the **confirmer** (21-C3-03), a different mechanism this fix does not touch. If
   my change had loosened something broadly, that cell would have moved too. It did not.

**Two of the four masking mechanisms remain open** (21-C3-03 confirmer, 21-C3-05 Windows
session-0 confirmer), plus 21-C3-04's checkout-root limit. Routed unchanged.

---

## 6. Instrument defects found in my own harness — repaired, not noted (§6b-ii)

1. **`\$` inside a bash double-quoted ssh argument escapes the dollar.** Reading
   `"D:\lane-f21bwo\$NONCE\status.txt"` left `$NONCE` literal for PowerShell, which expanded it
   as an undefined variable to empty, collapsing the path to `D:\lane-f21bwo\status.txt` — **a
   different, older, already-passing status file**. It reported `WLRC=0 / WLDONE` and read
   exactly like my run succeeding. Repaired by composing the path in bash first
   (`DIR="D:/lane-f21bwo/${NONCE}"`). Textbook self-passing instrument: a stale green from a
   file that was never mine.
2. **`SeanDesktop` is shared and a sibling process of this lane was live on it** — a
   `status2.txt` I had just written came back carrying another writer's format. Repaired by
   scoping every artifact to a per-run nonce directory (`D:\lane-f21bwo\r3x222112\`), which is
   LANE-BRIEF §6a-ii's shared-`/tmp` rule applied to the Windows box, and by reading `WLSHA`
   back out of the run to confirm the code under test was my branch HEAD.

---

## 7. What I did NOT do

- **`spawner.rs` was not modified.** The pair it builds is correct; see §1.2. If a reviewer
  prefers belt-and-braces deduplication there too, it is additive and harmless — but it is not
  a fix, and adding it would make the renderer's invariant look optional when it is not.
- **The write-deny path was not separately measured.** `authority_read_deny` also feeds
  `with_authority_write_deny` (`spawner.rs:2728`); that is `WorkspacePolicy`'s in-process VFS
  guard, not an OS mount, so it has no mount-ordering constraint. Stated as reasoning, not as
  a measurement.
- **Criterion 3 is still NOT MET.** See §5.3. One masking mechanism is gone; three limits
  remain.
- **The `.env`-vs-symlink aliasing case is not closed** — a deny reached by a symlinked alias
  of a denied ancestor is not recognised as nested and both mounts are emitted, which can still
  abort. It is the safe direction (never a discarded denial) and it is out of scope for a
  defect about literal nesting, but it is a live residual and is named here rather than buried.
- No merge to `main`, no PR, no tag, no release, no issue closed, no `wcore-contract generate`.
