# 29-DENY — dependency policy: RED -> GREEN, honestly

**Lane:** `lane/29-deny` · **Base:** `plan/f20-unified-audit-repair` @ `ef1d97be`
**Date:** 2026-07-29 · **Measured on:** `hetzner-dsm`, cargo-deny 0.20.2
**Evidence:** `evidence/29-deny/`

**Verdict in one line:** `cargo deny check` went from **exit 5** to **exit 0**, three of the
four findings were closed on substance rather than by configuration, the two that remain carry
dated, fully-traced exceptions, and the gate is proven able to fail in all four sections.
`cargo deny` is now chained into `check-all`. **Phase 29's Success Criterion 1 does NOT flip to
MET** — one of its two blockers is cleared, the other is untouched by this lane.

---

## 1. Before / after, with real exit codes

Exit status captured to a file and read back by a separate call, never inferred from a
pipeline (`echo "EXIT=${PIPESTATUS[0]}"` returns empty in this environment).

| | command | rc | verdict line | `error[]` blocks |
|---|---|---|---|---|
| **BEFORE** | `cargo deny --manifest-path Cargo.toml check` | **5** | `advisories FAILED, bans ok, licenses FAILED, sources ok` | **4** |
| **AFTER** | same | **0** | `advisories ok, bans ok, licenses ok, sources ok` | **0** |

Raw captures: `evidence/29-deny/deny-base.rc` (`WLRC=5` / `WLDONE`),
`evidence/29-deny/deny-final.rc` (`WLRC=0` / `WLDONE`), with distilled outputs alongside
(the full captures are 1.3 MB and 1.7 MB, ~95% `warning[duplicate]` trees; the distilled
files state the full line/byte counts and reproduce every `error[]` block verbatim).

**Exit codes here are a bitmask** — advisories=1, bans=2, licenses=4, sources=8 — which
independently cross-checks the baseline: `5 = 1|4` = advisories + licenses, exactly the two
sections that read FAILED. That was verified, not assumed, by the falsification battery in §4.

Side effects worth recording: `cargo audit` went **7 -> 6** warning-class advisories
(`evidence/29-deny/audit-after.txt`, exit 0, `warning: 6 allowed warnings found`), and the
cargo-deny duplicate-version warnings went **60 -> 59** because `syn` 1.x left the tree with
`proc-macro-error`. (They then read 76 after the graph was widened in §3 — a wider graph, not
a regression.)

---

## 2. The four findings, and what happened to each

### 2.1 `error[unlicensed]: wcore-fixture-harness = 0.1.0` — FIXED, one line

`deny.toml` sets `private = { ignore = true }`, which cargo-deny honours only for crates that
carry `publish = false`. `crates/wcore-fixture-harness/Cargo.toml` had neither `license` nor
`publish`, and was the **only** workspace member missing a license key (swept all
`crates/*/Cargo.toml`). Added `license.workspace = true` — Apache-2.0, identical to every
sibling. Falsification F1 confirms reverting that single line puts the gate back to
`licenses FAILED`, rc=4.

### 2.2 `RUSTSEC-2024-0370` proc-macro-error — ELIMINATED AT SOURCE

Sole path: `proc-macro-error 1.0.4 <- utoipa-gen 4.3.1 <- utoipa 4.2.3 <- wcore-acp <- wcore-cli`.
`utoipa-gen` 4.x declares `proc-macro-error ^1.0` as a **required** dependency; `utoipa-gen`
5.x dropped it entirely. Workspace pin moved `utoipa = "4.2"` -> `"5"`.

**The stated blocker for this bump was false, and cheaply checkable.** `Cargo.toml:241` read
*"Pinned to 4.x for axum 0.7 compatibility; utoipa 5 requires axum 0.8 (a repo-wide bump)"*,
and `.github/osv-scanner.toml` echoed it as *"a breaking major; chasing it ... is not worth the
REST-surface regression risk"*. Measured against crates.io metadata:

- **Neither utoipa 4.2.3 nor utoipa 5.5.0 declares `axum` as a dependency**, optional or
  otherwise. `axum_extras` is a codegen-behaviour flag forwarded to `utoipa-gen?/axum_extras`.
  The axum-0.8 coupling is *semantic* — utoipa 5 infers params from axum 0.8 extractors and
  `/{id}` routes.
- **This repo does not use that inference.** `wcore-acp/src/transport/rest.rs` declares every
  path parameter explicitly (`params(("id" = String, Path, ...))`) and already wrote `{id}`.

