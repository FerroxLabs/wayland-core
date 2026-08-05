# 23A-C1 — cross-audit of the revocation/rollback design (LANE-BRIEF §4)

Panel: codex `gpt-5.6-sol`, gemini `gemini-3.1-pro-preview`, kimi K3, plus one internal
adversarial pass arguing AGAINST the emerging consensus. Prompt: `panel-prompt.txt` (committed
beside this file). All three returned a real answer to a real 5-question design prompt — not a
one-word probe (§4 warns a one-word probe passes despite a broken invocation).

Vote extraction was **unanimous, 3/3 present** — no dropped votes. `PANEL_POSITION=` was
extracted unanchored, per §4 (kimi indents and bullet-prefixes, which an anchored `^` regex loses).

## Verbatim PANEL_POSITION lines

**codex:**
> PANEL_POSITION=Ship signature-keyed, crash-safe revocation and exact rollback first, store
> recovery generations in per-user application data rather than active config, and defer
> promotion entirely.

**gemini:**
> PANEL_POSITION=The tombstone design is structurally sound but must key on signatures and
> store rollbacks in OS data directories to prevent infinite re-draft loops and further config
> pollution.

**kimi:**
> PANEL_POSITION=Approve the tombstone-based, signature-keyed revoke/rollback design with
> snapshots relocated to a data dir, but gate everything on binding or revoking the unbound
> Procedure DB row, because as designed you risk building perfect reversibility for the file
> while the actual capability escapes through the database path.

## Tally

| Q | codex | gemini | kimi | outcome |
|---|-------|--------|------|---------|
| Q1 tombstone vs plain delete | tombstone | tombstone | tombstone | **3/3 tombstone** |
| Q2 key on name / signature / hash | signature | signature | signature | **3/3 signature** |
| Q3 snapshots in config dir? | no → data dir | no → data dir | no → split, generations to data dir | **3/3 out of config** |
| Q4 most likely failure | crash/race non-atomicity | DB `Procedure` row desync | DB row bypass (1st), crash-order (2nd) | **2/3 DB row, 1/3 atomicity** |
| Q5 scope | revoke+rollback only | revoke+rollback only | revoke+rollback only | **3/3 revoke+rollback only** |

## Accepted, with the reasons

**Q1 — tombstone. Accepted 3/3.** A delete-only revoke is overridden by the next drafting
trigger, which is designed to recur. All three independently named this: revocation is a durable
user intent, not a filesystem operation. Two added a requirement I had not planned and am
adopting: **an un-revoke / clear path must ship in the same change**, or the tombstone becomes
the new irreversible mutation. `rollback` serves this — it restores bytes *and* clears the
tombstone.

**Q2 — key on signature. Accepted 3/3, with one amendment from the internal adversarial pass.**
Today `name == format!("auto-{}", signature)` (drafter.rs:87), so the two are currently
equivalent and the panel's case is about future-proofing. But the adversarial pass found a case
the panel missed: `loader.rs:463` already handles drafts whose **`manifest.json` is missing or
damaged**, falling back to a body-marker classifier. For exactly those artifacts the signature is
unrecoverable, so signature-only keying fails on the damaged subset. **Amendment: record both,
match on either.** Strictly dominates either key alone, costs one extra field.

**Q3 — governance store OUT of the config dir. Accepted 3/3 over my own objection.** My
adversarial case was simplicity: two roots means the product writes to *two* places on the user's
machine, which is a version of the original complaint, and rollback then depends on two
directories staying consistent. The panel's counter-evidence is stronger and concrete: config
dirs are dotfile-tracked, synced and backed up; retained snapshot bytes are *data*, not config;
and unbounded growth inside a config dir is a disk leak in the worst possible location. I take
the majority. **Resolution that also answers my objection: ONE governance root, under the data
dir** — `<data_dir>/wayland-core/skills-governance/` — not a config/data split. codex and gemini
endorse this shape directly; kimi preferred splitting journal from generations, and I take the
2/3 majority for a single root because one root is one thing for a user to inspect or delete.
**Bounding is required** (all three raised unbounded growth); recorded as a follow-up if budget
does not reach it.

**Q4 — the resurrection path. Majority accepted in DIRECTION, but the panel names the wrong
database, and I am not designing around an unverified claim.** 2/3 said the unbound
`Procedure` row is the top risk. The panel did not have the source. Measured next (§ NOTES 2.7)
before any of it is built; the ordering fix from codex+kimi (make the tombstone durable BEFORE
the removal, so a crash cannot leave a deletion with no tombstone) is accepted unconditionally
because it is correct regardless of which DB is involved.

**Q5 — revocation + rollback only. Accepted 3/3.** Matches the lane brief's own instruction and
my pre-audit position, so this is agreement rather than new information — recorded as such rather
than as corroboration.

## Dissent recorded

- **kimi dissents on Q3's shape**: journal/tombstones are hot-path behavioural state and should
  stay near the skills they govern, with only the bulk generation bytes relocated. I overruled it
  2/3 for a single root. If the tombstone read ever shows up as a startup cost, kimi's split is
  the fix and this note is the pointer.
- **codex dissents on Q4's ranking**: it ranks crash/race atomicity above the DB-row concern.
  Given the measurement in NOTES §2.7, codex's ranking may prove to be the correct one; its
  ordering fix is adopted either way.
- **The internal adversarial pass dissents on Q2** as recorded above, and won an amendment.
