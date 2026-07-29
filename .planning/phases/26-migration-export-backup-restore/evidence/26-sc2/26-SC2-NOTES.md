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

---

## Measurement 1 — the provenance half had a hole nobody had named

Re-derived from source at my merged HEAD, not inherited.

`ProvenanceDocument` recorded `source_tool`, `source_version`, `source_path`,
`digest`, `imported_at` — **and no destination**. `QuarantineEntry` carried
`stored_path` beside its `Provenance`, so a CONTAINED item was traceable both
ways; an IMPORTED one was not. After a real import the live skills root holds
hundreds of directories under `sanitize_component`-ed, sometimes
digest-disambiguated names, and there was **no read-back surface at all** —
`migrate quarantined` exists, nothing answered for imported content.

So the brief's first half — *"after import, can you still tell where each
artifact came from"* — was **NO for everything that landed live or staged**,
which after 26-import is 1767 of the 1773 files.

**Closed** (commit `0cbfc730`, tests `be9686dd`..): `written_path` +
`deduplicated_with` on `Provenance`, set from the write that just happened;
`ProvenanceDocument::resolve_path` matching by path COMPONENT (so
`skills/notes` does not cover `skills/notes-2`, which a real import writes right
beside it); `migrate imported [--path P] [--json]` reading live, staged and
contained content through one lookup.

Gates, unproxied, on `hetzner-dsm`:

```
BEFORE my change: test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
AFTER  (t23+t24): test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`t23`'s fourth assertion is the one that licenses the other three: the same
query, run against the same document with destinations stripped — byte-for-byte
the record shape that shipped before this change — returns **empty**.

## Measurement 2 — the quarantine known-negative, and my own instrument was broken

`scripts/f26-quarantine-known-negative.sh` rips `classify_skill_body` out (every
skill classifies `Data`, i.e. containment removed) and requires the security
tests to go red.

```
M0 baseline   : test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
M1 APPLIED    : yes (8 changed lines)
M2 COMPILED   : yes
M3            : test result: FAILED. 23 passed; 10 failed; 0 ignored; 0 measured; 0 filtered out
M3 REQUIRED-RED t5_quarantined_content_is_absent_from_what_the_agent_would_load: FAILED
M3 REQUIRED-RED t19_live_negative_leg_quarantined_payload_does_not_execute:      FAILED
M4 RESTORED   : yes (byte-identical)
M5            : test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
KNOWN-NEGATIVE: PASS      REAL-RC=0
```

**10 of 33 tests go red when containment is removed**, including both security
assertions. M5 is what makes the red attributable — the tree returns to green.

### My M2 check could never have passed, and I repaired it rather than noting it

First run printed `M2 COMPILED: no` against a mutant that had **plainly compiled
and run 33 tests**. Cause: `grep -qE '^error'` over the cargo log, and
`cargo test` prints `error: test failed, to rerun pass …` whenever **any** test
fails — the exact condition this script exists to produce. The check was
**structurally incapable of passing on a successful run.** Twelfth instance of
the class in this programme, and §6b-ii says repair it in the same lane.

Repaired to read a positive signal (`running N tests`, only printed after a
successful build) with cargo's post-run summary excluded by name. Three-assertion
self-test, `--self-test`, all three PASS — the third being that the OLD matcher
gets the known-positive wrong, without which the self-test would pass on the
broken matcher too.

I also caught the pipe-steals-exit-status trap on myself in the same run:
`… | tail -40; echo rc=$?` reported `SCRIPT-RC=0` for a script that exited 1.
Every rc above is read from an unpiped invocation.

---

## Boundaries held

- Not touching backup/restore/rollback (lane `26-sc3-rollback` owns SC3).
- `/Users/seandonahoe/dev/resources/**` is read-only; nothing there is executed.
- No merge to `plan/f20-unified-audit-repair`, no PR, no tag, no `wcore-contract generate`.