Result: `cargo update -p utoipa` moved **2** packages and left **203** unchanged; axum stayed
at **0.7.9**; `cargo check -p wcore-acp --all-targets` passed with **zero** source changes.
`cargo tree -i proc-macro-error@1.0.4` now reports *"did not match any packages"*.

**Wire-visible consequence — flagged for the orchestrator.** `GET /openapi.json` now emits
**OpenAPI 3.1.0** where it emitted 3.0.3. There is no checked-in OpenAPI fixture anywhere in
the repo (`find -iname "*openapi*"` -> 0 files) and `wcore-contract generate` is **not**
implicated, so nothing needed regenerating. Two live assertions pinned the version
(`rest.rs:949`, `tests/rest_roundtrip.rs:185`, both `starts_with("3.0")`); both were changed to
`"3.1"`. **That is an updated fact of equal strictness, not a relaxed assertion** — each still
fails on an absent, malformed or wrong version, and each is now accompanied by a comment saying
why it changed. `cargo test -p wcore-acp`: **148 executed, 0 failed, 0 ignored, 0 filtered out**
across five binaries (129/2/2/11/4); the sixth run is `Doc-tests wcore_acp` at 0, which is
pre-existing (the crate has no doctests) and is named here rather than folded into the total.

### 2.3 + 2.4 `RUSTSEC-2025-0141` bincode and `RUSTSEC-2026-0192` ttf-parser — EXCEPTIONS

Both are `informational = "unmaintained"`. **Both advisory sources were read from
rustsec/advisory-db, not from memory**, and both carry `[versions] patched = []` with **no
`unaffected` range**.

That is decisive, and it kills the obvious-looking fix for one of them: **bincode 2.0.1 and
3.0.0 exist on crates.io**, so "just bump it" looks right. It is wrong. With `patched = []` and
no `unaffected`, every published version of the package is in scope — and the advisory's own
`url` points at the **v3.0** README announcing that development has permanently ceased. Only
removal clears these.

Full parent traces, read out of
`cargo tree -i <crate>@<ver> --all-features --target all --edges normal,build,dev`
(deliberately **wider** than the graph the gate evaluates), after first checking `Cargo.lock`
for multiple resolved versions — each resolves to exactly one, so no multi-version blind spot.
Raw output: `evidence/29-deny/parent-traces.txt`.

**bincode 1.3.3 — 1 direct parent, 1 path:**
```
bincode v1.3.3
└── syntect v5.3.0
    └── wcore-cli v0.12.25
```
Reachability: syntect's **compile-time-embedded** syntax and theme dumps.
`wcore-cli/src/tui/widgets/diff.rs` calls `SyntaxSet::load_defaults_newlines()` and
`ThemeSet::load_defaults()`; the bytes bincode deserializes are the ones compiled into the
syntect rlib. **No user- or network-supplied data reaches bincode on this path** — syntect's
runtime `.sublime-syntax` loader (`yaml-load`) is already off, dropped by an earlier lane to
kill `yaml-rust`. Irreducible: **syntect 5.3.0 is the latest published syntect**; dropping
`dump-load` would remove bincode and also delete TUI syntax highlighting.

**ttf-parser 0.25.1 — 1 direct parent, 1 root (fanning out across five first-party crates
*below* `wcore-tools`, which is fan-out of one third-party chain, not a second path):**
```
ttf-parser v0.25.1
└── lopdf v0.42.0
    └── pdf-extract v0.12.0
        └── wcore-tools v0.12.25
            ├── wcore-agent (-> wcore-cli)   ├── wcore-browser (-> wcore-agent)
            ├── wcore-cli                    ├── wcore-cua (-> wcore-agent)
            └── wcore-mcp (-> wcore-agent / wcore-cli / wcore-skills /
                              wcore-plugin-subprocess / wcore-eval / wcore-evolve /
                              wcore-eval-scenarios[dev])
```
**Reachability stated honestly and not minimised: this path DOES touch untrusted input.**
`wcore-tools` extracts text from user-supplied PDFs via the Read tool, and ttf-parser parses
embedded font tables out of those files. It is accepted **only** because the advisory is
informational-unmaintained with no known vulnerability and **no patch in existence** — there is
nothing to apply. Recorded in `deny.toml` and in BACKLOG: **if a concrete ttf-parser CVE is
ever published, the exception must be DELETED and the PDF path re-examined, not re-justified.**

