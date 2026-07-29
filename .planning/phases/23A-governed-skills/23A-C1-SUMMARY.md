# 23A-C1 — SUMMARY

**Lane:** `lane/23a-c1`, branched from `plan/f20-unified-audit-repair` @ `8bcb052b`.
**Verdict: the criterion moves from NOT MET to PARTIAL. Reversibility is done and
live-proven. Governed promotion is NOT done and was deliberately not attempted.**

The lane's own instruction was reversibility before capability, and if only one lands, make
it reversibility. That is what landed.

---

## 1. What the criterion asked, and where each clause now stands

> A drafted skill can be **observed**, **promoted**, **revoked**, and **rolled back**, and
> **cannot execute before governed promotion**.

| Clause | Before (`CRITERIA-GAP-LEDGER.md:190-219`) | Now | How proven |
|--------|-------------------------------------------|-----|------------|
| observed | met | **met** | `wcore-skill-govern list`, driven live |
| **revoked** | **nothing implements it. Zero.** | **MET** | real binary drive + 21 tests + mutation |
| **rolled back** | **nothing implements it. Zero.** | **MET** | byte-identical restore, sha256-compared |
| cannot execute before promotion | "satisfied **vacuously**" | **met, and NOT vacuously** | source measurement — see §4, honesty note |
| **promoted** | `bail!` | **STILL NOT MET** | untouched; see §5 |

---

## 2. What landed

**`wcore_skills::govern`** — a governance store under
`<data_dir>/wayland-core/skills-governance/`, holding an append-only `journal.jsonl`, a
suppression index, and the retained bytes of everything a revocation removed.

- `revoke()` retains every byte, makes suppression **durable**, and only then removes the
  artifact. The ordering is the point: no crash can leave the artifact deleted with nothing
  suppressing it. It is idempotent, so an interrupted revocation converges on retry.
- `rollback()` restores the exact prior bytes **and clears the suppression** — without
  which the tombstone would itself become a new irreversible mutation, reproducing the
  defect one level up.
- Revocations are keyed on **both** the drafter signature and the skill name, matching on
  either. Signature is the better key, but `loader.rs` already tolerates drafts with a
  damaged manifest and those have no recoverable signature.
- Snapshots refuse symlinks (a link could escape the skill directory) and are size- and
  depth-capped.

**`SkillDrafter::draft` consults the store before writing.** This is what makes a revocation
a revocation rather than an `rm` the product silently undoes on the next qualifying streak.
Suppressions are journalled, so "it did not come back" is *provable* rather than merely
observed. The engine logs that outcome as `info`, not through the failure arm, and emits no
capability activation for it.

**`wcore-skill-govern`** — a crate binary (`list` / `revoke` / `rollback` / `history`),
because `wcore-cli/src/main.rs` is fenced. The capability ships and is drivable today; the
flag wiring is SR-23A-C1-1.

**Root placement.** The store is a **sibling** of the skills directory, never inside it, and
resolves `WAYLAND_HOME` → `XDG_DATA_HOME` → `dirs::data_dir()`. Honouring `WAYLAND_HOME`
keeps profiles isolated and tests hermetic; preferring the data dir over the config dir was
the panel's unanimous call (§3).

---

## 3. Cross-audit (LANE-BRIEF §4)

3/3 present, no dropped votes, real 5-question design prompt. Full record in
`evidence/23A-C1/CROSS-AUDIT.md`.

Unanimous on: tombstone semantics over plain delete; signature keying; moving the store out
of the config dir; scoping this lane to revoke+rollback only.

**Two places I did not simply take the vote:**

- **The adversarial pass won an amendment the panel missed.** All three said "key on
  signature". `loader.rs:463` already handles drafts with a missing/damaged manifest, for
  which the signature is unrecoverable — signature-only keying fails on exactly that subset.
  Hence the dual key.
