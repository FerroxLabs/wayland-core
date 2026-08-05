---
phase: 26
title: Migration, Export, Backup, and Restore
graded_by: lane/grade-26
graded_at: 2026-07-29
base: 861d1b1a
verdict: GOAL PARTIALLY ACHIEVED
criteria: "SC1 MET-WITH-STATED-EXCEPTIONS; SC2 PARTIAL; SC3 PARTIAL; SC4 MET-WITH-STATED-EXCEPTIONS"
findings: "F26-GRADE-H1 HIGH (open) — a data skill is reported Imported and never written; F26-GRADE-M1 MEDIUM — quarantine classification covers 274 of 1909 real peer skills; F26-GRADE-M2 MEDIUM — the committed hermes corpus has 540 skill dirs and zero SKILL.md"
fences: "clean — no crates/, no .github/, no wcore-cli/src/lib.rs or main.rs"
---

# Phase 26 — Phase Verdict

**Goal (`.planning/ROADMAP.md:136`):** *Users can move, preserve, and restore Wayland
state without executing imported content or leaking secrets.*

This phase had **no verdict file** before this one. It is graded from its four Success
Criteria backwards, against the codebase and the live product — not from
`26-04-CERTIFICATION.md`, which is a party to the question. Where I reached the same
answer as that certification I say so; where I did not, the measurement is shown.

Every number below came from an unproxied tool (`/usr/bin/git`, `/usr/bin/grep`,
`/usr/bin/find`) or from a run of the real release binary. No arithmetic is inherited.

---

## Verdict

**GOAL PARTIALLY ACHIEVED.** Split by the goal's own verbs:

| goal clause | status |
|---|---|
| …**without executing** imported content | **ACHIEVED** — containment live-proven, positive control passed |
| …**without leaking secrets** | **ACHIEVED** — and, for the first time, positively controlled |
| **preserve** and **restore** Wayland state | **ACHIEVED** — backup/restore/exact rollback, Linux + real Windows |
| **move** Wayland state | **ACHIEVED Wayland↔Wayland; NOT ACHIEVED from a peer** |

The two safety properties — the hard ones, the ones that are expensive to retrofit and
embarrassing to get wrong — are real and are proven live. The capability properties are
where this phase is short, and the shortfall is concentrated in exactly one place: **the
peer→Core import half**, which the competitive ledger already named and which is the
thing that decides whether a competitor's user can switch to us.

| # | Success Criterion (verbatim) | Grade |
|---|---|---|
| 1 | Hermes/OpenClaw discovery and dry-run are typed, deterministic, secret-redacted, and non-mutating. | **MET-WITH-STATED-EXCEPTIONS** |
| 2 | Selective import/export preserves provenance and quarantines executable content. | **PARTIAL** |
| 3 | Backup, restore, profile migration, and reciprocal portability survive interruption and restore exact pre-operation state on rollback. | **PARTIAL** |
| 4 | Hostile fixture corpora prove conflict, secret-source remapping, isolation, and recovery semantics. | **MET-WITH-STATED-EXCEPTIONS** |

---

## Criterion 1 — MET-WITH-STATED-EXCEPTIONS

> *Hermes/OpenClaw discovery and dry-run are typed, deterministic, secret-redacted, and non-mutating.*

**I did not inherit this one. I re-ran it.** The macOS artifact binary from the original
leg still exists at `/Users/seandonahoe/f26-artifacts/b671/wayland-core`, and Sean's real
`~/.hermes` and `~/.openclaw` are on this machine, so the strongest evidence in the phase
was directly reproducible:

```
$ sh scripts/portability-real-state-check.sh /Users/seandonahoe/f26-artifacts/b671/wayland-core
extracted 7 real secret values to search for (values never printed)
hermes:   exit 0, well-formed JSON, items=14
hermes:   non-mutation confirmed (tree digest unchanged)
hermes:   searched 7 real values, 0 hits
openclaw: exit 0, well-formed JSON, items=3
openclaw: non-mutation confirmed (tree digest unchanged)
openclaw: searched 7 real values, 0 hits
REAL-STATE CHECK PASSED   (rc=0)
```

