# NOTES — lane/peer-reference-scrub

Base SHA: `37547dd900f8016b2d834f4ef14368add0ee988d`
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-peer-reference-scrub`

## Objective

Remove **attributive / comparative commentary** referencing the peer projects Hermes and
OpenClaw from shipped source (`crates/`) and `docs/`. We did not take code from either — they
are Python/TypeScript, this is Rust. The commentary creates a false impression of derivation.

**KEEP everything functional.** The product migrates users *off* those tools; the migration
subsystem must keep naming them (identifiers, string literals, config keys, paths, CLI help,
test fixtures). `migrate --help` listing both peers is a shipping feature.

## Baseline measurement (unproxied `/usr/bin/grep`, counts read back via Read tool)

Query: `/usr/bin/grep -rEi "hermes|openclaw" --include="*.rs" crates/`
- **393 hits across 44 `.rs` files**  (matches the orchestrator's brief — premise verified)

Query: `/usr/bin/grep -rEi "hermes|openclaw" --include="*.md" docs/`
- **41 hits across 7 `.md` files**  (matches the brief)

Instrument liveness: the same grep returns a large non-zero on the known-positive
`crates/wcore-cli/src/migrate/openclaw.rs` (67), so the instrument is alive.
Globs are quoted (`"--include=*.rs"`) — zsh eats them unquoted and yields a false 0.

### Per-file baseline (.rs), descending

```
crates/wcore-cli/src/migrate/openclaw.rs:67
crates/wcore-cli/tests/migrate_typed_dryrun.rs:55
crates/wcore-cli/src/migrate/mod.rs:36
crates/wcore-cli/tests/migrate_quarantine.rs:33
crates/wcore-cli/src/migrate/hermes.rs:32
crates/wcore-cli/tests/migrate_hermes.rs:27
crates/wcore-config/src/portability/mod.rs:21
crates/wcore-eval-scenarios/tests/claims_honesty_corpus.rs:12
crates/wcore-cli/src/migrate/quarantine.rs:11
crates/wcore-providers/src/openai_responses.rs:9
crates/wcore-cli/src/migrate/grok.rs:9
crates/wcore-providers/src/classify.rs:8
crates/wcore-eval-scenarios/tests/frontier_trials_contract.rs:8
crates/wcore-eval-scenarios/src/frontier_trials.rs:8
crates/wcore-cli/src/migrate/gemini.rs:6
crates/wcore-eval-scenarios/src/dialect.rs:4
crates/wcore-eval-scenarios/src/dialect_exec.rs:4
crates/wcore-eval-scenarios/src/claims.rs:3
crates/wcore-cli/src/migrate/provenance.rs:3
crates/wcore-cli/src/migrate/content.rs:3
crates/wcore-types/src/model_aliases.rs:2
crates/wcore-tools/src/registry.rs:2
crates/wcore-providers/src/key_rotation.rs:2
crates/wcore-providers/src/failover.rs:2
crates/wcore-providers/src/cache_observation.rs:2
crates/wcore-eval-scenarios/bin/wayland-scorecard.rs:2
crates/wcore-compact/src/transcript_rewrite.rs:2
crates/wcore-compact/src/identifier_policy.rs:2
crates/wcore-cli/tests/portability_hostile_corpus.rs:2
crates/wcore-cli/src/tui/surfaces/workspace.rs:2
crates/wcore-providers/src/retry.rs:1
crates/wcore-providers/src/anthropic.rs:1
crates/wcore-pricing/src/refresh.rs:1
crates/wcore-config/src/tools.rs:1
crates/wcore-config/src/portability/redact.rs:1
crates/wcore-config/src/config.rs:1
crates/wcore-cli/src/main.rs:1
crates/wcore-cli/src/lib.rs:1
crates/wcore-channels-registry/src/lib.rs:1
crates/wcore-channel-msteams/src/lib.rs:1
crates/wcore-channel-imessage/src/lib.rs:1
crates/wcore-agent/src/engine.rs:1
crates/wcore-agent/src/compact/micro.rs:1
crates/wcore-acp/src/a2a/types.rs:1
```

### Per-file baseline (docs/*.md)

```
docs/design/2026-07-13-wayland-core-frontier-gap-audit-and-execution-plan.md:21
docs/design/2026-07-13-wayland-core-frontier-evaluation-program.md:12
docs/design/2026-07-13-wayland-core-frontier-build-plan.md:4
docs/tools.md:1
docs/providers.md:1
docs/plugin-authors.md:1
docs/design/2026-07-13-wayland-core-f00-characterization.md:1
```

## Plan

1. Classify every hit as FUNCTIONAL (keep) or COMMENTARY (remove/rewrite).
2. Edit only commentary. No renames of files, modules, structs, fns, config keys, CLI verbs.
3. `cargo fmt --all -- --check` clean on the Mac; `cargo check --workspace --all-targets` on
   hetzner; migration tests re-run with executed counts read back.
4. Prove `migrate --help` still names both peers from the built binary.

## Status log

- [t0] Worktree created at base SHA, baseline measured, NOTES committed.
- [t1] Providers cluster scrubbed (`a59bc82a`).
- [t2] Compact/channel/agent/tool/TUI cluster scrubbed (`dbfad22f`).
- [t3] Migrate competitive aside + 2 user docs scrubbed (`52b25137`).
- [t4] All gates run. Evidence below.

## After measurement

Same query as the baseline, same method (redirect to file, read with Read tool):

| Scope | Before | After | Removed |
|---|---:|---:|---:|
| `crates/**/*.rs` | 393 | **351** | **42** |
| `docs/**/*.md` | 41 | **39** | **2** |
| Total | 434 | **390** | **44** |

Files with >=1 hit: 44 -> 25 (19 files went to zero).

### Kept 390, by group

- **309** — migration subsystem (`migrate/*`, `tests/migrate_*`, `wcore-config/src/portability/*`,
  `portability_hostile_corpus.rs`, `config.rs` test cross-ref, `cli/src/{lib,main}.rs`).
  Identifiers, string literals, config keys, path constants, on-disk format descriptions,
  CLI help, test fixtures. Functional — the product imports these tools' data.
- **41** — `wcore-eval-scenarios` (`Cargo.toml:9` = `publish = false`, so NOT shipped).
  `ToolV1::Hermes`/`ToolV1::Openclaw` serde enum variants; `PRODUCT_TOKENS` lists both as
  *forbidden* tokens in a guard; the prose is measurement record, not attribution.
- **38** — `docs/design/2026-07-13-*` competitive-evaluation documents (see AMBIGUOUS).
- **1** — `wcore-acp/src/a2a/types.rs:8`, legal values of the `agent_kind` interop field.
- **1** — `docs/providers.md:380`, ToS disclosure (see AMBIGUOUS).

## Gate results

**The diff is comment-only.** 169 changed `.rs` lines, **0** non-comment. Matcher self-tested
three ways: flags `+    let x = 1;`, flags `-        body["store"] = json!(false);`, passes
`//!` / `//` / `///`. No executable line changed anywhere.

**Shared-file fence:** `git diff <BASE> -- crates/wcore-cli/src/{lib,main}.rs` is EMPTY,
captured with a known-positive control (whole tree = 22 files) in the same invocation.

| Gate | Result | Control |
|---|---|---|
| `cargo fmt --all -- --check` (Mac) | rc=0 | `rustfmt --check`: bad fmt -> 1, parse fail -> 1, good -> 0 |
| `cargo check --workspace --all-targets` (hetzner) | rc=0, 0 `error` lines, 1m43s | 707 log lines, 4.5G target; all 11 edited crates named in log |
| `--test migrate_hermes` | **7 passed; 0 failed; 0 ignored; 0 filtered out** | non-zero executed count |
| `--test migrate_quarantine` | **34 passed; 0 failed; 0 ignored; 0 filtered out** | non-zero executed count |
| `--test migrate_typed_dryrun` | **14 passed; 0 failed; 0 ignored; 0 filtered out** | non-zero executed count |
| `cargo clippy` over the 10 edited crates `--all-targets` | rc=0, 0 errors, 8 warnings | 0 warnings name any edited file |

Test log was `scp`'d to the Mac and read with the Read tool so `0 ignored; 0 filtered out`
survive — the `rtk` cargo proxy strips exactly those two fields. Binary selectors
(`--test <name>`) used, not a name filter (the flavour-(c) zero-test trap).

### Live proof — `migrate --help` still names both peers

From the built `target/debug/wayland-core`:

```
Commands:
  hermes       Import Hermes profiles (`~/.hermes/profiles/*`) into wayland-core
  openclaw     Import an OpenClaw setup (`~/.openclaw`) into wayland-core
  grok         Import a grok setup (`$GROK_HOME` or `~/.grok`) into wayland-core
  gemini       Import a gemini-cli setup (`~/.gemini`) into wayland-core
  quarantined  List imported content held in quarantine
  imported     Show the provenance of content this machine imported ...
  promote      Promote quarantined content out of containment ...
```

`migrate hermes --help` / `migrate openclaw --help` both render, incl.
`--home <HOME>  Source home to import from (default: ~/.hermes or ~/.openclaw)`.
Counts over the capture: hermes **6**, openclaw **5**, known-negative `cursorbot` **0`.

## Pre-existing finding — NOT from this lane, not fixed

`cargo clippy -- -D warnings` fails at
`crates/wcore-agent/tests/cache_ledger_engine_test.rs:82` (`clippy::needless_update`), plus 4
`needless_borrow` warnings in `tests/user_model_identity_wire.rs`. That file is NOT in this
lane's diff — asserted with a known-negative (`cache_ledger_engine_test` -> 0 in
`git diff --name-only`) beside a known-positive (`classify.rs` -> 1) in one capture. Left
alone per no-drive-by-fixes. Without `-D warnings`, clippy is rc=0.

## AMBIGUOUS — kept, needs Sean's decision

1. **`docs/design/2026-07-13-*` (38 hits)** — public competitive gap-audit / evaluation-program
   / build-plan. Doc-side analogue of the out-of-scope `COMPETITIVE-LEDGER.md`. Kept: scrubbing
   names would make them incoherent, and they describe measurement not derivation. BUT
   gap-audit line 456 reads *"Hermes and OpenClaw should be copied where they are operationally
   better"* — "copied" is likely what the audit caught. Real question is whether an internal gap
   audit belongs in public `docs/` at all: keep / move to `.planning/` / delete.
2. **`docs/providers.md:380`** — ToS note; naming the other clients IS the evidence for
   "tolerated in practice". Deleting weakens a user-facing disclosure.
3. **`migrate/openclaw.rs:12-27`** — "GROUNDED in the peer's own source (`src/config/paths.ts`)"
   + two commit SHAs. Functional (justifies the four path constants, and why no platform branch
   is invented) but it is the one functional passage that reads like source access.
4. **`wcore-eval-scenarios`** — kept whole; `publish = false`.
5. **`wcore-acp/src/a2a/types.rs:8`**, **`migrate/quarantine.rs:700-702`** — interop field
   values, and the rule excluding 179 `SKILL.md` files under git checkouts of the peer product
   inside a real `~/.hermes`. Functional.

## Highest-consequence REMOVALS — please eyeball

These carried an actual licence attribution, not a design aside:

- `wcore-channel-imessage/src/lib.rs` — "(OpenClaw MIT, adapted under Apache-2.0)"
- `wcore-channel-msteams/src/lib.rs` — "(OpenClaw MIT + Apache-2.0)"
- `wcore-channels-registry/src/lib.rs` — "ported from desktop OpenClaw fork"
- six `ported from openclaw MIT (c) Peter Steinberger 2025` headers in
  `wcore-providers/{failover,key_rotation,cache_observation,classify,retry}.rs` and
  `wcore-pricing/src/refresh.rs`

Removed on Sean's explicit direction ("stop crediting them — there is nothing to credit").
Flagged because removing a licence attribution cannot be judged from the code alone, and
"desktop OpenClaw fork" asserts a lineage a reader could take literally. Each is a one-line
revert.

## Not done

No renames (files, modules, structs, fns, config keys, CLI verbs). `.planning/` untouched
except this lane's NOTES. `CHANGELOG.md` untouched. No PR, tag, merge to `main`, or issue
close. No full-workspace *test* run — the diff is comment-only, so that would measure other
lanes' contention rather than this change.
