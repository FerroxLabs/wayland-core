# 28-H2 — `F-28-02-002`, the stale AppContainer lease wedge

**Disposition: FIXED.**

Reproduced on real Windows hardware at the lane base, repaired, and re-measured on the same
hardware with the same harness. Both required legs are proved and both are proved
non-vacuous by mutation. Phase 28's acceptance gate is **not** unblocked by this alone — see
§8, which names what still blocks it.

| | |
|---|---|
| Lane branch | `lane/28-h2` |
| Base | `12fc794f` (`plan/f20-unified-audit-repair`) |
| HEAD | `3f3f93dc` |
| Hardware | `SeanDesktop` (`ssh SeanD@seandesktop`), rustc 1.95.0 |
| Evidence | `.planning/phases/28-native-cross-platform-certification/evidence/28-h2/` |

---

## 1. The finding as I found it, and one correction to the brief

The dispatch brief told me three things to confirm rather than re-derive. Two hold. **One is
wrong, and it matters, because it points at the wrong repair.**

| Brief claim | Verdict |
|---|---|
| The lease store lives under `%LOCALAPPDATA%` (`acl_lease/storage.rs:43`) | **Confirmed.** |
| `acl_lease/mutation_lock.rs` uses a named mutex with a 15s timeout and handles `WAIT_ABANDONED`, so the wedge is not the mutex | **Confirmed** (`mutation_lock.rs:77,85,199`). I did not go down that path. |
| The word `stale` appears nowhere in `acl_lease.rs` / `acl_lease/*.rs` | **Confirmed** — zero hits. |
| "There is no expiry, no reclamation, no owner-liveness check — the absence of any staleness concept" | **FALSE at `12fc794f`.** |

`LeaseFile` already carries `owner_pid` **and** `owner_creation_time` (`acl_lease.rs:129-130`);
`owner_is_live()` (`:720`) already checks both via `OpenProcess` + `GetProcessTimes` +
`WaitForSingleObject`; and `recover_dead_leases_locked()` (`:607`) already runs on **every**
`ExecutionIdentity::start` (`:250`) and already reclaims dead-owner leases. The owner-identity
scheme the brief prescribes as the fix was already there.

The defect is narrower and would have been missed by building what the brief described. In
`recover_dead_leases_locked`, a lease whose owner is dead and which cannot reconcile its
recorded SID against its own profile hit one of two `return Err(...)` statements. Those abort
the **whole recovery pass**, so every later `ExecutionIdentity::start` failed the same way. The
negative probe cache is in-process only, so a fresh process re-read the same file and failed
again. Nothing expired it and nothing quarantined it.

The code says so itself. `storage.rs:59` at base: *"returns `Err`, and fails closed FOREVER —
**there is no quarantine path**"*. The wedge was not mishandled staleness; it was a
**missing quarantine path for a self-clearing condition**.

## 2. Repro — real hardware, lane base, `SRC_DIRTY=0`

Harness `evidence/28-h2/repro.ps1` drives the shipped binary through
`wayland-core sandbox status` and `wayland-core sandbox exec`. `sandbox exec` dispatches through
`wcore_tools::bash::BashTool::execute_with_ctx` — the agent's own shell function — so what it
observes is what the agent gets. Three lease states, and the wedge is the archived real artifact
28-02 used (`WCore-storage-00002d20-00000000000000f2.toml`, `owner_pid = 11552`, `intents = []`,
`sid_sha256 = 5b22ee05…` = `TEST_SID_SENTINEL_SHA256`).

`evidence/28-h2/repro-before.log`, binary `91a1906d…`, `SRC_SHA=12fc794f`, `SRC_DIRTY=0`:

| Lease state | `sandbox status` | `sandbox exec` |
|---|---|---|
| clean | `backend appcontainer`, `available true` | **`Exit code: 0`, STDOUT `F28H2RAN`** |
| wedged | backend degrades to `fail_closed`; probe error | **refused, `ran=False`** |
| wedged, second run | identical | identical |

