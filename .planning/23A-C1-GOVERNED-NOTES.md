# 23A-C1 governed promotion / revocation / rollback — WORKING NOTES

Lane `23a-c1-governed`, branch `lane/23a-c1-governed`, base `75babf32`.
Append-and-recommit after every measurement (LANE-BRIEF §6b-i).

---

## T+0 — the inherited measurement is STALE in one direction and correct in another

The dispatch brief carried three claims from a prior lane and told me to re-verify
rather than inherit. Re-verified at `75babf32`:

| Inherited claim | Re-verified verdict at `75babf32` |
|---|---|
| `ProcedureStatus` has **no `Revoked` variant** | **TRUE.** `crates/wcore-memory/src/v2_types.rs:359-364` — `Staged, Active, Archived, Pinned`. Four variants, no `Revoked`. |
| **No generation store exists** to roll back to | **FALSE — now stale.** `crates/wcore-skills/src/govern.rs` (566 lines) implements a generation store: `generations/<id>/payload/`, `tombstones/<id>.json`, append-only `journal.jsonl`, with `revoke()` / `rollback()` / `live_revocations()` / `is_revoked()` / `record_suppression()`. |
| **No artifact-provenance binding** exists | **PARTIAL.** `Revocation` records `skill_name` + drafter `signature` + `source_dir` + counts. That is revocation-side provenance. There is no *promotion*-side provenance record, because there is no promotion. |
| `--skills-promote` hidden and still exits 1 | **TRUE.** `main.rs:474` declares it `hide`; `main.rs:2537 run_skills_promote(_id: &str)` ignores its argument entirely and is an unconditional `bail!`. |

**So the shape of the remaining gap is not what the brief assumed.** Revocation and
rollback were built by a prior lane. The hole is **promotion**, plus whatever of the
revocation path is unproven on the real binary.

### Search queries run (per LANE-BRIEF §3b-i — an absence needs its query)

All through `/usr/bin/grep`, unproxied.

```
/usr/bin/grep -rn "GovernanceStore" --include='*.rs' crates/          -> 17 hits   [known-positive control, instrument ALIVE]
/usr/bin/grep -rn "is_revoked|record_suppression|govern::" --include='*.rs' crates/
/usr/bin/grep -rn "enum ProcedureStatus" -A 12 --include='*.rs' crates/
/usr/bin/grep -rn "skills_promote|skills-promote" --include='*.rs' crates/
```

The `GovernanceStore` search is the deliberate known-positive: it returns 17, so a zero
from the same instrument in the same session means zero and not a dead grep.

## T+0 — what IS wired

- `crates/wcore-agent/src/auto_skill/drafter.rs:79-131` — the drafter resolves a
  `GovernanceStore` and calls `is_revoked(&name, Some(&trigger.signature))`, and on a hit
  calls `record_suppression`. **The drafter-side resurrection path is guarded.**
- `crates/wcore-skills/src/bin/wcore-skill-govern.rs` (251 lines) — a **separate binary**,
  not a subcommand of the shipped `wayland-core`. Whether a customer can reach it is an
  open question I must measure, not assume.
- `crates/wcore-skills/tests/govern_revoke_rollback.rs` — unit/integration coverage exists.

## T+0 — open questions I must settle by measurement, not reading

1. Is `wcore-skill-govern` actually **shipped**? If it is not in the release artifact, the
   revocation capability is unreachable by a customer and 23A-C1 is no better off.
2. The brief's named hazard: *"auto-draft router hydration can never fire today, but
   becomes a live resurrection hazard the moment promotion lifts quarantine."* The drafter
   path is guarded (above) — **the router hydration path is a different call site** and I
   have not yet checked whether it consults `is_revoked`. This is the one that becomes live
   because of my work.
3. Does a revoked skill actually **fail to execute**, or merely fail to load? Different
   claims. Needs a live known-positive/known-negative pair in one invocation.
4. Interrupted promotion — the program already found data loss on interrupted migration
   (truncating write, 331 orphaned payload dirs, 0 profiles imported, 5/35 kills). Promotion
   writes into the user's global directory and must be proven not to repeat that.

## Plan of record (subject to revision by measurement)

