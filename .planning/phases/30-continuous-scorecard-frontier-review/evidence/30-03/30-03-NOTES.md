# 30-03 NOTES — running record, committed early and re-committed after every measurement

Lane `lane/30-03`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-30-03`.
BASE (captured once, quoted everywhere) = `b79f141eaddf8d9b85638cfabd3ef3cc7b921ea9`.

## 0. The scope conflict between my dispatch prompt and the plan — resolved toward the plan

My dispatch brief says *"30-03 is where positioning is decided ... That call is yours, and it is
the one thing the whole phase exists to produce."*

**30-03-PLAN.md says the opposite, twice, in binding language:**
- `<execution_rules>`: *"THIS PLAN PUBLISHES WHAT MAY BE CLAIMED AND STOPS THERE. It does not
  position, does not recommend a position, does not grade the phase ... The verdict is 30-04's."*
- scope fence: *"No positioning, no readiness claim, no requirement marked complete."*

**Resolution: the plan wins, and the two are reconcilable.** My instruction is "execute
`30-03-PLAN.md` to completion"; the plan's own scope fence is part of what I execute. The
reconciliation is not a dodge: this plan *does* decide positioning, in the only way this program
permits it to be decided — it fixes **the set of claims that may be made at all**. The allowed set,
the generated prohibited set and the limitations are precisely the boundary of any position 30-04
can take. What I must NOT do is author a verdict sentence. Deciding the claim boundary is the
decision; writing the position from it is 30-04's.

This is recorded rather than smoothed because a reader comparing my dispatch to my output will
otherwise see an unexplained gap.

## 1. Inherited facts that constrain the register

- **Wayland lost two comparatives** to Hermes 0.17.0: correctness 0/30 vs 30/30, recovery 0/30 vs
  30/30. Published unaltered by 30-02. Not to be softened.
- **The protocol that produced them is defective, found by running it.** The canonical script emits
  a tool call named `write_file` — Hermes' name; Wayland's is `Write`. **OpenClaw also scored 0/30
  on the identical script.** Two of three harnesses failing one script is evidence about the
  script's dialect, not about two products. 30-02's own words: whether that is a Wayland
  interoperability defect or an artifact of the script *"is not settled by this evidence, and 30-03
  must not position from it as though it were."*
- **30-01: 13 CTRL-01 claims falsified by the tree, every one UNDERSTATING the program.** Headline
  finding binding me: *"Phase 30 must not position from the ledger's Limitation columns as
  written."* So the document I would naively position from is pessimistic, not optimistic.
- **`PEER-PROBE-2026-07-26` is UNRESOLVED and names no openable artifact**, yet carries half the
  Delta column in six families. 30-01: *"Any 30-03 claim resting on a peer comparison inherits
  it."* This is a HIGH and it directly constrains my allowed set.
- `HERMES_BASE_URL` does not exist at the pin (0 hits); the real override is `OPENAI_BASE_URL`.
- `wayland-core` refuses to start headless without an OS keyring; neither peer needs an equivalent.
  A separate lane is testing the remedy — **do not position on the headless story as settled**;
  if a conclusion would depend on it, it is marked pending.
- `security ×3` legs are UNPROVEN because the meter records body **digests**, not bodies. 30-02
  refused to silently substitute a narrower extraction. Seam request open. Not to be laundered.

## 2. MEASURED — leg accounting, and a transcription defect in an inherited summary

`30-02-SUMMARY.md` §Gates transcribes the authoritative gate as
`TRIALS_VERIFY=OK legs=15 run=6 unproven=9 comparatives=3`.

The underlying data says the **opposite**, and agrees with itself in three places:

| Source | RUN | UNPROVEN | comparatives |
|---|---|---|---|
| `evidence/30-02/legs.tsv` (`grep -c`) | **9** | **6** | — |
| `evidence/30-02/authoritative-gates.txt` | **9** | **6** | 6 |
| `30-02-SUMMARY.md` frontmatter (`legs_run`/`legs_unproven`) | **9** | **6** | — |
| `30-02-SUMMARY.md` §Gates prose | 6 | 9 | 3 |

**Finding F-30-03-001, severity LOW (documentation-only, no measurement affected):** the §Gates
prose line of `30-02-SUMMARY.md` inverts RUN/UNPROVEN and halves the comparative count relative to
its own committed evidence. The data is consistent; only the prose is wrong. I take **9 RUN /
6 UNPROVEN** as authoritative because `legs.tsv` and `authoritative-gates.txt` are the captures and
the frontmatter agrees with them. Consequence for me: the LIMITATIONS-COMPLETENESS gate floor is
`grep -c '::UNPROVEN::' legs.tsv` = **6**, not 9. The gate reads that file at run time, so it
self-corrects; recorded because a reader trusting the prose would think my floor is too low.

The 6 UNPROVEN legs are `LEG-03/08/13` (security, all three tools) and `LEG-05/10/15`
(cognitive_tax, all three tools).

## 3. Instrument hazards measured in THIS lane so far

- **rtk silently filtered `git log`.** `git log --oneline -3` in the fresh worktree printed
  `6c7254ee` as HEAD while `git rev-parse HEAD` printed `b79f141e`. `rtk proxy git log` showed the
  two merge commits rtk had dropped. `6c7254ee` is a genuine ancestor, so the output was not
  fabricated — it was *filtered*, which is worse, because it looks complete. **Everything
  load-bearing goes through `rtk proxy` or `/usr/bin/`.** Same defect class as the plan's own
  warning that the ambient `grep` is rtk-proxied and silently drops lines.
- **`/usr/bin/cat` does not exist on this Mac** — it is `/bin/cat`. The plan's `/usr/bin/` list
  (git, grep, shasum, cut, wc) is correct; `cat` is not in it and must not be added.

## 3b. MEASURED — `peer_delta` is still UNPROVEN on all 148 surface rows

The brief asked me to establish this rather than assume it. Measured, not assumed:

| Measurement | Command | Result |
|---|---|---|
| surface rows | `grep -cv '^#' evidence/30-01/surface-truths.tsv` | **148** |
| rows carrying UNPROVEN | `grep -c UNPROVEN` (149 incl. the header comment) | **all 148** |
| last commit touching the file | `rtk proxy git log -- surface-truths.tsv` | `4f749251` — **30-01's own commit** |

So 30-02 did **not** move it. That is consistent rather than surprising: 30-02's comparatives are
**dimension**-level (correctness / recovery / cost across three tools), while `peer_delta` is a
**surface**-row truth. Nothing in 30-02 produced a per-surface peer comparison, so the honest
state is unchanged: **`peer_delta` UNPROVEN on 148/148 rows.** It becomes a limitation, not a claim.

## 3c. MEASURED — the other limitation inputs

- **6 UNPROVEN legs**: `LEG-03/08/13` security (all three tools), `LEG-05/10/15` cognitive_tax.
- **6 shipped commands owned by no coverage family** (30-01, MEDIUM): `init`, `mcp-serve`,
  `models`, `profile`, `project-context`, `setup` — **15 surface rows**. Three of the six
  (`setup`, `init`, `profile`) are first-run and credential-adjacent.
- **2 UNRESOLVED evidence IDs**: `PEER-PROBE-2026-07-26` (HIGH — names no openable artifact yet
  carries half the Delta column in six families) and `F05-TRUTH-{n}` (LOW — template, not instance).
  Plus 1 PARTIAL: `F28-MATRIX-651`.
- **Dependencies**: serde, serde_json, sha2, thiserror, anyhow, clap all already declared in
  `wcore-eval-scenarios/Cargo.toml`. **No new dependency needed**, as the plan requires.

## 3d. DESIGN DECISION — how a comparative claim is BOUNDED, and why it is not "interval only"

This is the one genuine design tension in the plan and it is recorded because it is a judgement.

**The tension.** The plan states the rule two different ways:
- must_haves truth 3, first: the ledger sentences are correct *"because each is bound to a named
  evidence ID, a pinned peer baseline and **a stated limitation**"*;
- must_haves truth 3, second: *"a COMPARATIVE claim must carry a resolving evidence reference, a
  pinned peer baseline and **a real interval**"*.

These are not the same requirement, and **taken literally the second one refuses the mandatory
controls**: the ledger's delta sentences are comparative and carry **no interval at all**, because
they are structural observations of two pinned source trees, not sampled measurements. A census has
no sampling variance; demanding an interval on it demands a meaningless number. But simply dropping
the interval requirement would let the 0/30 correctness result be published as bare superiority,
which is the exact sentence this plan exists to prevent.

**The resolution: a comparative claim must be BOUNDED, and there are exactly two ways to be
bounded — determined by scope, not by the author's choice.**

| Evidence scope | Bounding requirement | Why |
|---|---|---|
| `SCRIPTED_HARNESS` / `LIVE_PROVIDER` | **a real interval**, and the directional rule applies | a sampled measurement has variance, so bounds are meaningful and mandatory |
| `STATIC_SOURCE` | **an explicit unproven-qualifier in the claim text** | a census of two pinned trees has no variance; what makes it honest is that it withholds the assertion |

**The loophole this could open is closed by scope containment, not by trust.** A claim resting on
the trial legs cites `SCRIPTED_HARNESS` evidence, and containment then forces the claim's own scope
to `SCRIPTED_HARNESS` — so it lands in row 1 and the interval is mandatory. It cannot relabel itself
`STATIC_SOURCE` to reach row 2, because its evidence's scope does not contain that claim. The two
rules interlock; neither works alone.

A `STATIC_SOURCE` comparative with **no** qualifier is refused as `unbounded_superiority`. So
*"Wayland is architecturally superior to Hermes"* is refused even at static scope, while
*"Core architectural lead, operationally unproven"* is accepted. That is the distinction the plan
is actually reaching for, and it is what makes the checker a classifier rather than a word ban.

**Consequence I am recording rather than hiding:** the ledger fragment the plan quotes as
*"this is Core's clearest unique capability"* is refused **when quoted alone**, because severed from
its family's `runtime certification required` qualifier it is an unhedged superlative. Quoted with
its qualifier it is accepted. I have four verbatim ledger controls that pass, so the plan's floor of
two accepted controls is met without weakening the rule. The severed-fragment case is carried in the
attack corpus as a finding in its own right: **truncating a hedge is itself a way to manufacture an
unsupported claim**, and it is worth having a rule fire on it.

## 4. Still to establish

- `peer_delta` state across the 148 surface rows — 30-01 had it UNPROVEN on all of them; the brief
  says establish what it actually is now rather than assume.
- Which of 30-01's 42 evidence IDs are citable by a claim (39 CONFIRMED, 1 PARTIAL, 2 UNRESOLVED).
- The exact `frontier_trials` directional-rule function to CALL (not copy).
- The verbatim ledger sentences to use as accepted pristine controls.

## 4b. RESOLVED — everything §4 listed as outstanding

- **`peer_delta`**: measured UNPROVEN on 148/148 rows (see §3b). Carried as `LIM-16`.
- **Citable evidence IDs**: 39 CONFIRMED, 1 PARTIAL, 2 UNRESOLVED. The two UNRESOLVED became
  rule `evidence_id_unresolved` rather than a note, so 30-01's HIGH is now mechanical.
- **The directional rule to CALL**: `frontier_trials::direction_for(&IntervalV1, f64)` at
  line 518, with `DirectionV1::is_directional()`. Called exactly once; `grep -c 'direction_for('`
  on claims.rs = **1**. Not copied.
- **Verbatim ledger controls**: four candidates carry an explicit qualifier and pass. Two are
  used as the mandated pristine controls (AUTH-\* and SUPPLY-\* delta sentences). Both verify.

## 4c. FINAL measured results (all read back from captured runs)

| leg | result |
|---|---|
| RED (`a88e8451`) | `RED_RC=101`, `error[E0432]: unresolved import ...::claims` |
| GREEN (`6e2f292f`) | 10 run, 10 passed, 0 skipped |
| corpus complete (`ba665f24`) | 11 run, 11 passed, 0 skipped |
| `claims verify` | `allowed=9 limitations=20 attempted_and_refused=10 rules_fired=9` |
| corpus TSV | 24 rows, 24 well-formed, 12 ACCEPTED, 12 REFUSED, **12 distinct rules** |
| re-render diff | byte identical; **tamper test DETECTED an appended sentence** |
| publish vs broken register | REFUSED, wrote nothing, named the offender |
| targeted suite | **485 run, 485 passed, 0 failed, 5 skipped** (470 baseline + 15 new) |
| clippy | 4 errors, ALL in Phase 24 `journey.rs`; **0 in my four files**, proven by a second run |
| fence vs BASE `b79f141e` | 0 fenced files changed; exactly 4 source files touched |

## 5. Termination state

**DETERMINED: state 2 — Sparse.** 9 allowed claims, **zero of them comparatives**, 20
limitations, 10 refusals. Every clause of state 1 is also satisfied (module green, all named
tests pass, corpus refuses every unsupported claim and accepts every hedged control, all four
artifacts survive the on-hardware re-render diff), but state 2 is the more informative and more
honest label because the substantive outcome is the SIZE of the allowed set. The sparseness is
a property of the evidence, not a shortfall of execution. Padding it would have been the
failure the plan exists to prevent.
