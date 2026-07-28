---
phase: 30-continuous-scorecard-frontier-review
plan: "03"
subsystem: eval-harness
status: complete
termination_state: 2 (sparse)
tags: [frontier-review, claims-register, publication-is-a-code-path, scope-containment, F30-04]
requires: ["30-01", "30-02"]
provides:
  - "a claims register in which an unsupported claim cannot be rendered into a published document"
  - "twelve typed refusal rules, every one attacked by a paired corpus case that actually fired it"
  - "the published allowed / prohibited / limitations set, rendered and re-render-verified on hardware"
  - "the finding that this phase's peer comparatives are not yet measurable, made mechanical rather than advisory"
affects: ["30-04"]
tech-stack:
  added: []
  patterns:
    ["publication-is-a-code-path", "scope-containment", "single-rule-no-second-copy", "paired-corpus-structural"]
key-files:
  created:
    - crates/wcore-eval-scenarios/src/claims.rs
    - crates/wcore-eval-scenarios/tests/claims_honesty_corpus.rs
    - .planning/phases/30-continuous-scorecard-frontier-review/30-03-CLAIMS-ALLOWED.md
    - .planning/phases/30-continuous-scorecard-frontier-review/30-03-CLAIMS-PROHIBITED.md
    - .planning/phases/30-continuous-scorecard-frontier-review/30-03-LIMITATIONS.md
    - .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-03/
  modified:
    - crates/wcore-eval-scenarios/src/lib.rs
    - crates/wcore-eval-scenarios/bin/wayland-scorecard.rs
decisions:
  - "Comparative bounding is SCOPE-DECIDED: a measured comparative needs a real interval, a static-source census needs an explicit unproven-qualifier. Reconciles the plan's two different formulations of the same rule."
  - "Two rules added beyond the plan's list, both STRICTER: evidence_id_unresolved and confounded_leg_supports_no_comparison."
  - "ZERO comparatives published. All nine RUN legs are confounded by the write_file dialect defect, so no peer comparison is publishable in either direction."
  - "Limitations are exempt from scope containment: a limitation asserts nothing about the world, so it makes no reach for containment to bound."
metrics:
  allowed_claims: 9
  allowed_comparatives: 0
  limitations: 20
  attempted_and_refused: 10
  refusal_rules: 12
  corpus_rows: 24
  distinct_rules_fired: 12
  targeted_suite: "485 passed, 0 failed, 5 skipped"
---

# Phase 30 Plan 03: Claims register and published claim set Summary

Made an unsupported claim mechanically unpublishable — publication is a code path, not a
writing task — and then ran the mechanism against this phase's own evidence. It returned
**nine allowed claims, none of them a peer comparison, twenty limitations, and ten refusals.**

**Termination state: 2 — Sparse.** Every clause of state 1 is also satisfied: the module
landed green, every named test passes, the corpus refuses every unsupported claim and accepts
every hedged control, and all four published artifacts survive an on-hardware re-render diff.
State 2 is nonetheless the honest label, because the substantive outcome is the SIZE of the
allowed set. The sparseness is a property of the evidence, not a shortfall of execution, and
per the plan padding it would have been the failure.

---

## 0. The headline: this phase publishes no peer comparison at all

Not a hedged one. **None.**

30-02 measured Wayland 0/30 against Hermes 30/30 on correctness and recovery and published it
unaltered, which was right. It also found — by running its own frozen protocol — that the
canonical script emits a tool call named `write_file`, a name only Hermes exposes, and that
**OpenClaw also scored 0/30 on the identical script**. Two of three harnesses failing one
script is evidence about the script's dialect, not about two products.

The temptation this plan had to defeat is not the flattering claim. It is the *unflattering*
claim, published because it looks like integrity. `Hermes completed the scripted task more
reliably than wayland-core` is well-formed, bounded, correctly scoped, and **wrong**, because
the number does not measure the dimension it is named after.

So the finding was made **mechanical instead of advisory**. All nine RUN legs are declared
confounded in the register, and rule `confounded_leg_supports_no_comparison` refuses any
comparison resting on one. The register's own attempted-claims list carries the tempting
sentence and the published prohibited document shows it refused.

**Equivalence is refused too, and deliberately.** All three tools spent an identical 20.00
cost units, but two completed 0/30 of the task. `cost is practically indistinguishable` would
read as a positive finding while describing equal spend for unequal work — on this data the
equivalence claim is the more dangerous of the two, so the rule covers both.

What remains publishable about the trials is the *observation*: **ALW-08 — two of the three
harnesses scored zero of thirty on the identical canonical script, which emits a tool call
named `write_file`.** That is factual, non-directional, and describes what was run rather than
comparing the products. It survives the checker; the comparison does not.

## 1. What was actually left in the allowed set

