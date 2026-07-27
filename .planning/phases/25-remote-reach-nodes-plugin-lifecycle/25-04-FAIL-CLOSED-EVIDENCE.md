# 25-04 — Fail-closed matrix and orphan proof: live evidence

Ledgers: `evidence/25-04-fail-closed-ledger.txt` (Linux),
`evidence/25-04-fail-closed-windows-ledger.txt` (Windows).
Transcripts: `evidence/25-04-fail-closed-linux.log` (277 lines),
`evidence/25-04-fail-closed-windows.log`.

- **Linux:** `hetzner-dsm`, release binary from `lane/25` @ `f0778ba1`
- **Windows:** `SeanD@seandesktop`, `C:\ferrox-win`, release binary `--release --locked`
- Suites run **serially** (`--test-threads=1`): under parallel load
  `admit_delegated_backend` rejects, which would produce refusals for the *wrong reason* —
  worse than a failure, because it looks like success.

---

## 1. The five hostile cases — both hosts

Every compromise was **induced**, not simulated. Each capture records its command, its
output and an `EXIT:` line.

| Case | Linux | Windows | Verdict observed |
|---|---|---|---|
| ROTATED-KEY | REFUSED exit=1 | REFUSED exit=1 | `receipt is invalid: receipt body digest mismatch` |
| TAMPERED-BUNDLE | REFUSED exit=1 | REFUSED exit=1 | `receipt body digest mismatch` |
| ATTESTATION-MISMATCH | REFUSED exit=1 | REFUSED exit=1 | `backend identity mismatch` |
| DENIED-SECRET | REFUSED exit=1 | REFUSED exit=1 | `has no credential …; refusing to run and NOT falling back` |
| DENIED-EGRESS | REFUSED exit=1 | REFUSED exit=1 | the credentialed egress surface refused before any request left the host |

The key rotation is a real key replacement — 32 fresh bytes written over
`keys/local.key`, a task run under the new key, and the pre-rotation signature then
presented under the new key id. The denied-secret case produced **no receipt at all**:

```
$ wayland-core backend run --backend cloud
wayland-core backend: backend 'cloud' is unavailable and this command does NOT fall back:
backend cloud has no credential (WAYLAND_F25_CLOUD_TOKEN); refusing to run and NOT falling back
(probe CredentialAbsent)
EXIT: 1
```

### Two verdict labels I corrected rather than let stand

Both of these were my own ledger overstating what the run proved. Recorded because the
correction is the point.

1. **ROTATED-KEY** was first labelled `rotated-key-signature-refused`. The CLI's
   `backend receipt verify` is **integrity-only by design** — it deliberately cannot pin
   identity and says so in its own doc. What the live leg proves is that a rotated-out
   signature is *refused*, via the body digest. The *identity* half of rotation refusal
   is proven by the unit test `case_rotated_key_is_refused_against_the_new_pinned_identity`,
   not by the CLI. The verdict now says exactly that.
2. **DENIED-EGRESS** was first recorded `REFUSED exit=1` from `backend probe cloud`.
   The capture showed `EXIT: 0` — a probe **reports** unavailability, it does not refuse.
   Claiming a nonzero exit that did not happen is precisely the engineered green this
   program forbids. The leg now runs an actual `backend run` against the credentialed
   egress surface, which does exit non-zero and produces no receipt.

---

## 2. Orphans — enumerated INDEPENDENTLY of the scanner

The independent enumeration is a **check on** the scanner, not a copy of it. That
distinction is what found the defect in §3.

### Linux (`hetzner-dsm`)

```
COMMAND: ps -eo pid,ppid,pgid,etimes,args   (filtered with /usr/bin/grep -F on the nonce)
--- WITH A DELIBERATE ORPHAN PLANTED ---
2165316 2164164 2164163       2 sh -c while :; do sleep 1; done # f25-04-orphan-2164164
ROWS-MATCHING-NONCE: 1
```
```
--- AFTER THE REAP ---
ROWS-MATCHING-NONCE: 0
```

