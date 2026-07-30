---
lane: port-import
phase: 26-migration-export-backup-restore
base: b2ddf113
verdict: brief refuted; criteria independently re-verified as MET
findings: [F-PI-01 MEDIUM, F-PI-02 LOW]
---

# Lane `port-import` — summary

## Verdict

**I did not build 26-02 or 26-04, because they already exist and work.** The
brief's central premise was false at HEAD, and the honest deliverable was to
measure that, then independently verify the work rather than duplicate it.

## What the brief claimed, and what is true

| Brief / ledger claim | Measured at `b2ddf113` | Verdict |
|---|---|---|
| "plans 26-02 and 26-04 were never started" | both have PLAN + SUMMARY; 26-04 also a CERTIFICATION | **FALSE** |
| "the F26-02 quarantine contract is still unbuilt" | `migrate/quarantine.rs`, 1132 lines | **FALSE** |
| "nothing has yet imported anything" | `apply_plan()` at `mod.rs:871`, behind `ApplyGuard` | **FALSE** |
| "the migration security boundary has never been crossed" | 19 hostile corpora cross it in CI | **FALSE** |
| peer trees read-only; prove non-mutation by digest | held — see below | held |
| both peers migrate from each other; Core has no reciprocal path | now false for **apply**, not just discovery | **stale** |

26-02 landed `ec9794b1`…`a170ee24` and 26-04 `1c13e9a2`…`0a75efd9`, both
2026-07-28. The `PORT-*` cell was authored the same day citing only
`Phase 26 (26-01, 26-03)` and was never refreshed; the ledger's last touch
(`69ae4224`) was a Phase 22 re-grade that did not revisit the row. I have
corrected the row and the Delta table entry.

## What apply now does

`wayland-core migrate {hermes,openclaw,grok,gemini} --home <src>` runs
detect → plan → confirm → apply. `--dry-run` and `--json` stop at the preview;
`--select`/`--exclude` address items by the identity the dry run published and
**refuse** an unpublished identity rather than ignoring it. Apply writes through
an atomic partial writer under a `rollback::ApplyGuard`, and prints a
conservation line: `discovered = imported + quarantined + excluded`.

## The quarantine contract, and both directions of its proof

**Contract.** Imported executable content is inert **by placement**, not by a
flag: it lands under `<config>/migrate-quarantine/`, which none of the four
agent-facing skill roots resolves to. Promotion consults only the caller's
identities and the store's own index — never a field, marker or manifest key
carried by the payload. Classification delegates to
`wcore_skills::shell::contains_shell_commands`, the same predicate the executor
enforces, so there is no second pattern list to drift.

**Live, on the real binary** (`LIVE-ACCOUNTING-BOTH-DIRECTIONS.txt`):

- *catch* — a payload declaring `trusted: true`, `auto_promote: true`,
  `wayland_quarantine: exempt`, `kind: data`: `discovered=2 imported=1
  quarantined=1`. Five self-declared trust claims do not move the classifier.
- *pass-through* — a persona whose prose contains shell-directive syntax:
  `discovered=2 imported=2 quarantined=0`. It is genuinely imported, not
  dropped to reach zero.

**Mutation-tested, because passing is not the same as load-bearing:**

| Mutation | Expected | Result |
|---|---|---|
| M1 classifier blinded → always `Data` | catch reddens | **RED** — 10 failures incl. `t19` live-negative, `t20` live-positive |
| M2 classifier over-broad → always `Executable` | pass-through reddens | **RED** in `migrate_quarantine` — 5 failures incl. `t2`, `t5`, `t25` |
| M3 `classify_data_kind` → `Executable` | hostile pass-through reddens | **GREEN, 23 passed — no effect** |

## Corpora run, with counts

```
migrate_hermes          7 passed;  0 failed; 0 ignored; 0 filtered out
migrate_quarantine     34 passed;  0 failed; 0 ignored; 0 filtered out
migrate_typed_dryrun   14 passed;  0 failed; 0 ignored; 0 filtered out
portability_hostile    23 passed;  0 failed; 0 ignored; 0 filtered out
                       78 total, TESTRC=0
```

