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
| 3 | rollback rehearsal not in CI | see the correction below | **TRUE** (my first read was wrong) |
| 4 | reproducibility accidental, no `--remap-path-prefix` | confirmed | **TRUE — real work** |

### Claim 3 — a drill IS wired into CI

`git grep -n release-manifest-drill` → `.github/workflows/ci.yml:755` invokes
`.github/scripts/release-manifest-drill.sh` on every push, `if: ${{ !cancelled() }}`.
`.github/scripts/release-manifest-drill.sh:127` contains
`mint rollback "${acceptance_seed}" ...`. Concept-searched `rollback|rehears` over `.github/`
and `justfile`: exactly 1 hit, that mint call.

**CORRECTION, T+40min — I was wrong, and the brief was right.** I initially graded this
"PARTLY FALSE" on the keyword hit. Reading the drill shows that case is
`mint rollback … "v${OLDER_VERSION}-wayland-base"` — **a genuine OLDER release that the
updater must REFUSE**. That is rollback *protection* (downgrade refusal), which is a
different property from rollback *rehearsal* (the third of the four release authorization
states, `ReleaseState::RollbackRehearsal`). Two distinct concepts, one word.

This is the exact failure §3b-i describes, inverted: I nearly published a **refutation**
manufactured out of a keyword match, when the substantive claim was correct. Re-measured on
the concept rather than the word:

- `state-append` / `state-verify` in `.github/workflows/`: **ZERO** (rc=1, no match)
- `environment:` in `.github/workflows/`: **ZERO** (rc=1, no match)
- known-positive in the same instrument: `cargo` in `release.yml` → **7**

So the four-state ledger existed and **nothing executed it**. Claim 3 is TRUE.

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
as an *unmet* limit). One of the two is wrong.

**RESOLVED — the ci.yml comment is right and both my brief and the Phase 29 verdict are
stale.** Measured directly at the source of truth, `crates/wcore-cli/src/update_trust.rs:82`:

```
pub const RELEASE_TRUST_ROOT_JSON: &str = r#"{"schema":"wayland.release.trust-root",
"schema_version":1,"keys":[{"key_id":"release-acceptance-key",
"public_key_base64":"ycwkW1xZnCxruh59zJnQiuoN5xuXYkMurhquhHMBXXY=",
"role":"release_acceptance","valid_from":0,"retired_at":null}]}"#;
```

The keys array is **populated**, not empty, and `update_trust.rs:1117` now asserts
`!RELEASE_TRUST_ROOT_JSON.contains("\"keys\":[]")` — a test that would fail if anyone
reverted it. This is a PUBLIC key, which is what belongs in a bundled trust root; no secret
is in the tree. **`F29-LIMIT-01` is DISCHARGED.**

`F29-LIMIT-02` is discharged too. `release.yml` now derives a manifest sequence from every
previously published manifest (lines 328-374), builds and signs
`wayland-core-<tag>-release-manifest.json` with the seed on **stdin only** (376-436),
verifies it against the trust root **extracted from the shipped binary's own constant**
(445-456), and asserts exactly one manifest is present before publishing (462-477). A guard
at 228-244 makes the whole thing fail SOFT with a warning when the CI secret is absent.

So **open HIGH F29-03-01 ("self-update installs nothing") is structurally closed** —
both of its two named preconditions now exist. What is NOT proven, and what I am not going
to claim, is that a real release has gone through it: that needs a tag push, which is
Sean's alone.

## T+35min — trim-paths is NOT available, so remap-path-prefix is the only lever

Probed on hetzner with a throwaway crate rather than assumed:

```
error: failed to parse manifest ...
Caused by: feature `trim-paths` is required
  The package requires the Cargo feature called `trim-paths`, but that feature is not
  stabilized in this version of Cargo (1.96.0 (30a34c682 2026-05-25)).
```

Two things follow. First, `[profile.release] trim-paths` cannot be used — the clean fix is
unavailable and `--remap-path-prefix` is the only stable mechanism. Second, and separately
useful: the ambient default toolchain on hetzner is **1.96.0** while the repo's pinned
toolchain resolves to **1.95.0** (`cargo --version` inside the worktree). Anything measured
with a bare `cargo` outside a repo tree is on a different compiler than the build.

Cargo's own docs confirm `trim-paths` would not have been a complete fix anyway: it exposes
`CARGO_TRIM_PATHS` **for build scripts to honour voluntarily**, and the variance here is
cranelift's build-script `OUT_DIR` reaching the binary through `file!()` in generated
sources — precisely the case a build script has to opt into.

## T+45min — first-hand results (not citations)

### The manifest drill, re-run independently at my commit on hetzner

```
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
retired root refused as required: key is retired: release-acceptance-key
NEGATIVE CONTROL: test result: FAILED. 1 passed; 9 failed; 0 ignored; 0 filtered out
DRILL PASSED: 10 tests executed against a CLI-minted corpus,
and 9 failed when the corpus was deliberately broken.
```

Both directions, first-hand. Note the `0 ignored; 0 filtered out` fields are intact because
I read the log **from a file with the Read tool** — through Bash the `rtk` proxy strips
exactly those two fields (§3b).

### Reproducibility arm A (control, no remap) — the variance reproduces

Two clean release builds of `-p wcore-cli` from the same commit `2329be9b`, at
**different-length** absolute paths (a stronger test than 29-02's equal-length `a1`/`a2`,
because equal-length substitution can mask a size-sensitive effect):

```
/root/wl-s2934/alpha                   -> e7264f4c892dd40c4791b4c0371b2f92251f8856ca322320f370e29eb56ebbe2
/root/wl-s2934/beta-considerably-longer-> b818148813031d78f1091b487d516283b27cf893d24f35678f5926d31f70c240
```

DIFFERENT, as F29-REPRO-VARIANCE predicted. This is the known-negative: the experiment can
detect non-reproducibility. Arm B (same two paths, with `--remap-path-prefix`) is running.
