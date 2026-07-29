---
phase: 23A-governed-skills
graded_by: lane/grade-23a
graded_at: 2026-07-29
base_sha: 861d1b1a716240165209336b1fa38d36f9445716
scope: "grading only — no crates/ edit, no workflow edit, no build, no merge"
criteria_owned: 1
criteria_met: 0
verdict: "SUCCESS CRITERION 1 NOT MET at base — and the standing record's REASONS are stale in both directions"
pending_work_graded: "lane/23a-c1-governed @ 3a2234d7 (NOT merged) — would plausibly close it"
new_high: "F23A-C1-H3 — base rollback() restores non-atomically into the live skills directory; a kill mid-restore leaves a partial skill. Present and unfixed at 861d1b1a."
---

# Phase 23A — Governed Skills — PHASE VERDICT

**No verdict file existed for this phase before this one.** The phase was never graded in
either direction; what stood in its place was `23A-04-SUMMARY.md` (a plan summary that
discharged its own Task 3) and a row in `ROADMAP.md:221` quoting it.

**Headline: Success Criterion 1 is NOT MET at `861d1b1a`. But every specific reason the
standing record gives for that is now wrong** — two claims are stale in the phase's favour and
one claim is stale against it. Re-derived from the tree, not inherited.

---

## 0. What 23A owns

| | |
|---|---|
| **Success Criterion** | Phase 23 SC-1 (`ROADMAP.md:102`), assigned to 23A by `ROADMAP.md:109` |
| **Text** | *"Generated skills cannot execute before governed promotion and can be observed, revoked, and rolled back."* |
| **Requirement** | **F23-01 only.** All four 23A plans declare exactly `F23-01` and nothing else (verified per-plan). |
| **Not 23A's** | SC-2…SC-6 and F23-02…F23-06 — `ROADMAP.md:109` assigns those to 23B. |

SC-1 is a **four-clause conjunction**, graded one clause at a time. It is MET only if all four
hold. `F23-01` is broader than SC-1 (it adds `detect, draft, evaluate, review/policy` stages);
F23-01's residual is reported in the gap list, not folded into the SC-1 grade.

---

## 1. The record was stale in BOTH directions — this is the finding

| Standing claim | Source | True at `861d1b1a`? |
|---|---|---|
| "Clause 3 *can be revoked* **NOT MET** — nothing implements revocation" | `23A-04-SUMMARY.md:77`, quoted into `ROADMAP.md:221` | **STALE — false.** `wcore-skills/src/govern.rs` is **566 lines** with `revoke()` :221, `rollback()` :290, append-only `journal()` :426, `is_revoked()` :370. Merged at `460fad3b`, 2026-07-29 08:50. The summary is dated 2026-07-26. |
| "Clause 4 *can be rolled back* **NOT MET**" | `23A-04-SUMMARY.md:79` | **STALE — false.** Same module. |
| "F23A-01-H2 open and committed red — any errored tool call kills the session" | `ROADMAP.md:221`, all four 23A summaries | **STALE — fixed.** `32a5fc90` (fix) and `81508b74` (RED-first test) are **both ancestors of my base**. Five regression tests wired at `orchestration/mod.rs:78`, one of which is a control. |
| "the capability ships and is drivable today" | `23A-C1-SUMMARY.md:53` | **FALSE, in the phase's disfavour.** It ships to **nobody** — see §2.c. |

`lane/record-truth` wrote `23A-STATUS-CORRECTION.md` recording the H2 correction. **That file is
not in my base** (`11a6c044` is not an ancestor) — so the correction exists but is invisible to
anyone reading the phase directory. That is itself a record defect worth fixing at merge.

---

## 2. Per-clause grade at base `861d1b1a`

### Clause (a) — "cannot execute before governed promotion" → **MET-WITH-STATED-EXCEPTIONS**

**Evidence.** The 16-route quarantine census was **re-measured at HEAD** (`2a315b83`,
`6ae6e611`, both in base) and, crucially, **its control was made to fire for the first time**:

