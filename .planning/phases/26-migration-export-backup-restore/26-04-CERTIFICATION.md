# Phase 26 Certification — Migration, Export, Backup and Restore

This is the phase's reconciliation. Every one of the four Success Criteria and
every one of the five requirements is either mapped to evidence **this plan can
name and re-run**, or recorded OPEN with its **specific unmet clause**. A
criterion marked closed on another plan's say-so, without evidence this plan can
point at, is not closed here.

`accept-with-named-open` is the honest close, not a consolation. Phase 20A closed
with four requirements explicitly open and each unmet clause named, and that was
the right call. Two of these nine judgements are OPEN, and naming them precisely
now is cheap; inheriting them unnamed into Phase 28 is not.

F26-CERT-SHA: 0a75efd98b461a72fd099542b8e0650026491173

---

## The four Success Criteria, in the ROADMAP's own wording

Quoted verbatim from `.planning/ROADMAP.md`, never paraphrased — a criterion
restated more loosely is a criterion quietly lowered.

> 1. Hermes/OpenClaw discovery and dry-run are typed, deterministic, secret-redacted, and non-mutating.
> 2. Selective import/export preserves provenance and quarantines executable content.
> 3. Backup, restore, profile migration, and reciprocal portability survive interruption and restore exact pre-operation state on rollback.
> 4. Hostile fixture corpora prove conflict, secret-source remapping, isolation, and recovery semantics.

## The five requirements, in REQUIREMENTS.md's own wording

> - **F26-01**: Hermes and OpenClaw discovery produces a typed dry-run plan without changing state or exposing secret values.
> - **F26-02**: Persona, memory, skills, settings, assets, profiles, credentials, and provenance migrate selectively with conflicts and executable content quarantined.
> - **F26-03**: Users can consume the F23 redacted session/evidence envelope to export a portable profile/session corpus and perform authenticated backup, restore, and reciprocal migration without executing imported content.
> - **F26-04**: Secret sources are explicitly remapped; rollback restores the exact pre-operation state after interruption or partial failure.
> - **F26-05**: Fixture installations and hostile import/export/restore corpora prove isolation, portability, and deterministic reporting.

---

## Status lines

These nine lines are the machine-checkable form of the judgements below. The
prose is what a human reads; the line is what makes the claim checkable, and
Task 4 RE-EXECUTES the evidence every `CLOSED` line names.

F26-SC1: CLOSED — typed, deterministic, structurally secret-redacted and non-mutating discovery for BOTH peer sources, proven on Linux canary corpora and on Sean's REAL macOS installs with live credentials, 0 secret hits and both homes unmutated. platform=linux+macos evidence=.planning/phases/26-migration-export-backup-restore/26-01-BASELINE.md
F26-SC2: CLOSED — selective import preserves per-item provenance and contains executable content by PLACEMENT outside every skill load root, proven live against the real binary with a paired positive control, and re-proven under hostile input on Linux and real Windows. platform=linux+windows evidence=crates/wcore-cli/tests/migrate_quarantine.rs
F26-SC3: OPEN — `backup restore` survives interruption and rolls back exactly on Linux AND real Windows (26-03, lane 26c). But the criterion also names "profile migration, and reciprocal portability", and NEITHER was ever interrupted: no plan in this phase killed a `migrate hermes` or `migrate openclaw` mid-apply, so the exact-rollback contract for the migration path rests on a partial-failure argument rather than on a measured interruption. That specific clause is the unmet one.
F26-SC4: CLOSED — hostile corpora with DECLARED outcomes prove conflict semantics for exact, case-folded and normal-form names; secret-source behaviour for secrets hidden in memory notes, persona bodies, skill bodies and dotenvs; isolation by an external sentinel tree digested before and after on BOTH platforms; and recovery semantics under refusal and manifest/payload mismatch. platform=linux+windows evidence=scripts/portability-native-matrix.sh
F26-01: CLOSED — `migrate --json` emits the typed plan in which a credential VALUE is unrepresentable by type; byte-identical across independent walks; tree digest unchanged on both corpora and both REAL peer homes; the mandatory macOS leg RAN and its provenance re-derives from GitHub. platform=linux+macos evidence=.planning/phases/26-migration-export-backup-restore/26-01-BASELINE.md
F26-02: CLOSED — profiles, skills, personas, memory, settings, MCP definitions and credentials migrate selectively by published identity with conflicts reported rather than silently applied, executable content quarantined, and provenance recorded per item. platform=linux+windows evidence=crates/wcore-cli/tests/migrate_quarantine.rs
F26-03: OPEN — authenticated backup, restore and reciprocal migration are built and proven, and imported content is never executed. But the requirement's FIRST clause — "consume the F23 redacted session/evidence envelope to export a portable profile/session corpus" — is entirely unaddressed: no plan in this phase reads an F23 envelope, `crates/wcore-cli/src/backup/` contains no reference to a session or evidence envelope, and 26-03's SUMMARY never mentions one. The session-corpus half of this requirement was never started.
F26-04: CLOSED — secret sources are explicitly remapped across all four credential backends with the operator told the backend, the count and the action, and no refusal wrote its target (measured by digest, not read off the message); exact rollback from an uncatchable mid-flight kill holds on Linux and on real Windows. platform=linux+windows evidence=scripts/portability-remap-capture.sh
F26-05: CLOSED — hostile corpora are generated on the target platform at run time, every case declares its expected outcome as data and the suite asserts that outcome against the REAL binary, isolation is proven by an external sentinel on both platforms, and the normalised report is byte-identical between Linux and real Windows. platform=linux+windows evidence=crates/wcore-cli/tests/portability_hostile_corpus.rs

