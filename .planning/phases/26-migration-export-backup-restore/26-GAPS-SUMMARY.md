---
phase: 26
plan: GAPS
subsystem: migration, portability, backup
status: complete-with-named-open
lane: lane/26-gaps
merge_base: c23a08b9d990a729005792f871eeab323a1543d6
requirements_disposition: "F26-03: first-clause-unimplemented-recorded; F26-04/SC3: open-on-a-sharper-clause"
findings: "F26-GAPS-H1 HIGH fixed and re-proved; F26-GAPS-01/02/03 to BACKLOG"
---

# Phase 26 — the two gaps, measured

This lane owned exactly the two things 26-04's certification named OPEN, and it
grades them the way that certification did: with the remaining unmet text quoted,
not summarised. **Neither is closed.** One produced a HIGH, which is the most
valuable thing this lane could have produced and is reported red below with its
evidence before its fix.

Phase 26 Success Criterion 3, quoted verbatim from `.planning/ROADMAP.md`:

> 3. Backup, restore, profile migration, and reciprocal portability survive interruption and restore exact pre-operation state on rollback.

---

## Target 1 — the interruption clause

### What was interrupted, and where

`migrate hermes` (profile migration) and `migrate openclaw` (reciprocal
portability) were each killed with an **uncatchable `SIGKILL`** at 21 points
swept across the measured apply window, over a 440-item corpus, on
`hetzner-dsm`. The harness is
`scripts/portability-migrate-interrupt-proof.sh`.

The kill lands inside `apply_plan`'s admit loop. That loop is the migration's
real write phase: it calls `QuarantineStore::admit` once per executable item, and
each admit writes the item's payload and then re-serialises and rewrites the
WHOLE quarantine index. `patch_global_config` — the only atomic writer in the
path — runs once, last, after every admit.

`migrate` carries no pacing affordance the way `backup restore` carries
`--pace-ms`, and adding one would have meant instrumenting the thing under test.
So the window was made real instead: 440 items is a size a real Hermes home
reaches (26-02 measured 540 skill directories in the real install), and the
per-item whole-index rewrite makes the apply cost O(N²) in index bytes. Measured
apply duration: **308 ms** (hermes), **275 ms** (openclaw).

### What the kill actually found — F26-GAPS-H1, HIGH

**`QuarantineStore::save_index` used a truncating `fs::write` on the live index,
once per admitted item.** A process killed inside that window leaves a
partially-written JSON document. Because `load_index` parses the file, the
consequences are terminal:

| | |
|---|---|
| post-kill index | 143,360 bytes, ending mid-string with no closing brace |
| `migrate quarantined` | **exit 1** — the operator cannot list anything |
| re-running the migration | **exit 0**, while refusing all 440 items with `quarantine index is invalid: EOF while parsing a string at line 3758 column 13` |
| payloads left on disk | **331 directories**, orphaned, unreachable by `promote` |
| profiles imported | **0** — `patch_global_config` never ran |

So the user's visible outcome is a migration that reports success and has
imported nothing, listed nothing, and left hundreds of contained directories
that no product surface can reach — and every retry reproduces it, because the
retry re-hits the same unparseable file.

Measured at the merge base `c23a08b9`, across both peer paths:

| peer | mid-flight kills | corrupt index | unrecovered | re-drive exit |
|---|---|---|---|---|
| hermes | 17 | 1 | **1** | 0 |
| openclaw | 18 | 4 | **4** | 0 |
| **total** | **35** | **5** | **5 (14%)** | **0 in every case** |

Evidence: `evidence/26-gaps/hermes.log`, `evidence/26-gaps/openclaw.log`, and
the fully preserved failing case in
`evidence/26-gaps/unrecovered-hermes-trial-13/`.

**The fix.** `save_index` now uses `wcore_config::atomic_write` — the project's
existing sibling-tempfile / `sync_all` / rename helper, which already carries the
Windows long-path handling 26-03 added for F26-03-D. A second definition here
would have been the duplication the crate map forbids.

**Re-proved at `a170ee24`, identical sweep, same hardware:**

| peer | mid-flight kills | corrupt index | unrecovered |
|---|---|---|---|
| hermes | 17 | **0** | **0** |
| openclaw | 18 | **0** | **0** |

Evidence: `evidence/26-gaps/hermes-fixed.log`,
`evidence/26-gaps/openclaw-fixed.log`.

