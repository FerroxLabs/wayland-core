---
lane: 26-sc2-import
criterion: "SC2 — Selective import/export preserves provenance and quarantines executable content"
was: "PARTIAL (26-PHASE-VERDICT.md, panel 3-0)"
now: "provenance half CLOSED; quarantine half PROVEN with a discriminating known-negative; criterion still PARTIAL on completeness"
base: "d5b462d5 (gh/plan/f20-unified-audit-repair, post-train)"
head: see git
new-finding: "F26-SC2-M1 (MEDIUM, mitigated) — 68 of 349 real peer skills ship a .sh/.py/.js helper; classify_skill_body reads only SKILL.md prose, so all 68 import live. Mitigated by explicit exec-bit removal + operator disclosure, NOT by quarantine, and the reason is argued below."
instrument-defect-found-and-repaired: "this script's own M2 compile check could never pass — repaired in-lane with a three-assertion self-test"
fence-exposure: "ZERO — 0 diff lines in crates/wcore-cli/src/lib.rs and main.rs, no .github/, no wcore-contract generate"
status: complete
---

# Phase 26 SC2 — the import half's provenance, and a containment claim that can fail

Lane `26-sc2-import`. Every number below came from an unproxied tool
(`/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/find`, a direct `ssh` to
`hetzner-dsm`) or from a run of the real release binary. Nothing is inherited
except where labelled.

---

## 0. The brief's premise, checked before building

The brief said a prior lane had done the import work and *"that work is on a
branch that may already be merged into your base."* Measured first:

```
$ /usr/bin/git merge-base --is-ancestor lane/26-import HEAD   → NO
$ /usr/bin/git merge-base HEAD lane/26-import                 → 861d1b1a
```

`lane/26-import` and my base `lane/grade-26` were **siblings off `861d1b1a`** —
the writer existed, and `crates/wcore-cli/src/migrate/content.rs` did not exist
in my tree. I merged it rather than rebuilding it, then took the orchestrator's
correction and merged the train (`gh/plan/f20-unified-audit-repair` @
`d5b462d5`). The train contains `lane/26-import` exactly, and **no other lane
has touched `crates/wcore-cli/src/migrate/`** since `861d1b1a` — verified before
merging, which is why the merge was conflict-free.

Every gate below was re-run at the post-train HEAD, not only at the pre-merge one.

---

## 1. Half one — provenance preserved. **Was NO for 1767 of 1773 files. Now YES.**

### What I measured

`ProvenanceDocument` recorded `source_tool`, `source_version`, `source_path`,
`digest`, `imported_at` — **and no destination**. `QuarantineEntry` carried
`stored_path` beside its `Provenance`, so a *contained* item was traceable both
ways; an *imported* one was not. And there was **no read-back surface at all**:
`migrate quarantined` exists; nothing answered for imported content.

So the brief's question — *"after import, can you still tell where each artifact
came from"* — was **NO for everything that landed live or staged.** That is not
a cosmetic gap: an imported skill lands under a `sanitize_component`-ed name,
digest-disambiguated on collision, and a real peer install reuses skill names
across profiles, so the mapping is not recoverable by inspection.

### What landed

- `Provenance` gains `written_path` (home-relative, `/`-separated) and
  `deduplicated_with`, **set from the write that just happened** — the same
  discipline F26-GRADE-H1 forced on the outcome. Both producers now speak one
  vocabulary: `QuarantineStore::admit` records
  `migrate-quarantine/payloads/<n>`, the content writer records
  `skills/<n>` or `migrate-imported/…`.
- `ProvenanceDocument::resolve_path` — the reverse lookup, matching **by path
  component**, so `skills/notes` does not cover `skills/notes-2`. That boundary
  is not hypothetical: a real import writes a base name and its
  digest-disambiguated sibling next to each other.
- `ProvenanceDocument::without_destination()` — the F26-GRADE-H1 shape one level
  up, made checkable: a record claiming an import without saying where it is.