Nine claims, all factual or static-source, every one checkable by a hostile reader with the
tree open. They are the pre-registration ordering proved from history; both peer pins
re-resolving at their declared versions; all three tools provisioned from their own lockfiles
at their own pins; the fifteen-leg accounting being complete and closed; CTRL-01's ten
families satisfying all seven declared clauses; 39-of-42 evidence-ID resolution; the thirteen
falsified tracking claims all understating the program; the `write_file` observation above;
and 30-02's verifier being demonstrably able to fail.

**The two ledger sentences the plan mandates as accepted controls are NOT in the allowed set,
and that is a deliberate call.** They verify — `a_hedged_evidence_bound_ledger_sentence_verifies`
passes on both, verbatim, which is what proves the checker is not a banned-words list. But
30-01 filed a HIGH: `PEER-PROBE-2026-07-26` names no openable artifact while carrying roughly
half the Delta column in six families, and *"any 30-03 claim resting on a peer comparison
inherits it."* Both control sentences sit in those six families. Publishing them as this
phase's claims would rest on a citation no reader can open.

Rather than leave that as a judgement I made once, it became rule `evidence_id_unresolved`,
and `ATT-03` — the SBOM lead claim — is refused by it in the published prohibited document.

## 2. RED before GREEN, proved on hardware

| | |
|---|---|
| RED commit (corpus alone) | `a88e8451` |
| RED result | `RED_RC=101`, `error[E0432]: unresolved import wcore_eval_scenarios::claims` |
| GREEN commit | `6e2f292f` |
| GREEN result | **10 tests run: 10 passed, 0 skipped** |
| After the attack corpus landed | **11 tests run: 11 passed, 0 skipped** |

Executed counts read back, never exit status.

## 3. The twelve refusal rules, and the paired corpus that fired every one

Ten in the plan's behaviour list plus two added. Both additions are STRICTER — the plan forbids
relaxing a rule to admit a claim; it does not forbid refusing more.

| # | rule | typed error names |
|---|---|---|
| 1 | `no_evidence_reference` | the claim |
| 2 | `evidence_does_not_resolve` | the offending reference |
| 3 | `evidence_leg_unproven` | the leg and its blocker |
| 4 | `evidence_id_unresolved` | **added** — the ID and 30-01's determination |
| 5 | `confounded_leg_supports_no_comparison` | **added** — the leg and the defect |
| 6 | `comparative_without_pinned_baseline` | the claim |
| 7 | `comparative_without_interval` | the scope and the missing-bounds code |
| 8 | `directional_on_interval_containing_zero` | the term, the interval, the entailed verdict |
| 9 | `misclassification` | the declared class and the matched term |
| 10 | `scope_not_contained` | both scopes and the reference |
| 11 | `unbounded_superiority` | the superiority term |
| 12 | `limitation_without_substitution_point` | the claim |

**Corpus record — `evidence/30-03/attack-corpus.tsv`, written by the run itself on hetzner:**

- 24 rows, **24 well-formed = 24 total**
- **12 ACCEPTED, 12 REFUSED**
- **12 DISTINCT rules actually fired** — no rule in the checker is unattacked
- every row names a capture that exists with ≥2 lines

Each case is one `AttackCase` value carrying both halves. `AttackCase::new` takes the pristine
and the mutation positionally, so **a case without its pristine control cannot be constructed**
— the pairing is structural, not a rule a reviewer has to remember twelve times.

### The finding inside the corpus: truncating a hedge manufactures a claim

`ATK-11` uses a real ledger fragment, quoted **verbatim** — *"This is Core's clearest unique
capability"* — severed from the `runtime certification required` qualifier its family carries.
Same evidence, same scope, same source document. With the qualifier it is accepted; without it
the checker refuses it as `unbounded_superiority`.

That is worth naming as a defect class in its own right: **a quotation can be word-for-word
accurate and still fabricate a claim, by dropping the clause that withheld it.** The plan's
own must_haves quote that fragment as an example of correct writing, and it is — in place. It
is not correct alone.

## 4. Publication is a code path — proved both ways

| gate | result |
|---|---|
| re-render all three docs from the committed register on hetzner and diff | **byte identical** (`RERENDER_IDENTICAL`, rc=0) |
| **tamper test** — append one flattering sentence, re-diff | **DETECTED** (`110d109 < wayland-core leads the field on every measured dimension.`) |
| publish against a register with one broken reference | **REFUSED**, wrote **nothing at all**, named the offender |

The tamper test matters more than the clean diff: without it the re-render gate could have been
passing vacuously. The refusal gate lets the REMOTE side decide — setup failures exit non-zero,
an unexpected publish success exits 9, only the refusal exits 0 — because asserting a non-zero
status locally would also pass when ssh is down, when the checkout fails, or when `sed` fails.

