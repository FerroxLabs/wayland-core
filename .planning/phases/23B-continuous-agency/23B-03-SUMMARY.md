---
phase: 23B-continuous-agency
plan: "03"
subsystem: persistent-repository-index
status: complete-with-named-open-clause
requirements:
  - F23-06
requirements_disposition:
  F23-06: incomplete
tags: [repomap, index, sqlite, fts5, bm25, incremental, provenance, staleness, secret-isolation]
provides:
  - crates/wcore-repomap/src/store.rs
  - crates/wcore-repomap/src/scope.rs
  - crates/wcore-repomap/src/search.rs
  - crates/wcore-cli/src/index_cmd.rs
  - scripts/f23-index-drive.sh
  - scripts/f23-index-drive.ps1
key-files:
  created:
    - crates/wcore-repomap/src/store.rs
    - crates/wcore-repomap/src/scope.rs
    - crates/wcore-repomap/src/search.rs
    - crates/wcore-repomap/tests/incremental_index.rs
    - crates/wcore-repomap/tests/retrieval_quality.rs
    - crates/wcore-cli/src/index_cmd.rs
    - scripts/f23-index-drive.sh
    - scripts/f23-index-drive.ps1
    - .planning/phases/23B-continuous-agency/23B-03-LIVE-EVIDENCE.md
  modified:
    - crates/wcore-repomap/Cargo.toml
    - crates/wcore-repomap/src/lib.rs
    - crates/wcore-repomap/src/types.rs
    - crates/wcore-cli/src/lib.rs        # FENCED — additive, one pub mod
    - crates/wcore-cli/src/main.rs       # FENCED — additive, one variant + one arm
    - crates/wcore-cli/src/tui/commands/mod.rs  # FENCED — one help string
    - Cargo.lock                          # 2 dependency edges, 0 new packages
---

# Phase 23B Plan 03: Persistent Incremental Repository Index Summary

`wcore-repomap` now holds a persistent, incrementally-maintained SQLite index
of a real repository — content-hash invalidation across add, change, delete,
rename and branch switch; Git-respecting scope and worktree identity; bounded
BM25-plus-symbol retrieval fused by reciprocal rank with an exact-search
fallback; provenance and a staleness verdict on every hit; and secret
isolation proved against the store's own bytes. It is reachable as
`wayland-core index build|status|search|verify`, and every number in
`23B-03-LIVE-EVIDENCE.md` was measured by driving that binary against this
workspace.

**Termination state: 3 — complete with one named open clause.**

Two of the three mandatory platform legs are PASS on real hardware with
caller-generated nonces. The **macOS leg did not run** and is recorded as
**NOT ACHIEVED** — not a pass, not a fail — with its exact blocking cause in
`23B-03-LIVE-EVIDENCE.md` §6. Because the plan's mandatory clause is "measured
on all three platforms", **F23-06 is not marked complete.** The optional
semantic layer is separately deferred under state 2's allowance, with the
non-claim recorded below.

## The dependency question, answered explicitly

**23B-02 is PARTIAL (its Tasks 2 and 3 did not ship), and 23B-03 needed none
of what it left unbuilt.** The declared `depends_on: 23B-02` is
**sequencing-only**, and I verified that rather than assuming it:

- The only genuine coupling is the shared-file seam. The plan states that
  23B-01 owns the `session` dispatch arm and 23B-02 owns the `memory`, `cost`
  and `compact` registry entries; this plan owns the `index` arm and the
  `/repomap` entry. 23B-02's unbuilt Task 2 is exactly the `/cost` and
  `/compact` entries — files I do not touch. Its absence cannot conflict with
  my edits.
- `wcore-memory`'s hybrid retriever is a **pattern this plan mirrors, not a
  dependency it takes**, and the pattern (`retrieve.rs::search_basic`) shipped
  long before Phase 23B. `wcore-repomap` still declares zero internal
  `wcore-*` dependencies.