- **I took the minority on the panel's top-ranked risk, and the majority was measurably
  wrong.** 2/3 ranked "the unbound `Procedure` DB row" as most likely to fail. They asserted
  it without the source, so I measured instead of designing around it. Neither DB row is a
  live resurrection path (§4.2). codex's minority ranking — crash/race non-atomicity — is
  the operative risk, and its ordering fix is what the implementation uses.

---

## 4. Measurements worth carrying forward

### 4.1 The harm is default-on, not opt-in

`observability.skills_lifecycle` **defaults ON**, asserted for both the TOML-omitted path
and the struct-`Default` path, the latter labelled "no-config first run"
(`config.rs:605/688/7233`). On a fresh install the product writes into the user's global
skills directory with no user action. That is what made the missing revocation a real
customer symptom rather than a theoretical one.

### 4.2 Three stores hold an auto-draft; only one could resurrect it

| Store | Live resurrection path? |
|-------|------------------------|
| on-disk `auto-<sig>/` | **YES** — the bucketer calls `draft()` again. **This lane closes it.** |
| `evolved_prompts` row (`scorer="auto_drafter"`) | **No** — hydration keys off `catalog.visible()`, which quarantine excludes |
| `Procedure` row (`Tier::Project`) | **No** — no production reader materialises or executes one |

The second finding is a **latent hazard for promotion specifically**: the moment promotion
lifts `disable_model_invocation`, that dead hydration path lights up against a retained DB
row no file-level revocation touches. Filed as SR-23A-C1-2. **Whoever implements promotion
must handle it.**

### 4.3 Honesty note on the no-execute clause — I am NOT inheriting the census claim

The ledger called this clause vacuous. It is not: `loader.rs:448` sets
`disable_model_invocation` on generated drafts, `slash/skill.rs:115` refuses them
("this skill is quarantined and cannot be run"), and `engine.rs:5351` gates the model path.
**But that is a source measurement I made by reading those three call sites — I did not
drive the boundary live**, and per the coordinator's note ten of the census's sixteen routes
were re-resolved citations rather than live drives. So: the clause is met on the evidence I
read, and I make no stronger claim than that.

---

## 5. What is NOT done — stated precisely

**Governed promotion is not implemented, and I did not start it.** `--skills-promote`
remains an unconditional `bail!` at `main.rs:2516` and remains `hide = true`. No half-built
promotion path exists, so the tree does not contain a partial forward path without its
reverse — which the lane brief explicitly forbade.

**Its shape, for whoever picks it up (estimate: 2–3 sessions):**

1. **Bind the artifact to a record.** Nothing today links the on-disk `SKILL.md` to the
   `Procedure` row — they are created by different code paths (`SkillDrafter` vs
   `DraftWriter` at `engine.rs:5077`) and share no identifier. Promotion cannot be
   "governed" until one names the other. The `govern` journal already carries the
   signature + name pair a binding can key on.
2. **Add the state transition.** `ProcedureStatus` has no `Revoked` variant; it is
   `Staged|Active|Archived|Pinned` and `Archived` is a curation state, not a revocation.
   Adding a variant is a serde wire question (`Serialize`/`Deserialize`, `rename_all`).
3. **Lift the quarantine transactionally**, clearing `disable_model_invocation` only for a
   specific promoted generation — **and handle SR-23A-C1-2 in the same change**, or
   promotion will resurrect revoked content through the router seed path.
4. Only then un-hide the flag.

**Also not done:** CLI flags (SR-23A-C1-1, fenced file); pruning/`purge` of retained
generations (SR-23A-C1-3, panel-raised); revocation does not clean the two inert DB rows —
measured harmless (§4.2) but incoherent, and worth closing with promotion.

---

## 6. Gates — real numbers, executed counts read back

