---
lane: 26-import
criterion: "SC2 — Selective import/export preserves provenance and quarantines executable content"
grade-26-C2: "was PARTIAL (panel 3-0); the import half is now built and live-proven"
files-per-category-before-after: "TOTAL 4 → 1773 | skills 0 → 1730 | personas 0 → 13 | memory 0 → 24 (planted) | provenance 0 → 1 | config.toml 1 → 1 | quarantine 3 → 4"
quarantine-inert: "PASS — with a positive control proving the harness observes execution, and a matcher control licensing the placement zero"
new-finding: "F26-IMPORT-M1 (MEDIUM, open, wcore-skills) — the skills-listing budget has no fallback below its minimal mode: 1664 imported skills render 45800 chars against an 8000-char budget, a 5.7x overshoot"
fence-exposure: "ZERO — no edit to crates/wcore-cli/src/lib.rs or main.rs (0 diff lines), no .github/ change, no untracked files"
status: complete
---

# Phase 26 — the peer→Core import half

Lane `26-import`, base `861d1b1a`, branch `lane/26-import`.

Grade lane `lane/grade-26` measured Criterion 2 **PARTIAL, panel 3-0**: against a real
peer home of 13 profiles / 542 skills / 13 personas, `migrate hermes --yes` left **four
files** in the Wayland home, with persona, memory, settings, assets and data-skill
bodies at **0 files each**. `COMPETITIVE-LEDGER.md:156` states the strategic form —
both peers migrate from each other and Core is the only party with no reciprocal path.

This lane built the writer that was missing. Every number below came from an unproxied
tool (`/usr/bin/find`, `/usr/bin/grep`, `/usr/bin/git`) or from a run of the real
release binary on `hetzner-dsm`. Nothing is inherited except where labelled.

---

## 1. Files written per category — before and after

Both arms driven by **one script against one corpus**: `scripts/f26-import-proof.sh`,
BEFORE = release binary at base, AFTER = release binary at HEAD, both built on
`hetzner-dsm`. Counts taken with `/usr/bin/find -type f` over disjoint subtrees, so no
category can borrow another's number.

| files in the Wayland home | BEFORE | AFTER |
|---|---:|---:|
| `config.toml` (profiles + non-executable MCP) | 1 | 1 |
| `migrate-quarantine/**` | 3 | 4 |
| `skills/**` — **live, agent-loadable** | **0** | **1730** |
| `migrate-imported/personas/**` | **0** | **13** |
| `migrate-imported/memory/**` *(planted corpus — see below)* | **0** | **24** |
| `migrate-imported/PROVENANCE.json` | **0** | **1** |
| **TOTAL** | **4** | **1773** |

**The BEFORE arm reproduces grade-26's independent "four files" exactly**, on a
different corpus and from a different script. That agreement is what licenses the AFTER
number: the two arms measure the same thing.

Discovery, from the product's own `--json` rather than from the filesystem — two
independent instruments for one claim: skill identities **276 → 1666**, persona **0 →
13**, memory **0 → 24**.

The product's own report at HEAD:

```
Accounting: discovered=1729 imported=1726 quarantined=3 excluded=0
Content written: 1767 files — 1664 skills, 13 personas, 24 memory notes.
```

1767 + `config.toml`(1) + quarantine(4) + provenance(1) = **1773**, which is what
`find` counted independently.

### The corpus, and why it is shaped this way

Path-exact rebuild of Sean's real `~/.hermes`: **1909 real `SKILL.md` paths and 14 real
`SOUL.md` paths**, extracted read-only with `/usr/bin/find`, refilled with generated
bodies (`scripts/f26-import-corpus.sh`). **Not one byte of his install was copied or
transmitted** — his home holds `auth.json` and `.env` files, and the lane brief forbids
moving a secret value to a build host. A hand-invented tree was the other option and it
is how F26-GRADE-M2 happened: the committed hermes fixture has 540 skill directories
and zero `SKILL.md`, so it classifies 0 of 540.

