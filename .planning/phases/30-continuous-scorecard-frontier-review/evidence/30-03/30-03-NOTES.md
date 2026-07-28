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

## 4. Still to establish

- `peer_delta` state across the 148 surface rows — 30-01 had it UNPROVEN on all of them; the brief
  says establish what it actually is now rather than assume.
- Which of 30-01's 42 evidence IDs are citable by a claim (39 CONFIRMED, 1 PARTIAL, 2 UNRESOLVED).
- The exact `frontier_trials` directional-rule function to CALL (not copy).
- The verbatim ledger sentences to use as accepted pristine controls.

## 5. Termination state

Not yet determined. States available: 1 Complete, 2 Sparse, 3 Escalated. Expectation on the
evidence so far is **2 (Sparse)** — a small allowed set, a long prohibited list and a longer
limitations document. Per the plan, that is a correct outcome and padding it is the failure.
