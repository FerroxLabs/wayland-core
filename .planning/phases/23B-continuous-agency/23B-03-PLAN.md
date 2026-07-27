---
phase: 23B-continuous-agency
plan: "03"
type: execute
wave: 3
depends_on:
  - "23B-02"
files_modified:
  - crates/wcore-repomap/Cargo.toml
  - crates/wcore-repomap/src/lib.rs
  - crates/wcore-repomap/src/store.rs
  - crates/wcore-repomap/src/scope.rs
  - crates/wcore-repomap/src/search.rs
  - crates/wcore-repomap/src/types.rs
  - crates/wcore-repomap/tests/incremental_index.rs
  - crates/wcore-repomap/tests/retrieval_quality.rs
  - crates/wcore-cli/src/index_cmd.rs
  - crates/wcore-cli/src/lib.rs
  - crates/wcore-cli/src/main.rs
  - crates/wcore-cli/src/tui/commands/mod.rs
  - scripts/f23-index-drive.sh
  - scripts/f23-index-drive.ps1
  - .planning/phases/23B-continuous-agency/evidence/
  - .planning/phases/23B-continuous-agency/23B-03-LIVE-EVIDENCE.md
autonomous: true
requirements:
  - F23-06
domain: code
must_haves:
  truths:
    - "THE CRATE IS 1,324 LINES AND FULLY IN-MEMORY TODAY, AND F23-06 IS THEREFORE MOSTLY NEW CONSTRUCTION. `wcore-repomap` currently walks a tree with the ignore crate, runs regex symbol extractors for Rust and TypeScript, and returns a RepoMap value. There is no persistence, no incremental update, no content hashing, no full-text search, no ranking, no staleness, no provenance and no perf gate. F23-06 asks for all of them. This is the largest single build in Phase 23B and the plan must be honest about that: if the mandatory core cannot be closed, the semantic and reciprocal-rank-fusion layer — which F23-06 itself marks OPTIONAL — is the thing that is deferred, and nothing in the mandatory core is quietly reduced to make room."
    - "THE ISOLATION RULE IS ARCHITECTURAL AND SURVIVES THIS PLAN. AGENTS.md declares `wcore-repomap` deliberately isolated with NO internal `wcore-*` dependencies. That constrains the design: the index cannot import `wcore-memory`'s hybrid retriever even though that retriever already fuses FTS5 BM25 with a vector pass by reciprocal rank fusion and is exactly the right shape. The retriever is a PATTERN to mirror, not a dependency to take. External crates are unaffected by the rule — `rusqlite` at the workspace version with the bundled feature and `sha2` are already workspace dependencies, and the bundled SQLite provides FTS5 (proved by `wcore-memory` already issuing MATCH queries against an FTS5 virtual table). Adding those two to this crate adds NO new package to the workspace lock."
    - "THE CRATE HAS LIVE CONSUMERS THAT MUST NOT BREAK. `crates/wcore-tools` exposes a repomap agent tool, and `crates/wcore-cli/src/tui/engine_bridge.rs` and `tui/commands/at_ref_send.rs` consume the map for at-reference resolution. The existing `RepoMap::build` and `build_with_options` entry points and the `FileSummary`, `Symbol`, `SymbolKind`, `Language` and `IndexOptions` types are public API those consumers depend on. Persistence is ADDITIVE: the existing entry points keep working with their current semantics, and the persistent index is a new surface beside them."
    - "SECRET AND AUTHORITY ISOLATION IS A PROPERTY OF WHAT THE INDEX STORES, NOT ONLY OF WHAT IT RETURNS. A file excluded by gitignore must never be READ into the store in the first place; filtering it at query time leaves the secret sitting in an index database that gets backed up, exported and migrated. The default already respects gitignore via the ignore crate — that default must hold for the persistent store, and the proof is a planted run-time nonce in an ignored file that is absent from the store's own bytes, not merely absent from query results."
    - "PERFORMANCE CLAIMS REQUIRE MEASUREMENT ON REAL HARDWARE AGAINST A REAL REPOSITORY. F23-06 demands warm-start, size, latency and retrieval-quality gates. This workspace is itself the right corpus: over eleven hundred Rust source files across more than fifty crates, including single files of 320 KB and 1.2 MB that will find the pathological cases a synthetic fixture never will. The numbers are MEASURED and RECORDED on Linux, macOS and Windows; a threshold chosen after seeing the measurement is legitimate and must be recorded as such, and a threshold quietly widened after a failure is not."
    - "A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. Never weaken an assertion, add an ignore or allow attribute, raise a timeout, re-gate, or delete an inconvenient test to reach a gate. Never widen a measured perf gate to make a run pass. If retrieval quality falls below its floor, that is a finding to report with its corpus and its numbers."
    - "A GATE THAT CANNOT GO RED IS WORSE THAN NO GATE, AND THIS PLAN ALREADY SHIPPED TWO OF THEM. The previous revision closed both Windows legs — the `wcore-repomap` suite in Task 1 and the index driver in Task 3 — with `ssh host '...' | grep -v CLIXML | grep -v '^<Objs'`. A pipeline's exit status is the LAST command's, so that reported grep's status, not ssh's: any surviving output line greened the gate even when the remote suite failed, and grep's exit 1 on empty output meant it reddened on silent success. That is doubly bad here, because Windows is the platform this plan EXPECTS to find defects on — mandatory byte-range locking and path representation both bite a SQLite-backed index — so a Windows gate that cannot fail defeats the plan's own thesis. A third instance is closed here: `cargo clippy` passing on a host tree that does not contain this plan's new modules. For every command written into a `<verify>` block, answer 'what makes this go red?' before writing it."
    - "THE macOS LEG HAD NO BINARY AND NO EXECUTABLE STEP, AND THE ARTIFACT IT NAMED DOES NOT EXIST. The previous revision drove macOS against a 'PREBUILT wayland-core artifact' with no local Cargo. Measured against `.github/workflows/`: `ci.yml` uploads only `nextest-junit-${{ matrix.os }}` JUnit XML and no binary of any kind, and `release.yml` builds Darwin binaries only on a `v*-wayland-*` tag push or an explicit dispatch — both Sean-only, as is pushing. No such artifact is reachable from inside this phase, and not one automated gate command executed anything on macOS: the macOS perf and quality numbers were closed by grepping an evidence file the executor itself wrote, which is a tautology and cannot be a measurement. The macOS leg now runs the real driver locally against the binary `scripts/f23-macos-binary.sh` resolves, and every leg's binary must prove its own provenance through `--build-info`."
  artifacts:
    - path: crates/wcore-repomap/src/store.rs
      provides: "The persistent index: a SQLite store holding file records keyed by content hash with an FTS5 virtual table over content and a symbol table, plus the incremental apply path handling add, change, delete and rename"
    - path: crates/wcore-repomap/src/scope.rs
      provides: "Git-respecting scope and worktree identity: which files are in scope, which HEAD and worktree the index was built against, and the invalidation triggered when that identity changes"
    - path: crates/wcore-repomap/src/search.rs
      provides: "Bounded retrieval: BM25 full-text plus symbol lookup, fused by reciprocal rank, with an exact-search fallback when full-text yields nothing, and provenance and staleness on every hit"
    - path: crates/wcore-cli/src/index_cmd.rs
      provides: "The `wayland-core index` operator surface — build, status, search and verify — which is also how the perf gates are measured against a real repository"
    - path: crates/wcore-repomap/tests/retrieval_quality.rs
      provides: "A fixed query-to-expected-symbol corpus with a recorded precision and recall floor, mirroring the shape of the existing eval acceptance gate"
    - path: .planning/phases/23B-continuous-agency/23B-03-LIVE-EVIDENCE.md
      provides: "Measured cold-build, warm-start, on-disk size and query-latency numbers per platform against this workspace, the incremental invalidation results, the secret-isolation proof and the retrieval-quality numbers"
  key_links:
    - from: crates/wcore-cli/src/index_cmd.rs
      to: crates/wcore-repomap/src/store.rs
      via: "the index subcommand driving the persistent store, which is how the perf gates are measured through the shipped binary"
      pattern: "cli-to-store"
    - from: crates/wcore-repomap/src/scope.rs
      to: crates/wcore-repomap/src/store.rs
      via: "worktree and HEAD identity change triggering incremental invalidation rather than a full rebuild"
      pattern: "scope-invalidation"
    - from: crates/wcore-repomap/src/search.rs
      to: crates/wcore-repomap/tests/retrieval_quality.rs
      via: "the fixed corpus measuring precision and recall against a recorded floor"
      pattern: "quality-gate"