**19 declared hostile corpora, 19 executed, 0 unrun, 0 skipped** — counted from
19 `HOSTILE-OBSERVED` lines, one per case, not inferred from exit status. Classes
covered: conflict (exact / casefold / normal-form), symlink escape (absolute /
traversal / directory), secret leakage (memory note / persona / skill body /
env), classification (both directions), malformed (truncated / wrong-type /
deep-nest), bounds (oversized member / item count), Windows naming (reserved
device / trailing dot).

## How I proved non-mutation

`nonmut.py` digests each corpus tree before and after driving the real binary,
covering relative path, mode, symlink target and bytes. **It self-tests three
assertions before reporting:** stable across two reads, red on a content change,
and red on a **mode-only** change — the last being what a content-only matcher
would miss. All green, then:

```
CORPORA-RUN=19 of 19  UNRUN=0  SOURCE-MUTATED=0 []
```

I did **not** read, execute or digest Sean's live peer homes or the read-only
reference trees under `/Users/seandonahoe/dev/resources/`. This lane never opened
them; every corpus is synthetic, generated by
`scripts/portability-hostile-gen.py`. A digest taken now with no "before" would
not be a proof, so I claim nothing about them beyond having never touched them.

## Findings (both BACKLOG, neither blocking)

**F-PI-01 (MEDIUM) — the hostile suite's pass-through case is a permanently-green
gate.** `hostile_data_that_merely_looks_executable_is_not_contained` sits under a
header reading `CLASSIFICATION, IN BOTH DIRECTIONS`, but **no mutation to any
classification predicate reddens it** (M2 green, M3 green). Its corpus is a
persona, and the persona path is never content-classified, so it asserts a
property true by construction. It fails safe and the direction *is* genuinely
covered — by `t2` in `migrate_quarantine`, which M2 does redden. Fix: give the
hostile pass-through case a **skill** body with no directive, so the hostile
set's own classification section is live both ways.

**F-PI-02 (LOW) — `classify_data_kind` is dead code whose comment claims
otherwise.** Its doc comment says it exists "so the breadth of the contract is
stated in code and can be measured". Zero production call sites; the only
reference is a unit test asserting a constant function returns its constant.
Known-positive in the same sweep: `classify_skill_body` returns 5 production
hits. Reported, not deleted, per AGENTS.md §3.

## Instrument defect in my own harness, repaired here

My first mutation probe ran both test binaries in one `cargo test`. Under M1,
cargo fail-fasted after `migrate_quarantine` failed, so
`portability_hostile_corpus` **never ran** — and the probe printed
`hostile catch-direction: 0 failed`, which reads as "unaffected" but means "never
executed". That is §6a-i, inside the harness hunting for exactly that class.

Repaired in this lane rather than written up and left (§6b-ii): one invocation
per binary, `--no-fail-fast`, and the probe now reads back `running N tests` and
**refuses to report** if the binary did not run. Third assertion that the repair
does something: the old matcher reported `0 failed` for M1 where the repaired one
reports `1 FAILED`.

Also: `/usr/bin/ls` does not exist on macOS (`/bin/ls`) — caught on the first
command because output was redirected to a file and read back rather than
trusted from stdout.

## What I did NOT do

- Did not build 26-02 or 26-04 — already built; rebuilding would have been
  manufacturing work to match a falsified brief.
- Did not fix F-PI-01 or F-PI-02 (MEDIUM/LOW → BACKLOG under §5).
- Did not run the Windows leg. 26-04 claims a byte-identical 3514-byte
  cross-platform report; I did not re-verify that claim on `seandesktop`.
- Did not touch peer reference trees or Sean's live peer homes.
- No `cargo` on the Mac; all builds and tests on `hetzner-dsm`,
  worktree `hz/port-import` at `b6d7ee58`, `CARGO_BUILD_JOBS=10`.