- `migrate imported [--path P] [--json]` — the surface. Answers for live,
  staged and contained content through one lookup, because reading only one
  store would answer "where did this come from?" with a confident *nowhere* for
  half the artifacts on disk.

### Gates

```
before my change : test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
after  (t23,t24) : test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
post-train + t25 : test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
cargo clippy -p wcore-cli --all-targets -- -D warnings : 0 errors
  (1 warning, pre-existing at base: imap-proto future-incompat, a dependency)
cargo fmt --all -- --check : clean
```

`t23` carries four assertions, and the fourth is the only one that makes the
first three worth anything: the **identical query**, run against the same
document with destinations stripped — byte-for-byte the record shape that
shipped before this change — returns **empty**. Without it, `t23` would pass
just as well on a document that recorded nothing new.

---

## 2. Half two — executable content quarantined. **The claim can now fail.**

The verdict's warning was explicit: this phase already produced one assertion
that could not fail, and *"if your test would pass against a build with
quarantine ripped out, it proves nothing."*

`scripts/f26-quarantine-known-negative.sh` rips it out and requires red.
Post-train run, real rc read from an **unpiped** invocation:

```
M0 baseline   : test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
M1 APPLIED    : yes (8 changed lines)          classify_skill_body always returns Data
M2 COMPILED   : yes
M3            : test result: FAILED. 24 passed; 10 failed; 0 ignored; 0 measured; 0 filtered out
M3 REQUIRED-RED t5_quarantined_content_is_absent_from_what_the_agent_would_load : FAILED
M3 REQUIRED-RED t19_live_negative_leg_quarantined_payload_does_not_execute      : FAILED
M6 APPLIED    : yes (11 changed lines)         write_tree preserves the source execute bit
M7 COMPILED   : yes
M8            : test result: FAILED. 33 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
M8 REQUIRED-RED t25_an_imported_peer_script_arrives_without_its_execute_bit     : FAILED
M4 RESTORED   : yes (both files byte-identical to their pre-mutation state)
M5            : test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
KNOWN-NEGATIVE: PASS      KN-RC=0
```

**10 of 34 tests go red when containment is removed**, including both security
assertions — `t5` asks the real agent-facing enumeration what it would load,
`t19` drives the real binary and looks for the payload's side effect on disk.
**M5 is what makes the red attributable**: the tree returns to green at the same
commit.

Mutation 2 is the sharper one and I added it because I caught myself: `t25`'s
mode assertion was **not** self-evidently discriminating. `fs::write` yields
`0644` on a new path regardless of the source mode, so simply *deleting* the
exec-bit guard would have left `t25` green and the guard would have read as
load-bearing while doing nothing. The realistic regression is a copy-based
`write_tree`, which mutation 2 simulates — and `t25` is the **only** test that
goes red under it, which is exactly the discrimination wanted.

### Live proof, real release binary, `wayland-core 0.12.25` on `hetzner-dsm`

`scripts/f26-sc2-live-proof.sh`, `LIVE-RC=0`, full transcript at
`evidence/26-sc2/sc2-live-proof.log`:

```
PASS: P0 positive control — the sentinel mechanism observes execution
PASS: N2 the executable skill's bytes ARE contained (so N1 is containment, not an empty home)
PASS: N1 the contained payload did not execute (licensed by P0 and N2)
N3 directive-carrying skills in the LIVE root: 0 (expect 0)
N3-CONTROL same matcher under quarantine:      1 (expect >0)
PASS: X1 the helper's BYTES crossed (a migration, not a filter)
PASS: X2 the imported helper carries no execute bit (644)
PASS: X2-CONTROL the source helper IS executable, so X2 measures a change
PASS: Q1 known-positive — a live imported artifact resolves to its peer source
PASS: Q2 known-negative — a locally authored skill is NOT attributed to a peer
PASS: Q3 contained content is locatable through the same surface as live content
PASS: Q4 the staged persona is locatable too
PASS: Q5 every record names where its bytes are
PASS: S1 the source tree is byte-identical after the import
SC2 LIVE PROOF: PASS
```

