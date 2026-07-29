# 26-SC2 lane notes — running log

Lane `26-sc2-import`, branch `lane/26-sc2-import`, base `7f5cdbc3` (`lane/grade-26`).
Criterion: **SC2 — "Selective import/export preserves provenance and quarantines
executable content."** Graded PARTIAL by `26-PHASE-VERDICT.md` today.

Append-and-commit after every measurement. Nothing here is inherited unless labelled.

---

## Minute 0–10 — inventory, before touching anything

**The brief's premise is HALF stale, and the half that is stale matters.**

The brief said a prior lane "already did substantial import work (4 files → 1,773)" and
that "that work is on a branch that may already be merged into your base." Measured:

```
$ /usr/bin/git merge-base --is-ancestor lane/26-import HEAD   → NO (rc!=0)
$ /usr/bin/git merge-base HEAD lane/26-import
861d1b1a716240165209336b1fa38d36f9445716
$ /usr/bin/git log --oneline -1 lane/grade-26
7f5cdbc3 verdict(26): grade all four criteria ...
```

So `lane/26-import` and my base `lane/grade-26` are **siblings off `861d1b1a`**, not
ancestor/descendant. The import work is REAL and is on disk on another branch; it is
**not in my tree**. `crates/wcore-cli/src/migrate/content.rs` does not exist at my HEAD.

Diff size `HEAD → lane/26-import`: 20 files, +3251/−990 (the −990 is grade-26's own
docs, which that branch does not have — the two branches each carry the other's gap).

**Consequence for this lane:** do not rebuild the writer. Merge `lane/26-import` in,
then do what a grader would not accept from the lane that wrote it — re-derive its
central claims from source and from a live run, and close what it left open.

### What `26-import` claims (its own doc, NOT yet verified by me)

| claim | status here |
|---|---|
| 4 → 1773 files written into the Wayland home | to re-measure |
| F26-GRADE-H1 fixed — `Outcome::Imported` now taken from writer return value | to re-derive from source |
| F26-GRADE-M1 fixed — scan 274 → 1666 via bounded recursion | to re-derive from source |
| quarantine inert with P0 positive control | **to re-prove independently — this is the trap** |
| personas/memory written but inert; settings/assets refused | to confirm shape |
| `t2` repaired with 3 assertions incl. old-test-misses | to re-run the mutation myself |

### What NOBODY has closed (from `26-PHASE-VERDICT.md` gap list)

- **G7 / F26-GRADE-M2** — committed hermes fixture: 540 skill dirs, **zero `SKILL.md`**,
  so at-scale conservation classifies 0 of 540. `26-import` built a *generated* corpus
  (`scripts/f26-import-corpus.sh`) rather than fixing the committed one. That leaves the
  repo's own test corpus unable to exercise classification. **Mine.**
- **peer coverage beyond hermes/openclaw** — brief names four peer trees under
  `/Users/seandonahoe/dev/resources/`: `hermes-agent, openclaw, grok-build, gemini-cli`.
  Only hermes + openclaw have importers. To measure.

### Claims I explicitly do NOT inherit

- The `1773` figure. Different corpus, different script, written by the party being
  graded.
- "quarantine inert" — the verdict's own warning is that this phase already shipped one
  self-passing known-negative. I re-prove it with a build that has quarantine **removed**
  and require my test to go RED.

---

## Boundaries held

- Not touching backup/restore/rollback (lane `26-sc3-rollback` owns SC3).
- `/Users/seandonahoe/dev/resources/**` is read-only; nothing there is executed.
- No merge to `plan/f20-unified-audit-repair`, no PR, no tag, no `wcore-contract generate`.
