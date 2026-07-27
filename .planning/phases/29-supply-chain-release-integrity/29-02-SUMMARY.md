---
phase: 29-supply-chain-release-integrity
plan: "02"
subsystem: supply-chain / SBOM, dependency policy, reproducibility
status: complete
termination_state: 2 (Complete with a red policy verdict)
requirements: [F29-01]
requirements_marked_complete: none — closure is claimed by 29-04
branch: lane/29-02
base_sha: 2fd771d2cd69e13f5c686f859547ab46ebddd41f
tags: [sbom, cyclonedx, cargo-deny, reproducibility, determinism, manifest-binding, seam-request]
---

# Phase 29 Plan 02: SBOM, Dependency Policy and Reproducibility — Summary

Turned three declared-but-unmeasured supply-chain properties into measured, bound evidence: a
byte-deterministic CycloneDX SBOM derived from the locked graph, the dependency policy that had
never once executed now executing with its real (red) verdict recorded, and reproducibility
measured for the first time and honestly graded as a documented variance with a named class —
all three bound into a manifest signed by a throwaway key, whose verification demonstrably
breaks when one bit of the SBOM changes.

**Termination state: 2 — Complete with a red policy verdict.** The plan names that state
explicitly as "a complete and successful outcome". No fourth state was invented, no plan was
spawned, and `deny.toml` was not weakened by a single character.

---

## Verdict against each success criterion

| # | Criterion | Verdict |
|---|---|---|
| 1 | SBOM is a pure, byte-deterministic function of the locked graph, proved by regeneration and a pinned fixture digest | **MET** — and proved *cross-path*, which caught a real defect |
| 2 | A package with no declared licence is explicit rather than absent | **MET** — 2 of 865, both present with `NOASSERTION` |
| 3 | The policy in `deny.toml` executes from both a developer command and a CI workflow | **MET** — baseline was zero sites; now `justfile` + `supply-chain.yml` |
| 4 | The real verdict captured verbatim with exit status, truthfully; `deny.toml` unweakened | **MET** — `FAIL, exit 5`, recorded; policy untouched in all four directions |
| 5 | `deny` chained into `check-all` **iff** the verdict is PASS | **MET** — verdict is FAIL, so `check-all` is untouched; gate asserts both agree |
| 6 | New workflow: new file, parses, explicit permissions, no secret, always-reports | **MET** |
| 7 | Reproducibility measured with both digests; verdict consistent with them; class named | **MET** — DOCUMENTED-VARIANCE, class `path_prefix`, identified single-variable |
| 8 | All three results bound into a manifest signed by a throwaway key and verified against an independent trust root | **MET** |
| 9 | One-byte SBOM mutation breaks verification, pristine control accepted first | **MET** — pristine exit 0, spliced exit 1 |
| 10 | Eight F29-01 clauses mapped to measurement, platform and artifact, pre-existing coverage *named* | **MET** — 11 ledger rows, every named file exists |
| 11 | No real key, account, token or published release used, and no gate required one | **MET** |

**One HIGH finding is OPEN and was NOT fixed by me** — see §4. That is the honest headline
alongside the eleven MET rows.

---

## What landed

| Artifact | What it is |
|---|---|
| `crates/wcore-eval-scenarios/src/sbom.rs` | The pure CycloneDX transform. No clock, no randomness, no env read, no filesystem access. Adds **no dependency**. |
| `crates/wcore-eval-scenarios/tests/sbom_contract.rs` | 12 contract tests — the 5 named behaviours plus 7 more; 3 further unit tests live in `sbom.rs` (+15 total, matching 310 → 325) |
| `crates/wcore-fixture-harness/fixtures/f29/` | Pinned offline corpus: `cargo-metadata.json` (2,649 B, `8ec20514…`), `expected-sbom.json` (2,809 B, `28f7c053…`), `MANIFEST.tsv` |
| `justfile` | `deny` recipe beside `audit`. **`check-all` untouched.** |
| `.github/workflows/supply-chain.yml` | New file, 2 jobs, 9 steps, `permissions: contents: read` |
| `bin/wayland-release.rs` | `sbom` subcommand; `manifest-build` extended to bind all three results |
| `29-02-CLEANROOM-RESULTS.md` + `evidence/29-02/` (10 files) | Every measurement |
| `.planning/SEAM-REQUESTS/29.md` | SR-29-6, SR-29-7 (both HIGH, both escalations) |

