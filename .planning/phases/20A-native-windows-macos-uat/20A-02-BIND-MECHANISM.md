# 20A-02 — AppContainer retained-workspace-authority bind: mechanism evaluation

**Status: COMPLETE for the decision checkpoint. Mechanism A measured in full, including the
production-shaped destructive-cleanup re-measurement that the first pass got wrong.
Mechanisms B and C are `NOT-EVALUATED-NOT-NEEDED` under the amended stop-at-first-qualifying
rule. Nothing was shipped; `crates/` carries no production change and no probe.**

Hardware: `SEANDESKTOP`, NTFS, `C:\ferrox-win` @ `0f3b8b49`. All probes throwaway, appended to
`directory_authority_windows_tests.rs` on the box, run, and reverted — `git status --porcelain`
is EMPTY on the box at the end of this task.

---

## 1. The retained authority's ACTUAL open — read from source, confirmed by probe

`crates/wcore-sandbox/src/directory_authority_windows.rs`, `open_directory()` (lines 68-83),
verbatim:

| Parameter | Value |
|---|---|
| Access mask | `GENERIC_READ \| GENERIC_WRITE \| DELETE` (`0xC0010000`) |
| Share mode | `FILE_SHARE_READ \| FILE_SHARE_WRITE \| FILE_SHARE_DELETE` (`7`) |
| Create options | `FILE_FLAG_BACKUP_SEMANTICS \| FILE_FLAG_OPEN_REPARSE_POINT` (`0x02200000`) |
| Disposition | `OPEN_EXISTING` |

Confirmed by probe: a `CreateFileW` with exactly these parameters against a live directory
succeeds and returns a handle. Source and runtime agree.

**The share mode INCLUDES `FILE_SHARE_DELETE`.** By RULE 1 of the measured rename truth table
that is precisely the condition under which a handle does *not* pin its own object.

### The distinction that the whole evaluation turns on

State it precisely, because the loose version of it produced a wrong first pass. **As shipped,
the retained handle DOES pin the underlying OBJECT — it keeps that object alive and every
handle-relative operation keeps reaching it. What it fails to pin is the NAME.** The pathname
can be renamed away or unlinked out from under the handle, after which the pathname resolves
to something else entirely while the handle still reaches the original object.

That asymmetry is exactly why a path-form `CreateProcess(lpCurrentDirectory = ...)` bind is
unsafe as shipped, and exactly what narrowing the share mode changes: narrowing extends the
pin from the object to the name.

### A measurement trap that invalidated the first probe run

The first probe was **discarded, not reported**. PowerShell 5.1 parses `0x80000000` as a
*negative* `Int32`, so every `CreateFileW` access-mask argument failed conversion to `UInt32`,
the authority handle was never opened, and the junction/symlink primitives were measured
**with no handle held** — they "succeeded" for the trivial reason that nothing was holding
anything. Rebuilt with `[Convert]::ToUInt32('80000000',16)`. Every result in §2 is from the
corrected run, where the handle open is asserted before the primitive is attempted.

Every measurement in §2.6 and §2.7 goes further and abandons PowerShell entirely: it is a
Rust probe compiled into the `wcore-sandbox` crate on the box, calling the REAL production
functions. That construction makes "did the handle actually open" unfalsifiable rather than
merely asserted, and it makes "is this the production shape" true by construction rather than
by imitation.

---

## 2. Mechanism A — the OS-enforced pin

### 2.1 Two-column enumeration of pathname-redirection primitives (all PROBED)

**AS SHIPPED — access `GR|GW|DELETE`, share `READ|WRITE|DELETE`:**

| Primitive | Result | DEFEATS / DOES NOT DEFEAT |
|---|---|---|
| rename the RETAINED directory itself | **SUCCEEDED** | **DOES NOT DEFEAT** |
| rename an ANCESTOR | REFUSED `err=5` (ACCESS_DENIED) | DEFEATS |
| delete-and-recreate: delete RETAINED (empty) | **SUCCEEDED** | **DOES NOT DEFEAT** |
| delete-and-recreate: delete ANCESTOR (after retained gone) | **SUCCEEDED** | **DOES NOT DEFEAT** |
| replace ANCESTOR with a JUNCTION / mount point | REFUSED `err=5` | DEFEATS (only because the ancestor *move* is blocked) |
| replace ANCESTOR with a SYMBOLIC LINK | REFUSED `err=5` | DEFEATS (same reason) |

**NARROWED — access `GR|GW|DELETE`, share `READ|WRITE` (no `FILE_SHARE_DELETE`):**

