# 28-E5-REPAIR — the sandbox repair has now met the matrix, on Windows

**Lane:** `lane/28-e5-repair` · **Date:** 2026-07-29 · **Merge-base:** `d3e871a0`
**Hardware:** `seandesktop` (`ssh SeanD@seandesktop`) · **Evidence:** `evidence/28-e5-repair/`

---

## 0. The result, first

**The `F-28-02-002` repair survives the E5 matrix. 219 of 219 Windows cells pass, all 26
sandbox cells among them, at a candidate that contains `15821c03`.**

And a control that was not asked for, because without it the green above means very little:
with the **real archived wedge artifact planted**, the same matrix scores **26 RED** on the
pre-repair candidate and **26 PASS** on the post-repair candidate. So the 26 sandbox cells are
not decoration — they detect the exact defect this lane exists to test, and they flip.

**What this is not.** This does not re-certify Phase 28 and does not make the certification
current. A 3/3 panel concluded certification belongs at a frozen release candidate, not against
a branch that moves hourly, and nothing here changes that. One evidentiary hole is closed:
the sandbox repair had never been exercised by a matrix run on any platform, and now it has
been on one — Windows. macOS and Linux remain unexercised against it.

---

## 1. Premise check — the drift lane's HIGH finding is TRUE as stated

I verified it before building anything, because four "impossible"/"absent" claims have been
falsified on this program in the last two days.

The three repair commits (`28-DRIFT.md` §1a): `15821c03` (the repair), `3f3f93dc`, `9c4d2612`.

`git merge-base --is-ancestor`, run in this worktree:

| repair commit | ancestor of `32e2f57d` (28-02 matrix candidate) | ancestor of `e4a3f5fc` (28-03 soak candidate) | ancestor of lane HEAD |
|---|---|---|---|
| `15821c03` | **NO** | **NO** | yes |
| `3f3f93dc` | **NO** | **NO** | yes |
| `9c4d2612` | **NO** | **NO** | yes |

Candidate bindings across the evidence:

| artifact | `candidate.commit` |
|---|---|
| `evidence/28-01/candidate.json` | `32e2f57d…` (`provisional: true`) |
| `evidence/28-02/candidate.json` | `32e2f57d…` |
| `evidence/28-02/results.json` — the 651-cell E5 matrix | `32e2f57d…` |
| `evidence/28-03/candidate.json` | `e4a3f5fc…` |
| `evidence/28-03/soak.json` | `e4a3f5fc…` |

`soak.json` is the only file of that name in the tree, and it names exactly one commit.
`grep -rl 15821c03` over the phase returns `28-h2` evidence (`repro-after.log`,
`unittest-after.log` — the targeted live harness and the unit tests), plus prose in `28-adj`,
`28-adj2`, `28-drift`, `28-04/findings.tsv` and receipt SUPERSEDING-002 as a **disposition
reference**. It appears in **no** matrix or soak results file.

**Verdict: confirmed.** No E5 matrix run had ever exercised the repair.

---

## 2. The candidate, and where it came from

| | |
|---|---|
| commit | `6db9e56b8b6c68a2b7939a0728beb06a92ceed0b` |
| tree | `1533c6a42b26522ee553e90954dd159bbaed2c3b` |
| contents | integration tip `d3e871a0` + one `.planning/` NOTES commit; contains `15821c03`, `3f3f93dc`, `9c4d2612` and both subsequent MEDIUM fixes |
| binary | `wayland-core.exe`, `x86_64-pc-windows-msvc` |
| sha256 | `4c48d6656f1d640fe1dbff7f2cceaaa260bca3b12ce51b930d7b9541d6d41f9d` |
| provenance | **CI run 30393960770**, job `Build (x86_64-pc-windows-msvc)`, artifact `wayland-core-x86_64-pc-windows-msvc`; digest asserted on the host before the run |

CI-built rather than hand-built on purpose: it reproduces 28-02's provenance line ("CI release
artifact, digest asserted on the host before the run") and it keeps the certification Windows
box at **zero compiles**, which was a 28-02 invariant. `ci.yml` fires on `push` to `lane/**`,
which is the route that exists for precisely this.

The control binary is the **byte-identical 28-02 Windows candidate**, recovered from its
original CI run 30269095004 and digest-asserted on the host as
`baf9bd692833eb7b9d54f00053739115b6ad5257fbdb0b0e99a8694a2ee996a6` — the value
`28-02-MATRIX-RESULTS.md` records.