---

## RED-before-GREEN: what I actually did

**Taken genuinely, twice, and the second one mattered.**

**RED #1** (commit `eb40ccd2`): the contract suite landed *with* a deliberately naive transform —
one that stamps a wall-clock timestamp, preserves input order, drops packages with no licence,
and copies the cargo package id (which embeds an absolute path) into the document. That is what
a careless implementation looks like, and the suite reported **`11 tests run: 0 passed, 11
failed`** — every test then existing, each for its own distinct reason (timestamp present,
component dropped, id leaked, no sort, no duplicate check, no source guard). GREEN at `7ef04ea8`.
(The 12th contract test did not exist yet; it was added by RED #2 below.)

I am stating plainly that this baseline was *authored by me to fail*, rather than claiming a
per-behaviour test-first cycle I did not run. It is a real RED against a realistic implementation,
not an empty stub, but it is not the same thing as discovering each behaviour incrementally.

**RED #2 — a real defect found by measurement, not by reading code.** The first implementation
derived `serialNumber` from a digest of the **raw** `cargo metadata` text. Everything passed: the
contract suite, the pinned fixture, same-directory regeneration. Then generating from the same
commit checked out at two different paths produced documents of identical length differing at
**byte 83** — the serial, because the raw text embeds the checkout path **1,538 times**. A second
party regenerating the SBOM from the same source would have got a different digest, and the whole
manifest binding would have been worthless.

Test written first (`b946474a`), confirmed RED with **exactly 1 of 12 failing** — the other 11
still green, so it isolates the defect. Fixed by deriving the serial from the canonical output
(`5b2fce2e`). Regression-locked by
`the_serial_number_is_derived_from_the_canonical_output_not_the_raw_input`.

---

## Gate results

**Local (Mac, `cargo fmt --all -- --check` only): 12/12 PASS** — fmt; module landed and declared;
no clock/randomness/env in the generator; CycloneDX marker present where the baseline was zero;
all five named test names; pinned fixture ≥2 entries; cargo-deny referenced at 2 sites where the
baseline was zero; workflow exists with an explicit `permissions:` block; workflow parses under a
real YAML parser with job-and-steps structure; `deny.toml` unweakened in four directions; verdict
recorded with a singular machine-parseable status line; recorded verdict and `check-all` agree.

**Task 3 local gates: 5/5 PASS** — repro verdict consistent with its digests; variance-class file
substantial; pristine exit 0 **and** mutated non-zero; 11 clause rows with every named file
present and non-empty; surgical diff across `crates/` clean.

**Authoritative (Hetzner Linux, `lane/29-02` @ `666244f9`), remote status captured into `rc`
before any filtering, no `ssh` line carrying a pipe:**
- `cargo clippy -p wcore-eval-scenarios --all-targets -- -D warnings` — **clean, rc=0**
- `cargo nextest run -p wcore-eval-scenarios --no-fail-fast` — **325 run, 325 passed, 0 failed,
  5 skipped.** Delta vs base 310: **+15 tests, +0 failures.** No residual failure to attribute.
- `cargo build --release --locked` + `wayland-release --help` — rc=0, `sbom` subcommand live
- `cargo deny --version` + full policy run — rc=5 (the verdict; recorded, not forced green)

**Two gate-authoring traps I hit and closed, worth passing on:**
1. **`rtk`'s git wrapper silently drops merge commits from `git log`.** `git rev-parse HEAD` and
   `git log --oneline` disagreed on my own HEAD. Every git command in this lane used
   `/usr/bin/git` thereafter. A fence gate reading rtk-filtered `git log` would be wrong.
2. **zsh's `nomatch`**: an unquoted `--include=*.rs` inside `eval` is a hard error ("no matches
   found"), not a literal. My first gate harness reported the CycloneDX gate FAIL when the plan's
   exact quoted form passes. The gate was fine; my transcription was not.
3. YAML 1.1 parses a bare `on:` key as boolean `True`; a workflow gate asserting `d['on']` would
   falsely go red.

---

## The measurements

### Dependency policy — first execution in the repo's history

```
advisories FAILED, bans ok, licenses FAILED, sources ok
F29-DENY-STATUS::FAIL::exit=5
```

`cargo-deny 0.20.2`, `deny.toml` sha256 `05a8535b…`, 1,017 crates. Six findings: two quick-xml
vulnerability advisories, three unmaintained, one unlicensed first-party crate. **Not chained
into `check-all`** — three other lanes run that recipe right now.

Two positives recorded as prominently as the gaps: **`sources ok`** independently reconfirms
29-01's F29-CEN-19 refutation (zero git sources) from a different tool, and **`bans ok`**.

### SBOM

865 components, 447,364 bytes, sha256 `5028fe28…`. Cross-path byte-identical from *differing*
metadata inputs (1,538 path occurrences each). Zero `file://` leaks, zero `manifest_path`, no
timestamp key, purl-sorted, 2 explicit-unknown licences, 0 git origins.

### Reproducibility

```
F29-REPRO::DOCUMENTED-VARIANCE::a=ca35c34f…::b=8272fae8…
```

| Observation | Varied | Result |
|---|---|---|
| A — two clean target dirs, **different** paths | the path | `ca35c34f…` vs `d6cd3154…` — **DIFFER** |
| B — one target dir path, **wiped between** | nothing | `8272fae8…` twice — **IDENTICAL** |

Class: **`path_prefix`**. Mechanism located: seven `OUT_DIR` paths from `cranelift-codegen`'s
build script (via wasmtime) reach the binary through `file!()` in ISLE-generated sources. Each
build took 320s and exited 0 — no cache hits, and B *wipes* between builds so its agreement
cannot come from reuse. Disk 919G free before the run.

**The useful conclusion: the shipped release is reproducible in practice but only
*accidentally*** — GitHub runners always check out at the same absolute path, so `release.yml`
holds the varying input constant without intending to.

### Binding and refusal

Pristine `exit=0` recorded **before** the mutation; one bit flipped (`cmp -l` = **1** byte),
`body_sha256` moved `00d5294c…` → `2e9ced38…`, spliced manifest refused with
`body digest mismatch`, `exit=1`.

---

## 4. The HIGH finding I did NOT fix — F29-02-H1

Wiring the policy surfaced something no source read had: `.cargo/audit.toml` silences
`RUSTSEC-2026-0194` / `0195` on a stated **"sole path"** through syntect's embedded dumps. The
real graph has **three** consumer paths; the two through `wcore-tools` — which reads
**user-supplied** docx/pptx/xlsx — are absent from the justification. calamine 0.26.1 has 25
`.attributes()` sites and zero `with_checks(false)`, so **0194 is reachable**. (**0195 is not** —
`NsReader` appears nowhere; that half of the claim holds.)

**Severity HIGH, on the control failure rather than the exploitability.** Cross-audit panel:
codex 5.6 **HIGH**, kimi K3 **HIGH**, gemini 3.1 Pro **MEDIUM** — 2–1. Gemini's dissent is
recorded in full in SR-29-6 rather than dropped.

**I did not fix it, and the reason is a fence, not a judgement.** The repair lives in
`.cargo/audit.toml`, `.github/osv-scanner.toml` and possibly `crates/wcore-tools/` — none in this
plan's `files_modified`, and its surgical-diff gate would reject them. Escalated as **SR-29-6**
per the plan's termination state 3. Under the amended rules a HIGH must be fixed or disproved
before the phase closes; **this one is open**, and 29-04 or Sean must dispose of it.

Likewise **SR-29-7**: 29-01's census assigns **F29-CEN-06** (release path installs `cross` from
unpinned git HEAD) to 29-02, but 29-02's own scope fence forbids touching `release.yml` and names
it an escalation trigger. The assignment and the fence contradict each other. Recorded, not
silently dropped.

