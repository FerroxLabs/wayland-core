# 28-02 — E5 hostile matrix results

**GENERATED from `evidence/28-02/results.json`. Do not edit by hand** — the prose and the
data cannot disagree because this file is rendered from that file.
Validated by `f28-check-matrix-results.py --verify`, `--check-control-precedence`,
`--check-activeness`, `--check-skips` and `--check-binary-binding`.

**Candidate:** `32e2f57d09fe4b287e513081862217dc9daa5901` tree `63ec0e6c36ff8e63789aab2f9760870304b671df`
**Run nonce:** `b6df686220e7b9a1a56bb2b3c268d059` **Control:** `control:appc-observe@seandesktop:f28ctl-20260727222551`

## 1. Headline

| | |
|---|---|
| cells with a recorded outcome | **651 of 651** |
| pass | **627** |
| red | **24** |
| **skipped** | **0** |
| critical cells | 147 — 123 pass, 24 red, **0 skipped** |
| sandbox cells carrying activeness on a green | 50 of 50 greens |

**No critical case was skipped. No case of any kind was skipped.** The observation-blocked
class was measured NOT AUTHORISED by the control, and no other class applied: all nine
dimensions are meaningful on all three families, no surface was claimed-but-absent for a
cell that ran, and nothing was architecturally impossible on the surfaces exercised.

## 2. Per family

| Family | Host | Binary digest | Ledger-bound | pass | red | skip |
|---|---|---|---|---|---|---|
| linux | `hetzner-dsm` | `e8431ba208c5e794…` | **yes** | 216 | 0 | 0 |
| macos | `Seans-MacBook-Pro.local` | `945534d6ebfab321…` | **yes** | 192 | 24 | 0 |
| windows | `seandesktop` | `baf9bd692833eb7b…` | **yes** | 219 | 0 | 0 |

Every family ran the **CI release artifact for the candidate**, digest-asserted on the host
before the run. No family's results come from a different build.

## 3. Per dimension x family

| Dimension | linux | macos | windows |
|---|---|---|---|
| `sandbox-probes` | 24 pass | 0 pass / **24 RED** | 26 pass |
| `unicode` | 24 pass | 24 pass | 24 pass |
| `long-paths` | 24 pass | 24 pass | 24 pass |
| `unc-reparse-symlink` | 24 pass | 24 pass | 24 pass |
| `process-cleanup` | 24 pass | 24 pass | 25 pass |
| `suspend-resume` | 24 pass | 24 pass | 24 pass |
| `offline` | 24 pass | 24 pass | 24 pass |
| `disk-full-read-only` | 24 pass | 24 pass | 24 pass |
| `hostile-inputs` | 24 pass | 24 pass | 24 pass |

## 4. The reds, attributed

### F-28-02-001 — 24 cells — HIGH

**Subject.** macOS sandbox activeness is not obtainable through any black-box surface of the shipped candidate: the only delegated-execution surface refuses on macOS because sandbox-exec does not meet the delegated admission contract and the Docker fallback is compiled out (feature `live-docker` off). With no containment differential the cell cannot produce positive activeness evidence, and under the contract's activeness rule that is a RED.

**Contradicted criterion:** 1 — **accept and defer paths CLOSED** (amendment A2, and by severity). Inherited severity: `-` (provenance only).

**Re-score rationale.** Criterion 1's subject matter is the hostile platform matrix including sandbox probes. A Criterion-subject property that cannot be evidenced at all means the criterion cannot be honestly asserted for this family, which the rubric scores HIGH. Amendment A2 closes the accept and defer paths regardless of the severity recorded.

**Cells:** all 24 `sandbox-probes` cells on `macos`.

**Exact invocation** (`sandbox-probes-macos-acp`): `artifacts/wayland-core-aarch64-apple-darwin/wayland-core acp --help with WAYLAND_SANDBOX=none and WAYLAND_ALLOW_NO_SANDBOX unset, plus the run's containment differential`

**Observable:** the surface did not misbehave with the backend refused (exit=0), but no positive activeness observation is available, so a green would be indistinguishable from a silently disabled sandbox: no worker could be spawned through the product's own sandbox path, so no containment differential is obtainable: {

## 5. Every sandbox green's activeness observation

A green on a sandbox cell is only expressible with positive evidence the sandbox was ACTIVE.
The evidence is a DIFFERENTIAL: the same probe outside the product and inside a worker the
product spawned, so absence of a violation can never stand in for it.

| Family | greens | Activeness observed |
|---|---|---|
| linux | 24 | process-id namespace changed (NSpid:1277328 outside, NSpid:4 inside); filesystem root reduced from 52 entries to 9 (mount namespace); DNS resolves outside and does not inside (network namespace) |
| macos | 0 | **none — every sandbox cell on this family is a RED** |
| windows | 26 | the child was refused \BaseNamedObjects with 0xC0000022, which AppContainer confines by construction |

## 6. Skips

**There are none.** `--check-skips` reports `skips_checked: 0` over the generated matrix.

## 7. Where the goal was NOT achieved

**macOS sandbox coverage is not achieved, and this is stated here rather than in a footnote.**
72 of 216 macOS cells are RED because no positive activeness observation is obtainable on
that family through any black-box surface of the shipped candidate: the only delegated-
execution surface refuses on macOS (`sandbox_exec cannot own descendants that escape a
process group`) and the Docker fallback is compiled out (`feature \`live-docker\` off`).
Under the contract's activeness rule that is a RED — never a green, and never a skip.
Criterion 1 is therefore **NOT satisfied on macOS** by this run.

What would close it: a black-box surface on macOS that executes a caller-supplied command
through `default_for_platform()`'s sandbox-exec backend and returns enough of the child's
environment to form a containment differential. `backend run --backend local` executes a
caller-supplied task and reports `platform containment backend 'sandbox_exec' probed
available`, but its receipt retains only the artifact DIGEST, not its bytes, so the child's
own view is not recoverable from it — and a probe task run through it was able to create a
file outside its workspace, which is recorded as `F-28-02-005` for the owner to interpret
against the actual sandbox-exec profile.