---

## Criterion by criterion

### Success Criterion 1 — CLOSED (linux + macOS)

| What | Evidence | Platform | What distinguished working from broken |
|---|---|---|---|
| Typed dry-run | `migrate hermes --json` / `migrate openclaw --json` | linux | A credential VALUE is unrepresentable in `PortabilityPlan` by type, with no inverse conversion |
| Deterministic | two independent walks of the same corpus | linux | byte-identical emitted documents |
| Secret-redacted | 36 hermes + 28 openclaw canaries | linux | **0** canaries in either emitted document, while 13 credential REFS were present (the positive half) |
| Secret-redacted, real credentials | `scripts/portability-real-state-check.sh` against `~/.hermes` and `~/.openclaw` | **macos** | 7 REAL secret values extracted (non-vacuous), **0** hits in either document |
| Non-mutating | tree digest before/after | linux + macos | digest unchanged on both corpora and both real homes |

**The macOS leg is the one an earlier revision of this phase tried to certify
around, so it is re-derived from GitHub rather than transcribed.** 26-01 records
`F26-SC1-MACOS: RAN — run=30229917833 sha=b671f9ad557f85a36cc67da3d3ec0218f0bf08e8
binary=… arch=arm64 hermes_profiles=12 openclaw_items=3 secret_hits=0`. Re-derived
on 2026-07-28: that run's `headSha` is `b671f9ad557f85a36cc67da3d3ec0218f0bf08e8`,
its `Build (aarch64-apple-darwin)` job concluded `success`, and it still publishes
a LIVE non-empty `wayland-core-aarch64-apple-darwin` artifact — checked with
`expired==false and size_in_bytes>0`, because an expired artifact still appears
in the listing and the name alone proves nothing.

**Honest limit, carried forward:** the macOS run was taken at `b671f9ad`. The
three hardening commits 26-01 landed after it are Linux-proven only.

### Success Criterion 2 — CLOSED (linux + windows)

Provenance per item (tool, version, source-relative path, domain-separated
digest, import time) is asserted field by field over every contained item.
Executable content is inert **by placement**: the quarantine root
(`<config dir>/migrate-quarantine`) is outside all four skill load roots, and
the REAL loader lists a contained skill only after an explicit promotion.
The live proof is paired: the negative leg drives a real agent turn and asserts
the Skill tool RAN and reported the skill unavailable with the sentinel ABSENT;
the positive control, identical but for `migrate promote`, produces the sentinel
in 2.4s while the negative leg exhausts its full 45s window.

