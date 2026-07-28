# 28-E5-REPAIR — running notes (append-only, re-committed after every measurement)

**Lane:** `lane/28-e5-repair` · **Started:** 2026-07-29 · **Base (merge-base):** `d3e871a0`

Goal: close ONE evidentiary hole — the `F-28-02-002` sandbox repair has never been exercised by
an E5 matrix run on any platform. Run the E5 Windows sandbox cells against a binary that
contains it, on `seandesktop`. This does NOT re-certify Phase 28.

---

## 1. Premise check (step 1 of the brief) — CONFIRMED

The repair commits, per `28-DRIFT.md` §1a:

```
15821c03  fix(sandbox): reclaim stale AppContainer ACL leases instead of wedging   (F-28-02-002)
3f3f93dc  fix(sandbox): extract the reclamation report so its wording stays pinned
9c4d2612  style(sandbox): rustfmt the 0-byte lease tests
```

### 1a. Every E5/soak candidate predates the repair

`git merge-base --is-ancestor`, run in this worktree:

| repair commit | ancestor of `32e2f57d` (28-02 matrix candidate) | ancestor of `e4a3f5fc` (28-03 soak candidate) | ancestor of lane HEAD |
|---|---|---|---|
| `15821c03` | **NO** | **NO** | yes |
| `3f3f93dc` | **NO** | **NO** | yes |
| `9c4d2612` | **NO** | **NO** | yes |

### 1b. Candidate bindings in the evidence

| artifact | `candidate.commit` |
|---|---|
| `evidence/28-01/candidate.json` | `32e2f57d…` (marked `provisional: true`) |
| `evidence/28-02/candidate.json` | `32e2f57d…` |
| `evidence/28-02/results.json` (the 651-cell E5 matrix) | `32e2f57d…` |
| `evidence/28-03/candidate.json` | `e4a3f5fc…` |
| `evidence/28-03/soak.json` | `e4a3f5fc…` |

`soak.json` is the only file of that name in the tree. It names `e4a3f5fc` and nothing else.
So the brief's phrasing ("every `candidate/results/soak.json` names only the two pre-repair
commits") holds, and it is exactly two commits, not more.

### 1c. Where the repair HAS been exercised (so the hole is precisely this shape)

`grep -rl '15821c03'` over the phase shows the repair appears in `28-h2` evidence
(`repro-after.log`, `unittest-after.log`, both `SRC_SHA=15821c035f14…`) — i.e. the targeted
live repro harness and the unit tests, on real `seandesktop` hardware. It appears in `28-adj`,
`28-adj2`, `28-drift` prose and in `28-04/findings.tsv` / receipt SUPERSEDING-002 as a
*disposition reference*. It appears in **no** matrix or soak results file.

**Premise verdict: TRUE as stated.** No E5 matrix run has ever exercised the repair.

---

## 2. The original invocation, recovered (step 3 of the brief)

`evidence/28-02/f28-win-matrix.ps1`, run as a scheduled task on `seandesktop`:

```
node scripts/f28-native-matrix.mjs --capture-activeness --bin $exe --out win-activeness.json
node scripts/f28-native-matrix.mjs --run --bin $exe --os windows \
     --commit <c> --tree <t> --nonce <n> --matrix evidence/28-01/matrix.tsv \
     --activeness win-activeness.json --log win-matrix-markers.log --json win-matrix.json
node scripts/f28-native-matrix.mjs --verify win-matrix-markers.log --matrix evidence/28-01/matrix.tsv \
     --os windows --commit <c> --tree <t> --nonce <n>
```

with a quiet check (`cargo`/`rustc`/`link` process count must be 0) before and after, and a
binary-digest assertion against the candidate ledger before anything runs.

`--run` filters `matrix.tsv` by `--os`, not by dimension. **I will run the whole 219-cell
Windows leg**, because that is the invocation that produced the 219 pass / 0 red row, and
comparability with that row is the point. The 26 `sandbox-probes` cells are reported separately.

### 2a. Why this run can actually fail

`captureActiveness()` obtains the inside half of the containment differential by spawning a
worker through the product's own sandbox path (`wayland-core swarm --workers 1 …`), which on
Windows is the AppContainer backend. **That is the exact path `F-28-02-002` wedged.** If the
lease repair regressed it, no worker spawns, `observed:false`, and all 26 sandbox cells go RED
under the activeness rule. The gate is not self-passing.

### 2b. Instrument delta since the 28-02 run — stated, not hidden

