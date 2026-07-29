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
