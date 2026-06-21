use std::collections::HashMap;
use std::path::Path;

use wcore_memory::prompt::build_memory_prompt_minimal;
use wcore_plugin_api::{RuleScope, RuleSpec};
use wcore_skills::prompt::format_skills_within_budget;
use wcore_skills::refs::SkillRef;

use crate::agents_md;
use crate::plan::prompt as plan_prompt;

/// Output-side token optimization (Part B): a single, fixed terseness
/// directive injected into the cached system prefix.
///
/// It MUST be byte-identical on every turn — it lives ahead of the volatile
/// message tail in the cached prefix, so any variation would bust the prompt
/// cache. Keeping it a `const` (not a `format!`) guarantees that. Injected
/// only when the route optimizes client-side
/// (`compat.input_optimization() == "client"`); router-optimized routes pass
/// `terse_enabled = false` and the section is omitted entirely.
pub const TERSENESS_DIRECTIVE: &str = "Be concise. Lead with the answer or the \
    action; skip restating the question and ceremonial preamble/closing.";

/// Session-scoped cache for system prompt sections.
///
/// Each section (intro, tool guidance, AGENTS.md, memory, skills) is cached
/// independently. The `joined` field holds the pre-joined full prompt string
/// and is invalidated whenever any section changes.
pub struct SystemPromptCache {
    /// Cached section strings, keyed by section name.
    pub(crate) sections: HashMap<&'static str, String>,
    /// Pre-joined full prompt. Invalidated on any section change.
    pub(crate) joined: Option<String>,
    /// Track last plan_mode_active value to detect changes.
    pub(crate) last_plan_mode: bool,
    /// Track last toon_enabled value to detect changes.
    pub(crate) last_toon_enabled: bool,
    /// Track last terse_enabled value to detect changes (Part B route gate).
    pub(crate) last_terse: bool,
}

impl SystemPromptCache {
    pub fn new() -> Self {
        Self {
            sections: HashMap::new(),
            joined: None,
            last_plan_mode: false,
            last_toon_enabled: false,
            last_terse: false,
        }
    }

    /// Invalidate a specific section by name.
    pub fn invalidate(&mut self, section: &str) {
        self.sections.remove(section);
        self.joined = None;
    }

    /// Invalidate all cached sections (e.g., on /compact).
    pub fn invalidate_all(&mut self) {
        self.sections.clear();
        self.joined = None;
    }
}

impl Default for SystemPromptCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Return the tool-usage guidance section for the system prompt.
///
/// This section teaches the model when to prefer dedicated tools over Bash,
/// how to handle parallel vs sequential calls, and cross-tool best practices.
/// Intentionally redundant with individual tool descriptions — the dual
/// placement ensures the model follows the rules regardless of attention span.
fn tool_usage_guidance() -> &'static str {
    "\
# Using your tools
 - Do NOT use Bash when a dedicated tool is available. Using dedicated tools \
allows the user to better understand and review your work:
   - File search: Glob (not find or ls)
   - Content search: Grep (not grep or rg)
   - Read files: Read (not cat, head, or tail)
   - Edit files: Edit (not sed or awk)
   - Write files: Write (not echo redirection or cat with heredoc)
 - You can call multiple tools in a single response. If there are no \
dependencies between them, make all independent calls in parallel. \
However, if one call depends on a previous result, run them sequentially.
 - Prefer Edit over Write for modifying existing files — Edit sends only \
the diff, which is easier to review.
 - Always Read a file before editing it.
 - Some tools are deferred — only their names are visible. Before calling \
a deferred tool, use ToolSearch to load its full schema first."
}

