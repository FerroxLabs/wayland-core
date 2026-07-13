# Wayland Core F06 Skill-Lifecycle Containment Contract

**Status:** approved design contract; implementation not yet started

**Baseline:** `a9202f5` on `frontier/m0`, production source unchanged from Wayland Core `0.12.25`

**Scope:** emergency containment only. F06 makes the existing lifecycle switch authoritative and generated skills inert. It does not build the final autonomous-skill governance system; that remains F23.

## 1. Decision

F06 is broader than the original four-file patch because the authority split crosses configuration, bootstrap, engine observation, persistence, catalog loading, model resolution, routing, slash guidance, and CLI promotion.

The bounded design is:

- keep the product default enabled;
- make any explicit global or project `false` dominate true or absence;
- retain ordinary memory when lifecycle is disabled;
- prevent both drafting paths from changing generated-skill state when disabled;
- when enabled, keep generated artifacts inspectable but non-model-visible and non-executable;
- remove the legacy drafter's process-global registration and redundant secondary write;
- temporarily reject promotion until F23 can perform an identity-bound, audited promotion transaction.

This uses provenance-based quarantine in the existing canonical draft location. A new quarantine storage service would create F23 architecture inside an emergency patch and is rejected for F06.

## 2. Confirmed 0.12.25 failure chain

1. Memory and skill lifecycle both resolve enabled by default.
2. Global and project lifecycle values are ORed after absence has already become true, so a single explicit opt-out is lost.
3. Bootstrap opens memory when memory or lifecycle is enabled, then constructs the legacy `SkillDrafter` whenever memory exists.
4. The engine calls legacy observation on natural and terminated exits without checking lifecycle.
5. Legacy drafting writes a loader-visible draft, a manifest-less secondary draft, a PromptStore candidate, and a model-invocable process-global bundled definition.
6. Loader quarantine fails open when the manifest is absent, unreadable, malformed, or no longer says `needs_review=true`.
7. Hidden metadata affects advertisement only. `SkillTool` resolves a guessed hidden name and includes hidden names in error hints.
8. Router candidates and auto-drafter PromptStore hydration can include hidden generated skills.
9. `--skills-promote` changes one procedure row, then ignores that identity and bulk-copies every legacy draft missing from the loader location without a manifest.

Exact source evidence is frozen in `2026-07-13-wayland-core-f00-characterization.md`.

## 3. Authority and security invariants

1. The resolved lifecycle boolean is the only F06 authority for generated-skill mutation. `memory.enabled` cannot widen it.
2. An explicit `false` in either global or project configuration is monotonic. A lower-trust source cannot re-enable a managed/global disable, and a project can opt out of a default-enabled global source.
3. With lifecycle disabled, neither drafting path may mutate draft buffers, P4 procedures, disk, PromptStore, bundled state, generated router state, or draft telemetry.
4. Construction and execution are both gated: bootstrap omits the legacy drafter and the engine returns before legacy bucketing.
5. Generated provenance, not mutable review status, determines quarantine during F06.
6. A quarantined skill cannot appear in model prompts, router choices, model-facing diagnostics, or execute through a model-facing skill surface, including a guessed name.
7. Auto-generated content never enters the process-global bundled registry.
8. Intentional built-in, plugin, MCP, project, and user-authored skills retain existing registration, visibility, and precedence.
9. No F06 path depends on Unix-only separators, shells, or commands.

## 4. Configuration contract

The file layer must preserve source presence:

```rust
struct ObservabilityFileConfig {
    skills_lifecycle: Option<bool>,
    // existing file-layer observability fields
}
```

The resolved runtime config remains a plain `bool`. Its value is:

```text
effective = global.unwrap_or(true) && project.unwrap_or(true)
```

The default therefore remains on, while either source can safely disable it. Other observability fields retain their current merge behavior.

Required matrix:

| Global | Project | Memory | Effective lifecycle | Generated mutations |
|---:|---:|---:|---:|---|
| false | false | false | false | none |
| false | false | true | false | none |
| false | true | false | false | none |
| false | true | true | false | none |
| true | false | false | false | none |
| true | false | true | false | none |
| true | true | false | true | quarantined only |
| true | true | true | true | quarantined only |

Also prove absent/absent resolves true, global false/project absent resolves false, and global absent/project false resolves false.

## 5. Runtime containment contract

### Bootstrap and engine

- Real memory remains available whenever memory is enabled.
- The legacy `SkillDrafter` is constructed only when lifecycle is enabled and real memory is available.
- `observe_auto_skill` checks lifecycle before creating trajectories or mutating the bucketer.
- The newer P4 writer retains its pre-mutation lifecycle guard.
- PromptStore auto-drafter seeds are hydrated only when lifecycle is enabled.
- Router candidates are drawn only from model-visible catalog entries.

### Legacy draft persistence

- Write one canonical draft directory only.
- Atomically publish durable generated provenance before publishing `SKILL.md`.
- Remove the legacy secondary `WAYLAND_HOME/skills/auto` write.
- Remove auto-draft calls to `register_bundled_skill`.
- PromptStore recording may remain only on the lifecycle-enabled path; its candidate must not bypass catalog visibility.

### Quarantine and invocation

- `manifest.auto_drafted == true` is quarantined regardless of `needs_review`.
- Released manifest-less drafts are quarantined only when both the exact released generator marker and generated naming shape match.
- An ordinary user-authored skill named `auto-*` remains ordinary unless it carries generated provenance.
- Add a model-specific catalog resolver that rejects hidden local and cross-project entries with the same not-found behavior.
- `SkillTool` uses that resolver and visible-only names.
- Operator inspection may continue to list/show hidden drafts, but `/skill run` must not tell the user to ask the model to execute one.

### Promotion

