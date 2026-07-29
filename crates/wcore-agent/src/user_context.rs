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
use wcore_user_model::correction::Corrections;
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
///
/// Back-compat shim: renders with no user corrections. Production goes through
/// [`render_user_context_block_with_corrections`].
pub fn render_user_context_block(brief: &UserBrief, prefs: &Preferences) -> Option<String> {
    render_user_context_block_with_corrections(brief, prefs, &Corrections::default())
}

/// Render the user-context block, with user-authored corrections taking
/// **precedence** over every inferred value they cover.
///
/// 23B-C3 (user-model half). Precedence here is subtractive, not additive: a
/// corrected subject's inferred line is *suppressed* rather than printed
/// alongside the correction. Printing both would put a contradiction in the
/// prompt and leave the model to choose which of the two the user meant, which
/// is not user control — it is a coin flip the user cannot see.
///
/// Corrections also survive `LocalBackend::observe`'s EMA fold and the P5
/// `UserModelInferencer`'s session-end overwrite, because they are not stored
/// in either structure. See `wcore_user_model::correction`.
pub fn render_user_context_block_with_corrections(
    brief: &UserBrief,
    prefs: &Preferences,
    corrections: &Corrections,
) -> Option<String> {
    if is_brief_empty(brief)
        && prefs.expertise.is_empty()
        && prefs.tags.is_empty()
        && brief.dialectic.is_empty()
        && corrections.is_empty()
    {
        return None;
    }
    let mut out = String::new();
    out.push_str("\n\n# User context (from rolling profile)\n");
    if let Some(name) = brief.name.as_deref()
        && !name.is_empty()
        && !corrections.suppresses("name")
    {
        out.push_str(&format!("- name: {name}\n"));
    }
    if !brief.summary.is_empty() && !corrections.suppresses("summary") {
        out.push_str("- summary: ");
        out.push_str(brief.summary.trim());
        out.push('\n');
    }
    if has_non_default_style(&brief.style) && !corrections.suppresses("style") {
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
    let expertise: Vec<(&String, &wcore_user_model::preferences::ExpertiseLevel)> = prefs
        .expertise
        .iter()
        .filter(|(domain, _)| !corrections.suppresses(&format!("expertise.{domain}")))
        .collect();
    if !expertise.is_empty() {
        out.push_str("- expertise:\n");
        for (domain, level) in expertise {
            out.push_str(&format!("  - {domain}: {level:?}\n"));
        }
    }
    let tags: Vec<(&String, &String)> = prefs
        .tags
        .iter()
        .filter(|(k, _)| !corrections.suppresses(&format!("tags.{k}")))
        .collect();
    if !tags.is_empty() {
        out.push_str("- tags:\n");
        for (k, v) in tags {
            out.push_str(&format!("  - {k} = {v}\n"));
        }
    }
    // v0.8.1 U3 — dialectic inferences from backends with a depth
    // layer (Honcho today). Render the top-N by
    // confidence × sqrt(evidence_count) so a many-observation
    // medium-confidence trait outranks a single-shot guess.
    let dialectic: Vec<_> = brief
        .dialectic
        .iter()
        .filter(|inf| {
            !corrections.suppresses("dialectic")
                && !corrections.suppresses(&format!("dialectic.{}", inf.subject))
                && !corrections.suppresses(&format!("{}.{}", inf.kind, inf.subject))
        })
        .cloned()
        .collect();
    if !dialectic.is_empty() {
        out.push_str("\n## Known about the user (dialectic inferences)\n");
        let mut sorted = dialectic;
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
    // 23B-C3: user-authored corrections, last and authoritative. Everything
    // above is inferred from observed behaviour and can be wrong; everything
    // here the user stated about themselves on purpose. The instruction line
    // is explicit because a model that treats a correction as merely one more
    // signal will average it against the inferences it was meant to replace.
    if !corrections.is_empty() {
        out.push_str("\n## Corrected by the user (authoritative — overrides the above)\n");
        for c in corrections.iter() {
            out.push_str(&format!("- {} = {}\n", c.key, c.value));
        }
        out.push_str(
            "These are the user's own statements about themselves. Where one \
             conflicts with anything inferred above, the user's statement is \
             correct and the inference is wrong.\n",
        );
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

    // ---------------------------------------------------------------
    // 23B-C3 user-model half — correction precedence in the rendered
    // block. These assert on the text that is appended verbatim to the
    // system prompt at `bootstrap.rs:2108-2118`, i.e. on what reaches
    // the provider.
    // ---------------------------------------------------------------

    use wcore_user_model::correction::{CorrectionStore, Corrections};

    /// Build a `Corrections` through the real store rather than by
    /// constructing the type directly, so these tests exercise the same
    /// normalisation and validation the slash command hits.
    fn corrections_of(pairs: &[(&str, &str)]) -> Corrections {
        let store = CorrectionStore::in_memory();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            for (k, v) in pairs {
                store.correct("u", k, v, 0).await.unwrap();
            }
            store.corrections("u").await
        })
    }

    #[test]
    fn correction_suppresses_the_inferred_expertise_it_replaces() {
        let brief = UserBrief::default();
        let mut prefs = Preferences::default();
        prefs
            .expertise
            .insert("rust".to_string(), ExpertiseLevel::Novice);
        prefs
            .expertise
            .insert("python".to_string(), ExpertiseLevel::Novice);

        let corrections = corrections_of(&[("expertise.rust", "expert")]);
        let block =
            render_user_context_block_with_corrections(&brief, &prefs, &corrections).unwrap();

        // The corrected value is present and marked authoritative.
        assert!(block.contains("expertise.rust = expert"));
        assert!(block.contains("authoritative"));
        // KNOWN-NEGATIVE: the inference this correction replaces must be
        // GONE. Before 23B-C3 the block rendered `- rust: Novice`
        // unconditionally, so this assertion fails on the old renderer —
        // which is the whole point of the fix.
        assert!(
            !block.contains("rust: Novice"),
            "the replaced inference must not also be in the prompt; \
             block was:\n{block}"
        );
        // A sibling domain the user did NOT correct stays inferred.
        assert!(
            block.contains("python: Novice"),
            "an uncorrected sibling must survive; block was:\n{block}"
        );
    }

    #[test]
    fn correcting_a_whole_subject_suppresses_all_its_children() {
        let brief = UserBrief::default();
        let mut prefs = Preferences::default();
        prefs
            .expertise
            .insert("rust".to_string(), ExpertiseLevel::Novice);
        prefs
            .expertise
            .insert("python".to_string(), ExpertiseLevel::Expert);

        let corrections = corrections_of(&[("expertise", "I am a beginner at everything")]);
        let block =
            render_user_context_block_with_corrections(&brief, &prefs, &corrections).unwrap();

        assert!(!block.contains("rust: Novice"));
        assert!(!block.contains("python: Expert"));
        assert!(
            !block.contains("- expertise:\n"),
            "the whole inferred expertise section must go, not just its rows"
        );
        assert!(block.contains("I am a beginner at everything"));
    }

    #[test]
    fn correcting_style_removes_the_inferred_axis_line() {
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
        let corrections = corrections_of(&[("style", "blunt, no preamble")]);
        let block = render_user_context_block_with_corrections(
            &brief,
            &Preferences::default(),
            &corrections,
        )
        .unwrap();
        assert!(
            !block.contains("formality=0.70"),
            "corrected style must not sit next to the numbers it corrects"
        );
        assert!(block.contains("style = blunt, no preamble"));
    }

    #[test]
    fn correction_alone_produces_a_block_for_an_otherwise_empty_user() {
        // A user whose very first action is to state something about
        // themselves must reach the model, even with zero observations.
        let corrections = corrections_of(&[("name", "Sean")]);
        let block = render_user_context_block_with_corrections(
            &UserBrief::default(),
            &Preferences::default(),
            &corrections,
        )
        .expect("a correction alone must produce a block");
        assert!(block.contains("name = Sean"));
    }

    #[test]
    fn no_corrections_renders_byte_identically_to_the_old_two_arg_form() {
        // Guard: the corrections feature must be invisible to every user
        // who has never made one.
        let brief = UserBrief {
            summary: "prefers ASCII diagrams".into(),
            last_observed_ts: 5,
            ..Default::default()
        };
        let mut prefs = Preferences::default();
        prefs
            .expertise
            .insert("rust".to_string(), ExpertiseLevel::Expert);
        let with_empty =
            render_user_context_block_with_corrections(&brief, &prefs, &Corrections::default());
        assert_eq!(with_empty, render_user_context_block(&brief, &prefs));
        assert!(
            !with_empty.unwrap().contains("authoritative"),
            "the corrections section must not appear when there are none"
        );
    }

    #[test]
    fn uncorrected_name_still_renders() {
        let brief = UserBrief {
            name: Some("Inferred Name".into()),
            last_observed_ts: 1,
            ..Default::default()
        };
        let corrections = corrections_of(&[("summary", "x")]);
        let block = render_user_context_block_with_corrections(
            &brief,
            &Preferences::default(),
            &corrections,
        )
        .unwrap();
        assert!(
            block.contains("- name: Inferred Name"),
            "correcting `summary` must not suppress `name`"
        );
    }
}