/// Render plugin-contributed rules into a single system-prompt section.
///
/// `RuleScope::Universal` rules are always included. `RuleScope::ProjectScoped`
/// rules are included only when `cwd` lies inside a project — detected, like
/// the rest of the engine (`AGENTS.md` collection, skill discovery), by walking
/// up to a `.git` directory via [`wcore_skills::paths::find_git_root`]. When
/// `cwd` is not inside a project, project-scoped rules are dropped.
///
/// Returns an empty string when no rule applies, so the caller can skip the
/// section entirely.
fn format_plugin_rules(rules: &[RuleSpec], cwd: &str) -> String {
    if rules.is_empty() {
        return String::new();
    }

    let in_project = wcore_skills::paths::find_git_root(Path::new(cwd)).is_some();

    let applied: Vec<&str> = rules
        .iter()
        .filter(|r| match r.scope {
            RuleScope::Universal => true,
            RuleScope::ProjectScoped => in_project,
        })
        .map(|r| r.content.trim())
        .filter(|c| !c.is_empty())
        .collect();

    if applied.is_empty() {
        return String::new();
    }

    // Rule content is contributed by installed plugins (register_rules). Defang
    // any forged host trust tags so a rule cannot impersonate or break out of
    // the host framing, and label provenance so plugin rules are
    // distinguishable from core host instructions.
    let joined = applied
        .iter()
        .map(|c| wcore_config::hooks::neutralize_trust_delimiters(c))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("The following operating rules were contributed by installed plugins:\n\n{joined}")
}

