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

## Still to establish

- [ ] Audit all 34 production unconditional-trues against what each tool actually touches.
- [ ] Give `partition()` a test with REAL shared state (a mock cannot exhibit interference).
- [ ] Verify the honest-negative family (kubectl/aws/gcloud/sql) and do NOT touch it.
