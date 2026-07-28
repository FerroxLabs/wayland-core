# F-WR-06 — exit status over ssh+PowerShell: the collapse, and the pattern that survives it

**Measured on `SeanD@seandesktop` 2026-07-28**, host load at the time: 312 processes,
CPU 12%, 2 foreign `cargo`/`rustc` processes (another lane building). Load does not bias
this measurement in either direction — exit-status propagation is not load-sensitive.

## The collapse is real, and it is at the ssh boundary — NOT in PowerShell

The previous lane recorded "every non-zero exit collapses to 1 over ssh+PowerShell". That is
correct at the boundary, but the root cause is narrower than the wording suggests, and the
distinction is what makes a workaround possible.

**Inside PowerShell, `$LASTEXITCODE` is perfectly faithful:**

```
NAIVE want=0   LASTEXITCODE=0
NAIVE want=1   LASTEXITCODE=1
NAIVE want=2   LASTEXITCODE=2
NAIVE want=3   LASTEXITCODE=3
NAIVE want=7   LASTEXITCODE=7
NAIVE want=100 LASTEXITCODE=100
NAIVE want=255 LASTEXITCODE=255
```

**Crossing the ssh session boundary is where the value dies.** Same script, `exit $rc` as its
last statement, status read by the *calling shell* on the Mac:

| want | ssh exit status | verdict |
|---|---|---|
| 0 | 0 | preserved |
| 1 | 1 | preserved (coincidence — 1 is the collapse value) |
| 2 | **1** | COLLAPSED |
| 3 | **1** | COLLAPSED |
| 7 | **1** | COLLAPSED |
| 100 | **1** | COLLAPSED |
| 255 | **1** | COLLAPSED |

So the only information that survives is the single bit `zero / non-zero`. **A Windows gate
asserting a specific exit code over ssh is asserting a value that cannot arrive**, and — worse
— a gate asserting `rc == 1` passes for *every* failure mode, including ones it was written to
distinguish from.

## Why stdout framing alone is NOT sufficient

The obvious fix is to print the status into stdout behind a sentinel and parse it. That is
**not reliable on this transport.** PowerShell over ssh splices CLIXML progress records into
the stream (`#< CLIXML` … `<Objs …>` blocks are visible in every capture below). In a 7-point
sweep, one point lost its status line while its completion marker survived:

```
want=2 | ssh_exit_status=1 | sentinel=       | marker_present=1
```

A caller trusting stdout alone would have read that as "ran, no status" — and the tempting
reading, "marker present so it completed, treat as pass", is exactly the self-passing-gate
defect. **Marker-present-with-status-absent must be graded UNREADABLE, never pass.**

## The verified pattern: file carrier, status first, marker last, read in a separate call

Producer (on the Windows host) writes the real status to a file, **status line first and
completion marker last**, so a truncated file can never present a marker without its status:

```powershell
cmd.exe /c "<the real command>"
$rc = $LASTEXITCODE
Set-Content -Path $statusFile -Value @("WLRC=$rc","WLDONE") -Encoding ASCII
```

Consumer reads that file back in a **separate** ssh invocation and ignores exit status entirely:

```powershell
$lines = @(Get-Content $statusFile -EA SilentlyContinue)
$rc    = ($lines | Where-Object { $_ -like 'WLRC=*' }) -replace 'WLRC=',''
$done  = [bool]($lines -contains 'WLDONE')
```

Grading rule for the caller — all three cases must be distinguishable:

- `WLDONE` present **and** `WLRC=<n>` present → the run completed, `<n>` is the true status.
- `WLDONE` absent → the run did not complete (still running, killed on disconnect, or crashed).
  **Not a pass and not a fail — incomplete.**
- `WLDONE` present but `WLRC` absent/unparseable → **UNREADABLE.** Never render as a pass.

### Verification, same 2/3/7/100/255 range the collapse was measured over

Written by one ssh call, read back by a second, independent one:

```
want=0   file_rc=0   marker=True
want=1   file_rc=1   marker=True
want=2   file_rc=2   marker=True
want=3   file_rc=3   marker=True
want=7   file_rc=7   marker=True
want=100 file_rc=100 marker=True
want=255 file_rc=255 marker=True
```

**7/7 faithful, including every value that collapses to 1 over the naive path.**

This pattern composes with the scheduled-task requirement (Windows OpenSSH kills session
children on disconnect): the scheduled task is the producer, the status file is the carrier,
and polling for `WLDONE` is what tells the caller the task finished — which is strictly better
than polling for process absence, because a task that never started also has no process.

## A second trap found while building this, recorded because it produced a silent empty

`"WLRC:$LASTEXITCODE:WLEND"` renders as `WLRC:7:` — the status vanishes. PowerShell parses
`$LASTEXITCODE:WLEND` as **namespace/drive notation** (the same syntax as `$env:PATH`), i.e.
variable `WLEND` in scope `LASTEXITCODE`, which is empty. A sentinel with a colon immediately
after a variable name silently loses its value and everything after it on that line.

**Always brace the variable in an interpolated sentinel: `"WLRC=${rc}"`.** This one nearly got
attributed to the transport, which would have been a fabricated finding on top of a real one.
