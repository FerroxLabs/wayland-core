# F25-05 HIGH — the Windows orphan scanner reported a MEASURED ZERO while an orphan ran

Host `SeanDesktop` (Windows), 2026-07-27. Both runs drove the shipped release
binary built from `lane/25` on that host.

A false negative in containment is the worst output this module can produce. It is
strictly worse than an error: an error is visibly absent evidence, whereas a
measured zero **reads as proof of correctness**. Everything downstream that consumed
`orphans: 0` on Windows was consuming a value that could not distinguish *none* from
*cannot see any*.

---

## RED — `tasklist`, commit `1f5cdf29`

Method line the scanner printed: `live-task registry UNION a real enumeration of the
host process table`. The enumeration was `tasklist /V /FO CSV`.

**`tasklist` does not print command lines at all.** Its columns are image name, PID,
session name, session number, memory, status, user, CPU time and window title. The
task nonce lives in the *command line*, so it was never present in the output that was
being filtered.

A real orphan was planted and confirmed present by an independent enumeration:

```
COMMAND: Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like "*f25-04-win-orphan-21476*" } | Select-Object ProcessId,ParentProcessId,CommandLine

ProcessId       : 41836
ParentProcessId : 21476
CommandLine     : "C:\WINDOWS\system32\cmd.exe" /c ping -n 600 127.0.0.1 > nul & rem
                  f25-04-win-orphan-21476

ROWS-MATCHING-NONCE: 1
```

The scanner, at the same moment, on the same host, for the same nonce:

```
=== planted=1 scannerPlanted=0 after=0 scannerAfter=0 ===
```

**Independent enumeration: 1. Scanner: 0 (MEASURED).**

### The second-order failure, which is the more instructive one

That same run wrote this into the ledger:

```
F25-SC4-SCANNER-AGREEMENT: AGREE scanner=0 manual=0
```

The agreement check ran only **after the reap**, when both values were legitimately
zero. It therefore agreed — while the scanner was incapable of ever returning anything
else. **A comparison taken only in the state where both sides are zero cannot detect a
scanner that always says zero.** That is the "gate was already green" failure class,
and it was sitting inside this plan's own evidence.

Fixed: the agreement verdict is now taken **while the orphan is planted** as well as
after the reap, so it has a state in which it can genuinely disagree.

---

## GREEN — `Get-CimInstance Win32_Process`, commit `b0bb30d5`

Same script, same host, same planted-orphan procedure:

```
=== planted=1 scannerPlanted=1 after=0 scannerAfter=0 ===
```

The scanner's own output while planted:

```
  backend    local
  mechanism  kernel-backed: ProcessTreeMechanism::WindowsJobObject — a kill-on-close Job Object owning the descendant tree
  method     live-task registry UNION a real enumeration of the host process table
  count      1 (MEASURED)
  row        process table: 41836 21476 "C:\WINDOWS\system32\cmd.exe" /c ping -n 600 127.0.0.1 > nul & rem f25-04-win-orphan-21476
```

**Independent enumeration: 1. Scanner: 1.** And after the reap, both 0.

---

## The residual the instrument swap did NOT close, and how it is closed — commit `f0778ba1`

Swapping `tasklist` for `Win32_Process` fixed *this* instrument. It did not fix the
*failure mode*, and the failure mode is the actual defect.

`Win32_Process.CommandLine` is documented to return NULL when the caller lacks
sufficient privilege for the owning process — and under some conditions it comes back
empty across the board. An enumeration that "succeeds" with every command line blank
produces **exactly the same false zero** with a different tool. Nothing in the
`tasklist` fix would have caught that.

So the instrument now **self-tests**:

> This process's own row must be present in the enumeration AND carry a non-empty
> command line. We know our own PID and we know we have a command line; if we cannot
> see our own, we cannot see anyone's.

and the return type makes the bad answer unrepresentable:

```rust
pub enum ProcessTableScan {
    Enumerated { rows: Vec<String> },
    CannotDetermine { reason: String },
}
```

There is deliberately **no `count()` on this type**. The only way to obtain a number is
through the `Enumerated` arm, so "could not look" cannot be rendered as `0` — not
merely discouraged, unrepresentable. `CannotDetermine` flows to
`OrphanEvidence::unobserved`, which the CLI prints as:

```
  count      NOT MEASURED — <reason>
```

The NULL-`CommandLine` condition cannot be induced on a Linux CI box, so it is pinned by
unit tests over captured output shapes rather than left untested:

| Test | Asserts |
|---|---|
| `windows_null_command_lines_are_cannot_determine_not_zero` | blank command lines ⇒ `CannotDetermine`, and the reason names the consequence |
| `an_enumeration_that_cannot_see_its_own_process_is_cannot_determine` | our own row absent ⇒ `CannotDetermine` |
| `a_healthy_enumeration_passes_the_self_test` | the self-test can also PASS, so it is not a constant refusal |
| `the_real_process_table_passes_its_own_self_test_on_this_host` | the live host's table is genuinely determinate |
| `cannot_determine_carries_no_count_at_all` | no number is reachable from the indeterminate arm |

"We could not reproduce it so we did not test it" is how this survived the first time.
