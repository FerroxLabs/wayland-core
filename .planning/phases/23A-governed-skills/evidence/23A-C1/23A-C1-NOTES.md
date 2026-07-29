# 23A-C1 NOTES — governed skill promotion / revocation / rollback

Lane `lane/23a-c1`, branched from `plan/f20-unified-audit-repair` @ `8bcb052b`.
Append-only. Re-committed after every measurement (LANE-BRIEF §6b-i).

---

## 0. Instrument defect found and repaired IN-LANE (§6b-ii)

**Defect.** The Bash tool in this environment routes `grep` through an `rtk` hook. When a
`grep` invocation's stdout is **not** piped, the hook replaces the real output with a
compressed rendering (`N matches in M files:` followed by truncated, re-ordered lines) and
**misattributes the file count**. Measured directly:

- `grep -rn "global" crates/wcore-skills/src/paths.rs` (one file, unpiped, in a compound
  command) rendered as `9 matches in 7 files:` — seven files, from a one-file grep.
- The block also appeared **after** a later `echo` in the same compound command, i.e. output
  ordering is not preserved.
- The true answer is **0 matches** — confirmed with `rtk proxy grep ... | wc -c` → `0` bytes.

This is the same class the brief warns about: an instrument that silently destroys a result
rather than failing loudly. A "grep returns nothing" conclusion drawn through the hook is
worthless, and "grep for a revoke surface returns zero" is *precisely* the measurement this
lane's brief rests on.

**Repair (applied, not merely noted).** Every evidence-grade grep in this lane runs as
`rtk proxy grep ... | cat`. `rtk proxy` bypasses the hook; the pipe keeps the raw stream.

**Self-test, three assertions** — see `harness-selftest.sh` in this directory.
1. known-positive: a pattern that exists is found (raw path reports it);
2. known-negative: a pattern that does not exist reports 0 matches;
3. **the old broken instrument would have missed it**: the unpiped hooked form on the
   same corpus returns a byte-count/label that differs from the raw form. Assertion 3 is the
   only one that fails on the *unrepaired* instrument, so it is the one that proves the repair.

---

## 1. What the criterion says

`23A-C1` (ROADMAP): a drafted skill can be **observed**, **promoted**, **revoked**, and
**rolled back**, and **cannot execute before governed promotion**.

Prior grading (`CRITERIA-GAP-LEDGER.md:190-219`): NOT MET, three of four clauses unimplemented.

---

## 2. Measurements at `8bcb052b` (all via repaired instrument)

### 2.1 The write into the user's GLOBAL directory is real and unconditional

`crates/wcore-agent/src/auto_skill/drafter.rs:86-116`, `SkillDrafter::draft()`:

```
let loader_dir = self.loader_root.clone()
    .or_else(wcore_config::config::app_config_dir)
    .map(|d| d.join("skills").join(&name))
    .unwrap_or_else(|| self.skill_dir.join(&name));
std::fs::create_dir_all(&loader_dir)?;
... atomic_write(manifest.json) ; atomic_write(SKILL.md)
```

`loader_root` is `#[cfg(test)]`-only (drafter.rs:65-76). **In production it is always `None`**,
so the target is always `app_config_dir()/skills/auto-<sig>/` — i.e. `user_skills_dir()`
(`wcore-skills/src/paths.rs:12`), the user's *global* skills directory, NOT a project dir.
Triggered by the bucketer after N successful turns. **No user action is involved at any point.**

There is a second, `$WAYLAND_HOME`-rooted family (`paths.rs:38`, `~/.wayland/skills/auto`)
that the loader also reads, documented as the drafter's "legacy/secondary write target".

### 2.2 `revoke` / `rollback`: confirmed absent — and the brief's grep is CORRECT

Re-measured through the repaired instrument (the unrepaired one is not admissible):
`rtk proxy grep -rni "revoke\|rollback" crates/wcore-skills/src/ | cat` → **0 bytes, 0 lines.**
So the brief's claim survives instrument repair. Recorded because a zero measured through a
lossy instrument would otherwise have been a tautology.

### 2.3 `ProcedureStatus` has no `Revoked` variant — confirmed

`crates/wcore-memory/src/v2_types.rs:357-364`: `Staged | Active | Archived | Pinned`.
Transitions (`can_transition_to`, :380): Staged→{Active,Archived}, Active→{Archived,Pinned},
Pinned→{Active,Archived}. `Archived` is a *curation* state, reached from Staged/Active, and is
not a revocation: it carries no record that the thing was ever promoted, no prior state to
return to, and nothing binds it to the on-disk artifact.

