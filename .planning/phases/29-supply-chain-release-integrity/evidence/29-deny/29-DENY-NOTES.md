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

---

## T+35 — BASE VERDICT MEASURED (hetzner, cargo-deny 0.20.2, commit ef1d97be)

Command: `cargo deny --manifest-path Cargo.toml check`
Working dir: `/root/wayland-29-deny` (hetzner worktree `hz/29-deny` at ef1d97be).
Exit code captured to a file, read back by a separate call (brief §6b-ii pattern):

```
WLRC=5
WLDONE
```

Output: **20862 lines / 1351920 bytes** (`wc -l -c`). Final line:

```
advisories FAILED, bans ok, licenses FAILED, sources ok
```

Counts read out of the capture, not assumed:
`grep -c "^error\["` = **4**; `grep -c "^warning\["` = **62**
(60 `duplicate` + 1 `no-license-field` + 1 `unlicensed`).

### The 4 errors, verbatim headers with capture line numbers

| line | error |
|---|---|
| 6208 | `error[unlicensed]: wcore-fixture-harness = 0.1.0 is unlicensed` |
| 20771 | `error[unmaintained]: Bincode is unmaintained` — RUSTSEC-2025-0141 |
| 20795 | `error[unmaintained]: proc-macro-error is unmaintained` — RUSTSEC-2024-0370 |
| 20819 | `error[unmaintained]: \`ttf-parser\` is unmaintained` — RUSTSEC-2026-0192 |

The brief's "1 unlicensed + 3 unmaintained" is CONFIRMED against the tool's own output.

## T+40 — RustSec advisory sources read (NOT inferred)

Fetched from `raw.githubusercontent.com/rustsec/advisory-db/main/crates/<pkg>/<id>.md`.
**All three are `informational = "unmaintained"` with `[versions] patched = []` and NO
`unaffected` range.** That is decisive and it kills one obvious-looking fix:

- `bincode`: `patched = []`. bincode **3.0.0 exists on crates.io** and it would be natural to
  assume a bump clears this. It does **not** — with `patched = []` and no `unaffected`, EVERY
  published version of `bincode` is in scope, and the advisory URL literally points at the
  v3.0 README announcing the cessation. Bumping bincode is not a fix.
- `ttf-parser`: `patched = []`. Same — no version clears it.
- `proc-macro-error`: `patched = []`. Same for that package — but the fix here is not a bump
  of `proc-macro-error`, it is **removing it from the tree** (see below).

## T+50 — reducibility measured per finding, from crates.io metadata

### 1. `wcore-fixture-harness` unlicensed — FIXABLE, one line

`deny.toml:49` sets `private = { ignore = true }`, which cargo-deny only honours for crates
carrying `publish = false`. `crates/wcore-fixture-harness/Cargo.toml` has neither `license`
nor `publish`. Every other workspace member uses `license.workspace = true`
(`[workspace.package] license = "Apache-2.0"`, `Cargo.toml:158`). Fix = add that one line.

### 2. `proc-macro-error` (RUSTSEC-2024-0370) — FIXABLE AT SOURCE via utoipa 5

Path: `proc-macro-error 1.0.4 <- utoipa-gen 4.3.1 <- utoipa 4.2.3 <- wcore-acp <- wcore-cli`.

crates.io dependency metadata, read per version:

- `utoipa-gen 4.3.1` deps include `proc-macro-error ^1.0` (**required**, not optional).
- `utoipa-gen 5.5.0` deps: `proc-macro2`, `quote`, `syn ^2.0`, + optionals. **`proc-macro-error`
  is absent entirely.** So utoipa 5 removes the crate from the tree.

**The repo's own pin comment (`Cargo.toml:241`) says "utoipa 5 requires axum 0.8 (a repo-wide
bump)". Measured against crates.io metadata, that is not true at the dependency level:**
`utoipa 5.5.0`'s full dependency list is `indexmap`, `serde`, `serde_json`, `serde_norway`(opt),
`utoipa-gen`(opt) — **zero axum, not even optional**. `utoipa 4.2.3` likewise has no axum dep.
`axum_extras` is a *codegen-behaviour* feature that forwards to `utoipa-gen?/axum_extras`; it
has never pulled axum as a dependency. The real axum-0.8 coupling is *semantic*: utoipa 5's
`axum_extras` inference targets axum 0.8's `Path` extractor and `/{id}` route syntax.