- `evidence/23A-census/run-E-baseline-final.log:11` →
  `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
- `evidence/23A-census/run-F-selftest-final.log:25` →
  `test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 filtered out`
- `run-F` lines 5–8: under `WAYLAND_F23A_SELFTEST=refusal`, **all four** route checks flip to
  `refused=false`. The runs disagree; the control discriminates on every driven route.
- `run-F:9` — `F23A-SELFTEST-LEGACY: R7 /skill list old_matcher=true`: the **pre-repair**
  matcher declared a quarantined draft hidden while being handed a *visible* control skill. A
  self-passing gate found inside the phase's own instrument, and repaired in-lane.
- Enforcement sites read directly at base: `loader.rs:448` sets `disable_model_invocation`;
  `slash/skill.rs:115-117` refuses *"this skill is quarantined and cannot be run."*;
  `refs.rs` filters at :129/:294/:313/:325/:335.
- `run-status-sentinels.txt` — `WLRC=0`/`WLDONE` and `WLRC=101`/`WLDONE`, read back by a
  separate call. Exit status never relied on.

**Why not a clean MET — two stated exceptions:**

1. **Coverage: 4 of 16 routes are live-driven.** 10 are STATIC@HEAD (citations re-resolved, not
   driven) and 2 are UNREACHABLE@HEAD. The two highest-value undriven routes are **R2**
   (system-prompt listing) and **R5** (per-turn router hint) — precisely the two that put
   content into a *model-visible payload*, which is the criterion's own boundary. They are
   undriven for a named, structural reason: `OpenAiFixtureScript` retains only hashes
   (`crates/wcore-eval-scenarios/src/fixtures/openai.rs:311-321`, intent stated at :317), so a
   nonce cannot be searched for. That is a recorded limit with a cause, not a silent drop.
2. **"Before governed promotion" has no promotion to be before.** At base
   `run_skills_promote` (`crates/wcore-cli/src/main.rs:2537`) is an **unconditional `bail!`**.
   The pre-promotion state is inert because it is *permanently* inert. The property is real and
   now genuinely proven — I do **not** repeat `23A-04`'s "vacuous" grade, because the census
   upgraded this from assertion to measurement — but the governance semantics the clause names
   are not exercised.

### Clause (b) — "can be observed" → **PARTIAL**

**Met, and live-driven:** `/skill list` tags the draft `(hidden)` on its own rendered line
(`slash/skill.rs:148-164`, matcher repaired and bound — R7 LIVE-DRIVEN); `/skill show` reports
`visibility: hidden from model` and withholds the body (`slash/skill.rs:172,193` — R8
LIVE-DRIVEN). Both are on the **shipped** binary.

**The H2 caveat that `23A-04-SUMMARY.md:73` attached to this clause is void** — H2 is fixed in
base with a controlled regression suite. Observation is no longer "not survivable".

**Not met:** *governance* observation. The append-only journal, `live_revocations()` and
`history` exist in `govern.rs` but their only surface is `wcore-skill-govern`, which does not
ship (§2.c). An operator who installs `wayland-core` can see **what is quarantined** and cannot
see **what was revoked, when, by what authority, or what is retained**.

### Clause (c) — "can be revoked" → **PARTIAL**

**The capability is implemented and proven.** `govern.rs::revoke()` :221 — retains every byte,
makes suppression durable, *then* removes; idempotent; symlink-refusing; size- and depth-capped.
Gate counts from `23A-C1-SUMMARY.md:151-152`, executed counts read back:
`govern_revoke_rollback` `15 passed; 0 failed; 0 ignored; 0 filtered out` and `govern_cli_drive`
`6 passed; …; 0 filtered out`, with a red-before-green mutation (`is_revoked → false`) turning
the suite `6 passed; 1 failed`.

**It reaches no customer.** Measured by me, with a live control in the same invocation:

```
git show 861d1b1a:.github/workflows/release.yml | /usr/bin/grep -c "wayland-core"   -> 31   [KNOWN-POSITIVE]
git show 861d1b1a:.github/workflows/release.yml | /usr/bin/grep -c "skill-govern"   ->  0   [the absence]
git grep -n "wcore-skill-govern" 861d1b1a -- ':!*.rs'  -> 9 hits, ALL of them .planning/ docs
```

`wcore-skill-govern` is a dev-only auto-discovered bin of a library crate, in no workflow and no
packaging manifest. And `crates/wcore-cli/src/main.rs` at base has **no** `skills_revoke` flag at
all — control: `skills_archive` **is** present at :483/:1546 in the same grep, so the instrument
is alive. **A revocation the shipped product cannot perform does not satisfy "can be revoked"
for the user the criterion is written about.**

### Clause (d) — "can be rolled back" → **PARTIAL, and it carries a new HIGH**

Same shipping gap as (c) — no `skills_rollback` flag on the shipped binary at base.

**And worse: the base implementation is not crash-safe.** Read directly from base:

- `govern.rs:290 rollback()` restores with a bare `copy_tree(&payload, &record.source_dir)`
  straight into the **live** skills directory.
- `copy_tree` (:536) is `create_dir_all` + per-file `std::fs::copy` (:561). **No staging
  directory, no `rename(2)`.** The `sync_all` at :454 lives inside `atomic_write`, which is used
  for the JSON records only — never for the restored tree.

→ **F23A-C1-H3 (HIGH, new, unfixed at base):** a kill mid-rollback leaves a *partially restored
skill directory* — present, loadable, missing content. Same defect class as the migration data
loss already on this programme's record, on the recovery verb itself. The pending lane measured
this at **`PARTIAL 11/35` kills** against exactly this code; I did not re-execute that harness
(§4), but the code shape at base is precisely what produces it.

### Overall at base: **SUCCESS CRITERION 1 — NOT MET**

One clause MET-WITH-STATED-EXCEPTIONS, three PARTIAL. The honest one-line reason is **not** the
one on record. It is: *revocation and rollback are built and proven, but reach no shipped
surface; governed promotion does not exist; and the rollback that does exist is not crash-safe.*

---

## 3. The pending lane — graded separately, and clearly marked

`lane/23a-c1-governed`, branch head **`3a2234d7`**, **NOT merged** (`git merge-base
--is-ancestor 3a2234d7 861d1b1a` → rc=1). Its deliverable `.planning/23A-C1-GOVERNED.md`
self-grades `23A-C1: MET-for-the-shipped-surface`. Diff vs its merge-base `75babf32`: 20 files,
**+2941 / −188**.

**What I verified in its source myself** (not taken on trust):

| Claim | Verified at `3a2234d7` |
|---|---|
| promotion exists | `crates/wcore-skills/src/promote.rs`, 589 lines. **ABSENT at base** (rc≠0) with `loader.rs` present at both as control, so the negative is not a dead instrument. |
| grant is content-bound | `content_digest()` :527 → `sha256:{:x}` :542; grant field :108; `promotion_state()` :234 compares found vs granted, mismatch path :249. |
| revoked artifacts are refused | `Refusal::Revoked` :122, revocation checked **first** at :281 and :327. |
| one governance choke point | `loader.rs:185 apply_governance`, called at **:83 and :156** — both `load_all_skills` and `load_catalog`. |
| the capability reaches the shipped binary | `main.rs` :479 promote (un-hidden), :496 revoke, :502 rollback, :507 govern; dispatch :1566–1583. |
| the flag guard was inverted, not deleted | `skills_promote_advertised_and_works.rs`, **5 `#[test]`**; the negative test :151 carries an explicit positive control. |
| `ProcedureStatus::Revoked` | `wcore-memory/src/v2_types.rs`, +80. |