---

## 3. The invocation — reproduced, not approximated

`evidence/28-e5-repair/f28e5-win-matrix.ps1` is `evidence/28-02/f28-win-matrix.ps1` with the
bindings changed and the exit path hardened. Same three node calls, same order:

```
node f28-native-matrix.mjs --capture-activeness --bin <exe> --out win-activeness.json
node f28-native-matrix.mjs --run  --bin <exe> --os windows --commit <c> --tree <t> --nonce <n> \
     --matrix evidence/28-01/matrix.tsv --activeness win-activeness.json \
     --log win-matrix-markers.log --json win-matrix.json
node f28-native-matrix.mjs --verify win-matrix-markers.log --matrix evidence/28-01/matrix.tsv \
     --os windows --commit <c> --tree <t> --nonce <n>
```

Launched as a **scheduled task**, as 28-02 launched it, not straight off the ssh session.
`--run` filters by `--os`, not by dimension, so the whole **219-cell Windows leg** ran; that is
the invocation that produced the row this is being compared against.

**Instrument, staged and digest-checked on the host:**

| file | Mac sha256 | host sha256 |
|---|---|---|
| `scripts/f28-native-matrix.mjs` | `01e84b22fe33…` | `01e84b22fe33…` **identical** |
| `evidence/28-01/matrix.tsv` | `510a10431e00…` | `510a10431e00…` **identical** |

`matrix.tsv` is byte-identical to the one 28-02 used. `f28-native-matrix.mjs` is +50/-4 since
28-02's harness commit `9529c46a`; the diff is confined to `captureActiveness()` and is the
`F-28-02-001` macOS repair (an added `/etc` read signal, and a `sandbox exec` fallback that
fires only when the swarm worker did not run). The cell probes, the nine dimensions and the
marker grammar are untouched. I used lane HEAD's harness because it is the instrument the
project now ships, and I state the delta rather than hiding it.

`node f28-native-matrix.mjs --self-test` on the host: **25 assertions passed, 0 failed**.

---

## 4. Cell results, with the real exit codes

Exit status is written to a status file (`WLRC=` first, `WLDONE` last) and read back by a
**separate** ssh call, because over ssh+PowerShell every non-zero collapses to `1`.

### 4a. Main run — empty lease directory (as found)

`evidence/28-e5-repair/win-matrix.status`, verbatim:

```
WLRC=0
WLCELLS=219 WLPASS=219 WLRED=0 WLSKIP=0
WLSBCELLS=26 WLSBPASS=26 WLSBRED=0
WLACTRC=0 WLRUNRC=0 WLVERRC=0
WLLEASEPRE=0 WLLEASEPOST=0 WLQPRE=0 WLQPOST=0
WLBUSY=4 WLBUSYEND=11 WLCPU=22.3 WLCPUEND=63.3
WLDONE
```

| leg | real code |
|---|---|
| `--capture-activeness` | `ACTIVENESS_RC=0`, `observed: true` |
| `--run` | `RUN_RC=0`, stdout `cells=219 red=0` |
| `--verify` | `VERIFY_RC=0`, stdout `VERIFIED platform=windows cells=219` |
| script | `EXIT=0` |

Counts are read back from `win-matrix.json`, not inferred from exit status, and the script
treats `cells == 0` as `rc=4` — a suite that exits 0 having run zero cells fails here rather
than passing.

Per dimension, all Windows: `sandbox-probes` 26 pass, and 24 pass each for `unicode`,
`long-paths`, `unc-reparse-symlink`, `suspend-resume`, `offline`, `disk-full-read-only`,
`hostile-inputs`; `process-cleanup` 25 pass. **Identical to 28-02's Windows row.**

The activeness observation is the same one 28-02 recorded, which matters because it is the
half that the wedge destroys:

> `the child was refused \BaseNamedObjects with 0xC0000022, which AppContainer confines by
> construction [inside reading via swarm]`

`[inside reading via swarm]` is the new annotation from the `F-28-02-001` repair; the swarm
path — the one `F-28-02-002` wedged — is what produced the reading.

### 4b. The wedge differential — the control that makes 4a mean something

The main run had an **empty** lease directory, so on its own it shows only that the repair does
not regress the matrix. `f28e5-wedge-differential.ps1` plants the real archived artifact
`C:\p22-evidence\stale-leases-backup\WCore-storage-00002d20-00000000000000f2.toml`
(sha256 `b2ae390a…`, the file 28-02 and 28-h2 both used) and runs the **same matrix twice**.

