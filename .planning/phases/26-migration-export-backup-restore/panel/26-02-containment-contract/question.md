# Decision — is the F26-02 containment contract one an operator can live with at real 540-item scale?

Wayland Core imports agent state from two peers (Hermes, OpenClaw). Imported
executable content — peer skill bodies carrying shell directives, peer MCP
definitions carrying launch commands, peer hook commands — must be INERT on
arrival and promotable only by an explicit operator action that the imported
content itself cannot perform. This is the same contract GHSA-8r7g established
in this codebase for project-defined hooks.

Judge whether the contract as built is acceptable as it stands, or must be
amended, and in which direction.

Answer with exactly one line `PANEL-VERDICT: <option-id>` and one line
`PANEL-BASIS: <one sentence>`, then up to 250 words.

## Options (rotated order; ids are fixed)

### `contract-amend-classification`
- **Name:** Classification is over-broad or under-broad; amend the breadth and re-measure
- **Pros:** Over-broad classification trains operators to promote without reading and hollows the contract out; under-broad classification leaves a child-process launch surface uncontained, which is the critical threat this plan exists to close. Either is a real defect with a bounded fix.
- **Cons:** Changing breadth changes what the earlier tasks' tests assert, so the classification suite has to be re-run rather than assumed.

### `contract-reject`
- **Name:** A load-bearing check failed — the counts do not balance, the positive control did not fire, or a ceiling was raised
- **Pros:** Names the failure honestly instead of absorbing it. Each of these three means a headline claim of this plan is not true as stated.
- **Cons:** It is not an outcome that can be recorded and passed: each is a defect this plan must FIX and re-measure before it can close, so choosing it is choosing more work now.

### `contract-accept`
- **Name:** The containment contract stands as built
- **Pros:** The counts balance, classification is bounded correctly in both directions, the positive control fired, the ceilings were not touched, and the cost of promoting a realistic subset does not grow with the size of that subset.
- **Cons:** Nothing is amended, so any ergonomic roughness that the measurement did not capture ships as-is.

### `contract-amend-ergonomics`
- **Name:** Containment is correct but promotion at real scale is not usable; amend the promotion path and record the amendment
- **Pros:** Fixes the failure mode that actually destroys containment in the field — an operator who has to promote items one at a time will route around the contract entirely. Amends the promotion path, which is where the defect is.
- **Cons:** Costs work inside this plan's declared files, and a promotion path made more convenient is a promotion path that must be re-checked against the rule that imported content cannot promote itself.

## The six things being judged

1. The four counts balance arithmetically over the full corpus.
2. Promotion at 540 items is a path an operator would actually use rather than one they would bypass.
3. Classification is not over-broad — personas, memory notes, settings and assets import without ceremony.
4. Classification is not under-broad — peer MCP launch commands and hook commands are contained, because they are child processes and not settings.
5. The inertness proof's positive control demonstrably fired, since a negative leg without one proves nothing.
6. The existing workspace-trust ceilings were NOT raised to admit a realistic import.

## The measurement (`promotion-scale.txt`, printed by the script, not typed)

```
SCALE-DISCOVERED: 554
SCALE-IMPORTED: 13
SCALE-QUARANTINED: 541
SCALE-EXCLUDED: 0
SCALE-BALANCES: yes
PROMOTE-COST: items=1 invocations=1
PROMOTE-COST: items=256 invocations=1
PROMOTE-SCALING-RULE: bounded when larger_invocations <= 2 * smaller_invocations
PROMOTE-SCALING: bounded
CLASSIFY-DATA-QUARANTINED: 0
CLASSIFY-EXEC-UNCONTAINED: 0
CEILING-REFUSES-REALISTIC: yes
CEILING-CONSTANTS: files=512 file_bytes=4194304 total_bytes=33554432
POSITIVE-CONTROL: fired
SCALE-CORPUS-MATERIALISED-SKILLS: 540
SCALE-STORE-ADMITTED: 512
```

Reading of those lines:

- 554 items discovered; 13 imported; 541 quarantined; 0 excluded; the last
  three sum to the first.
- `SCALE-CORPUS-MATERIALISED-SKILLS: 540` — the committed canary corpus
  reproduces the real install's 540 skill DIRECTORIES, but its bounded
  generator left them as markers with no `SKILL.md`. The script materialised a
  realistic committed fixture body into those already-present directories, in a
  scratch copy, so the scale is the real install's structure carrying a real
  payload shape. Stated explicitly so the number is never mistaken for one the
  corpus shipped.
- `SCALE-STORE-ADMITTED: 512` against `SCALE-QUARANTINED: 541` — the existing
  `MAX_EXECUTABLE_FILES = 512` ceiling REFUSED 29 of the 541. Each refusal is
  named individually in the apply report and still appears in the arithmetic;
  none is silently dropped and none is imported live. The ceiling was NOT
  raised.