---

## Deviations from the plan

1. **Gate paths retargeted to the lane worktree.** The plan's gates `cd` to
   `/Users/seandonahoe/dev/waylandcore-ferrox`; all work is in
   `.../waylandcore-frontier-worktrees/lane-29-02` per LANE-BRIEF §1. Same gates, same repo.
2. **Remote gates run in my own Hetzner worktrees** (`/root/wayland-29-02`, `/root/wl29-pathb`),
   not the plan's `cd /root/wayland`. That directory is shared; `git checkout --detach` there
   would have yanked the tree out from under another lane mid-build.
3. **A second Hetzner worktree was added deliberately.** The plan asks for two runs; two runs *at
   different paths* is a strictly stronger test, and it is the one that found the serial defect.
4. **The reproducibility measurement uses `--locked`; `release.yml` does not** (that is 29-01's
   F29-CEN-02b, LOW). So this measures the locked graph, not literally release.yml's invocation.
5. **`cargo deny check` full output is 21,203 lines**, almost all repeated reverse-dependency
   trees. `deny-verdict.txt` carries the header, the verdict, the complete error and advisory
   set, and the reachability analysis — 224 lines — plus the exact command to reproduce the rest.
   Committing a 21k-line dump into `.planning/` would have been noise, and I am flagging the
   trim rather than implying the file is the raw capture.