---

<objective>
Make Success Criterion 6 true: `wcore-repomap` becomes a persistent incremental hybrid repository index with content-hash invalidation across add, change, delete, rename and worktree switch; Git-respecting scope; BM25 full-text plus symbol retrieval with an exact-search fallback; provenance and staleness on every hit; secret and authority isolation; and measured warm-start, size, latency and retrieval-quality gates — all driven through the shipped binary against this real workspace on Linux, macOS and Windows.

Purpose: F23-06 is what makes continuous agency useful over a real codebase rather than over a context window. Today the crate rebuilds an in-memory map from scratch on every call, which is why it cannot serve a long-running session. This is the largest build in Phase 23B and it is deliberately placed last among the construction plans so it inherits, rather than competes with, the operator and memory surfaces.
Output: A persistent SQLite-backed index inside the crate's existing isolation rule; a `wayland-core index` operator surface; incremental invalidation proved by real file and worktree mutations; a secret-isolation proof against the store's own bytes; and measured perf and quality numbers recorded per platform.
</objective>

<execution_context>
@$HOME/.codex/gsd-core/workflows/execute-plan.md
@$HOME/.codex/gsd-core/templates/summary.md
</execution_context>

<context>
@AGENTS.md
@.planning/HANDOFF-2026-07-26-phase20-20A-complete.md
@crates/wcore-repomap/src/lib.rs
@crates/wcore-repomap/src/types.rs
@crates/wcore-repomap/src/extractor/mod.rs
@crates/wcore-repomap/Cargo.toml
@crates/wcore-memory/src/retrieve.rs
@crates/wcore-cli/src/tui/commands/at_ref_send.rs
</context>

<execution_rules>

**THE TWO AMENDED PHASE RULES — verbatim, and they bound this plan.**

- Findings at CRITICAL or HIGH must be fixed or disproved. MEDIUM and below are logged to BACKLOG and DO NOT BLOCK execution.
- Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean.

**TERMINATION CRITERION FOR THIS PLAN (hard).** This plan builds the index once and measures it once per platform. It terminates in exactly one of four states, and in all four it writes its SUMMARY and stops:
1. **Complete** — the mandatory core and the optional semantic layer both ship, all gates are measured and met on all three platforms.
2. **Complete, semantic layer deferred** — the mandatory core ships and is measured; the OPTIONAL semantic and reciprocal-rank-fusion layer is deferred with an explicit non-claim recorded. F23-06 marks the semantic layer optional, so this is a legitimate complete state — but ONLY the semantic layer may be deferred this way, and only if every mandatory clause is closed.
3. **Complete with named open clauses** — a mandatory clause could not be closed. Record it as OPEN with its blocking evidence, mark F23-06 incomplete, and stop.
4. **Escalated** — a CRITICAL or HIGH finding requires a change outside this plan's declared files, or the isolation rule would have to be broken to proceed. Record it with severity and stop.
Under no circumstances does this plan create additional plans or extend its own task list.

**THE ISOLATION RULE IS ABSOLUTE AND IS A STOP CONDITION.** `wcore-repomap` takes NO internal `wcore-*` dependency. If closing a clause appears to require one, that is termination state 4 — escalate, do not take the dependency. External crates are permitted; `rusqlite` and `sha2` are already workspace dependencies and adding them here adds no new package to the lock.

**ADDITIVE ONLY — THE EXISTING PUBLIC API SURVIVES UNCHANGED.** `RepoMap::build`, `RepoMap::build_with_options`, `FileSummary`, `Symbol`, `SymbolKind`, `Language`, `IndexOptions` and `RepoMapError` are consumed by `wcore-tools`' repomap agent tool and by the TUI's engine bridge and at-reference resolution. Their signatures and semantics do not change. The persistent index is a new surface beside them. The crate's own `missing_docs` warning and `unsafe_code` denial stay in force.

**SCOPE BOUNDARY (hard).** Session operator verbs are 23B-01's and memory control and context economics are 23B-02's; both are admitted inputs. The multi-day journey and terminal acceptance are 23B-04's. Memory retrieval and repository retrieval are DIFFERENT subsystems — this plan does not touch `wcore-memory`, and the memory hybrid retriever is a pattern to mirror rather than code to move or share.

**THE `main.rs` AND `tui/commands/mod.rs` SEAMS.** 23B-01 owns the session dispatch arm and 23B-02 owns the memory, cost and compact registry entries. This plan owns the index dispatch arm and the repomap registry entry, and touches nothing else in either file. The consecutive waves exist for exactly this reason.

