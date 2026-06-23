//! v0.8.1 U6 — autonomous skill creation. After N=3 consecutive successful
//! turns on the same task signature, draft a candidate skill to
//! `$WAYLAND_HOME/skills/auto/` and record it in GEPA's `PromptStore` so
//! the per-turn `SkillRouter` (v0.8.1 U1) picks it up at next bootstrap.
//!
//! This is the closed-loop self-improvement path:
//!   runtime observation → bucket → draft → PromptStore → SkillRouter seed.
//!
//! Sibling, NOT replacement, of `wcore_skills::draft::PatternDetector`
//! (W9.1 T3 / T10b). The pattern detector keys on `tool_sequence +
//! input_shape` and stages a `Procedure` in memory. U6 keys on a normalized
//! task-signature derived from the *user input text* and bridges into
//! GEPA's evolved-prompt store so the next session's `SkillRouter`
//! hydrates the draft as a seed pair. The two paths are complementary:
//! the detector captures execution patterns, U6 captures intent patterns.
//!
//! Self-learning from runtime observation without RL.

pub mod bucketer;
pub mod drafter;
pub mod recorder;

pub use bucketer::{Bucketer, DraftTrigger, signature};
pub use drafter::{DraftError, DraftResult, SkillDrafter};
pub use recorder::{TurnOutcome, TurnTrajectory};
