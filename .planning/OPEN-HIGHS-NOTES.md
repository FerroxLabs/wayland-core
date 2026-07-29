# OPEN-HIGHS — running notes

Lane `lane/open-highs`, base `75babf32`, worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-open-highs`.

Notes-first per LANE-BRIEF §6b-i. Appended after every measurement, not at the end.

---

## T+0 — orientation, established by reading the tree

Read: `LANE-BRIEF.md` (full), `HANDOFF-2026-07-29.md` §3, `MILESTONE-RC.md`, `ZOMBIE-PROBE.md`.

Worktree verified: `git rev-parse --show-toplevel` = the lane path, NOT
`/Users/seandonahoe/dev/waylandcore`. Branch `lane/open-highs` @ `75babf32`.

### Instrument check done up front (§3b-i)

First `grep -rn ... --include=*.rs` returned **`no matches found`** — zsh ate the unquoted glob.
That is exactly the failure mode §3b-i names: an unquoted glob returns zero, and **zero confirms
every absence claim for free**. Had I been asserting an absence I would have "proved" it with a
dead instrument on my first command. Re-run quoted, with a known-positive control in the same
invocation:

- known-positive `fn main` → **73** hits (instrument alive)
- `owner_is_live|owner_pid|process_is_live|is_process_alive` → 41 hits

All load-bearing greps in this lane use `/usr/bin/grep` with quoted globs and a known-positive.

---

## Target 1 — Zombie liveness, macOS binary-level unproven

**Status: state established, not yet measured.**

Verified present at this base (not assumed from the doc):

- `crates/wcore-types/src/process_liveness.rs` — exists (19.8K)
- `crates/wcore-types/tests/real_zombie.rs` — exists (16.0K)
- `git merge-base --is-ancestor 797d4889 HEAD` → **YES**, so the zombie-probe lane's work is in
  this tree.

`ZOMBIE-PROBE.md` §6 states the gap precisely and honestly: Linux and Windows are proven on real
hardware against a real corpse; **macOS is proven only at kernel-semantics + algorithm level, in
C.** The Rust translation executing on Darwin is unproven. It names the single closing command:

```
cargo test -p wcore-types --test real_zombie
```

**Two routes now exist that did not exist when that gap was written:**

1. **LANE-BRIEF §0 Darwin-behaviour exception** (added 2026-07-29) explicitly permits
   `cargo test -p <crate> --test <file>` on the Mac for platform behaviour only Darwin can
   demonstrate. That is verbatim the command above. I qualify: hetzner cannot prove this — no
   permitted host executes Darwin code.
2. **`sean-mac-arm64` self-hosted runner** — queried live via `gh api`:
   `{"id":34,"name":"sean-mac-arm64","os":"macOS","status":"online","busy":false}`, labels
   `self-hosted, macOS, ARM64`. Online and **idle**.

Route 2 is fenced for me: I may dispatch to the runner but must NOT touch `.github/workflows/*`
(two lanes hold those), so I can only dispatch a workflow that already targets macOS. Route 1 is
unfenced and is the cheaper, more direct measurement. **Plan: take Route 1, disclose its use per
§0.** Check for an existing macOS workflow as a corroborating second route.

**The trap I must not fall into (§3b-i, and the lane's own headline):** containment was working
the whole time and the probes could not tell a corpse from a live process. So a green
`real_zombie` on macOS proves nothing unless assertion 3 — *the old shape would have missed it* —
is confirmed to have actually executed and passed on Darwin. I must read back the executed count
and confirm the decoy discrimination, not the exit status. `cargo` is proxied by `rtk` and
**strips `0 ignored` / `0 filtered out`** — use `/usr/bin/env cargo` absolute, or `rtk proxy`.

Also to verify, not assume: that the macOS arm's raw-offset read (`p_stat`/`p_pid` at offsets
36/40, `sizeof=648`) still holds on this machine's OS version. `ZOMBIE-PROBE.md` says an ABI drift
degrades to `Indeterminate`, never a wrong answer — so a *pass* could in principle be a pass on a
degraded arm. Must confirm the corpse reads `Dead`, not `Indeterminate`.

## Target 2 — 23A resurrection hazard

**Status: hazard re-measured at HEAD and CONFIRMED. Guard site decided on dependency-graph
grounds. Coordination checked — a real divergence found, and it is not a duplication.**

### Re-measured at HEAD (line numbers had drifted, structure held)

| Claim (from `F23A-C1-M1` / `23A-C1-NOTES.md` §6.3) | At `75babf32` |
|---|---|
| hydration at `bootstrap.rs:2145` | **moved to `:2173`**, `seed_pairs_for(&candidate_names, "auto_drafter", 1)` |
| `candidate_names` = `catalog.visible()` | **TRUE**, `bootstrap.rs:2147` |
| `visible()` filters `!disable_model_invocation` | **TRUE**, `refs.rs:129` |
| auto-drafts quarantined | **TRUE**, `loader.rs:448` sets `disable_model_invocation = true` |
| Layer 1b additionally gated | **NEW** — also behind `self.config.observability.skills_lifecycle` (`:2172`) |

**Checked for a second production hydration path** (the "sole path had three" trap):
`seed_pairs_for` has 2 non-test call sites in `bootstrap.rs` (`:2152` scorer `bench`, `:2173`
scorer `auto_drafter`). `drafter.rs:511` also calls it with the draft's own name, bypassing
`visible()` — **but it is inside `#[test] drafted_skill_hydrates_router_seed_via_auto_drafter_scorer`**,
so it is not a production path. It is still evidence *for* the hazard: it proves the retained row
hydrates to a 4-success prior whenever the name reaches the candidate list.

### Revocation does not, and structurally CANNOT, purge the row

Concept sweep over `wcore-skills/src/` for `evolved_prompts|PromptStore|prompt_store|auto_drafter`
→ **0 code hits** (one unrelated doc comment in `router.rs:103`). Known-positive control in the
same session: `evolved_prompts` across `crates/` → **22** hits, so the instrument is alive and the
zero is a real zero.

`prompt_store.rs:160-162` states the reason in-tree: *"`wcore-skills` cannot depend on
`wcore-evolve` (the dep already runs the other way), so callers (e.g. agent bootstrap) bridge the
two via this helper."*

**This decides the guard site, on the dependency graph rather than on preference.** Making
`govern::revoke()` purge the `evolved_prompts` row would require `wcore-skills → wcore-evolve`,
inverting an existing edge. So the only correct place for the guard is **the bridge point: agent
bootstrap, Layer 1b.** That is the site in my scope.

### `seed_pairs_for` matches on NAME ALONE — this widens the hazard

`prompt_store.rs:163-182` + `:193` — the query is `WHERE skill_name = ?1` and a scorer match.
**No signature, no provenance, no governance.** So the hazard is not only "a revoked draft comes
back":

> A user revokes auto-skill `deploy-helper` (directory deleted, tombstone written, DB row
> retained). Later they hand-write their own unrelated skill named `deploy-helper`. It is visible,
> so it enters `candidate_names`, and Layer 1b hydrates **the revoked skill's 4-success prior onto
> the user's new skill.** No promotion required for this variant once quarantine lifts.

The drafter-side guard does not cover this: it suppresses re-*drafting*, it does not remove the row.

### Coordination — divergence found, NOT duplication

`lane/23a-c1-governed` @ `098c2eb9` (read-only inspection) has committed **only** its NOTES file,
no code. Its plan of record claims the hazard: *"promotion is the path I am building. So closing
this is mine."* I therefore do **not** touch promotion, `ProcedureStatus`, the loader, or the CLI
surface — all theirs.

**But its named mechanism is a different store from the graded finding's, and prior measurement
found its store inert.** Its T+25 chain is `Procedure` row → promotion materialises directory →
`seed_from_prioritizer` (Layer 2). That is store **(c)** in `23A-C1-NOTES.md` §6.3, which that
same file measured as **INERT**: *"No path materialises a `Procedure` into an on-disk skill or
executes one."* Its notes never mention `evolved_prompts`, `seed_pairs_for`, or Layer 1b — store
**(b)**, which is the one §6.3 measured as gated shut *only* by the quarantine their work lifts.

This is a **recurrence of a recorded error**: `CROSS-AUDIT.md` Q4 records that 2/3 of the panel
ranked the `Procedure` row top risk and that the measurement showed *"the panel names the wrong
database"*. The concurrent lane has independently re-selected the wrong database.

Their guards (refuse revoked name at promotion; loader enforcement) would cover the
directory-rebuild variant. They do **not** cover the name-collision variant above, because those
guards are directory-level and the row is name-keyed.

**Action: build the Layer 1b guard (mine, non-overlapping), and flag the divergence to them
rather than fixing their half.**

## Target 3 — grades resting on a dead instrument

**Status: not yet started.** The precedent is `BL-23B-H1` (`MILESTONE-RC.md` §2 row 4): a HIGH
downgraded to MEDIUM on a harness that pointed at a dead port with a placeholder key, so no run
ever dispatched a tool event, and **non-reaching runs were silently counted as successes**. That
is one instrument with two independent self-passing defects. Task: sweep current grades for the
same shape — specifically any re-grade justified by a *negative* result.

---

## Fence exposure so far

None. No files modified outside `.planning/`. `crates/wcore-cli/src/{lib,main}.rs` untouched.
`.github/workflows/*` untouched.
