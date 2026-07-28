# 30-01 — Surface inventory, derived from the shipped binary's own command tree

**Not read off a planning document.** Every row below came out of a release
`wayland-core` binary by running it.

| | |
|---|---|
| Source commit | `4f749251060a0b22546dd6341c82a5e049083237` (lane `lane/30-01`) |
| Review base | `eab69cdbc244cfe90b0a623a9fb15c80da249d24` |
| Binary | `target/release/wayland-core`, sha256 `e73453a5ba0cdb23ace670106de036d24e28fc6ed6f38202a842c87a86c5aaae` |
| Built | `cargo build --release --locked -p wcore-cli -p wcore-eval-scenarios`, rc=0, on `hetzner-dsm` |
| Walker | `wayland-scorecard surfaces --binary <that binary>` |
| Raw transcript | `evidence/30-01/help-tree.txt` (carries the source SHA and the binary digest) |
| Table | `evidence/30-01/surfaces.tsv` — 148 surfaces + header |

## The regeneration claim, stated precisely

A determinism claim is only as good as the thing on the other side of the diff, so:

- **What was regenerated:** the surface table, by re-running the *same* shipped
  `wayland-scorecard surfaces` binary against the *same* `wayland-core` binary
  (sha256 asserted `e73453a5…` before and after), on the *same* host, at the same
  commit.
- **What it was compared against:** the **committed bytes** of
  `.planning/phases/30-continuous-scorecard-frontier-review/evidence/30-01/surfaces.tsv`
  as they exist in this git tree — not against an in-memory value and not against a
  second copy produced in the same process.
- **Result:** `diff -u` → `REGENERATION_DIFF=IDENTICAL`, 149 lines each side.
  Capture: `evidence/30-01/regeneration-gate.txt`.

So a hand edit to the committed inventory fails this diff. What this does **not**
prove is cross-host or cross-build determinism: both walks ran on one Linux host
against one binary. Determinism across platforms is unmeasured and is not claimed.

## Headline numbers

| Measure | Value | Note |
|---|---|---|
| Shipped surfaces (all depths) | **148** | walk bounded at depth 3 |
| Shipped top-level commands | **28** | the plan was calibrated at **21** — the binary grew 7 |
| `docs/` command tokens extracted | 12 | a **floor**, see limitations |
| BINARY_AND_DOCS | 42 | |
| BINARY_ONLY (shipped, undocumented) | 91 | |
| DOCS_ONLY | 4 (1 real, 3 extraction noise) | |
| NO_FAMILY (no CTRL-01 owner) | **15 rows / 6 top-level commands** | |

Classification: `evidence/30-01/surface-diff.tsv`, one row per shipped path plus one
per documented-but-absent reference, shape
`SURF-NNN::<bucket>::path=<path>::family=<family or NONE>`.

The 28 top-level commands: `acp agent auth backend backup channel cron crucible
fetch forge gateway goal image index init mcp-serve migrate models node plugin
profile project-context sandbox self-update session setup swarm workflow`.

## Finding — six shipped commands are owned by no coverage family (MEDIUM)

`init`, `mcp-serve`, `models`, `profile`, `project-context`, `setup` (15 surface rows
including subcommands) map to none of CTRL-01's ten families by declared scope.

A shipped surface owned by no family has **no security authority owner, no recorded
maturity, no evidence IDs and no peer baseline**. It is unreviewed surface. Three of
the six (`setup`, `init`, `profile`) are first-run and credential-adjacent paths,
which is where an unowned surface matters most.

This is the finding that produced a real change in the type system. `MaturityV1` has
no member meaning "nobody has graded this", and it must not grow one — `ABSENT`
would assert the capability does not exist, which is false and worse than silence.
So the unmeasured case was lifted **out** of the enum into `MaturityTruthV1`: the
closed eight-state enum still refuses any token the ledger never declared, and
"not yet graded" stays sayable without a plausible guess. Proved by
`a_maturity_state_the_ledger_never_declared_fails_to_deserialize`, which asserts
`{"state":"measured","value":"UNPROVEN"}` is still refused.