**And this repo does not depend on that inference.** `crates/wcore-acp/src/transport/rest.rs`
declares every path parameter explicitly — `params(("id" = String, Path, description = ...))`
— and already writes brace syntax `path = "/v1/sessions/{id}"`, which is the 0.8/utoipa-5 form.
So the stated blocker for this bump does not apply to the code that exists.

**What the bump DOES change (this is the real cost, and it is wire-visible):** utoipa 5 emits
**OpenAPI 3.1.0** instead of 3.0.3. `GET /openapi.json` is a public REST surface. There is NO
checked-in OpenAPI fixture anywhere in the repo (`find -iname "*openapi*"` -> 0 files), so
nothing needs regenerating and `wcore-contract generate` is NOT implicated. But two live
assertions pin the emitted version:
`crates/wcore-acp/src/transport/rest.rs:949` and `crates/wcore-acp/tests/rest_roundtrip.rs:185`,
both `starts_with("3.0")`. Those must become `"3.1"` — an updated factual expectation of equal
strictness, NOT a relaxation. Flagging for the orchestrator regardless.

### 3. `bincode` (RUSTSEC-2025-0141) — NOT fixable here

Path: `bincode 1.3.3 <- syntect 5.3.0 <- wcore-cli`.
- syntect max published = **5.3.0** (crates.io). There is no newer syntect.
- syntect 5.3.0 declares `bincode ^1.0` as **optional**, enabled by `dump-load`/`dump-create`.
- `Cargo.toml:359` already runs syntect with `default-features = false` and a hand-picked
  feature set (a previous lane dropped `yaml-load` to kill `yaml-rust`/RUSTSEC-2024-0320).
  The retained `default-syntaxes` + `default-themes` + `dump-load` are precisely what
  `crates/wcore-cli/src/tui/widgets/diff.rs` needs — it calls
  `SyntaxSet::load_defaults_newlines()` / `ThemeSet::load_defaults()`, which read syntect's
  bundled **bincode** dumps. Dropping `dump-load` deletes TUI syntax highlighting.
- Bumping bincode does not help (`patched = []`, see T+40).
=> genuine exception candidate.

### 4. `ttf-parser` (RUSTSEC-2026-0192) — NOT fixable here

Path: `ttf-parser 0.25.1 <- lopdf 0.42.0 <- pdf-extract 0.12.0 <- wcore-tools <- (many)`.
- `lopdf 0.44.0` makes `ttf-parser` **optional** (0.42.0 has it **required**) — so a lopdf bump
  would in principle drop it. **But `pdf-extract 0.12.0` requires `lopdf = "^0.42"`**, which for
  a 0.x crate means `>=0.42.0, <0.43.0`. lopdf 0.44 is out of range.
- `pdf-extract` max published = **0.12.0**. No newer release relaxes that bound.
- Reaching lopdf 0.44 therefore requires forking/patching `pdf-extract`, i.e. a `[patch]`
  section carrying a third-party crate. Not a supply-chain improvement.
=> genuine exception candidate.

## T+55 — prior art found: `.github/osv-scanner.toml` already disposition ALL THREE

Dated, reasoned `[[IgnoredVulns]]` entries with `ignoreUntil = 2026-09-02` exist for
RUSTSEC-2025-0141, RUSTSEC-2024-0370 and RUSTSEC-2026-0192. That file also carries the
rotation recipe I must follow: run `cargo tree -i <crate>@<ver>` **once per resolved version**
and state the path count explicitly. Consequences for this lane:
- deny.toml exceptions must match that format's rigour (id + reason + full trace + tracking).
- the RUSTSEC-2024-0370 entry in osv-scanner.toml must be **deleted** once utoipa 5 lands,
  following the established "eliminated at source, not re-justified" pattern.

## Next
Derive traces with `cargo tree -i` per resolved version; then apply fixes in order.
