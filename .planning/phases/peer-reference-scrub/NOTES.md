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