| Primitive | Result | DEFEATS / DOES NOT DEFEAT |
|---|---|---|
| rename the RETAINED directory itself | REFUSED `err=32` (SHARING_VIOLATION) | DEFEATS |
| rename an ANCESTOR | REFUSED `err=5` | DEFEATS |
| delete-and-recreate: delete RETAINED (empty) | REFUSED `err=32` | DEFEATS |
| delete-and-recreate: delete ANCESTOR | REFUSED `err=145` (DIR_NOT_EMPTY) | DEFEATS (consequent — retained survives, so ancestor stays non-empty) |
| replace ANCESTOR with a JUNCTION / mount point | REFUSED `err=5` | DEFEATS |
| replace ANCESTOR with a SYMBOLIC LINK | REFUSED `err=5` | DEFEATS |

### 2.2 The finding that the rename-only truth table could not have produced

The first pass showed ancestor-delete refused with `err=145`. **That refusal is structural,
not handle-enforced** — the directory merely happened to be non-empty. Re-probed with the
retained directory EMPTY, and under the shipped share mode:

```
AS SHIPPED share=R|W|D  RemoveDirectory(RETAINED itself) => SUCCEEDED err=0
AS SHIPPED share=R|W|D  RemoveDirectory(ANCESTOR)        => SUCCEEDED
```

**The retained workspace NAME can be unlinked out from under a held authority handle.** It
renames nothing, so no rule in the rename truth table covers it, and reading `err=145` as a
pin would have been a false safety argument carried into the checkpoint.

### 2.3 CORRECTED — the narrowing does NOT break handle-bound destructive cleanup

**The first pass recorded this as the open question gating the mechanism, and it recorded it
WRONG.** The measurement behind it opened a FRESH `DELETE`-intent handle to the workspace
while the authority was held, and reported the resulting `err=32` as "the narrowing costs the
handle-bound destructive cleanup".

That is not the shape production uses. Read from source:

- `remove_open_dir_all` (`directory_authority_windows.rs:719`) `Arc::try_unwrap`s the
  **RETAINED** handle and passes it straight to `delete_open_object`.
- `delete_open_object` → `mark_open_object_for_delete` (`:415`) sets `FILE_DISPOSITION_INFO_EX`
  / `FILE_DISPOSITION_INFO` **on that same retained handle**.
- `rename_handle_into` (`:605`) calls `NtSetInformationFile(FileRenameInformationEx)` **on the
  retained source handle**, with the destination named by the target parent's retained handle.

**Production never opens a fresh DELETE-intent handle to an object it already holds.** The
`err=32` was an artifact of probing a shape the code does not use.

### 2.4 A separate measured correction to the source record

`open_directory_observational`'s doc comment states that a `DELETE`-bearing handle blocks
`SetCurrentDirectory` into that directory with a sharing violation. Probed:

```
WITH DELETE access     CreateProcess(cwd=retained) => OK, child reports ...\ws
WITHOUT DELETE access  CreateProcess(cwd=retained) => OK, child reports ...\ws
```

`CreateProcess`'s `lpCurrentDirectory` is **not** blocked by a held `DELETE`-bearing handle, in
either share mode. The documented failure is real but specific to *in-process*
`SetCurrentDirectory` (which git/MSYS call), not to the process-creation path a bind uses.

### 2.5 Mechanism A verdict — as shipped

- **As shipped: DOES NOT QUALIFY.** Two probed primitives redirect the pathname — rename of the
  retained directory, and delete-and-recreate of the retained directory or its ancestor.

### 2.6 THE DECISIVE MEASUREMENT — narrowed authority, real production code paths

Rust probe compiled into `wcore-sandbox` on the box with `open_directory` narrowed to
`share = FILE_SHARE_READ | FILE_SHARE_WRITE`, calling the real production functions. Verbatim
probe output:

```
===== 20A-02 NARROWED-AUTHORITY PRODUCTION-SHAPE PROBE =====
OP1a external rename of retained NAME     => REFUSED err=Some(32) (PINNED)
OP1b external unlink of retained NAME     => REFUSED err=Some(32) (PINNED)
OP2  remove_descendants (nested+read-only) => OK
OP3  remove_open_dir_all (disposition on RETAINED handle) => OK
OP4a rename_into DIRECTORY via retained handle => OK (landed=true)
OP4b atomic_write_child (file publish rename) => OK (bytes=Some(b"published"))
OP5a substitution attempt before spawn   => Some(Some(32))
OP5b CreateProcess(cwd=display_path)     => OK status=Some(0)
     child_cwd="C:\Users\seand\AppData\Local\Temp\.tmpt6UyZl\ws"
OP5c child_names THROUGH RETAINED HANDLE => ["proof.txt"]
============================================================
```

