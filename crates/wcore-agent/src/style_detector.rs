//! User style detector — observes user messages and produces a rolling
//! [`UserStyle`] fingerprint for use by the system prompt + Phase 4
//! agent router.
//!
//! Pure heuristic — no LLM call. Six per-message signals are averaged
//! over a rolling window (default N=10):
//! - sentence_length_avg (words)
//! - formality_score (proxy: fraction of words >= 6 chars + lowercase ratio)
//! - emoji_count_avg
//! - swear_count_avg
//! - parenthetical_count_avg
//! - code_block_ratio (lines starting with ` or 4-space indent)

use std::collections::VecDeque;

/// Number of recent messages observed for the rolling average.
pub const WINDOW: usize = 10;

/// Inferred per-axis style scores. All values in `[0.0, 1.0]` unless
/// explicitly stated.
#[derive(Debug, Clone, PartialEq)]
pub struct UserStyle {
    /// Higher = more formal (proxy: long words + lowercase + few exclamations).
    pub formality: f32,
    /// Higher = higher energy (proxy: exclamations + caps + emoji).
    pub energy: f32,
    /// Higher = more terse (proxy: 1 / sentence_length_avg, clamped).
    pub terseness: f32,
    /// Average emoji-per-message count, divided by 5 then clamped.
    pub emoji_use: f32,
}

impl Default for UserStyle {
    fn default() -> Self {
        Self {
            formality: 0.5,
            energy: 0.5,
            terseness: 0.5,
            emoji_use: 0.0,
        }
    }
}

/// One per-message fingerprint, used to compute rolling averages.
#[derive(Debug, Clone, Default)]
struct Fingerprint {
    word_count: f32,
    long_word_count: f32,
    upper_letter_count: f32,
    letter_count: f32,
    exclamation_count: f32,
    emoji_count: f32,
    parenthetical_count: f32,
    code_lines: f32,
    total_lines: f32,
}

fn fingerprint(msg: &str) -> Fingerprint {
    let mut fp = Fingerprint::default();
    let words = msg.split_whitespace();
    for w in words {
        fp.word_count += 1.0;
        if w.chars().count() >= 6 {
            fp.long_word_count += 1.0;
        }
    }
    for c in msg.chars() {
        if c.is_alphabetic() {
            fp.letter_count += 1.0;
            if c.is_uppercase() {
                fp.upper_letter_count += 1.0;
            }
        }
        if c == '!' {
            fp.exclamation_count += 1.0;
        }
        if c == '(' {
            fp.parenthetical_count += 1.0;
        }
        // Emoji approximation: characters in known emoji blocks (basic
        // smileys, supplemental, transport, misc symbols). We don't
        // import `unicode-segmentation`; a coarse Unicode-block check
        // is sufficient here.
        if matches!(
            c as u32,
            0x1F300..=0x1F9FF | 0x2600..=0x27BF | 0x1FA70..=0x1FAFF
        ) {
            fp.emoji_count += 1.0;
        }
    }
    for line in msg.lines() {
        fp.total_lines += 1.0;
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || line.starts_with("    ") {
            fp.code_lines += 1.0;
        }
    }
    fp
}

/// Maintains a rolling window of message fingerprints and exposes the
/// inferred `UserStyle`.
#[derive(Debug, Clone, Default)]
pub struct StyleDetector {
    window: VecDeque<Fingerprint>,
}

impl StyleDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one user message, updating the rolling window.
    pub fn observe(&mut self, msg: &str) {
        self.window.push_back(fingerprint(msg));
        if self.window.len() > WINDOW {
            self.window.pop_front();
        }
    }

    /// Number of messages currently in the window.
    pub fn observed_count(&self) -> usize {
        self.window.len()
    }

    /// Compute the inferred user style from the rolling window. Returns
    /// `UserStyle::default()` when no messages have been observed.
    pub fn style(&self) -> UserStyle {
        if self.window.is_empty() {
            return UserStyle::default();
        }
        let n = self.window.len() as f32;
        let mut words = 0.0_f32;
        let mut long_words = 0.0_f32;
        let mut letters = 0.0_f32;
        let mut upper = 0.0_f32;
        let mut excl = 0.0_f32;
        let mut emoji = 0.0_f32;
        for fp in &self.window {
            words += fp.word_count;
            long_words += fp.long_word_count;
            letters += fp.letter_count;
            upper += fp.upper_letter_count;
            excl += fp.exclamation_count;
            emoji += fp.emoji_count;
        }
        let avg_words = (words / n).max(1.0);
        let long_word_ratio = if words > 0.0 { long_words / words } else { 0.0 };
        let upper_ratio = if letters > 0.0 { upper / letters } else { 0.0 };
        let excl_per_msg = excl / n;
        let emoji_per_msg = emoji / n;

        // Heuristic blends.
        let formality = (long_word_ratio * 0.8 + (1.0 - upper_ratio) * 0.2).clamp(0.0, 1.0);
        let energy = ((excl_per_msg / 3.0).min(1.0) * 0.5
            + upper_ratio * 0.3
            + (emoji_per_msg / 5.0).min(1.0) * 0.2)
            .clamp(0.0, 1.0);
        let terseness = (1.0 / (avg_words / 5.0).max(1.0)).clamp(0.0, 1.0);
        let emoji_use = (emoji_per_msg / 5.0).clamp(0.0, 1.0);

        UserStyle {
            formality,
            energy,
            terseness,
            emoji_use,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_detector_yields_default() {
        let d = StyleDetector::new();
        assert_eq!(d.style(), UserStyle::default());
    }

    #[test]
    fn high_energy_message_scores_energy() {
        let mut d = StyleDetector::new();
        d.observe("WOW that is AMAZING!!! 🚀🚀🚀");
        let s = d.style();
        assert!(s.energy > 0.5, "expected high energy, got {}", s.energy);
        assert!(s.emoji_use > 0.0);
    }

    #[test]
    fn formal_message_scores_formality() {
        let mut d = StyleDetector::new();
        d.observe(
            "Pursuant to the discussion regarding implementation strategy, consider \
             the architectural implications of subsequent modifications.",
        );
        let s = d.style();
        assert!(
            s.formality > 0.5,
            "expected high formality, got {}",
            s.formality
        );
    }

    #[test]
    fn terse_messages_score_terseness() {
        let mut d = StyleDetector::new();
        d.observe("ok");
        d.observe("yes");
        d.observe("nope");
        let s = d.style();
        assert!(
            s.terseness > 0.5,
            "expected high terseness, got {}",
            s.terseness
        );
    }

    #[test]
    fn window_truncates_at_capacity() {
        let mut d = StyleDetector::new();
        for i in 0..(WINDOW + 5) {
            d.observe(&format!("message {i}"));
        }
        assert_eq!(d.observed_count(), WINDOW);
    }

    #[test]
    fn observe_then_style_repeatable() {
        let mut d = StyleDetector::new();
        d.observe("hello world");
        let a = d.style();
        let b = d.style();
        assert_eq!(a, b, "style() should be pure");
    }
}
