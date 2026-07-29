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

**Nothing below this line is established yet.**
