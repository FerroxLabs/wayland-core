# Pre-measurements taken BEFORE the Windows soak, and why each one was taken

Each of these answers a question of the form "what would make this run unreadable?", asked
*before* the run rather than diagnosed after a red. Two of them changed what this lane did.

---

## 1. Does `--json-stream` emit protocol bytes on Windows?

**Why it matters:** every tenth soak session is tier-2 (`--json-stream`, stdin closed), so
100 of the 1,000 sessions ride on it. Tier-2 correctness is defined as *"it produced protocol
bytes and did not panic"*. If the surface emitted nothing on Windows, all 100 would score
incorrect, the run-level rate would be 0.90 against a **0.99 floor**, and the soak would go
RED on quality — a red that would have been a harness artifact, not a product defect.

**Result: it emits.**

```
ARGV=--json-stream
STATUS=1 SIGNAL=null
MS=57
STDOUT_BYTES=496 STDERR_BYTES=588
STDOUT_HEAD="{\"type\":\"error\",\"error\":{\"code\":\"init_failed\",\"message\":\"Engine failed
to start during init: No API key found. …\"
```

496 bytes of real protocol JSON, exit in 57 ms, no panic sentinel. Tier-2 scores correct.

### The first attempt at this probe was WRONG, and is kept here because it would have lied

The first probe used PowerShell `Start-Process -RedirectStandardInput`, and reported:

```
JSONSTREAM_KILLED_AT_15s
JSONSTREAM_STDOUT_BYTES=0
JSONSTREAM_STDERR_BYTES=0
```

Read at face value that says *"`--json-stream` hangs and emits nothing on Windows"* — a HIGH
product finding. It is false. The probe redirected stdin from a file it created **after**
`Start-Process` had already been called, so the child never saw the stdin close that makes
the surface exit.

The corrected probe drives the binary through **`spawn(...)` with `stdio:['pipe','pipe','pipe']`
followed immediately by `child.stdin.end()` — byte-for-byte the mechanism
`f28-native-soak.mjs::runSession` uses**. A probe that does not use the harness's own
mechanism is not measuring what the harness will measure. Had this not been re-run, this lane
would have filed a fabricated HIGH defect against the candidate.

---

## 2. Do tier-1 and tier-3 surfaces behave?

```
HELP_EXIT=0        HELP_BYTES=28260
TIER1 argv='session list'  rc=0 bytes=1594
TIER1 argv='node identity' rc=0 bytes=450
TIER1 argv='skill list'    rc=1 bytes=1804
```

Exercised on a COPY of the candidate at a distinct path
(`C:\wl-winrequeue\probe\wayland-core.exe`), never the census-bound
`C:\wl-winrequeue\in\wayland-core.exe`, so nothing this probe spawned could be miscounted as
an orphan by the real run. `skill list` exiting 1 is classified by the harness's own warm-up
schema, not by this probe.

---

## 3. Can the digest gate go red?

The runner refuses to soak a binary that is not the ledger-bound candidate. Proven by running
it against a deliberately wrong expected digest:

```
F28_SOAK_BINARY_SHA256 54b12e8e5576ee54e88a93975c360e6c624202059f449d80574b71adf00c631e
F28_SOAK_LEDGER_SHA256 deadbeef00000000000000000000000000000000000000000000000000deadbeef
F28_SOAK_EXIT=91 reason=digest-mismatch
```

The real binary on the host hashes to `54b12e8e…c631e`, which is exactly the
`x86_64-pc-windows-msvc` row of 28-03's target manifest, so the candidate binding is asserted
**on the host** before the first session rather than assumed from a document.

---

## 4. Can `touch` run on Windows? (leg 3 precondition)

The 26-02 quarantine payload's shell directive is `touch <sentinel>`, and `touch` is not a cmd
builtin. If it did not resolve, the **positive control could not fire** and a t20 failure would
mean "the fixture cannot run here", not "containment leaked".

```
where touch  -> C:\Program Files\Git\usr\bin\touch.exe   (rc=0)
cmd /C "touch <path>"  -> rc=0, file created: True
```

So the positive control is capable of firing on this host. It is re-asserted at run time inside
`quarantine.ps1`, because SYSTEM and the interactive user do not share a PATH.

---

## 5. Is the competing load real, or resident?

The quiet-window wait would never fire if some cargo/rustc process simply sits resident. Measured
over a 6-second window:

```
BUSY pid=11668 name=cargo  cpu_total=6      cpu_delta_6s=0
BUSY pid=12124 name=cargo  cpu_total=6.1    cpu_delta_6s=0
BUSY pid=38920 name=rustc  cpu_total=1201.4 cpu_delta_6s=5.97
BUSY pid=39712 name=rustc  cpu_total=190.1  cpu_delta_6s=5.98
BUSY pid=43976 name=rustc  cpu_total=138.6  cpu_delta_6s=6
```

The rustc processes are genuinely working (≈1 core each); the cargo parents are idle waiting on
them. So the load is real CI work that ends, not a resident process that would deadlock the
wait. The load is GitHub Actions self-hosted runner work — the toolchain path is
`C:\WINDOWS\ServiceProfiles\NetworkService\.rustup\…`, i.e. the runner service account, not
another lane.