`scripts/f28-native-matrix.mjs` changed by +50/-4 between the 28-02 harness (`9529c46a`) and
lane HEAD. `git diff` shows the change is confined to `captureActiveness()` and is the
`F-28-02-001` macOS repair: an added `/etc` read signal, and a `sandbox exec` fallback that
fires **only** when the swarm worker did not run. On Windows the swarm worker did run at 28-02,
so the fallback is unreached, and the comment claims `/etc` is denied on both sides on Windows
(adds no difference). `evidence/28-01/matrix.tsv` is **byte-identical** (no diff). The cell probe
logic (`runMatrix`, the nine dimension probes, the marker grammar) is untouched.

I use lane HEAD's harness, not `9529c46a`'s, because it is the instrument the project now ships.

## 3. Candidate binary provenance

`ci.yml` fires on `push` to `lane/**` (added 2026-07-27 for exactly this reason) and its
`build (x86_64-pc-windows-msvc)` job uploads `wayland-core-x86_64-pc-windows-msvc`. That
reproduces 28-02's provenance line ("CI release artifact, digest asserted on the host before the
run") rather than substituting a hand-built binary, and it keeps the certification Windows box at
**0 compiles**, which was a 28-02 invariant (`QUIET_CHECK`).

Pushed `lane/28-e5-repair` → CI run **30393960770**.

## 3a. Host as-found state, and a hazard nobody has written down

`asfound.ps1` → `asfound.log`, run before anything else:

```
WHOAMI=seand   HOSTNAME=SEANDESKTOP
LEASE_DIR=C:\Users\seand\AppData\Local\Wayland\Core\AppContainerLeases\v1
LEASE_DIR_EXISTS=True   LEASE_ENTRY_COUNT=0
QUIET_CHECK build_processes=6          <-- NOT quiet
FREE_BYTES_C=215299235840   NODE=v24.16.0   SCHEDULED_TASKS_F28=0
```

**The lease directory is empty.** So this run measures whether the repair *regresses* the
matrix, not whether it clears a wedge in flight — 28-h2 already proved the clearing leg on this
same hardware. Stated up front so the result is not over-read.

**The hazard: `seandesktop` IS the self-hosted Windows CI runner.** The six busy processes are
`cargo`/`rustc` under `C:\WINDOWS\ServiceProfiles\NetworkService\.rustup\…` — the GitHub Actions
runner service, compiling the CI run **my own push just triggered**. So the act of pushing a lane
branch to obtain a CI-built Windows candidate makes the certification host fail its own
`QUIET_CHECK`. 28-02 asserted "0 compile" on this box and would have tripped on this too.

Mitigation taken: the artifact-producing `Build (x86_64-pc-windows-msvc)` job runs on a
**GitHub-hosted** `windows-latest` runner (`ci.yml` `build:` matrix), not the self-hosted one, so
cancelling the workflow after the artifact uploads frees the box without costing me the binary.
Duplicate run `30394092408` cancelled immediately; `30393960770` will be cancelled once its
Windows build artifact is up. My lane changes only `.planning/` documents, so no CI signal is
being discarded.

## 3b. Instrument, staged and self-tested on the host

| file | sha256 on Mac | sha256 on `seandesktop` |
|---|---|---|
| `scripts/f28-native-matrix.mjs` | `01e84b22fe33…` | `01e84b22fe33…` **identical** |
| `evidence/28-01/matrix.tsv` | `510a10431e00…` | `510a10431e00…` **identical** |

`node f28-native-matrix.mjs --self-test` on the box: **`25 assertions passed, 0 failed`**,
`SELFTEST_RC=0`. That is the marker verifier's own rejection suite (absent / duplicate /
reordered / foreign / misordered / unbound), so the instrument is demonstrably able to reject
before it is pointed at anything.

## 4. Closed

- [x] CI windows-msvc artifact downloaded (run `30393960770`), sha256 `4c48d665…`, digest
      asserted on the host.
- [x] Matrix run on `seandesktop` as a scheduled task: **219 cells, 219 pass, 0 red, 0 skip;
      26/26 sandbox**. Counts read back from `win-matrix.json`, not from exit status.
- [x] Real codes via `WLRC=`/`WLDONE`: `WLRC=0 WLACTRC=0 WLRUNRC=0 WLVERRC=0`.
- [x] **Wedge differential** (not asked for, and the part that makes the green mean something):
      same planted artifact, pre-repair binary `baf9bd69…` → **26 RED**, post-repair
      `4c48d665…` → **26 PASS**; wedge survived the pre leg and was quarantined by the post leg.
- [x] Marker mutations: control + null-mutation accepted, three mutants rejected each for its
      own reason. The **first** mutation suite was itself defective (CRLF writer → every mutant
      rejected for the wrong reason) and was repaired in-lane per §6b-ii; both logs kept.
- [x] Box left as found — tasks unregistered, `C:\f28e5` removed, lease dir back to 0 files,
      no quarantine dir, archive byte-intact, zero compiles by this lane.

Full write-up: `../../28-E5-REPAIR-RUN.md`.
