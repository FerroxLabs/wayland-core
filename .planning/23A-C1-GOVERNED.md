---
lane: 23a-c1-governed
criterion: 23A-C1
grade-23A-C1: MET-for-the-shipped-surface (was PARTIAL)
resurrection-hazard-closed: yes-for-my-half-see-seam
deferred:
  - promote_new materialisation from a procedure with no on-disk artifact (refuses instead)
  - purging a revoked skill's evolved_prompts / Procedure rows (made unreachable, not deleted)
  - promotion authority beyond "which surface invoked it" (no identity system exists)
  - wcore-skill-govern still unpackaged (superseded by the wayland-core flags, not removed)
new-finding: "HIGH: the revocation capability shipped to nobody; and rollback wrote the user's skills directory non-atomically (11/35 kills left a partial restore)"
fence-exposure: "crates/wcore-cli/src/main.rs +69/-14 additive in two contiguous blocks; crates/wcore-cli/src/lib.rs +6/-0 (one pub mod). No reorders, no renames, no reformatting."
status: complete
---

# 23A-C1 — governed skill promotion, revocation and rollback

Lane `23a-c1-governed`. Base `75babf32`. Head `597c3275`.

---

## 1. What the measurement found before any code was written

The brief carried three inherited claims and told me to re-verify. Two held, one was stale,
and the stale one changed the shape of the work.

| Inherited claim | Verdict at `75babf32` |
|---|---|
| `ProcedureStatus` has no `Revoked` variant | **TRUE** — `v2_types.rs:359`, four variants |
| No generation store exists to roll back to | **FALSE, stale** — `govern.rs` (566 lines) had generations, tombstones, an append-only journal, `revoke`, `rollback` |
| No artifact-provenance binding | **PARTIAL** — revocation-side provenance existed; there was no promotion-side record because there was no promotion |
| `--skills-promote` hidden and exits 1 | **TRUE** — `main.rs:2537`, an unconditional `bail!` ignoring its argument |

### The finding that reordered the lane — HIGH

**The revocation capability that a prior lane built shipped to nobody.**

`release.yml` packages exactly one executable. `BINARY_NAME: wayland-core`; six matrix rows
all name `wayland-core`/`wayland-core.exe`; the upload glob is `artifacts/wayland-core-*`.
`wcore-skill-govern` is a **dev-only auto-discovered bin of a library crate**, named in no
workflow, no packaging script and no manifest. Its only non-source reference in the tree is
its own test.

Measured with a live control in the same file and the same invocation:

```
/usr/bin/grep -c "wayland-core"  .github/workflows/release.yml   -> 31   [known-positive]
/usr/bin/grep -c "skill-govern"  .github/workflows/release.yml   ->  0   [the absence]
```

So `RC-READINESS` graded 23A-C1 as lacking revocation for the **right reason but the wrong
cause**: the code existed, the surface did not. The prior lane's own module docs record
that it wanted the flags on `wayland-core` and believed the shared-file fence forbade it.
**It does not** — LANE-BRIEF §6 permits additive edits in one contiguous block, which is
what this lane made.

---

## 2. What was built

**`crates/wcore-skills/src/promote.rs`** (new). Promotion is defined as *lifting the
loader's generated-draft quarantine for one artifact* — not copying files, not editing
skills. Because the only thing it changes is reachability, the whole transaction fits in one
grant file, and "what was promoted, from where, on whose authority" has a literal answer on
disk.

The grant binds a **sha256 content digest**. That is deliberately the opposite key choice
from revocation, because the two have opposite requirements:

- **Revocation keys on name/signature, never content.** The drafter's trigger is designed to
  recur, so any regeneration yields different bytes; a content-keyed revocation would be
  defeated by the exact process it exists to stop. It must be **loose**.
- **Promotion keys on content, never name alone.** A name-keyed grant survives *mutation* —
  promote a reviewed skill, let anything rewrite `SKILL.md`, and unreviewed content inherits
  model-facing status. It must be **strict**.

**`loader.rs apply_governance`** — one choke point after dedup, covering both
`load_all_skills` and `load_catalog`. Revoked skills are **dropped from the catalog
entirely, not quarantined**: quarantine only hides a skill from the model, leaving it
loaded, resolvable by name and executable through the user-invocable path. Revocation is
checked first and unconditionally, so a stale grant can never re-expose a revoked artifact.
Cost when nothing is governed is one directory read.

**`ProcedureStatus::Revoked`** — terminal, reachable from every state. Revocation must be
reachable wherever a procedure sits because it encodes user intent; it is terminal because
every exit is a resurrection. The supported way back is `rollback`, a governed operation
with its own journal entry, not a status edit.