Scanner, same moments: **1 → 0**.

```
F25-SC4-SCANNER-AGREEMENT-PLANTED: AGREE scanner=1 manual=1
F25-SC4-SCANNER-AGREEMENT:         AGREE scanner=0 manual=0
```

### Windows (`SeanDesktop`)

```
COMMAND: Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like "*<nonce>*" } | Select-Object ProcessId,ParentProcessId,CommandLine
--- AFTER THE REAP ---
ROWS-MATCHING-NONCE: 0
```

Scanner while planted:

```
  backend    local
  mechanism  kernel-backed: ProcessTreeMechanism::WindowsJobObject — a kill-on-close Job Object
  method     live-task registry UNION a real enumeration of the host process table
  count      1 (MEASURED)
  row        process table: 6336 40608 "C:\WINDOWS\system32\cmd.exe" /c ping -n 600 127.0.0.1 > nul & rem f25-04-win-orphan-40608
```

```
F25-SC4-SCANNER-AGREEMENT-PLANTED: AGREE scanner=1 manual=1
F25-SC4-SCANNER-AGREEMENT:         AGREE scanner=0 manual=0
```

### Container

```
COMMAND: docker ps -a --filter label=wayland.task-nonce=<nonce>
ROWS-MATCHING-NONCE: 0
```

---

## 3. Three findings, all HIGH, all found by driving the real thing

None was a crash. All three were **false answers**, which is the class a green suite
cannot see.

### 3.1 The local scan could not see an orphan at all — `1f5cdf29`

`scan_orphans` consulted **only the live-task registry**. A terminal event *removes* the
registry entry, so a process that outlived its task is by construction no longer listed.
The scan was structurally blind to the exact thing it exists to find.

Measured on `hetzner-dsm`: independent `ps` found **1** row carrying the nonce, the
scanner reported **0**. Now the union of the registry crossing and a real process-table
enumeration.

### 3.2 The scanner counted itself — same commit

Fixing 3.1 immediately reproduced the defect plan 25-01 had already hit remotely:
`backend scan --task-id <nonce>` carries the nonce **on its own argv**, so the scanner
matched itself. Measured: scanner **1**, independent enumeration **0**, and the single
row was the scanner. Now excluded by pid — and a row whose pid cannot be parsed is
**kept**, because a filter that silently drops rows is the worst failure this module has.

### 3.3 The Windows scanner reported a MEASURED ZERO while an orphan ran — `b0bb30d5`, `f0778ba1`

Full write-up with both captures: **`evidence/25-04-WINDOWS-FALSE-ZERO.md`**.

`tasklist /V /FO CSV` **does not print command lines at all**. The nonce lives in the
command line, so it was never in the filtered output.

```
RED  (tasklist):        planted=1  scannerPlanted=0
GREEN (Win32_Process):  planted=1  scannerPlanted=1
```

Two further points, both of which matter more than the instrument swap:

- **The residual.** `Win32_Process.CommandLine` returns NULL without sufficient
  privilege. An enumeration that "succeeds" with every command line blank reproduces the
  same false zero with a different tool. The instrument now **self-tests**: this
  process's own row must be present *and* carry a non-empty command line — we know our
  own pid and we know we have a command line, so if we cannot see our own we cannot see
  anyone's. `ProcessTableScan` is `Enumerated | CannotDetermine` with **no `count()`**,
  so "could not look" is *unrepresentable* as zero rather than merely discouraged.
- **The evidence gate was itself self-passing.** The original run recorded
  `SCANNER-AGREEMENT: AGREE scanner=0 manual=0` — taken only *after the reap*, where both
  sides are legitimately zero. A comparison available only in the state where both sides
  are zero cannot detect a scanner that always says zero. The agreement verdict is now
  taken **while the orphan is planted** too, and a run in which the plant never appeared
  is recorded DISAGREE rather than allowed to pass vacuously.

---

