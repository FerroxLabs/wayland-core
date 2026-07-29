# F21-BWRAP-OVERLAP — running notes

Lane `f21-bwrap-overlap` · branch `lane/f21-bwrap-overlap` · base `eaff921d` on
`plan/f20-unified-audit-repair`. Remit: fix 21-C3-01 (overlapping `fs_read_deny` aborts
bubblewrap), prove containment survives, check the other backends.

Append-only. Re-committed after every measurement.

---

## T0 — reading the defect (no code changed yet)

### The two sites, exact, at `eaff921d`

- `crates/wcore-agent/src/spawner.rs:1833`
  `authority_read_deny: vec![parent.as_ref().clone(), git_common_dir],`
  `git_common_dir` comes from `manager.git_common_dir()` at `spawner.rs:1767`.
- `crates/wcore-sandbox/src/backends/bwrap.rs`, the `fs_read_deny` loop (~L286-L311):
  a directory deny renders as `--ro-bind <shared empty tempdir mask> <denied path>`;
  a non-directory deny renders as `--ro-bind /dev/null <denied path>`;
  a vanished path renders as nothing.

Mechanism: bwrap applies binds in argv order. `--ro-bind mask /parent` makes `/parent` an
empty READ-ONLY mount. The next `--ro-bind mask /parent/.git` needs a mount point created
*inside* that read-only mount, so bwrap's `mkdir` fails and it aborts before `execve`.

### First non-obvious fact — the defect is ORDER-DEPENDENT

`[/parent, /parent/.git]` aborts. `[/parent/.git, /parent]` does not (the inner mount is
simply shadowed by the outer one). The renderer therefore realizes a *set* of denials whose
outcome depends on vector order. That is a renderer invariant violation, not a caller error.

### Second non-obvious fact — the spawner's pair is SEMANTICALLY CORRECT

`git_common_dir` is only `<parent>/.git` when the parent is an ordinary clone. When the
parent is itself a linked git worktree, `--git-common-dir` resolves to the MAIN repo's
`.git`, which is **outside** the parent workspace. So the pair overlaps or not depending on
the parent's git layout, which the spawner cannot know and should not special-case for one
backend's mount mechanics. Dropping `git_common_dir` at the spawner would open a real hole
for the worktree-parent case.

### Third fact — spawner is NOT the only producer

Independent producers that can hand the renderer overlapping paths:

- `crates/wcore-tools/src/bash.rs:133` — `manifest.fs_read_deny = p.secret_deny_paths_dynamic()`.
  `crates/wcore-tools/src/bash/tests.rs:814-815` asserts that path carries **both** `parent`
  and `git_common` — i.e. the ordinary Bash path ships the same overlapping pair.
- `crates/wcore-swarm/src/dispatch.rs:645` — `manager.sandbox_read_denies(workspace)`.
- `WorkspacePolicy::secret_deny_paths_dynamic` (`workspace_policy.rs:561`) walks for secrets
  and unions `authority_read_deny`; a walk that finds a creds *directory* and a `.pem` file
  inside it is a nested pair by construction.

=> the fix belongs in the RENDERER. Fixing at `spawner.rs` fixes one of at least three
callers and leaves the order-dependence in place for every future one.

### Fourth fact — the other backends do not have this shape

- macOS `sandbox_exec.rs:163` emits `(deny file-read* (subpath "<p>"))` per path. SBPL deny
  rules are independent predicates under last-match-wins; overlapping subpaths both match and
  both deny. No mount, nothing to abort. **To be PROVEN, not assumed** — Darwin exception
  (single crate, single test) applies since sandbox-exec is macOS-only.
- Windows AppContainer `appcontainer/acl_lease.rs:456,1149` — ACL/DACL based, not mount
  based. **Not yet read. Must be checked, per the remit.**

## Still to establish

1. Reproduce the abort at base on hetzner (control first).
2. Land the renderer fix + unit tests incl. order-swap and a known-negative.
3. Both-halves containment proof at the shipped-binary level: shell RUNS, parent read REFUSED,
   `.git` read REFUSED, in the same run.