26-02 had **no Windows leg** (`seandesktop` was believed unreachable at the
time). This plan supplies it: the Windows matrix run exercises the same
quarantine and classification path on NTFS at the certified SHA, and its
platform report records the outcome for every case.

### Success Criterion 3 — OPEN

**What IS proven.** `backup restore` interrupted by an uncatchable mid-flight
kill rolls back to the exact pre-operation tree, on Linux (`SIGKILL`) and on real
Windows (`TerminateProcess`), over a target that carried state, with
`DIGEST-EQUAL: yes` on both and a negative control proving the mid-flight check
can fire (exit 9). The Windows long-path defect F26-03-D was fixed at the
product, not by narrowing the fixture, and re-proven at 377 absolute characters.
This plan adds a refused-restore leg over an OCCUPIED target holding a live
profile, measured by digest rather than read off the refusal message.

**The specific unmet clause.** The criterion names four things —
"Backup, restore, **profile migration, and reciprocal portability**". Only
`backup restore` was ever interrupted. No plan in this phase killed a
`migrate hermes` or `migrate openclaw` mid-apply, so exact rollback for the
MIGRATION path rests on a partial-failure argument and on the atomic
`patch_global_config` writer rather than on a measured interruption. Recording
this as closed would be exactly the rounding-up this certification exists to
prevent.

**Also carried:** F26-03-E (MEDIUM) — on Windows the handler probe does not
fire, so `fired=no` there is corroborated by documented Win32 semantics plus a
delivered-but-unrecorded event rather than by an instrumented probe. None of the
four load-bearing measurements depends on that probe.

### Success Criterion 4 — CLOSED (linux + windows)

| Clause | Evidence | Platform |
|---|---|---|
| conflict | exact, case-folded and Unicode normal-form name collisions, each with a declared outcome asserted | linux + windows |
| secret-source remapping | canaries hidden in a memory note, a persona body, a skill body and a dotenv; **0** reached any plan, report or `migrate quarantined` listing, with the canary first proven PRESENT in the corpus | linux + windows |
| isolation | a sentinel tree OUTSIDE every target home, digested by the product's own `backup digest` before and after every hostile operation, required unchanged | linux + windows |
| recovery | a refused restore leaves an occupied target byte-identical; a manifest whose payload no longer matches it is refused by `backup verify`, with the untampered archive verifying first so the rejection is not vacuous | linux |

Full per-case results, including what each case deforms and which field it
attacks, are in `26-04-SUMMARY.md`.

---

## Deliberately uncrossed seams

Recorded so a later phase finds them rather than rediscovering them.

1. **`wcore-protocol` — untouched across the entire phase.** No migration,
   quarantine, backup or restore event was added to the JSON stream protocol.
   The D1 producer contract, its generated fixtures and every Desktop consumer
   are unaffected by all four plans. If any of this work should become
   observable to a host, that is a **contract change requiring the D1
   checkpoint**, not an additive edit.
2. **The credential backends were read, not changed.** Whether a home whose
   secrets live in the OS keyring can ever be made portable without the operator
   re-entering them is an open **product** question, not a defect. The remap
   names the gap and prescribes the action; no plan proved that following the
   prescribed action produces a working install.
3. **No configuration key was added to `config.toml` by this plan.** Had one
   been, it would be a shared-schema change, and this project's own standing
   lesson requires verifying such a change with a workspace-wide check across
   all targets rather than a per-crate one, because a per-crate check misses
   downstream exhaustive matches. `cargo check --locked --workspace
   --all-targets` was run at the certified SHA regardless, and passed.

## Recorded unknowns — noted, not resolved here

- **macOS is not a gate host in this phase.** Case and normal-form behaviour on
  APFS is measured only by the generator's own post-creation verification (both
  distinctions COLLAPSE there), not by running the product against those corpora.
  Phase 28's native certification owns that.
- Whether a hostile class exists that neither peer format suggests, and which
  these corpora therefore omit.
- Whether a keyring-backed home can be made portable at all without operator
  re-entry.

## The four-plan cap

**Held.** Phase 26 executed exactly four plans. Work these corpora surfaced that
does not fit is in `.planning/BACKLOG.md` with a severity; no plan five was
created.