Two additions are labelled rather than folded in: **2 planted executable skills** (the
containment arm) and **24 planted memory notes**. The real home has **13 `memories/`
directories and 0 notes in them**, so a real-corpus memory figure would be a structural
zero proving nothing. Every memory number in this document is from the planted set.

### Coverage closes exactly

```
corpus SKILL.md under scan roots         1732
separately identified by the importer    1666   (1664 imported + 2 quarantined)
difference                                 66
of those 66, nested inside a found skill   66   (66/66, ancestor-walk verified)
```

The 66 are sub-skills bundled *inside* another skill
(`skills/ferrox/ferrox-ns-context/skills/<sub>/`). **Their bytes do land** — `find` over
the written skills root shows **1664 `SKILL.md` at depth 2 and 66 deeper**, total 1730,
and one reads back at `skills/ferrox-ns-context/skills/graphify/SKILL.md`. They are
carried inside their parent rather than separately addressable, which is the intended
consequence of "do not descend into a skill; its subdirectories are its assets".

Full accounting: **1666 identified + 66 nested + 179 vendor-excluded = 1911 corpus =
1909 real paths + 2 planted.**

---

## 2. F26-GRADE-H1 — fixed, and the fix is proven to discriminate

**The defect.** `mod.rs:621` recorded `Outcome::Imported` for a `Classification::Data`
skill with **no write of any kind**, and `print_report` counted it in `imported=`. I
re-derived this from source at my base rather than trusting the citation: the complete
production write set in `migrate/` was `patch_global_config` plus
`QuarantineStore::admit`/`promote`. There was no writer for data content.

**The fix.** `crates/wcore-cli/src/migrate/content.rs` is the writer, and the outcome is
now recorded **from its return value**. A failed write becomes
`Outcome::Quarantined(ImportFailed)` — a named failure that still balances the
conservation invariant — never a silent success. `MigrationReport` gained
`files_written`, taken from the writer's own counter, so a run that claims imported
content while writing nothing now contradicts itself where a user can see it.

**The test the old one replaced.** `t2` asserted only
`!store.contains("skill:skills/release-notes")` (a known-negative) plus
`report.quarantined >= 2` (which proves the *siblings* were contained). Neither looks at
where a data skill would land, so **a no-op import passed it**.

The repaired `t2` carries three assertions:

- **A1 known-positive** — the skill's bytes ARE in the Wayland home, and equal the
  peer's bytes. Existence is asserted *before* any absence is claimed.
- **A2 known-negative** — it is not in quarantine (the original intent, kept).
- **A3 the-old-test-misses** — the product's own `files_written` and `skills_imported`
  are non-zero.

### The third assertion, proven by execution rather than argument

A scratch mutation reproducing the pre-fix product (report success, write nothing) was
applied on `hetzner-dsm` and both assertion sets were run against **the same mutated
build**:

```
NEW t2                      test result: FAILED. 0 passed; 1 failed; 0 ignored; 30 filtered out
  panicked at migrate_quarantine.rs:263:
  a data skill must be WRITTEN into the Wayland home, not merely counted; home held: []

OLD t2 assertions, verbatim test result: ok. 1 passed; 0 failed; 0 ignored; 31 filtered out
```

**The old assertions pass on a no-op. The new ones fail on it.** That is the difference
the repair makes, measured. The mutation was reverted and the hetzner tree verified back
to a clean `git status` before any other run.

A second discriminating test, `t2b`, drives the same public path with the live skills
destination blocked by a regular file so every write must fail, and asserts
`skills_imported == 0`, that the accounting still balances, and that the refusal is
**named** in the operator-facing notices. It blocks with a file rather than a permission
bit deliberately: the build host runs as root, where `chmod 000` is not enforced and the
"failure" leg would silently become a success leg.

Evidence: `evidence/26-import/mutation-t2.log`, `mutation-oldt2.log`.

---