- Nothing in F23-06 consumes memory provenance, memory controls, cache
  diagnostics or cost truth.

There is one real, non-blocking cost: `scripts/f23-context-economics-drive.sh`
does not exist, so this driver could not literally read it as a sibling. The
argument contract it would have shared is fully specified by
`scripts/f23-session-operator-drive.sh`, which 23B-01 **did** land, and this
driver follows it.

## What shipped

**`store.rs`** — the persistent index. One record per in-scope file keyed by a
hex SHA-256 of its bytes, a symbol table joined to it, and an FTS5 virtual
table carrying the file text. Refresh is genuinely incremental: a file whose
size *and* modification time match the record is **never opened**; one whose
metadata moved is opened once and re-extracted only if its hash actually
changed; a new path whose hash matches a record that just left scope is a
**rename** — the record moves, the hash is reused and nothing is re-extracted;
what remains is deleted with its symbols and its full-text row.

Storing the text is a decision, not an accident. It is what lets the
exact-search fallback answer without re-reading the tree, and it is what gives
the secret-isolation proof its teeth: with no content stored, a planted nonce
could never appear in the store's bytes and the gate could not go red.

**`scope.rs`** — git HEAD, symbolic ref and worktree gitdir, parsed straight
from `.git/HEAD`, loose refs, `packed-refs` and `commondir`. No git library
dependency; the crate's dependency-light stance is the reason and the files
are a stable documented plain-text format. It also owns `normalize_rel`, the
single comparison boundary every stored and every looked-up path passes
through.

**`search.rs`** — BM25 over FTS5 ordered ascending (lower is better in
SQLite), an exact-then-prefix symbol pass, reciprocal-rank fusion at k = 60,
and an `instr()`-based exact scan for the queries FTS5 cannot serve at all.
Every hit carries its path, line, every modality that selected it with the
rank it held there, its fused score, the scope identity the index was built
against, and a three-axis staleness verdict.

**`index_cmd.rs`** — `wayland-core index build|status|search|verify`. Every
verb prints greppable `F23_INDEX=` `key=value` lines to STDOUT, because this
subcommand is also the measurement instrument and a driver must observe
outcomes without parsing prose. `verify` exits **6** on disagreement: a drift
check whose only signal is a line of text is a check a script forgets to read.

## Four decisions worth naming

**`.git` is excluded from the persistent scope, and `RepoMap::build` is
untouched.** The in-memory walk sets `.hidden(false)`, which re-admits the
`.git` directory. For a one-shot map that is noise. For a persistent store it
is three separate defects: the object store is not source and pollutes
retrieval; `.git/logs`, `COMMIT_EDITMSG` and the index churn on every git
operation, so a store that indexed them could never report an honest "nothing
changed"; and persisting repository internals widens what a backed-up store
contains. `scope_files` drops it by name. The existing entry point keeps its
current behaviour exactly.

**The WAL is checkpointed before a size is reported.** Measured: immediately
after a cold build the store and its log reported 133,366,096 bytes, and after
the checkpoint 66,420,792 — a 2× difference that is pure journalling
transient. A size gate sampling the first number measures when it looked, not
how large the index is.

**The retrieval-quality gate is split in two, deliberately, and both halves
are published.** The unit gate's corpus is this crate's own 18 files, whose
ground truth no other lane's churn can move; it scores 1.00 / 1.00 and catches
a *ranking regression*. The live driver measures the same 16 queries against
the **whole 3,603-file workspace** and scores `precision@1 = 0.8125`. The
lower number is the honest one about the product, and it is reported as such
rather than hidden by trimming the corpus.

**No pass-fail threshold was invented for the four perf figures.** They are
measured, three samples each, all samples recorded. A first-ever measurement
has no prior to be a regression against, and a bound picked from one session
on a shared 96-core host would be a number invented to be passed. What *is*
gated absolutely — and was chosen before measuring — is the property that
makes the warm number mean anything: a warm start opens **zero** files.

## Termination state and the semantic layer

