# Phase 20 — Pre-Seal Findings Disposition

Acceptance record for the Ferrox-migrated planning candidate before sealing and
resuming plan 20-03. Two MEDIUM findings carried in from the transfer
(`HANDOFF.json` blockers) plus one stale counter. Every finding is fixed or
rejected with executable counter-evidence, per the repository rule that every
substantiated severity is repaired or disproved before acceptance.

Candidate: `plan/f20-unified-audit-repair` in the standalone clone
`/Users/seandonahoe/dev/waylandcore-ferrox` (base pin `4018e5c3`, tree `f9600c64`).
Execution toolchain: local Ferrox (GSD v1 fork), `runtime: codex`.

---

## M1 — Reviewer identity binding (MEDIUM) — REJECTED (bounded by trust model) — cross-audit-confirmed

**Finding (as transferred):** `verify-review-result.mjs` accepts
`source_executor_id` and `reviewer_id` as any two distinct format-valid strings;
bind them to actual execution history (`.planning/agent-history.json`) with
hostile tests.

**Why no in-scope, non-forgeable mechanical fix exists (verified, cross-audited):**

1. **No execution-identity ledger exists.** `.planning/agent-history.json` is
   absent, and Ferrox's `history-digest` is a semantic phase digest with no
   per-commit executor identity. There is nothing durable to bind to.
2. **Git-author binding is satisfiable but forgeable.** Authorship is NOT
   uniform — the Phase 20 source was authored by `Wayland F20 Builder
   <f20-builder@ferroxlabs.invalid>` (in-lineage commits ce3464f, 4d98c1b,
   b57f9b7, b9cc669), distinct from `ci/Sean <sean@seandonahoe.com>` who
   authored the planning/review commits. So binding `source_executor_id` /
   `reviewer_id` to commit authorship IS satisfiable — but `git commit
   --author=` and `GIT_AUTHOR_*`/`GIT_COMMITTER_*` are freely settable by a
   same-UID actor, so the binding is **forgeable** and adds no real assurance
   against the threat. (Correction: an earlier draft of this disposition wrongly
   claimed uniform authorship, drawn from a last-40-commit window that excluded
   the builder identity; caught and corrected via independent cross-audit.)
3. **The trust model excludes the non-forgeable path by design.** `PROJECT.md`
   §"Local trust boundary": *"Phase 20 does not claim cryptographic resistance to
   a same-UID actor that can rewrite Git history, scripts, and Git-private state;
   that requires an external signer or isolated control plane and is outside this
   pure-GSD execution milestone."* An unforgeable identity binding needs exactly
   that out-of-scope external signer.

**What the gate already guarantees mechanically (the achievable maximum):**

- `verify-review-pair.sh` proves the review is a **distinct sole-parent commit
  over the exact, byte-unchanged source**, on a linear merge-free metadata chain
  changing only the summary + approved metadata paths. Source-tamper and
  hidden-in-history tamper are rejected (proof suite lines 117–203).
- `verify-review-result.mjs` binds `source_sha`/`source_tree` to the admitted
  candidate and rejects `reviewer_id == source_executor_id` (proof suite line
  108, `same-reviewer`), non-zero findings, failed disposition/evidence, wrong
  profile, and schema drift.
- Full hostile suite passes: `phase20-proof-script-tests-ok`.

**Disposition:** REJECTED as **out of scope for non-forgeable mechanical closure
per the accepted trust boundary**; the residual (one same-UID actor authoring
both source and review) is inherent, because any git-author binding is forgeable
under same-UID and the non-forgeable path (external signer) is explicitly out of
scope. The gate is already at its achievable maximum: commit-topology separation
(`verify-review-pair.sh`, hostile-tested) + label distinctness + exact-candidate
binding. No verifier change is made, because a git-author binding would add
apparent rigor without real assurance (theater).

**Decision authority:** Made under Sean's standing delegation ("make the best
decisions but cross-audit every decision and continue") and **confirmed by
independent cross-audit** (which validated the REJECT conclusion and corrected
fact 2 above). Not self-blocking. Sean may override on return by either accepting
this rejection or amending `PROJECT.md` scope to require an external-signer
identity source (a milestone scope change, not a Phase 20 gate fix).

---

## M2 — Stale model bake (MEDIUM) — REJECTED (immaterial)

**Finding:** Installed agents are older than `.planning/config.json`, so requested
model overrides are "not baked" for the static-frontmatter `codex` runtime.

**Counter-evidence (reproducible against the clone):**

1. Config declares **zero** model overrides — there is nothing to bake:
   ```
   $ node -e 'const c=require("./.planning/config.json"); \
       console.log({model_overrides:c.model_overrides, models:c.models, \
                    model_profile:c.model_profile})'
   { model_overrides: null, models: null, model_profile: undefined }
   ```