### 2.4 No generation store, no provenance binding

`Procedure` (v2_types.rs:338-355) has `artifact: String` and no prior-version field, no
content hash, no link to the on-disk `SKILL.md` path. The DB row and the file in the user's
global directory are **two unrelated objects**. Nothing can roll back to a prior state
because no prior state is retained.

### 2.5 The promote path is a `bail!` — and the flag is already hidden

`crates/wcore-cli/src/main.rs:2516` `run_skills_promote` → unconditional `bail!`.
Flag declared at `main.rs:463-475` and **already carries `hide = true`** with a long comment
recording that this closes the *advertisement* only, not the criterion.
**`main.rs` is FENCED for this lane** — read-only. Any change there is a seam request.

### 2.6 The one clause that is NOT vacuous — quarantine exists

`crates/wcore-skills/src/loader.rs:443-449`: a draft classified by `is_generated_draft()`
(manifest `auto_drafted=true`, else the released body marker) gets
`metadata.disable_model_invocation = true`. So "cannot execute before governed promotion"
has a **real mechanism** for the model-facing surface, not only the vacuous one the ledger
attributes to the `bail!`. Prior grading understates this.

**OPEN QUESTION (must measure next):** does `disable_model_invocation` also block the
**user-invocable** path (slash command)? If not, a drafted skill in the user's global
directory is user-executable with no promotion, which would be a *worse* finding than the
ledger's, and would land against the non-negotiable "nothing is promoted without an explicit
user action".

---

## 3. Position (provisional, pre-cross-audit)

The brief's ordering — reversibility before capability — is correct, and the measurements
sharpen why: the product **mutates a directory the user owns**, with no user action, and
offers no product surface to undo it. That is the customer-visible harm. Promotion is a
missing *feature*; unrevocable auto-mutation of `~/.config/.../skills/` is a *defect*.

Therefore the intended slice, in this order:
1. revocation + rollback for what is already being written (the defect);
2. only then, governed promotion (the feature), if budget remains.

Design to be cross-audited (§4) before implementation.

---

## 4. Still to establish

- [ ] does quarantine cover the user-invocable/slash surface, or only model invocation?
- [ ] what exactly is in a user's global skills dir after a real drafting run (live drive)
- [ ] where revocation state must live so it survives the file being re-drafted
- [ ] whether `Archived` can be reused or a `Revoked` variant is required (serde wire impact:
      `ProcedureStatus` is `Serialize`/`Deserialize`, so a new variant is a compat question)

---

## 5. Self-test mutation result (gates must be able to fail — §3.2)

`env PATH=/usr/bin:/bin bash harness-selftest.sh` (rtk removed = unrepaired instrument):

```
A1 FAIL known-positive: got '0' lines, expected 40
A2 PASS known-negative: 0 bytes (expected 0)
A3 FAIL: rtk not on PATH -- raw() silently degrades to the hooked instrument
SELFTEST: FAILED -- grep NOT admissible as evidence
EXIT=1
```

Unmutated run: A1/A2/A3 all PASS, `EXIT=0`. So the gate **can** fail.

**Additional finding from the mutation — recorded because it is the same defect class.**
Under mutation **A2 still PASSED**, because a broken instrument that returns nothing trivially
satisfies a "expect 0 bytes" assertion. **A known-negative assertion is self-passing on a dead
instrument.** It is worthless alone. Only A1 (known-positive) and A3 (repair-is-real) discriminate.
This is the reason §2.2's "revoke/rollback grep returns zero" was re-measured through the repaired
instrument rather than trusted: a zero is exactly the answer a broken grep gives for free.

---

## 6. Resolving the open questions from §4, and the panel's Q4

### 6.1 Quarantine DOES cover the user-invocable surface (open question closed)

`crates/wcore-agent/src/slash/skill.rs:115-119`:
```
Some(skill) if skill.disable_model_invocation => Ok(SlashOutcome::Handled {
    output: Some(format!("/skill run '{name}': this skill is quarantined and cannot be run.")),
```
plus `engine.rs:5351` `if skill.disable_model_invocation`. So **both** the model path and the
user slash path refuse a quarantined draft. The feared worse-finding does NOT exist.

**Consequence: the criterion's "cannot execute before governed promotion" clause is genuinely
MET, by a real mechanism, not vacuously by the `bail!`.** The prior ledger grading is wrong on
this clause in the product's favour. Recorded because honest grading cuts both ways.