Every absence carries a positive control **taken in the same invocation**: P0
runs the payload's own command by hand before any "did not execute" is claimed;
N3's zero is licensed by N3-CONTROL, where the identical matcher returns 1
under the quarantine root; X2's "not executable" is licensed by X2-CONTROL
proving the source was.

---

## 3. New finding — F26-SC2-M1 (MEDIUM, mitigated, disclosed)

**Measured read-only across the four peer trees under
`/Users/seandonahoe/dev/resources/` (`hermes-agent`, `openclaw`, `grok-build`,
`gemini-cli`), node_modules excluded, with a positive control and a negative
control on every matcher:**

```
SKILL.md files                                          : 349
carrying Wayland's ```! shell directive                 :   0
carrying a .sh/.py/.js/.mjs helper or an exec-bit file  :  68   (19.5%)
positive control (files matching "name")                : 420/421
negative control (a token that cannot exist)            :   0
```

`classify_skill_body` reads the SKILL.md **prose** and looks for Wayland's own
directive. **Zero real peer skills use that syntax** — it is Wayland's — so the
classifier's answer for essentially every real peer skill is `Data`, and all 68
script-carrying skills import **live**.

**I am not calling this a containment breach, and here is why the distinction
holds.** Wayland's only auto-execution-on-load surface *is* the directive, and
that is classified and contained — proven above by mutation. A peer's
`install.sh` is not run by the loader. It is one `./install.sh` away from
running, though, and that containment decision was never made.

**Why not just quarantine all 68.** The quarantine ceiling is 512 items /
32 MiB, mirrored from `workspace_trust` and expressly not to be raised for a
migration. On Sean's real 1730-skill install, ~20% carrying scripts would blow
that ceiling and produce mass refusals — which converts a completeness win back
into *"safe because almost nothing is imported"*, the vacuity the grading lane
named at F26-GRADE-M1. So the proportionate control is:

- the **bytes cross** (this is a migration, not a filter);
- the **execute bit does not**, set explicitly rather than left to `fs::write`'s
  incidental `0644` — an accident is not a control: it depends on the umask, it
  does not hold over an existing target, and nothing announced it;
- the **count is surfaced to the operator** in the report, with the measurement
  that motivates it.

Live output from the real binary:

```
  1 imported file carried an execute bit; it was REMOVED. Measured against the
  real peer trees, 68 of 349 peer skills ship a .sh/.py/.js helper, and a skill is
  classified on its SKILL.md prose — so those helpers import live. They arrive inert:
  running one is an explicit act (`sh <script>`), which goes through tool approval.