2. Model resolution returns a concrete, usable model for every executor
   agent-type with no bake step:
   ```
   $ ferrox-tools resolve-model executor --cwd <clone>
   { "model": "sonnet", "profile": "balanced", "effort": "high" }
   ```
3. The runtime warning literally reads *"ignores the new model_overrides"* — but
   the override set is empty (step 1), so it ignores nothing. The trigger is an
   mtime heuristic: the clone's freshly-written `config.json` is newer than the
   global agent bake, not an actual pending model change.

**Conclusion:** With zero overrides configured, a re-bake changes no model
selection; it would only refresh frontmatter timestamps. The finding is
immaterial to this candidate. Should real `model_overrides` be added later, a
re-bake becomes required — tracked as a config-change precondition, not an open
Phase 20 blocker.

---

## H0 — Stale plan counter (housekeeping) — FIXED

`.planning/STATE.md` was corrected from a 14-plan graph to the real 18-plan graph
(`total_plans: 18`, `Plan: 3 of 18`, `11%`, and the three "fourteen"→"eighteen"
prose references). Ferrox's dynamic `progress` already computed 18/2/11%; this
aligns the on-disk text.

---

## Security audit (B3 — independent prosecution of the gate scripts)

An independent all-severity audit prosecuted `verify-review-result.mjs`,
`verify-review-pair.sh`, `verify-task-scope.sh`, `verify-f20-03-scope.sh`, and
`task-base-authority.mjs`. It confirmed the large majority of the attack surface
DEFENDED (abbreviated OIDs, generation-ID forgery, symlink/FIFO/permission drift
on authority objects, replay/substitution, concurrent-publication races,
source-mutation smuggling through the review metadata chain, self-authorizing
plan scope, TOCTOU, glob injection). Three findings surfaced; all dispositioned
below to zero.

### S1 — MEDIUM — F20-03 scope gate accepted a file→symlink type change — FIXED

`verify-f20-03-scope.sh` guarded structural mutation with `awk '$1 ~ /^[DR]/'`,
which catches delete/rename but **not** a type change (`T`, regular file →
symlink, mode `120000`). An in-scope path swapped to a symlink stayed inside the
41-path canonical set, was neither D nor R, and landed past the gate — a
downstream clean-checkout build on the proof host would then follow it to
out-of-tree content.

**Fix:** (a) the destructive guard now fails closed on any status that is not a
plain add/modify (`$1 !~ /^[AM]/` — rejects D, R, C, **T**); (b) a tree-mode
assertion requires every changed in-scope path to be a regular-file blob
(`100644`/`100755`), which also rejects a *new* in-scope path added directly as a
symlink (`120000`) or gitlink (`160000`), which would surface as an add.

**Proof:** negative control on the real exploit — old guard returns empty (would
accept), new guard catches the `T` line, mode assertion rejects `120000`. A
durable regression test (`f20-03-symlink-swap`) is added to
`test-phase20-proof-scripts.sh`; full suite passes.

### S2 — LOW — scope verifiers blind to gitignored untracked files — REJECTED (bounded by trust model)

`git ls-files --others --exclude-standard` hides untracked paths matching
`.gitignore` (e.g. `.env`, `.claude/`, `target/`). An actor could drop an
agent-consumable file (e.g. `.claude/settings.json`) into the standalone clone
undetected by the scope gates.

**Disposition:** REJECTED as bounded by the accepted trust model. The injector
must be a **same-UID local actor** writing into the clone's working tree —
exactly the capability `PROJECT.md`:63 places out of scope. Such files **cannot
enter the committed candidate** (they can't be `git add`-ed without becoming
tracked and visible to the gate), so this is coordinator-side local side effects,
not a source-into-candidate smuggle. A naive "fix" (enumerate all ignored files)
would flag legitimate `target/` build artifacts and break normal execution — a
worse cure. Residual is inherent to same-UID local execution.

### S3 — LOW — review-result trusted an unchecked (source_sha, source_tree) pair — FIXED

`verify-review-result.mjs` checked the review JSON's `source_tree` against the
CLI `sourceTree` argument but never verified that argument equals
`source_sha^{tree}`. Closed by the caller in current wiring, but a latent hazard.

**Fix:** the verifier now asserts `git rev-parse sourceSha^{tree} === sourceTree`
before reading the review blob. **Proof:** a regression test supplies a
matching-but-wrong tree (review JSON matches the bogus tree, so the pre-existing
equality check passes) and confirms the verifier now rejects it; full suite
passes.