**The OPTIONAL semantic / dense-vector layer was NOT built.** F23-06 marks it
optional and the plan's termination state 2 permits exactly this deferral.
The explicit non-claim: there is no embedding pass, no vector table and no
semantic recall in `wcore-repomap`. Rather than shipping a half-wired feature
flag, the product **reports its own unavailability** —
`wcore_repomap::semantic_status()` rides on every `SearchOutcome` and every
`index status` / `index search` invocation prints
`F23_INDEX=semantic status=unavailable: dense/semantic retrieval is not built
…; lexical BM25 + symbol retrieval only`. A test asserts that string starts
with `unavailable`, so silently degrading to lexical-only would fail.

The three concept queries that lose top-1 on the full workspace (§2 of the
evidence) are precisely the class that layer would fix. That is recorded as
finding 23B-03-M1 rather than presented as a quirk.

## Per-criterion disposition

| Success criterion | Disposition | Evidence |
|---|---|---|
| A persistent incremental hybrid repository index exists | **MET** | `store.rs` + `search.rs`; 58/58 tests on Linux, 57/57 on native Windows |
| Content-hash invalidation across add, change, delete, rename, worktree switch | **MET** | five real mutations driven on Linux and Windows, `unchanged_reextracted=0` on all ten; unit tests assert by READ COUNT, red-proved by disabling the skip |
| Git-respecting scope and worktree identity | **MET** | `scope.rs`; live branch-switch moved the recorded fingerprint on both platforms while 3,603 unchanged records went untouched |
| BM25 + symbol retrieval with an exact-search fallback | **MET** | `F23_03_FALLBACK_REPORTED=true` on both; the unit gate distinguishes "no matches" from "cannot be served" |
| Provenance and staleness on every hit | **MET** | every hit carries path, line, per-modality rank, fused score, scope identity and a 3-axis staleness verdict; `F23_03_STALENESS_REPORTED=true` asserted **before and after** the edit |
| Secret and authority isolation proved against the store's own bytes | **MET** | `F23_03_STORE_NONCE_OCCURRENCES=0` with `CONTROL_OCCURRENCES=1` on both platforms; red-proved by disabling gitignore |
| Measured warm-start, size, latency and retrieval-quality gates | **PARTIALLY MET** | measured and recorded on Linux and Windows, three samples each, all samples published. **No macOS numbers exist.** |
| Driven through the shipped binary on Linux, macOS **and** Windows | **NOT MET** | Linux PASS, Windows PASS, **macOS NOT ACHIEVED** |
| Crate isolation rule holds; no new package in the lock | **MET** | `grep -cE '^wcore-' Cargo.toml` = 0; lock diff = 2 edges, **0** `[[package]]` entries |
| Existing public API and live consumers unchanged | **MET** | `RepoMap::build` / `build_with_options` / every public type untouched; `wcore-tools` + CLI at-ref suites green (1244/1244) |
| Optional semantic layer shipped behind an off-by-default flag **or** deferred with a recorded non-claim | **MET (deferred)** | not built; `semantic_status()` reports unavailability on every search and every `index status`, and a test pins that |
| Nothing weakened, ignored, re-gated, timed out differently, deleted, or threshold-widened | **MET** | no `#[ignore]`, no `#[allow]`, no timeout change, no deleted test; two clippy findings **fixed**; every threshold's choice order recorded |

## Requirement disposition

| Requirement | Disposition |
|---|---|
| **F23-06** | **INCOMPLETE.** The index is built, complete in function, and proved live on two of three platforms. The mandatory clause "measured on Linux, macOS and Windows" is unmet because the macOS leg did not run. Everything needed to close it is in place — the driver already resolves `PLATFORM=macos`, and §6 of the evidence gives the exact five-line procedure. What is missing is one CI `build`-job artifact. |

## Deviations from plan

