---
phase: 26-migration-export-backup-restore
plan: "02"
status: complete
termination_state: 1 (Complete)
requirements: [F26-02]
lane_branch: lane/26-02
---

# Phase 26 Plan 02: Selective Import, Quarantine and Provenance — Summary

Imported executable content is now **inert on disk by placement**, promotable only
by an explicit operator action the content itself cannot reach — proven live on
Linux by a payload that demonstrably CANNOT run while quarantined and
demonstrably CAN once promoted.

**Termination state: 1 (Complete).** F26-02 is claimed. Three findings are
recorded below rather than glossed, none of them CRITICAL or HIGH.

## Verdict on the plan's own success criteria

| Criterion | Outcome |
|---|---|
| Classified using the detector the executor actually enforces (skills, peer MCP launch commands, hook commands) | **MET** — `quarantine.rs` calls `wcore_skills::shell::contains_shell_commands`; no second pattern list |
| Quarantined content inert by PLACEMENT, proven absent from what the agent would really load | **MET** — store root is outside all four skill roots; the REAL loader lists it only after promotion |
| Nothing the content carries can promote it; GHSA-8r7g default + no-self-trust gate-checked intact | **MET** — 5 frontmatter claims + a `PROMOTE` marker + a manifest, all inert |
| Provenance per item: tool, version, source-relative path, domain-separated digest, import time | **MET** — asserted field-by-field over every contained item |
| Selection by the SAME published identity; an unpublished identity refused | **MET** — refused by name, not ignored |
| imported + quarantined + excluded == discovered, over the FULL committed corpora | **MET** — both corpora, and 554 items at 540-scale |
| Export preserves provenance, still excludes secrets by default, round trip does not launder quarantine | **MET** — existing default-exclusion assertions untouched and still green |
| Inertness proven live against the REAL binary, negative leg with three joint assertions + a positive control that fires | **MET** — positive fired in 2.4s, negative exhausted 45s |
| Containment contract MEASURED at 540-item scale, then decided by a four-way cross-audited panel bound to the measurement | **MET** — 4/4 `contract-accept`, bound by gate |
| Panel four-way IN FACT: checker survives kimi's bullet prefix AND codex's repeated block | **PARTIAL — see F26-02-C** — kimi shape tolerated; codex repeated-identical-block shape is REJECTED by 26-01's checker |
| Every gate can go red, including a non-zero test count on the quarantine run | **MET** |
| Every Linux gate ran in this plan's OWN worktree with the SHA asserted before and after | **MET** — `/root/wayland-f26-02`, never the shared tree |

## What landed

- `crates/wcore-cli/src/migrate/quarantine.rs` — classification + containment.
  Quarantine root is `<wayland config dir>/migrate-quarantine`, which is not
  `skills`, not `skills/auto`, and not under any `.wayland-core/`. Ceilings and
  the symlink refusal mirror `workspace_trust` (512 / 4 MiB / 32 MiB), with a
  drift guard that READS `workspace_trust.rs` at test time so "mirrored" is
  checked rather than asserted in a comment.
- `crates/wcore-cli/src/migrate/provenance.rs` — domain-separated digest
  (`wayland-migrate-item-v1\0`, distinct from the workspace-trust prefix),
  path-normalized so a corpus digests identically across platforms.
- `crates/wcore-cli/src/migrate/select.rs` — conservation as a TYPE property
  first (one outcome per identity, so an item cannot hold two) and arithmetic
  second.
- `crates/wcore-cli/src/migrate/mod.rs` — `--select` / `--exclude`, plus two new
  verbs: `migrate quarantined` and `migrate promote`.
- `crates/wcore-config/src/profile.rs` + `crates/wcore-cli/src/profile.rs` —
  selective export carrying `PROVENANCE.json`; import sweeps executable content
  into quarantine so a round trip cannot launder it.
- `scripts/portability-promotion-scale.sh`, the fixtures, and the panel record.

**No `crates/wcore-cli/src/{lib,main}.rs` edits at all** — `pub mod migrate`
already existed and the new subcommands are additive enum variants. Verified
against the merge-base SHA `6df10dab`, not the branch name.

## How each executable kind is classified