Reading it operation by operation:

| Operation | Production path exercised | Result |
|---|---|---|
| OP1a | external `rename` of the pinned NAME | REFUSED `err=32` — **the pin holds** |
| OP1b | external `unlink` of the pinned NAME | REFUSED `err=32` — **the pin holds** |
| OP2 | `remove_descendants` over nested dirs + a read-only file child | **OK** |
| OP3 | `remove_open_dir_all` — disposition set on the RETAINED handle | **OK** |
| OP4a | `rename_into` — handle-relative `NtSetInformationFile` via the retained handle | **OK** |
| OP4b | `atomic_write_child` — the file-publish rename | **OK** |
| OP5b | `CreateProcess(lpCurrentDirectory = display_path)` under the pin | **OK**, exit 0 |
| OP5c | child's write read back **through the retained handle** | `["proof.txt"]` |

OP5 is the bind itself, proven end to end: a substitution against the bound working directory
was attempted and **refused (`err=32`)**, the child then ran, and the file the child created
is visible through the RETAINED handle — so the child provably operated on the retained
object, not on any substitute.

The same narrowing was also measured against the full `wcore-sandbox` suite on the box:
**135 tests run: 129 passed, 6 failed, 45 skipped** (baseline 135/135/0/45). Every one of the
production destructive and rename proofs PASSED under the narrowing, by name:

```
PASS cleanup_refuses_outstanding_handle_loan_then_retries_same_authority
PASS windows_destructive_removal_succeeds_through_a_directory_child
PASS windows_destructive_removal_succeeds_through_a_read_only_file_child
PASS windows_handle_relative_delete_rejects_same_path_replacement
PASS created_directory_rolls_back_every_post_create_validation_failure
PASS created_file_rolls_back_every_post_create_validation_failure
PASS concurrent_atomic_write_exposes_only_whole_old_or_new_payloads
PASS root_mutation_authority_supports_directory_durability
PASS read_only_child_open_does_not_require_or_receive_delete_authority
```

**§2.3's "the narrowing costs the handle-bound destructive cleanup" is disproved on hardware.
It costs nothing of the sort.**

### 2.7 The pin CANNOT be scoped to the bind — measured, not reasoned

The obvious way to avoid a global change is a **process-lifetime name lease**: leave
`open_directory` alone and, only for the duration of a bound execution, acquire a SECOND
handle to the same object — opened HANDLE-RELATIVELY so no pathname is ever resolved — whose
share mode omits `FILE_SHARE_DELETE`. The existing `bind_command_cwd` error message already
names this idea verbatim ("without a process-lifetime name lease").

Probed with `NtCreateFile(RootDirectory = retained handle, ObjectName = empty)` against an
as-shipped authority:

```
===== 20A-02 NAME-LEASE PROBE v2 (authority AS SHIPPED) =====
LEASE attrs-only     share=R|W   => OPENED
      rename NAME        => SUCCEEDED (NO PIN)
      unlink NAME (tree) => SUCCEEDED (NO PIN)

LEASE GENERIC_READ   share=R|W|D => OPENED
      rename NAME        => SUCCEEDED (NO PIN)
      unlink NAME (tree) => SUCCEEDED (NO PIN)

LEASE GENERIC_READ   share=R|W   => REFUSED NTSTATUS 0xC0000043 win32=32
LEASE DELETE         share=R|W   => REFUSED NTSTATUS 0xC0000043 win32=32
=============================================================
```

The result is a clean pincer, and it closes the option:

- A lease requesting a **share-arbitrated** access (`GENERIC_READ`, or `DELETE`) with
  `share = R|W` is **REFUSED `STATUS_SHARING_VIOLATION` (0xC0000043 / win32 32)** — because the
  new open's share mode must permit the `DELETE` access the retained handle already holds, and
  it does not.
- A lease that **does** open — attributes-only, or one that shares delete — delivers **NO PIN**.
  An attributes-only open requests none of READ/WRITE/DELETE data access, so it neither is
  checked against, nor contributes to, share arbitration at all.

**So while the retained handle holds `DELETE` access, nothing else can pin its name.** The pin
must come from the retained handle's OWN share mode, which means `open_directory` narrows
globally. The blast radius cannot be reduced by scoping.