| Gate | Result |
|------|--------|
| `wcore-skills --test govern_revoke_rollback` | `15 passed; 0 failed; 0 ignored; 0 filtered out` |
| `wcore-skills --test govern_cli_drive` (drives the real binary) | `6 passed; 0 failed; 0 ignored; 0 filtered out` |
| `wcore-skills` whole crate, **isolated** | lib `623 passed; 0 failed; 2 ignored`; all integration targets green |
| `wcore-agent --lib`, **serial** (`--test-threads=1`) | `2152 passed; 0 failed; 3 ignored; 0 filtered out` |
| `cargo clippy -p wcore-skills --all-targets` / `-p wcore-agent --lib` | no warnings |
| `cargo fmt --all -- --check` | clean |
| Fence (`main.rs`, `lib.rs`, `ci.yml`, `BACKLOG.md`) vs merge-base `8bcb052b` | **no changes** |

`0 filtered out` on both new suites is stated deliberately: LANE-BRIEF §3.2 flavour (c) is a
filter matching no test, which exits 0 having run nothing. Both report exactly the count they
contain, and neither has any `#[ignore]`.

**Red-before-green.** Reverting the suppression check
(`is_revoked(...)` → `false`, 1 site, count asserted) turns
`revoked_draft_is_not_recreated_and_rollback_restores_it` **RED** with
`test result: FAILED. 6 passed; 1 failed`. Both positive-path tests stay green under the same
mutation, so the suite is not uniformly sensitive and a pass is attributable.

**Two instrument defects found and repaired in-lane, not merely noted (§6b-ii):**

1. **The `grep` this lane depended on was lossy.** An unpiped `grep` is rewritten by an
   `rtk` hook that compresses output, re-orders it against surrounding `echo`s, and
   misreports file counts — it rendered a **one-file** grep as `9 matches in 7 files` when
   the true answer was **0**. The lane's central measurement is "grep for a revoke surface
   returns zero", which that instrument would have produced for free. Repaired
   (`rtk proxy grep … | cat`), with a **three-assertion** self-test proven able to fail
   (`harness-selftest.sh`, exit 1 under mutation). Recorded alongside: **a known-negative
   assertion is self-passing on a dead instrument** — under mutation A2 still passed.
2. **The rendering could not be checked without an unbound matcher.** Acting on the
   coordinator's note, `list` now emits every fact on that skill's own line as `key=value`,
   and the checker binds to one line. `field_matcher_selftest` uses a listing whose two rows
   carry **opposite** statuses and shows the unbound form concluding "auto-present is
   revoked" on the same bytes the bound form reads correctly.

**A trap that fired and was caught:** the mutation run reported `MUTATION_EXIT=0` for a run
that had just FAILED — a pipe stole cargo's status. Every grade above is taken from a read
count, never from an exit status.

---

## 7. Live evidence (LANE-BRIEF §3.1)

Full transcript in `evidence/23A-C1/LIVE-EVIDENCE.md`. The shipped binary, driven on
`hetzner-dsm` with `WAYLAND_HOME` pinned, **with a user-authored control skill present
throughout** so that a binary which revoked everything would fail rather than pass:

- draft and control both listed → `revoke` → draft gone from the directory, `ls` shows only
  the control → `rollback` → **`sha256` before == after, `BYTE_IDENTICAL=yes`** → journal
  shows `REVOKED` then `ROLLED-BACK`, the rollback having **appended**, not erased.

---

## 8. Compliance

- **No credential was used.** No live provider turn was needed; nothing in this lane reaches
  a provider. Nothing was swept for because nothing was injected.
- `wcore-contract generate` **not** run. No protocol type touched, no contract change requested.
- No merge, no PR, no tag, no issue closed. Lane branch pushed only.
- No `git add -A`; no `checkout`/`reset`/`stash`/`rebase` in the shared Mac repo. The one
  hetzner mutation used `cp`-backup and restore, **not** git.
- `.planning/BACKLOG.md` is fenced, so its two rows are filed as SR-23A-C1-2/3 instead.
