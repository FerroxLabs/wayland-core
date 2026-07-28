# 29-DENY NOTES — running log (append-only, committed after every measurement)

Lane: `lane/29-deny`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-29-deny`.
Base: `plan/f20-unified-audit-repair` @ `ef1d97beb61f1b084bdfba745e8f49830924d757`.
Goal: get `cargo deny check` honestly green, or prove it cannot be — then decide the
`check-all` chaining question.

---

## T+0 — recon done from source (Mac, read-only; NO cargo run yet)

### Where the gate lives today

- **`justfile:153`** — `check-all: fmt-check lint test-ci hakari-verify audit`.
  `cargo deny` is **absent**. Confirmed by reading the recipe, not by grep alone.
- **`.github/workflows/supply-chain.yml:117`** —
  `run: cargo deny --manifest-path Cargo.toml check`, guarded by
  `if: steps.relevance.outputs.needed == 'true'` (a path-relevance step).
  So the gate **does** block `Cargo.lock`-touching PRs in CI today. It is not dormant.
- `deny.toml` has `[advisories] ignore = []` and `[licenses] exceptions = []`.
  **Both exception lists are currently empty.** Anything I add is the first.

### Licenses FAILED — candidate root cause (to be confirmed against real output)

`deny.toml:49` sets `private = { ignore = true }`. cargo-deny only honours that for crates
that are *actually* private, i.e. carry `publish = false`.

`crates/wcore-fixture-harness/Cargo.toml` is the **only** workspace member with neither a
`license` nor a `license.workspace` key (swept all `crates/*/Cargo.toml`), and it also has
**no `publish` key**, so `private.ignore` cannot cover it. That is consistent with a
one-line fix, but the fix must be verified against the tool's actual finding, not assumed.

### Advisories FAILED — utoipa 4 -> 5 is NOT obviously cheap

`Cargo.toml:241` carries an explicit, deliberate pin comment:

```
# Pinned to 4.x for axum 0.7 compatibility; utoipa 5 requires axum 0.8
# (a repo-wide bump). `axum_extras` adds the Path-extractor glue ...
utoipa = { version = "4.2", features = ["axum_extras"] }
```

`axum 0.7` is declared directly by **five** crates:
`wcore-acp`, `wcore-eval-scenarios`, `wcore-agent`, `wcore-cli` (and referenced in
`wcore-agent`'s inbound-webhook comment). `Cargo.lock` resolves a single `axum 0.7.9`.
Only `wcore-acp` depends on `utoipa`.

So "take utoipa 4 -> 5" as written may transitively mean "bump axum 0.7 -> 0.8 across five
crates". **Open question to settle with evidence, not opinion:** does utoipa 5 pull axum
only through the `axum_extras` feature? If yes, dropping/keeping that feature decides
whether this is a one-line bump or a repo-wide one. Must be measured, not reasoned.

### Standing constraint on any exception I write

`.cargo/audit.toml`'s rotation policy (written after the quick-xml entry was found wrong in
three ways) demands, for every exception: **every** parent path derived mechanically from
`cargo tree -i`, a threat-model note per path, a tracking item, a date — never a bare id,
never a trace asserted from memory. The advisory's own `patched`/`unaffected` ranges must be
read from the RustSec source before claiming any version is out of scope.

### Environment

- Mac: NO cargo (brief §0). Only `cargo fmt --all -- --check`.
- hetzner-dsm: `cargo-deny 0.20.2` present at `/root/.cargo/bin/cargo-deny`; `/root` 712G free;
  load 4.76. All measurement runs happen there.

---

## Still to establish

1. Real `cargo deny check` verdict at `ef1d97be` — full output, byte count, real exit code.
2. Exact license finding (crate + reason) and whether the one-line fix clears it.
3. The three unmaintained advisory ids, each with a **complete** `cargo tree -i` parent trace.
4. Whether utoipa 5 is tractable without an axum bump.
5. `check-all` chaining decision, conditioned on (1)-(4).
