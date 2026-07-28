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

---

## T+95 — GREEN, and proven able to fail

`cargo deny --manifest-path Cargo.toml check` at the fixed tree:
`WLRC=0` / `WLDONE`, **0 error blocks**, final line `advisories ok, bans ok, licenses ok, sources ok`.
Duplicate warnings 60 -> 59: the vanished pair is `syn` (proc-macro-error carried syn 1.x).

8-case falsification battery (`evidence/29-deny/falsify.sh`, verbatim log in
`falsification.txt`). cargo-deny's exit code is a **bitmask** — advisories=1, bans=2,
licenses=4, sources=8 — which cross-checks the baseline: exit **5 = 1|4** = advisories +
licenses, exactly the two sections that read FAILED. Every section flips independently:

| case | mutation | rc | verdict |
|---|---|---|---|
| F0 | none (control) | 0 | all ok |
| F1 | revert the license one-liner | 4 | licenses FAILED |
| F2 | drop RUSTSEC-2025-0141 from ignore | 1 | advisories FAILED |
| F3 | drop RUSTSEC-2026-0192 from ignore | 1 | advisories FAILED |
| F4 | ban `serde` | 2 | bans FAILED |
| F5 | empty `allow-registry` | 8 | sources FAILED |
| F6 | drop MIT from the allowlist | 4 | licenses FAILED |
| F7 | restore (control) | 0 | all ok |

F2/F3 matter most: each ignore id is load-bearing on its own, so neither is a
blanket suppression riding on the other.

## T+105 — `cargo audit` re-measured: 7 -> 6

`cargo audit` exit 0, `warning: 6 allowed warnings found` (5 unmaintained + 1 unsound),
`proc-macro-error` absent. `.cargo/audit.toml`'s header claimed 7; corrected.

## T+115 — SCOPE GAP FOUND IN THE GREEN (this is the important one)

`deny.toml` sets `[graph] all-features = false`, so a green `cargo deny` certifies only the
**default-feature graph**, not the lockfile. Measured, not assumed:

- `cargo deny --all-features check advisories` -> **exit 1, 3 extra `error[unmaintained]`**:
  `paste`, `number_prefix`, `rustls-pemfile` (via candle SIMD / hf-hub / bollard, all
  optional and default-OFF).
- `cargo deny --all-features check licenses bans sources` -> **exit 0, 0 errors.**

So widening the graph costs exactly three advisory exceptions and nothing else.

## T+125 — cross-audit panel (§4) on the two judgement calls

Q1 "chain `cargo deny` into `check-all` now that it is green?" — **codex YES, gemini YES,
kimi YES. Unanimous.** kimi added the argument none of the others made and that I had not
weighted: the CI job sits behind a *path-relevance guard*, so on PRs that do not touch the
policy paths cargo-deny does not run at all — which makes `check-all` a genuine backstop,
not a duplicate.

Q2 "flip `all-features` to true, buying optional-feature coverage for 3 more exceptions?" —
**codex NO, gemini YES, kimi YES (2-1).**

Internal adversarial pass, arguing AGAINST the YES majority, and how it resolved:
1. *"`--all-features` is flaky on a workspace this size — mutually exclusive / platform
   features will make the gate fail for reasons unrelated to supply chain."* **Disproved by
   the measurement itself:** cargo-deny builds a *metadata* graph and never compiles, so
   feature combinations that would not build do not perturb it. The `--all-features` run
   completed and produced a verdict for all four sections.
2. *"Exception count 2 -> 5 in the one repo whose documented failure mode is a bad
   exception."* Real, and it is the reason codex says NO. But the response to a bad
   exception is a better exception, not a narrower gate — and all three are already dated,
   traced and public in `.github/osv-scanner.toml`; moving them here consolidates an
   existing decision rather than manufacturing a new one.
3. *"cargo audit already covers the whole lockfile, so the coverage is duplicated."* True
   **for advisories** — and that is codex's strongest point. But it is **false for
   licenses and sources**, and that is decisive: nothing else in this repo checks the
   license of an optional dependency. `cargo audit` does not check licenses; `osv-scanner`
   does not check licenses. Today a GPL/AGPL crate arriving through the `hf-hub` or
   `bollard` path would pass every gate the repo has. That hole is not defence-in-depth,
   it is uncovered — and I measured that closing it costs nothing on the licenses axis
   (exit 0 under `--all-features`).