**`revoke()` now withdraws promotion grants.** A grant outliving its revocation would mean
the artifact returns **model-facing** rather than quarantined — strictly worse than before
the user ever revoked it.

**`crates/wcore-cli/src/skill_govern.rs` + four flags on the shipped binary**:
`--skills-promote` (un-hidden), `--skills-revoke`, `--skills-rollback`, `--skills-govern`.

### A second HIGH, found while designing the kill harness

**`rollback` wrote the user's skills directory file by file.** It restored the retained
generation with a plain recursive copy, so a kill part-way through left a *partially
restored skill directory* — present, loadable, missing content. Same class as the migration
data loss already on this program's record, on a shipped verb. Now staged into a directory
outside the skills tree, fsynced, then `rename(2)`d into place.

Staging sits **outside** the skills tree rather than under a hidden name inside it because
`collect_skill_md` does not skip dot-directories: a half-built staging directory holding a
`SKILL.md` inside the skills root would be discovered and loaded as a skill.

---

## 3. The three live proofs

All on `hetzner-dsm`, driving the real `wayland-core` binary. **34 assertions, 0 failures.**
Isolation: `WAYLAND_HOME` is a fresh `mktemp -d` per leg — what both
`paths::wayland_home_skills_dirs` and `govern::governance_root` resolve against. A final leg
hashes the real global skills directory before and after every leg and asserts it unchanged.

### The instrument had to be repaired first, and the repair is the reason the result counts

v1 of the harness measured catalog membership by grepping `--skills-audit` for the skill
**name**. `--skills-audit` does not print names. So:

- v1's known-positive **failed** — the control doing its job, declaring the assertion
  beneath it meaningless rather than letting it pass;
- v1's `REVOKED SKILL IS ABSENT` **passed for the wrong reason**: a grep for a string that is
  never printed is absent unconditionally;
- v1's rollback leg produced a genuine **false pass**, `grep auto-gen` matching
  findings-dependent output and also matching `auto-gen2`.

v2 uses `Total skills: N` from the real `load_catalog`, and carries the three-assertion
self-test: count rises on add, falls on remove, **and the old matcher wrongly reports ABSENT
for a present skill**. That third assertion is the only one that proves the repair does
anything.

| Leg | Effect on disk | Effect on behaviour | One-variable control that reddens |
|---|---|---|---|
| **Promote** | exactly 1 grant, carrying `sha256:…`, `authority`, `target_dir` | `status=installed` → `status=promoted` | untouched sibling stays `installed`; **editing the bytes** flips to `quarantined-digest-mismatch` |
| **Revoke** | directory gone; retained generation present | real catalog `2 → 1`, and **does not recover** when the files are rewritten | bystander count `0 → 1` in the same measurement; promoting the bystander succeeds while promoting the victim is refused |
| **Rollback** | restored **byte-identical** (md5 matches the pre-revocation generation) | catalog `0 → 1`; `REVOKED (0)` | rollback of an unknown id exits nonzero |

Promotion of a revoked skill is refused, the refusal names `revoked`, and it is journalled.

---

## 4. The mid-promotion kill distribution

35 kills per verb, 600-file payload, delay sampled uniformly across a **self-calibrated**
window.

### The first attempt was vacuous and its own guard said so

v1 sampled `[0.5ms, 30ms]`. The debug binary is 331MB and a bare `--version` takes **26ms**,
so 35/35 promote trials landed in `NO-GRANT` with `GRANTED=0` — every kill hit startup and
no trial reached the write. **Without the vacuity guard this would have reported 0
destructive states across 70 kills and been believed.**

v2 measures each verb's real duration first, then samples across `[startup_floor,
1.05 × duration]`. Calibration on the run reported: floor `.0261s`, promote `.1697s`,
rollback `.0991s`.

| Verb | Distribution over 35 kills | Destructive |
|---|---|---|
| `--skills-rollback` | COMPLETE **11**, NOT-STARTED **24**, **PARTIAL 0** | 0 |
| `--skills-promote` | GRANTED **4**, NO-GRANT **31**, TORN-GRANT **0**, SKILLS-DIR-MUTATED **0** | 0 |

Both verbs saw completions *and* interruptions, so the window straddles the work. Every
NOT-STARTED trial was **retried and asserted to restore correctly** — 0 retry failures.

### And `PARTIAL = 0` is not a free zero — the control reddens

The atomic restore was reverted on hetzner, the binary rebuilt, and the identical harness
re-run:

```
ROLLBACK over 35 kills:  COMPLETE=12  NOT-STARTED=12  PARTIAL=11  retry-failures=0
  !! PARTIAL trial 1  (delay 0.0771s): 436 files, expected 600
  !! PARTIAL trial 3  (delay 0.0723s): 327 files, expected 600
  !! PARTIAL trial 34 (delay 0.0653s): 165 files, expected 600   [+8 more]