Independent tree digests I took myself, before and after my own runs, are byte-identical
— `~/.hermes` `d385dfa6…d75d` → `d385dfa6…d75d`, `~/.openclaw` `b43014b2…c71e` →
`b43014b2…c71e`. **Sean's real installs were not mutated.**

### The redaction claim was self-passing in shape, and now is not

"0 hits in either emitted document" is a **known-negative assertion**. The script defends
its *inputs* unusually well — an empty secret extraction, a zero item count, an
unparseable document and a changed tree digest are each a hard failure, which is more than
most instruments in this program carry. But **nothing in it demonstrated that the matcher
could fire.** A dead `grep` produces a clean pass.

Per §6b-ii I repaired the instrument rather than writing the defect up, and gave the
repair three assertions rather than two — new
`scripts/portability-redaction-positive-control.sh`:

```
EXTRACTED-TOTAL: 7
ACTUALLY-SEARCHED: 7
PLANT-LENGTH: 122   (value never printed)
hermes/openclaw  A1 known-positive:         PASS   matcher fires on a PLANTED real secret
hermes/openclaw  A2 known-negative:         PASS   untouched document carries none
hermes/openclaw  A3 dead-instrument-misses: PASS   the guarded failure mode reports 0
POSITIVE-CONTROL: PASS
```

A3 is the assertion that makes the control worth anything: it confirms the shape being
guarded against (a matcher pointed at a file that is not there) really does return the
comforting zero, so A1's success is discriminating. The plant goes into a **copy** of the
emitted document; no peer home is written. **With this, the 0-hit result is non-vacuous
for the first time.**

`--json` typing is structural, not conventional: `PortabilityPlan` cannot represent a
credential value, the drop happens at the `MigrationPlan` → `PortabilityPlan` boundary,
and there is no inverse conversion — so a consumer cannot render a secret through
`serde`, `Debug`, `Display` or an error formatter even deliberately.

### The exception, and it is the certification's own

The real-credential leg ran at **`b671f9ad`**, not at the certified tree. That is not a
technicality here:

```
$ /usr/bin/git log --oneline b671f9ad..HEAD -- crates/wcore-config/src/portability/ crates/wcore-cli/src/migrate/
   → 10 commits, and crates/wcore-config/src/portability/redact.rs is among the changed files
255d06ba fix(portability): scrub credentials embedded in free-form plan details
dd8579bc fix(portability): make the details scrub a type invariant, not a call-site habit
f63da68a fix(portability): narrow the credential name field to an identifier shape
```

**The redaction module itself changed after the only run that used real credentials.**
The direction is favourable — all three commits harden — but "the later code is stricter"
is an argument, and replacing arguments with measurements was the entire point of that
leg. At HEAD, redaction is proven on Linux canary corpora only. Re-running the probe I
already wrote against a HEAD macOS binary would close this; it needs a macOS build, which
is why it is an exception rather than a gap I could close in-lane.

`26-04-CERTIFICATION.md` states this limit in its own status line rather than smoothing
it. That is the behaviour to want and it is why I grade this MET-with-exceptions rather
than PARTIAL.

---

## Criterion 2 — PARTIAL. This is the criterion that decides the competitive question.

> *Selective import/export preserves provenance and quarantines executable content.*

`26-04-CERTIFICATION.md` records `F26-SC2: CLOSED` and `F26-02: CLOSED`. **Both are
overclaims,** and the live product says so.

### What I measured, on the real release binary

Built `wcore-cli --release` at my HEAD on `hetzner-dsm` (`wayland-core 0.12.25`), and
drove `migrate hermes --yes` against the committed hermes fixture plus one sentinel of
each content class `F26-02` names.

> Two earlier probe generations failed and I am recording that, because the first
> failure is what makes the third trustworthy. v1 hand-authored a `config.yaml`, the
> profile did not parse, and its known-positive went RED — so it **declared its own five
> absences meaningless** instead of reporting a clean sweep. v2 wrote the shell directive
> as ` ```!shell `, which the detector's block regex `(?s)```!\s*\n` does not match; that
> was **my error, not the product's**, and I corrected it rather than filing it.

**v3, with the positive control passing:**