Refusal message, in full:

> `register does not verify, publication refused: claim ALW-04 cites .../evidence/30-02/does-not-exist.tsv which does not resolve`

Register digest `6e60102cf284c3115615bbca1176eb705c3d06fd2588445bd99689bf69cbadb3`, embedded
once in each of the three documents and matching `shasum -a 256` computed independently.

## 5. Gate results, with real numbers

| gate | result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| module landed / declared exactly once | `pub mod claims;` ×1 |
| directional rule CALLED not copied | `direction_for(` appears **exactly once**; 4 refs to `frontier_trials` |
| `deny_unknown_fields` boundary structs | 4 (≥2 required) |
| classifier + misclassification present | `COMPARATIVE_LEXICON` ×3, `misclassif` ×4 |
| ten named corpus tests present | 10/10 |
| register ↔ digest agree (both operands proved non-empty first) | **MATCH** |
| digest embedded in all three published docs | 1 / 1 / 1 |
| limitations rows ≥ UNPROVEN legs | **20 ≥ 6** |
| scripted-scope limitation as its own row | 11 rows match |
| **Hetzner targeted suite** | **485 run: 485 passed, 0 failed, 5 skipped** |
| new dependencies | **0** — `Cargo.toml` and `Cargo.lock` untouched |

Suite delta accounted for exactly: 30-02's baseline was 470; this lane adds 11 corpus tests and
4 inline unit tests = **485**. No residual failure.

**Clippy is RED, and it is not mine.** 4 errors, all four in Phase 24's `journey.rs`
(683/695/707/717). I did **not** assume the abort spared my files — re-run without `-D warnings`
it completes at rc=0 with **0 hits across all four of my files** and 4 in `journey.rs`. Not
fixed, not silenced, not attributed here.

## 6. Fence — verified against the MERGE-BASE SHA, not the branch name

`BASE=b79f141eaddf8d9b85638cfabd3ef3cc7b921ea9`, captured once at start and quoted throughout.

- fenced files changed vs BASE (`frontier_trials.rs`, `fixtures/openai.rs`, `receipt.rs`,
  `redaction.rs`, `receipt_policy.rs`, `release_integrity.rs`, `e5_soak.rs`, `process_tree.rs`,
  both `wcore-cli` files, `Cargo.toml`, `Cargo.lock`, `COMPETITIVE-LEDGER.md`): **0**
- 30-02's frozen `evidence/30-02/` changed vs BASE: **0**
- source files this lane changed: **exactly 4** — `claims.rs`, `lib.rs`,
  `wayland-scorecard.rs`, `claims_honesty_corpus.rs`
- working tree dirty: **0**

`redaction.rs` was REUSED, not edited: the bundle calls `SecretRedactor` rather than adding a
second redactor.

## 7. Deviations, each with its reason

1. **Own hetzner worktree `/root/wayland-30-03`** rather than the plan's `cd /root/wayland`.
   Four lanes share that checkout and `git checkout --detach` there would yank another lane's
   tree. Same deviation 30-02 made, for the same reason.
2. **Two refusal rules added** (`evidence_id_unresolved`, `confounded_leg_supports_no_comparison`).
   Both strictly narrow what is publishable. Without the second, the single most misleading
   sentence available in this phase would have been publishable.
3. **Comparative bounding is scope-decided.** The plan states the rule two ways — "a stated
   limitation" and "a real interval". Taken literally the second refuses the mandatory control
   sentences, which carry no interval because a census of two pinned trees has no sampling
   variance. Resolution: measured comparatives need an interval; static-source comparatives
   need an explicit unproven-qualifier. **The relabelling loophole is closed by scope
   containment**, not by trust — a claim citing scripted evidence cannot declare static scope.
   The two rules interlock and neither is sufficient alone.
4. **Limitations are exempt from scope containment.** A limitation asserts nothing about the
   world, so it makes no reach for containment to bound. Directional text in a limitation is
   still refused as misclassification, so it cannot be used to smuggle a superiority claim.
5. **`cargo fmt --all` in write mode on the Mac**, not only `--check`. rustfmt performs no
   compilation; the repo's own history carries a rustfmt-only commit from 30-02.
6. **`CLAIMS_VERIFY` splits `allowed` from `limitations`.** It first printed `allowed=29`,
   which was `claims.len()` — the sum of 9 allowed and 20 limitations. One total overstates the
   allowed set by the size of the larger list. Fixed before it reached any published number.
7. **Removed regenerable test output from the hetzner worktree** (`attack-captures/`,
   `attack-corpus.tsv`) to unblock a checkout. Targeted `rm` of two known-regenerable paths,
   never `git clean`, and both were already committed.

## 8. Findings