**NON-NEGOTIABLE.** A REPORTED RED IS WORTH FAR MORE THAN AN ENGINEERED GREEN. The specific temptations here are to widen a perf gate after seeing it fail, to prove secret isolation by filtering at query time instead of never storing, and to prove incrementality by timing a rebuild instead of asserting that unchanged files were not re-read. All three are engineered greens and all three are forbidden.

**ENVIRONMENT.**
- Linux (authoritative Cargo proof): `ssh -o BatchMode=yes hetzner-dsm`, `/root/wayland`, 96 cores, full aggregate ~194s.
- Windows (native live): `ssh -o BatchMode=yes SeanD@seandesktop`, checkout `C:\ferrox-win`, cargo at `C:\Users\seand\.cargo\bin\cargo.exe`. The remote default shell is PowerShell, so an `ssh` command string is PowerShell source and must end with an explicit `exit $LASTEXITCODE` for the status to propagate. `cargo fmt --all` fails there with os error 206. Clippy runs with warnings denied BEFORE tests.
- macOS (native live): THIS Mac. See the macOS binary decision below. `cargo fmt --all -- --check` is the local formatting gate.

**GATE DISCIPLINE — every command in a `<verify>` block must be able to go RED. Three hard rules, each closing a defect this plan actually shipped.**

1. **A gate is NEVER a pipeline into a filter.** `ssh host 'cmd' | grep -v CLIXML | grep -v "^<Objs"` reports GREP's exit status, not ssh's. Any surviving output line greens it even when the remote suite failed, and grep's exit 1 on EMPTY output means it reddens on silent success. BOTH Windows gates in the previous revision had exactly that shape — which is worse here than anywhere else in the phase, because Windows is the platform this plan expects to FIND defects on. The correct form redirects, captures the status on the NEXT line, asserts on it, and only then reads the log:
   `ssh -o BatchMode=yes HOST "…; exit \$LASTEXITCODE" > LOG 2>&1; rc=$?; test "$rc" -eq 0 && /usr/bin/grep -qF "MARKER" LOG`
   Filtering CLIXML noise while READING a log for a human is fine; it is fatal only when the pipeline IS the gate.
2. **Never read an exit code from a block that also emits output.** In PowerShell, `$x = & { cargo … | Tee-Object …; $LASTEXITCODE }` returns an ARRAY of every output line plus the code, so `if ($x -ne 0)` is an always-truthy array filter. That bug made an all-PASS 12/12 + 6/6 Windows soak report failure; the fix and its post-mortem are in `scripts/wayland-e2e-windows-soak.ps1:174-190` and `:244-255`. Read `$LASTEXITCODE` on the line AFTER the pipeline, and always end a driver with an explicit `exit`.
3. **Never let a gate pass on a tree that does not contain the work.** `cargo clippy -p wcore-repomap -- -D warnings` on a host synced to the last PUSHED tip is clean and proves nothing about `store.rs`, `scope.rs` or `search.rs` before they exist there. Every remote gate therefore pins the exact commit under test and asserts a file THIS PLAN CREATES is present, in the same `&&` chain, before the compiler runs: take `SHA=$(/usr/bin/git rev-parse HEAD)` locally, `git checkout -q --detach $SHA` on the host, then `test -f <file this plan creates> && cargo …`. **Commit this task's declared files and get the working branch onto `gh` BEFORE running a remote gate.** Do not respond to a missing SHA by dropping the assertion.

**macOS BINARY SOURCE — DECIDED IN 23B-01 AND CARRIED HERE, WITH ITS BASIS AND ITS MEASUREMENTS.** The previous revision drove the macOS leg against a "PREBUILT `wayland-core` artifact only". That artifact does not exist and cannot be produced from inside this phase. Measured, not assumed: `.github/workflows/ci.yml:204-208` uploads only `nextest-junit-${{ matrix.os }}` — JUnit XML, no binary of any kind, on any branch; `.github/workflows/release.yml:1-24` fires only on a `v*-wayland-*` tag push, a `workflow_call`, or an explicit `workflow_dispatch`, and its Darwin targets at `:70-74` therefore never build for `plan/f20-unified-audit-repair`. Tagging, releasing, dispatching and pushing are all Sean-only, so no CI run producing a macOS binary can be triggered from inside plan execution. **Decision: the macOS leg builds its own binary on this Mac, through `scripts/f23-macos-binary.sh`, which 23B-01 owns and this plan consumes unchanged.** Basis: HANDOFF §3 item 7 — "This Mac CAN compile the workspace. The old 'never compiles on Mac' note is a workflow convention, not a fact" — plus the pinned toolchain `1.95.0-aarch64-apple-darwin` present under `~/.rustup/toolchains` and matching `rust-toolchain.toml`. **The convention's real purpose is preserved exactly: `hetzner-dsm` stays the sole authority for clippy, nextest and the aggregate proof. The Mac build produces a DRIVE TARGET, never a proof verdict, and is isolated in `--target-dir target/f23-macos`, which the existing `/target/` ignore rule already covers.** This matters more here than elsewhere: the macOS perf numbers are MEASUREMENTS, and a measurement taken against an unidentifiable binary is not a measurement — so the resolver asserts the binary's own `--build-info` source SHA equals the commit under test, and a mismatch reddens. If the Mac build fails, that is a RED to record: the macOS rows go OPEN with the compiler's exact error under this plan's termination state 3. It is never a silent skip. If `scripts/f23-macos-binary.sh` is absent because 23B-01 did not land it, STOP and record that as a blocking dependency rather than improvising a second resolver.
- **Windows path representation is the known defect class for anything storing paths.** `std::fs::canonicalize` returns a verbatim extended-length prefix that git-for-Windows and PowerShell drive-info cannot parse; the `dunce` simplification helper is CONDITIONAL and no-ops on components over 255 characters, non-UTF-8 names, reserved DOS names and trailing dots or spaces. The established rule is to normalise at the COMPARISON boundary on BOTH operands, not only at storage. An index that stores one representation and looks up another will silently miss on Windows and pass every Linux test. `crates/wcore-swarm/src/worktree_paths.rs` holds the existing normalisation helper as the reference for the shape (it is in another crate and cannot be imported here — mirror the discipline, not the code).
- ALWAYS `/usr/bin/grep` on the Mac with `-F` for literals.
- Always `git fetch origin plan/f20-unified-audit-repair` explicitly. In the Mac repo `origin` is a stale local worktree; the real remote is `gh`.
- NO push to main, merge, PR, tag, release, deployment or issue closure.

