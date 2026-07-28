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

---

## What was TAKEN

### T1 — `%TEMP%` scratch narrowing (`db391a0a`, CR-2 + CR-3)

`scratch_dirs()` now takes a `WorkspaceTrust` and returns a bounded directory inside the
temp tree instead of the temp tree itself. Three things beyond a literal re-author:

1. **Keyed by trust (CR-3).** `trusted_local` and `contained` get sibling directories,
   never nested and never equal. #254 used one fixed name for both, which would have
   handed an untrusted session a writable host directory a trusted session reads back —
   a trust-crossing channel *created by the narrowing*.
2. **Fails closed.** If the directory cannot be established the grant is empty, never a
   fallback to `%TEMP%`. A fallback would silently restore the defect.
3. **Unix squat check.** `temp_dir()` is the shared world-writable `/tmp` there, so the
   uid goes in the *top* component (a shared parent would let the first user to create it
   own the permissions for everyone else), and since `create_dir_all` follows symlinks we
   verify we got a real directory we own before granting a write ACE to it.

### T2 — `\\?\` cwd strip (`a870ba8b`, CR-1)

The cwd computation is extracted into `resolve_cwd()` so the exact UTF-16 buffer handed to
`lpCurrentDirectory` is assertable, and VerbatimDisk is rewritten to its ordinary
drive-letter spelling. Verbatim-UNC, device and plain UNC are left byte-identical — those
name genuinely remote objects and stripping their prefix would change which object is
named. `is_verbatim_disk_path` was un-gated from `#[cfg(test)]`; it was already the right
classifier, just unreachable from production.

The strip runs on the wide encoding rather than a `to_str()` round-trip, so a non-UTF-8
filename is handled exactly instead of silently passing through unstripped.

**Not a sandbox widening:** `\\?\C:\a` and `C:\a` name the same filesystem object, and the
AppContainer allow/deny ACEs are applied to the object, not to the spelling. The MAX_PATH
objection the package pre-emptively killed (§3.3) stays killed — a Win32 process working
directory is MAX_PATH-limited whichever spelling is passed.

---

## What was deliberately NOT taken

| Dropped | Why |
|---|---|
| **C1a — `$HOME` removal** | Already fixed upstream and *better*. `workspace_policy.rs:183-190` builds `readable_extra` from a curated `detect_developer_capabilities()` set; #254's version is a Windows-only `#[cfg(windows)] let readable_extra = Vec::new();` fork. Taking it would be a regression dressed as a fix. Verified still present at HEAD before dropping. |
| **C2 — `SidsToDisable`** | Superseded and worse. HEAD passes `0/null` (disables nothing), backed by the 2026-07-23 hardware matrix recorded at `windows_impl/process.rs:407-425`; #254 still marks `Administrators` deny-only. |
| **C4 — Relaxed Sandbox Mode** | Rejected as a security hole, per package §4. Its "trusted_local only" restriction exists in doc comments only, it is not implementable as written (`SandboxManifest` carries no trust field), and its two config keys use plain `project.or(global)` while `allow_no_sandbox`/`auto_approve`/`allow_list` are clamped tighten-only in the same function — so a cloned repo's `.wayland-core.toml` would disable the Windows sandbox. **No part of it was implemented.** |
| **C5 — `BashTool::description()` warning** | Out of this lane's scope (the brief names two takes). Not rejected on merit — the `cmd /C` quote-mangling it warns about is real, and I hit that exact class driving this lane over SSH (see Open items). |

---

## Red-then-green proof

Every guard below was **watched failing against the un-fixed code** before it was trusted.
The un-fixed state was produced by reverting only the fix and keeping the guard, so the
red is a measurement, not an assumption.

### T1 — scratch narrowing · Linux (`hetzner-dsm`, `/root/wayland-254-take` @ `db391a0a`)

**RED** — `scratch_dirs()` body reverted to `vec![canon(temp_dir())]`, signature kept so
call sites still compile:

```
running 3 tests
test workspace_policy::tests::scratch_dir_is_a_real_directory_we_own ... ok
test workspace_policy::tests::scratch_grant_is_bounded_not_the_whole_temp_tree ... FAILED
test workspace_policy::tests::trusted_and_contained_do_not_share_a_scratch_directory ... FAILED

panicked at workspace_policy/tests.rs:721: the whole host temp tree "/tmp" is granted writable
panicked at workspace_policy/tests.rs:765: a Contained session shares writable root "/tmp"
                                           with a Trusted session
test result: FAILED. 1 passed; 2 failed
```

Reported precisely: **two of the three guards are narrowing guards and both went red.**
The third (`scratch_dir_is_a_real_directory_we_own`) passed in the red run because it
guards the *new helper's* squat/ownership property, which the revert did not touch. It is
a guard on T1's own added surface, not a regression guard for the narrowing, and I am not
counting it as one.

**GREEN** — fix restored, same worktree, same commit:

```
test result: ok. 35 passed; 0 failed  (filtered to workspace_policy)
```

### T2 — cwd strip · Windows (`SeanD@seandesktop`, `C:\ferrox-254-take` @ `db391a0a`)

This fix is `#[cfg(windows)]`, so both legs had to run on Windows — Linux cannot exercise
it at all.

**GREEN** — `cargo test -p wcore-sandbox --lib`:

```
test backends::appcontainer::windows_impl::tests::resolve_cwd_keeps_the_absolute_and_null_contract ... ok
test backends::appcontainer::windows_impl::tests::resolve_cwd_leaves_every_other_shape_byte_identical ... ok
test backends::appcontainer::windows_impl::tests::resolve_cwd_strips_verbatim_disk_prefix ... ok
test result: ok. 129 passed; 0 failed; 23 ignored
```

**RED** — `resolve_cwd` reverted to the pre-fix `widen_os(p.as_os_str())` pass-through,
same worktree, guards untouched:

```
test ...::resolve_cwd_keeps_the_absolute_and_null_contract ... ok
test ...::resolve_cwd_leaves_every_other_shape_byte_identical ... ok
test ...::resolve_cwd_strips_verbatim_disk_prefix ... FAILED

panicked at windows_impl\tests.rs:94: assertion `left == right` failed
  left: "\\?\C:\work\repo"
 right: "C:\work\repo"
test result: FAILED. 128 passed; 1 failed; 23 ignored
```

The two negative-control cases stayed **green** through the revert, which is the point:
the guard fails on the defect specifically, not on any change to the function.

<!-- ferrox:write-continue -->