### 6.2 `skills_lifecycle` DEFAULTS ON — this is default-on behaviour

`wcore-config/src/config.rs:605` "defaults ON so the learn-and-evolve loop is on by default";
`:688` / `:696` `self.skills_lifecycle.unwrap_or(true)`; test
`observability_skills_lifecycle_defaults_true` (:7233) asserts it for **both** the TOML-omitted
path **and** the struct-`Default` path, the latter explicitly labelled "no-config first run".

So on a fresh install with no config file, the product will write into the user's global skills
directory with no user action. This is what makes the missing revocation customer-visible rather
than theoretical.

### 6.3 The three stores holding an auto-draft, and which can resurrect it

| # | Store | Written by | Live resurrection path? |
|---|-------|-----------|-------------------------|
| a | `<config>/wayland-core/skills/auto-<sig>/{SKILL.md,manifest.json}` | `SkillDrafter::draft` (drafter.rs:100-116) | **YES** — the bucketer calls `draft()` again |
| b | `evolved_prompts` row, `scorer="auto_drafter"` | `SkillDrafter::draft` (drafter.rs:135) | **NO** — see below |
| c | `Procedure` row, `Tier::Project`, `status=Staged` | `DraftWriter::stage` via `engine.rs:5077` | **NO** — see below |

**(b) is not live, and the reason is a latent hazard worth recording.** Hydration is
`bootstrap.rs:2145` `store.seed_pairs_for(&candidate_names, "auto_drafter", 1)`, where
`candidate_names` is `catalog.visible()` (`bootstrap.rs:2119`), and `visible()` is
`refs.rs:128-130` `filter(|r| !r.disable_model_invocation)`. Auto-drafts are quarantined, so they
are **never in `visible()`** and Layer 1b can never match one. The comment at `bootstrap.rs:2100-2109`
describes this as "closes the U6 read-back… the closed-loop weight was written but never read" —
**the F06 quarantine silently re-broke exactly the loop that comment says it closed.** Not this
lane's defect and not a customer symptom (it makes the product safer, not worse), so it goes to
BACKLOG as MEDIUM. **But it is a live hazard for governed promotion specifically:** the moment
promotion lifts `disable_model_invocation`, Layer 1b starts firing off a retained DB row that no
file-level revocation touches. Any future promotion work must handle (b) or it will resurrect
revoked content.

**(c) is inert.** Production readers of `Procedure` rows are `curate.rs:66` (curator),
`slash/memory.rs:378` (`/memory` display), and `main.rs:2580/2685` (the promote/archive CLI).
**No path materialises a `Procedure` into an on-disk skill or executes one.**

### 6.4 Verdict on the panel's Q4 — I take the minority

2/3 of the panel ranked "the unbound `Procedure` DB row" as the most likely failure. **Measured,
that ranking is wrong**: (c) cannot resurrect or execute, and (b) is gated shut by the quarantine.
The panel asserted this without the source, and I queued it for measurement rather than designing
around it — which is why this is caught.

**I take codex's minority ranking: crash/race non-atomicity is the operative risk**, because it is
the one that can actually produce a wrong outcome in this design. Its ordering fix is adopted:

> snapshot bytes → **tombstone durable** → journal append → remove directory

A crash then leaves either "nothing happened" or "tombstone present, directory still there" — the
latter is safe and idempotently completed by a re-run. The rejected order (remove-then-tombstone)
has a window where the draft is gone with no tombstone, so the next trigger silently re-creates it:
revocation violated precisely by the failure mode governance exists to handle.

The DB rows are still addressed, but as a **coherence** matter, not a resurrection one, and their
exact status is stated in the SUMMARY rather than silently left out.

---

## 7. Design as built (post-audit)

- Module `wcore-skills::govern`. **No new dependencies.**
- One governance root, **out of the config dir** (panel 3/3):
  `<data_dir>/wayland-core/skills-governance/{journal.jsonl, tombstones/, generations/<id>/}`
- Dual key (adversarial amendment): tombstone records **both** signature (from `manifest.json`)
  and name; `is_revoked` matches on either, so drafts with a damaged manifest are still covered.
- `revoke(name)` → snapshot → tombstone → journal → remove. `rollback(id)` → restore bytes →
  clear tombstone → journal. `is_revoked()` → consulted by `SkillDrafter::draft`.
- Surface: a crate binary (`wcore-eval`/`wcore-evolve`/`wcore-contract` establish the pattern),
  because `wcore-cli/src/main.rs` is fenced. Seam request covers the eventual flag.