4. macOS: prove overlap tolerated with a real `sandbox-exec` run.
5. Windows: read the AppContainer deny path; measure or name the gap.
6. Report anything the unmasking reveals (21-C3-02: four masking mechanisms, this is one).

---

## T1 — primitive reproduction on `hetzner-dsm` (bubblewrap 0.9.0)

`/tmp/f21bwo-repro2`, per-block stderr capture, control first:

```
--- A_control_parent_only  : rc=0  stdout: SHELL_RAN   stderr: (empty)
--- B_ancestor_first       : rc=1  stdout: (empty)     stderr: bwrap: Can't mkdir /tmp/f21bwo-repro2/workspace/.git: Read-only file system
--- C_descendant_first     : rc=0  stdout: SHELL_RAN   stderr: (empty)
--- D_deduped_ancestoronly : rc=0  stdout: SHELL_RAN   stderr: (empty)
```

**B vs C is the order-dependence.** Same two paths, opposite order, opposite outcome.

Containment at the primitive level, `/tmp/f21bwo-contain`, with a LEAK CONTROL:

```
E_no_deny (instrument liveness)  SHELL_RAN  PARENT_READ=LEAKED   GIT_READ=LEAKED
F_dedup   (ancestor only)        SHELL_RAN  PARENT_READ=REFUSED  GIT_READ=REFUSED
G_both    (descendant first)     SHELL_RAN  PARENT_READ=REFUSED  GIT_READ=REFUSED
```

F ≡ G. Dropping the nested deny is containment-equivalent; E proves the probe could
have seen both secrets, so the REFUSED readings are not free.

## T2 — the fix

`crates/wcore-sandbox/src/backends/bwrap.rs` — `DenyMountKind` + `reduce_read_deny_mounts`.
Classify each deny once (Directory / NonDirectory / Absent), drop entries strictly nested
under a **Directory** deny (component-wise `Path::starts_with`) and exact repeats, then
render. **Renderer, not `spawner.rs`** — reasons in T0.

## T3 — base RED / head GREEN, shipped-backend level

Both worktrees on `hetzner-dsm`, unproxied `/root/.cargo/bin/cargo`.

**BASE `eaff921d` (unfixed)** — the committed live test transplanted verbatim as
`tests/f21bwo_base_red.rs` (base worktree only, never committed):

```
thread '…' panicked at crates/wcore-sandbox/tests/f21bwo_base_red.rs:70:5:
bwrap aborted on the overlapping deny pair (21-C3-01):
  stderr="bwrap: Can't mkdir /tmp/.tmpGQI7qq/.git: Read-only file system\n"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

It reached arm 2, which means **arm 1 (the no-deny leak control) PASSED** — the probe
could read both secrets — so the abort is not an artifact of a dead instrument.

**HEAD `a9902ed5` (fixed)** — `cargo test -p wcore-sandbox --lib`:

```
test result: ok. 89 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.35s
```

All nine additions named and executed:
`absent_ancestor_is_dropped_and_does_not_subsume_its_descendant`,
`disjoint_denies_are_left_untouched`, `deny_reduction_is_independent_of_input_order`,
`exact_duplicate_deny_collapses_to_one_mount`,
`nested_directory_deny_collapses_onto_its_ancestor`, `non_directory_deny_subsumes_nothing`,
`nested_file_and_deep_chain_collapse_to_the_outermost_directory`,
`string_prefix_sibling_is_not_treated_as_nested`, and the live
`required_live_bwrap_overlapping_deny_runs_shell_and_still_contains` — all `ok`.

---

## T4 — known-negatives, on both platforms

Uncommitted harnesses (deleted after the run) drive the committed tests' arm 2 with an
injected deny list and report the three observables:

Linux, `cargo test -p wcore-sandbox --test f21bwo_kn -- --nocapture` at head:
```
KN as_committed : shell_ran=true parent_leak=false git_leak=false abort=false
KN drop_ancestor: shell_ran=true parent_leak=true  git_leak=false abort=false
KN no_deny      : shell_ran=true parent_leak=true  git_leak=true  abort=false
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