```
Imported 13 profiles (0 skipped), 0 MCP servers, 0 credentials.
Accounting: discovered=16 imported=14 quarantined=2 excluded=0

=== FILES IN WAYLAND HOME ===
HOME/config.toml
HOME/migrate-quarantine/index.json
HOME/migrate-quarantine/payloads/mcp_server_ijfw-memory-dc8580321779/mcp-server.json
HOME/migrate-quarantine/payloads/skill_skills_probe-exec-a11e5e4fc1b6/SKILL.md
TOTAL: 4

Q1 CONTAINMENT: PASS — the executable skill IS in the home, quarantined
Q2 IMPORT:      data-skill body files_containing=0   (product reported Outcome::Imported)
```

**Q1 is what licenses Q2.** The probe demonstrably *can* see a skill-shaped write, because
it found the quarantined one. Q2's zero is a measurement, not an instrument dying.

**After importing a 13-profile, 542-skill, 13-persona peer home, the Wayland home holds
four files.**

| class named by F26-02 | bytes land in the Wayland home? |
|---|---|
| profiles | **YES** — 13, into `config.toml` |
| credentials | **YES** — opt-in, `--include-credentials` |
| provenance | **YES** — per quarantined item: digest, source tool, source path, promote-as |
| (MCP definitions, non-executable) | **YES** |
| persona (`SOUL.md`) | **NO** — 0 files |
| memory notes | **NO** — 0 |
| settings | **NO** — 0 |
| assets | **NO** — 0 |
| skills (as usable content) | **NO** — 0 |

This is not a hidden defect. `crates/wcore-cli/src/migrate/mod.rs:16` says so in the
module header — *"Skills, personas (`SOUL.md`), and long-term memory are detected and
counted in the preview but are NOT written in this slice"* — and `ImportSurface`
(`mod.rs:339`) carries exactly one field. Corroborated in source: outside
`quarantine.rs`, every filesystem write in `migrate/` is inside `#[cfg(test)]` fixture
setup; the complete production write set is `patch_global_config` (profiles + non-exec
MCP) and `QuarantineStore::admit`.

### F26-GRADE-H1 (HIGH, OPEN) — the product reports content imported that it never writes

`mod.rs:621`: a `Classification::Data` skill records `Outcome::Imported` **with no write
of any kind**, and `print_report` (`mod.rs:1026`) counts it in `imported=`. The same run
prints `542 skill directories` under `Detected but NOT imported`. So one invocation makes
two incompatible statements about the same items, and the filesystem supports neither.

**The test that should have caught this is the self-passing shape.**
`migrate_quarantine.rs:217` `t2_skill_without_a_directive_is_data_and_needs_no_promotion`
asserts `!store.contains("skill:skills/release-notes")` — a known-negative — and its
declared "positive half" (`report.quarantined >= 2`) proves the *siblings were
quarantined*, not that the data skill was written anywhere. **A no-op import passes t2.**
No assertion in that 21-test file checks that a data skill exists in the Wayland home
after an apply.

This is a HIGH rather than a MEDIUM because it is an honesty defect on the operator-facing
success path: a user migrating from a competitor is told their content came across.

### F26-GRADE-M1 (MEDIUM) — containment covers 14% of the real peer install

`peer_skill_roots()` scans `<home>/skills`, `<home>/plugin-skills`,
`<home>/profiles/*/skills`, `<home>/agents/*/skills`, and `scan_peer_skills()` inspects
only an immediate child directory holding a `SKILL.md`. Measured read-only against
`~/.hermes`, with a negative control on the matcher:

```
TOTAL SKILL.md in the real home  : 1909
skills/<d>/SKILL.md              :   22
plugin-skills/<d>/SKILL.md       :    0
profiles/<p>/skills/<d>/SKILL.md :  252
agents/<a>/skills/<d>/SKILL.md   :    0
SCANNED: 274 / 1909      UNSCANNED: 1635
negative control ./zzz-not-a-root/*/SKILL.md : 0
```

*(My first attempt at this used zsh globs and returned zero for a directory I could see
held 122 — the §3b-i instrument-death trap, caught by a positive control and re-measured
with `/usr/bin/find` exact-shape patterns. The numbers above are the re-measurement.)*