**AGENTS.md discipline.** Surgical diffs. Keep every module under 1000 lines — this plan adds four modules rather than growing `lib.rs`. Public API errors use thiserror. Never call a shell interpreter directly; anything spawning a process goes through the central shell helper in argv mode. Clippy-clean with warnings denied.

**Git hygiene.** `/usr/bin/git` on the Mac. Stage the exact paths in `files_modified`, never `-A`, never `.`. No `Co-Authored-By` trailers.
</execution_rules>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: The persistent store, content-hash invalidation, Git-respecting scope and secret isolation</name>
  <files>crates/wcore-repomap/Cargo.toml, crates/wcore-repomap/src/store.rs, crates/wcore-repomap/src/scope.rs, crates/wcore-repomap/src/types.rs, crates/wcore-repomap/src/lib.rs, crates/wcore-repomap/tests/incremental_index.rs</files>
  <read_first>crates/wcore-repomap/src/lib.rs in full (the walk built on the ignore crate with standard filters bound to the respect-gitignore option, hidden files included so dotfiles are visible while gitignore still applies, the per-entry error skip that is the crate's light-tool stance, the max-file-bytes and max-lines cutoffs and how an oversized file is recorded with an Other language and no symbols), crates/wcore-repomap/src/types.rs (IndexOptions with its three fields and defaults, and the RepoMapError variants), crates/wcore-repomap/src/extractor/mod.rs plus rust.rs and typescript.rs (the regex extractors, which languages are covered and what a Symbol carries), crates/wcore-repomap/tests/fixture_index.rs (the existing fixture repository including the deliberately ignored target directory file, which is the seed of the secret-isolation case), crates/wcore-memory/src/db.rs (the established rusqlite connection and schema-migration idiom in this workspace — mirror the shape, take no dependency), crates/wcore-memory/src/schema/ (how an FTS5 virtual table is declared and kept in step with its base table here)</read_first>
  <behavior>
    - A first build against a repository writes a persistent store on disk containing one record per in-scope file keyed by a content hash, plus its extracted symbols.
    - Re-opening that store without any filesystem change performs no file reads beyond the scope walk, proved by counting reads rather than by timing.
    - Adding a file adds exactly one record; changing a file's bytes changes exactly that record's hash and re-extracts only that file; deleting a file removes exactly its record and its symbols.
    - Renaming a file whose content is unchanged is recognised as a rename — the content hash is reused rather than re-extracted, and the old path no longer resolves.
    - Switching the checkout to a different HEAD or worktree changes the recorded scope identity and invalidates exactly the records whose content differs, never the whole store.
    - A file excluded by gitignore is never READ into the store: a run-time-generated nonce planted in an ignored file is absent from the store file's own bytes, not merely absent from query results.
    - A file outside the indexed root is never stored, including through a symlink that points outside it.
    - Every stored path round-trips on Windows: the representation written and the representation looked up compare equal for a path with a non-ASCII component and for a deeply nested path.
    - A corrupt or truncated store returns a structured error naming the file and offers a rebuild, and never panics or silently returns an empty index.
    - The pre-existing `RepoMap::build` and `build_with_options` entry points behave exactly as before.
  </behavior>
  <action>Add `rusqlite` at the workspace version and `sha2` to this crate's dependencies. Both are already workspace dependencies so no new package enters the lock, and neither is an internal crate so the isolation rule holds. Record that reasoning in the Cargo.toml comment beside them because the crate's description advertises its isolation and a future reader will otherwise assume a violation.

Create `store.rs` holding the persistent index: a file-record table keyed by relative path carrying the content hash, size, language and modification identity; a symbol table joined to it; and an FTS5 virtual table over file content kept in step with the base table the way `wcore-memory`'s schema module already does it. Create `scope.rs` holding the Git-respecting scope and the worktree identity: which files are in scope by the existing ignore-crate walk, and which HEAD and worktree the store was built against, read directly from the repository's own Git metadata rather than by taking a Git library dependency, which matches the crate's dependency-light stance.

Write the tests first, one per behavior bullet, in `tests/incremental_index.rs` against real temporary repositories. Prove no-op re-open by COUNTING file reads, not by timing — a timing assertion is flaky under load and proves the wrong thing. Prove rename by asserting the content hash was reused and the old path no longer resolves. Prove worktree invalidation by actually creating a second branch with a differing file and switching to it.

For secret isolation, plant a run-time-generated nonce into a gitignored file and assert the nonce is absent from the STORE FILE'S OWN BYTES. Filtering at query time is not acceptable: the store is an artifact that gets backed up and migrated, and the requirement is isolation, not suppression. Do the same for a symlink pointing outside the indexed root.

For Windows path representation, normalise at the comparison boundary on both operands. Store one canonical representation and normalise the lookup key the same way. Include a non-ASCII component case and a deeply nested case, because the simplification helper conditionally no-ops on exactly those shapes and an index that stores one form and looks up another passes every Linux test and silently misses on Windows.

Leave `RepoMap::build` and `build_with_options` and every public type untouched; assert that by running the existing fixture test unchanged. Addresses F23-06; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $SHA &amp;&amp; test -f crates/wcore-repomap/src/store.rs &amp;&amp; test -f crates/wcore-repomap/src/scope.rs &amp;&amp; test -f crates/wcore-repomap/tests/incremental_index.rs &amp;&amp; cargo clippy -p wcore-repomap --all-targets -- -D warnings"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo nextest run -p wcore-repomap --profile ci --no-tests=fail --no-fail-fast"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-03-windows-repomap-suite.log &amp;&amp; ssh -o BatchMode=yes SeanD@seandesktop "Set-Location C:\ferrox-win; git fetch -q origin plan/f20-unified-audit-repair; git checkout -q --detach $SHA; if (\$LASTEXITCODE -ne 0) { exit 91 }; if (-not (Test-Path crates\wcore-repomap\src\store.rs)) { exit 92 }; cargo nextest run -p wcore-repomap --profile ci --no-tests=fail --no-fail-fast; exit \$LASTEXITCODE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0</automated>
    <!-- The trailing zero-internal-dependency clause is a REGRESSION GUARD, not completion coverage: it is already true on the untouched tree and stood alone here, so it could never show the task was done. It is kept because the crate's isolation is load-bearing, and it is now chained behind completion clauses that are RED at base — store.rs, scope.rs and the incremental suite do not exist, and rusqlite and sha2 are not yet declared. -->
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -f crates/wcore-repomap/src/store.rs &amp;&amp; test -f crates/wcore-repomap/src/scope.rs &amp;&amp; test -f crates/wcore-repomap/tests/incremental_index.rs &amp;&amp; /usr/bin/grep -qE '^rusqlite' crates/wcore-repomap/Cargo.toml &amp;&amp; /usr/bin/grep -qE '^sha2' crates/wcore-repomap/Cargo.toml &amp;&amp; test "$(/usr/bin/grep -cE '^wcore-' crates/wcore-repomap/Cargo.toml)" -eq 0</automated>
  </verify>
  <done>The persistent store exists with content-hash records, symbols and an FTS5 table; add, change, delete, rename and worktree-switch invalidation each have a passing test, with the no-op re-open proved by read count and not by timing. A nonce planted in a gitignored file is absent from the store file's bytes. Windows path round-trip passes on real Windows hardware for a non-ASCII and a deeply nested path. `Cargo.toml` declares zero internal `wcore-` dependencies. The pre-existing fixture test passes unchanged.</done>
</task>
<task type="auto" tdd="true">
  <name>Task 2: Bounded hybrid retrieval with provenance, staleness and an exact-search fallback, plus the operator surface</name>
  <files>crates/wcore-repomap/src/search.rs, crates/wcore-repomap/src/lib.rs, crates/wcore-cli/src/index_cmd.rs, crates/wcore-cli/src/lib.rs, crates/wcore-cli/src/main.rs, crates/wcore-cli/src/tui/commands/mod.rs, crates/wcore-repomap/tests/retrieval_quality.rs</files>
  <read_first>crates/wcore-memory/src/retrieve.rs `search_basic` (the exact pattern to mirror: an FTS5 BM25 pass ordered ascending because lower BM25 is better in SQLite, joined against the base table on rowid and filtered by tier; a vector pass; reciprocal-rank fusion; a diversity cap; and a per-modality limit — mirror this shape, take no dependency on it), crates/wcore-memory/src/cross_project.rs (the hand-written BM25 scorer with its k1 and b constants and its tokenizer, which documents how this workspace already thinks about lexical scoring), crates/wcore-cli/src/swarm.rs (the established subcommand module shape — a clap Args struct plus a run entry point), crates/wcore-cli/src/main.rs (the TopCmd enum and dispatch — 23B-01 already added the session arm here; add only the index arm), crates/wcore-cli/src/tui/commands/mod.rs (the existing repomap registry entry and its help idiom, and note that 23B-02 owns the memory, cost and compact entries), crates/wcore-cli/src/tui/commands/at_ref_send.rs and crates/wcore-cli/src/tui/engine_bridge.rs (the live consumers of the existing in-memory map, whose behavior must not change), crates/wcore-eval/ acceptance gate (the established shape of a precision-and-recall threshold gate in this workspace — mirror it for retrieval quality)</read_first>
  <behavior>
    - A lexical query returns hits ranked by BM25 over the full-text table, bounded by an explicit result limit that the caller sets and the store enforces.
    - A symbol query returns hits from the symbol table for an exact or prefix symbol name, and symbol hits and lexical hits are fused by reciprocal rank into one ordered result.
    - A query that the full-text index cannot serve falls back to exact search over in-scope file content and says so in the result, rather than returning an empty result that looks like an absence of matches.
    - Every hit carries provenance: its file path, its line, which modality produced it, its rank and fused score, and the scope identity the index was built against.
    - Every hit carries a staleness verdict: whether the record's content hash still matches the file on disk, and whether the store's scope identity still matches the working tree.
    - A query against a store whose scope identity has drifted reports staleness on every hit rather than silently serving stale results.
    - A query never returns content from a file the index was not permitted to store, including after a gitignore rule is added mid-life.
    - The optional semantic layer, if built, sits behind a feature flag that is OFF by default, and with it off the product reports semantic retrieval as unavailable rather than silently degrading to lexical-only without saying so.
    - The `index` subcommand exposes build, status, search and verify; status reports record count, on-disk size, scope identity and staleness; verify detects a store that disagrees with the working tree.
    - Retrieval quality over a fixed query-to-expected-symbol corpus meets a recorded precision and recall floor.
  </behavior>
  <action>Create `search.rs` mirroring the shape of the memory crate's hybrid retriever without depending on it: a BM25 pass over the FTS5 table ordered so the best score comes first, a symbol-table pass, and reciprocal-rank fusion over the two. Bound every query with an explicit limit the caller sets and the store enforces, because an unbounded retrieval over a repository this size is a denial-of-service surface against the caller's own context window.

Add the exact-search fallback for queries full-text cannot serve — punctuation-heavy literals and short tokens are the realistic cases — and mark the result as having come from the fallback. An empty result and an unserviceable query are different answers and the product must distinguish them.

Attach provenance and staleness to every hit from what the store already knows: the path, line, modality, rank, fused score, and the scope identity recorded at build time, plus a comparison of the record's content hash against the file on disk and of the stored scope identity against the working tree.

Build the optional semantic and reciprocal-rank-fusion layer behind a Cargo feature that is OFF by default. With it off, the product must REPORT that semantic retrieval is unavailable — the workspace already has a readiness-truth discipline for capabilities and this follows it. If the mandatory core consumes the plan's budget, defer this layer entirely under termination state 2 and record the non-claim; do not ship a half-wired flag.

Add `crates/wcore-cli/src/index_cmd.rs` with build, status, search and verify, wired through one new `Index` arm on the TopCmd enum and re-exported from the CLI lib, following the existing subcommand module pattern. This subcommand is also the measurement instrument for Task 3, so its status output must carry the record count, on-disk size, scope identity and staleness in a form a script can parse, and search must be able to report its own latency.

Update the existing `/repomap` registry entry's help to match the new capability and point it at the persistent index; do not touch the memory, cost or compact entries, which belong to 23B-02.

Write `tests/retrieval_quality.rs` with a fixed corpus of queries and their expected symbols drawn from this workspace's own crates, and assert a precision and recall floor, mirroring the shape of the existing eval acceptance gate. Choose the floor AFTER measuring, and record in the SUMMARY both the measured value and the floor, so a later widening is visible as a change rather than absorbed.

Verify the existing at-reference resolution and engine-bridge behavior is unchanged by running their suites unmodified. Addresses F23-06; marks no requirement complete.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; cargo fmt --all -- --check</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $SHA &amp;&amp; test -f crates/wcore-repomap/src/search.rs &amp;&amp; test -f crates/wcore-cli/src/index_cmd.rs &amp;&amp; cargo clippy --workspace --all-targets -- -D warnings"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo nextest run -p wcore-repomap -p wcore-tools --profile ci --no-tests=fail --no-fail-fast"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo nextest run -p wcore-cli --profile ci --no-tests=fail --no-fail-fast"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo build --release -p wcore-cli --bin wayland-core &amp;&amp; ./target/release/wayland-core --build-info &amp;&amp; ./target/release/wayland-core index --help"</automated>
  </verify>
  <done>The shipped binary exposes `index` with build, status, search and verify. Lexical, symbol, fused and fallback retrieval each have a passing test; every hit carries provenance and a staleness verdict; a drifted store reports staleness on every hit. The semantic layer is behind an off-by-default feature and reports unavailability when off, or is deferred with a recorded non-claim. Retrieval quality meets a floor recorded alongside its measured value. `wcore-tools`' repomap tool suite and the CLI's at-reference suites pass unchanged. Workspace clippy clean with warnings denied.</done>
</task>

<task type="auto">
  <name>Task 3: LIVE — index this real workspace through the shipped binary and measure every gate on Linux, macOS and Windows</name>
  <files>scripts/f23-index-drive.sh, scripts/f23-index-drive.ps1, .planning/phases/23B-continuous-agency/evidence/, .planning/phases/23B-continuous-agency/23B-03-LIVE-EVIDENCE.md</files>
  <read_first>scripts/f23-macos-binary.sh, scripts/f23-session-operator-drive.sh and scripts/f23-context-economics-drive.sh from 23B-01 and 23B-02 (the driver family this one joins — the `--binary` / `--sha` / `--nonce` argument contract, the `--build-info` provenance assertion before any measurement, the hermetic run directory, the per-check transcripts, the nonce-bound terminal PASS marker, and non-zero exit on a missing observable outcome; consume the macOS binary resolver rather than writing a second one), scripts/wayland-e2e-windows-soak.ps1 lines 174-190 and 244-255 (the worked example of PowerShell exit-code capture and the post-mortem on the `$x = &amp; { … ; $LASTEXITCODE }` array-filter bug that reported a fully passing run as a failure), .planning/HANDOFF-2026-07-26-phase20-20A-complete.md section 5 (the Windows defect classes that transfer: path representation, handle semantics, mandatory byte-range locks where a whole-file lock breaks the crate's own readers so a one-byte sentinel is used instead, and the platform-gate blindness that hid a dead rename primitive for months — an index doing concurrent SQLite access on Windows will meet the lock class directly), .planning/HANDOFF-2026-07-26-phase20-20A-complete.md section 3 item 3 (the environment-variable trailing-space trap in the Windows command shell, which silently produces confidently wrong measurements)</read_first>
  <behavior>
    - Each drive script takes `--binary <path>`, `--sha <commit>` and `--nonce <hex>` — the same contract 23B-01's driver established — refuses to run if the binary is missing or not executable, and asserts the binary's own `--build-info` source SHA equals `--sha` before measuring anything. A measurement taken against an unidentifiable binary is not a measurement.
    - Each drive script emits exactly one terminal marker, `F23_03_DRIVE=PASS platform=&lt;linux|macos|windows&gt; nonce=&lt;the nonce it was given&gt;`, and emits it ONLY after every measurement and every check passed. Any failure exits non-zero and emits no PASS marker. The nonce is generated by the caller at run time, so a stale log from an earlier run cannot satisfy the caller's check.
    - On macOS the binary comes from `scripts/f23-macos-binary.sh`, which 23B-01 owns; this plan consumes it unchanged and does not write a second resolver.
    - It indexes THIS workspace — over eleven hundred Rust source files across more than fifty crates, including single files of 320 KB and 1.2 MB — not a synthetic fixture.
    - Cold build wall time, warm-start wall time, on-disk store size, and query latency at the median and the ninety-fifth percentile over a fixed query set are each measured and recorded per platform.
    - Warm start is measured as a second open of an unchanged store and is compared against the cold build, with the ratio recorded.
    - Incremental invalidation is driven by REAL mutations: a file is added, a file is edited, a file is deleted, a file is renamed, and the checkout is switched to a different branch. After each, the driver records what the index changed and asserts the unchanged files were not re-extracted.
    - A run-time-generated nonce planted in a gitignored file is searched for in the store file's own bytes and must not be found.
    - Retrieval quality over the fixed corpus is run through the shipped binary's search operation and its precision and recall are recorded against the floor.
    - The exact-search fallback is driven with a query full-text cannot serve, and the product's report that it fell back is captured.
    - A staleness case is driven by editing a file after the index is built and confirming the hit reports itself stale.
    - The driver exits non-zero if any measurement is missing, and never treats a missing measurement as a skip.
  </behavior>
  <action>Write `scripts/f23-index-drive.sh` and its PowerShell port, reusing the idioms established by the two earlier drivers so all three read as one family: `--binary`, `--sha` and `--nonce` arguments, the `--build-info` provenance assertion before any measurement, the hermetic run directory, the per-check transcripts, the nonce-bound terminal marker and the cleanup traps. The PowerShell port reads `$LASTEXITCODE` on the line AFTER any pipeline and never as the trailing value of a `&amp; { … }` block, and always ends with an explicit `exit` — copy the discipline and the post-mortem comment from `scripts/wayland-e2e-windows-soak.ps1:174-190`. That rule is load-bearing here: this driver's Windows leg is where the byte-range-lock and path-representation defect classes are expected to surface, and a driver that reports success from an always-truthy array filter would hide exactly the finding the plan exists to produce.

Index this workspace with the shipped binary and record cold build time, warm-start time, on-disk size, and median and ninety-fifth-percentile query latency over a fixed query set, per platform. Run each timing at least three times and record all runs, not just the best — the Linux host is 96 cores and the Windows box is shared, so a single sample is not a measurement. When setting any environment variable in the Windows command shell, use the quoted form and PROVE the value took effect before trusting a run that depends on it; the unquoted form appends a trailing space and has already produced one confidently wrong conclusion in this program.

Drive incremental invalidation with real mutations in a scratch clone, never in the phase's measurement checkout: add a file, edit a file, delete a file, rename a file without changing its content, and switch to a branch with a differing file. After each, capture what the index changed and assert the unchanged files were not re-extracted. Remove the scratch clone when done.

Plant a run-time-generated nonce in a gitignored file, rebuild, and search the store file's own bytes for it. Zero occurrences is the pass condition; any occurrence is a CRITICAL finding that stops the plan.

Run the retrieval-quality corpus through the shipped binary's search operation and record precision and recall against the floor. Drive the exact-search fallback with a punctuation-heavy literal and capture the product's report that it fell back. Drive the staleness case by editing an indexed file and capturing the stale verdict on the hit.

Run all of it three times, each against the exact commit under test and each with a nonce the caller generates at run time: Linux on `hetzner-dsm` against a release binary built there after `git checkout -q --detach $SHA`; Windows on `SeanDesktop` through the PowerShell port after the same detached checkout on `C:\ferrox-win`, with the ssh command string ending in an explicit `exit $LASTEXITCODE` and NEVER piped into a filter; macOS on this Mac against the binary `scripts/f23-macos-binary.sh` resolves, which is a real local invocation of the real product rather than an evidence-file grep. Each leg's ssh or local exit status is the primary gate; the nonce-bound terminal marker in the captured log is the second, independent one. Expect Windows to be the platform that finds defects — the handoff records that mandatory byte-range locks break a crate's own readers there, and a SQLite-backed index doing concurrent access will meet that class directly. A Windows-only failure is a real finding at its own severity, not a platform exception.

Write `23B-03-LIVE-EVIDENCE.md` carrying, per platform: the three timing samples for cold build and warm start, the on-disk size, the median and ninety-fifth-percentile latency, the five incremental-mutation results, the secret-isolation result, the measured precision and recall against the floor, the fallback report and the staleness verdict. Record every gate threshold alongside its measurement and state explicitly whether the threshold was chosen before or after the measurement. A threshold widened after a failure must be called out as such. Marks F23-06 complete only if every mandatory clause is PASS on all three platforms; if only the optional semantic layer is deferred, record that non-claim and mark F23-06 complete with the deferral named.</action>
  <verify>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; test -x scripts/f23-macos-binary.sh &amp;&amp; test -x scripts/f23-index-drive.sh &amp;&amp; test -f scripts/f23-index-drive.ps1 &amp;&amp; bash -n scripts/f23-index-drive.sh</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-03-linux-drive.log &amp;&amp; ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland &amp;&amp; git fetch -q origin plan/f20-unified-audit-repair &amp;&amp; git checkout -q --detach $SHA &amp;&amp; cargo build --release -p wcore-cli --bin wayland-core &amp;&amp; bash scripts/f23-index-drive.sh --binary target/release/wayland-core --sha $SHA --nonce $NONCE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_03_DRIVE=PASS platform=linux nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; test "$(uname -s)" = Darwin &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; BIN=$(bash scripts/f23-macos-binary.sh) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-03-macos-drive.log &amp;&amp; bash scripts/f23-index-drive.sh --binary "$BIN" --sha "$SHA" --nonce "$NONCE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_03_DRIVE=PASS platform=macos nonce=$NONCE" "$L"</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; mkdir -p .planning/phases/23B-continuous-agency/evidence &amp;&amp; NONCE=$(/usr/bin/openssl rand -hex 8) &amp;&amp; SHA=$(/usr/bin/git rev-parse HEAD) &amp;&amp; L=.planning/phases/23B-continuous-agency/evidence/23B-03-windows-drive.log &amp;&amp; ssh -o BatchMode=yes SeanD@seandesktop "Set-Location C:\ferrox-win; git fetch -q origin plan/f20-unified-audit-repair; git checkout -q --detach $SHA; if (\$LASTEXITCODE -ne 0) { exit 91 }; cargo build --release -p wcore-cli --bin wayland-core; if (\$LASTEXITCODE -ne 0) { exit 90 }; if (-not (Test-Path scripts\f23-index-drive.ps1)) { exit 94 }; powershell -NoProfile -ExecutionPolicy Bypass -File scripts\f23-index-drive.ps1 -Binary target\release\wayland-core.exe -Sha $SHA -Nonce $NONCE; exit \$LASTEXITCODE" > "$L" 2>&amp;1; rc=$?; test "$rc" -eq 0 &amp;&amp; /usr/bin/grep -qF "F23_03_DRIVE=PASS platform=windows nonce=$NONCE" "$L"</automated>
    <!-- Every measurement is asserted against the CAPTURED DRIVE LOGS written by the three gates above, never against 23B-03-LIVE-EVIDENCE.md. A word-count over that table could not tell a measured number from a typed one — and the three replaced gates would each have passed on prose alone. Cold build and warm start additionally require the three samples the action mandates, so a single sample cannot be reported as a measurement. RED at base: no log exists. -->
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for P in linux macos windows; do L=.planning/phases/23B-continuous-agency/evidence/23B-03-$P-drive.log; test -f "$L" || exit 1; N=$(/usr/bin/grep -oE 'nonce=[0-9a-f]{16}' "$L" | tail -1 | cut -d= -f2); test -n "$N" || exit 1; /usr/bin/grep -qF "F23_03_DRIVE=PASS platform=$P nonce=$N" "$L" || exit 1; for M in cold-build warm-start store-size latency-p50 latency-p95 precision recall; do /usr/bin/grep -qE "^F23_03_MEASURE=$M platform=$P sample=[0-9]+ value=[0-9][0-9.]*" "$L" || exit 1; done; for M in cold-build warm-start; do test "$(/usr/bin/grep -cE "^F23_03_MEASURE=$M platform=$P sample=" "$L")" -ge 3 || exit 1; done; done</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for P in linux macos windows; do L=.planning/phases/23B-continuous-agency/evidence/23B-03-$P-drive.log; /usr/bin/grep -qE '^F23_03_STORE_NONCE_OCCURRENCES=0$' "$L" || exit 1; for M in add edit delete rename branch-switch; do /usr/bin/grep -qE "^F23_03_MUTATION=$M platform=$P status=PASS unchanged_reextracted=0" "$L" || exit 1; done; done</automated>
    <automated>cd /Users/seandonahoe/dev/waylandcore-ferrox &amp;&amp; for P in linux macos windows; do L=.planning/phases/23B-continuous-agency/evidence/23B-03-$P-drive.log; /usr/bin/grep -qE '^F23_03_FALLBACK_REPORTED=true$' "$L" &amp;&amp; /usr/bin/grep -qE '^F23_03_STALENESS_REPORTED=true$' "$L" || exit 1; done</automated>
  </verify>
  <done>All three drive legs ran against the exact commit under test and each exited zero with its own fresh nonce echoed in the terminal PASS marker: Linux over ssh to `hetzner-dsm`, Windows over ssh to `SeanDesktop` with the status carried by an explicit `exit $LASTEXITCODE` and never through a pipeline, and macOS by invoking the real binary locally through 23B-01's `scripts/f23-macos-binary.sh`. Each binary's `--build-info` source SHA equalled the commit under test, so every number is attributable to an identified build. `23B-03-LIVE-EVIDENCE.md` carries measured cold-build, warm-start, store-size and median and ninety-fifth-percentile latency numbers on all three platforms, each from at least three samples with all samples recorded, plus the three run nonces. The five incremental mutations are each driven for real and their results recorded per platform. The gitignored-file nonce is absent from the store's bytes on all three platforms. Precision and recall are recorded against a floor whose choice order is stated. The fallback and staleness reports are captured. Every gate threshold is recorded beside its measurement, and any threshold widened after a failure is explicitly called out. F23-06's disposition is stated, including the semantic layer's deferral if that state was taken.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| repository filesystem → index store | Arbitrary repository content, including content an attacker may control in a checked-out branch, is read and persisted |
| index store → agent context window | Retrieved file content is injected into a prompt the model acts on |
| operator query → store | Operator-supplied query strings reach a SQLite full-text query |
| gitignore and scope rules → what is stored | The boundary between indexable source and excluded secret material |

## STRIDE Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation Plan |
|-----------|----------|-----------|----------|-------------|-----------------|
| T-23B03-01 | Information Disclosure | gitignored or out-of-root content persisted into the store | critical | mitigate | Excluded files are never READ into the store; proof is a run-time nonce absent from the store file's own bytes, and the symlink-outside-root case is covered (Task 1, Task 3) |
| T-23B03-02 | Tampering | index poisoning — repository content shaping what the agent retrieves | high | mitigate | Every hit carries provenance naming its file, line, modality and the scope identity it was built against, so retrieved content is attributable rather than anonymous context (Task 2) |
| T-23B03-03 | Information Disclosure | stale results served silently after the working tree moves | high | mitigate | Content-hash and scope-identity comparison on every hit; a drifted store reports staleness on every hit rather than serving quietly (Task 2, Task 3) |
| T-23B03-04 | Tampering | operator query strings reaching SQLite unparameterised | high | mitigate | All queries use bound parameters; the full-text query string is escaped through a dedicated helper the way the memory crate already escapes its match term (Task 2) |
| T-23B03-05 | Denial of Service | unbounded retrieval exhausting the caller's context window or the host | high | mitigate | Every query carries an explicit caller-set limit the store enforces; the existing max-file-bytes and max-lines cutoffs continue to bound ingestion (Task 2) |
| T-23B03-06 | Denial of Service | Windows mandatory byte-range locking breaking the crate's own readers | medium | mitigate | The handoff records that a whole-file lock breaks readers on Windows and that a one-byte sentinel is the working pattern; concurrent access is driven on real Windows hardware in Task 3 and a Windows-only failure is a real finding, not an exception |
| T-23B03-07 | Spoofing | path representation mismatch causing silent lookup misses on Windows | medium | mitigate | Normalise at the comparison boundary on both operands; non-ASCII and deeply nested cases tested on real Windows hardware, because the simplification helper conditionally no-ops on exactly those shapes (Task 1) |
| T-23B03-08 | Repudiation | perf or quality thresholds widened after a failure | medium | mitigate | Every threshold is recorded beside its measurement with the choice order stated; a post-failure widening must be explicitly called out (Task 3) |
| T-23B03-SC | Tampering | package-manager installs | low | accept | This plan adds `rusqlite` and `sha2` to one crate; both are already workspace dependencies at pinned versions, so NO new package enters the lock and the Package Legitimacy Gate is not triggered. A genuinely new crate would trigger it and a blocking human checkpoint before install, and this plan STOPS rather than installing |
</threat_model>

<verification>
- Workspace clippy clean with warnings denied on Linux and on Windows.
- `cargo fmt --all -- --check` clean, run on the Mac.
- `cargo nextest run --profile ci --no-fail-fast` green on `hetzner-dsm` for `wcore-repomap`, `wcore-tools` and `wcore-cli`, and the `wcore-repomap` suite additionally green on real Windows hardware.
- `crates/wcore-repomap/Cargo.toml` declares zero internal `wcore-` dependencies.
- The pre-existing `RepoMap::build` public API and the `wcore-tools` repomap tool and TUI at-reference suites pass unchanged.
- `scripts/f23-index-drive.sh` exits zero on Linux and macOS and the PowerShell port exits zero on Windows.
- Every remote gate pinned the exact commit under test with `git checkout -q --detach $SHA` and asserted a file this plan creates is present before the compiler ran, so no gate could pass on a tree lacking the work.
- No gate in this plan is a pipeline into a filter, and no exit code is read from a block that also emits output. The Windows `wcore-repomap` suite and all three drive legs are closed by their own process exit status first, and the drive legs by a caller-generated nonce echoed in the log second.
- The macOS leg ran a real `wayland-core` binary on this Mac, resolved and provenance-checked by 23B-01's `scripts/f23-macos-binary.sh`; no macOS measurement is closed by grepping the evidence file alone.
- `23B-03-LIVE-EVIDENCE.md` carries all four perf gates and the quality gate measured on all three platforms, plus the three run nonces.

<human-check>Every perf and quality threshold in `23B-03-LIVE-EVIDENCE.md` states whether it was chosen before or after its measurement. A threshold that was widened after a failing run is explicitly labelled as such rather than presented as the original gate.</human-check>
</verification>

<success_criteria>
- Success Criterion 6: a persistent incremental hybrid repository index exists, is driven through the shipped binary against this real workspace on Linux, macOS and Windows, and provides bounded lexical and symbol retrieval with an exact-search fallback, provenance and staleness on every hit, secret and authority isolation proved against the store's own bytes, and measured warm-start, size, latency and retrieval-quality gates.
- The crate's isolation rule holds: zero internal dependencies, no new package in the workspace lock.
- The existing public API and its live consumers are unchanged.
- The optional semantic layer is either shipped behind an off-by-default flag that reports its own unavailability, or deferred with an explicit recorded non-claim.
- Nothing was weakened, ignored, re-gated, timed out differently, deleted, or threshold-widened to reach a gate.
</success_criteria>

<output>
Create `.planning/phases/23B-continuous-agency/23B-03-SUMMARY.md` when done, recording the termination state, the measured perf and quality numbers per platform with their thresholds and choice order, the five incremental-mutation results, the secret-isolation result, any Windows-specific finding with its severity, the semantic layer's disposition, and F23-06's disposition.
</output>
