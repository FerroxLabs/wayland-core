use unicode_width::UnicodeWidthStr;

use crate::refs::SkillRef;
use crate::types::SkillSource;

// Skill listing gets 1% of the context window (in characters)
pub const SKILL_BUDGET_CONTEXT_PERCENT: f64 = 0.01;
pub use wcore_config::compact::CHARS_PER_TOKEN;
/// Fallback listing budget when the caller does not know the context window.
///
/// FerroxLabs/wayland#1150 deleted the fabricated 200,000-token window from
/// every OTHER boundary and left it here: this constant was `8_000`, whose own
/// comment read "1% of 200k x 4". For an unlisted model with no
/// `[compact] context_window` — the #1150 reporter's exact configuration —
/// `known_context_window` correctly returns `None`, the bootstrap passes that
/// `None` straight through, and the skills listing was sized against 200,000
/// while every other boundary in the same session was sized against
/// [`wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW`] = 32,768. Measured:
/// 1,310 chars intended, 8,000 granted — 6.1x, about 2,000 tokens of a 32,768
/// -token window spent on the skill listing.
///
/// It is now the SAME assumption the rest of the session makes, derived rather
/// than restated so the two cannot drift apart again.
pub const DEFAULT_CHAR_BUDGET: usize =
    wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW * CHARS_PER_TOKEN / 100;

/// The derivation above must stay the integer image of the `Some` arm's
/// formula, checked by the compiler rather than by a test that could be
/// deleted with the constant.
const _: () = assert!(
    DEFAULT_CHAR_BUDGET == wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW * CHARS_PER_TOKEN / 100
);
pub const MAX_LISTING_DESC_CHARS: usize = 250;

const MIN_DESC_LENGTH: usize = 20;

/// The reachability half of FerroxLabs/wayland#1280 (c2), in one fixed line.
///
/// The ceiling below TRIMS skills out of the listing. A listing bounded by
/// silently dropping skills the model then cannot reach is a wrong refusal and
/// worse than the bytes it saves, so a trimmed listing always ends with this
/// line: it names the escape hatch (`Skill { query }`, see
/// `wcore_agent::skill_tool`) and the count of what was withheld. It is a
/// constant plus the decimal digits of one number, so it is bounded in the
/// skill count exactly like the rest of the listing.
pub const SKILL_OVERFLOW_HINT: &str = "more installed skills are not listed here. \
     Call the Skill tool with {\"query\": \"<what you need to do>\"} to search \
     every installed skill by name and description, then invoke the one you \
     want by its exact name.";

/// Calculate character budget from context window size.
pub fn get_char_budget(context_window_tokens: Option<usize>) -> usize {
    match context_window_tokens {
        Some(tokens) => {
            ((tokens as f64) * (CHARS_PER_TOKEN as f64) * SKILL_BUDGET_CONTEXT_PERCENT) as usize
        }
        None => DEFAULT_CHAR_BUDGET,
    }
}

/// Format a skill's combined description string (description + when_to_use),
/// truncated to MAX_LISTING_DESC_CHARS.
pub fn format_skill_description(skill: &SkillRef) -> String {
    let desc = match &skill.when_to_use {
        Some(wtu) if !wtu.is_empty() => format!("{} - {}", skill.description, wtu),
        _ => skill.description.clone(),
    };

    if UnicodeWidthStr::width(desc.as_str()) > MAX_LISTING_DESC_CHARS {
        truncate_to_width(&desc, MAX_LISTING_DESC_CHARS)
    } else {
        desc
    }
}

/// Cut `text` to at most `limit` display columns, ending in an ellipsis.
///
/// Extracted so the description cap and the search-result cap cannot drift
/// into two different notions of "truncate".
fn truncate_to_width(text: &str, limit: usize) -> String {
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw >= limit {
            break;
        }
        out.push(ch);
        width += cw;
    }
    out.push('\u{2026}');
    out
}

/// Format a single skill entry for the listing: `- name: description`.
pub fn format_skill_entry(skill: &SkillRef) -> String {
    format!("- {}: {}", skill.name, format_skill_description(skill))
}

/// Width of a rendered listing: every entry plus the newlines that join them.
fn listing_width(entries: &[String]) -> usize {
    entries
        .iter()
        .map(|e| UnicodeWidthStr::width(e.as_str()))
        .sum::<usize>()
        + entries.len().saturating_sub(1)
}