**This is not a live-execution hole, and the distinction matters.** An unscanned skill is
not imported either, so nothing executable escapes containment. The safety property holds
— but it holds substantially **because almost nothing is imported**. That is the vacuous
satisfaction this program has already named once (23A's governed promotion). **Whoever
wires skill import must widen `peer_skill_roots` in the same change**, or a completeness
gap converts into a containment gap.

### F26-GRADE-M2 (MEDIUM) — the committed corpus does not exercise skill classification

`crates/wcore-cli/tests/fixtures/portability/hermes` carries **540 skill directories and
zero `SKILL.md` files**. So `t13 conservation holds over both full committed corpora`
classifies **0 of 540**. The real install does use `SKILL.md` (1909 of them), so this is
fixture fidelity, not a product defect — but it means the at-scale corpus proves
conservation over profiles and MCP only.

### What genuinely IS met

Selection is real: `--select`/`--exclude` operate on the identities the dry-run published
and **refuse** an unpublished id rather than ignoring it. Provenance on contained items is
complete and verified live in `migrate quarantined` output. Containment is real and
correctly reasoned — an MCP definition carrying a launch command is executable *regardless
of declared transport*, so peer-controlled data cannot talk the classifier out of a
containment decision. And `profile export` / `profile import` is a genuine, complete,
selective Wayland↔Wayland round trip with secrets excluded by default.

**Panel: 3-0 PARTIAL** (codex gpt-5.6-sol, gemini-3.1-pro, kimi K3), plus an internal pass
arguing NOT MET on the grounds that a migration importing 2 of 8 named classes has not
"imported" in any sense a user would recognise. It loses on the criterion's literal text:
provenance preservation and executable quarantine are both genuinely proven, and a real
selective export/import exists. PARTIAL, not NOT MET.

---

## Criterion 3 — PARTIAL

> *Backup, restore, profile migration, and reciprocal portability survive interruption and restore exact pre-operation state on rollback.*

Four named operations. They do not have the same answer, so they are graded separately.

### backup + restore — the full clause is MET

Interruption survival and exact rollback, Linux **and real Windows**
(`SeanD@seandesktop`, release binary built on the box). The Windows uncatchable-kill leg
(`TerminateProcess`) reports mid-flight established and `DIGEST-EQUAL: yes`. This path
also produced the phase's best product outcome: **F26-03-D was fixed at the product, not
at the fixture** — `atomic_write`'s tempfile round trip reached Win32 without long-path
handling and failed with **os error 3** at a 320-character non-verbatim absolute path, in
a function 41 modules call. The single-variable isolation is recorded: the earlier fixture
had `canonicalize()`d its base, which on Windows returns a `\\?\` **verbatim** path, so
the fixture had been running in the very mode that works. That is a real defect found by
refusing to accept a green.

### profile migration + reciprocal portability — interruption MET, rollback NOT

**The data-loss defect the brief asked me to confirm is genuinely fixed, and it is proven
by a kill distribution rather than by inspection.** I re-derived every figure from the
logs myself:

| log | source SHA | TRIAL lines | mid-apply | corrupt index | unrecovered | verdict line |
|---|---|---|---|---|---|---|
| `hermes.log` | `c23a08b9` (pre) | 21 | 17 | 1 | **1** | `PROOF: FAIL` |
| `openclaw.log` | `c23a08b9` (pre) | 21 | 18 | 4 | **4** | `PROOF: FAIL` |
| `hermes-fixed.log` | `a170ee24` (post) | 21 | 17 | 0 | **0** | `PROOF: PASS` |
| `openclaw-fixed.log` | `a170ee24` (post) | 21 | 18 | 0 | **0** | `PROOF: PASS` |

**35 mid-apply `SIGKILL`s per side. Pre-fix 5/35 unrecovered — 14.3%, matching the brief's
"5 of 35". Post-fix 0/35.**

The single most important fact about this instrument: **the pre-fix logs carry
`PROOF: FAIL` from the same harness.** The gate could fail and did. Supporting controls,
read out of the logs rather than the prose: `DETERMINISM-CONTROL: pass`;
`SENSITIVITY-CONTROL: pass (profile-drop, payload-byte, index-entry all detected)` — three
deformations that must each move the fingerprint, which is what stops the comparand from
being normalised into uselessness; `nokill.log` as the classifier's negative control
(`KILL-ENABLED: no`, `CLASS-MID: 0`); `SENTINEL-UNCHANGED: yes` on every run.

Fix confirmed at my base, not just claimed: `quarantine.rs:369-374` `save_index` calls
`wcore_config::atomic_write`; `git log -1 -- .../quarantine.rs` → `a170ee24`; and
`git merge-base --is-ancestor a170ee24 HEAD` → **ancestor**, so nothing has edited that
file since the proof.

**But the criterion's literal text is still unmet by the migration path, and the
interruption is what exposed it: `migrate` has no rollback.** It does not return the home
to its pre-operation state; it leaves partial work in place and converges on the
*completed* state when the product is driven again. That is a defensible import contract —
arguably better than rollback, since re-running is what a user does — and it is now
proven. It is not "restore exact pre-operation state on rollback."

Two limits I did not round off: **Linux only** — the migration harness is POSIX `sh` with
no PowerShell peer, so the migration path has no Windows interruption leg. And the 440-item
corpus is synthetic; no real peer home was interrupted. Residual after the fix:
`ORPHAN-PAYLOAD-TRIALS` is still 9 (hermes) / 11 (openclaw) — a payload can be written
before its index entry exists. Every such trial recovers on re-drive, so it is not data
loss; it is not "no orphans" either.

**`26-GAPS-SUMMARY.md` graded this OPEN on exactly this reasoning and I reached the same
place independently.** That lane found a HIGH, reported it red with its evidence before
its fix, and then declined to claim the criterion. That is the correct behaviour and the
grade should not punish it.

---

## Criterion 4 — MET-WITH-STATED-EXCEPTIONS

> *Hostile fixture corpora prove conflict, secret-source remapping, isolation, and recovery semantics.*

`crates/wcore-cli/tests/portability_hostile_corpus.rs` carries **23 tests**, and its
construction defends against the failure mode that matters for a corpus suite: each case
**declares its expected outcome as data** in `scripts/portability-hostile-gen.py`, and the
suite binds itself to that declaration (`assert_eq!(case.expect, …)`) before asserting the
class-specific predicate. A case cannot silently degrade from `refused` to `imported` and
still pass. Corpora are materialised on the target platform at run time rather than
committed, which is what lets the Linux and Windows legs be compared at all.

The four named semantics each have a corpus: conflict (exact, case-folded, normal-form
names); secret-source remapping across all four credential backends; isolation via an
external sentinel tree digested before and after on both platforms; recovery under refusal
and manifest/payload mismatch.

The remapping arm is measured correctly — `scripts/portability-remap-capture.sh` derives
`REMAP-TARGET-WRITTEN` by digesting the target **before and after** (`digest_of()` via the
product's own `backup digest`), so "no refusal wrote its target" is a digest fact rather
than a message read back off the console. That is the right shape for a known-negative and
it is the one place besides SC1 where this phase got that shape right unprompted.

**Exceptions.** I did **not** independently re-execute this suite or the Windows matrix —
I graded its construction and its committed outputs, not a fresh run. And the isolation and
remapping arms are known-negative families; the sentinel-digest and target-digest controls
are genuine, but neither carries a *planted-positive* arm of the kind I added for SC1. I
would not call that a gap, but it is the difference between MET and MET-with-exceptions.

---

## Costed gap list

Estimates are lane-sessions of the size this program has been running.

| # | Missing capability | Criterion | Lane-sessions | Credential needed |
|---|---|---|---|---|
| **G1** | **The import half — peer→Core apply writes persona, memory, skills, settings and assets into the Wayland home, with `peer_skill_roots` widened in the same change so containment coverage grows with it.** | SC2 | **3–4** | **No** |
| G2 | F26-GRADE-H1 — stop reporting `Outcome::Imported` for content that is not written; make the accounting vocabulary match the filesystem, and add the assertion `t2` is missing (a data skill IS present in the home after apply) | SC2 | 0.5 | No |
| G3 | Rollback for `migrate` — a journal + reverse-apply so an interrupted migration restores the pre-operation home, rather than converging forward on re-drive | SC3 | 2 | No |
| G4 | A Windows interruption leg for the migration path (PowerShell peer to the POSIX harness, `TerminateProcess` as in 26-03) | SC3 | 1 | No — but needs `SeanD@seandesktop` |
| G5 | Re-run the real-credential redaction probe + my positive control against a **HEAD** macOS binary, closing the `b671f9ad` gap on `redact.rs` | SC1 | 0.5 | No — needs a macOS build |
| G6 | F26-03 first clause — consume the F23 `SessionExportEnvelope` to export a portable session corpus (measured absent, disposed 4-0, `F26-GAPS-03`) | F26-03 | 2 | No |
| G7 | F26-GRADE-M2 — give the committed hermes corpus real `SKILL.md` bodies so at-scale conservation exercises classification | SC2/SC4 | 0.5 | No |

### G1 is the one that decides the competitive question — stated plainly

`COMPETITIVE-LEDGER.md:156` records that **both peers migrate from each other, and Core is
the only party with no reciprocal path.** Two corrections to that row, in opposite
directions, and both matter:

1. **The row is stale.** It says plans 26-02 and 26-04 "were never started" and "Nothing
   has yet imported anything." Both are false at HEAD — 26-02, 26-04 and the certification
   are all on disk, and `migrate` has a real apply path that writes. The row was written
   2026-07-28 and should be refreshed.
2. **Its conclusion nonetheless survives the refresh.** A peer→Core migration that lands
   four files — thirteen `config.toml` profile stanzas and two inert quarantine payloads —
   is a **credential and endpoint import, not a setup migration.** The user's personas,
   memory, skills, settings and assets stay on the competitor's disk. Against peers that
   each ship a working importer for the other, that is the gap that decides whether a
   competitor's user can actually leave, and it is **3–4 lane-sessions and no credential.**

The ledger's open question for F30 — *"Core is the only party with no reciprocal path"* —
should be answered: **false for discovery and dry-run, still true for apply in every
content class except profiles and MCP.**

---

## What is ungradeable, and why

- **The macOS real-credential arm at HEAD.** I reproduced it at `b671f9ad` and supplied
  its missing positive control, but `redact.rs` changed after that commit and no macOS
  binary at HEAD exists on this machine. Compiling on the Mac is forbidden and no permitted
  host runs macOS. This is a **rule-and-hardware gap, not an execution shortfall** — the
  probe is written and takes minutes once a HEAD macOS artifact exists (G5).
- **Windows legs.** I did not re-execute the 26-03 Windows interruption legs or the
  `portability-native-matrix.ps1` run; `hetzner-dsm` cannot reach `seandesktop`
  (`Permission denied (publickey)`, a real and separately-pending authorization). Those
  legs are graded on their committed artifacts and their construction, and I say so rather
  than implying I ran them.
- **The 2 failures in `cargo test --release -p wcore-cli` (2119 passed, 2 failed, 5
  ignored).** Characterised in `26-GAPS-SUMMARY.md` as a deliberately-panicking scaffolded
  fixture picked up from the target directory. I did not re-run the suite to confirm that
  characterisation. It does not bear on any criterion above — every criterion here is
  graded on live product exercise, not on a suite result.

---

## Fence exposure vs `861d1b1a`

```
$ /usr/bin/git diff --name-status 861d1b1a HEAD
A  .planning/phases/26-migration-export-backup-restore/26-GRADE-NOTES.md
A  .planning/phases/26-migration-export-backup-restore/evidence/26-grade/import-half-probe.sh
A  scripts/portability-redaction-positive-control.sh
$ /usr/bin/git status --porcelain | grep '^??'   → (none)
```

**No `crates/` change. No `.github/workflows/` change. No edit to
`crates/wcore-cli/src/lib.rs` or `main.rs`.** Three files added, all additive: the grading
notes, the import probe, and the redaction positive control. Nothing merged, no PR, no
tag, no issue closed, no `wcore-contract generate`.

Sean's real `~/.hermes` and `~/.openclaw` were read only; independent before/after tree
digests are byte-identical and recorded above. `/Users/seandonahoe/dev/resources/` was not
touched.

---

_Graded by lane `grade-26` at base `861d1b1a`. Live legs on `hetzner-dsm`
(`/root/wayland-grade26`, `wayland-core 0.12.25`) and on this Mac using the pre-existing
`b671` artifact binary — nothing was compiled on the Mac._