**DECIDED: Q1 = YES (chain it). Q2 = YES (flip `all-features`).** Q2 follows the majority,
and specifically on the licenses/sources argument rather than the advisories one; codex's
dissent is recorded because its exception-hygiene point is correct and is the reason each
of the three new entries carries a full derived trace rather than a cross-reference.

---

## T+150 — decisions APPLIED, and two pre-existing dispositions found WRONG

Flipping `all-features = true` required deriving traces for the three newly-visible
advisories. Deriving them (rather than copying the existing justifications) caught two
errors in `.github/osv-scanner.toml` — the same defect class as the quick-xml entry, and
the reason the brief insists traces come out of the tool:

- **paste / RUSTSEC-2024-0436** claimed pullers "the candle SIMD stack ... **AND ratatui**".
  Measured: `cargo tree -p ratatui --all-features -e normal | grep -c paste` -> **0**.
  It named a parent that does not exist. It also omitted a root that does:
  `macro_rules_attribute <- tokenizers <- wcore-memory`. Ten direct parents, two roots.
- **rustls-pemfile / RUSTSEC-2025-0134** claimed "Transitive **ONLY** via bollard".
  Measured: **two** direct parents — `bollard 0.17.1` AND `rustls-native-certs 0.7.3`.
  The conclusion survives (rustls-native-certs' own sole parent is bollard) but only
  because of a fact the entry had never checked.
- **proc-macro-error / RUSTSEC-2024-0370** carried a *cost* claim — "a breaking major ...
  not worth the REST-surface regression risk" — which is a claim about the dependency
  graph that was never read out of the graph. The bump needed **zero** source changes.

All three corrected/deleted in the same commit, per the standing rule that a written-up
instrument defect is one you have agreed to keep.

## T+160 — final gate state

`cargo deny --manifest-path Cargo.toml check` (all-features = true, 5 exceptions):
**WLRC=0**, 0 errors, 76 duplicate warnings, `advisories ok, bans ok, licenses ok, sources ok`.

Falsification run #2 — 12 cases, all four sections flip independently, and **each of the
five ignore ids is dropped individually** (a battery that drops the whole list at once
cannot distinguish a load-bearing exception from a passenger). F10 narrows the graph back
and is the one case expected to stay green; it is reported as a control, not a pass.

`just --dry-run check-all` now expands to six commands ending
`vx cargo deny --manifest-path Cargo.toml check`; with `deny` removed from the recipe it
expands to five. The chaining is real and its absence is detectable.

`cargo fmt --all -- --check` -> exit 0. Shared-file fence: `git diff $BASE --
crates/wcore-cli/src/{lib,main}.rs` -> EMPTY (neither touched).

## T+170 — LIVE evidence (brief 3.1): the real binary on the real wire

`target/debug/wayland-core 0.12.25`, `acp serve --bind 127.0.0.1:18929`, real HTTP:

```
http_status=200   bytes=18286
openapi_version=3.1.0        <- the utoipa 5 bump, on the wire
path_count=8   schema_count=21
/v1/sessions, /v1/sessions/{id}, /v1/sessions/{id}/prompt : all PRESENT
count_3_0_form_nullable_true = 0
count_3_1_form_type_null     = 9     SHAPE_DIFFERENTIAL=PASS
doc_status=200
sessions_no_key_status=401           <- negative control
```

Two things this establishes beyond the unit tests: (1) the served document changed
**shape**, not only its version string — nine fields moved from 3.0's `"nullable": true`
to 3.1's `"type": [..., "null"]`; (2) the 200s mean something, because a non-carve-out
endpoint on the same listener returns 401.

**Instrument repaired mid-run (6b-ii).** The first version of the shape check was written
inline in the shell as ``echo "... replaces `nullable: true` with ..."``. Backticks inside a
double-quoted string are command substitution, so the shell tried to EXECUTE the phrase
(`nullable:: command not found`) and the label was destroyed. It still printed `0` — the
right answer for the wrong reason, and indistinguishable from "the check never ran". Rather
than note it, it was rewritten as `shapecheck.py`, a real 3.0-vs-3.1 differential with a
**three-assertion** self-test: known-positive passes, known-negative fails, and — the only
assertion that proves the repair does anything — **the old matcher scores a correct 3.1
document and an EMPTY document identically (both 0)**, so it could never have detected the
failure mode it was there to guard.

Side note, not a finding of this lane: `acp serve` cannot mint its server key on a headless
box (`no keychain backend available: Secret Service`). Bypassed with
`WAYLAND_ACP_SERVER_KEY`. A separate lane owns headless keyring.
