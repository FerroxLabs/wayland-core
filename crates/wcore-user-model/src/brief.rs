//! User brief — the rolling summary the engine injects into each
//! turn's system prompt.

use serde::{Deserialize, Serialize};

/// Inferred user style. Mirrors the 4 axes produced by the
/// `StyleDetector` in 1.B.3; the backend folds new fingerprints into
/// these running averages.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UserStyle {
    pub formality: f32,
    pub energy: f32,
    pub terseness: f32,
    pub emoji_use: f32,
}

/// v0.8.1 U3 — one dialectic inference about the user, produced by
/// upstream dialectic backends (e.g. Honcho's representations layer)
/// rather than explicit `learn_preference` writes.
///
/// Each inference carries a self-reported confidence (0.0..=1.0) and
/// an `evidence_count` (number of supporting observations the backend
/// saw). The engine surfaces the top-N by `confidence × sqrt(evidence)`
/// in the per-turn user-context block — high-confidence inferences
/// backed by many observations beat single-shot guesses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialecticInference {
    /// Inference category — `"preference"`, `"expertise"`, `"trait"`,
    /// or any backend-specific label. Free-form by design so new
    /// dialectic shapes don't require a schema bump.
    pub kind: String,
    /// What the inference is about (e.g. `"code_style"`, `"rust"`,
    /// `"communication"`).
    pub subject: String,
    /// The inferred value (e.g. `"terse"`, `"expert"`, `"blunt"`).
    pub value: String,
    /// Backend-reported confidence in this inference, 0.0..=1.0.
    pub confidence: f32,
    /// Number of underlying observations the backend folded into this
    /// inference. Used as the second sort key so a many-observation
    /// medium-confidence inference outranks a single-shot guess.
    pub evidence_count: u32,
}

/// Rolling brief consumed by `UserContextMiddleware` (2.B.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UserBrief {
    /// Display name. Optional — anonymous users have None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Free-form one-paragraph summary the backend maintains.
    #[serde(default)]
    pub summary: String,
    /// Rolling style estimate.
    #[serde(default)]
    pub style: UserStyle,
    /// Unix epoch seconds of the last observation folded in.
    #[serde(default)]
    pub last_observed_ts: i64,
    /// v0.8.1 U3 — dialectic inferences pulled from backends that
    /// support them (Honcho today). Empty for backends without a
    /// dialectic layer (LocalBackend); the engine renders a dedicated
    /// section only when this is non-empty so brand-new users still
    /// produce identical prompts to pre-v0.8.1 sessions.
    #[serde(default)]
    pub dialectic: Vec<DialecticInference>,
}