- `PROMOTE-COST` was measured at two different subset sizes, 1 and 256, each in
  a fresh home. An earlier run of this same script measured `items=256
  invocations=256` and `PROMOTE-SCALING: linear`; the cause was reproduced
  directly (256 quarantined items share only 46 distinct directory names,
  because a real install reuses one skill name across profiles, and the
  promotion aborted the whole set on the first collision). That was fixed
  inside the plan's own files and re-measured; the line above is the re-measurement.

## The apply report at that scale (tail, verbatim)

```
  • skill:profiles/fred/skills/ijfw-memory-audit — refused: imported executable surface exceeds the quarantine limits (max 512 files, 33554432 bytes total)
  • skill:profiles/fred/skills/ijfw-plan-check — refused: imported executable surface exceeds the quarantine limits (max 512 files, 33554432 bytes total)
  • skill:profiles/fred/skills/ijfw-preflight — refused: imported executable surface exceeds the quarantine limits (max 512 files, 33554432 bytes total)
  • skill:profiles/fred/skills/ijfw-recall — refused: imported executable surface exceeds the quarantine limits (max 512 files, 33554432 bytes total)
  • skill:profiles/fred/skills/ijfw-review — refused: imported executable surface exceeds the quarantine limits (max 512 files, 33554432 bytes total)
  • skill:profiles/fred/skills/ijfw-status — refused: imported executable surface exceeds the quarantine limits (max 512 files, 33554432 bytes total)
  … (23 further refusals, each named identically) …
  Review with: wayland-core migrate quarantined
=== quarantined head ===
Quarantined imports (512):
  • mcp_server:ijfw-memory
      reason: mcp definition carries a launch command
      from:   mcp_servers/ijfw-memory (hermes)
      digest: f15cea594d800dcf8c321e4702ba943f5c77b33b8bc609a25ffc7932fc0c5add
  • skill:profiles/flux-backend-eng/skills/apple
      reason: skill body carries a shell directive
      from:   profiles/flux-backend-eng/skills/apple (hermes)
      digest: 021369888a005c5da9bfea065a5acc2d3f21973750cbee8ac379f937bf2de59e
  • skill:profiles/flux-backend-eng/skills/autonomous-ai-agents
      reason: skill body carries a shell directive
      from:   profiles/flux-backend-eng/skills/autonomous-ai-agents (hermes)
```

## How classification is decided (source)

```rust
/// Classify an imported skill body.
///
/// Delegates to `wcore_skills::shell::contains_shell_commands` — the SAME
/// predicate the executor and the permission checker use, including its
/// exemption for MCP-loaded content (whose directives the executor returns
/// unchanged rather than running).
pub fn classify_skill_body(content: &str, loaded_from: LoadedFrom) -> Classification {
    if contains_shell_commands(content, loaded_from) {
        Classification::Executable(ExecutableReason::SkillShellDirective)
    } else {
        Classification::Data
    }
}

/// Classify an imported peer MCP definition.
///
/// A definition carrying a launch command is executable regardless of the
/// declared transport: `command` is what gets spawned, and a transport field is
/// peer-controlled data that must not be able to talk the classifier out of a
/// containment decision.
pub fn classify_mcp_server(server: &McpServerConfig) -> Classification {
    match server.command.as_deref() {
        Some(cmd) if !cmd.trim().is_empty() => {
            Classification::Executable(ExecutableReason::McpLaunchCommand)
        }
        _ => Classification::Data,
    }
}

/// Classify an imported hook definition by its command.
pub fn classify_hook_command(command: &str) -> Classification {
    if command.trim().is_empty() {
        Classification::Data
    } else {
        Classification::Executable(ExecutableReason::HookCommand)
    }
}

```

## How promotion is decided (source) — the no-self-trust half

