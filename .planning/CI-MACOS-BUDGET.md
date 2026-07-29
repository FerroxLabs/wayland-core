---
lane: ci-macos-budget
finding-addressed: .planning/CI-STARVATION.md (HIGH — CI has produced no verdict on the integration branch since the clippy unblock)
verdict: DIAGNOSIS HELD under independent re-measurement, with one of its proposed remedies REFUTED. Repair landed and proven at branch level; the fleet-level effect is arithmetic, not yet observed, because the fix cannot take effect on the integration branch until the orchestrator merges it.
trade-off-accepted: On a `lane/**` push with no `[ci-darwin]` opt-in, the three macOS jobs do not run — no native macOS fmt/clippy/nextest/audit/release-smoke/eval-gate, and no macOS binary artifacts. Detection of macOS-only breakage moves from the lane to the integration branch, one serial merge hop later, and never past the gate to `main`. Linux and Windows coverage on lane pushes is completely unchanged.
new-finding: "ubuntu-latest is ALSO congested (live census 32 queued / 13 running), which invalidated my own first implementation — a `needs:` setup job that sat queued 14+ min in front of all compute. Reverted to a zero-job inline expression. Second new finding: the head-of-line integration run is released only when its LAST macOS job completes, so the concurrency group turns over at macOS speed."
fence-exposure: "ZERO on both shared files. `git diff 15cda12d -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs` is empty (known-positive: the same command reports 183+/28- on .github/workflows/ci.yml). No crates/ changes at all. Paths touched: .github/workflows/ci.yml, .planning/CI-MACOS-BUDGET*.md, .planning/evidence/ci-macos-budget/."
status: complete
---

# macOS runner budget — repair of CI starvation on the integration branch

Lane `ci-macos-budget`. Branch `lane/ci-macos-budget`, merge-base `15cda12d`.
Working notes and the full measurement log: `.planning/CI-MACOS-BUDGET-NOTES.md`.

---

## 1. Did the handed-down diagnosis hold?

**Yes on its central claim. One of its suggested remedies is refuted.**

I re-measured from the live API rather than inheriting `.planning/CI-STARVATION.md`.

**Held.** macOS runner scarcity is the cause and the `lane/**` push trigger is the multiplier.
I put sharper numbers on it than the original finding had:

| quantity | measured |
|---|---|
| hosted macOS concurrency ceiling | **5** (peak concurrent *executed* macOS jobs; a later live census showed exactly 5 `in_progress` against 36 `queued`) |
| macOS throughput | **11.25 executed jobs/hour**; median job 19.6 min, p90 26.8 min |
| macOS demand | **22.4 jobs/hour** — **~2x capacity, permanently**; the queue grew ~11 jobs/hr and could never drain |
| share of demand from `lane/**` | **96.3%** (234 of 243 macOS jobs created in a 10.84 h / 303-run window) |
| integration branch's share | **3.7%** — nine macOS jobs in 10.8 hours, seven of which executed |
| observed macOS queue waits | **4h06m, 5h03m, 5h31m** |

**macOS is specifically the constrained pool**, established with a within-run control: in run
`30421332107`, every job was dispatched at the same instant (04:20:17Z), and `ubuntu-latest`
started in 1-8s and `windows-latest` in 2-5s while the macOS jobs were still queued **80+
minutes later**. That rules out account-wide throttling.

**Refuted — "give the integration branch its own concurrency group".** `concurrency.group` is
`CI-${{ github.ref }}`, which is *already* per-ref, so a lane run and an integration run are in
different groups and cannot evict each other. Measured directly: integration run `30424714942`
was created 05:18:59Z and cancelled **05:33:49Z**; three lane runs created inside that interval
(05:17:11Z, 05:29:44Z, 05:32:17Z) did **not** cancel it, and the **integration** push at
05:33:48Z did, one second before. Lane traffic starves the integration branch of *runners*, it
does not evict its runs. That option would have been inert.

**Refined.** The head-of-line integration run is released only when its **last** macOS job
finishes. Run `30399974106`'s final job (`Build (x86_64-apple-darwin)`) completed 04:20:16Z and
the next integration run was dispatched **04:20:17Z** — one second later. So the concurrency
group turns over at macOS speed, which is why the branch's verdict rate collapsed to ~1 per 7 h
on an already-stale SHA while every intermediate push was discarded.

### Instrument defects I found in my own work, and repaired (LANE-BRIEF §6b-ii)

1. **Concurrency sweep counted cancelled jobs.** A cancelled macOS job never runs but still
   carries an `started_at`(enqueue) → `completed_at`(cancel) span — up to **368 min**. Including
   them reported **61 concurrent** macOS jobs. Filtering to `success|failure` gives **5**. I
   nearly reported 61. Repaired, with a three-assertion self-test: known-positive (2 overlapping
   → 2), known-negative (2 disjoint → 1), and **the old matcher would have missed it** (fixture
   with one real + two never-ran jobs: old=3, new=1). Without the third assertion the self-test
   passes on the broken instrument too.