macOS 26.3, `cargo test -p wcore-sandbox --test f21bwo_macos_kn -- --nocapture`:
```
KN-macos as_committed : shell_ran=true parent_leak=false git_leak=false
KN-macos drop_ancestor: shell_ran=true parent_leak=true  git_leak=false
KN-macos no_deny      : shell_ran=true parent_leak=true  git_leak=true
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`drop_ancestor` matters twice: it proves the parent-containment assertion **can** fire, and
`git_leak=false` in the same row proves the two assertions are **independent** — `.git` is
still enforced on its own when the ancestor is removed, so the ancestor mask is not the only
thing doing the work.

## T5 — macOS: no analogue (MEASURED, not inferred)

`crates/wcore-sandbox/src/backends/sandbox_exec.rs`, new
`overlapping_read_deny_runs_shell_and_still_contains`. Production `build_profile` +
production `execute`, three-way assertion that both nested denies reach the profile as
independent SBPL rules.

Run under the LANE-BRIEF §0 **Darwin-behaviour exception** — single crate, single filtered
test, on the Mac, because `sandbox-exec` is macOS-only and no permitted build host runs
macOS. **Disclosed as required.**

```
$ /Users/seandonahoe/.cargo/bin/cargo test -p wcore-sandbox --lib \
    overlapping_read_deny_runs_shell_and_still_contains -- --nocapture
test backends::sandbox_exec::tests::overlapping_read_deny_runs_shell_and_still_contains ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 104 filtered out; finished in 0.05s
```

**macOS TOLERATES the overlapping pair**: the shell ran and both denies were enforced. SBPL
`(deny file-read* (subpath …))` rules are independent last-match-wins predicates; there is no
mount, so there is nothing to abort.

## T6 — THE UNMASKING (this is the headline)

`cargo test -p wcore-cli --test child_authority_corpus` at head `a9902ed5` on `hetzner-dsm`:
**29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** in 24.29 s.

The `corpus_tool` **standalone × live** cell — the exact cell 21-C3 §4 recorded as masked by
this defect — has flipped, through the **real `wayland-core` binary in a headless PTY**:

| cell | at `fde83e9a` (21-C3, pre-fix) | at `a9902ed5` (post-fix) |
|---|---|---|
| `corpus_tool` linux standalone live | **NOT-EXPRESSIBLE** — "the delegated child's shell never ran"; `Exit code: 1 … bwrap: Can't mkdir …/workspace/.git` | **REFUSED** |

Verbatim, post-fix:

> `corpus_tool :: linux :: standalone :: live :: REFUSED :: obtained no Bash effect :: the
> delegated child's SHELL RAN — its stdout marker returned on the wire in a served provider
> request — and its write still produced no effect in the hermetic home. 2 delegated child
> provider turn(s) arrived. ATTRIBUTED TO WORKSPACE CONTAINMENT, NOT TOOL AUTHORITY. The same
> shell command's write to a RELATIVE path — inside the child's own workspace, where
> containment has nothing to bind — succeeded and returned its marker on the wire. So the
> child demonstrably HELD and exercised Bash, and only the out-of-workspace destination was
> refused.`

**This is the both-halves proof at the shipped-binary level, and it is not my instrument —
it is the phase's own corpus.** Half 1: the child's shell RAN, its marker returned on the
wire, and a relative-path write inside its own workspace SUCCEEDED. Half 2: the
out-of-workspace write still produced NO effect. The child holds and exercises Bash and is
still contained.

Three things follow, and the third cuts against the fix looking better than it is:

1. One of 21-C3-02's four masking mechanisms is **gone**. This cell was never measuring
   enforcement; now it is.
2. The refusal is attributed to **workspace containment** — which IS one of the two mechanisms
   `21-04-PHASE-VERDICT.md` named. 21-C3 §4 said neither named mechanism was among the four
   causes. Post-fix, on this one cell, the verdict's mechanism is the real one. The verdict
   was right about the mechanism and wrong that it had been measured.