A second scoping attempt also failed. The delegated checkout authority is opened
`open_observational` (`wcore-swarm/src/worktree_manager.rs:960` and `:1334`), which holds only
`GENERIC_READ` and therefore has no `DELETE` grant to conflict with. Narrowing THAT open
instead was probed: with the narrowed observational handle held, opening the OWNER directory
as a mutating authority is **REFUSED `err=32`** — and that open is the first step of
`RetainedWorkspaceAuthority::new`'s identity re-proof, so this scoping breaks the workspace
authority constructor outright. Not viable.

### 2.8 Mechanism A verdict — narrowed

**NARROWED (drop `FILE_SHARE_DELETE` from `open_directory`): QUALIFIES WITH RESIDUAL RISK.**

It binds, and the anti-swap property is enforced by the OPERATING SYSTEM rather than by our own
re-check, so there is **no residual window at all** between resolution and the child's first
filesystem operation: the pin is established when the authority is opened and holds for as long
as the handle is held, which spans the entire bound execution. The pin defeats all six probed
pathname-redirection primitives. Production destructive cleanup, handle-relative rename, file
publish and rollback all keep working, measured per operation.

The residual risk is real, and it is NOT the one anticipated at the start:

- **R1 — six existing tests become unconstructible, two of them security regression guards.**
  Under the narrowing the `wcore-sandbox` suite goes 135/129/6. All six fail at their SETUP,
  on the `err=32` the pin now returns, because each constructs the very swap the pin refuses:
  `retained_parent_routes_children_after_path_replacement` (`:119`),
  `windows_handle_relative_rename_stays_bound_to_target_parent` (`:263`),
  `windows_handle_relative_file_publish_stays_bound_to_target_parent` (`:301`),
  `windows_command_cwd_stays_bound_to_renamed_directory_object` (`:630`), and
  `authority_boundary_tests::{streaming,buffered}_authority_rejects_same_path_replacement_before_backend`
  (`lib.rs:1036`).
  Their INVARIANTS survive a fortiori — the swap goes from survivable to impossible, which is
  strictly stronger. Their CONSTRUCTIONS do not: you cannot build a decoy at a name the OS
  will not let you move. Two of the six are documented as the only guard against reintroducing
  the `RootDirectory = NULL` + full-pathname rename form — the defect whose source comment
  records that it "silently disabled `atomic_write_child` … for the whole life of the Windows
  port". **Shipping the narrowing keeps that guarantee in the code and removes its proof.**
- **R2 — the residual cannot be reduced by scoping.** §2.7 measured both scoping routes closed.
  Accepting the mechanism means accepting the global narrowing.
- **R3 — NTFS-local.** Every rule here was measured on this box's default NTFS volume. ReFS,
  FAT and SMB workspaces are unproven.

---

## 3. Mechanism B — post-spawn identity re-proof

**NOT-EVALUATED-NOT-NEEDED.** Mechanism A (narrowed) reached QUALIFIES WITH RESIDUAL RISK, and
the amended plan rule stops the evaluation at the first mechanism that qualifies. This is a
recorded decision to stop, not an unmeasured gap. It carries no verdict and is therefore not
selectable at the checkpoint.

## 4. Mechanism C — handle transport

**NOT-EVALUATED-NOT-NEEDED.** Same reason as §3. Not selectable at the checkpoint.

## 5. Fourth mechanism

Not evaluated. Under the amended rule the fourth-mechanism allowance unlocks only if all three
named mechanisms are `DOES NOT QUALIFY`, which is not the case.

---

## 6. What the decision checkpoint now needs from Sean

The measurement the checkpoint was blocked on is done, and it went the *good* way: the cost
that was feared (handle-bound destructive cleanup) **does not exist**, and the bind demonstrably
works with the anti-swap property intact and no residual window.

A different cost surfaced in its place, and it is a security-relevant one that was never put in
front of Sean:

> **Shipping the narrowing requires rewriting six existing tests, two of which are the only
> regression guard on the `RootDirectory = NULL` pathname-form rename defect. That guard cannot
> be rebuilt under the pin, because the substitution it constructs is exactly what the pin
> refuses.**

This is the trade to authorize: **gain** an OS-enforced anti-swap pin on the delegated
workspace's NAME (which closes the 15 red Windows tests and two red native proof targets), and
**lose** the constructibility of the anti-swap regression proof on the rename DESTINATION.

No production code has been written. `crates/` is unmodified in the repo and on the box.