## Finding — the walk under-reports the surface: clap aliases are invisible (MEDIUM)

`docs/workflows.md` documents `wayland-core forgeflows list`. The walk does not list
`forgeflows`, so the diff first classified it DOCS_ONLY. **Run live, it works:**

```
$ ./target/release/wayland-core forgeflows list   → "no saved ForgeFlows in /root/.wayland/workflows", rc=0
$ ./target/release/wayland-core workflow list     → identical output, rc=0
```

`forgeflows` is a hidden clap alias for `workflow`. The docs are correct; **my
walker is incomplete.** `--help` renders canonical names only, so the inventory
measures the binary's *advertised* command tree, not its full *accepted* command
surface. A hidden alias is a shipped, reachable surface with no inventory row and
therefore no security owner.

This is a real ceiling on the strongest artifact in this plan and it is recorded
rather than papered over. Closing it needs alias extraction from the clap
definitions, which is a source-side derivation and would weaken the
"truth-from-the-artifact" property — a trade 30-02+ should make deliberately, not
one I should make silently here.

The other three DOCS_ONLY hits are extraction noise, verified individually:
`npm view @ferroxlabs/wayland-core version` and
`cargo install --git …/wayland-core wcore-cli` — neither is a wayland-core
subcommand. Reported as noise, not as findings.

## The seven-truth table

`evidence/30-01/surface-truths.tsv`, 148 rows, columns: id, command_path,
versioned_activation, operator_completeness, maturity, security_authority_owner,
evidence, peer_delta, last_refreshed_phase.

Driven through the **real** `SurfaceRowV1` verifier by the named test
`every_surface_row_in_the_committed_inventory_deserializes_and_verifies`, green on
Hetzner. A row that would not survive the type cannot sit in the document.

**Where a truth was not measured it reads `UNPROVEN`. No cell carries a guess.**

| Truth | State | Why |
|---|---|---|
| versioned activation | `0.12.25` — measured | the version that emitted this command tree |
| operator completeness | **UNPROVEN** on all 148 | a command-tree walk cannot observe an operator journey; a three-platform journey would measure it |
| maturity | inherited from the owning family's CTRL-01 row; **UNPROVEN** on the 15 unowned rows | see the finding above |
| security authority owner | family owner (`core`, or `shared` for NATIVE-\*/SUPPLY-\*); **UNPROVEN** on the 15 unowned rows | |
| evidence | the owning family's coverage ID; **UNPROVEN** where unowned | |
| **peer delta** | **UNPROVEN on every one of the 148 rows** | **no comparative trial has run.** 30-02 owns it. A number here would forge the very figure this phase exists to earn |
| last refreshed phase | the owning family's CTRL-01 refresh phase | |

Note the maturity column inherits values from a ledger this plan's own review found
**stale and understating** (PORT-\*, REACH-\* at HIGH). Those inherited values are
therefore a floor, not a verdict, and 30-04 should re-derive them after the row
owners refresh.

## Limitations, stated as a floor

- **`docs/` extraction is a floor.** The pattern is the product name followed by a
  subcommand token — the invocation form the documentation actually uses. Twelve
  tokens is low against 28 shipped commands, so the 91 BINARY_ONLY rows are an
  **upper** bound on undocumented surface, not a measured count. A documented form
  the pattern misses is under-counted, never over-counted.
- **The family mapping is by declared scope and is a judgement.** Where a command
  plausibly belongs to two families it is assigned to the one whose security
  authority owner is accountable, never duplicated. The mapping is in
  `build-surface-diff.py` and is auditable in one screen.
- **One host, one platform.** Everything here is Linux. macOS and Windows command
  trees are unmeasured and could differ — Phase 24 already measured a case where the
  macOS binary provably did not carry code the Linux one did.
