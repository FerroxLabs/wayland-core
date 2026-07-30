# lane/concurrency-safe — NOTES

Base: `c9ab048b952c5bc74c75ea8f76df06788408de59` (asserted via `git rev-parse HEAD`
in the lane worktree). Branch `lane/concurrency-safe`.

Base-choice deviation: the lane brief §1 says branch from `plan/f20-unified-audit-repair`;
my task brief pins `c9ab048b`. Measured: `c9ab048b` IS an ancestor of
`plan/f20-unified-audit-repair`, exactly **1** commit behind, and that commit is
`docs: evening handoff...` — docs only. Took the pinned SHA (what the scoping agent
measured against). Recorded, not buried.

## Instrument

`.planning/scripts/census-concurrency-safe.py` — stdlib-only, brace-balanced body
extraction, comment-stripped classification, `#[cfg(test)] mod` span detection.
Written because rtk rewrites grep/wc counts (LANE-BRIEF §3b). Output is redirected
to a file and read with the Read tool, never through Bash stdout.

Self-test: 5 assertions, `SELFTEST=PASS`.
  A1 known-positive (`true` body, with a comment) -> UNCOND_TRUE
  A2 known-negative (`false` body) -> UNCOND_FALSE
  A3 discrimination (conditional body) -> neither bucket
  A4 bodiless trait decl -> BODILESS
  A5 **old-broken-matcher control**: the naive regex `fn is_concurrency_safe[^{]*\{\s*true\s*\}`
     MISSES the comment-bearing positive. Proves the repair does something.

## PREMISE CORRECTIONS (both prior numbers are wrong)

| source | declarations | "unconditional true" |
|---|---|---|
| orchestrator brief | 97 | 45 |
| scoping agent | 124 | 45 raw / 28 production `/src/` |
| **this lane, measured** | **120** | **52 non-test / 34 production `/src/`** |

The scoping agent's list of 28 production `/src/` unconditional-trues **omits six real
production tools**:
  - `crates/wcore-tools/src/linear_tool.rs:427`
  - `crates/wcore-tools/src/postgres_schema_tool.rs:366`
  - `crates/wcore-tools/src/spotify_tool.rs:988`
  - `crates/wcore-tools/src/spotify_tool.rs:1267`
  - `crates/wcore-tools/src/vision_tools.rs:473`
  - `crates/wcore-tools/src/web_tools.rs:753`

Full census: `.planning/evidence/lane-concurrency-safe/census.txt`.

## Core defect — CONFIRMED verbatim at base

- `crates/wcore-tools/src/doc_tool.rs:215-218` — `is_concurrency_safe` returns `true`
  with the comment "Read-only filesystem access — safe to run alongside other tools."
- `:363` calls `write_doc_artifact`.
- `:388-403` `#[cfg(feature = "doc-extract")] fn write_doc_artifact` does
  `create_dir_all` + `fs::write` into `std::env::temp_dir().join("wayland-doc-extract")`.
- Feature is default-on.
- So the comment is false: the tool is NOT read-only.

Blast radius, stated honestly: filename is `{hash:016x}.md` over
(display, len, full_markdown), so two DIFFERENT documents cannot collide. The window is
two IDENTICAL extractions racing one path while the model is told to `read` it — a torn
read. Not a general data race.

## Still to establish — ALL DONE

- [x] Audit all 34 production unconditional-trues against what each tool actually touches.
- [x] Give `partition()` a test with REAL registered tools (a mock cannot exhibit interference).
- [x] Verify the honest-negative family (kubectl/aws/gcloud/sql) and do NOT touch it.

---

# RESULTS

## Magnitude of the defect (measured, by reverting the fix)

**15,392 torn (partial) reads out of 15,581 successful reads = 98.8%.** With the atomic
fix: **0**. The brief's "torn read, not a general data race" framing is correct and kept;
the magnitude was understated.

## Both-directions proof

| mutation | expected red | observed |
|---|---|---|
| revert atomic write -> in-place `fs::write` | torn-read test | FAILED 15392/15581; 7 others passed |
| flip `doc_extract` safe -> false | `doc_extract_really_is_placed_in_a_parallel_batch` | FAILED (2 batches vs 1); 7 passed |
| make `partition` ignore call input | `input_dependent_real_tool_is_batched_per_invocation` | FAILED; 7 passed |

Each reddened exactly its target, then restored via `git checkout -- <path>` and re-verified.

## Gates (all read from files, unproxied)

- `cargo fmt --all -- --check` = 0
- `cargo metadata --locked` = 0
- `cargo check -p wcore-tools -p wcore-agent --all-targets` = 0
- `cargo clippy -p wcore-tools --all-targets -D warnings` = 0
- `cargo clippy -p wcore-agent --lib -D warnings` = 0
- `cargo clippy -p wcore-agent --all-targets -D warnings` = **101, PRE-EXISTING**
  (`needless_update` at `tests/cache_ledger_engine_test.rs:82:11`; base control at
  `c9ab048b` gives the identical error, `BASE_RC=101`). Not my file, left alone.
- `cargo test -p wcore-tools` = **1240 passed / 0 failed / 5 ignored / 0 filtered**
- new doc_tool tests = **3 passed / 0 failed / 0 ignored**
- new partition tests = **8 passed / 0 failed / 0 ignored**
- `cargo test -p wcore-agent --lib -- --test-threads=1` =
  **2260 passed / 0 failed / 3 ignored / 0 filtered**, `DONE_RC=0`

## wcore-agent parallel-run failures were CONTENTION, not regression

Parallel full-lib at my HEAD: 2246 passed / 14 failed. Base control at `c9ab048b`:
2231 passed / **21 failed** — base is worse. Same modules fail each time with different
members. Host: load 6.87, 4 concurrent cargo/rustc from other lanes. Isolated re-run of
all 7 affected clusters: **215 tests, 0 failed**. Single-threaded full lib: **0 failed**.
My only wcore-agent change is an additive `#[cfg(test)]` module.

## Unrun cells (counted, not hidden)

- 5 `#[ignore]` in wcore-tools, 3 in wcore-agent lib. Not run; not claimed.
- `cargo clippy -p wcore-agent --all-targets` aborts at the pre-existing
  `cache_ledger_engine_test` error, so integration-test targets after it in the build
  order were **not linted**. Pre-existing condition, not introduced here.
- wcore-agent integration-test binaries were not run to completion in the full suite
  (cargo stops after the lib target failed under contention). The lib itself is green
  single-threaded.

## Instrument defects found and REPAIRED in-lane (4 + 2 in my own tests)

1. Line-number shift from length-collapsing blanking. Control: OLD=[5] vs REPAIRED=[11].
2. **Glob-pattern phantom comment** — `"**/*.rs"` read as a block-comment opener, blanking
   ~130 lines of real code. Could hide a real production write => false absence.
   Replaced regex with a real Rust lexer. Control: OLD=[] vs REPAIRED=[2].
3. `#[cfg(all(test, feature=".."))]` not suppressed => test writes reported as production.
4. Delegated writers invisible (`wcore_config::atomic_write`) — caught by the live
   known-positive control on `edit.rs` returning hits=0.
Plus, in my own tests: lowercase tool names (caught by the both-directions registry
control) and a fixed reader round-count that finished before any write landed
(`successful_reads=0`).