This finding is worth stating plainly against what 26-04 wrote, because it is the
point of having interrupted the thing. That certification recorded exact rollback
for the migration path as resting "on a partial-failure argument and on the
atomic `patch_global_config` writer". The argument was reasonable and it was
wrong: `patch_global_config` is indeed atomic, but it is not the writer the kill
lands in.

### Recovery was observed, not asserted

Every trial re-drives the real product after the kill and compares the resulting
home to a clean reference run. Post-fix, **35 of 35** mid-apply interruptions
converge on the reference state with a zero-exit re-drive.

The comparand is `scripts/portability-migrate-state.py`. It normalises exactly
one field — `Provenance::imported_at`, which is wall-clock and therefore differs
between two CORRECT runs — plus `config.toml` section order and the embedded home
path, both of which were measured to vary legitimately. It does **not** normalise
the set of quarantined identities, their recorded digests, the bytes of every
payload on disk, or the set of profiles in `config.toml`.

Because normalising is exactly how a comparand stops being able to fail, the
harness deforms a clean home three ways before any kill is allowed to count — drop
a profile section, mutate one quarantined payload byte, remove one index entry —
and requires the fingerprint to change for each. It also requires two clean runs
to agree first, and requires the reference to be non-vacuous (≥8 index entries,
≥1 profile, ≥8 payload files), so a comparison of two empty homes cannot pass.
`--no-kill` is the classifier's negative control: 5 trials, **0** classified
mid-apply, **5** complete.

### The sentinel

A tree outside every target home, digested by the product's own `backup digest`
before and after. `SENTINEL-UNCHANGED: yes` on every run of both peers, before
and after the fix — including the runs in which a kill landed mid-write, which is
when isolation is most likely to fail.

### Honest grade on SC3 — still OPEN, and here is the exact remaining clause

26-04's stated unmet clause is now closed: profile migration and reciprocal
portability HAVE been interrupted, 35 times, and the contract no longer rests on
an argument.

**But the criterion's literal text is still not met by the migration path, for a
reason the interruption exposed rather than resolved: `migrate` has no rollback.**
It does not return the home to its pre-operation state. It leaves the partial
work in place and converges on the COMPLETED state when the product is driven
again. That is a defensible contract for an import — arguably better than a
rollback, since re-running is what a user does anyway — and it is now proven.
It is not "restore exact pre-operation state on rollback".

Two further limits, stated rather than rounded off:

- **Platform: Linux only.** `backup restore` has a real Windows interruption leg
  (26-03, `TerminateProcess`). The migration path does not. The harness is POSIX
  `sh` and has no PowerShell peer.
- The 440-item corpus is synthetic. It is grounded in 26-02's measurement of the
  real install (540 skill directories), but no real peer home was interrupted.

So: **SC3 OPEN.** Backup and restore survive interruption and roll back exactly
(26-03, Linux + real Windows). Profile migration and reciprocal portability now
survive interruption and recover deterministically on re-drive (this lane,
Linux), but do not roll back, and have no Windows leg.

---

## Target 2 — F26-03's first clause

> *"consume the F23 redacted session/evidence envelope"*

### What the F23 envelope actually is

`SessionExportEnvelope`, in `crates/wcore-agent/src/session_lifecycle.rs:205`,
carrying a comment that is itself the strongest evidence the clause was intended
and never wired: *"The redacted, portable export envelope. F26-03 consumes this
shape."* It is produced by `wayland-core session export <id>`, and it deliberately
carries no transcript text, tool arguments, tool output, prompts, filesystem paths
or provider payloads — per-message provenance is digest and length only.

### Measured, not read off the source