## 3. F26-GRADE-M1 — widened in the same change, and it was a missing recursion

The grading lane measured containment classification at **274 of 1909** real peer
skills and required whoever wires the import to widen the roots in the same change.

Measuring the real home myself first changed the diagnosis. `SKILL.md` files occur at
depths 2–6, and the **largest single bucket is 960 items at
`profiles/<p>/skills/<group>/<skill>/`** — one level below where the scanner looked.
`scan_peer_skills` inspected only an *immediate child* of a root, which reproduces
exactly 22 + 252 = **274** from the code path. **No set of additional roots can reach a
grouped skill.** The fix is a bounded recursive walk (`MAX_SKILL_ROOT_DEPTH`, set from
the measured distribution), plus `plugins/` and per-profile `plugin-skills/` roots, plus
symlink refusal and identity dedup so one directory reachable through two roots keeps
the one-outcome-per-identity invariant.

Result on the path-exact corpus: **274 → 1666 separately identified**, with 66 more
carried inside their parents.

### What is deliberately excluded, and why

**179 of the real home's 1909 `SKILL.md` live under `hermes-agent/` and
`hermes-office/`, which are git checkouts of the peer product itself** —
`~/.hermes/hermes-agent/.git` exists, beside `cli.py`, `Dockerfile` and
`CONTRIBUTING.md`. Their `optional-skills/` is the *vendor's* shipped catalog, not the
user's setup: it arrives with the peer product and is re-obtained by installing it.
Copying it would duplicate a library the user never authored, and it is the single
easiest way to inflate an "imported" count without migrating anything of the user's.

---

## 4. Quarantine inertness — the positive control runs first

```
P0 POSITIVE-CONTROL: PASS  the harness observes execution (sentinel created by running the payload command)
N1 INERTNESS:        PASS  neither sentinel exists after a full import
N2 PAYLOAD-CONTAINED: 2 quarantined SKILL.md on disk
N3 NO-EXEC-IN-LIVE-SKILLS: 0 (expect 0)
N3-CONTROL matcher-fires-elsewhere: 4 (expect >0)
N4 PROVENANCE-LINES: 3
```

- **P0 comes first on purpose.** The payload's own command is executed and the sentinel
  appears, so the sentinel mechanism is demonstrably alive before any absence is
  claimed. A dead sentinel produces the comforting zero for free, and "imported
  executable content did not run" is the single easiest claim in this phase to pass
  without doing any work.
- **N2 rules out the empty-home reading**: the executable payloads really were imported
  and contained, so N1 is containment rather than the absence of any import.
- **N3's zero is licensed by N3-CONTROL**, where the identical `find` pattern returns 4
  under the quarantine root. The matcher fires; the placement zero is a fact.
- **Containment held while live imports went from 0 to 1730 files.** That is the pairing
  the grading lane warned about — a completeness gap converting into a containment gap —
  and it did not happen.

Provenance survives on contained items (`migrate quarantined` prints tool, path and
digest for all 3). Provenance for imported data is written to
`migrate-imported/PROVENANCE.json` via `atomic_write`, before the config patch, so an
interruption cannot leave written content with no record of where it came from.

**Persona defang, measured not asserted.** The corpus `SOUL.md` carries a forged
`<system-reminder>`. `/usr/bin/grep -c "<system-reminder>"` returns **1** on the source
and **0** on the imported copy, which reads `&lt;system-reminder>…`; the persona body
itself survived (`grep -c "You are the persona"` = 1), so the zero is a defang and not a
dropped file.

At the loader level, `t5` now asserts in **one enumeration** that the quarantined skill
is absent *and* the imported data skill `release-notes` is present — the strongest
available statement that the import is real, since it is not "a file exists" but "the
real loader will hand this skill to the agent", and it is a live positive control for
the absence beside it.

---

## 5. Interruption safety — re-run, because I added a write path

