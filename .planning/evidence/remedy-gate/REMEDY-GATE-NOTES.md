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
- **Inventory landed** (`d348cb40`). 1178 production sources, 39,322 string literals,
  **1504 rows**; 887 carry an extractable advertised token, **617 are instructive prose with
  no token** and are therefore not gate-checkable. Two classifier defects found and fixed
  before trusting the data: (a) case 1's TOML lives in a `const`, so a context whitelist of
  error/help/stdout dropped the single highest-value string; (b) section↔key pairing was
  line-scoped and bound `backend` to `[session]` in the live headless-keyring string.
- **Gate landed** (`9ad8678b`), compiled first try on hetzner (`BUILDRC=0`, 1m41s).
- **Run 1: 2 passed, 3 FAILED.** Every failure was a vacuity guard doing its job. `toml` 1.x's
  `FromStr` rejected all 36 candidate snippets, so the main check counted **zero** checked and
  36 "illustrative" — without the `checked >= 15` floor this ships as a green measuring
  nothing. Also: the key regex had no CLOSING backtick, so it recovered nothing from case 5's
  own pre-fix wording.
- **Run 2: 4 passed, 1 FAILED with 9 reports.** Six were instrument defects (array-of-tables
  flattened; sibling required fields missing because keys were checked one at a time; a key
  literally named `set`, harvested from the English "backend **is set to** \"plaintext\"").
  Fixed by grouping per carrying string and preferring the carrier verbatim when it is already
  well-formed TOML.
- **Three survived. One is a real, previously-unrecorded defect — a SIXTH instance of the
  class.** `wayland-core init --model X` writes `model = "X"` at the ROOT of
  `.wayland/config.toml`; the loader reads `default.model`. Root `model` is not a `ConfigFile`
  field, `ConfigFile` is `#[serde(default)]` with no `deny_unknown_fields`, so it parsed
  cleanly and was discarded. Live on hetzner: generated file captured verbatim; a real engine
  boot in that project produced 5,630 bytes of stderr with **zero** occurrences of the
  product's own "ignoring unknown or mis-sectioned config key" warning. Fully silent.
  Every other config written anywhere in the workspace already spells it `[default]`.
  Fixed at `623ae4e8` with a round-trip regression test in `init.rs`.
- **Run 3: 5 passed, 0 failed. 29 assignments checked, 5 illustrative.**
- **Mutation harness, first full run: case 5 came back STILL-GREEN** after I fixed an
  over-qualification bug. That is the most useful result of the lane: case 5 had only ever
  been caught **by accident** — its path was mis-bound to `session.credentials.backend`, and
  `session` is a real root, so admission fired for a reason unrelated to the defect. Its real
  message contains "confidential" but never "config", so the wording-based rule missed it too.
  Replaced with admission rule B: *a real schema path that has lost its leading section(s)* —
  the defect stated as a predicate.
- **Mutation harness, final: 6/6 expectations met (`MUTRC=0`).** case1 RED, case2 RED,
  case5 RED, case6 RED; case3 and case4 STILL-GREEN **as expected and as measured**, not
  merely asserted.

## Open risks I am tracking

- **Vacuity is the primary failure mode here.** A regex that extracts nothing passes silently.
  Every extraction must assert a nonzero count, and the acceptance test is red-before-green on
  five real historical defects — not a self-written fixture.
- Prose remedies ("configure an OS keyring") name no token and are not checkable. Must be
  counted and reported, not silently dropped.