- **The macOS binary came from CI's `build` job artifact, not from
  `scripts/f23-macos-binary.sh`.** That script does not exist — 23B-01 did not
  land it, and says so in its own summary. The plan instructs me to STOP
  rather than improvise a second resolver, and its stated fallback ("the Mac
  builds its own binary") is forbidden by this lane's controlling instruction
  and, per `.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md`, unnecessary:
  `ci.yml:484-490` uploads `wayland-core-<target>` for all six targets from a
  `build` job independent of the failing contract-corpus check, and `lane/**`
  is already in `push.branches`. Downloading that artifact is not a second
  *Cargo* resolver — it is the rule-compliant path the plan's own premise
  measured wrong. The route did not complete for a measured reason and not a
  conceptual one: **11 CI runs queued, 0 in progress** across the repository —
  the frontier execution saturated the org's Actions capacity — so run
  `30277494031` sat at `pending` with zero jobs for the rest of the session.
  Reported as NOT ACHIEVED rather than worked around.
- **Remote gates ran in a per-lane worktree, not in `/root/wayland` directly.**
  The plan's verify blocks say `cd /root/wayland && git checkout --detach $SHA`.
  Five lanes share that checkout concurrently; detaching it would yank the
  tree out from under them. Gates ran in `/root/wayland-23B-03`, a dedicated
  worktree, with the same `git checkout -q --detach $SHA` plus the same
  `test -f <file this plan creates>` assertion in the same `&&` chain, so the
  "no gate passes on a tree lacking the work" property is unchanged.
- **`cargo hakari verify` was not run.** `cargo-hakari` is not installed on
  `hetzner-dsm` and is not a CI step; installing it is a package-manager
  install this plan is not permitted to perform unattended. Reported as NOT
  RUN. The substantive property it would guard was verified directly: the lock
  diff adds two dependency edges and zero `[[package]]` entries.
- **`rusqlite` was added to `[dev-dependencies]` as well as `[dependencies]`.**
  The incremental suite writes a wrong `schema_version` through a plain SQLite
  connection to prove the store refuses it. Doing that through a test-only
  method on `IndexStore` would put a mutation hole in the production type. No
  new package; the same already-locked one.

## Deviation rules applied

- **[Rule 2 — missing critical behaviour] `.git` exclusion.** Not in the plan;
  without it the persistent store would index the object database and could
  never report an honest unchanged refresh.
- **[Rule 2 — missing critical behaviour] WAL checkpoint before size
  reporting.** Not in the plan; without it the size gate measures a
  journalling transient.
- **[Rule 1 — bug in this plan's own test] the absent-token probe was present
  by construction.** The literal sat in the test file, and the corpus is the
  crate's own tree. Fixed by generating it at run time, not by weakening the
  assertion.
- **[Rule 1 — bug in this plan's own driver] a field extractor returned empty
  for the first field on a line.** The anchored regex demanded whitespace
  before the key. Caught because `verify` exited 6 while the parsed field said
  nothing.

## Findings

No CRITICAL or HIGH finding is open. The plan's one CRITICAL threat
(T-23B03-01, excluded content persisted into the store) is closed by
measurement on both platforms that ran, with a control marker proving the
assertion could have failed.

| ID | Severity | Finding |
|---|---|---|
| 23B-03-M1 | MEDIUM | Full-workspace `precision@1 = 0.8125`: three concept queries rank a prose-heavy planning document or another crate's doc-comment above the definition. Recall@10 = 1.0000, so nothing is lost, only mis-ordered. This is the class the deferred semantic layer addresses. Identical on Linux and Windows. |
| 23B-03-M2 | MEDIUM | On Windows the scope fingerprint carries the verbatim `\\?\` prefix, slash-normalised (`gitdir=//?/C:/…`). Self-consistent, so nothing breaks today; a fingerprint produced without `fs::canonicalize` would not compare equal. |
| 23B-03-M3 | MEDIUM | The exact-search fallback is a full `instr()` scan: 51,601 µs vs 5,810 µs p50 for an indexed query. Bounded by the caller's limit, so not a DoS surface. |
| 23B-03-L1 | LOW | `cargo hakari verify` not runnable — `cargo-hakari` absent from `hetzner-dsm`, not a CI step. Reported NOT RUN. |

All four are appended to `.planning/BACKLOG.md`. Per the phase's amended
rules, MEDIUM and below do not block.

**Pre-existing, and PROVED so rather than assumed.**
`wcore-cli::child_authority_corpus::{corpus_time, corpus_token, corpus_cost,
corpus_depth}` fail on `hetzner-dsm`. They fail identically at the untouched
base `32e2f57d09fe4b287e513081862217dc9daa5901` — on a tree asserted by
`test ! -f crates/wcore-repomap/src/store.rs` not to contain this work — and
their own message dates the cause to the F21-02 child-budget change. Reported,
not hidden, and not counted as this plan's green.

## Verification

| Gate | Host | Result |
|---|---|---|
| `cargo fmt --all -- --check` | Mac | clean |
| `cargo clippy -p wcore-repomap --all-targets -- -D warnings` | `hetzner-dsm` | clean; 2 findings **fixed** (collapsible `if`), none allowed |
| `cargo clippy --workspace --all-targets -- -D warnings` | `hetzner-dsm` | clean |
| `cargo nextest run -p wcore-repomap --profile ci` | `hetzner-dsm` | **58/58**, 0 retries consumed |
| `cargo nextest run -p wcore-repomap -p wcore-tools --profile ci` | `hetzner-dsm` | **1244/1244**, 3 skipped, 0 retries |
| `cargo nextest run -p wcore-cli --profile ci` | `hetzner-dsm` | 2177 run, **2173 passed, 4 failed** — all four pre-existing at base, proved |
| `cargo nextest run -p wcore-repomap --profile ci` | **native Windows** | **57/57** (58th is `#[cfg(unix)]`) |
| `scripts/f23-index-drive.sh` | `hetzner-dsm` | **exit 0**, `F23_03_DRIVE=PASS platform=linux nonce=d3b14061fc7a3735` |
| `scripts/f23-index-drive.ps1` | **SeanDesktop** | **exit 0**, `F23_03_DRIVE=PASS platform=windows nonce=8ed4d1215a01c1f4` |
| `scripts/f23-index-drive.sh` | macOS | **NOT RUN** — no binary; see evidence §6 |
| `cargo hakari verify` | — | **NOT RUN** — tool absent |

Every remote gate pinned the exact commit with `git checkout -q --detach $SHA`
and asserted a file this plan creates was present, in the same `&&` chain,
before the compiler ran. No gate in this plan is a pipeline into a filter.

**Five gates were made to go RED on purpose**, on real hardware, and the
failure output was read — the read-count incrementality assertion, the
secret-isolation assertion, the rename assertion, and both halves of the
Windows ssh gate shape (exit 92 on a missing file, exit 4 on `--no-tests=fail`).
Full detail in `23B-03-LIVE-EVIDENCE.md` §3.

## What was NOT done

- **No macOS leg.** No macOS numbers exist anywhere in this plan's output.
- **The optional semantic / dense-vector layer was not built.** Recorded as an
  explicit non-claim; the product reports its own unavailability.
- **No perf pass-fail thresholds were invented.** The four figures are
  recorded as a baseline, not asserted as a gate met.
- **`.planning/STATE.md`, `ROADMAP.md` and `REQUIREMENTS.md` were not
  touched.** All five concurrent lanes share them and the orchestrator merges
  lanes serially; editing them here would guarantee a conflict for no benefit.
  F23-06's disposition is recorded above and in the evidence document.
- **No `wcore-contract generate`**, no PR, no merge, no tag, no issue closed.

## Self-Check

All nine created files exist on disk; both drive logs exist and carry their
run's own nonce in a terminal PASS marker; every commit below resolves in
`git log`. The macOS drive log is asserted **absent**, consistent with the
NOT ACHIEVED disposition — there is no file to grep and no number claimed.