A predecessor measured **5 of 35** mid-apply kills ending unrecovered, fixed with
`atomic_write`, re-proved at **0/35**. I added a new write path, so I re-ran the
distribution at HEAD rather than assuming the fix still covered it.

| run | trials | mid-apply | recovered | verdict |
|---|---:|---:|---:|---|
| hermes, kill enabled | 21 | 18 | **21** | `PROOF: PASS` |
| openclaw, kill enabled | 21 | 19 | **21** | `PROOF: PASS` |
| hermes, `--no-kill` (negative control) | 6 | **0** | 6 | `PROOF: PASS` |

**37 mid-apply `SIGKILL`s, 0 unrecovered.** The no-kill control produces zero `mid`
trials, which is what proves the classifier can tell a mid-apply interruption from a
completed one. `DETERMINISM-CONTROL: pass` and `SENSITIVITY-CONTROL: pass (profile-drop,
payload-byte, index-entry all detected)` on every run, so the comparand has not been
normalised into uselessness. `SENTINEL-UNCHANGED: yes` on all three — the external
sentinel tree, digested by the product's own `backup digest`, is untouched.

Evidence: `evidence/26-import/interrupt-{hermes,openclaw,nokill}.log`.

---

## 6. What I deliberately chose NOT to import

Each of these is a decision with an argument, recorded in code as well as here. "We
chose not to import personas" is a fine answer if argued; silently importing nothing is
not — and two of these are *partial* refusals I want stated plainly rather than counted
as wins.

**Personas — imported as bytes, deliberately NOT activated.** Three reasons, each
sufficient. (1) There is nowhere to put them: `ProfileConfig` has no `system_prompt`
field, and Core's only system-prompt setting is the single global
`default.system_prompt`. The measured peer home has **13 profile personas**; thirteen
values do not fit one field, and picking one silently would be a guess presented as a
migration. (2) Core has already decided foreign prompt text is untrusted — the
GHSA-8r7g companion at `config.rs:3998` folds an untrusted project's `system_prompt`
through `neutralize_trust_delimiters` precisely so it cannot inject fake
`<system-reminder>` delimiters, while a *trusted* global value is used verbatim. A
peer's `SOUL.md` is the same class of content by a different route, so writing it into
the trusted slot would grant by migration what that code path denies by trust level.
(3) Silently replacing the agent's identity is the persona equivalent of running an
imported skill. So the bytes cross the machine boundary — which is what migration is —
defanged, and activation stays an explicit operator action, exactly as promotion does
for executables.

**Memory notes — imported as bytes, staged, for a structural reason.** Core's flat-file
memory is **per project** (`auto_memory_dir(cwd)`, keyed by the project root). A peer's
`profiles/<p>/memories/*.md` are scoped to a **profile**, not a project, so there is no
project to write them into without inventing one — and attaching a peer's notes to
whatever directory the migration happened to run from is worse than not writing them,
because it is permanent and wrong.

**Settings — not imported at all.** The parts of a peer configuration with a Wayland
equivalent (provider, model, base URL, MCP servers, credentials) are already imported,
as profiles. What remains — OpenClaw's `flows/`, `tasks/`, `tui/`, `workspace/`,
`identity/`, Hermes's non-`model:` keys — has no Core semantics to map onto, so
importing it means guessing. **Settings are the one content class where a wrong guess
can reduce safety**: approval mode, egress policy, sandbox posture and trust flags all
live there. They are reported in the deferred inventory and left on the peer's disk.

**Assets — not imported.** Measured against the real peer home, `profiles/*/skins/**`
holds **0 files**; no Core surface consumes a peer asset; and assets are the
highest-byte, lowest-value class in a peer tree. A skill's *own* assets are imported,
because they travel inside the skill directory and the skill is useless without them.

**The honest shape of this:** of the five categories that measured zero, **one (data
skills) is now a full live import**, **two (personas, memory) are byte-complete but
inert pending one operator action**, and **two (settings, assets) are refused with an
argument**. A reader who wants "everything is imported" should not read this as that.

