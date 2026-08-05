# 23A-01 — Surface Census: routes from generated skill content to execution or model-visible context

**Base SHA:** `2ecdfdf54ff7fda920eec7d068337006e5da4ee4`
**Method:** every claim below carries a `path:line` citation read out of the tree at that SHA. Nothing here is inferred from a doc comment alone.

---

## Termination state

**GAP-CLOSED.**

Every route by which *generated skill content* reaches byte execution or a model-visible payload resolves to `GATED`. The nonce cannot get out. That half is `REFUTED-NO-GAP`.

But the census did not stop at the execution boundary, and one HIGH finding was measured at the **generation** boundary of the same governed-skills surface: the operator's written instruction not to generate skills for a project is silently discarded. That is finding **F23A-01-H1**, it was reproduced live against the shipped binary with a working control, and it is closed inside this phase. Hence GAP-CLOSED rather than REFUTED-NO-GAP.

---

## 1. The artifact under judgement

The drafter writes two files under the resolved user skills directory (`crates/wcore-skills/src/paths.rs:12`):

- `manifest.json` carrying `auto_drafted: true` and `needs_review: true`
- `SKILL.md`, whose bytes are composed by `crates/wcore-agent/src/auto_skill/drafter.rs:159` (`compose_body`)

Two properties of `compose_body` matter and are stated because later sections depend on them:

1. It emits **no YAML frontmatter at all** — the body starts at `# Auto-drafted skill: {name}` (`drafter.rs:161`). A released draft therefore declares no `hooks:`, no `mcp:`, no `paths:` and no `artifacts:` of its own.
2. It **interpolates model- and user-derived text verbatim** into the body: `t.user_input` truncated to 80 chars and `t.summary` (`drafter.rs:171-178`). This is the untrusted-content injection point, and it is exactly what the quarantine exists to contain.

The provenance classifier is `crates/wcore-skills/src/loader.rs:463` (`is_generated_draft`). Its single production call site is `loader.rs:447`, which sets `metadata.disable_model_invocation = true` at `loader.rs:448`. The classifier is manifest-first (`auto_drafted == true`) with a fallback to the exact released body shape matched by `crates/wcore-skills/src/draft.rs:40` (`is_released_generated_skill`).

---

## 2. Route table

`GATED-BY-<mechanism>` means generated-unpromoted content does not survive the route. Each row cites the line where the route is taken and the line that gates it.