Irreducible: `lopdf` made `ttf-parser` **optional** in 0.44.0 (it is **required** in 0.42.0), so
a lopdf bump would drop it — but `pdf-extract 0.12.0` declares `lopdf = "^0.42"`, which for a
0.x crate is `>=0.42.0, <0.43.0`, and pdf-extract 0.12.0 is the latest published pdf-extract.
Reaching lopdf 0.44 needs a `[patch]` override carrying a forked third-party crate, which makes
the supply chain worse, not better.

**What was deliberately NOT done.** cargo-deny 0.18+ exposes
`[advisories] unmaintained = "workspace"`, which would silence every transitive unmaintained
advisory in one line and turn the section green instantly. That is a policy weakening dressed as
a config knob. Not taken; `unmaintained` is left at its default so a newly-unmaintained
transitive still fails loudly.

---

## 3. A scope gap inside the green — found, measured, closed

`deny.toml` had `[graph] all-features = false`, so a green verdict certified only the
**default-feature graph**, not the lockfile. Measured both ways:

| run | rc | result |
|---|---|---|
| `cargo deny --all-features check advisories` | 1 | **3 additional** `error[unmaintained]`: `paste`, `number_prefix`, `rustls-pemfile` |
| `cargo deny --all-features check licenses bans sources` | 0 | 0 errors |
| `cargo tree -i paste@1.0.15` (no `--all-features`) | — | *"did not match any packages"* (same for the other two) |

The advisory half of that gap is duplicated coverage — `cargo audit` and
`.github/osv-scanner.toml` both read the whole lockfile. **The licence half is not.** Nothing
else in this repository checks the licence of a dependency behind an optional feature, so under
the old setting a GPL/AGPL crate arriving via `hf-hub` or `bollard` would have passed every gate
we have. That is an uncovered hole, not defence in depth — and closing it measured **free** on
the licences axis.

**Decision: flipped `all-features = true`,** at a cost of exactly three more traced exceptions
(`RUSTSEC-2024-0436` paste, `RUSTSEC-2025-0119` number_prefix, `RUSTSEC-2025-0134`
rustls-pemfile — all reachable only through `wcore-memory/bge-local` and
`wcore-sandbox/live-docker`, both optional and default-OFF, so none is in the shipped binary).

Cross-audit (§4 of the lane brief) split **2-1**: gemini YES, kimi YES, codex NO. codex's
dissent — that growing the exception list from 2 to 5 is the wrong trade in the one repo whose
documented failure mode is a bad exception — is **recorded in `deny.toml` rather than
dismissed**, and is the reason every new entry carries a trace derived here rather than a
cross-reference to an existing one. The internal adversarial pass raised one objection that was
**disproved rather than argued**: that `--all-features` would make the gate flaky by enabling
combinations that do not build. cargo-deny builds a *metadata* graph and never compiles, so
non-building combinations cannot perturb it, and the run returned a verdict for all four
sections.

### 3.1 Deriving those traces caught two pre-existing dispositions that were WRONG

This is the finding I would most want a reviewer to see, because it is a recurrence of the exact
defect class the repo has been burned by, and it was found only by re-deriving traces from the
tool instead of copying the existing justification:

- **`paste` / RUSTSEC-2024-0436.** `.github/osv-scanner.toml` claimed pullers *"the candle SIMD
  stack ... **AND ratatui**"*. Measured: `cargo tree -p ratatui --all-features -e normal |
  grep -c paste` -> **0**. It named a parent that does not exist, and omitted one that does
  (`macro_rules_attribute <- tokenizers <- wcore-memory`). Truth: **10 direct parents, 2 roots**,
  both terminating at `wcore-memory`.
- **`rustls-pemfile` / RUSTSEC-2025-0134.** Claimed *"Transitive **ONLY** via bollard"*.
  Measured: **2** direct parents — `bollard 0.17.1` **and** `rustls-native-certs 0.7.3`. The
  conclusion survives, but only because `rustls-native-certs 0.7.3`'s own sole parent is also
  bollard — a fact the entry had never checked. "Only via X" was asserted where "two edges, one
  root" is the fact.
- **`proc-macro-error` / RUSTSEC-2024-0370.** Carried a *cost* claim ("a breaking major ... not
  worth the REST-surface regression risk") that is a claim about the dependency graph and was
  never read out of the graph. The bump needed **zero** source changes.

All three **corrected or deleted in this lane**, not written up and left — a documented
instrument defect is a defect you have agreed to keep. Logged as `BL-F29-OSV-TRACES-WERE-STALE`.

---

## 4. The gate is proven able to fail

12-case falsification battery, `evidence/29-deny/falsify.sh`, verbatim stdout in
`evidence/29-deny/falsification.txt`. Every mutation must flip green -> red.

| case | mutation | rc | verdict |
|---|---|---|---|
| F0 | none (control) | 0 | all ok |
| F1 | revert the license one-liner | 4 | licenses FAILED |
| F2a | drop `RUSTSEC-2025-0141` from ignore | 1 | advisories FAILED |
| F2b | drop `RUSTSEC-2026-0192` | 1 | advisories FAILED |
| F2c | drop `RUSTSEC-2024-0436` | 1 | advisories FAILED |
| F2d | drop `RUSTSEC-2025-0119` | 1 | advisories FAILED |
| F2e | drop `RUSTSEC-2025-0134` | 1 | advisories FAILED |
| F7 | ban `serde` (a crate that IS in the tree) | 2 | bans FAILED |
| F8 | empty `allow-registry` | 8 | sources FAILED (892 errors) |
| F9 | remove MIT from the allowlist | 4 | licenses FAILED (183 errors) |
| F10 | revert `all-features` to false | 0 | all ok — **expected, reported as a control** |
| F11 | restore (control) | 0 | all ok |

Two deliberate design points. **F2 is run five times, once per ignore id**, because a battery
that drops the whole list at once cannot distinguish a load-bearing exception from a passenger
riding on another's suppression; each id is independently load-bearing. **F10 is the one case
expected to stay green** — narrowing the graph makes three of the five advisories invisible,
which is precisely the gap §3 closed; it is reported as a control, never as a pass.

Also verified: `cargo fmt --all -- --check` exit 0. Shared-file fence —
`git diff $BASE -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs` against the
captured merge-base SHA (`ef1d97be`, captured once, quoted) is **empty**; neither fenced file
was touched.

---

## 5. The `check-all` decision: **chain it. Done, not deferred.**

`justfile:153` is now `check-all: fmt-check lint test-ci hakari-verify audit deny`.

The reasoning, and it is conditioned on the result exactly as the brief framed it:

1. **The recipe's own comment set the condition.** It read *"NOT chained into `check-all`,
   deliberately ... Chain it only once the verdict is clean."* The verdict is now exit 0. The
   condition is met, and leaving it unchained would be ignoring a decision the repo had already
   made.
2. **Had it stayed red, chaining would have been wrong** — it would break every concurrent
   lane's local `check-all` on a policy failure unrelated to their work. That risk is gone.
3. **It is not a duplicate of CI, and this is the argument I had underweighted** until kimi
   raised it in cross-audit: the CI job at `supply-chain.yml:117` sits behind a
   path-relevance guard and **skips on PRs that do not touch the policy paths**. On those
   branches, `check-all` is the *only* place the policy runs at all.

Cross-audit on this was **unanimous YES** (codex, gemini, kimi).

Proven, not assumed: `just --dry-run check-all` expands to six commands ending in
`vx cargo deny --manifest-path Cargo.toml check`; with `deny` removed from the recipe the same
command expands to five. The chaining is real and its absence is detectable.

---

## 6. Live evidence — the real binary on the real wire

A green unit test is not evidence for a change to a served wire surface. Full transcript:
`evidence/29-deny/live-openapi.txt`; served document: `evidence/29-deny/live-openapi.json`.

`target/debug/wayland-core 0.12.25`, `acp serve --bind 127.0.0.1:18929`, real HTTP:

```
http_status=200        bytes=18286
openapi_version=3.1.0
path_count=8           schema_count=21
/v1/sessions, /v1/sessions/{id}, /v1/sessions/{id}/prompt : all PRESENT
count_3_0_form_nullable_true = 0
count_3_1_form_type_null     = 9        SHAPE_DIFFERENTIAL=PASS
doc_status=200
sessions_no_key_status=401
```

Two things this establishes that the unit tests do not. **The document changed shape, not just
its version string** — nine fields moved from 3.0's `"nullable": true` to 3.1's
`"type": [..., "null"]`, and zero remain in 3.0 form. And **the 200s mean something**, because a
non-carve-out endpoint on the same listener returns 401.

**Instrument repaired mid-run.** The first version of the shape check was written inline as
``echo "... replaces `nullable: true` with ..."``. Backticks inside a double-quoted shell string
are command substitution, so the shell tried to *execute* the phrase it was meant to print
(`nullable:: command not found`) and the label was destroyed. It still printed `0` — the right
answer for the wrong reason, and indistinguishable from "the check never ran". Rather than note
it and move on, it was rewritten as `evidence/29-deny/shapecheck.py`, a real 3.0-vs-3.1
differential with a **three-assertion** self-test: known-positive passes, known-negative fails,
and — the only assertion that proves the repair does anything — **the old matcher scores a
correct 3.1 document and an EMPTY document identically (both 0)**, so it could never have
detected the failure mode it existed to guard.

Not a finding of this lane: `acp serve` cannot mint its server key on a headless box
(`no keychain backend available: Secret Service`), bypassed with `WAYLAND_ACP_SERVER_KEY`.
A separate lane owns headless keyring. No real credential was used; the placeholders are not
secrets.

---

## 7. Does Phase 29's Criterion 1 move? **Partly. It stays PARTIAL.**

C1 is *"Clean-room builds verify provenance, SBOM, dependency policy, signatures, and
reproducibility or documented variance."* `29-PHASE-VERDICT.md` graded it **PARTIAL** for two
named reasons.

| C1 clause | before this lane | after |
|---|---|---|
| **dependency policy** | **NOT MET** — *"the policy now executes and its verdict is FAIL. A clean-room build that runs a policy and does not pass it has not verified it."* | **MET.** Exit 0 across all four sections, policy not weakened by a character, the two remaining advisories held by traced exceptions rather than by loosened rules, and the gate proven able to fail in every section. Additionally now covers optional-feature dependencies, which it did not before. |
| provenance | PARTIAL — the **ACCEPT** path against the real transparency log has never been observed (`F29-LIMIT-04`) | **unchanged.** Out of this lane's scope; no work done on it. |
| signatures | PARTIAL — keyless Sigstore accept path unexercised | **unchanged.** |
| SBOM / reproducibility | MET | unchanged. |

**So: C1's dependency-policy clause moves NOT MET -> MET, and one of the two stated reasons for
C1 being PARTIAL is cleared. C1 itself does NOT become MET,** because the provenance ACCEPT path
is still unobserved and `signatures` is still PARTIAL. Neither was touched here, and neither
should be graded on this lane's evidence.

The related HIGH `F29-02-H1` (the quick-xml pair) was already closed at source by another lane
earlier the same day; this lane did not re-open or re-verify it beyond observing that
`.cargo/audit.toml`'s ignore list is empty and stays empty.

---

## 8. What I did NOT do

- **Did not weaken the policy.** No `unmaintained = "workspace"`, no license added to the
  allowlist, no `[bans]` relaxation, no advisory silenced without a derived trace.
- **Did not fork or `[patch]` any third-party crate** to reach lopdf 0.44.
- **Did not touch** `crates/wcore-cli/src/lib.rs` or `main.rs` (shared-file fence — verified
  empty against the captured merge-base).
- **Did not run** `wcore-contract generate`; no contract fixture is implicated.
- **Did not merge, open a PR, tag, release, or close an issue.**
- **Did not run a full-workspace build or test.** Compilation was scoped to
  `cargo check -p wcore-acp --all-targets`, `cargo test -p wcore-acp`, and one
  `cargo build -p wcore-cli` needed for the live test. The rest of the workspace is unverified
  against the utoipa bump by this lane — **see §9.**
- **Did not create a GitHub issue** for the exception tracking. Tracking is
  `.planning/BACKLOG.md` -> `BL-F29-DENY-UNMAINTAINED` plus the dated `ignoreUntil` entries in
  `.github/osv-scanner.toml`.

## 9. Open, and for the orchestrator to serialize

1. **Wire-visible change: `GET /openapi.json` now emits OpenAPI 3.1.0, not 3.0.3.** No fixture
   exists to regenerate and `wcore-contract` is not involved, but this is a public REST surface.
   Any consumer pinned strictly to 3.0.x must be updated. Flagged, not silently landed.
2. **The utoipa bump is compile-verified only for `wcore-acp` and `wcore-cli`.** `utoipa` has
   exactly one direct consumer (`wcore-acp`) and `cargo update -p utoipa` left 203 packages
   unchanged, so a wider break is unlikely — but "unlikely" is not "measured", and I am saying
   so rather than implying full-workspace coverage. A full-workspace run at merge time will
   settle it; per the brief, take any failure cluster in one crate back to that crate in
   isolation before calling it a regression.
3. **`Cargo.lock` is modified** (4 insertions, 29 deletions: utoipa/utoipa-gen 4->5,
   proc-macro-error + proc-macro-error-attr removed). Any lane merged after this one that also
   touches `Cargo.lock` will conflict there.
4. **`deny.toml` now runs against the all-features graph in CI as well as locally.** A lane that
   adds a dependency behind an optional feature will now be evaluated on it. That is intended.