6. **Seven extra contract tests** beyond the five named. Additive.
7. **`.planning/SEAM-REQUESTS/29.md` and `.planning/BACKLOG.md` edited** though not in
   `files_modified` — both are directed by the plan's own termination rules.

---

## What I did NOT do

- **Did not touch `deny.toml`** — not one character, in any direction.
- **Did not chain `deny` into `check-all`** — the verdict is red and three lanes run that recipe.
- **Did not fix F29-02-H1 or F29-CEN-06** — both behind the fence; escalated instead.
- **Did not add `publish = false` to `wcore-fixture-harness`** despite it being the correct
  one-line fix for the licence failure — outside `files_modified`, and it would not have changed
  the verdict or the `check-all` decision. Filed as F29-02-L2 with the exact remedy.
- **Did not dispatch the new workflow.** Authored and parsed, never run.
- **Did not run `wcore-contract generate`.** Not needed, not run.
- **Did not touch** `Cargo.toml`, `Cargo.lock`, `ci.yml`, `release.yml`, `.cargo/audit.toml`,
  `.github/osv-scanner.toml`, Phase 28's receipt files, or `crates/wcore-cli/` — all 14 verified
  untouched by a **merge-base** diff (`2fd771d2`), never against the branch name.
- **Did not add a workspace dependency.** The SBOM is derived from `cargo metadata`.

---

## Shared-file edits the orchestrator must serialize against 28-02

**One file, one line, additive, in alphabetical position. No reordering, no reformatting.**

**`crates/wcore-eval-scenarios/src/lib.rs`** — 2 lines inserted between `pub mod runner;` and
`pub mod scenario;`:
```rust
/// Phase 29 deterministic CycloneDX SBOM transform (F29-01, closes F29-CEN-05).
pub mod sbom;
```

**`crates/wcore-eval-scenarios/Cargo.toml` — NOT TOUCHED by this plan.** The `sbom` subcommand
was added to the existing `wayland-release` binary rather than as a new `[[bin]]`, so 28-02 has
that manifest to itself.

Other files in `crates/` touched: `src/sbom.rs` (new), `tests/sbom_contract.rs` (new),
`bin/wayland-release.rs` (29-01's, extended), `fixtures/f29/` (new). None shared with 28-02.

---

## Self-Check: PASSED

All 10 evidence files, the fixture corpus, the workflow, the results document and the seam
requests exist on `lane/29-02`; all commits verified present in `git log`. Local gates 12/12 and
5/5; Hetzner `rc=0` with 325/325.

**No requirement marked complete — closure is claimed by 29-04.**