| # | Route | Taken at | Disposition | Gate |
|---|---|---|---|---|
| R1 | `Skill` tool call (the model's own invocation path) | `crates/wcore-agent/src/skill_tool.rs:187` | **GATED-BY-resolve_for_model** | `crates/wcore-skills/src/refs.rs:290` returns `NotFound` when the local ref carries `disable_model_invocation`, before `resolve()` is ever called; it re-checks resolved metadata AND re-runs `is_generated_draft` for the file-backed case, and re-checks the LRU and the cross-project branch separately |
| R2 | System-prompt skill listing | `crates/wcore-agent/src/context.rs:325` | **GATED-BY-visible-filter** | the `.filter(|s| !s.disable_model_invocation)` at `crates/wcore-agent/src/context.rs:327` runs before the listing is formatted, so a quarantined draft's name and description never enter the prompt |
| R3 | Router candidate seed pool (bootstrap) | `crates/wcore-agent/src/bootstrap.rs:2026` | **GATED-BY-catalog-visible** | `catalog.visible()` (`crates/wcore-skills/src/refs.rs:128`) filters on `disable_model_invocation`; the `auto_drafter` PromptStore hydration at `bootstrap.rs:2052` consumes `candidate_names`, which is that already-filtered list — so the drafter's own PromptStore row cannot re-admit the draft |
| R4 | Per-turn router candidate pool (engine) | `crates/wcore-agent/src/engine.rs:8622` | **GATED-BY-catalog-visible** | same `catalog.visible()` iterator |
| R5 | Per-turn router hint line injected into the prompt | `crates/wcore-agent/src/engine.rs:5311` | **GATED-BY-explicit-recheck** | `catalog.find()` here is the *unrestricted* lookup, but `engine.rs:5312` immediately returns `None` on `disable_model_invocation`. This is a second, independent gate, not a reliance on R4 |
| R6 | `/skill run <name>` | `crates/wcore-agent/src/slash/skill.rs:114` | **GATED-BY-explicit-refusal** | `slash/skill.rs:115` matches `disable_model_invocation` first and answers `"this skill is quarantined and cannot be run."`. The non-quarantined arm does not execute either — it instructs the user to route through `SkillTool` — so this surface has no execution edge at all |
| R7 | `/skill list` | `crates/wcore-agent/src/slash/skill.rs:147` | **GATED-BY-design (operator-visible, model-invisible)** | iterates `catalog.refs()` (unrestricted, `refs.rs:123`) deliberately, tagging quarantined entries `(hidden)` and printing a visible/hidden summary. Name and source only; **no body, no description** |
| R8 | `/skill show <name>` | `crates/wcore-agent/src/slash/skill.rs:173` | **GATED-BY-design (metadata only)** | renders description, when_to_use, paths, source, file path and `visibility: hidden from model`. It **never renders the body**, so the `!shell:` region and the interpolated user text are not disclosed |
| R9 | Cron skill sink — pre-dispatch body scan | `crates/wcore-agent/src/cron.rs:262` and `crates/wcore-agent/src/bootstrap.rs:3260` | **GATED-BY-downstream-SkillTool** | the scan uses the *unrestricted* `catalog.resolve()` on purpose, to read the post-substitution bytes via `crates/wcore-skills/src/executor.rs:20` (`render_shell_input`) and refuse denylisted payloads. It composes but does not execute. Actual dispatch is `SkillTool::execute`, i.e. route R1, which refuses. Refusal text carries only the scan `reason`, never the body |
| R10 | Skill-declared hooks | parsed at `crates/wcore-agent/src/skill_tool.rs:436`, merged at `crates/wcore-agent/src/orchestration/mod.rs:3115` | **GATED-BY-success-precondition** (see §3 — recorded as a MEDIUM fragility) | `orchestration/mod.rs:3086` calls `maybe_merge_skill_hooks` only under `if !block_is_error(&result)`. A quarantined skill's `execute` always returns `is_error: true` via R1, so the merge is unreachable. Note `parse_skill_hooks` (`crates/wcore-skills/src/hooks.rs:30`) is a pure parse — it does not run anything |
| R11 | Skill-declared MCP servers | `crates/wcore-skills/src/mcp.rs:23` (`load_mcp_skills`) | **GATED-BY-inapplicability** | this function loads skills *from* an `McpManager`, i.e. MCP → skills. It is not a skills → MCP registration path. No production call site registers an MCP server declared by an on-disk skill body |
| R12 | Conditional path-glob activation | `crates/wcore-skills/src/conditional.rs:79` (`partition_skills`), `:107` (`activate_for_paths`) | **GATED-BY-no-production-caller** | `ConditionalSkillManager` has no production construction site; grep at the base SHA finds it only in `crates/wcore-skills/src/integration_tests.rs:13` and `crates/wcore-skills/src/conditional_supplemental_tests.rs:7`. It cannot activate a draft because nothing constructs it |
| R13 | Artifact materialisation to disk | `crates/wcore-agent/src/skill_tool.rs:260` → `crates/wcore-skills/src/artifacts.rs:33` | **GATED-BY-resolve_for_model** | `skill_tool.rs:260` is downstream of the `resolve_for_model` return at `:187`; a quarantined name never reaches it. Additionally a released draft declares no `artifacts:` (§1, point 1) |
| R14 | Shell composition for execution | `crates/wcore-skills/src/executor.rs:20` (`render_shell_input`) | **GATED-BY-resolve_for_model** | the only production caller that leads to a spawn is inside `SkillTool::execute`, downstream of `:187`. The cron pre-scan caller (R9) composes for inspection and discards |
| R15 | Session-start prioritizer reordering | `crates/wcore-agent/src/bootstrap.rs:1815` | **GATED-BY-ordering-only** | `SkillPrioritizer` consumes `iter_names()` (`crates/wcore-skills/src/refs.rs:171`), which is deliberately unrestricted so a hidden entry can be *ordered*. Reordering the catalog changes no visibility bit; every downstream consumer re-filters at R1–R5 |
| R16 | Cross-project sibling resolution | `crates/wcore-skills/src/refs.rs:290` (cross-project branch) | **GATED-BY-post-resolution-recheck** | the cross-project miss path re-checks `disable_model_invocation` on the resolved metadata before caching or returning, and the LRU hit path re-checks it too, so an unrestricted operator lookup that warmed the cache cannot become a model-facing authority |

**Sixteen routes, zero UNGATED at the execution boundary.**

---

## 3. Recorded fragilities (MEDIUM — non-blocking, logged for BACKLOG)

These are not gaps. They are gates that hold for a reason weaker than the reason a reader would assume, and the F21-02 vacuous-truth lesson says to name them.

**M1 — the hook gate is a success-precondition, not a quarantine check.** `skill_hooks_for` (`crates/wcore-agent/src/skill_tool.rs:434-449`) looks the skill up with `find_metadata_sync` (`crates/wcore-skills/src/refs.rs:177`) and authorises it with `skill_is_authorized` (`skill_tool.rs:159`), which is `permission_for(skill) == Allow` — a **permissions** check that does not consult `disable_model_invocation` at all. The route is unreachable today only because (a) `merge_skill_hooks_into` runs solely on a non-error result (`orchestration/mod.rs:3086`) and (b) `find_metadata_sync` reads the `eager` map and the LRU, and `resolve_for_model` returns `NotFound` at `refs.rs:293` *before* populating the LRU. Two accidental conditions, no explicit check. Anything that warms the LRU for a quarantined name (the cron pre-scan at `cron.rs:262` does exactly that, via the unrestricted `resolve()`) removes condition (b). Condition (a) still holds, so this is MEDIUM and not HIGH — but a future refactor that merges hooks before or independently of the tool result would silently convert it into an execution route with no test standing in the way.

**M2 — `is_generated_draft` reads its own evidence from inside the directory it is judging.** Both inputs — `manifest.json` and the `SKILL.md` body — live under `$WAYLAND_HOME/skills/<name>/`. See §4; the two measurements keep this at MEDIUM.

---

## 4. The forgery hypothesis — two independent measurements, not collapsed

The hypothesis: rewriting `SKILL.md` out of the released body shape while clearing or deleting `manifest.json` yields content the loader no longer classifies as generated, un-quarantining it.

**Measurement A — classifier behaviour. CONFIRMED.**
Read directly from `loader.rs:463-475`: `is_generated_draft` returns true only if the manifest parses AND `auto_drafted` is boolean-true, else it falls back to `draft::is_released_generated_skill`, which at `draft.rs:46-49` requires *all four* of: leaf name starts with `auto-`, body starts with the exact `# Auto-drafted skill: {leaf}\n\n` header, body contains `RELEASED_AUTO_DRAFT_NOTE` verbatim (`draft.rs:36`), and body contains "\n\nSignature: `". Breaking any one of the four, with the manifest absent or `auto_drafted` false, yields `false` → the content loads model-visible. The crate's own existing test `user_authored_auto_prefixed_skill_remains_visible` demonstrates precisely this shape loading visible — which is *correct* behaviour for a genuinely user-authored skill and is the same shape a forger would need.

So the classifier half holds. **This is by design, not by accident**: the classifier's doc comment (`loader.rs:459-462`) states that an `auto-*` name alone must never quarantine user-authored content, and the negative control in the corpus depends on exactly that discrimination.

**Measurement B — write reachability. NOT ESTABLISHED as reachable in a default configuration, and deliberately NOT asserted either way beyond what was measured.**
The precondition for A to become a defect is that the *agent's own* Write/Edit tools can write under the resolved user skills directory. `$WAYLAND_HOME/skills/` is outside the workspace root in every configuration this census exercised (the runner pins `cwd = <project>` and `WAYLAND_HOME = <home>` as distinct trees — `crates/wcore-eval-scenarios/src/runner.rs:313`). Writing there is a workspace-boundary question owned by `crates/wcore-tools/src/workspace_policy/`, which is outside this plan's declared files and outside its competence to change.

**Severity derived from the pair: MEDIUM, with a stated precondition.** A holds; B is unestablished. A defect requires both. Recorded as a residual risk whose precondition is "an actor that can already write arbitrary bytes under `$WAYLAND_HOME/skills/`" — and an actor with that capability can equally write a plain user-authored skill and skip the forgery entirely, which is why the marginal severity is MEDIUM rather than HIGH. **The two measurements are not collapsed and A alone is not reported as a defect.**

---

## 5. F23A-01-H1 (HIGH) — the project-level generation opt-out fails open

This is the census's one substantive finding, and it is at the **generation** boundary rather than the execution boundary.

### The claim

`[observability] skills_lifecycle` is the switch that governs autonomous generated-skill drafting. Its own doc comment at `crates/wcore-config/src/config.rs:666-670` says, verbatim: *"`skills_lifecycle` is the one observability switch whose explicit `false` is an authority boundary."*

In an **untrusted** workspace — the default state of any freshly created or freshly cloned project — an explicit project-level `skills_lifecycle = false` is **silently discarded and re-defaults to `true`**.

### Root cause, cited

`resolve_config_files` calls `merge_config_files_with_trust(global, project, workspace_trust.is_trusted())` at `crates/wcore-config/src/config.rs:3284`. When the workspace is untrusted, the project config is first passed through `restrict_untrusted_project_config` (`config.rs:4351`), which constructs a fresh `ConfigFile::default()` at `config.rs:4352` and copies forward only an allowlist of power-reducing settings: `max_tokens`, `max_turns`, `approval_mode`, `system_prompt`, `user`, `read_only`, `tools.allow_list`, `tools.skills.deny`, `tools.verify_edits`, `security.enabled`, `anvil.enabled` (`config.rs:4356-4373`).

`observability.skills_lifecycle` is **not** in that allowlist. `ConfigFile::default()` yields `ObservabilityFileConfig::default()` → `skills_lifecycle: None` → `resolved_skills_lifecycle()` returns `true` (`config.rs:695-697`). The AND-merge at `config.rs:4171-4173` then computes `true && global`, so the project's explicit `false` has no effect whatsoever.

### Live measurement, with a control that fires

Driven against the real `wayland-core` binary built from this base SHA on `hetzner-dsm`, reading the `capability_activation` events off the `--json-stream` protocol surface. Full transcript in `23A-01-LIVE-EVIDENCE.md`.

| global `skills_lifecycle` | project `skills_lifecycle` | workspace trust | expected | **observed** |
|---|---|---|---|---|
| `true` | `false` | untrusted (default) | unavailable | **ready** ❌ |
| `false` | `true` | untrusted | unavailable | unavailable ✓ |
| `true` | `true` | untrusted | ready | ready ✓ |
| `false` | `false` | untrusted | unavailable | unavailable ✓ |
| *(absent)* | `false` | untrusted | unavailable | **ready** ❌ |
| `false` | *(absent)* | untrusted | unavailable | unavailable ✓ |
| *(absent)* | `false` | **trusted** (`--trust-workspace`) | unavailable | unavailable ✓ |

The last two rows are the control pair: the *same* project file, the *same* binary, differing only in workspace trust, produces `ready` vs `unavailable`. That isolates the cause to the trust-restriction path and rules out "the project file is not read at all" — which was independently disproved by writing malformed TOML into `.wayland-core.toml` and observing the binary fail with `failed to parse .wayland-core.toml: TOML parse error at line 1, column 6`.

Both project layout forms were tested (`.wayland-core.toml` and `.wayland-core/config.toml`); both are ignored identically, so this is not a layout-selection artifact (`config.rs:3119` `project_config_selection`).

### Impact

1. An operator who writes `skills_lifecycle = false` into a project's config to stop the agent learning from that project's traffic **still gets skills auto-drafted from it**, and they are written into the *global* `$WAYLAND_HOME/skills/` — so the leak crosses into every other project on the machine.
2. `want_memory = self.config.memory.enabled || skills_lifecycle_enabled` (`crates/wcore-agent/src/bootstrap.rs:1548`). The failed opt-out therefore also force-constructs Memory in a project the operator asked to keep out of the learn loop.
3. It fails **open on a restricting setting**, which inverts the stated contract of the very function that drops it: *"Untrusted repositories retain useful prompt/resource-tightening settings, while every executable or authority-expanding surface is made inert"* (`config.rs:3895-3897`).

### Pre-existing red this explains

`packaged_lifecycle_memory_matrix_has_real_effects_and_quarantine` in `crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs` is **already failing on the plan branch** at cell `global=true, project=false, memory=false`, with `CapabilityHonesty: ProcedureSkillDrafting advertised ready before required unavailability`. Measured at the base SHA before this phase changed anything. **The test is correct and the product is wrong.** No assertion in it was weakened; the product was fixed.

### Severity: HIGH, not CRITICAL

It does not produce code execution — quarantine (§2) holds regardless, so a draft generated against the operator's wishes still cannot run. The harm is a silently-inert consent control plus cross-project data movement. That is HIGH.

### Disposition: FIXED (not escalated) — cross-audited

The fix lands in `crates/wcore-config/src/config.rs`, which 23A-01's own termination criterion names as an ESCALATE example. That conflict was put to the four-way panel; the decision and the dissent are recorded in `23A-01-SUMMARY.md`.

---

## 6. Sources consulted with no route found

Recorded so a later reader knows the absence was checked rather than overlooked: `crates/wcore-skills/src/watcher.rs`, `crates/wcore-skills/src/curate.rs`, `crates/wcore-skills/src/router.rs`, `crates/wcore-skills/src/prompt.rs`, `crates/wcore-skills/src/audit.rs`. None of these reaches a spawn or an outbound provider body with generated body bytes; the first four consume names and metadata already filtered upstream, and `audit.rs` is an operator-facing report.

## 7. Known unknowns — recorded, not resolved here

- Whether a skill-declared MCP server or hook can be registered from a quarantined draft on a path this census did not reach. R10 and R11 are gated on the paths that exist at this SHA; both are shape-fragile (M1).
- Whether cross-project resolution can surface a sibling project's generated draft under a configuration not exercised here. R16's re-checks are correct on the code path read, but the sibling-discovery configuration was not driven live.
- Whether macOS behaves identically. Not measured in 23A-01; 23A-04 owns the macOS disposition.
- Whether the *other* settings absent from the `restrict_untrusted_project_config` allowlist include further restricting values that fail open the same way. F23A-01-H1 was found by following one setting; the class was not swept. Logged as a follow-up, deliberately not expanded into this plan.