- `--skills-promote` exits nonzero before database or filesystem mutation with a stable message that governed promotion is temporarily unavailable.
- Remove the bulk legacy migration helper.
- Archive, list, show, and audit remain available.
- F23 restores promotion only with artifact identity, provenance, evaluation, approval identity, audit, activation, and rollback in one transaction.

## 6. Files and ownership

| File | F06 contract |
|---|---|
| `crates/wcore-config/src/config.rs` | Preserve file-layer presence and implement false-dominant resolution. |
| `crates/wcore-agent/src/bootstrap.rs` | Gate legacy construction/hydration and use visible router candidates. |
| `crates/wcore-agent/src/engine.rs` | Add the first-instruction legacy observation guard. |
| `crates/wcore-agent/src/auto_skill/drafter.rs` | One canonical atomic draft; no secondary write or global registration. |
| `crates/wcore-skills/src/draft.rs` | Own stable generated provenance/legacy-marker classification shared by writer and loader. |
| `crates/wcore-skills/src/loader.rs` | Quarantine generated provenance independently of review state. |
| `crates/wcore-skills/src/refs.rs` | Enforce hidden status in model-specific local and cross-project resolution. |
| `crates/wcore-agent/src/skill_tool.rs` | Use model-safe resolution and visible-only diagnostics. |
| `crates/wcore-agent/src/slash/skill.rs` | Keep inspection; refuse model-run guidance for quarantined skills. |
| `crates/wcore-cli/src/main.rs` | Suspend promotion before mutation and remove bulk migration. |
| `docs/advanced.md` | Align default, precedence, quarantine, and promotion documentation. |

The bundled registry API and intentional plugin/built-in registration call sites are out of scope and must not change.

## 7. Red tests required before production edits

1. All configuration matrix rows plus absent-source cases.
2. Lifecycle false plus memory true retains real memory but installs no legacy drafter.
3. A manually injected drafter cannot mutate its bucketer, disk, PromptStore, P4, trace stream, or bundled registry on either natural or terminated exit when lifecycle is false.
4. Lifecycle true writes exactly one legacy draft location, emits provenance before content, may record PromptStore, and registers no bundled definition.
5. The newer writer creates no procedure row when disabled and only `Staged` when enabled.
6. Generated drafts stay hidden for `needs_review=true`, `needs_review=false`, malformed manifest with released marker, and released manifest-less content.
7. A guessed hidden local or cross-project name cannot execute or appear in `SkillTool` errors.
8. A concurrent second runtime/catalog cannot observe a generated bundled definition.
9. An intentional bundled/plugin sentinel and a user-authored `auto-*` skill remain usable.
10. Promotion fails before mutation/copy while archive still works.
11. Every filesystem test uses platform-native temporary paths.

Each red test must fail for the intended 0.12.25 reason before the production patch is written. A test that fails because its fixture is invalid does not count.

## 8. Migration and compatibility

- Existing TOML syntax remains valid; serialized lifecycle values remain booleans.
- Missing lifecycle fields remain default-on. Explicit false becomes effective.
- Existing generated disk and database records are not deleted or rewritten.
- Existing `auto_drafted=true` artifacts remain quarantined regardless of review flag.
- Exact released manifest-less generated artifacts become quarantined; ordinary user skills do not.
- Existing generated definitions already resident in a process disappear after restart. F06 does not attempt unsafe selective removal from an unprovenanced global registry.
- Promotion is the only intentional temporary compatibility break, because its current behavior cannot safely bind the requested approval to one artifact.

## 9. Verification gates

Minimum F06 proof:

- red-to-green targeted tests for every invariant above;
- `cargo fmt --all --check`;
- targeted crate tests on pinned Rust 1.95.0;
- full workspace `cargo nextest run` on Hetzner;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` or the repository's stricter equivalent;
- release build and strict provenance test from the exact candidate commit;
- packaged CLI negative proof for promotion and lifecycle-off behavior;
- cross-audit by an independent reviewer, Sol, and Grok, with every accepted/rejected finding recorded;
- no changes to Desktop protocol schemas in F06.

Mac and Windows source branches must compile in CI; M0 closure still requires the broader three-OS packaged evidence supplied by F01-F05.

## 10. Rollback

Revert the F06 patch and restart Core processes. F06 performs no destructive database or filesystem migration, so data rollback is not required.

If legacy classification produces a false positive, narrow or revert only that classifier while retaining lifecycle gating, removal of global auto-registration, and the model invocation guard. Reverting those containment controls reopens the security defect and requires release-owner approval.

## 11. Deferred work

F09 owns runtime/session-scoped replacement of process-global mutable registries and policy state.

F23 owns the final detect -> draft -> quarantine -> evaluate -> approve -> promote -> observe -> revoke lifecycle, including signatures/digests, capability permissions, evaluation thresholds, reviewer identity, versioning, rollback, retention, protocol events, TUI, and Desktop UI.

F06 does not redesign PromptStore, curator/GEPA scoring, general skill precedence, memory defaults, provider behavior, or the global bundled registry.

## 12. Implementation order and stop conditions

1. Commit red reproductions only.
2. Review that every failure maps to this contract.
3. Implement configuration authority.
4. Implement bootstrap and engine zero-mutation gating.
5. Implement persistence, quarantine, model-resolution, router, slash, and promotion containment.
6. Run targeted proof, then full proof.
7. Cross-audit and integrate only verified findings.

Stop and amend this contract before proceeding if:

- a model-driven execution surface bypasses `SkillTool` or the model-specific resolver;
- legacy generator versions cannot be classified without hiding plausible user-authored content;
- disabling lifecycle necessarily disables ordinary memory;
- safe promotion can be preserved only by implementing F23's governance transaction;
- a required change alters the Desktop/Core wire protocol.