1. Close the router-hydration resurrection hazard (obligation #1 in the brief).
2. Add `ProcedureStatus::Revoked` so revocation is representable in the memory tier, not
   only on the filesystem.
3. Build governed promotion with a provenance record + crash-safe (staged, atomic) install.
4. Restore `--skills-promote` **only after** 1-3 live-prove, per the brief's warning about
   re-creating the advertised-but-dead defect class (9 recorded instances).
5. Live proof on `hetzner-dsm` against the real binary, isolated `WAYLAND_HOME`, with a
   one-variable negative control per leg, plus a repeated mid-promotion kill distribution.

---

## T+25 — NEW FINDING, HIGH: the revocation capability is **unreachable by any customer**

`govern.rs` is real, tested, and correct-looking. **It ships to nobody.**

- The release builds and packages exactly one binary. `.github/workflows/release.yml:30`
  `BINARY_NAME: wayland-core`; six matrix rows all name `wayland-core` / `wayland-core.exe`;
  the upload globs `artifacts/wayland-core-*.tar.gz`.
- `wcore-skill-govern` is a **dev-only auto-discovered bin of a library crate**
  (`crates/wcore-skills/src/bin/wcore-skill-govern.rs`). It is named in **no** workflow, **no**
  packaging script, and **no** manifest `[[bin]]` block. Its only non-source reference in the
  whole tree is its own integration test.

**Instrument check, same file, same invocation (LANE-BRIEF §3b-i):**

```
/usr/bin/grep -c "wayland-core"  .github/workflows/release.yml   -> 31   [known-positive, ALIVE]
/usr/bin/grep -c "skill-govern"  .github/workflows/release.yml   ->  0   [the measured absence]
```

The 31 is the control: the instrument reads that file and returns non-zero for a string that
is there, so the 0 is a real zero rather than a dead grep, a wrong path or an eaten glob.

**Consequence for the grade.** `RC-READINESS` records 23A-C1 as lacking revocation. That is
the right grade for the *wrong reason*: the code exists, **the surface does not**. A user who
installs the release has no command that can revoke or roll back anything. So the top-priority
item in this lane is not "write revocation" — it is **put revocation on the binary customers
actually get**, and everything else is worth less until that is true.

## T+25 — the resurrection hazard, made concrete

The brief's hazard is real and I can now name its mechanism exactly:

1. `GovernanceStore::revoke()` is **filesystem-only**. It deletes the skill directory and
   writes a tombstone. It does **not** touch the P4 `Procedure` row.
2. That row keeps `status: Staged` (or `Active`). `ProcedureStatus` has no `Revoked`
   (`v2_types.rs:359`), so revocation is **not representable** in the memory tier.
3. `can_transition_to` (`v2_types.rs:380-391`) permits `Staged → Active`. Nothing consults
   governance.
4. So a governed promotion that materialises an artifact from a promoted procedure would
   **rebuild the exact directory the user revoked**, and neither the loader
   (`/usr/bin/grep -n "govern|tombstone|revok" loader.rs` → **1** hit, a comment at line 444,
   zero code) nor `transition_procedure` would stop it.
5. `SkillPrioritizer::priority_order` then re-ranks it and `SkillRouter::seed_from_prioritizer`
   hydrates it into the bandit — the hydration half of the hazard.

The drafter path *is* guarded (`auto_skill/drafter.rs:128`). **The promotion path is not, and
promotion is the path I am building.** So closing this is mine, exactly as the brief says.

## Revised plan of record

1. Put `skill-revoke` / `skill-rollback` / `skill-promote` / `skill-govern-list` on the
   **shipped `wayland-core` binary**. Without this nothing else is reachable.
2. `ProcedureStatus::Revoked`, terminal — no transition out of it, so the DB cannot resurrect.
3. Governed promotion: consult `GovernanceStore` and **refuse** a revoked name/signature;
   write a promotion provenance record; install crash-safely (stage + atomic rename).
4. Loader-side enforcement so a revoked skill **cannot execute** even if its directory returns
   by some other route.
5. Restore `--skills-promote` last, only after 1-4 live-prove.

---

## T+150 — the `evolved_prompts` hydration vector, verified rather than accepted

The coordinator relayed a claim from `lane/open-highs`: that the resurrection vector is the
`evolved_prompts` row rather than the `Procedure` row, that my notes never mention it, and
that **my mechanism is the `Procedure` row, "store (c), which your own notes measure as
INERT"**. I was told to verify rather than accept. Verified, and the result splits.

### The substantive half is TRUE and I had missed it

`evolved_prompts` is real and it does hydrate the router.
`crates/wcore-agent/src/bootstrap.rs:2122-2176` seeds the `SkillRouter` from it in two
layers — `scorer="bench"` (GEPA winners) and `scorer="auto_drafter"` (Layer 1b, the
auto-draft read-back) — plus `seed_from_prioritizer` as layer 2. **My notes did not mention
it.** That was a genuine gap in my analysis and the lane was right to raise it.

### The half about *my* mechanism is FALSE, and misattributed

- **The file it cites does not exist here.** It reports reading `23A-C1-NOTES.md` §6.3. My
  notes file is `23A-C1-GOVERNED-NOTES.md`; `ls .planning/23A-C1*.md` returns exactly one
  file, mine, which has no §6.3 and contains the string "inert" nowhere. It read a
  different lane's notes and attributed them to me.
- **My mechanism is not the `Procedure` row.** `ProcedureStatus::Revoked` is a secondary
  defence. My primary guard is the **loader catalog post-pass** (`loader.rs
  apply_governance`), which *drops* revoked skills from the catalog entirely rather than
  quarantining them.

### And the loader guard **does** cover the `evolved_prompts` vector

Measured, by reading the hydration block itself:

```
bootstrap.rs:1836   skill_refs   = load_catalog_with_bundled(...)   <-- apply_governance runs HERE
bootstrap.rs:2114   catalog      = SkillCatalog::from_refs(skill_refs)
bootstrap.rs:2148   candidate_names = catalog.visible().map(|r| r.name).collect()
bootstrap.rs:2152   store.seed_pairs_for(&candidate_names, "bench", 1)
bootstrap.rs:2173   store.seed_pairs_for(&candidate_names, "auto_drafter", 1)   <-- Layer 1b
bootstrap.rs:2189   sk_router.seed_from_prioritizer(&candidate_names)
```

**All three hydration layers are scoped to `candidate_names`**, and `candidate_names` is
derived from the catalog my post-pass filters. `PromptStore::best_for_skill` (which
`seed_pairs_for` drives) is `WHERE skill_name = ?1` — a **per-name lookup**, so a name that
never enters the candidate list is never queried at all.

So a revoked skill's `evolved_prompts` row is not purged — it is made **unreachable**. The
row survives; nothing ever asks for it. And `SkillRouter::choose` calls
`thompson_pick(input.candidates)`, so even a seeded arm for a non-candidate name cannot be
picked.

**This is why `wcore-skills` does not need a `wcore-evolve` dependency.** The dependency
graph does not force the guard up to `bootstrap.rs`, because the catalog sits *below* both
stores and both are scoped to it. Unreachability is achievable at the lower layer;
deletion would not be.

### The honest limit of my guard, stated rather than smoothed

My guard holds **because** every current hydration path is catalog-scoped. It is a property
of those three call sites, not a structural guarantee — a future hydration path that reads
`evolved_prompts` unscoped would bypass it. That residual is exactly what a guard sited at
`bootstrap.rs` covers, and `lane/open-highs` has landed one (+182/−0, with a mutation that
reddens both drop assertions against a green control).

**Seam, stated so neither lane assumes the other:**

| Half | Owner | Mechanism |
|---|---|---|
| Promotion / revocation / rollback feature, catalog-level drop, promotion refusal, grant withdrawal, `ProcedureStatus::Revoked` | **this lane** | revoked skills never enter the catalog, so never enter `candidate_names` |
| Defence-in-depth guard at the agent-bootstrap bridge | **`lane/open-highs`** | catches any future unscoped hydration path |

I own the first. I am relying on the second for the residual. **The hazard is closed
against `evolved_prompts` and Layer 1b by my catalog guard as measured above** — but the
structural guarantee, as opposed to the call-site property, is the other lane's.

## T+150 — the flaky baseline, noted so I do not misread my own results

`cargo test -p wcore-agent --lib` is red at my base `75babf32` — 2152 passed / 20 failed,
varying set, mostly `session journal writer lease is already held`. Owned by
`lane/flaky-root-cause`. **Not mine, and a green run there proves nothing either.** My
gates are scoped to `wcore-skills`, `wcore-memory` and `wcore-cli`.

**Nothing below this line is established yet.**