/// The trailing line a trimmed listing always carries. See
/// [`SKILL_OVERFLOW_HINT`].
fn overflow_line(omitted: usize) -> String {
    format!("- (+{omitted} {SKILL_OVERFLOW_HINT})")
}

/// The hard ceiling — FerroxLabs/wayland#1280 c1.
///
/// Keeps entries in order until the next one would not leave room for the
/// overflow line, then stops and states how many were withheld. The result is
/// at most `budget` columns wide, except in the degenerate case where the
/// budget is smaller than a single overflow line (a 100-token context window
/// gives a 4-char budget), where it is the overflow line alone. That residual
/// is a CONSTANT — it does not grow with the skill count, which is the property
/// c1 asks for — and dropping it instead would leave the model with neither the
/// skills nor the means to find them.
fn clamp_to_budget(entries: Vec<String>, budget: usize) -> String {
    if listing_width(&entries) <= budget {
        return entries.join("\n");
    }

    let total = entries.len();
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0usize;
    for (i, entry) in entries.iter().enumerate() {
        let sep = usize::from(!kept.is_empty());
        let after = used + sep + UnicodeWidthStr::width(entry.as_str());
        let still_omitted = total - i - 1;
        let tail = if still_omitted == 0 {
            0
        } else {
            1 + UnicodeWidthStr::width(overflow_line(still_omitted).as_str())
        };
        if after + tail > budget {
            break;
        }
        kept.push(entry.clone());
        used = after;
    }

    let omitted = total - kept.len();
    if omitted == 0 {
        return kept.join("\n");
    }
    kept.push(overflow_line(omitted));
    kept.join("\n")
}

/// Format all skills within budget, applying four-level degradation and a
/// hard ceiling.
///
/// Levels:
/// 1. Full mode: all skills with full descriptions
/// 2. Truncated mode: bundled skills full, non-bundled descriptions trimmed
/// 3. Minimal mode: bundled skills full, non-bundled names only
/// 4. Names mode: every skill, bundled included, as a bare name
///
/// Every level is CHECKED against the budget rather than assumed to fit, and
/// whatever survives is clamped by [`clamp_to_budget`]. Before
/// FerroxLabs/wayland#1280 the bundled entries were SUBTRACTED from the budget
/// (`remaining_budget = budget.saturating_sub(bundled_chars)`) and never
/// capped, level 3 emitted every bundled skill at full description plus every
/// non-bundled name, and a listing with no non-bundled skill at all returned
/// unconditionally. All three terms grew linearly in the skill count and none
/// was bounded by the window: measured against the 1,310-char budget a 32,768
/// -token window implies, 1,000 project skills rendered 19,999 chars (15.3x)
/// and 100 bundled skills rendered 22,399 (17.1x, about 5,600 tokens of the
/// window, on every ordinary turn).
pub fn format_skills_within_budget(
    skills: &[SkillRef],
    context_window_tokens: Option<usize>,
) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let budget = get_char_budget(context_window_tokens);

    // Build full entries for all skills
    let full_entries: Vec<String> = skills.iter().map(format_skill_entry).collect();

    // Level 1: full mode
    if listing_width(&full_entries) <= budget {
        return full_entries.join("\n");
    }

    // Partition into bundled and non-bundled
    let mut bundled_indices: Vec<usize> = Vec::new();
    let mut rest_indices: Vec<usize> = Vec::new();
    for (i, skill) in skills.iter().enumerate() {
        if skill.source == SkillSource::Bundled {
            bundled_indices.push(i);
        } else {
            rest_indices.push(i);
        }
    }

    // Space the bundled block would take at full description. #1280: this is
    // charged AGAINST the budget, not subtracted from it — if it does not fit,
    // levels 2 and 3 are skipped outright instead of overrunning.
    // +1 per bundled entry accounts for the trailing newline separator
    let bundled_chars: usize = bundled_indices
        .iter()
        .map(|&i| UnicodeWidthStr::width(full_entries[i].as_str()) + 1)
        .sum();

    if !rest_indices.is_empty() && bundled_chars < budget {
        let remaining_budget = budget - bundled_chars;

        // name_overhead = Σ (name.len() + 4) for each non-bundled skill
        // where 4 = "- " (2) + ": " (2) prefix/suffix
        // plus (rest_count - 1) newline separators between non-bundled entries
        let rest_name_overhead: usize = rest_indices
            .iter()
            .map(|&i| UnicodeWidthStr::width(skills[i].name.as_str()) + 4)
            .sum::<usize>()
            + rest_indices.len().saturating_sub(1);

        let available_for_descs = remaining_budget.saturating_sub(rest_name_overhead);
        let per_desc_budget = available_for_descs / rest_indices.len();

        // Level 2: truncated mode — non-bundled descriptions trimmed
        if per_desc_budget >= MIN_DESC_LENGTH {
            let entries: Vec<String> = skills
                .iter()
                .enumerate()
                .map(|(i, skill)| {
                    if skill.source == SkillSource::Bundled {
                        return full_entries[i].clone();
                    }
                    let desc = format_skill_description(skill);
                    let trimmed = if UnicodeWidthStr::width(desc.as_str()) > per_desc_budget {
                        truncate_to_width(&desc, per_desc_budget.saturating_sub(1))
                    } else {
                        desc
                    };
                    format!("- {}: {}", skill.name, trimmed)
                })
                .collect();
            if listing_width(&entries) <= budget {
                return entries.join("\n");
            }
        }

        // Level 3: minimal mode — non-bundled show names only
        let entries: Vec<String> = skills
            .iter()
            .enumerate()
            .map(|(i, skill)| {
                if skill.source == SkillSource::Bundled {
                    full_entries[i].clone()
                } else {
                    format!("- {}", skill.name)
                }
            })
            .collect();
        if listing_width(&entries) <= budget {
            return entries.join("\n");
        }
    }

    // Level 4: names only, bundled included, then the hard ceiling. This is
    // the arm that used to not exist: level 3 returned unconditionally, and a
    // set with no non-bundled skill at all returned every bundled entry at
    // full description however far over budget that put it.
    let names: Vec<String> = skills.iter().map(|s| format!("- {}", s.name)).collect();
    clamp_to_budget(names, budget)
}