**If merged and independently re-verified, this closes all four clauses**: (a) stops being
promotion-less, (b) gains `--skills-govern`, (c) and (d) gain shipped surfaces, and F23A-C1-H3
is fixed by the staged-then-`rename(2)` restore.

**Confidence, stated explicitly.** Its live figures — 34 assertions / 0 failures, and 0
destructive states over 70 kills — are **audited but NOT re-executed by me**. Its hetzner
worktree `/root/wayland-23a-c1` is gone (verified: `No such file`), and rebuilding `wcore-cli` on
a host carrying 21 other lane worktrees is not a grading lane's call. What I *did* establish is
that its harness **can fail**: `kill-23a-c1-distribution.sh` carries a vacuity guard
(`CO==0 || GR==0` → `INVALID MEASUREMENT`, exit 2 — which **already fired once** and killed a
v1 run where 35/35 trials never reached the write), self-calibrates its kill window, seeds
delays deterministically, **retries every NOT-STARTED trial and asserts recovery**, and backs its
`PARTIAL=0` with a reddening control at `PARTIAL=11/35`. That last point matters most: it is a
known-positive for a known-negative claim, which is the one thing this programme has repeatedly
been burned for omitting.

---

## 4. Instrument warnings that bear on this grade

- **`no-tests = "fail"` is silently ignored** by the installed nextest. Every count I quote is a
  read-back `N passed; … 0 filtered out` line from a committed log or a lane summary, never an
  exit status. The census's `0 filtered out` is load-bearing: `f23a_boundary_drive.rs` is
  `#![cfg(feature = "packaged-driver-gate")]`, so omitting the feature yields a target with
  **zero** tests that exits 0.
- **`cargo nextest` "flakiness" in this repo was fd/inotify exhaustion, never a real failure.**
  No red I encountered was graded as a regression on that basis.
- **`rtk` rewrites `git`, `grep` and `cargo` output.** Every number in this verdict came from
  `/usr/bin/git` or `/usr/bin/grep`.
- **Every absence claim here carries a known-positive in the same invocation** (31 vs 0 for
  packaging; `skills_archive` present vs `skills_revoke` absent; `loader.rs` present vs
  `promote.rs` absent). Per LANE-BRIEF §3b-i, an unguarded absence is self-passing, and the
  prior "nothing implements revocation" grade was exactly that shape — and was wrong.
- **Not graded by me:** whether the census's 10 STATIC@HEAD routes would survive being driven.
  Un-driven is un-driven; I did not upgrade them.