3. **The tool dimension is still NOT proved enforced.** The corpus says so itself in the same
   row: the refusal is containment, not tool authority. My fix removed the shell-can't-run
   limit (21-C3-04's first half); the checkout-root limit (its second half) stands.

`corpus_tool` **host-protocol × live** is still NOT-EXPRESSIBLE, cause "the delegated child's
shell never ran" — that cell is masked by the **confirmer** (21-C3-03), a different mechanism
this fix does not touch. Expected, and it is the control that shows the flip above is
specific to the bubblewrap path rather than a general loosening.

## T7 — Windows: no analogue (MEASURED, not inferred)

The finding lane explicitly did NOT check Windows (21-C3-06). `live_fs_acl.rs` gains
`overlapping_directory_denies_run_the_command_and_still_contain`, three arms so that
allow-then-deny ORDERING (arm 2) and NESTING (arm 3) cannot be conflated, behind a no-deny
instrument control (arm 1). `NATIVE_ACCEPTANCE_CASES` 11 → 12 so the zero-execution gate
stays honest.

Run on `SeanD@seandesktop`, repo + `CARGO_TARGET_DIR` under `D:\lane-f21bwo` per LANE-BRIEF
§6. Seed files stay under `%PUBLIC%` — that is the existing 11 cases' deliberate choice (a
shallow AppContainer-traversable ancestor chain); moving them to `D:\` risks a false negative
from an untraversable ancestor rather than from the deny. They are text files of a few bytes.

Sentinel pattern per §3.2 — remote writes a status file, a separate ssh call reads it back,
exit status ignored:

```
WLRC_TEST=0
WLRC_KN=0
WLSHA=a791979c9a6d5559428308d69613455f10725663
WLDONE
```

Counts read back from the log, never from exit status:

```
Running tests\live_fs_acl.rs
test overlapping_directory_denies_run_the_command_and_still_contain ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.41s

Running tests\f21bwo_kn_win.rs
KN-win as_committed : shell_ran=true parent_leak=false git_leak=false
KN-win drop_ancestor: shell_ran=true parent_leak=true  git_leak=false
KN-win no_deny      : shell_ran=true parent_leak=true  git_leak=true
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.38s
```

**Windows AppContainer TOLERATES the overlapping pair.** The command ran, both nested denies
were enforced, and no AppContainer ace survived on either object. `apply_protected_deny`
strips package ALLOW aces and sets `PROTECTED_DACL_SECURITY_INFORMATION` **per object**;
there is no mount, so there is nothing to abort. Known-negatives fire identically to the
other two platforms.

**All three backends now measured. Only bubblewrap had the defect, and only because it is the
only mount-based one.**

### T7-a — two instrument defects found in my own harness during this leg (§6b-ii: repaired, not noted)

1. **`\$` in a bash double-quoted ssh argument escapes the dollar.** I read
   `"D:\lane-f21bwo\$NONCE\status.txt"` — bash left `$NONCE` literal, PowerShell then expanded
   it as an undefined variable to empty, and the path collapsed to
   `D:\lane-f21bwo\status.txt`: **a different, older, already-passing status file.** It
   reported `WLRC=0 / WLDONE` and looked like my run succeeding. Repaired by building the path
   in bash first (`DIR="D:/lane-f21bwo/${NONCE}"`) and using forward slashes. This is the
   self-passing-instrument class exactly — a stale green from a path that was never mine.
2. **`SeanDesktop` is shared and a sibling instance of this lane was live on it.** A
   `status2.txt` I had just written was found carrying another writer's format. Repaired by
   scoping every artifact to a per-run nonce directory (`D:\lane-f21bwo\r3x222112\`), which is
   the §6a-ii `/tmp` rule applied to the Windows box. Every Windows number above comes from
   that nonce directory and from a run whose `WLSHA` I read back and matched to my branch HEAD.