| Kind | Executable when | Decided by |
|---|---|---|
| Skill body | carries a shell directive | `wcore_skills::shell::contains_shell_commands` — the SAME predicate `permissions.rs` keys off and `executor.rs` runs, including its `LoadedFrom::Mcp` exemption |
| Peer MCP definition | carries a launch command | `command` is non-empty — deliberately NOT keyed on the declared transport, which is peer-controlled data that must not talk the classifier out of a containment decision |
| Hook definition | carries a command | non-empty command, per GHSA-8r7g's own reasoning in source |
| Persona / memory note / settings / asset | never | `classify_data_kind` — data imports without ceremony, because treating everything as dangerous trains an operator to promote without reading |

## The promotion mechanism, and why content cannot trigger it

`QuarantineStore::promote` consults exactly two things: the identities its
CALLER passed (in production, the operator's `migrate promote --id …` command
line) and the store's own index, which the store wrote. It reads no frontmatter
key, no marker file, no manifest entry and no filename out of the payload. The
committed fixture asserts its own trust five ways in frontmatter (`trusted`,
`auto_promote`, `promoted`, `wayland_quarantine: exempt`, `quarantine: false`),
plus a sibling `PROMOTE` file and a `manifest.json` claiming `"promoted": true`.
It stays contained. That is the half of GHSA-8r7g that actually holds.

## The live inertness proof — both legs

- **NEGATIVE** (`t19`): after importing the corpus, the REAL binary is driven
  through a real agent turn (`--json-stream --force`, scripted mock provider,
  `Skill` tool call). Four assertions together: import discovered a non-zero
  count; the payload is reported quarantined; **the stream shows the Skill tool
  ran and reported the skill unavailable**; the sentinel is ABSENT.
- **POSITIVE CONTROL** (`t20`): SAME payload, SAME turn, differing only by
  `migrate promote`. Sentinel PRESENT.
- **Measured asymmetry:** positive returned in **2.4s** (the moment the sentinel
  appeared); negative exhausted its full **45s** window. Both sentinels live
  inside the per-run home and absence is asserted before each leg begins.

**Two false greens this proof caught in its own construction**, both worth
recording because either would have shipped as a pass:

1. The sentinel initially sat in a tempdir outside the session workspace. The
   positive control did NOT fire — the leg was measuring the **sandbox**, not
   the quarantine boundary.
2. With that fixed, BOTH legs still failed: the engine refused the turn
   (`no encrypted credentials vault is unlocked`) and the legs were measuring a
   **dead engine**. The negative leg would have read as a clean pass. The
   turn-ran assertion added in between is what surfaced it.

## The Task 4 measurement (`panel/26-02-containment-contract/promotion-scale.txt`)

```
SCALE-DISCOVERED: 554     SCALE-IMPORTED: 13
SCALE-QUARANTINED: 541    SCALE-EXCLUDED: 0        SCALE-BALANCES: yes
PROMOTE-COST: items=1   invocations=1
PROMOTE-COST: items=256 invocations=1              PROMOTE-SCALING: bounded
CLASSIFY-DATA-QUARANTINED: 0   CLASSIFY-EXEC-UNCONTAINED: 0
CEILING-REFUSES-REALISTIC: yes
CEILING-CONSTANTS: files=512 file_bytes=4194304 total_bytes=33554432
POSITIVE-CONTROL: fired
SCALE-CORPUS-MATERIALISED-SKILLS: 540   SCALE-STORE-ADMITTED: 512
```

**The measurement changed the code twice**, which is the point of taking it:

1. `CLASSIFY-EXEC-UNCONTAINED` first read **1 with nothing actually
   uncontained**. The script had subtracted a contained count from a published
   count, and the emitted plan lists each executable item twice (once in
   `published`, once in `would_quarantine`). A subtraction between two reports
   is not a measurement of the surface. It now reads the home the import
   actually wrote.
2. `PROMOTE-SCALING` first read **linear** — `items=256 invocations=256`. Cause
   reproduced directly: a real install reuses one skill name across profiles
   (256 quarantined items, **46 distinct directory names**), and `promote`
   aborted the whole set on the first collision. That is precisely the
   "operator routes around containment" failure. Fixed inside this plan's own
   files, regression-guarded by `t22`, re-measured at `invocations=1`.

**Decision:** `CHOSEN: contract-accept`, `BASIS: majority`, 4/4 unanimous
(codex, gemini, kimi, internal). The internal pass was written AGAINST that
consensus and pressed `contract-reject` and `contract-amend-ergonomics` by name.