```rust
    /// Promote the named identities into `dest_root`, one directory each.
    ///
    /// # The no-self-trust half of the contract
    ///
    /// `ids` comes from the CALLER — in production, from the operator's
    /// `migrate promote --id …` command line. This function reads nothing out
    /// of the quarantined payload: not its frontmatter, not a marker file, not
    /// a manifest, not its filename. The only other thing it consults is the
    /// store's own index, which the store wrote in [`Self::admit`]. So there is
    /// no field an imported artifact can carry that reaches this decision,
    /// which is the property GHSA-8r7g requires and the property a `trusted:
    /// true` frontmatter key would otherwise defeat.
    ///
    /// Promotion of a whole set costs ONE invocation, so promoting a realistic
    /// subset does not cost one operator action per item.
    ///
    /// # Why a name collision must not abort the set
    ///
    /// A real peer install carries the SAME skill name under many profiles —
    /// measured on 26-01's structural corpus, 256 quarantined items shared just
    /// 46 distinct directory names. Aborting the whole promotion on the first
    /// collision forces the operator to promote one item at a time, which is
    /// precisely the cost that makes an operator route around containment
    /// altogether. So a collision is RESOLVED, not fatal: the item is promoted
    /// under a name disambiguated by a digest of its identity, and the mapping
    /// is returned so the caller can report it. Nothing is silently overwritten
    /// and nothing is silently dropped.
    pub fn promote(
        &self,
        ids: &[String],
        dest_root: &Path,
    ) -> Result<Vec<PromotedItem>, QuarantineError> {
        let mut index = self.load_index()?;
        // Validate every identity BEFORE moving anything, so a typo in a set
        // cannot leave half a promotion applied.
        for id in ids {
            if !index.entries.contains_key(id) {
                return Err(QuarantineError::UnknownIdentity(id.clone()));
            }
        }
        fs::create_dir_all(dest_root)?;
        let mut promoted = Vec::new();
        let mut taken: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for id in ids {
            let entry = index.entries.get(id).expect("validated above").clone();
            let from = self.root.join(&entry.stored_path);

            // Resolve a unique on-disk name. The plain name is preferred; on a
            // collision the identity's digest disambiguates, so two items never
            // land on one directory and neither is lost.
            let mut name = entry.promote_as.clone();
            let mut renamed = false;
            if name.is_empty() || taken.contains(&name) || dest_root.join(&name).exists() {
                let digest = item_digest("id", entry.id.as_bytes());
                let base = if name.is_empty() { "item" } else { &name };
                name = format!("{}-{}", base, &digest[..12]);
                renamed = true;
            }
            if taken.contains(&name) || dest_root.join(&name).exists() {
                // Two DISTINCT identities cannot produce one digest, so reaching
                // here means the disambiguated name was already on disk — a real
                // conflict the operator must resolve rather than one to paper over.
                return Err(QuarantineError::PromotionTargetExists(
                    dest_root.join(&name),
                ));
            }

            copy_tree(&from, &dest_root.join(&name))?;
            fs::remove_dir_all(&from).ok();
            index.entries.remove(id);
            taken.insert(name.clone());
            promoted.push(PromotedItem {
                id: entry.id,
                promoted_as: name,
                renamed,
            });
        }

```

The fixture corpus deliberately includes a payload whose frontmatter asserts
`trusted: true`, `auto_promote: true`, `promoted: true`, `wayland_quarantine:
exempt` and `quarantine: false`, PLUS a sibling `PROMOTE` marker file and a
`manifest.json` claiming `"promoted": true`. It remains contained; the
promotion path reads none of them.

## The GHSA-8r7g contract this mirrors (`crates/wcore-config/src/hooks.rs`)

```rust
    pub dispatch_enabled: bool,
    /// GHSA-8r7g — operator opt-in to run hooks defined in a PROJECT config
    /// (`.wayland-core.toml` in the working directory). A `HookDef.command` is
    /// executed as a child process, so a project config that travels with a
    /// cloned repo is an arbitrary-code-execution surface. Default `false`:
    /// project-defined `pre_tool_use` / `post_tool_use` / `stop` hooks are NOT
    /// run. Only the operator's GLOBAL config value is honored (a project
    /// cannot set this to trust its own hooks — see `merge_config_files`). Set
    /// `true` in the global config to run project hooks, accepting that any
    /// repo you open can then execute its configured hooks.
    #[serde(default)]
    pub trust_project_hooks: bool,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            pre_tool_use: Vec::new(),
            post_tool_use: Vec::new(),
            stop: Vec::new(),
            dispatch_enabled: default_dispatch_enabled(),
            trust_project_hooks: false,
        }
    }
}

fn default_dispatch_enabled() -> bool {
    true
```

## The inertness proof

Two paired legs run against the REAL built binary on Linux, driven through a
real agent turn with a scripted mock provider:

- NEGATIVE: after importing the corpus, the driven turn leaves the sentinel
  ABSENT. It additionally asserts the import discovered a non-zero count, the
  payload is reported quarantined, and the stream shows the Skill tool ran and
  reported the skill unavailable — so a crashed engine cannot read as
  containment.
- POSITIVE CONTROL, SAME payload, SAME turn, differing only by an explicit
  operator promotion: the sentinel is PRESENT.

Measured asymmetry: the positive leg returned in 2.4s (the moment the sentinel
appeared); the negative leg exhausted its full 45s window with the sentinel
absent. `POSITIVE-CONTROL: fired` above was produced by RE-RUNNING both legs at
this commit, not by transcribing this paragraph.

An earlier revision of these legs failed for a reason worth stating: the engine
refused the turn ("no encrypted credentials vault is unlocked") and BOTH legs
were measuring a dead engine. The turn-ran assertion is what surfaced it.

## Constraints on your answer

Raising `MAX_EXECUTABLE_FILES` to admit the 540-directory import is expressly
forbidden — it would widen the executable-surface bound for every
workspace-trust consumer to solve a migration problem. A ceiling that refuses a
realistic import is a finding with a severity, not a limit to widen.
