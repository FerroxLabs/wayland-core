# wcore-repomap

Aider-style light symbol extractor and codebase index for `wayland-core`.

**Isolated crate.** No internal `wcore-*` dependencies; no protocol
events. Future waves wire `RepoMap` into the agent tool registry; this
crate alone ships only the indexer and the renderer.

## Usage

```rust
use wcore_repomap::{RepoMap, render::render_compact};

let map = RepoMap::build(std::path::Path::new("."))?;
let view = render_compact(&map);
println!("{view}");
# Ok::<(), wcore_repomap::RepoMapError>(())
```

## Design

See `docs/superpowers/specs/2026-05-14-wcore-super-agent-design.md` §5.6
for the rationale: light regex-based extraction (NOT tree-sitter) keeps
the binary growth in the ~5 MB band per the design contract.

## Languages

W3 covers Rust (`.rs`) and TypeScript/JavaScript (`.ts`, `.tsx`, `.js`,
`.mjs`, `.cjs`, `.jsx`). Python, Go, and others are out of scope for
W3 — extending the dispatcher in `src/extractor/mod.rs` is the way to
add them.

### Rust symbol kinds (and what's intentionally NOT extracted)

Extracted: `fn`, `struct`, `enum`, `trait`, `impl` (inherent and
`impl Trait for Type`), `mod`, `pub use`.

**Intentionally not extracted: `const` and `static` declarations.** The
design contract §5.6 line 918 names "fn, struct, enum, impl, mod,
pub use" — `const` and `static` are omitted by design. A future
reviewer who reads the regex set and wonders "why no const?" should
see this note: spec said no, plan said no. Adding `const`/`static`
support is a separate, additive change (one new regex pattern + one
new `SymbolKind` variant + tests).

`trait` IS extracted even though the spec omits it: the engine itself
relies on the `LlmProvider` trait, and "where is `LlmProvider`
defined?" is one of the spec's named acceptance queries. Adding
`trait` is consistent with the spec's intent.

## Acceptance gates (W3 follow-up)

The design contract's empirical acceptance has **three unverified
claims** that W3 is shaped-to-hit but does not measure:

1. **Build time <60 s on 5K files.** Linear walker + regex per file is
   ~10 ms/file on modern hardware; 5K files projects to ~50 s, but
   unmeasured.
2. **Index size <50 MB on disk.** `serde`-derived. 150K symbols ×
   ~100 bytes = ~15 MB JSON projection, but no fixture measures it.
3. **Query latency <50 ms.** W3 ships `Vec<FileSummary>` (sorted by
   path); there is no query API in this wave. Linear scan over ~150K
   symbols projects to ~10 ms, but **untested** and **architecturally
   distinct from a real index**. A `find_symbol(name: &str)` helper
   plus a micro-benchmark is the closing gate.

All three are reserved for a **benchmark wave** that runs against real
repos (this engine and the Wayland Desktop app are reasonable
candidates). W3 ships architecture + fixture-level correctness only.