---

## 5. GAP LIST — what is missing, what it costs

Cost = lane-sessions. "Credential?" = needs a Sean-reserved secret/authority, vs pure build work.

| # | Missing capability | Cost | Credential? |
|---|---|---|---|
| G1 | **Governed promotion** — `run_skills_promote` is an unconditional `bail!` at base | **0.5** to merge + re-verify `3a2234d7` (it is built); **2–3** if rebuilt from scratch | No — pure build |
| G2 | **Revoke/rollback/govern verbs on the shipped binary** — `wcore-skill-govern` is in no packaging manifest | **0.5** (same merge as G1; four flags already written) | No — pure build |
| G3 | **F23A-C1-H3: crash-safe rollback** (stage outside the skills tree, fsync, `rename(2)`) — HIGH, live in base | **0.5** (in `3a2234d7`); **1** standalone if that lane is rejected | No — pure build |
| G4 | **Drive R2 + R5**, the two model-visible-payload routes — needs an opt-in request-retention mode on `OpenAiFixtureScript` (shared file → seam decision) plus one assertion pair | **1** | No — pure build |
| G5 | **Drive R9 / R16** (cron sink, cross-project sibling) — needs a scheduled cron entry and a `cross_project_root` with a sibling carrying `memory.db`. *R10 is unreachable-by-construction at a quarantined draft and should be recorded, not driven* | **1** | No — pure build |
| G6 | **F23-01's `evaluate` and `review/policy` stages** — no implementation in base or in the pending lane | **2** | No — pure build |
| G7 | **Purge `evolved_prompts` / `Procedure` rows on revoke** — pending lane achieves *unreachability* only; deletion needs `wcore-skills → wcore-evolve`, which the crate graph forbids | **1** + an architectural decision | No — pure build |
| G8 | **Promotion authority beyond "which surface invoked it"** — no identity system exists; the grant records a command, not a principal | **2+**, architectural | No — pure build |
| G9 | **macOS coverage of the boundary drive** — 23A-04 Task 2 never ran. Its stated premise ("no macOS binary obtainable") is **FALSE**: CI has uploaded `wayland-core-aarch64-apple-darwin` since `d9c7683b` | **0.5** | **Yes** — needs a Sean-gated CI dispatch (hetzner cannot reach a macOS host; a workspace build on the Mac is barred) |
| G10 | **Merge `23A-STATUS-CORRECTION.md`** (on `lane/record-truth`, unmerged) so the phase directory stops reporting a fixed HIGH as open | **0.1** | No — record only |

**G1+G2+G3 are one merge.** If `lane/23a-c1-governed` merges and is independently re-verified,
SC-1 plausibly moves to MET and the remaining gaps (G4–G9) are coverage and F23-01 residue, not
criterion blockers.

---

## 6. Phase goal verdict

> **Phase 23 goal:** *"The agent can pursue verified outcomes over time, learn safely, and let
> users inspect, correct, recover, and control that state."* — 23A's share is the
> **learn-safely / inspect / recover / control** half for generated skills.

**NOT ACHIEVED at `861d1b1a`.** A user of the shipped product can *inspect* a quarantined draft
and is genuinely protected from it executing — that half is real and, since the census, is
measured rather than asserted. The user cannot *correct*, *recover* or *control* it: no
promotion, no revoke verb, no rollback verb, and the rollback that exists internally can leave
their skills directory in a partial state.

**The phase's real output was not the criterion.** It was four HIGH-grade findings — H1
(authority boundary failing open in every fresh clone, fixed), H2 (any errored tool call kills
the session, fixed at `32a5fc90`), the shipped-surface gap, and F23A-C1-H3 — plus two
self-passing gates found *inside the phase's own instruments* and repaired. That is worth more
than a green would have been, and it is why this verdict corrects the record in both directions
rather than simply restating NOT MET.

---

## 7. Compliance

- **Fences held.** Changes vs `861d1b1a` are two files, both `.planning/phases/23A-governed-skills/`:
  `23A-PHASE-VERDICT.md` (new) and `23A-GRADE-NOTES.md` (new). **Zero `crates/` files, zero
  `.github/workflows/*`, zero shared-fence files** (`wcore-cli/src/{lib,main}.rs` untouched).
- No merge, no PR, no tag, no publish, no issue closed, no `wcore-contract generate`.
- No build run anywhere. One read-only `ssh hetzner-dsm` (`ls`, `git worktree list`, `df`).
- No credential used, none needed, none printed.
- No `git add -A`; no `checkout`/`reset`/`stash`/`rebase`.

_Graded 2026-07-29 · base `861d1b1a` · lane/grade-23a · source + committed-evidence measurement;
the pending lane's live figures audited, not re-executed._