## Findings

| ID | Severity | Finding |
|---|---|---|
| **F26-02-A** | *(fixed in-plan)* | Promotion aborted an entire set on the first name collision, costing one operator invocation per item at real scale. Fixed: collisions resolve to a digest-disambiguated name, mapping reported, nothing overwritten or dropped. |
| **F26-02-B** | **MEDIUM** | The 512-item store ceiling refuses 29 of a realistic 541-item executable surface, and the **naive recovery does not work** — measured: after `promote --all`, a plain re-import reports `discovered=554 imported=0 quarantined=554` and refills with the same first 512, because scan order is stable. A recovery DOES exist and is discoverable from the tool's own output: every refusal is named, and `--select` on those identities gives `quarantined=29 excluded=525` (balancing), after which one `promote --all` leaves **541 skills on the load path — the complete corpus**, in four invocations total. `MAX_EXECUTABLE_FILES` was NOT raised. → BACKLOG. |
| **F26-02-C** | **MEDIUM** | `scripts/panel-decision-check.sh` (26-01's file) **rejects a capture carrying a repeated IDENTICAL verdict line** — `capture for 'codex' carries 2 PANEL-VERDICT lines; exactly 1 required` — which is a shape codex measurably emits. Isolated: kimi's bullet-prefixed verdict IS tolerated (rc=0); only the codex duplicate shape fails (rc=1). The ambiguity direction is correct (two DIFFERENT verdicts → rejected). **Not fixed here**: `panel-decision-check.sh` is not in this plan's `files_modified`, and the plan directs that a red trap gate is "a 26-01 defect to record and escalate rather than a file for this plan to edit". **No vote was lost in this run** — codex emitted exactly one verdict line, and the real panel record passes the checker. → escalate to 26-01. |
| **F26-02-E** | **MEDIUM** | `wcore-protocol::desktop_contract_corpus::checked_corpus_matches_real_serializers_byte_for_byte` fails: `Desktop contract corpus drift: drifted=["adversarial/events/fixture-mismatch.jsonl", "adversarial/events/schema-mismatch.jsonl", "adversarial/events/version-mismatch.jsonl", "events/ready.json", "manifest.json"]; run \`wcore-contract generate\``. **Verified PRE-EXISTING**: re-run alone at my merge-base `6df10dab` in a separate worktree it fails with byte-identical drift text (`BASE_RC=100`). This lane touches no `wcore-protocol` source and adds no protocol event — the scope fence forbids it. The repair is `wcore-contract generate`, which this lane is **forbidden** to run (release-coordination action). → BACKLOG + **seam request to the orchestrator**, below. |
| **F26-02-D** | **LOW** | The 540 scale point was **materialised** by the measurement script into 26-01's structural corpus, which ships those directories as markers with no `SKILL.md` (26-01 deviation 5, bounded generator). Structure is the real install's, body is the committed fixture. The script prints `SCALE-CORPUS-MATERIALISED-SKILLS: 540` so it can never be mistaken for a number the corpus shipped. |

Residual noted by every panel member: `CLASSIFY-EXEC-UNCONTAINED: 0` is a floor
over the surfaces it inspects (MCP definitions carrying a command in
`config.toml`; directive-carrying skill bodies on the load path), not a proof of
exhaustiveness over surfaces nobody has named.

## Gate results — real numbers

- `cargo fmt --all -- --check`: clean (Mac).
- `cargo clippy --locked -p wcore-config -p wcore-cli --all-targets -- -D warnings`: **clean**.
- `cargo nextest run --locked -p wcore-config -p wcore-cli --no-fail-fast`:
  **2840 run, 2840 passed**, 9 skipped, 1 flaky
  (`deterministic_openai_loop::packaged_core_cancels_an_active_stream`, passed
  on retry, pre-existing and unrelated).
- `migrate_quarantine` alone: **29 run, 29 passed** (22 authored here + 7
  support-module self-tests).
- Scale script self-red: exits **3** against a binary that does not exist.
- Panel checker on the real record: **PANEL RECORD OK**.
- Measurement-binding / final-state gate: **PASS** (ceilings pinned, chosen
  option bound to the measurement, balances/PC/0/0 all hold).
- Panel secret-hygiene gate: **PASS, non-vacuous** — 7 real secret values
  extracted from both real peer homes, **0 hits** under the panel directory.
- Trap gate: **PARTIAL** — see F26-02-C.
- Aggregate `cargo nextest run --locked --profile ci --no-fail-fast`:
  **12502 run, 12501 passed, 1 failed**, 50 skipped, 2 flaky, 1 leaky.
  The single failure is
  `wcore-protocol::desktop_contract_corpus::checked_corpus_matches_real_serializers_byte_for_byte`
  — **PRE-EXISTING, see F26-02-E.** Delta against the two-crate run above:
  `wcore-config` + `wcore-cli` contribute 2840/2840 green in both.

**Two measurement traps this lane walked into and caught**, recorded because
either would have produced a false report:

1. The background harness reported the aggregate run as **"exit code 0"**. It
   was not — `AGG_RC=100`. The harness reported the status of the trailing
   `tail`, not of the `ssh`. The status was captured on its own line and read
   there; the log content is what the numbers above come from.
2. The trap-gate fixtures were first built with `tail -2` (as the plan's gate
   text suggests), which in THIS capture duplicated prose rather than the
   verdict line, so the "two different verdicts" fixture carried only one and
   the gate passed vacuously. Rebuilt to append the verdict lines explicitly —
   at which point it correctly went red on F26-02-C.
3. The trap gate was also first run under the tool's **zsh**, where
   `for o in $OPTS` does not word-split, so the "other" option expanded to all
   four ids and the mutation was a no-op. Re-run under POSIX `sh`.

## Deviations, each with its reason

1. **`crates/wcore-cli/tests/migrate_hermes.rs` edited** (not in `files_modified`).
   Unavoidable: `HermesArgs` gained two fields, so every construction site had to
   compile. The substantive change is one assertion, **strengthened not
   weakened**: it previously asserted `[mcp.servers.ijfw-memory]` WAS written to
   `config.toml`, which is exactly the launchable child-process surface
   T-26-02-03 rates critical — the old assertion pinned the defect in place. It
   now asserts the definition is absent from the live config AND that it is
   contained AND that profiles still imported, so the change cannot pass by the
   import doing nothing.
2. **Peer MCP definitions with a launch command no longer reach `config.toml`.**
   A deliberate behaviour change, required by T-26-02-03. Withheld server names
   are also stripped from imported profiles' `mcp_servers` lists, because a
   dangling reference would be silently picked up by a server of that name
   defined later, quietly undoing the containment decision.
3. **Ceilings mirrored as local constants** rather than by making
   `workspace_trust`'s `pub` — that file is not in `files_modified`. The drift
   guard reads it at test time instead, which is stronger than a comment.
4. **`AGENTS.md` shows as modified in the worktree.** Not mine, not in any of my
   commits — an external tool rewrote its own frontmatter timestamp. Left
   unstaged.
5. **TDD RED not observed per-test**, as in 26-01: the Mac cannot compile and
   each Linux round trip is a multi-minute remote build. Rigour was preserved by
   every absence assertion carrying a positive half — and in practice the suite
   went red on five real defects before it went green.

## Recorded unknowns (not resolved here)

- Whether any of the 540 real skills carries a directive form the existing
  detector does not match — a pre-existing detector gap belonging to
  `wcore-skills`, not a quarantine gap.
- Whether OpenClaw plugin and flow content needs a classification beyond the
  four kinds handled here.
- How quarantined content behaves under Windows path semantics — 26-04 owns it,
  and no Windows leg ran (see below).

## Seam request for the orchestrator (do not let this merge silently)

`wcore-contract generate` needs to be run by whoever owns the Desktop wire
contract, to reconcile the five drifted corpus files listed in F26-02-E. It is
**pre-existing at `6df10dab`**, so it is not this lane's to fix and not this
lane's to hide — but the aggregate suite is red at HEAD because of it, and a
reviewer who runs the full suite will see that red.

## Not achieved

- **No Windows leg.** `seandesktop` was reported refusing SSH auth on every
  account before this lane began, and I did not attempt to work around a
  Sean-reserved credential. Windows path semantics for quarantine placement are
  **NOT ACHIEVED — blocked on a Sean-reserved credential**, and belong to 26-04
  by the plan's own scope fence.
