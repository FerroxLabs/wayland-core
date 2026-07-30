# SUPPLY-29-34 — lane notes (append-only, committed continuously)

Lane `supply-29-34`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-supply-29-34`,
branch `lane/supply-29-34`, base integration `b2ddf113`.

Brief: "build plans 29-03 and 29-04, the unbuilt three-fifths of the release supply chain."

---

## T+10min — THE BRIEF'S CENTRAL PREMISE IS FALSE. 29-03 AND 29-04 ARE BUILT.

LANE-BRIEF §"Your brief's MEASUREMENTS are probably stale — re-verify every one before acting"
applies in full. Measured at `b2ddf113`, all via `/usr/bin/git`:

`git ls-files .planning/phases/29-supply-chain-release-integrity/` returns 16 top-level
documents and 96 evidence files, including:

- `29-03-PLAN.md` (65.3K), `29-03-SUMMARY.md` (16.1K), `29-03-UPDATE-TRUST-RESULTS.md` (13.2K)
- `29-04-PLAN.md` (58.3K), `29-04-SUMMARY.md` (12.8K), `29-04-TAMPER-RESULTS.md` (19.7K)
- `29-PHASE-VERDICT.md` (20.4K)
- `evidence/29-03/` — 13 files incl. `CLAUSE-LEDGER.tsv`, `LIVE-LEDGER.tsv`,
  `REAL-KEY-LIMITS.tsv`, `rotation-drill.txt`, `mutation-drill.txt`, `live-downgrade.txt`
- `evidence/29-04/` — 26 files incl. `TAMPER-LEDGER.tsv`, `STATE-SEPARATION.tsv`,
  `COLLAPSE-ATTEMPTS.tsv`, `PHASE-LIMITS.tsv`, `VERDICT-LEDGER.tsv`, 7 `collapse-*.txt`
- `evidence/29-03-windows/` — a Windows downgrade-refusal leg with `RESULT.md`

Commits that landed them (`git log --oneline -- <paths>`):

```
195a856f Merge lane/windows-requeue: Phase 28 C2 soak MET, KR-01 misattributed
a5a9643f measure(29-03): the Windows downgrade refusal RAN - MET, matches Linux on every clause
7be28079 docs(29-04): summary — the goal was not achieved, and that is the answer
01c5b507 docs(29-04): tamper results, the Phase 29 verdict, and the evidence ledgers
b8d349c2 evidence(29-03): the fence gates are vacuous as written - re-run against the merge base
92258e5a docs(29-03): grade F29-02 clause by clause, with the evidence that settled each
```

**So there is no 29-03/29-04 to build.** The `COMPETITIVE-LEDGER.md` SUPPLY row is itself
stale: its "Next action" column still reads "Plans 29-03 … and 29-04" and its limitation
column still says "Three of the family's five named members … are entirely unbuilt." Both
were written on 2026-07-28 and the plans landed after.

My job therefore converts to: (a) report the refutation with evidence, (b) verify the
*substance* of what landed rather than its existence, (c) execute the parts of my brief that
measurement shows are genuinely still open.

## T+12min — which of my brief's four numbered claims survive

| # | Brief claim | Measured at `b2ddf113` | Verdict |
|---|---|---|---|
| 1 | 29-03 unbuilt | 3 documents + 13 evidence files + a Windows leg | **FALSE** |
| 2 | 29-04 unbuilt | 3 documents + 26 evidence files | **FALSE** |
| 3 | rollback rehearsal not in CI | needs precision — see below | **PARTLY FALSE** |
| 4 | reproducibility accidental, no `--remap-path-prefix` | confirmed | **TRUE — real work** |

### Claim 3 — a drill IS wired into CI

`git grep -n release-manifest-drill` → `.github/workflows/ci.yml:755` invokes
`.github/scripts/release-manifest-drill.sh` on every push, `if: ${{ !cancelled() }}`.
`.github/scripts/release-manifest-drill.sh:127` contains
`mint rollback "${acceptance_seed}" ...` — so a *rollback-rehearsal state record* is minted
in CI today. The brief's "rollback rehearsal does not exist anywhere in CI" is therefore not
literally true. Whether it is a *rehearsal* (does it exercise reverting to a prior version?)
or merely a *ledger state named "rollback"* is the question to settle. Concept-searched
`rollback|rehears` over `.github/` and `justfile`: exactly 1 hit, the mint call.

### Claim 4 — `--remap-path-prefix` genuinely absent from all build configuration

`git grep -n -- "remap-path-prefix"` returns **2** hits, both prose in `.planning/`:
- `29-02-CLEANROOM-RESULTS.md:189` — "**Remedy, not applied** (release.yml is outside this fence)"
- `evidence/29-02/repro-variance-class.txt:74` — the same remedy as a suggestion

Zero hits in `.cargo/config.toml`, `.github/workflows/`, `justfile`, or any `Cargo.toml`.
`.cargo/config.toml` contains only an `[env] RUST_MIN_STACK` block. `git grep RUSTFLAGS`
returns 3 hits, none of them a build configuration (two are hetzner shell notes, one is the
prose above). **Instrument alive-check in the same invocation:** `git grep -c cargo --
.github/workflows/release.yml` → **7**. So the zeros are real zeros.

Note the zsh trap fired here on the first attempt: `grep -rn ... --include=*` was eaten by
zsh (`no matches found: --include=*`) and printed "(none)" for remap-path-prefix — a **false
negative that agreed with my brief**. Caught only by re-running through `git grep`. This is
LANE-BRIEF §3b-i exactly: an absence claim is self-passing on a dead instrument.

## T+15min — a contradiction that outranks everything else in this lane

`.github/workflows/ci.yml:713-714` states, in a comment:

> "The real FerroxLabs trust root was substituted into `RELEASE_TRUST_ROOT_JSON`, replacing
> the placeholder that made `self-update` fail closed"

This **directly contradicts** both my brief ("no real trust root is bound", "binding a real
trust root is Sean's alone") and `29-PHASE-VERDICT.md`'s `F29-LIMIT-01` ("The real
FerroxLabs release trust root replaces the empty `keys` array in `update_trust.rs`" — listed
as an *unmet* limit). One of the two is wrong. Verifying next, against
`crates/wcore-cli/src/.../update_trust.rs` directly, not against either document.