```

**What I did NOT do:** widen classification to inspect a skill's file payload.
That is the fuller answer and it needs a ceiling design that does not reintroduce
mass refusal. Logged, not done.

---

## 4. My own instrument was broken, and I repaired it rather than noting it

The first run of the known-negative script printed **`M2 COMPILED: no`** against
a mutant that had plainly compiled and run 33 tests. Cause: `grep -qE '^error'`
over the cargo log, and `cargo test` prints
`error: test failed, to rerun pass …` whenever **any** test fails — the exact
condition the script exists to produce. **The check was structurally incapable
of passing on a successful run.**

Per §6b-ii this was repaired in-lane, not written up: `compiled_ok()` now reads
a positive signal (`running N tests`, printed only after a successful build) and
excludes cargo's post-run summary by name. `--self-test` carries three
assertions, and the third is the one that proves the repair does anything:

```
SELF-TEST A1 known-positive        : PASS (a compiled-then-failed log reads as compiled)
SELF-TEST A2 known-negative        : PASS (a real rustc error reads as not compiled)
SELF-TEST A3 old-matcher-misses-it : PASS (the OLD matcher calls A1 'did not compile')
SELF-TEST: PASS
```

I also caught the pipe-steals-exit-status trap on myself in the same run:
`… | tail -40; echo "SCRIPT-RC=$?"` reported **0** for a script that exited **1**.
Every rc in this document comes from an unpiped invocation.

---

## 5. Does peer→Wayland import work now, and from which peers?

**From `hermes` and `openclaw`: yes, for profiles, MCP, credentials (opt-in),
data skills (live), personas and memory notes (staged), with provenance on every
one and executable content contained.**

**From `grok-build` and `gemini-cli`: no — there is no importer at all.**
Stated with its query so it can be re-run, and with a positive control on the
same matcher:

```
$ /usr/bin/grep -rniE "grok|gemini" crates/wcore-cli/src/migrate/    → 0 lines
$ /usr/bin/grep -rniE "hermes|openclaw" crates/wcore-cli/src/migrate/ → 150 lines
$ PeerSource enum (crates/wcore-config/src/portability/mod.rs:45)     → Hermes, OpenClaw
```

`PeerSource` has two variants. Adding a third is a discovery/mapping job per
peer; nothing in the containment, provenance or selection machinery is
peer-specific, so the expensive half is already built.

---

## 6. Honest verdict on Criterion 2

> *Selective import/export preserves provenance and quarantines executable content.*

**Both named properties are now met and both can fail.** Provenance is preserved
*and readable back*, live/staged/contained, with a known-negative and a
proven-blind legacy shape. Executable content is quarantined, inert, live-proven
against the real binary, and the claim goes red under two independent mutations.

**The criterion as a whole I still grade PARTIAL**, on completeness rather than
on either named property:

- **peer coverage is 2 of 4** — no importer for `grok-build` or `gemini-cli`;
- **personas and memory arrive inert**, byte-complete but pending one operator
  action (argued in `26-IMPORT.md` §6, and I agree with the argument);
- **settings and assets are refused by argument**, not imported;
- **skill classification does not inspect the file payload** (F26-SC2-M1),
  mitigated rather than resolved;
- **F26-GRADE-M2 / G7 is still open** — the committed hermes fixture has 540
  skill directories and **zero `SKILL.md`** (re-measured myself with
  `/usr/bin/find`: `566` dirs, `0` SKILL.md), so `t13`'s at-scale conservation
  still classifies 0 of 540. `26-import` built a *generated* path-exact corpus
  instead of fixing the committed one, so the repo's own fixture still cannot
  exercise classification. **I did not close this** — I chose the newly-measured
  executable-payload gap over fixture fidelity and I am naming the trade rather
  than burying it.

**I did not touch backup, restore or rollback** — lane `26-sc3-rollback` owns
SC3, and `migrate` still has no rollback (G3, untouched here).

---

## 7. Fence exposure vs the train (`d5b462d5`)

```
$ /usr/bin/git diff d5b462d5 HEAD -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs | wc -l
0
$ /usr/bin/git diff --name-status d5b462d5 HEAD
A  .planning/phases/26-migration-export-backup-restore/evidence/26-sc2/26-SC2-NOTES.md
A  .planning/phases/26-migration-export-backup-restore/evidence/26-sc2/quarantine-known-negative.log
A  .planning/phases/26-migration-export-backup-restore/evidence/26-sc2/sc2-live-proof.log
M  crates/wcore-cli/src/migrate/content.rs
M  crates/wcore-cli/src/migrate/mod.rs
M  crates/wcore-cli/src/migrate/provenance.rs
M  crates/wcore-cli/src/migrate/quarantine.rs
M  crates/wcore-cli/tests/migrate_quarantine.rs
A  scripts/f26-quarantine-known-negative.sh
A  scripts/f26-sc2-live-proof.sh
```

`MigrateCmd::Imported` is declared inside `migrate/mod.rs`, so the new
subcommand cost **zero** shared-fence lines. No `.github/` change, no
`wcore-contract generate`, no merge to `plan/f20-unified-audit-repair`, no PR,
no tag, no release, no issue closed.

`/Users/seandonahoe/dev/resources/` was **read only** — `find` and `grep`
exclusively, nothing executed, nothing written. No credential was used, read or
transmitted; no live provider run was needed for this criterion.