2. **A jq type error produced a silent zero.** `.id+"\t"+.head_branch` fails with
   `cannot add: number and string`, so an artifact sweep returned "no lane run has a Darwin
   artifact". That is an absence claim from a dead instrument. A known-positive probe
   (`total_count` on one run → 6 artifacts) exposed it; the corrected sweep found
   `lane/24-gateway-surface` run `30419996325` carrying `wayland-core-x86_64-apple-darwin`.

---

## 2. What I changed

One file: `.github/workflows/ci.yml`.

On a `lane/**` push the three macOS jobs (`CI (macos-latest)`,
`Build (aarch64-apple-darwin)`, `Build (x86_64-apple-darwin)`) are scheduled **only on opt-in**,
via `[ci-darwin]` (alias `[ci-macos]`, case-insensitive) in any pushed commit message.
`main`, `plan/f20-unified-audit-repair` and every `pull_request` keep the **full, unmodified**
matrix.

**The `lane/**` trigger is NOT deleted.** It exists so a lane that cannot compile for Darwin or
Windows locally can obtain a CI-built binary for its unmerged changes. Deleting it would
re-break that — this program's most-repeated failure mode. Instead:

- **Windows stays unconditional.** It is not constrained (2-5s waits, plus a self-hosted
  runner), so half the capability the trigger was added for costs nothing and is untouched.
- **macOS becomes on-demand** rather than on-every-push.

Job names and artifact names are **byte-identical** (`CI (macos-latest)`, `CI (Array)`,
`Build (aarch64-apple-darwin)`, `wayland-core-<target>`), so nothing keying on a check name
breaks. Every removed line in the diff is a matrix literal now produced by the expression; no
step, job, trigger or artifact upload was removed.

### Implementation, and the first version I threw away

The first cut computed the matrices in a small `budget` job that `ci` and `build` depended on.
**I measured it and reverted it.** It puts a new serialisation point in front of all compute,
and a live census showed `ubuntu-latest` is *itself* congested — **32 queued / 13 running**. On
run `30426418225` that job sat queued **14+ minutes** before `ci` and `build` were even created:
a self-inflicted regression on the critical path of every run, on the exact metric this change
exists to improve.

The shipped version evaluates the condition inline in `strategy.matrix`, costing **zero extra
jobs and zero hops**. It is also strictly safer: `head_commit.message` and `commits.*.message`
are only ever operands of `contains()` inside the expression evaluator, so they never reach a
shell, and the expression can only select between two JSON literals written in the file —
attacker input can toggle *which* literal is chosen, never alter either one.

A job-level `if:` cannot do this: `jobs.<id>.if` cannot read the `matrix` context, so it can
only skip an entire job — which would also drop the self-hosted Windows CI leg and all four
non-Darwin build targets. Splitting the macOS legs into separate jobs would duplicate ~140 lines
of steps, and this file already documents a real defect caused by exactly that drift (the
`--no-fail-fast` divergence that made every historical CI failure count a lower bound).

The panel's unanimous ask — **make the skip loud** — is a step inside the existing `ci-linux`
job, so reporting also costs no extra job. It writes a `DARWIN_SKIPPED` / `DARWIN_REQUESTED`
run-summary table and a run-level annotation naming the token.

### Gate

`.planning/evidence/ci-macos-budget/gate.py` **extracts** the condition from the workflow rather
than hardcoding it (a hardcoded copy would be a tautology), evaluates a 9-case truth table,
checks matrix-shape invariants, and asserts the three condition copies are byte-identical.

It is proven able to fail by four mutation arms (`gate.py --self-test`), each mutating the real
workflow in memory:

| mutation | gate result |
|---|---|
| unmutated | `GATE_FAILURES=0` |
| drop the integration-branch clause | **1 failure** — integration case flips to false |
| break the `[ci-darwin]` literal | **3 failures** — all token cases flip |
| drift one condition copy | **1 failure** — byte-identity check |
| drop `aarch64-apple-darwin` from the full matrix | **1 failure** — shape invariant |

---

## 3. Live proof

### 3a. Controlled before/after on one branch

Both arms are pushes to `lane/ci-macos-budget`, same repo, same shared macOS queue, 30 min apart.

| arm | commit | config | jobs at dispatch | macOS jobs |
|---|---|---|---|---|
| **A (baseline)** | `27df8b7c` | unmodified `ci.yml` | 11 | **3** |
| **B1 (repaired)** | `d8daf8e0` | this change, no token | 8 | **0** |

