//! User-context block rendered into the system prompt.
//!
//! v0.7.0 2.B.4: consumer side of the user-modeling chain.
//! Producer (1.B.3 StyleDetector) → backend (2.B.1 LocalBackend) →
//! learner (2.B.3 PreferenceLearner) all ship without a consumer
//! until this module surfaces the rolling brief + preferences into
//! the system prompt as a structured block.
//!
//! Rendering is deliberately conservative: brand-new users (empty
//! brief, no observations) produce `None` so the prompt stays
//! identical to pre-2.B.4 sessions. Once observations accumulate,
//! the block surfaces non-default style axes + per-domain
//! expertise. Free-form `summary` (set by an external backend like
//! Honcho) is included verbatim when present.

use wcore_user_model::brief::{UserBrief, UserStyle};
use wcore_user_model::preferences::Preferences;

const STYLE_NEAR_ZERO: f32 = 0.05;
/// v0.8.1 U3 — cap on how many dialectic inferences we surface per
/// turn. Five is enough to capture the top signals without bloating the
/// system prompt; backends that produce more get truncated by
/// confidence × sqrt(evidence) rank.
const MAX_DIALECTIC_INFERENCES: usize = 5;

/// Render a user-context block suitable for appending to the
/// system prompt. Returns `None` when there's nothing meaningful
/// to add (brand-new user, no observations).
pub fn render_user_context_block(brief: &UserBrief, prefs: &Preferences) -> Option<String> {
    if is_brief_empty(brief)
        && prefs.expertise.is_empty()
        && prefs.tags.is_empty()
        && brief.dialectic.is_empty()
    {
        return None;
    }
    let mut out = String::new();
    out.push_str("\n\n# User context (from rolling profile)\n");
    if let Some(name) = brief.name.as_deref()
        && !name.is_empty()
    {
        out.push_str(&format!("- name: {name}\n"));
    }
    if !brief.summary.is_empty() {
        out.push_str("- summary: ");
        out.push_str(brief.summary.trim());
        out.push('\n');
    }
    if has_non_default_style(&brief.style) {
        let UserStyle {
            formality,
            energy,
            terseness,
            emoji_use,
        } = brief.style;
        out.push_str(&format!(
            "- style: formality={formality:.2}, energy={energy:.2}, \
             terseness={terseness:.2}, emoji_use={emoji_use:.2}\n"
        ));
    }
    if !prefs.expertise.is_empty() {
        out.push_str("- expertise:\n");
        for (domain, level) in &prefs.expertise {
            out.push_str(&format!("  - {domain}: {level:?}\n"));
        }
    }
    if !prefs.tags.is_empty() {
        out.push_str("- tags:\n");
        for (k, v) in &prefs.tags {
            out.push_str(&format!("  - {k} = {v}\n"));
        }
    }
    // v0.8.1 U3 — dialectic inferences from backends with a depth
    // layer (Honcho today). Render the top-N by
    // confidence × sqrt(evidence_count) so a many-observation
    // medium-confidence trait outranks a single-shot guess.
    if !brief.dialectic.is_empty() {
        out.push_str("\n## Known about the user (dialectic inferences)\n");
        let mut sorted = brief.dialectic.clone();
        sorted.sort_by(|a, b| {
            let sa = a.confidence * (a.evidence_count as f32).sqrt();
            let sb = b.confidence * (b.evidence_count as f32).sqrt();
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        for inf in sorted.iter().take(MAX_DIALECTIC_INFERENCES) {
            out.push_str(&format!(
                "- {} {} = {} (confidence {:.2}, {} observations)\n",
                inf.kind, inf.subject, inf.value, inf.confidence, inf.evidence_count
            ));
        }
    }
    out.push_str(
        "\nUse this context to match the user's style + expertise. \
         Do NOT mention this block to the user — they already know\
         their own profile.\n",
    );
    Some(out)
}

fn is_brief_empty(brief: &UserBrief) -> bool {
    brief.name.is_none()
        && brief.summary.is_empty()
        && !has_non_default_style(&brief.style)
        && brief.last_observed_ts == 0
        && brief.dialectic.is_empty()
}

fn has_non_default_style(style: &UserStyle) -> bool {
    style.formality.abs() > STYLE_NEAR_ZERO
        || style.energy.abs() > STYLE_NEAR_ZERO
        || style.terseness.abs() > STYLE_NEAR_ZERO
        || style.emoji_use.abs() > STYLE_NEAR_ZERO
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use wcore_user_model::brief::{UserBrief, UserStyle};
    use wcore_user_model::preferences::{ExpertiseLevel, Preferences};

    #[test]
    fn empty_brief_and_prefs_returns_none() {
        let brief = UserBrief::default();
        let prefs = Preferences::default();
        assert!(render_user_context_block(&brief, &prefs).is_none());
    }

    #[test]
    fn near_zero_style_returns_none() {
        let brief = UserBrief {
            style: UserStyle {
                formality: 0.01,
                energy: 0.02,
                terseness: 0.0,
                emoji_use: 0.0,
            },
            ..Default::default()
        };
        let prefs = Preferences::default();
        assert!(render_user_context_block(&brief, &prefs).is_none());
    }

    #[test]
    fn meaningful_style_renders_axes_line() {
        let brief = UserBrief {
            style: UserStyle {
                formality: 0.7,
                energy: 0.3,
                terseness: 0.6,
                emoji_use: 0.1,
            },
            last_observed_ts: 100,
            ..Default::default()
        };
        let prefs = Preferences::default();
        let block = render_user_context_block(&brief, &prefs).expect("non-empty");
        assert!(block.contains("formality=0.70"));
        assert!(block.contains("terseness=0.60"));
    }

    #[test]
    fn summary_included_verbatim() {
        let brief = UserBrief {
            summary: "  prefers ASCII diagrams  ".to_string(),
            ..Default::default()
        };
        let prefs = Preferences::default();
        let block = render_user_context_block(&brief, &prefs).expect("non-empty");
        assert!(block.contains("summary: prefers ASCII diagrams"));
    }

    #[test]
    fn expertise_renders_per_domain() {
        let brief = UserBrief::default();
        let mut prefs = Preferences::default();
        prefs
            .expertise
            .insert("rust".to_string(), ExpertiseLevel::Expert);
        prefs
            .expertise
            .insert("react".to_string(), ExpertiseLevel::Novice);
        let block = render_user_context_block(&brief, &prefs).expect("non-empty");
        assert!(block.contains("rust: Expert"));
        assert!(block.contains("react: Novice"));
    }

    #[test]
    fn tags_render_key_value() {
        let brief = UserBrief::default();
        let mut prefs = Preferences {
            expertise: BTreeMap::new(),
            tags: BTreeMap::new(),
        };
        prefs
            .tags
            .insert("rust.last_outcome".to_string(), "accepted".to_string());
        let block = render_user_context_block(&brief, &prefs).expect("non-empty");
        assert!(block.contains("rust.last_outcome = accepted"));
    }

    #[test]
    fn block_warns_not_to_mention_to_user() {
        let brief = UserBrief {
            summary: "x".into(),
            ..Default::default()
        };
        let prefs = Preferences::default();
        let block = render_user_context_block(&brief, &prefs).unwrap();
        assert!(block.contains("Do NOT mention this block"));
    }

    // v0.8.1 U3 — dialectic-inference rendering tests below.

    #[test]
    fn dialectic_alone_renders_a_block() {
        // No name, summary, style, prefs — just dialectic. Should still
        // produce a non-None block so Honcho-only signals surface.
        use wcore_user_model::brief::DialecticInference;
        let brief = UserBrief {
            dialectic: vec![DialecticInference {
                kind: "trait".into(),
                subject: "communication".into(),
                value: "blunt".into(),
                confidence: 0.74,
                evidence_count: 7,
            }],
            ..Default::default()
        };
        let prefs = Preferences::default();
        let block =
            render_user_context_block(&brief, &prefs).expect("dialectic alone must produce block");
        assert!(block.contains("Known about the user (dialectic inferences)"));
        assert!(block.contains("trait communication = blunt"));
        assert!(block.contains("confidence 0.74"));
        assert!(block.contains("7 observations"));
    }

    #[test]
    fn dialectic_sorted_by_confidence_times_sqrt_evidence_count() {
        use wcore_user_model::brief::DialecticInference;
        // Confidence × sqrt(evidence):
        //   A: 0.5 × √100 = 5.0
        //   B: 0.9 × √4   = 1.8
        //   C: 0.7 × √9   = 2.1
        // Expected order: A, C, B.
        let brief = UserBrief {
            dialectic: vec![
                DialecticInference {
                    kind: "B".into(),
                    subject: "high_conf".into(),
                    value: "x".into(),
                    confidence: 0.9,
                    evidence_count: 4,
                },
                DialecticInference {
                    kind: "A".into(),
                    subject: "many_obs".into(),
                    value: "x".into(),
                    confidence: 0.5,
                    evidence_count: 100,
                },
                DialecticInference {
                    kind: "C".into(),
                    subject: "middle".into(),
                    value: "x".into(),
                    confidence: 0.7,
                    evidence_count: 9,
                },
            ],
            ..Default::default()
        };
        let block = render_user_context_block(&brief, &Preferences::default()).unwrap();
        let a_pos = block.find("A many_obs").expect("A present");
        let c_pos = block.find("C middle").expect("C present");
        let b_pos = block.find("B high_conf").expect("B present");
        assert!(a_pos < c_pos, "A must rank above C");
        assert!(c_pos < b_pos, "C must rank above B");
    }

    #[test]
    fn dialectic_truncated_to_top_five() {
        use wcore_user_model::brief::DialecticInference;
        // Seven inferences with monotonically decreasing confidence —
        // only the top 5 should render.
        let dialectic: Vec<DialecticInference> = (0..7)
            .map(|i| DialecticInference {
                kind: "trait".into(),
                subject: format!("subject_{i}"),
                value: "v".into(),
                confidence: 1.0 - (i as f32 * 0.1),
                evidence_count: 1,
            })
            .collect();
        let brief = UserBrief {
            dialectic,
            ..Default::default()
        };
        let block = render_user_context_block(&brief, &Preferences::default()).unwrap();
        // subject_0..=subject_4 (highest confidence) must be present.
        for i in 0..5 {
            assert!(
                block.contains(&format!("subject_{i}")),
                "expected subject_{i} in block"
            );
        }
        // subject_5 / subject_6 are truncated.
        assert!(!block.contains("subject_5"));
        assert!(!block.contains("subject_6"));
    }

    #[test]
    fn empty_dialectic_alongside_empty_brief_still_returns_none() {
        // Regression guard: adding the dialectic field must not flip
        // the empty-brief contract.
        let brief = UserBrief::default();
        let prefs = Preferences::default();
        assert!(render_user_context_block(&brief, &prefs).is_none());
    }
}