The clean row is the positive control: sandboxed execution genuinely runs, so a later green
cannot be manufactured by universal denial. The third row is the finding: **the refusal is
permanent, not a transient.**

What the operator actually saw (verbatim, `repro-before.log:32-40`, wrapped by the console):

> `ERROR AppContainer real-spawn probe failed; sandbox disabled. If the failure is transient
> (AV, disk contention), the probe re-runs after the negative-cache TTL. error=… AppContainer
> ACL lease …\WCore-storage-…toml was written by wcore-sandbox's OWN TEST SUITE … **not
> transient**: the sandbox stays disabled on this machine until this file is DELETED.`

…and then, as the only line an operator acts on:

> `Refused: shell is unavailable because the active sandbox backend cannot enforce
> secret-read-deny for this workspace.`

Three separate message defects, and they are not the one the finding described:

1. The outer wrapper **asserts transience**, and is immediately contradicted by its own inner
   error asserting the opposite. Whichever the reader believes, one of them lied.
2. The inner text was already good — it names the file and the remedy. An earlier repair landed
   that. So "reads like a platform limitation" was only **half** true at `12fc794f`: the
   diagnosis was right and the wrapper around it was wrong.
3. The **refusal line the operator acts on** names neither the lease, the file, nor the cause.

I also independently re-confirmed 28-02's central claim on my own build: `ran=False` in both
wedged observations. The dispatcher fails **closed**. This is denial of service, not loss of
containment. HIGH, not CRITICAL, is the right score.

## 3. The fix

`acl_lease.rs`, `acl_lease/storage.rs`, `windows_impl/process.rs`.

- A dead-owner lease that cannot reconcile is **reclaimed**, not refused: `quarantine_lease()`
  moves it into a `quarantine\` sub-directory and `recover_dead_leases_locked` continues.
- **Moved, never deleted.** The file is the only record of how the wedge formed. Two such files
  were found on a real developer box.
- **Gated on the owner being provably gone.** Reclamation is reached only after `owner_is_live`
  returns false, which is unchanged.
- The quarantine directory is **explicitly allow-listed** in the directory scan. This is not
  incidental: that scan hard-errors on any unrecognised entry, so a quarantine directory that
  was not allow-listed would itself wedge the sandbox from the second pass onward — the same
  defect, one indirection down. `M1` below exists to keep that honest.
- Residual grants are reported, not glossed. A mismatching SID cannot be reconstructed from its
  digest, so recorded ACL grants cannot be revoked automatically. Refusing forever never revoked
  them either — it only *also* disabled the sandbox — so quarantining strictly dominates, but the
  operator is told exactly what may remain. For the measured real artifacts (`intents = []`)
  the message states that nothing was left behind.
- Message repaired: the probe no longer asserts transience, and defers to the inner error.

**The refusal itself was never the bug.** Failing closed is correct and is unchanged for every
condition that still warrants it. The bug was treating a *self-clearing* condition — a dead
owner's unreconcilable lease has authority over nothing — as permanent.

## 4. Both legs, on hardware

`evidence/28-h2/repro-head.log`, binary `2114d41c…`, `SRC_SHA=3f3f93dc`, `SRC_DIRTY=0`, same
harness, same artifact:

| Lease state | active | quarantined | `sandbox exec` ran? |
|---|---|---|---|
| clean | 0 | 0 | **yes** (`Exit code: 0`, `F28H2RAN`) |
| wedged | 1 | 0 | **yes** — reclaimed in-flight |
| wedged, second run | 0 | 1 | **yes**, silently |

`sandbox status` stays `backend appcontainer / available true` instead of degrading to
`fail_closed`. **The positive leg is the point: execution actually runs. The fix is not a green
by universal denial.**

Operator text now (`repro-head.log:32-44`):

> `ERROR RECLAIMED a stale AppContainer ACL lease: it was written by wcore-sandbox's OWN TEST
> SUITE … and can never match a real AppContainer profile, and its owning process 11552 is
> gone. This was persistent on-disk state — NOT a platform limitation, NOT an SSH or session-0
> effect, and NOT transient. Until this reclamation landed, a file in this state disabled ALL
> sandboxed execution on this machine until a human DELETED it. The file has been MOVED (not
> deleted) to …\quarantine\WCore-storage-…toml.quarantined-00008e5c-… so the cause stays
> inspectable. It recorded NO filesystem ACL grant, so nothing was left behind on this machine.`

`owner_pid 11552` matches the artifact's recorded owner exactly.

### Honour-when-alive

Proved twice, and the second is at the real-profile level:

- Unit: `live_owner_unreconcilable_lease_is_honoured_not_reclaimed` — identical to the reclaim
  test in every respect except the recorded owner is this running process. Passes.
- Live: `live_owner_is_never_reclaimed` **... ok** under `WAYLAND_SANDBOX_LIVE_WINDOWS=1`, with a
  real `CreateAppContainerProfile` identity (`final.log`). So does
  `killed_owner_is_recovered_before_next_execution`, which additionally proves the dead-owner ACEs
  are actually removed from a real granted path.

## 5. Gate results — real numbers, and how each was made able to fail

`cargo test -p wcore-sandbox --lib -- --test-threads=1` on Windows at `3f3f93dc`:
**`133 passed; 0 failed; 23 ignored`** (`unittest-named.log`).

Four new tests, each additionally run **by exact name** with the executed count read back:

| Test | |
|---|---|
| `dead_owner_unreconcilable_lease_is_reclaimed_not_refused_forever` | `passed=1` |
| `live_owner_unreconcilable_lease_is_honoured_not_reclaimed` | `passed=1` |
| `quarantine_directory_does_not_become_a_second_wedge` | `passed=1` |
| `reclamation_reports_grants_it_could_not_revoke` | `passed=1` |

They are **not** `#[ignore]`d and need no env gate — nothing in the defect requires a real
AppContainer profile — so they run in the ordinary suite rather than behind two switches that
can each silently zero the run.