`scripts/portability-session-envelope-probe.sh`, at `a170ee24`, with a canary
planted in a session the product itself accepts (`session list` lists it, so the
fixture is valid by the product's own standard):

| | |
|---|---|
| envelope really is produced | `session export` rc=0, **874 bytes**, names the session |
| envelope redacts | canary **ABSENT** |
| `backup create` archive | rc=0, 4 entries |
| canary in the archive | **PRESENT, in 2 files** — `payload/sessions/<id>.json` and `payload/sessions/index.json` |
| envelope in the archive | **absent** |

The canary was proven present in the session on disk before either absence was
asserted, and the archive was searched **decompressed** — grepping a gzip stream
would have reported a comforting absence.

The index hit is worth naming separately: `SessionMeta::summary` is documented as
"first user message, truncated to 80 chars", so the session index carries user
prose by design, and a portable artefact carries it even if the session files
were somehow excluded.

### Disposition: genuine, unimplemented, NOT superseded — recorded, not built

The requirement names **two different artefacts** and only one is missing.
`backup`/`restore` is a same-user round trip; an archive that redacted its own
transcripts could not restore them, so `backup create` carrying them is correct
for ITS artefact and is not a defect. What does not exist is the *portable*,
share-to-another-party corpus that substitutes the envelope for the raw session.

Decided **4-0** — codex (gpt-5.6-sol), gemini-3.1-pro, kimi K3, plus an internal
pass arguing the opposite. The dissenting argument, and why it did not survive:
*"recording it for a follow-up is exactly how this clause reached today
unnoticed."* True of the last pass, but the failure mode then was **zero record**;
it now carries a measurement, a named disposition in `REQUIREMENTS.md`, a BACKLOG
entry and a re-runnable probe. Against that, building a new export surface inside
a repair lane would ship an undesigned corpus format with an unanswered product
question — whether an envelope-only corpus is meant to be importable at all, or
only inspectable — and this program's own history says a precisely-named OPEN beats
a half-built green.

Recorded in `REQUIREMENTS.md` (Phase 26 amendment) and `BACKLOG.md`
(`F26-GAPS-03`).

---

## Gates and suite results, with the run each figure came from

| gate | result |
|---|---|
| `cargo fmt --all -- --check` (Mac, the sanctioned exception) | rc=0 |
| `cargo clippy --release -p wcore-cli` (hetzner) | rc=0, no warning from this crate |
| `cargo test --release -p wcore-cli` (hetzner) | **2119 passed, 2 failed, 5 ignored** |
| interruption proof, hermes + openclaw, post-fix | rc=0 both |
| `--no-kill` negative control | rc=0, 0 mid, 5 complete |
| envelope probe | rc=0, PROBE: COMPLETE |

**The two suite failures, characterised rather than waved past.** Neither is
this lane's, and both were checked rather than assumed:

1. `always_fails` — a fixture the product writes on purpose.
   `crates/wcore-cli/src/plugin/scaffold.rs:274` emits
   `#[test] fn always_fails() { panic!("deliberate"); }` into a scaffolded plugin,
   and a bare `cargo test -p wcore-cli` picks the scaffolded crate up from the
   target directory. `plugin_test_propagates_a_failing_suite` — the test that
   exists to observe it fail — passes.
2. `import_is_idempotent_without_overwrite` — a pre-existing coin flip, **13 of 20
   runs failed** at `a170ee24`. The assertion is correct and the product's output
   is not stable: `Config::profiles` is a `HashMap`, so two identical runs emit the
   same profile sections in different orders. The ordering was measured on the
   **base** binary at `c23a08b9`, before any change here, and this lane's only
   product change writes `migrate-quarantine/index.json` and never touches
   `config.toml`. Filed as `F26-GAPS-01` (MEDIUM) with the prescribed
   `HashMap`→`BTreeMap` fix deliberately not taken, because it is a public
   shared-type change in `wcore-config` that needs a workspace-wide check and four
   lanes were building concurrently.

A note on one trap this run walked into and out of: the first suite invocation was
`cargo test -p wcore-cli migrate`, which exited 0 having run **0 tests** in every
target — the filter matched no test NAME. The numbers above come from the unfiltered
crate run.

---

## Still open

- **SC3**, on the clause named above: `migrate` has no rollback, and the migration
  path has no Windows interruption leg.
- **F26-03 first clause**, recorded and scoped, not built.
- `F26-GAPS-01` (config ordering, MEDIUM), `F26-GAPS-02` (orphan payloads, LOW,
  with an explicitly UNMEASURED stale-merge residual), `F26-GAPS-03` — all in
  `BACKLOG.md`, all non-blocking under the standing severity policy.

## Not done

No Windows run. No macOS run. No real peer home was interrupted. `wcore-protocol`
untouched — no migration, quarantine or backup event was added to the JSON stream
protocol, so the D1 producer contract and every Desktop consumer are unaffected.
No shared-file edit: `crates/wcore-cli/src/lib.rs` and `main.rs` are untouched by
this lane.
