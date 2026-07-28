# CORE #254 — what was taken, what was not

**Lane:** `lane/254-take` · base `plan/f20-unified-audit-repair` @ `14905684`
**Spec:** `.planning/intel/CORE-254-MAINTAINER-PACKAGE.md` (SPLIT-AND-TAKE-PART, 4/4 panel)
**Contributor:** `frankforges` found both defects below and proposed the split himself.

> This lane took **no GitHub action of any kind** on #254 — no merge, comment, review,
> close, or push to the contributor's branch. All of those are Sean's. The fixes below are
> re-authored from scratch against the integration branch, because #254's base (`61b79c4f`)
> predates the `appcontainer/` module split and cannot be conflict-resolved (package §2).

---

## Stage 0 — both defects independently reconfirmed live at HEAD `14905684`

Before writing anything I re-read both sites in this worktree. Both bugs are present.

**Defect A — `%TEMP%` granted wholesale.** `crates/wcore-tools/src/workspace_policy.rs:935-938`:

```rust
fn scratch_dirs() -> Vec<PathBuf> {
    let tmp = std::env::temp_dir();
    vec![canon(tmp)]
}
```

The entire host temp tree is handed out as a writable root. Called from exactly two sites,
and this is the CR-3 problem the package flags:

- `:173` — `WorkspacePolicy::trusted_local` (`let mut writable_extra = scratch_dirs();`)
- `:242` — `WorkspacePolicy::contained`   (`let writable_extra = scratch_dirs();`)

A `Contained` (untrusted/remote) session and a `Trusted` local session therefore receive a
write grant to the *same* host directory.

**Defect B — `\\?\` cwd passed through to Win32.**
`crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs:387-395`:

```rust
let cwd_w: Option<Vec<u16>> = match cmd.cwd.as_ref() {
    Some(p) => {
        if !p.is_absolute() { return Err(...); }
        Some(widen_os(p.as_os_str()))     // <- unmodified
    }
    None => None,
};
```

`std::fs::canonicalize` returns the verbatim `\\?\C:\…` spelling for every local path on
Windows, so a canonicalized cwd reaches `lpCurrentDirectory` in a form `cmd.exe` treats as
UNC and silently replaces with `C:\Windows`.

**Two helpers already exist in-tree but neither is reachable from production code:**

- `windows_impl/command.rs:223` `is_verbatim_disk_path()` — correct `Prefix::VerbatimDisk`
  classifier, but `#[cfg(test)]`-gated.
- `acl_lease/storage.rs:731` `strip_verbatim()` — correct behaviour, but declared *inside*
  that file's `mod tests`, so it is test-local.

<!-- ferrox:write-continue -->