**Mutation (`mutate2-*.log`, `mutants.diff`) — each mutant restores one half of the defect:**

| Mutant | Result |
|---|---|
| **M1** remove the quarantine-dir allow-list | `quarantine_directory_does_not_become_a_second_wedge` **FAILED**; other three pass |
| **M2** reclaim regardless of `owner_is_live` | `live_owner_unreconcilable_lease_is_honoured_not_reclaimed` **FAILED**; other three pass |

M2 is the one that matters: it proves the honour-when-alive test is not satisfied by an
implementation that reclaims unconditionally.

**Clippy:** 4 warnings at HEAD, **4 warnings at base `12fc794f`** (`basecheck.log`), all
`unused import` in `tests/hard_process_containment_windows.rs`, a file this lane did not touch.
Zero new. Not fixed — out of scope.

**Live acceptance** (`--lib --ignored`, `WAYLAND_SANDBOX_LIVE_WINDOWS=1`):
**20 passed, 3 failed.** All 3 failures are `required_live_bwrap_*`, panicking on *"required
live bwrap must be installed and usable"* — Linux bubblewrap tests on a Windows box. Measured at
base too: **all 3 fail identically at `12fc794f`** (`basecheck.log`). Pre-existing and
environmental. Reported red, not silenced.

Only the `--lib` target was run under the live env var. Integration tests under `tests/` compile
without `cfg(test)` and lease into the **real** `%LOCALAPPDATA%`; running them is how finding
F-4 poisoned production leases, and this lane did not regress it.

## 6. Three self-passing gates I hit, all caught by reading counts rather than exit status

Recording these because each would have produced a confident wrong answer, and one was in my
own instrument — the failure mode the brief names as costing eight instances so far.

