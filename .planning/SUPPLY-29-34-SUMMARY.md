# SUPPLY-29-34 — lane summary

Branch `lane/supply-29-34`, based at integration `b2ddf113`.
Evidence: `.planning/phases/29-supply-chain-release-integrity/evidence/29-34/`.
Running notes: `.planning/SUPPLY-29-34-NOTES.md`.

---

## The headline: I was sent to build 29-03 and 29-04. They were already built.

**Two of my brief's four claims are false at integration head, and so is the
`SUPPLY-*` ledger row that generated them.** Measured with `/usr/bin/git`, not inferred:

| Brief claim | Measured at `b2ddf113` | Verdict |
|---|---|---|
| 29-03 (update identity, revocation, rotation) unbuilt | `29-03-PLAN.md`, `29-03-SUMMARY.md`, `29-03-UPDATE-TRUST-RESULTS.md`, 13 evidence files, a Windows leg | **FALSE** |
| 29-04 (tamper corpus, four-state separation, verdict) unbuilt | `29-04-PLAN.md`, `29-04-SUMMARY.md`, `29-04-TAMPER-RESULTS.md`, `29-PHASE-VERDICT.md`, 26 evidence files | **FALSE** |
| rollback rehearsal absent from CI | correct — see below | **TRUE** |
| reproducibility only accidental | correct | **TRUE** |

Landed by commits `92258e5a`, `b8d349c2`, `01c5b507`, `7be28079`, `a5a9643f`. The ledger's
"Next action" column still reads *"Plans 29-03 … and 29-04"* and its limitation column still
says *"Three of the family's five named members … are entirely unbuilt."* Both were written
2026-07-28; the plans landed after. **The ledger row needs correcting — I have not edited it,
because it is a heavily shared file and this is the orchestrator's to serialize.**

### And the trust root IS bound — which contradicts my brief AND the Phase 29 verdict

My brief says *"no real trust root is bound"* and *"binding a real trust root is Sean's
alone."* `29-PHASE-VERDICT.md` lists `F29-LIMIT-01` as unmet. `.github/workflows/ci.yml:713`
says the opposite. I checked the source of truth rather than either document:

`crates/wcore-cli/src/update_trust.rs:82` ships a **populated** `keys` array — key id
`release-acceptance-key`, role `release_acceptance`, a real base64 public key — and line 1117
now asserts `!RELEASE_TRUST_ROOT_JSON.contains("\"keys\":[]")`, so a revert would fail a test.
A public key is exactly what belongs in a bundled trust root; no secret is in the tree.

`F29-LIMIT-02` is discharged too: `release.yml` derives a sequence from every previously
published manifest (328-374), builds and signs the manifest with the seed on **stdin only**
(376-436), verifies it against the trust root **extracted from the shipped binary's own
constant** (445-456), and asserts exactly one manifest before publishing (462-477).

**So open HIGH `F29-03-01` ("self-update installs nothing") is structurally closed** — both of
its named preconditions now exist. What I will **not** claim: that any real release has gone
through it. That needs a tag push, which is Sean's.

**I was explicitly instructed to state that the trust root is unbound. It is bound, so I am
not going to state that.** The instruction was correct when written and is now stale.

---

## What I built

### 1. Rollback rehearsal now runs in CI (F29-04-03)

`.github/scripts/release-state-drill.sh` + `.github/workflows/release-rehearsal.yml`.

The four-state ledger existed and **nothing executed it**: `state-append`/`state-verify` and
`environment:` each return **zero** across `.github/workflows/`, against a live known-positive
of 7 `cargo` hits in `release.yml`.

I nearly filed a false refutation here. `rollback` does appear once in `.github/` — but it is
the manifest drill's *downgrade-refusal* case, which is rollback **protection**, a different
property from rollback **rehearsal**. Re-measured on the concept rather than the word, the
brief was right and my first read was wrong.

Validated live on hetzner before wiring, **both directions**:

```
CHAIN VERIFIED highest_state=rollback_rehearsal records=3 accepted=false
ceiling  : release acceptance requires an observed certification binding
refused  : key release-acceptance-key is bound to role ReleaseAcceptance but role
           RollbackRehearsal is required
refused  : record at position 1 is RollbackRehearsal but canonical order requires
           DeploymentPreparation
refused  : body digest mismatch
restore  : CHAIN VERIFIED highest_state=rollback_rehearsal records=3 accepted=false
```

The restore step is the point: without re-proving the positive control *after* the negatives,
three refusals are equally consistent with a verifier that had simply stopped working.

**I fixed a control that passed for the wrong reason.** Negative control 1 originally signed
with the *packaging* key and was refused — by the key-reuse rule, before ever reaching the role
check. A pass for a property it never tested. Switched to the release-acceptance key, which has
signed nothing in the chain; the refusal message changed to a genuine role mismatch.

**Scope stated, not implied:** this rehearses the rollback *authorization state*. It does not
rehearse an operational rollback, because the product has no rollback command — the updater
deliberately refuses downgrades. Whether an operational rehearsal should exist is a product
question I did not answer and do not pretend to.

### 2. Reproducibility is now deliberate — proven, with one third of the recipe landed