Arm A was a **docs-only** commit (a notes file) and still scheduled three macOS jobs — which is
the waste, stated plainly. Arm B1 (run `30427513255`) dispatched all 8 jobs at 06:14:11Z and
retained: `CI (Array)` self-hosted Windows, `CI (linux-containerized)`, `Build (x86_64-pc-windows-msvc)`,
`Build (aarch64-pc-windows-msvc)`, `Build (x86_64-unknown-linux-gnu)`,
`Build (aarch64-unknown-linux-gnu)`, the eval gate and browser-live.

Arm A additionally demonstrated the failure in miniature: its macOS jobs never obtained runners,
and because a run holds its branch's concurrency slot until it completes, **it blocked the next
run on my own branch** until I force-cancelled it. That is the integration branch's pathology
reproduced under controlled conditions.

### 3b. The counterfactual — the old config was evicting runs in the same window

The obligation is to show the pre-change configuration would have evicted the run now shown
surviving. In the **same 22-minute window** as arm B1, on the same repo and the same macOS queue,
the integration branch — still running the **unmodified** config — produced this, via the
identical `jobs.total_count` call:

| run | branch | config | created | jobs |
|---|---|---|---|---|
| 30427396244 | plan/f20… | **old** | 06:12:02Z | **0** |
| 30426790209 | plan/f20… | **old** | 06:00:51Z | **0** |
| 30426629499 | plan/f20… | **old** | 05:57:50Z | **0** |
| 30426570745 | plan/f20… | **old** | 05:56:39Z | **0** |
| 30426416317 | plan/f20… | **old** | 05:53:34Z | **0** |
| 30426366364 | plan/f20… | **old** | 05:52:36Z | **0** |
| **30425956850** | lane/ci-macos-budget | **old** | 05:44:18Z | **12** ← known-positive |
| **30427513255** | lane/ci-macos-budget | **new** | 06:14:11Z | **8, zero macOS** ← known-positive |

The two known-positives are load-bearing: they prove the query discriminates, so the uniform
zeros are a result and not a dead instrument. **A green run would not have proved anything here;
what is proved is that six runs under the old config obtained no runner at all during the exact
window in which the repaired config dispatched immediately.**

### 3c. A lane can still obtain a macOS binary

Proven end-to-end, on this arm64 Mac, using the artifact-upload step this change leaves
byte-identical:

```
$ gh run download 30399974106 -n wayland-core-aarch64-apple-darwin
$ file wayland-core
wayland-core: Mach-O 64-bit executable arm64
$ ./wayland-core --version
wayland-core 0.12.25          # rc=0, on uname -m = arm64
```

And the route works **from a lane branch**, not only from the integration branch: run
`30419996325` on `lane/24-gateway-surface` carries `wayland-core-x86_64-apple-darwin`.
Under this change that same route is reached by adding `[ci-darwin]` to the commit:

```
git commit --allow-empty -m "[ci-darwin] need a macOS binary for live testing"
```

### 3d. What is NOT proved, and cannot be by this lane

**The integration branch has not been observed surviving under the repaired config, because the
repaired config is not on it.** Workflow changes take effect from the ref being pushed, and I am
fenced from merging. The remaining step is arithmetic, and I am labelling it as such rather than
implying it was observed: removing lane macOS demand takes demand from **22.4 → ~0.83 jobs/hour**
against **11.25/hour** capacity, i.e. from 2x oversubscribed to roughly 13x headroom.

**And the honest ceiling on the claim** (this is the strongest objection to my own work, from the
adversarial pass): even with macOS unblocked, the integration branch still serialises through its
per-ref concurrency group, and a full run's critical path is ~40-50 min — set by
`CI (linux-containerized)` (40 min) and `CI (Array)` (32 min), **not** by macOS. Integration
pushes land roughly every 15 min. So the expected outcome is **a verdict roughly every 45
minutes on a ≤45-minute-old SHA, not a verdict on every push.** Two of every three SHAs will
still be superseded while pending. That is a ~10x improvement over the measured baseline of
**zero verdicts in nine hours**, and it is not the same as full coverage.

---

## 4. The trade-off I am accepting

**On a `lane/**` push with no opt-in token, this stops being covered:**

- native macOS `fmt`, `clippy`, full `nextest`, `cargo audit`, release-binary smoke and the
  eval acceptance gate;
- the arm64 and x86_64 macOS release compiles;
- both downloadable macOS binary artifacts;
- therefore lane-local detection of macOS-only failures: arm64-specific behaviour,
  case-insensitive APFS path assumptions, BSD-vs-GNU userland differences, and macOS-only
  dependencies such as keychain and fsevents.

