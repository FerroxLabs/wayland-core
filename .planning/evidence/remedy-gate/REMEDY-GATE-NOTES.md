# NOTES — lane/remedy-gate (running log, re-committed after every measurement)

**Base** `plan/f20-unified-audit-repair` @ `2a306ac8` (captured once; do NOT diff against the
branch name — it moves).
**Worktree** `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-remedy-gate`

## Goal

A remediation string that names something the product cannot honour should fail CI, not ship.

## The defect class (five recorded instances, all found by a human driving the binary)

| # | instance | what was advertised | why it was dead |
|---|---|---|---|
| 1 | `27-C2(a)` | browser remediation hint | impossible loop |
| 2 | `23A-C1` | `--skills-promote` in `--help` | always exited 1 |
| 3 | `24-C2` | `--trigger webhook:` / `poll:` | accepted at add, never fired |
| 4 | `ollama:` hint | "select a model prefixed `ollama:`" | config resolution returned MissingApiKey before the model string was read |
| 5 | headless keyring | `credentials.backend = "encrypted-file"` + "supply its unlock passphrase" | wrong section, unparseable value, struct variant, and a mechanism named in 0 docs/0 help/0 errors |

## The countermeasure in miniature (already landed)

`crates/wcore-agent/src/recovery_confidential.rs:508` — `every_backend_value_the_messages_
advertise_actually_parses` extracts `backend = "<v>"` from the real `Display` output of the
error enum and re-parses `<v>` through the real `CredentialsStorageConfig`. Plus
`:557` asserts the advertised `[session] enabled = false` parses through the real `SessionConfig`.
Both carry a vacuity guard (`checked > 0`). This lane generalises that shape.

## Plan

1. **Inventory first, committed as data** before any check is written
   (`.planning/REMEDY-INVENTORY.tsv`). Sweep operator-facing strings; extract what each
   *advertises* (config key, config value, env var, CLI flag, subcommand, path).
2. Build the gate over the highest-value checkable slice.
3. **Acceptance = run the gate against the pre-fix state of all five historical cases.**
   If it does not go red on them it does not work.
4. Report honest coverage, including what is unparseable prose.

## Log

- **T+0** worktree created at `2a306ac8`, verified `git rev-parse --show-toplevel`.
- **T+10** read `HEADLESS-KEYRING-FINDING.md` and the two miniature gates. Confirmed the shape
  to generalise: *extract advertised token from the real runtime string → re-parse/resolve
  through the real consumer → vacuity-guard the extraction*.
- Next: write the sweeper, produce the inventory, commit it.

## Open risks I am tracking

- **Vacuity is the primary failure mode here.** A regex that extracts nothing passes silently.
  Every extraction must assert a nonzero count, and the acceptance test is red-before-green on
  five real historical defects — not a self-written fixture.
- Prose remedies ("configure an OS keyring") name no token and are not checkable. Must be
  counted and reported, not silently dropped.