1. **A filter matching no test name.** My first named run reported
   `result=ok; passed=0` for all four tests. The module is `appcontainer_acl_lease`, not
   `acl_lease`, so every `--exact` filter matched nothing and exited 0. Caught only by asserting
   the executed count; the command *looked* targeted.
2. **A stale binary — my mutation harness carried the defect it was hunting.** `Copy-Item`
   preserves the source's `LastWriteTime`, and all three variants were `scp`'d inside the same
   second, so cargo saw no mtime change and silently re-ran the **previous** mutant's binary.
   M2 therefore reproduced M1's failure exactly. I noticed because that result was implausible,
   not because anything failed. Repaired by stamping `LastWriteTime` **and** asserting
   `Compiling wcore-sandbox` actually appeared (`MUT_COMPILED=True`); the discarded first attempt
   is kept at `mutdiag-m2.log`.
3. **A nested child test-process's summary spliced into the parent's stream.** Two live tests
   re-spawn the test binary, so `test result:` appears more than once. Taking the *first* match
   read the child's `1 passed` and hid the parent's `20 passed; 3 failed`. Fixed by taking the
   **last** match.

A fourth, avoided by construction: a release build compiles no `cfg(test)` code, so renaming
`unreconcilable_lease_message` left a broken test caller that the product build reported clean.
Only running the unit tests on Windows caught it (`unittest-after.log`).

## 7. Deviation: an existing test I had to retarget, and did not weaken

Renaming `unreconcilable_lease_message` broke `a_leaked_test_lease_is_diagnosed_by_name`
(`storage.rs`), which pinned the operator wording. Rather than drop its remedy assertion, I
extracted the text into a pure `reclamation_report()` the test can call. It keeps every
assertion it had and gains two: that the message denies all three false explanations that kept
this defect alive, and that it names where the evidence was moved. `a_leaked_test_lease_is_
diagnosed_by_name ... ok`.

## 8. Does Phase 28's acceptance gate now pass? — **No, and not because of this finding.**

`F-28-02-002` was the one finding with disposition `OPEN`, and at HIGH only FIXED or DISPROVED
were available. It is now **FIXED**, by repair on hardware, not by re-scoring — the downgrade to
MEDIUM that 28-04 deliberately declined was not taken here either.

What I did **not** do, and what still stands between here and a passing gate:

- **The ledger still says `OPEN`.** `28-04-FINDING-LEDGER.md:126` and `evidence/28-04/findings.tsv:39`
  are unchanged by this lane. The gate is *"zero findings lack an explicit, evidence-backed
  terminal disposition"*, and it reads the ledger, not this file. Re-adjudicating a finding is
  28-04's job and doing it from the lane that authored the fix would be marking my own homework.
  **This summary is the evidence for that re-adjudication; it is not the re-adjudication.**
- **The fix is on `lane/28-h2` only.** Not merged, no PR — both reserved to Sean.
- Scope, stated so it is not over-read: this repairs the *lease* wedge. `KR-05` half 3 — "logs a
  message that reads like a platform limitation" — was CONFIRMED by 28-02 and is addressed here
  for this path, but I measured only the AppContainer lease surface. I did not exercise
  `default_for_platform()` / the `WAYLAND_ALLOW_NO_SANDBOX=1` opt-in path, and nothing here
  should be read as closing `KR-05`.
- `F-28-02-003` and `F-28-02-004` are MEDIUM, remain BACKLOG, untouched.

## 9. Housekeeping

The box is left as found: active leases `0` → `0`, the quarantine directory the repro created
under the real `%LOCALAPPDATA%` removed, both archived artifacts in
`C:\p22-evidence\stale-leases-backup` intact (`cleanup.ps1` output). Disk 203 GB free. No
hetzner worktree was created — this code is `#[cfg(windows)]` and cannot compile there.
`C:\f28h2-repo` and `C:\f28h2-target` remain, for anyone re-running this.

No shared-file edits (`wcore-cli/src/lib.rs`, `main.rs` untouched). No contract regeneration.