**Unchanged on those pushes:** the full Linux containerized CI (which still runs fmt, clippy,
the whole nextest suite and audit), the full self-hosted Windows CI leg, both Windows release
builds, both Linux release builds, the eval gate and browser-live.

**Why acceptable.** Detection moves later by exactly one serial hop and never past the gate to
`main`: the integration branch runs the unmodified macOS matrix on every push and lanes merge
serially through it, so a macOS break is still caught and still attributable. Against that,
today's lane-level macOS check **does not complete at all** — it is a check that never runs, and
a check that never runs detects nothing. Converting a theoretical per-lane check into an actual
per-integration one is a strict improvement, not a reduction.

**Residual risks I am NOT claiming to have closed:**

1. **Silent absence.** A token-less lane run goes green having never touched macOS. Mitigated —
   the missing jobs are visibly absent from the checks list and the run summary says
   `DARWIN_SKIPPED` with the reason and the remedy — but *mitigated, not eliminated*. Nothing
   fails when a lane forgets the token.
2. **Reflexive opt-in.** Nothing stops lanes adding `[ci-darwin]` to every push, which would
   restore the original problem. There is headroom for roughly three opt-in lane runs per hour
   before saturation returns; there is no enforcement.
3. **ubuntu-latest congestion is untouched.** This change removes macOS demand only. The live
   census showed 32 ubuntu jobs queued. If ubuntu becomes the next binding constraint, the same
   analysis will need repeating against a different pool. I did not expand scope to it.

---

## 5. Cross-audit

Panel (LANE-BRIEF §4) on the mechanism: **3-0 ENDORSE** — codex `gpt-5.6-sol`,
`gemini-3.1-pro-preview`, kimi K3. Votes extracted unanimously, unanchored, last-match-per-file.
codex initially returned zero bytes (`Failed to read prompt from stdin`) and was re-run with
stdin closed rather than counted as a dropped vote.

On the sub-question *"drop the redundant-looking `Build (aarch64-apple-darwin)` job?"* the panel
split **2-1 against** (kimi NO, codex NO, gemini YES-with-artifact-transfer). **I took the
majority and kept the job.** gemini's variant — move the upload into `CI (macos-latest)` — is
worse for the stated use case: that job gates on fmt, clippy and the full suite *before* it
would reach an upload step, so a lane whose tests are red would lose its binary, which is
precisely when a lane most needs one. The saving would have been one third of 3.7% ≈ **1.2% of
pool load**. Trading a measured 1.2% for re-breaking a deliberately-added capability is the
wrong trade.

The internal adversarial pass (`evidence/ci-macos-budget/adversarial.md`) is the reason §3d and
§4 are worded as they are: it forced two claims down — the fix restores a ~45-minute verdict
cadence rather than per-push coverage, and silent absence is mitigated rather than closed.

---

## 6. Fences

- **Shared-file fence: CLEAN.** `git diff 15cda12d -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` → zero bytes. Known-positive: the same command on
  `.github/workflows/ci.yml` reports `183 insertions(+), 28 deletions(-)`, so the instrument is
  alive. No `crates/` change of any kind.
- Paths touched: `.github/workflows/ci.yml`, `.planning/CI-MACOS-BUDGET.md`,
  `.planning/CI-MACOS-BUDGET-NOTES.md`, `.planning/evidence/ci-macos-budget/`.
- **Not done (reserved):** no merge to `main`, no PR, no tag, no release, no GitHub issue
  closed, no `wcore-contract generate`.
- **Run cancellations:** three, all belonging to **my own** lane branch
  (`30425956850`, `30426418225`, and their supersessions). **No run belonging to another lane
  was cancelled** — in particular the queued runs for `24-media-live`, `24-media-bounds`,
  `24-reconnect`, `24-msteams-attach` and `openapi-consumer` were left untouched.
- Numbers in this document come from `gh api` and from `/usr/bin/git` / `/usr/bin/grep` /
  `/usr/bin/awk` absolute paths, per LANE-BRIEF §3b. Pushes were verified by comparing
  `git ls-remote gh` against local `HEAD`, never by exit status.

---

## 7. Recommended follow-ups (not done here)

1. **Document the token** in the lane agent template / `AGENTS.md`, so a lane needing a Darwin
   binary knows the route exists. Without that, risk 1 above is much more likely to bite.
2. **Consider a self-hosted macOS runner.** It raises the ceiling rather than cutting demand,
   and is the supply-side complement to this change. kimi and codex both flagged it as a
   legitimate later step, not a first one.
3. **Add `workflow_dispatch` to `ci.yml` on `main`.** Two separate comments in this file record
   escape hatches rejected solely because the default branch lacks that trigger. It is a
   Sean-reserved change to `main`, but it would retire a recurring constraint.