---

## 7. New finding

**`F26-IMPORT-M1` (MEDIUM, open) — the skills-listing budget has no fallback below its
minimal mode.**

`wcore_skills::prompt::format_skills_within_budget` degrades in three levels: full
descriptions → truncated descriptions → **names only**. Level 3 has no further fallback
and does not drop skills, so it can exceed the budget without bound. Measured on the
imported home: **1664 skills render 45,800 characters of names-only listing against
`DEFAULT_CHAR_BUDGET = 8_000` — a 5.7x overshoot**, roughly 11k tokens in every
session's system prompt, with every skill's description dropped.

This is not a regression I introduced and it is not in my lane's files
(`crates/wcore-skills/src/prompt.rs`). It was previously **unreachable**, because no
import path had ever put a realistic number of skills on disk. MEDIUM and non-blocking
per the severity policy → BACKLOG. Worth saying plainly: **making import work is what
made this reachable**, and a user migrating a real peer install will hit it on their
first session.

---

## 8. Gates

| gate | result |
|---|---|
| `cargo test -p wcore-cli --test migrate_quarantine` | **31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** |
| `cargo test -p wcore-cli --test migrate_hermes` | **7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** |
| `cargo clippy -p wcore-cli --all-targets -- -D warnings` | **0 errors.** 1 warning, pre-existing at base (`imap-proto` future-incompat, a dependency) |
| `cargo fmt --all -- --check` | clean |
| mutation: new `t2` vs no-op product | **FAILED** (as required) |
| mutation: old `t2` assertions vs no-op product | **ok, 1 passed** (as required) |

The executed counts are read back explicitly, including `0 ignored` and `0 filtered
out`, from a direct `ssh` rather than through `rtk` — which strips exactly those two
fields.

---

## 9. Fence exposure vs `861d1b1a`

```
$ /usr/bin/git diff --name-status 861d1b1a HEAD
A  .planning/phases/26-migration-export-backup-restore/26-IMPORT-NOTES.md
A  crates/wcore-cli/src/migrate/content.rs
M  crates/wcore-cli/src/migrate/mod.rs
M  crates/wcore-cli/src/migrate/quarantine.rs
M  crates/wcore-cli/tests/migrate_quarantine.rs
A  scripts/f26-import-corpus.sh
A  scripts/f26-import-proof.sh

$ /usr/bin/git diff 861d1b1a HEAD -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs | wc -l
0
$ /usr/bin/git diff --name-only 861d1b1a HEAD -- .github/ | wc -l
0
$ /usr/bin/git status --porcelain | grep '^??' | wc -l
0
```

**Shared fence line delta: 0.** No `.github/workflows` change, no
`wcore-contract generate`, no merge, no PR, no tag, no release, no issue closed.

Sean's real peer installs were **read only**. Only path names left the Mac — 1909
`SKILL.md` paths, 14 `SOUL.md` paths, 12 profile names — never file contents, never a
credential. No file under `/Users/seandonahoe/dev/resources/` was touched.

---

## 10. Honest verdict

**Criterion 2's import half is built and live-proven, and the criterion is not fully
met.**

What is met: provenance is preserved on both imported and contained items; executable
content is quarantined and stays inert, with a positive control proving the harness
could have seen it run; selective import still refuses an unpublished identity; and the
import now writes **1773 files where it wrote 4**, with data skills landing live and
loadable.

What is not: personas and memory are byte-complete but inert; settings and assets are
refused by argument, not imported; 66 nested sub-skills are carried inside their parents
rather than separately addressable; and `migrate` still has **no rollback** (G3 in the
grade lane's gap list, untouched here) — an interrupted migration converges forward on
re-drive rather than restoring the pre-operation home. The ledger's `PORT-*` answer
should now read: **Core has a reciprocal path for discovery, dry-run, profiles, MCP,
credentials and skills; personas and memory arrive inert; settings and assets do not
arrive.**