`evidence/28-e5-repair/wedge-diff.status`:

| leg | binary | activeness | cells | pass | red | sandbox pass | sandbox red |
|---|---|---|---|---|---|---|---|
| **A — pre-repair** | `baf9bd69…` (`32e2f57d`) | **`observed: false`** | 219 | 193 | **26** | **0** | **26** |
| **B — post-repair** | `4c48d665…` (`6db9e56b`) | `observed: true` | 219 | **219** | **0** | **26** | 0 |

`WLWEDGEOK=True` · `WLWEDGESURVIVED=True` (the pre-repair binary left the wedge in place —
permanence is the finding) · `WLWEDGERECLAIMED=True` · `WLRESTORED=0` · `WLRC=0`.
Both legs verified: `VERIFIED platform=windows cells=219`, `VERIFY_RC=0`.

The pre-repair leg's failure is the wedge itself, in the product's own words
(`wedge-diff.log`, via the matrix's activeness capture):

> `"status": {"Failed": "sandbox backend fail_closed cannot enforce delegated read denial"}`
> `… AppContainer ACL lease \\?\C:\Users\seand\AppData\Local\Wayland\Core\AppContainerLeases\v1\WCore-storage-00002d20-00000000000000f2.toml was written by wcore-sandbox's OWN TEST SUITE … the sandbox stays disabled on this machine until this file is DELETED.`

Exactly 26 cells moved, and they are exactly the 26 `sandbox-probes` cells; the other 193 are
identical in both legs. Lease counts: `after-plant active=1`, `after-A active=1 quarantined=0`,
`after-B active=0 quarantined=1`.

**This also says something about 28-02 that its own results file cannot.** Its Windows
219 pass / 0 red was taken on a **clean** lease directory. Had the box been wedged that day,
the same leg on the same binary would have scored 26 RED. The green was contingent on host
state that nothing in the receipt records.

### 4c. Marker mutations — and an instrument defect I created and then repaired

`mutations2.log`, four assertions:

| assertion | result |
|---|---|
| A — untouched log **accepted** | `RC=0`, `VERIFIED … cells=219` |
| B — **null mutation** (rewritten through the same writer, content identical) accepted | `RC=0` |
| C — three mutants **rejected** | absent `RC=1`, unbound-commit `RC=1`, duplicate `RC=1` |
| C′ — each rejected **for its own reason**, not a shared artefact | `CR_REASON=False` on all three |

Rejection messages, distinct and correct: `final acceptance marker before all cells were
recorded` / `cell sandbox-probes-windows-acp commit drift` / `duplicate cell marker:
sandbox-probes-windows-acp`.

**The first version of this suite (`mutations.log`) was defective and is why the second
exists.** It wrote its mutants with `Set-Content`, which emits CRLF; the verifier rejects any
CR byte under its LF-only authority grammar, so all three mutants were rejected with
`CR byte in authority artifact` — the **wrong reason**. That suite would have reported a
healthy instrument even if every mutation had been invisible. Per §6b-ii I repaired the
instrument in this lane rather than writing the defect up and moving on, and assertion **B** is
the one that proves the repair did anything: the broken writer fails it, the repaired one
passes it. Both logs are committed; the defective one is kept deliberately.

---

## 5. Arguing against my own conclusion

### 5a. The differential does not isolate `15821c03` — it isolates the window that contains it

Leg A is built at `32e2f57d` and leg B at `6db9e56b`: **194 commits under `crates/` apart**.
So the 26 RED → 26 PASS difference is attributable to that whole window, not to the lease
repair alone by this lane's own measurement. What ties it to the repair is (i) leg A's failure
text names the lease file and the SID-sentinel condition verbatim, (ii) leg B ends with the
artifact **quarantined**, which is the repair's specific mechanism and nothing else in the
window does that, and (iii) `28-h2` already isolated it at `12fc794f` → `3f3f93dc` on this same
hardware. The attribution is sound; the isolation is inherited, not re-derived here. Anyone who
wants the isolation inside the matrix should build a Windows binary at `15821c03^` and re-run
leg A against it. I did not.

### 5b. The run was taken under load, and I relaxed a 28-02 invariant to take it

28-02 required `build_processes = 0`. This run measured `4` at start and `11` at end, at 22%
and 63% total CPU of 32 logical processors. **`seandesktop` is also the self-hosted Windows CI
runner**, and seven other lanes had CI runs queued on it; a strictly quiet window was not
obtainable, and my own push for the candidate binary was itself part of the load. I added an
explicit `-AllowLoad` switch that records the load and continues, rather than editing the check
quietly.

The relaxation is one-directional for the dimension that matters: every E5 probe's pass
condition is "completed within its budget and did not misbehave", so contention produces
timeouts and therefore **false REDs**, never false greens. **One exception, stated because it
cuts the other way:** the Windows `suspend-resume` probe passes partly on observing the child
*not yet exited* while suspended, and load makes that easier to observe — so a `suspend-resume`
green under load is weaker than one taken quiet. That dimension is not this lane's subject, and
it was already 24/24 green quiet at 28-02.

### 5c. The `sandbox exec` activeness fallback is unusable on a credential-less host

In leg A the `F-28-02-001` fallback surface did not produce a second independent confirmation —
it exited with `No API key found. Provide via --api-key, config file, or environment variable`.
The verdict is unaffected (the swarm leg carries it, and its error names the lease file), but
the fallback added for macOS **cannot substitute for swarm on a host with no credentials**.
That is a real limitation of the instrument, MEDIUM at most, and it belongs in BACKLOG rather
than in this lane's scope. No credential was supplied, sought or copied.

### 5d. Only Windows

The brief's precondition was the Windows sandbox cells and that is what ran. macOS and Linux
E5 legs have still never been run against a binary containing the repair. The macOS sandbox
cells were 24 RED at 28-02 for an unrelated reason (`F-28-02-001`), so a macOS re-run is a
different question with a different answer.

---

## 6. The host was left as it was found, and that is measured

`cleanup.log`:

```
TASK_REMOVED f28e5WinMatrix still_present=False
TASK_REMOVED f28e5WedgeDiff  still_present=False
F28E5_DIR_PRESENT=False
LEASE_DIR_EXISTS=True   LEASE_FILES=0   QUARANTINE_DIR_EXISTS=False
ARCHIVE_EXISTS=True
ARCHIVE_FILE=WCore-storage-00002d20-00000000000000f2.toml|len=322|sha256=b2ae390a…
ARCHIVE_FILE=WCore-storage-00006314-00000000000000f2.toml|len=322|sha256=abf8733d…
OTHER_F28_TASKS=0
```

As-found (`asfound.log`) was `LEASE_ENTRY_COUNT=0`, `SCHEDULED_TASKS_F28=0`; both restored. The
planted wedge was a **copy**; the archive it came from is byte-intact, digest unchanged. The
quarantined copy the repair produced was removed, since the as-found directory had no
quarantine directory. Residue check after cleanup: `RESIDUE_HOME=0`, `F28E5_DIR=False`,
`F28_TASKS=0`. Nothing was compiled on the box by this lane.

Housekeeping I did do and should say plainly: I cancelled **my own** lane's CI runs
(30394092408, 30393960770, 30397059468, 30398085988) once the Windows build artifact was
uploaded, to stop them occupying the shared self-hosted runner. My changes are `.planning/`
documents only, so no CI signal was discarded. I cancelled no other lane's run.

---

## 7. What this establishes, and what it does not

**Establishes:**

1. The `F-28-02-002` repair has now been exercised by the E5 matrix on Windows: **219/219,
   including 26/26 sandbox cells**, at a digest-bound CI artifact containing `15821c03`.
2. The matrix's 26 Windows sandbox cells **can fail, and do**, on exactly this defect: 26 RED
   pre-repair vs 26 PASS post-repair with the same real wedge planted, on the same host, same
   instrument, same hour.
3. The marker verifier accepted this run's log and rejects absent, unbound and duplicate
   markers on it, each for its own reason.
4. 28-02's Windows green was **contingent on a clean lease directory** and its receipt does not
   record that.

**Does not establish:**

1. Anything about Phase 28's certification currency. The certification still covers `32e2f57d`
   and `e4a3f5fc` and still does not cover the tip.
2. Isolation of `15821c03` inside the matrix (§5a) — the window, not the commit.
3. Anything about macOS or Linux against the repair.
4. Anything about a host other than `seandesktop`.

A release cut still must not read Phase 28's passing gate as covering the shipped binary. One
hole in that record is now closed; the record itself is unchanged.