**F-30-03-001 — LOW, documentation only.** `30-02-SUMMARY.md` §Gates transcribes its own
authoritative gate as `legs=15 run=6 unproven=9 comparatives=3`. Its committed evidence says the
opposite and agrees with itself in three places: `legs.tsv` (`grep -c` → 9 RUN, 6 UNPROVEN),
`authoritative-gates.txt` (`run=9 unproven=6 comparatives=6`) and 30-02's own frontmatter
(`legs_run: 9`). **The prose is inverted; the data is sound.** No measurement is affected. It
mattered here because the limitations-completeness gate counts `::UNPROVEN::` from `legs.tsv`,
so my floor is 6 rather than 9 — a reader trusting the prose would think the floor was set too
low. Filed for 30-04 to correct at the source; I did not edit another plan's summary.

**F-30-03-002 — the truncated-hedge defect class.** See §3. A verbatim quotation can fabricate
a claim by dropping the clause that withheld it. Now carried as `ATK-11`.

**Instrument defect measured in this lane.** `rtk` silently FILTERED `git log`: in the fresh
worktree `git log --oneline -3` printed `6c7254ee` as HEAD while `git rev-parse HEAD` printed
`b79f141e`; `rtk proxy git log` revealed two merge commits rtk had dropped. The output was not
fabricated — it was *filtered*, which is worse, because it looks complete. Had I taken the
merge-base from it, every fence diff in this summary would have been wrong. Everything
load-bearing went through `rtk proxy` or `/usr/bin/`. This is the ninth instance of the
program's standing pattern: the instrument that hunts a defect class tends to carry it.

Also: **`/usr/bin/cat` does not exist on this Mac** (it is `/bin/cat`). The plan's `/usr/bin/`
list is otherwise correct.

## 9. What this plan did NOT do

- **It did not position.** The plan says so twice in binding language: *"THIS PLAN PUBLISHES
  WHAT MAY BE CLAIMED AND STOPS THERE... The verdict is 30-04's."* My dispatch brief framed
  30-03 as "where positioning is decided". Both are satisfied: this plan fixes **the boundary
  of any position 30-04 can take** — which is the decision — without authoring a verdict
  sentence. The reconciliation is recorded in `evidence/30-03/30-03-NOTES.md §0` rather than
  smoothed over, because a reader comparing dispatch to output would otherwise see a gap.
- **No requirement marked complete.** F30-04 records evidence only; closure is 30-04's.
- **No credential of any kind** was read, requested, printed, logged or committed. No gate here
  requires one and none can be passed by supplying one.
- **No `wcore-contract generate`**, no merge, no PR, no tag, no release, no issue action.
- **The headless-keyring story is NOT positioned on.** It is `LIM-18`, explicitly marked as
  having its remedy under test by another lane, with that lane's result as the substitution
  point.
- **The `security ×3` legs were not laundered.** They remain UNPROVEN (`LIM-01/02/03`) with the
  meter seam as their substitution point. No narrower extraction was substituted.
- **`peer_delta` was measured, not assumed.** It is UNPROVEN on **148/148** surface rows,
  unchanged since 30-01's own commit `4f749251`; 30-02's comparatives are dimension-level, not
  surface-level. Carried as `LIM-16`.

## 10. Honest limits of this evidence

The checker proves a claim points at something that exists, is bounded, is scoped correctly and
is classified consistently. **It cannot prove the sentence means what the evidence shows.** A
claim can be well-formed, fully supported, and still badly worded. That residual is a reading
task and is stated rather than hidden; the mitigation is that the prohibited document makes
visible exactly which sentences were attempted and refused.

The comparative lexicon is a finite list and a sufficiently creative sentence can compare
without matching it — which is why scope containment and the evidence-pointer requirement,
which apply to EVERY claim regardless of class, do more work than the lexicon does.

**The evidence-bundle scan holds ZERO secrets**, so in production it cannot fail and proves only
that the mechanism ran — not that the bundle is secret-free. That is stated in `MANIFEST.tsv`
itself rather than presented as a clean result. What proves the scan CAN fail is
`the_bundle_scan_is_able_to_fail_and_refuses_the_whole_bundle`, which seeds a synthetic canary
and asserts that one leaking projection refuses all three.

## 11. Seam request for the orchestrator

None new. 30-02's open request against `crates/wcore-eval-scenarios/src/fixtures/openai.rs`
stands and is now cited as the substitution point for four limitations (`LIM-01/02/03/19`):
request-body retention under a redaction policy or leaf-hash exposure, and content-routed
rather than FIFO-cursored matching. **A third need is added by this plan's findings:** per-tool
dialect compilation of the canonical script, without which the correctness, recovery and cost
comparatives cannot be re-taken (`LIM-20`).

## Self-Check: PASSED

All created files confirmed present on disk; all commit hashes confirmed in
`git log --oneline --all`; every number in this summary read back from a captured run.
