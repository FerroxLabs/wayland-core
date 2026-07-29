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