/// How many results a skill search may return.
///
/// The point of the search is to be the bounded escape hatch from a bounded
/// listing; an unbounded result would reintroduce the term the ceiling exists
/// to remove, on the tool-result path instead of the prompt.
pub const SKILL_SEARCH_MAX_RESULTS: usize = 10;

/// Columns of description each search hit may carry.
pub const SKILL_SEARCH_DESC_CHARS: usize = 120;

/// Rank `skills` against `query`, best first, at most `limit` hits.
///
/// FerroxLabs/wayland#1280 c2: this is how a skill the ceiling trimmed out of
/// the listing is found again. Scoring is deliberately dumb — a name hit is
/// worth more than a description hit, and a skill matching no query token at
/// all is not a hit — because the caller is the model, which will refine the
/// query itself if the first answer is wrong.
pub fn search_skills<'a>(skills: &'a [SkillRef], query: &str, limit: usize) -> Vec<&'a SkillRef> {
    let tokens: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect();
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(usize, &SkillRef)> = skills
        .iter()
        .filter_map(|skill| {
            let name = skill.name.to_lowercase();
            let haystack = format!(
                "{} {}",
                skill.description,
                skill.when_to_use.as_deref().unwrap_or("")
            )
            .to_lowercase();
            let score: usize = tokens
                .iter()
                .map(|t| {
                    usize::from(name.contains(t.as_str())) * 3
                        + usize::from(haystack.contains(t.as_str()))
                })
                .sum();
            (score > 0).then_some((score, skill))
        })
        .collect();

    // Stable, name-ordered tie-break so the same query returns the same answer
    // twice — a search that reshuffles is a search the model cannot re-issue.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    scored.truncate(limit);
    scored.into_iter().map(|(_, s)| s).collect()
}