Full detail: `evidence/29-34/REPRO-DELIBERATE.txt`. Five arms, two clean release builds of the
shipped `-p wcore-cli` binary each, at **different-length** absolute paths (stronger than
29-02's equal-length pair).

| Arm | Recipe | Digests |
|---|---|---|
| A | control, no remap | **DIFFER** — 8 embedded paths, 1619 registry paths |
| B | `--remap-path-prefix` | **DIFFER** — 7 of 8 gone, all 1619 registry gone, **1 survivor** |
| C | + gate one `CARGO_MANIFEST_DIR` site | **DIFFER** — no measurable change |
| D | + gate both sites | **DIFFER** by 48 bytes — **0 embedded paths** |
| E | + `-Cstrip=symbols` | **BYTE-IDENTICAL** `406860899137222a…` |

**29-02's filed remedy was necessary but not sufficient, and that is the finding.** It
identified only cranelift's `OUT_DIR` class and filed `--remap-path-prefix`. Had that been
applied and believed, the release would have been graded REPRODUCED while still not being
reproducible — because a second, unidentified class survives it: `env!("CARGO_MANIFEST_DIR")`
in shipped `wcore-cli` code, which expands to a **string literal**. Cargo documents that path
sanitizing "does not affect hard-coded paths within source code strings", so no remap can ever
reach it. `trim-paths` is not an alternative — probed, not assumed: *"feature `trim-paths` is
required … not stabilized in this version of Cargo (1.96.0)"*, and it would only have exposed
`CARGO_TRIM_PATHS` for build scripts to honour voluntarily.

Arm C is a **recorded negative result**: gating one of the two sites changed nothing, because
both expand to the same deduplicated literal. The diff looked right and accomplished nothing.

**Landed:** the two `#[cfg(debug_assertions)]` gates (`5fa47abd`). Zero behavioural cost —
both branches are already no-ops for an installed binary, and `cargo test -p wcore-cli --lib
plugin::scaffold` is **5 passed, 0 failed** at the fix, identical to the pre-fix baseline,
including `both_shipped_templates_resolve`.

**Not landed, with reasons rather than silence:**
- **`--remap-path-prefix` in `release.yml`.** Free of behavioural cost and worth having. Not
  wired because `release.yml` builds linux-aarch64 through `cross`, which compiles inside a
  container mounting the source at a *different* path, so a `$GITHUB_WORKSPACE`-based prefix
  would silently fail to match on that one leg while looking correct on the others — a gate
  that passes without measuring. Needs a per-leg prefix and a container check I cannot run here.
- **`-Cstrip=symbols`.** Removes `.symtab`, so production backtraces lose function names. The
  profile deliberately ships `strip = "debuginfo"` to keep them. Trading diagnosability for the
  final 48 bytes is a product decision with real cost on both sides, on a shared profile every
  other lane builds against. Not mine to make unilaterally.

### 3. Tamper corpus, re-run independently — both directions

I ran the CI manifest drill myself at my own commit rather than citing 29-04:

```
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
retired root refused as required: key is retired: release-acceptance-key
NEGATIVE CONTROL: test result: FAILED. 1 passed; 9 failed; 0 ignored; 0 filtered out
DRILL PASSED: 10 tests executed against a CLI-minted corpus,
and 9 failed when the corpus was deliberately broken.
```

Corpus: pristine control, rollback, overage, revoked, packaging-signed, tampered, plus a
retired trust root. Ten cases caught, nine turn red on a deliberately broken corpus. **No
skipped cells: `0 ignored; 0 filtered out` on both runs.** Those two fields survive here only
because I read the log from a file with the Read tool — through Bash the `rtk` proxy strips
exactly them.

---

## What remains unproven, and why

**The trust root is bound, but nothing has been released through it.** That is the honest
residual, and it is different from what my brief said. Every proof in Phase 29 and in this lane
uses throwaway Ed25519 keys minted at run time into a temp dir. `release.yml` will only sign
when `WAYLAND_RELEASE_ACCEPTANCE_SEED` is set, and fails soft with a warning otherwise.

So: **key rotation and revocation are proven as mechanisms and are NOT proven for the product.**
A rotation drill against a disposable key demonstrates the code path; it does not demonstrate
that the real key can be rotated, that anyone can operate the procedure, or that a revocation
would reach users. `F29-LIMIT-04` through `-08` remain untouched by this lane. No release was
published, tagged or triggered.

Also unproven: whether the reproducibility recipe holds on macOS, Windows, or the aarch64
`cross` leg. Measured on x86_64 Linux only.

---

## Fences and boundaries

- **Did not touch** `.cargo/audit.toml`, `deny.toml`, or `supply-chain.yml` — the sibling
  advisories lane owns `F29-02-H1` and the deny RED disposition. Added a **new** workflow file
  instead of editing shared ones, following `supply-chain.yml`'s own recorded reasoning.
- **Did not touch** `wcore-cli/src/lib.rs` or `main.rs` (the §6 fence).
- **Did not** publish, tag, release, open a PR, merge, or close an issue.
- **Did not** run `wcore-contract generate`, `git rebase`, `git reset --hard`, or `git clean`.
- **Did not** edit `COMPETITIVE-LEDGER.md` despite it being provably stale — shared file.
- One production source file changed: `crates/wcore-cli/src/plugin/scaffold.rs`.
- `cargo fmt --all -- --check` clean. Full clippy/nextest **not** run — flagged below.

## Open items for the orchestrator

1. **`SUPPLY-*` ledger row is stale on three counts** — 29-03/29-04 built, `F29-LIMIT-01`
   and `-02` discharged, `F29-03-01` structurally closed.
2. **`29-PHASE-VERDICT.md` is stale** on `F29-LIMIT-01`/`-02`. Criterion 2 may re-grade.
3. **Two decisions I declined to take unilaterally:** wiring `--remap-path-prefix` into
   `release.yml` (needs the `cross` per-leg fix), and `strip = "symbols"` (diagnosability cost).
4. **Not run: full workspace clippy/nextest.** My change is two `#[cfg]` gates in one file and
   the targeted suite is green, but the workspace gate belongs to the merge.