/// Build the system prompt from config and environment.
///
/// Sections are assembled in this order:
/// 1. Base intro (role, model identity, working directory, date)
/// 2. Tool usage guidance (dedicated tools, parallel calls, etc.)
///    2b. Terseness directive (output-side opt — only when `terse_enabled`)
/// 3. Custom prompt (user config)
/// 4. AGENTS.md (project instructions)
/// 5. Memory system prompt (behavioral instructions + MEMORY.md content)
/// 6. Plan mode instructions (when active)
/// 7. Skills reminder (available skills listing)
/// 8. Plugin rules (universal + project-scoped, gated on cwd)
///
/// `terse_enabled` gates the static [`TERSENESS_DIRECTIVE`] section. The caller
/// passes the route-optimization flag here (`true` when
/// `compat.input_optimization() == "client"`); the directive is byte-identical
/// every turn so it stays inside the cached prefix without busting the cache.
///
/// Session-permanent sections (intro, tool guidance, custom prompt, AGENTS.md,
/// plugin rules) are cached in `cache.sections` and reused across calls. The
/// `joined` field caches the final concatenated result; it is returned on
/// subsequent calls unless plan_mode_active has changed.
#[allow(clippy::too_many_arguments)]
pub fn build_system_prompt(
    cache: &mut SystemPromptCache,
    custom_prompt: Option<&str>,
    cwd: &str,
    model: &str,
    skills: &[SkillRef],
    context_window_tokens: Option<usize>,
    memory_dir: Option<&Path>,
    plan_mode_active: bool,
    toon_enabled: bool,
    plugin_rules: &[RuleSpec],
    terse_enabled: bool,
) -> String {
    // Fast path: return cached joined result if nothing changed
    if let Some(ref joined) = cache.joined
        && cache.last_plan_mode == plan_mode_active
        && cache.last_toon_enabled == toon_enabled
        && cache.last_terse == terse_enabled
    {
        return joined.clone();
    }

    let mut parts = Vec::new();

    // Section: intro (session permanent)
    let intro = cache.sections.entry("intro").or_insert_with(|| {
        let today = chrono::Local::now().format("%Y-%m-%d");
        format!(
            "You are an AI assistant that can use tools to help with tasks.\n\
             You are powered by the model {model}.\n\
             Working directory: {cwd}\n\
             Current date: {today}\n\
             When constructing time-bound queries (web searches, news, \
             releases \"this week\"), use the Current date above as the \
             authoritative \"today\". Do NOT substitute a different month \
             or year — your training cutoff is older than the Current \
             date, and guessing a future month produces wrong queries."
        )
    });
    parts.push(intro.clone());

    // Section: tool guidance (session permanent)
    let guidance = cache
        .sections
        .entry("tool_guidance")
        .or_insert_with(|| tool_usage_guidance().to_string());
    parts.push(guidance.clone());

    // Section: terseness directive (output-side opt, route-gated).
    // Fixed `const` content, cached under a stable key so it never perturbs the
    // prompt-cache prefix. Only emitted when the route optimizes client-side.
    if terse_enabled {
        let terse = cache
            .sections
            .entry("terseness")
            .or_insert_with(|| TERSENESS_DIRECTIVE.to_string());
        parts.push(terse.clone());
    }

    // Section: custom prompt (session permanent)
    if let Some(custom) = custom_prompt {
        let custom_cached = cache
            .sections
            .entry("custom")
            .or_insert_with(|| custom.to_string());
        parts.push(custom_cached.clone());
    }

    // Section: AGENTS.md (session permanent, hierarchical)
    let agents_section = cache.sections.entry("agents_md").or_insert_with(|| {
        let files = agents_md::collect_agents_md(cwd);
        agents_md::format_agents_md_section(&files)
    });
    if !agents_section.is_empty() {
        parts.push(agents_section.clone());
    }

    // Section: memory (cached, event-invalidated)
    // Uses the minimal prompt to save ~2,500 tokens — omits full type taxonomy
    // and examples. The full instructions are available via build_memory_prompt().
    if let Some(dir) = memory_dir {
        let memory_section = cache
            .sections
            .entry("memory")
            .or_insert_with(|| build_memory_prompt_minimal(dir));
        if !memory_section.is_empty() {
            parts.push(memory_section.clone());
        }

        // Durable-memory tools note. Tells the model it has cross-session
        // memory it can read AND write, so it recalls proactively instead of
        // treating each session as a blank slate. Only the tools actually
        // registered when memory is enabled are named here, so the prompt
        // never over-promises (unlike the unused full v2 taxonomy prompt).
        let tools_section = cache.sections.entry("memory_tools").or_insert_with(|| {
            "## Durable memory\n\
             You have long-term memory that persists across sessions:\n\
             - `session_search` — recall past sessions/episodes. Use proactively when the user \
             references prior work (\"last time\", \"we did this before\") or asks about something \
             not in the current context.\n\
             - `record_episode` — log a meaningful event worth remembering (a decision, a fix, a \
             learned preference).\n\
             - `assert_fact` — store a durable (subject, predicate, object) truth (e.g. a user \
             preference or a project fact).\n\
             Prefer recalling before asking the user to repeat themselves; store sparingly — only \
             what will matter in a later session."
                .to_string()
        });
        if !tools_section.is_empty() {
            parts.push(tools_section.clone());
        }
    }

    // Section: TOON format instructions (session permanent once enabled)
    if toon_enabled {
        let toon_section = cache
            .sections
            .entry("toon")
            .or_insert_with(|| wcore_compact::toon_format_instructions().to_string());
        parts.push(toon_section.clone());
    }

    // Section: plan mode (NOT cached — rebuilt every call when active)
    if plan_mode_active {
        parts.push(plan_prompt::plan_mode_instructions().to_string());
    }

    // Section: skills (cached, event-invalidated)
    let visible_skills: Vec<SkillRef> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .cloned()
        .collect();

    if !visible_skills.is_empty() {
        let skills_section = cache.sections.entry("skills").or_insert_with(|| {
            // Skill name/description come from plugins/MCP resources (untrusted).
            // Defang any forged host trust tags (e.g. a skill named
            // `</system-reminder>...`) before embedding in the system prompt,
            // matching the hook/memory sinks (hooks/mod.rs, engine.rs).
            let listing = wcore_config::hooks::neutralize_trust_delimiters(
                &format_skills_within_budget(&visible_skills, context_window_tokens),
            );
            if listing.is_empty() {
                String::new()
            } else {
                format!(
                    "<system-reminder>\nThe following skills are available for use with the Skill tool:\n\n{listing}\n</system-reminder>"
                )
            }
        });
        if !skills_section.is_empty() {
            parts.push(skills_section.clone());
        }
    }

    // Section: plugin rules (session permanent — content + cwd-gating are
    // both fixed for the session). `RuleScope::Universal` fragments always
    // apply; `RuleScope::ProjectScoped` ones apply only inside a project.
    let rules_section = cache
        .sections
        .entry("plugin_rules")
        .or_insert_with(|| format_plugin_rules(plugin_rules, cwd));
    if !rules_section.is_empty() {
        parts.push(rules_section.clone());
    }

    let joined = parts.join("\n\n");
    cache.joined = Some(joined.clone());
    cache.last_plan_mode = plan_mode_active;
    cache.last_toon_enabled = toon_enabled;
    cache.last_terse = terse_enabled;
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt_includes_cwd() {
        // Verify that the returned prompt contains the provided working directory path
        let cwd = "/some/test/path";
        let prompt = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            cwd,
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(prompt.contains(cwd), "system prompt should contain the cwd");
    }

    #[test]
    fn test_build_system_prompt_includes_model_name() {
        let prompt = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "deepseek-chat",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            prompt.contains("deepseek-chat"),
            "system prompt should contain the model name"
        );
        assert!(
            prompt.contains("You are powered by the model deepseek-chat"),
            "system prompt should contain the model identity line"
        );
    }

    #[test]
    fn test_build_system_prompt_with_custom_instructions() {
        // Verify that custom instructions are included in the returned prompt
        let custom = "Always respond in haiku.";
        let prompt = build_system_prompt(
            &mut SystemPromptCache::new(),
            Some(custom),
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            prompt.contains(custom),
            "system prompt should contain the custom instructions"
        );
    }

    // --- build_system_prompt Phase 9 tests ---

    use wcore_skills::refs::SkillRef;
    use wcore_skills::types::{LoadedFrom, SkillSource};

    fn make_test_skill(name: &str, description: &str, bundled: bool, hidden: bool) -> SkillRef {
        SkillRef {
            name: name.to_string(),
            display_name: None,
            description: description.to_string(),
            when_to_use: None,
            paths: vec![],
            source: if bundled {
                SkillSource::Bundled
            } else {
                SkillSource::User
            },
            loaded_from: if bundled {
                LoadedFrom::Bundled
            } else {
                LoadedFrom::Skills
            },
            file_path: std::path::PathBuf::from(format!("/tmp/{name}/SKILL.md")),
            content_length_hint: 0,
            user_invocable: true,
            disable_model_invocation: hidden,
            has_artifacts: false,
            inline_content: None,
        }
    }

    #[test]
    fn test_build_system_prompt_no_skills_no_reminder() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            !result.contains("The following skills are available"),
            "empty skills should not inject skill reminder"
        );
    }

    #[test]
    fn test_build_system_prompt_with_skills_injects_reminder() {
        let skills = vec![
            make_test_skill("skill-one", "Does one", false, false),
            make_test_skill("skill-two", "Does two", false, false),
        ];
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &skills,
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("<system-reminder>"),
            "result should contain <system-reminder>"
        );
        assert!(
            result.contains("The following skills are available for use with the Skill tool:"),
            "result should contain skills header"
        );
        assert!(
            result.contains("</system-reminder>"),
            "result should close <system-reminder>"
        );
        assert!(result.contains("skill-one"), "result should list skill-one");
        assert!(result.contains("skill-two"), "result should list skill-two");
    }

    #[test]
    fn test_build_system_prompt_hidden_skill_filtered() {
        let skills = vec![
            make_test_skill("visible-skill", "Visible", false, false),
            make_test_skill("hidden-skill", "Hidden", false, true),
        ];
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &skills,
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("visible-skill"),
            "visible skill should appear"
        );
        assert!(
            !result.contains("hidden-skill"),
            "hidden skill should be filtered out"
        );
    }

    #[test]
    fn test_build_system_prompt_all_hidden_no_reminder() {
        let skills = vec![
            make_test_skill("hidden-a", "Hidden A", false, true),
            make_test_skill("hidden-b", "Hidden B", false, true),
        ];
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &skills,
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            !result.contains("The following skills are available"),
            "all-hidden skills should not inject reminder"
        );
    }

    #[test]
    fn test_build_system_prompt_custom_prompt_and_skills() {
        let skills = vec![make_test_skill("my-skill", "My desc", false, false)];
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            Some("Custom instructions here"),
            "/tmp",
            "test-model",
            &skills,
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("Custom instructions here"),
            "custom prompt should appear"
        );
        assert!(
            result.contains("The following skills are available for use with the Skill tool:"),
            "skills reminder should also appear"
        );
    }

    #[test]
    fn test_build_system_prompt_skills_reminder_after_custom_prompt() {
        let skills = vec![make_test_skill("my-skill", "My desc", false, false)];
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            Some("Custom text"),
            "/tmp",
            "test-model",
            &skills,
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        let custom_pos = result.find("Custom text").unwrap();
        let reminder_pos = result.rfind("<system-reminder>").unwrap();
        assert!(
            reminder_pos > custom_pos,
            "skills reminder should appear after custom prompt"
        );
    }

    #[test]
    fn test_build_system_prompt_small_budget_triggers_minimal_mode() {
        // context_window_tokens = 50 → budget = 2 chars, triggers minimal mode for non-bundled
        let skill = make_test_skill("nb-skill", &"x".repeat(100), false, false);
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[skill],
            Some(50),
            None,
            false,
            false,
            &[],
            false,
        );
        // Minimal mode: skill appears as name only, no ': '
        assert!(
            result.contains("- nb-skill"),
            "skill name should appear in minimal mode"
        );
        assert!(
            !result.contains("- nb-skill: "),
            "non-bundled should not have description in minimal mode"
        );
    }

    #[test]
    fn test_build_system_prompt_cwd_in_prompt() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/workspace/my-project",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("/workspace/my-project"),
            "cwd should appear in the system prompt"
        );
    }

    #[test]
    fn test_build_system_prompt_loads_agents_md_not_claude_md() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = tmp.path();

        // Create both AGENTS.md and CLAUDE.md
        std::fs::write(cwd.join("AGENTS.md"), "AGENTS_CONTENT_HERE").unwrap();
        std::fs::write(cwd.join("CLAUDE.md"), "CLAUDE_CONTENT_HERE").unwrap();

        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            &cwd.to_string_lossy(),
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );

        assert!(
            result.contains("AGENTS_CONTENT_HERE"),
            "should load AGENTS.md content"
        );
        assert!(
            !result.contains("CLAUDE_CONTENT_HERE"),
            "should NOT load CLAUDE.md content"
        );
        assert!(
            result.contains("(project instructions)"),
            "header should indicate project instructions"
        );
        assert!(
            result.contains("AGENTS.md"),
            "header should contain AGENTS.md filename"
        );
    }

    #[test]
    fn test_build_system_prompt_no_agents_md_no_injection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = tmp.path();

        // Plant a `.git` marker so `collect_agents_md` bounds its ancestor
        // walk to this tempdir. Otherwise the boundary falls back to the home
        // directory, and on Windows runners the temp dir lives *under* the
        // user profile — so the walk would reach a home-level AGENTS.md and
        // inject "(project instructions)", failing the assertions below. On
        // Linux/mac the temp dir is outside `$HOME`, which masked the bug.
        std::fs::create_dir(cwd.join(".git")).unwrap();

        // Only CLAUDE.md exists, no AGENTS.md
        std::fs::write(cwd.join("CLAUDE.md"), "SHOULD_NOT_APPEAR").unwrap();

        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            &cwd.to_string_lossy(),
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );

        assert!(
            !result.contains("SHOULD_NOT_APPEAR"),
            "CLAUDE.md should be ignored"
        );
        assert!(
            !result.contains("(project instructions)"),
            "no project instructions should be injected"
        );
    }

    // --- Memory integration tests ---

    #[test]
    fn memory_none_dir_no_injection() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            !result.contains("auto memory"),
            "no memory content when memory_dir is None"
        );
    }

    #[test]
    fn memory_with_dir_injects_prompt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(
            mem_dir.join("MEMORY.md"),
            "- [Role](user_role.md) \u{2014} senior engineer\n",
        )
        .unwrap();

        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            Some(&mem_dir),
            false,
            false,
            &[],
            false,
        );

        assert!(
            result.contains("auto memory"),
            "should contain memory system display name"
        );
        assert!(
            result.contains("Memory types:"),
            "should contain compact memory type summary"
        );
        assert!(
            result.contains("user_role.md"),
            "should contain MEMORY.md content"
        );
    }

    #[test]
    fn memory_nonexistent_dir_graceful_degradation() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            Some(Path::new("/nonexistent/memory/dir")),
            false,
            false,
            &[],
            false,
        );

        // Should not panic and should show empty state
        assert!(
            result.contains("currently empty"),
            "nonexistent memory dir should show empty state"
        );
    }

    #[test]
    fn memory_empty_dir_shows_empty_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        // No MEMORY.md

        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            Some(&mem_dir),
            false,
            false,
            &[],
            false,
        );

        assert!(
            result.contains("currently empty"),
            "empty memory dir should show empty state"
        );
    }

    #[test]
    fn memory_appears_after_agents_md_before_skills() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = tmp.path();

        // Create AGENTS.md
        std::fs::write(cwd.join("AGENTS.md"), "PROJECT_RULES_HERE").unwrap();

        // Create memory dir with content
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("MEMORY.md"), "- [A](a.md) \u{2014} test\n").unwrap();

        let skills = vec![make_test_skill("test-skill", "A skill", false, false)];

        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            &cwd.to_string_lossy(),
            "test-model",
            &skills,
            None,
            Some(&mem_dir),
            false,
            false,
            &[],
            false,
        );

        let agents_pos = result.find("PROJECT_RULES_HERE").unwrap();
        let memory_pos = result.find("auto memory").unwrap();
        let skills_pos = result.find("test-skill").unwrap();

        assert!(
            agents_pos < memory_pos,
            "AGENTS.md should appear before memory"
        );
        assert!(
            memory_pos < skills_pos,
            "memory should appear before skills"
        );
    }

    #[test]
    fn memory_no_bb_brand_in_prompt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(
            mem_dir.join("MEMORY.md"),
            "- [Test](test.md) \u{2014} entry\n",
        )
        .unwrap();

        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            Some(&mem_dir),
            false,
            false,
            &[],
            false,
        );

        assert!(
            !result.contains("~/.claude"),
            "should not contain bb brand path"
        );
        assert!(
            !result.contains("CLAUDE.md"),
            "should not reference CLAUDE.md"
        );
    }

    // --- Tool usage guidance tests (task 4.3) ---

    #[test]
    fn tool_guidance_section_exists() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("# Using your tools"),
            "system prompt should contain the tool guidance heading"
        );
    }

    #[test]
    fn tool_guidance_contains_bash_prohibition_list() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("Glob"),
            "should mention Glob as find/ls replacement"
        );
        assert!(
            result.contains("Grep"),
            "should mention Grep as grep/rg replacement"
        );
        assert!(
            result.contains("Read"),
            "should mention Read as cat/head/tail replacement"
        );
        assert!(
            result.contains("Edit"),
            "should mention Edit as sed/awk replacement"
        );
        assert!(
            result.contains("Write"),
            "should mention Write as echo/heredoc replacement"
        );
    }

    #[test]
    fn tool_guidance_contains_parallel_call_rules() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("parallel"),
            "should contain parallel call guidance"
        );
        assert!(
            result.contains("sequentially"),
            "should explain when to run sequentially"
        );
    }

    #[test]
    fn tool_guidance_contains_edit_over_write_preference() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("Prefer Edit over Write"),
            "should contain Edit-over-Write preference"
        );
    }

    #[test]
    fn tool_guidance_contains_read_before_edit_rule() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("Read a file before editing"),
            "should contain Read-before-Edit rule"
        );
    }

    #[test]
    fn tool_guidance_after_intro_before_custom_prompt() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            Some("CUSTOM_MARKER_43"),
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        let intro_pos = result.find("Working directory").unwrap();
        let guidance_pos = result.find("# Using your tools").unwrap();
        let custom_pos = result.find("CUSTOM_MARKER_43").unwrap();
        assert!(
            guidance_pos > intro_pos,
            "tool guidance should appear after intro"
        );
        assert!(
            guidance_pos < custom_pos,
            "tool guidance should appear before custom prompt"
        );
    }

    #[test]
    fn tool_guidance_before_skills_reminder() {
        let skills = vec![make_test_skill("guide-test-skill", "A skill", false, false)];
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &skills,
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        let guidance_pos = result.find("# Using your tools").unwrap();
        let skills_pos = result.find("guide-test-skill").unwrap();
        assert!(
            guidance_pos < skills_pos,
            "tool guidance should appear before skills reminder"
        );
    }

    #[test]
    fn tool_guidance_present_in_plan_mode() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            true,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("# Using your tools"),
            "tool guidance should be present in plan mode"
        );
    }

    #[test]
    fn tool_guidance_contains_deferred_instruction() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            result.contains("deferred"),
            "tool guidance should mention deferred tools"
        );
        assert!(
            result.contains("ToolSearch"),
            "tool guidance should mention ToolSearch"
        );
    }

    #[test]
    fn tool_guidance_before_memory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("MEMORY.md"), "- [X](x.md) \u{2014} test\n").unwrap();

        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            Some(&mem_dir),
            false,
            false,
            &[],
            false,
        );
        let guidance_pos = result.find("# Using your tools").unwrap();
        let memory_pos = result.find("auto memory").unwrap();
        assert!(
            guidance_pos < memory_pos,
            "tool guidance should appear before memory section"
        );
    }

    // --- SystemPromptCache tests ---

    #[test]
    fn cache_new_is_empty() {
        let cache = SystemPromptCache::new();
        assert!(cache.joined.is_none());
        assert!(cache.sections.is_empty());
    }

    #[test]
    fn cache_stores_and_retrieves_section() {
        let mut cache = SystemPromptCache::new();
        cache.sections.insert("intro", "Hello world".to_string());
        assert_eq!(cache.sections.get("intro").unwrap(), "Hello world");
    }

    #[test]
    fn cache_invalidate_removes_section_and_joined() {
        let mut cache = SystemPromptCache::new();
        cache.sections.insert("intro", "Hello".to_string());
        cache
            .sections
            .insert("memory", "Memory content".to_string());
        cache.joined = Some("Hello\n\nMemory content".to_string());

        cache.invalidate("memory");

        assert!(!cache.sections.contains_key("memory"));
        assert!(cache.joined.is_none());
        // Other sections preserved
        assert_eq!(cache.sections.get("intro").unwrap(), "Hello");
    }

    #[test]
    fn cache_invalidate_all_clears_everything() {
        let mut cache = SystemPromptCache::new();
        cache.sections.insert("intro", "Hello".to_string());
        cache.sections.insert("memory", "Mem".to_string());
        cache.joined = Some("joined".to_string());

        cache.invalidate_all();

        assert!(cache.sections.is_empty());
        assert!(cache.joined.is_none());
    }

    #[test]
    fn cache_invalidate_nonexistent_key_is_noop() {
        let mut cache = SystemPromptCache::new();
        cache.sections.insert("intro", "Hello".to_string());
        cache.joined = Some("joined".to_string());

        cache.invalidate("nonexistent");

        // joined is still invalidated (conservative behavior)
        assert!(cache.joined.is_none());
        assert_eq!(cache.sections.get("intro").unwrap(), "Hello");
    }

    // --- Cache integration tests ---

    #[test]
    fn build_system_prompt_uses_cache_on_second_call() {
        let mut cache = SystemPromptCache::new();
        let first = build_system_prompt(
            &mut cache,
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(cache.joined.is_some());

        let second = build_system_prompt(
            &mut cache,
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert_eq!(first, second);
    }

    #[test]
    fn build_system_prompt_plan_mode_change_rebuilds() {
        let mut cache = SystemPromptCache::new();
        let without_plan = build_system_prompt(
            &mut cache,
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        let with_plan = build_system_prompt(
            &mut cache,
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            true,
            false,
            &[],
            false,
        );
        assert_ne!(without_plan, with_plan);
    }

    // --- TOON format injection tests ---

    #[test]
    fn toon_enabled_injects_format_instructions() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            true,
            &[],
            false,
        );
        assert!(
            result.contains("TOON"),
            "toon_enabled should inject TOON format instructions"
        );
        assert!(
            result.contains("Token-Oriented Object Notation"),
            "should contain full TOON description"
        );
    }

    #[test]
    fn toon_disabled_no_format_instructions() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            !result.contains("TOON"),
            "toon_disabled should not inject TOON format instructions"
        );
    }

    // --- Plugin rules tests (Task 1.4) ---

    use wcore_plugin_api::{RuleScope, RuleSpec};

    fn make_rule(name: &str, content: &str, scope: RuleScope) -> RuleSpec {
        RuleSpec {
            name: name.to_string(),
            content: content.to_string(),
            scope,
        }
    }

    #[test]
    fn universal_rule_included_in_prompt() {
        let rules = vec![make_rule(
            "u-rule",
            "UNIVERSAL_RULE_BODY_MARKER",
            RuleScope::Universal,
        )];
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &rules,
            false,
        );
        assert!(
            result.contains("UNIVERSAL_RULE_BODY_MARKER"),
            "universal rule content must appear in the system prompt"
        );
    }

    #[test]
    fn no_rules_no_section() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false,
        );
        assert!(
            !result.contains("UNIVERSAL_RULE_BODY_MARKER"),
            "empty rules must not inject any rule content"
        );
    }

    #[test]
    fn project_scoped_rule_included_inside_project() {
        // A `.git` dir makes `find_git_root` succeed → cwd is "in a project".
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        let cwd = tmp.path().to_string_lossy();

        let rules = vec![make_rule(
            "p-rule",
            "PROJECT_SCOPED_RULE_MARKER",
            RuleScope::ProjectScoped,
        )];
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            &cwd,
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &rules,
            false,
        );
        assert!(
            result.contains("PROJECT_SCOPED_RULE_MARKER"),
            "project-scoped rule must appear when cwd is inside a project"
        );
    }

    #[test]
    fn project_scoped_rule_excluded_outside_project() {
        // No `.git` anywhere up the tree → not a project.
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = tmp.path().to_string_lossy();

        let rules = vec![make_rule(
            "p-rule",
            "PROJECT_SCOPED_RULE_MARKER",
            RuleScope::ProjectScoped,
        )];
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            &cwd,
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &rules,
            false,
        );
        assert!(
            !result.contains("PROJECT_SCOPED_RULE_MARKER"),
            "project-scoped rule must be excluded when cwd is not inside a project"
        );
    }

    #[test]
    fn universal_rule_included_outside_project() {
        // Universal rules apply regardless of project membership.
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = tmp.path().to_string_lossy();

        let rules = vec![
            make_rule("u-rule", "UNIVERSAL_RULE_BODY_MARKER", RuleScope::Universal),
            make_rule(
                "p-rule",
                "PROJECT_SCOPED_RULE_MARKER",
                RuleScope::ProjectScoped,
            ),
        ];
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            &cwd,
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &rules,
            false,
        );
        assert!(
            result.contains("UNIVERSAL_RULE_BODY_MARKER"),
            "universal rule must apply even outside a project"
        );
        assert!(
            !result.contains("PROJECT_SCOPED_RULE_MARKER"),
            "project-scoped rule must still be excluded outside a project"
        );
    }

    // --- Output-side opt (Part B): terseness directive ---------------------

    /// The directive constant must be non-empty so the injected section
    /// carries actual instruction text.
    #[test]
    fn terseness_directive_is_non_empty() {
        assert!(!TERSENESS_DIRECTIVE.trim().is_empty());
    }

    /// Cache-stability: the directive must render byte-identically across two
    /// `build_system_prompt` calls so it never busts the prompt-cache prefix.
    #[test]
    fn terseness_directive_is_cache_stable_across_calls() {
        let build = || {
            build_system_prompt(
                &mut SystemPromptCache::new(),
                None,
                "/tmp",
                "test-model",
                &[],
                None,
                None,
                false,
                false,
                &[],
                true, // terse_enabled
            )
        };
        let first = build();
        let second = build();
        assert_eq!(
            first, second,
            "terseness-enabled prompt must be identical across turns (cache-safe)"
        );
        assert!(
            first.contains(TERSENESS_DIRECTIVE),
            "terse_enabled=true must inject the directive verbatim"
        );
    }

    /// Route gate: with `terse_enabled = false` the directive is omitted.
    #[test]
    fn terseness_directive_omitted_when_disabled() {
        let result = build_system_prompt(
            &mut SystemPromptCache::new(),
            None,
            "/tmp",
            "test-model",
            &[],
            None,
            None,
            false,
            false,
            &[],
            false, // terse_enabled
        );
        assert!(
            !result.contains(TERSENESS_DIRECTIVE),
            "terse_enabled=false must omit the directive (router-optimized route)"
        );
    }
}