/// Render [`search_skills`] hits as the body of a `Skill { query }` result.
///
/// Bounded by construction: at most `limit` lines, each at most a name plus
/// [`SKILL_SEARCH_DESC_CHARS`] columns of description.
pub fn format_skill_search_results(hits: &[&SkillRef], total_installed: usize) -> String {
    if hits.is_empty() {
        return format!(
            "No skill matched that query. {total_installed} skill(s) are installed; \
             try fewer or different keywords, or invoke a skill directly by its \
             exact name."
        );
    }
    let mut out = format!(
        "{} of {total_installed} installed skill(s) matched. Invoke one by its \
         exact name with {{\"skill\": \"<name>\"}}.\n",
        hits.len()
    );
    for hit in hits {
        let desc = format_skill_description(hit);
        let desc = if UnicodeWidthStr::width(desc.as_str()) > SKILL_SEARCH_DESC_CHARS {
            truncate_to_width(&desc, SKILL_SEARCH_DESC_CHARS)
        } else {
            desc
        };
        out.push_str(&format!("\n- {}: {}", hit.name, desc));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refs::SkillRef;
    use crate::types::{LoadedFrom, SkillSource};

    fn make_skill(
        name: &str,
        description: &str,
        when_to_use: Option<&str>,
        bundled: bool,
        hidden: bool,
    ) -> SkillRef {
        SkillRef {
            name: name.to_string(),
            display_name: None,
            description: description.to_string(),
            when_to_use: when_to_use.map(|s| s.to_string()),
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
            skill_root: None,
            content_length_hint: 0,
            user_invocable: true,
            disable_model_invocation: hidden,
            has_artifacts: false,
            inline_content: None,
        }
    }

    // --- get_char_budget ---

    #[test]
    fn test_get_char_budget_none_returns_default() {
        assert_eq!(get_char_budget(None), DEFAULT_CHAR_BUDGET);
    }

    /// FerroxLabs/wayland#1150 D16 — the `None` arm must make the SAME window
    /// assumption as every other boundary in the session.
    ///
    /// #1150 removed the fabricated 200,000-token window everywhere else and
    /// left it here: `DEFAULT_CHAR_BUDGET` was 8,000, "1% of 200k x 4". For an
    /// unlisted model with no `[compact] context_window` — the reporter's exact
    /// configuration — `known_context_window` correctly returns `None`, the
    /// bootstrap passes it straight through, and the skills listing was sized
    /// against 200,000 while everything else was sized against 32,768: 1,310
    /// chars intended, 8,000 granted, 6.1x, about 2,000 tokens of a
    /// 32,768-token window spent on the listing.
    #[test]
    fn the_unknown_window_budget_is_the_unverified_window_not_the_old_200k() {
        assert_eq!(
            get_char_budget(None),
            get_char_budget(Some(wcore_config::compact::UNVERIFIED_CONTEXT_WINDOW)),
            "the unknown-window arm must not assume a window the rest of the \
             session refuses to assume"
        );
        assert_eq!(get_char_budget(None), 1_310);
        assert_ne!(
            get_char_budget(None),
            get_char_budget(Some(wcore_config::compact::DEFAULT_CONTEXT_WINDOW)),
            "8,000 chars is 1% of 200,000 tokens, and #1150 deleted that \
             assumption from every other boundary"
        );
    }

    #[test]
    fn test_get_char_budget_200k_tokens() {
        // 200_000 * 4 * 0.01 = 8_000
        assert_eq!(get_char_budget(Some(200_000)), 8_000);
    }

    #[test]
    fn test_get_char_budget_small_window() {
        // 100 * 4 * 0.01 = 4
        assert_eq!(get_char_budget(Some(100)), 4);
    }

    #[test]
    fn test_get_char_budget_zero_tokens() {
        assert_eq!(get_char_budget(Some(0)), 0);
    }

    #[test]
    fn test_get_char_budget_large_window() {
        // 1_000_000 * 4 * 0.01 = 40_000
        assert_eq!(get_char_budget(Some(1_000_000)), 40_000);
    }

    // --- format_skill_description ---

    #[test]
    fn test_format_skill_description_no_when_to_use() {
        let skill = make_skill("s", "A simple skill", None, false, false);
        assert_eq!(format_skill_description(&skill), "A simple skill");
    }

    #[test]
    fn test_format_skill_description_with_when_to_use() {
        let skill = make_skill("s", "Does X", Some("Use when Y"), false, false);
        assert_eq!(format_skill_description(&skill), "Does X - Use when Y");
    }

    #[test]
    fn test_format_skill_description_truncates_long_description() {
        // description is 300 ASCII chars, no when_to_use
        let desc = "a".repeat(300);
        let skill = make_skill("s", &desc, None, false, false);
        let result = format_skill_description(&skill);
        // implementation truncates by char count: result chars <= MAX_LISTING_DESC_CHARS
        assert!(
            result.chars().count() <= MAX_LISTING_DESC_CHARS,
            "result should be truncated to MAX_LISTING_DESC_CHARS chars"
        );
        assert!(
            result.ends_with('\u{2026}'),
            "truncated result should end with ellipsis"
        );
    }

    #[test]
    fn test_format_skill_description_truncates_combined_over_limit() {
        // description 200 chars + " - " + when_to_use 100 chars = 303 > 250
        let desc = "a".repeat(200);
        let wtu = "b".repeat(100);
        let skill = make_skill("s", &desc, Some(&wtu), false, false);
        let result = format_skill_description(&skill);
        assert!(
            result.ends_with('\u{2026}'),
            "should be truncated with ellipsis"
        );
    }

    #[test]
    fn test_format_skill_description_empty_description() {
        let skill = make_skill("s", "", None, false, false);
        assert_eq!(format_skill_description(&skill), "");
    }

    #[test]
    fn test_format_skill_description_empty_when_to_use_ignored() {
        // empty when_to_use string should not add " - "
        let skill = make_skill("s", "desc", Some(""), false, false);
        assert_eq!(format_skill_description(&skill), "desc");
    }

    #[test]
    fn test_format_skill_description_exactly_at_limit() {
        // description exactly 250 chars — should NOT be truncated
        let desc = "x".repeat(MAX_LISTING_DESC_CHARS);
        let skill = make_skill("s", &desc, None, false, false);
        let result = format_skill_description(&skill);
        assert_eq!(result, desc);
        assert!(!result.ends_with('\u{2026}'));
    }

    // --- format_skill_entry ---

    #[test]
    fn test_format_skill_entry_basic() {
        let skill = make_skill("my-skill", "Does things", None, false, false);
        assert_eq!(format_skill_entry(&skill), "- my-skill: Does things");
    }

    #[test]
    fn test_format_skill_entry_with_when_to_use() {
        let skill = make_skill("my-skill", "Does things", Some("When needed"), false, false);
        assert_eq!(
            format_skill_entry(&skill),
            "- my-skill: Does things - When needed"
        );
    }

    #[test]
    fn test_format_skill_entry_truncates_long_description() {
        let desc = "a".repeat(300);
        let skill = make_skill("x", &desc, None, false, false);
        let result = format_skill_entry(&skill);
        assert!(
            result.starts_with("- x: "),
            "entry should start with '- x: '"
        );
        assert!(
            result.contains('\u{2026}'),
            "long description should be truncated"
        );
    }

    #[test]
    fn test_format_skill_entry_empty_name() {
        let skill = make_skill("", "desc", None, false, false);
        assert_eq!(format_skill_entry(&skill), "- : desc");
    }

    // --- format_skills_within_budget ---

    #[test]
    fn test_format_skills_within_budget_empty_returns_empty() {
        assert_eq!(format_skills_within_budget(&[], None), "");
        assert_eq!(format_skills_within_budget(&[], Some(0)), "");
    }

    #[test]
    fn test_format_skills_within_budget_full_mode() {
        // 3 short skills well within 8_000 char default budget
        let skills = vec![
            make_skill("skill-a", "Desc A", None, false, false),
            make_skill("skill-b", "Desc B", None, false, false),
            make_skill("skill-c", "Desc C", None, false, false),
        ];
        let result = format_skills_within_budget(&skills, None);
        assert!(result.contains("- skill-a: Desc A"));
        assert!(result.contains("- skill-b: Desc B"));
        assert!(result.contains("- skill-c: Desc C"));
        assert!(
            !result.contains('\u{2026}'),
            "full mode should not truncate"
        );
    }

    #[test]
    fn test_format_skills_within_budget_full_mode_line_count() {
        let skills = vec![
            make_skill("a", "Desc A", None, false, false),
            make_skill("b", "Desc B", None, false, false),
            make_skill("c", "Desc C", None, false, false),
        ];
        let result = format_skills_within_budget(&skills, None);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 3, "each skill should be on its own line");
    }

    #[test]
    fn test_format_skills_within_budget_truncated_mode() {
        // budget = 10_000 * 4 * 0.01 = 400 chars
        // 1 bundled skill (short), 5 non-bundled each with 200-char description
        // bundled ~60 chars, remaining ~340 / 5 = 68 chars per non-bundled (>= MIN_DESC_LENGTH=20)
        let bundled = make_skill("bundled", "Bundled description here", None, true, false);
        let non_bundled: Vec<SkillRef> = (0..5)
            .map(|i| make_skill(&format!("nb-{i}"), &"z".repeat(200), None, false, false))
            .collect();

        let mut skills = vec![bundled];
        skills.extend(non_bundled);

        let result = format_skills_within_budget(&skills, Some(10_000));

        // bundled skill should be complete (no ellipsis in its description)
        assert!(
            result.contains("Bundled description here"),
            "bundled skill description should be intact"
        );
        // at least some non-bundled should be truncated
        assert!(
            result.contains('\u{2026}'),
            "non-bundled descriptions should be truncated in truncated mode"
        );
    }

    /// Minimal mode (level 3) is reached when the per-description budget falls
    /// under `MIN_DESC_LENGTH` but the resulting listing still FITS. That
    /// distinction is the #1280 fix: this case used to be tested at a 2-char
    /// budget and asserted that the bundled entry stayed full anyway, which is
    /// the unbounded behaviour the ceiling removes. Level 3 is exercised here
    /// at a budget that can actually hold it.
    #[test]
    fn test_format_skills_within_budget_minimal_mode() {
        let bundled = make_skill("bundled", "Bundled full desc", None, true, false);
        let nb_skills: Vec<SkillRef> = vec![
            make_skill("nb-alpha", &"x".repeat(100), None, false, false),
            make_skill("nb-beta", &"y".repeat(100), None, false, false),
        ];

        let mut skills = vec![bundled];
        skills.extend(nb_skills);

        // 1,500 tokens -> 60 chars. Full mode needs ~240, and 60 chars over two
        // non-bundled skills leaves under MIN_DESC_LENGTH each, so level 3 is
        // selected — and it fits, so no clamp.
        let result = format_skills_within_budget(&skills, Some(1_500));

        assert!(
            result.contains("Bundled full desc"),
            "bundled skill should remain full in minimal mode: {result}"
        );
        assert!(
            result.contains("- nb-alpha\n") || result.ends_with("- nb-alpha"),
            "nb-alpha should appear as name only: {result}"
        );
        assert!(
            result.contains("- nb-beta\n") || result.ends_with("- nb-beta"),
            "nb-beta should appear as name only: {result}"
        );
        assert!(
            !result.contains("- nb-alpha: "),
            "non-bundled should not have description in minimal mode"
        );
        assert!(
            !result.contains(SKILL_OVERFLOW_HINT),
            "level 3 fit this budget, so nothing should have been trimmed"
        );
        assert!(
            UnicodeWidthStr::width(result.as_str()) <= get_char_budget(Some(1_500)),
            "minimal mode overran its budget: {result}"
        );
    }

    #[test]
    fn test_format_skills_within_budget_single_skill_full() {
        let skill = make_skill("solo", "Solo description", None, false, false);
        let result = format_skills_within_budget(&[skill], None);
        assert!(result.contains("- solo: Solo description"));
    }

    #[test]
    fn test_format_skills_within_budget_max_desc_limit_respected() {
        // Single skill with 300-char description; default budget is large enough for full mode,
        // but format_skill_description always caps at MAX_LISTING_DESC_CHARS.
        let long_desc = "d".repeat(300);
        let skill = make_skill("big", &long_desc, None, false, false);
        let result = format_skills_within_budget(&[skill], None);
        let prefix = "- big: ";
        let desc_part = result.strip_prefix(prefix).unwrap_or(&result);
        // implementation truncates at char boundary: MAX_LISTING_DESC_CHARS - 1 chars + ellipsis = 250 chars
        assert!(
            desc_part.chars().count() <= MAX_LISTING_DESC_CHARS,
            "entry description must not exceed MAX_LISTING_DESC_CHARS chars"
        );
        assert!(desc_part.ends_with('\u{2026}'));
    }

    /// An all-bundled set is charged against the budget like any other.
    ///
    /// This test previously asserted the opposite — "even if over budget, all
    /// are shown full" — which is the `rest_indices.is_empty()` early return
    /// FerroxLabs/wayland#1280 c1 names as unbounded in the skill count.
    #[test]
    fn test_format_skills_within_budget_only_bundled_skills() {
        let skills: Vec<SkillRef> = (0..3)
            .map(|i| {
                make_skill(
                    &format!("bundled-{i}"),
                    &format!("Desc {i}"),
                    None,
                    true,
                    false,
                )
            })
            .collect();

        // Roomy: nothing is trimmed, so the clamp is not an unconditional cut.
        let roomy = format_skills_within_budget(&skills, None);
        for i in 0..3 {
            assert!(
                roomy.contains(&format!("- bundled-{i}: Desc {i}")),
                "bundled skill {i} should be intact when it fits"
            );
        }
        assert!(!roomy.contains(SKILL_OVERFLOW_HINT));

        // Zero budget: bounded, and what it drops it names.
        let result = format_skills_within_budget(&skills, Some(1));
        assert!(
            result.contains(SKILL_OVERFLOW_HINT),
            "an all-bundled set was trimmed with no route back: {result}"
        );
        assert!(
            UnicodeWidthStr::width(result.as_str())
                <= UnicodeWidthStr::width(format!("- (+3 {SKILL_OVERFLOW_HINT})").as_str()),
            "an all-bundled listing overran a zero budget: {result}"
        );
    }

    // --- CJK / multi-byte UTF-8 boundary tests ---

    #[test]
    fn test_format_skill_description_cjk_short_preserved() {
        // TC-31: short CJK description should be returned as-is
        let skill = make_skill("s", "这是一个技能描述", None, false, false);
        let result = format_skill_description(&skill);
        assert_eq!(result, "这是一个技能描述");
    }

    #[test]
    fn test_format_skill_description_cjk_long_truncated_no_panic() {
        // TC-32: 300 CJK chars must be truncated to <= 250 chars without panicking
        let desc = "技".repeat(300);
        let skill = make_skill("s", &desc, None, false, false);
        let result = format_skill_description(&skill);
        assert!(
            result.chars().count() <= MAX_LISTING_DESC_CHARS,
            "CJK description should be truncated to <= {} chars",
            MAX_LISTING_DESC_CHARS
        );
        assert!(
            result.ends_with('…'),
            "truncated CJK result should end with ellipsis"
        );
    }

    #[test]
    fn test_format_skill_description_mixed_cjk_ascii_truncated_no_panic() {
        // TC-33: mixed ASCII + CJK over 250 chars must be truncated without panicking
        let desc = format!("Skill: {}", "描述".repeat(150));
        let skill = make_skill("s", &desc, None, false, false);
        let result = format_skill_description(&skill);
        assert!(
            result.chars().count() <= MAX_LISTING_DESC_CHARS,
            "mixed CJK/ASCII description should be truncated to <= {} chars",
            MAX_LISTING_DESC_CHARS
        );
        assert!(
            result.ends_with('…'),
            "truncated mixed result should end with ellipsis"
        );
    }

    #[test]
    fn test_format_skills_within_budget_truncated_mode_cjk_no_panic() {
        // TC-34: truncated mode with CJK descriptions must not panic
        // budget = 10_000 * 4 * 0.01 = 400 chars; each CJK desc is 200 chars → triggers truncation
        let bundled = make_skill("bundled", "Bundled desc", None, true, false);
        let non_bundled: Vec<SkillRef> = (0..3)
            .map(|i| {
                make_skill(
                    &format!("nb-{i}"),
                    &"中文描述".repeat(50),
                    None,
                    false,
                    false,
                )
            })
            .collect();

        let mut skills = vec![bundled];
        skills.extend(non_bundled);

        // should not panic
        let result = format_skills_within_budget(&skills, Some(10_000));
        assert!(
            result.contains('…') || !result.is_empty(),
            "result should be non-empty and handle CJK without panic"
        );
        assert!(
            result.contains("bundled"),
            "bundled skill must appear in result"
        );
    }
}