```

**11/35 with the old code, 0/35 with the fix**, same harness, same payload, same window.
The prior migration finding on this program was 5/35; this is the same defect class at
higher incidence, and it was on a shipped verb.

`--skills-promote` never mutated the user's skills directory in any of 35 kills, which is the
structural claim: the shipped promotion path writes only the grant.

---

## 5. The resurrection hazard — and an honest seam

`MILESTONE-RC.md` records: *"auto-draft router hydration can never fire today, but becomes a
live resurrection hazard the moment promotion lifts quarantine."* This lane lifts quarantine.

`lane/open-highs` relayed that the vector is the `evolved_prompts` row rather than the
`Procedure` row, and that my mechanism was the (inert) `Procedure` row. **Verified rather
than accepted, and the claim splits.**

**The substantive half is true and I had missed it.** `evolved_prompts` hydration is real:
`bootstrap.rs:2122-2189` seeds the router from it in two layers, `bench` and the
`auto_drafter` Layer 1b read-back.

**The half about my mechanism is false and misattributed.** It cites `23A-C1-NOTES.md` §6.3;
`ls .planning/23A-C1*.md` returns exactly one file, `23A-C1-GOVERNED-NOTES.md`, which has no
§6.3 and contains "inert" nowhere. `ProcedureStatus::Revoked` is my *secondary* defence. My
primary guard is the loader catalog post-pass.

**And the catalog guard does cover the `evolved_prompts` vector:**

```
bootstrap.rs:1836  skill_refs      = load_catalog_with_bundled(...)   <- apply_governance HERE
bootstrap.rs:2114  catalog         = SkillCatalog::from_refs(skill_refs)
bootstrap.rs:2148  candidate_names = catalog.visible()
bootstrap.rs:2152  seed_pairs_for(&candidate_names, "bench", 1)
bootstrap.rs:2173  seed_pairs_for(&candidate_names, "auto_drafter", 1)    <- Layer 1b
bootstrap.rs:2189  seed_from_prioritizer(&candidate_names)
```

All three layers are scoped to `candidate_names`, and `PromptStore::best_for_skill` is a
per-name lookup (`WHERE skill_name = ?1`). A revoked skill's row is not purged — it is made
**unreachable**; nothing ever asks for it. `SkillRouter::choose` then calls
`thompson_pick(input.candidates)`, so even a seeded arm for a non-candidate is inert.

**This is why `wcore-skills` needs no `wcore-evolve` dependency.** The catalog sits below
both stores and both are scoped to it.

**The honest limit.** My guard holds *because* every current hydration path is
catalog-scoped. That is a property of three call sites, not a structural guarantee: a future
unscoped path would bypass it. That residual is what a guard at the agent-bootstrap bridge
covers, and `lane/open-highs` has landed one.

| Half | Owner |
|---|---|
| Promotion/revocation/rollback feature; catalog-level drop; promotion refusal; grant withdrawal; `ProcedureStatus::Revoked` | **this lane** |
| Defence-in-depth guard at the bootstrap bridge, catching future unscoped hydration | **`lane/open-highs`** |

I own the first and rely on the second for the residual. Neither half alone is the whole
closure, and I am not claiming the other lane's.

---

## 6. Restoring `--skills-promote`, and the class it belongs to

The flag is advertised again — **only after** the capability was live-proven. The guard was
**inverted, not deleted**: `skills_promote_not_advertised.rs` became
`skills_promote_advertised_and_works.rs`, moving from `hidden ⇒ fails loudly` to
`advertised ⇒ succeeds on a real artifact`. The new form is strictly harder: the old one
could be satisfied by a flag that did nothing.

**The false-advertising class in `wcore-cli/src` is now empty.** All six `DEAD_END_MARKERS`
return 0 against a live 110-hit `bail!` control. That tripped `remedy_advertisements.rs`'s
own anti-vacuity assert, whose message invited deleting the scanner. **Deleting a class
scanner because the class is briefly empty is how the class returns unnoticed**, so the
detector was extracted to `scan_dead_ends()` and given a three-assertion self-test: fires on
a synthetic dead end, silent on a conditional refusal, and **the naive matcher flags the
conditional case while the real detector does not**. Anti-vacuity no longer requires a real
defect to exist in the tree.

---

## 7. A defect I committed, recorded rather than amended away

Commit `93beff32` claimed the implementation and **contained only a file rename**. Its
`git add` carried a stale pathspec (a file renamed moments earlier); `git add` aborts the
*entire* add on an unmatched pathspec; and I had piped its stderr to `/dev/null`. The commit
message described work still sitting in the working tree.

It was caught by the hetzner build failing with *"cannot find `promote` in the crate root"*.
This is the suppressed-exit-status class the brief warns about, committed by the lane hunting
it. Recorded in `c3f5b4fc` rather than quietly amended, and every later commit verified with
`git diff --cached --stat` before committing.

---

## 8. Gates

| Gate | Result |
|---|---|
| `cargo clippy -p wcore-skills -p wcore-memory -p wcore-cli --all-targets -- -D warnings` | rc=0 |
| `cargo fmt --all -- --check` | rc=0 |
| `wcore-skills --test govern_catalog_enforcement` | **5 passed**, 0 failed, 0 ignored, 0 filtered out |
| `wcore-skills --test govern_revoke_rollback` | **15 passed**, 0 ignored, 0 filtered out |
| `wcore-skills --test govern_cli_drive` | **6 passed**, 0 ignored, 0 filtered out |
| `wcore-memory --lib` | **350 passed**, 0 ignored, 0 filtered out |
| `wcore-cli --test skills_promote_advertised_and_works` | **5 passed**, 0 ignored, 0 filtered out |
| `wcore-cli --test skills_lifecycle_cmd` | **4 passed**, 0 ignored, 0 filtered out |
| `wcore-cli --test remedy_advertisements` | **8 passed**, 0 ignored, 0 filtered out |
| Live proof, real binary | **34 assertions, 0 failures** |
| Kill distribution | 0 destructive / 70 kills; control reddens at **11/35** |

A filtered run caught its own defect: `cargo test -p wcore-memory --lib -- procedure_status`
reported `1 passed` because neither new test's name contains that string — flavour (c) of the
self-passing class. Re-run with `-- revoked`: **2 passed**.

`cargo test -p wcore-agent --lib` is red at base (2152/20, `session journal writer lease`),
owned by `lane/flaky-root-cause`. Not mine; not treated as evidence either way.

---

## 9. Grade

**`23A-C1`: MET for the shipped surface.** Governed promotion, revocation and rollback exist,
are reachable on the binary customers install, are bound to a checkable provenance record,
refuse revoked artifacts, and survive interruption. Every clause was live-proven on real
hardware with a control that reddens.

**Not claimed:** that every conceivable resurrection route is structurally closed. Mine is
closed by catalog unreachability, verified against `evolved_prompts` and Layer 1b; the
structural guarantee against future unscoped hydration is `lane/open-highs`' guard.

## 10. Deferred, deliberately

- **`promote_new` materialisation.** Promotion binds a reviewed procedure to an artifact on
  disk; it does not invent one. A procedure with no installed artifact is refused with that
  reason. The staged-and-renamed install exists and is now load-bearing for `rollback`.
- **Purging `evolved_prompts` / `Procedure` rows on revoke.** Unreachability is achieved at
  the catalog; deletion would require `wcore-skills → wcore-evolve`, which the graph forbids.
- **Promotion authority beyond the invoking surface.** No identity system exists; the grant
  records what is true (an explicit command) rather than inventing a principal.
- **Unpackaging `wcore-skill-govern`.** Retained as the harness its own tests drive, now a
  peer rather than the only surface.

## 11. Fence exposure vs `75babf32`

- `crates/wcore-cli/src/main.rs` — **+69 / −14**, additive, two contiguous blocks (flag
  declarations after `skills_archive`; dispatch arms after the `skills_archive` arm), plus
  replacing the `run_skills_promote` dead end with a delegation. No reorders, no renames, no
  reformatting of surrounding code.
- `crates/wcore-cli/src/lib.rs` — **+6 / −0**, one `pub mod skill_govern;` at the end.

No other lane's files touched. No PR, no merge, no tag, no issue closed, no
`wcore-contract generate`, no `.github/workflows/*` edit.

## 12. Seam requests

None. No wire/protocol change was needed: governance state is filesystem-local and no
`ProtocolEvent` was added. `ProcedureStatus::Revoked` is internal to `wcore-memory` and is
not on the Desktop wire contract.
