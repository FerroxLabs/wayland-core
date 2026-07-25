# 20A-02 — AppContainer retained-workspace-authority bind: mechanism evaluation

**Status: INCOMPLETE — Mechanism A measured in full; Mechanisms B and C NOT EVALUATED.**
This document is NOT ready to support the Task 2 decision checkpoint. See §6.

Hardware: `SEANDESKTOP`, NTFS, `C:\ferrox-win` @ `12c0229a`. All probes throwaway
(`C:\Users\SeanD\pin-probe*.ps1`), never added to production code. `crates/` carries no
probe at the end of this task.

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

**The share mode INCLUDES `FILE_SHARE_DELETE`.** By RULE 1 of the measured rename truth
table that is precisely the condition under which a handle does *not* pin its own object.
The pin Mechanism A depends on is therefore absent as shipped — predicted from source and
then confirmed on hardware below.

### A measurement trap that invalidated the first probe run

The first probe was **discarded, not reported**. PowerShell 5.1 parses `0x80000000` as a
*negative* `Int32`, so every `CreateFileW` access-mask argument failed conversion to
`UInt32`, the authority handle was never opened, and the junction/symlink primitives were
measured **with no handle held** — they "succeeded" for the trivial reason that nothing was
holding anything. Rebuilt with `[Convert]::ToUInt32('80000000',16)`. Every result in §2 is
from the corrected run, where the handle open is asserted before the primitive is attempted.

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

**The retained workspace object can be deleted out from under a held authority handle.**
This is exactly the class of hole the plan required be probed rather than reasoned about:
it renames nothing, so no rule in the rename truth table covers it, and reading `err=145`
as a pin would have been a false safety argument carried into the checkpoint.

### 2.3 Cost of the narrowing (measured, not argued)

| Operation with the authority held | AS SHIPPED | NARROWED |
|---|---|---|
| 2nd DELETE-bearing open of the workspace | OK | **BLOCKED `err=32`** |
| fresh DELETE-intent handle (the destructive-cleanup shape) | OK | **BLOCKED `err=32`** |
| observational read-only open | OK | OK |
| read a file inside the workspace | OK | OK |
| `CreateProcess(cwd=workspace)` | OK | OK |

The narrowing buys the pin and **costs the handle-bound destructive cleanup**: while the
authority is held, no second delete-intent handle can be opened. That is threat
`T-20A-02-04` landing exactly where the threat model predicted. Whether the cleanup path can
be restructured to reuse the retained handle instead of opening a fresh one is **not
measured** and is the open question gating this mechanism.

### 2.4 A separate measured correction to the source record

`open_directory_observational`'s doc comment states that a `DELETE`-bearing handle blocks
`SetCurrentDirectory` into that directory with a sharing violation. Probed:

```
WITH DELETE access     CreateProcess(cwd=retained) => OK, child reports ...\ws
WITHOUT DELETE access  CreateProcess(cwd=retained) => OK, child reports ...\ws
```

`CreateProcess`'s `lpCurrentDirectory` is **not** blocked by a held `DELETE`-bearing handle,
in either share mode. The documented failure is real but specific to *in-process*
`SetCurrentDirectory` (which git/MSYS call), not to the process-creation path a bind would
use. This does not by itself make any mechanism work, but it removes an obstacle that the
source comment would have led a reader to assume was fatal.

### 2.5 Mechanism A verdict

- **As shipped: DOES NOT QUALIFY.** Two probed primitives redirect the pathname — rename of
  the retained directory, and delete-and-recreate of the retained directory or its ancestor.
- **Narrowed (drop `FILE_SHARE_DELETE`): QUALIFIES WITH RESIDUAL RISK.** The pin defeats all
  six probed primitives, but it breaks the handle-bound destructive cleanup (`err=32`), and
  the residual risk is **unquantified** because the cleanup restructuring was not measured.

---

## 3. Mechanism B — post-spawn identity re-proof

**NOT EVALUATED.** No probe was run. No verdict is recorded, and none should be inferred.
Required before the checkpoint: whether a child can be created held, whether its actual
working directory can be re-opened and compared by volume serial plus 128-bit file id, and
what the child can do between creation and the check completing.

## 4. Mechanism C — handle transport

**NOT EVALUATED.** No probe was run. No verdict is recorded, and none should be inferred.
Required before the checkpoint: whether the retained handle can be inherited into the
AppContainer and whether any supported mechanism establishes the working directory from a
handle rather than a pathname. A clean negative here is a real and useful result.

## 5. Fourth mechanism

Not evaluated. The one-mechanism allowance remains unspent.

---

## 6. Why this does not yet support the decision checkpoint

The checkpoint asks Sean to authorize ONE mechanism. Two of the three named candidates carry
no measurement at all, so any selection now would be a choice between one measured option
and two unknowns — which is the shape of decision the plan's termination criterion exists to
prevent. Mechanism A's own verdict is *conditional* on a cost (destructive cleanup) that is
measured as broken but not measured as unfixable.

Nothing was shipped. `crates/` carries no production change from this task and no probe.
