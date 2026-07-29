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