## 4. Re-measurement list — what consumed the false zero

The blast radius is bounded and was verified by inspection, not assumed.

| Consumer | Status |
|---|---|
| `wayland-core backend scan` (Windows) | **RE-MEASURED.** False-zero window: `f846e471` → `b0bb30d5`. Both bounds are inside this plan. |
| `wcore_exec_backend::orphan::{scan_all, scan_one}` (Windows) | **RE-MEASURED**, same window. |
| 25-04 Windows ledger, first run — `F25-SC4-ENUM-WINDOWS`, `F25-SC4-SCANNER-AGREEMENT` | **SUPERSEDED AND RE-MEASURED.** The `AGREE scanner=0 manual=0` line was spurious. Replaced by a run that also compares while planted. |

**Verified NOT affected:**

- **25-01's cancellation / zero-residual proof.** Ran on `hetzner-dsm` using `ps`, and
  25-01's own SUMMARY records *"No Windows leg. No `wayland-core` build exists on
  SeanDesktop in this window."* It never consumed a Windows orphan count.
- **25-02 and 25-03.** Neither makes an orphan claim. 25-03's `terminate_in_flight` ran
  against an empty registry and the transcript says so rather than claiming a
  termination.
- **`wcore-browser::supervisor::process_alive` and
  `wcore-exec-backend::backends::local::process_alive`.** Both also shell out to
  `tasklist`, but they filter on **PID** (`/FI "PID eq <pid>"`) — a column `tasklist`
  *does* print. The defect was specific to filtering on a command line. Unaffected.
- **Everything outside this plan.** The scanner is new in `f846e471`; nothing predating
  it could have consumed its output.

---

## 5. The Windows known-RED assessment

`F25-SC4-WINDOWS-KNOWN-RED: INDEPENDENT test=live_future_drop_reaps_descendant_job_tree`

The handoff records that test as deterministically RED on Windows and **escalated**,
because every candidate fix changes what the sandbox permits. This plan does **not** fix
it, and must state whether the Phase 25 orphan claim rests on the same behaviour.

**It does not, and here is the reasoning rather than an assertion.** That test exercises
the Job Object *reaping* path — whether dropping a future tears down a descendant tree.
The Phase 25 Windows orphan claim is an **observation**, not a reaping claim: the
enumeration reports what is present, and the reap in the live exercise was performed by
`Stop-Process`, not by the sandbox's Job Object teardown. The scanner would report a
surviving descendant whether the Job Object reaped it or not — that is precisely what it
is for.

**The consequence, stated plainly:** Phase 25 does **not** claim that the Windows Job
Object reaps descendant trees. It claims only that the scanner can *see* whether one
survived, and that claim is now backed by a planted-orphan measurement
(`scannerPlanted=1`). Anyone reading a Windows `orphans: 0` should read it as "nothing
was left behind in this run", not as "the Job Object mechanism is proven".

---

## 6. Unexercised, with reasons

- **SSH backend orphan enumeration.** `WAYLAND_EXEC_SSH_TARGET` is not set on the proof
  host, so the surface reports `NOT MEASURED` with that reason — never zero. Its
  mechanism is recorded `BEST-EFFORT`: `ProcessTreeMechanism` has no variant that
  crosses an ssh connection.
- **Cloud backend orphan enumeration.** No credential exists, so the machine list cannot
  be queried. Reported `NOT MEASURED`; mechanism recorded `NONE`.
- **A real egress *policy* denial.** No egress policy is installed on either proof host,
  and the only credentialed egress surface has no credential, so no outbound request is
  ever attempted. What is proven is that the surface **fails closed**; what is NOT proven
  is a policy-level deny of an attempted request.
- **The three-gate plugin refusal end-to-end through the binary.** Proven at the
  digest, signature and approval gates by `case_tampered_plugin_bundle_fails_the_digest_and_the_signature_and_the_approval`,
  and live through the CLI in plan 25-02's transcript; not re-driven through the CLI here.
