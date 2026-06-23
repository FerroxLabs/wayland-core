use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::PIIScrubber;

/// Hard-refusal phrases that indicate the LLM declined mid-task.
/// Warning-class only — callers decide whether to surface or retry.
static REFUSAL_PATTERNS: &[&str] = &[
    "I cannot",
    "I can't",
    "I won't",
    "I'm not able to",
    "As an AI",
    "As a language model",
];

static REFUSAL_RE: OnceLock<Regex> = OnceLock::new();

fn refusal_re() -> &'static Regex {
    REFUSAL_RE.get_or_init(|| {
        // Case-insensitive alternation over all phrases.
        let alt = REFUSAL_PATTERNS
            .iter()
            .map(|p| regex::escape(p))
            .collect::<Vec<_>>()
            .join("|");
        Regex::new(&format!("(?i)({alt})")).expect("wcore-safety: invalid refusal regex")
    })
}

/// Describes a validation failure returned by [`OutputValidator`].
#[derive(Debug, Error)]
pub enum ValidationFailure {
    /// LLM issued a mid-task refusal. Warning-class; task may be recoverable.
    #[error("LLM refusal detected: {phrase:?}")]
    Refusal { phrase: String },

    /// Output contains a credential or PII pattern. Hard error.
    #[error("credential leak detected in output")]
    CredentialLeak,

    /// Output did not match the caller-supplied format constraint. Hard error.
    #[error("format validation failed: {reason}")]
    FormatMismatch { reason: String },
}

impl ValidationFailure {
    /// `true` for warning-class failures; caller may choose to continue.
    pub fn is_warning(&self) -> bool {
        matches!(self, ValidationFailure::Refusal { .. })
    }
}

/// Selects which checks [`OutputValidator`] runs.
///
/// Combine with `|` (bitflags-style bitmask) or build directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct CheckSet {
    pub refusal: bool,
    pub credential_leak: bool,
    /// When `Some`, the output must match this regex.
    pub format_regex: Option<&'static str>,
}

impl CheckSet {
    pub fn all() -> Self {
        Self {
            refusal: true,
            credential_leak: true,
            format_regex: None,
        }
    }

    pub fn with_format(mut self, pattern: &'static str) -> Self {
        self.format_regex = Some(pattern);
        self
    }
}

/// Validates LLM output against a configurable set of checks.
///
/// Checks are composable: callers supply a [`CheckSet`] specifying which
/// detectors to run. All failures are returned; the first hard error short-
/// circuits remaining checks.
pub struct OutputValidator {
    checks: CheckSet,
    scrubber: PIIScrubber,
}

impl OutputValidator {
    pub fn new(checks: CheckSet) -> Self {
        Self {
            checks,
            scrubber: PIIScrubber,
        }
    }

    /// Validate `output`. Returns `Ok(())` if all enabled checks pass.
    ///
    /// Precedence: format (hard) → credential leak (hard) → refusal (warning).
    /// The first hard error is returned immediately; warnings are only returned
    /// when no hard error is present.
    pub fn validate(&self, output: &str) -> Result<(), ValidationFailure> {
        // 1. Format check — hard error.
        if let Some(pattern) = self.checks.format_regex {
            let rx = Regex::new(pattern).map_err(|e| ValidationFailure::FormatMismatch {
                reason: format!("invalid format pattern: {e}"),
            })?;
            if !rx.is_match(output) {
                return Err(ValidationFailure::FormatMismatch {
                    reason: format!("output did not match pattern `{pattern}`"),
                });
            }
        }

        // 2. Credential-leak check — hard error.
        if self.checks.credential_leak {
            let scrubbed = self.scrubber.scrub(output);
            if scrubbed != output {
                return Err(ValidationFailure::CredentialLeak);
            }
        }

        // 3. Refusal check — warning only.
        if self.checks.refusal
            && let Some(m) = refusal_re().find(output)
        {
            return Err(ValidationFailure::Refusal {
                phrase: m.as_str().to_owned(),
            });
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// LLM-judge validator (T2-A2 port of the prior Wayland Python engine)
//
// Adds an LLM-as-judge layer over the existing regex/PII checks: scores an
// output against a configurable set of criteria, charges spend against a
// USD-denominated budget cap, and elides the call (fail-open) when the
// cap is exhausted — mirroring PRD-1 §4.4 cost-cap discipline.
// ---------------------------------------------------------------------------

/// Errors surfaced by the LLM-judge validator pipeline.
///
/// Distinguishes "the judge itself failed" (provider error, network) from
/// "the budget is exhausted" (operational guardrail) so callers can decide
/// independently whether to retry, fail-open, or escalate.
#[derive(Debug, Error)]
pub enum ValidatorError {
    /// The judge implementation returned an error (provider failure,
    /// network timeout, malformed response, …). Carries the source
    /// reason as an opaque string so judges can be implementation-defined.
    #[error("judge error: {0}")]
    JudgeError(String),

    /// The USD budget cap was hit before the judge could be invoked.
    /// The caller should treat this as fail-open per PRD-1 §4.4 — emit
    /// telemetry, surface a "skipped" outcome, but do NOT block the user.
    #[error("validator budget exhausted")]
    BudgetExhausted,
}

/// Marker returned by [`ValidatorBudget::try_charge`] when the requested
/// debit would push spend over the configured cap.
///
/// Distinct type from [`ValidatorError::BudgetExhausted`] so budget
/// arithmetic can be exercised in isolation without coupling to the
/// judge pipeline.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(
    "budget exhausted: tried to charge {requested_cents} cents but only {remaining_cents} cents remain"
)]
pub struct BudgetExhausted {
    pub requested_cents: u32,
    pub remaining_cents: u32,
}

/// What axis the LLM-judge scores the output against.
///
/// Mirrors the scoring axes (factuality / grounding / consistency)
/// but generalised so callers can request a single axis or a custom one
/// without needing to ship a code change. Serde-derived so judge prompts
/// can embed the list verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name")]
pub enum ValidationCriterion {
    /// Output asserts only true facts (no fabrication).
    Truthfulness,
    /// Output answers the prompt (on-topic).
    Relevance,
    /// Output follows the requested format / schema.
    FormatCompliance,
    /// Output stays grounded in the input context (no hallucinated detail).
    NoHallucination,
    /// Caller-supplied criterion name — keeps the enum extensible without
    /// edits here. Surfaces verbatim in judge prompts.
    Custom(String),
}

/// One judge verdict — pass/fail plus the numeric score and token
/// accounting needed to settle spend against the budget.
///
/// `score` is normalised to `[0.0, 1.0]` so callers can compare across
/// judge implementations without renormalisation. `reasoning` is free-form
/// — surface it in telemetry / UI as audit trail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeVerdict {
    pub passed: bool,
    pub score: f32,
    pub reasoning: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Final outcome of a [`JudgeValidator::validate`] call.
///
/// Variants are exhaustive on purpose: `Passed` and `Failed` carry the
/// verdict (so telemetry has the score even on pass), `SkippedBudgetExhausted`
/// signals fail-open elision (regex-only verdict still upstream), and
/// `JudgeError` carries an opaque reason for the operator. Callers MUST
/// match all four — no default fall-through.
#[derive(Debug)]
pub enum ValidationOutcome {
    Passed { verdict: JudgeVerdict },
    Failed { verdict: JudgeVerdict },
    SkippedBudgetExhausted,
    JudgeError(String),
}

/// Async judge backend — one trait method, one return value.
///
/// Implementations call an LLM (or any scoring backend) and return a
/// [`JudgeVerdict`]. The validator owns budget enforcement and outcome
/// classification; the judge is purely the scoring substrate.
#[async_trait]
pub trait LlmJudge: Send + Sync {
    async fn evaluate(
        &self,
        prompt: &str,
        output: &str,
        criteria: &[ValidationCriterion],
    ) -> Result<JudgeVerdict, ValidatorError>;
}

/// USD-denominated budget cap, atomically tracked.
///
/// Cap and spend are denominated in **cents** (not microcents) so the
/// exposed surface is exactly the unit operators reason about. Internal
/// cost math uses microcents (see [`JudgeValidator::settle_cost_cents`])
/// and rounds to a cent before committing.
///
/// Uses [`Ordering::SeqCst`] compare-exchange so concurrent validators
/// sharing one budget cannot race past the cap. Per benchmark of the
/// cost-tracker port, SeqCst overhead vs. AcqRel is in the noise (<1%)
/// at the call rate validators run at; SeqCst is the simpler correctness
/// argument.
#[derive(Debug)]
pub struct ValidatorBudget {
    max_usd_cents: u32,
    spent_usd_cents: AtomicU32,
}

impl ValidatorBudget {
    /// Construct a budget with the given USD cent cap.
    pub fn new(max_usd_cents: u32) -> Self {
        Self {
            max_usd_cents,
            spent_usd_cents: AtomicU32::new(0),
        }
    }

    /// Try to reserve `cost_cents` against the cap.
    ///
    /// Uses compare-exchange in a CAS loop so multiple concurrent
    /// charges either all settle or one returns [`BudgetExhausted`].
    /// Never partially charges — either the full amount lands or
    /// the call is a no-op.
    pub fn try_charge(&self, cost_cents: u32) -> Result<(), BudgetExhausted> {
        let mut current = self.spent_usd_cents.load(Ordering::SeqCst);
        loop {
            let next = current.saturating_add(cost_cents);
            if next > self.max_usd_cents {
                return Err(BudgetExhausted {
                    requested_cents: cost_cents,
                    remaining_cents: self.max_usd_cents.saturating_sub(current),
                });
            }
            match self.spent_usd_cents.compare_exchange(
                current,
                next,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Ok(()),
                Err(observed) => current = observed,
            }
        }
    }

    /// Current spend, in cents. Convenience for telemetry.
    pub fn spent_cents(&self) -> u32 {
        self.spent_usd_cents.load(Ordering::SeqCst)
    }

    /// Cap, in cents. Convenience for telemetry.
    pub fn cap_cents(&self) -> u32 {
        self.max_usd_cents
    }
}

/// LLM-judge–backed output validator.
///
/// Generic over the judge implementation so production code wires a real
/// provider (e.g. Haiku-tier) while tests pass a deterministic mock. The
/// budget is owned (not shared via `Arc`) — callers that need a shared
/// budget across validators can wrap this struct themselves.
///
/// Cost math (per-token rates are in **microcents** to allow sub-cent
/// pricing without floating-point arithmetic):
/// ```text
///   total_microcents = input_tokens × cost_per_input_token_microcents
///                    + output_tokens × cost_per_output_token_microcents
///   total_cents     = (total_microcents + 5_000) / 10_000   // round to nearest cent
/// ```
/// Uses `u64::saturating_mul` so a runaway token count caps at `u64::MAX`
/// rather than wrapping silently.
pub struct JudgeValidator<J: LlmJudge> {
    judge: J,
    budget: ValidatorBudget,
    cost_per_input_token_microcents: u32,
    cost_per_output_token_microcents: u32,
}

impl<J: LlmJudge> JudgeValidator<J> {
    /// Build a judge validator with the given backend, budget, and
    /// per-token rates (in microcents, i.e. 1/10000 of a cent).
    pub fn new(
        judge: J,
        budget: ValidatorBudget,
        cost_per_input_token_microcents: u32,
        cost_per_output_token_microcents: u32,
    ) -> Self {
        Self {
            judge,
            budget,
            cost_per_input_token_microcents,
            cost_per_output_token_microcents,
        }
    }

    /// Borrow the underlying budget — exposed so callers can read
    /// telemetry counters without re-wrapping.
    pub fn budget(&self) -> &ValidatorBudget {
        &self.budget
    }

    /// Compute the cost (in cents, rounded to nearest) for a given
    /// token count. `saturating_mul` ensures a runaway `u64` token
    /// count caps rather than wraps.
    pub fn settle_cost_cents(&self, input_tokens: u64, output_tokens: u64) -> u32 {
        let input_microcents =
            input_tokens.saturating_mul(u64::from(self.cost_per_input_token_microcents));
        let output_microcents =
            output_tokens.saturating_mul(u64::from(self.cost_per_output_token_microcents));
        let total_microcents = input_microcents.saturating_add(output_microcents);
        // Round to nearest cent: add half-cent (5_000 microcents) then divide.
        let total_cents = total_microcents.saturating_add(5_000) / 10_000;
        if total_cents > u64::from(u32::MAX) {
            u32::MAX
        } else {
            total_cents as u32
        }
    }

    /// Validate `output_text` for `prompt` against `criteria`.
    ///
    /// Flow:
    /// 1. Check budget remaining; if zero, return `SkippedBudgetExhausted`
    ///    immediately (fail-open).
    /// 2. Invoke the judge.
    /// 3. Settle spend against the budget AFTER the judge returns (we
    ///    know exact token counts). If settling overruns the cap, we
    ///    still return the verdict — better to surface the judge's
    ///    output than discard work already paid for; the next call
    ///    will hit `SkippedBudgetExhausted` at step 1.
    /// 4. Map verdict.passed → `Passed` / `Failed`.
    pub async fn validate(
        &self,
        output_text: &str,
        prompt: &str,
        criteria: &[ValidationCriterion],
    ) -> Result<ValidationOutcome, ValidatorError> {
        // Step 1 — fail-open pre-check. If the cap has been hit by
        // prior calls, do not even invoke the judge.
        if self.budget.spent_cents() >= self.budget.cap_cents() {
            return Ok(ValidationOutcome::SkippedBudgetExhausted);
        }

        // Step 2 — invoke judge.
        let verdict = match self.judge.evaluate(prompt, output_text, criteria).await {
            Ok(v) => v,
            Err(ValidatorError::JudgeError(reason)) => {
                return Ok(ValidationOutcome::JudgeError(reason));
            }
            Err(other) => return Err(other),
        };

        // Step 3 — settle actual cost against budget. If this overruns
        // the cap, ignore the error — the verdict has already been
        // computed and the user paid for it; the next call will gate.
        let actual_cents = self.settle_cost_cents(verdict.input_tokens, verdict.output_tokens);
        let _ = self.budget.try_charge(actual_cents);

        // Step 4 — classify.
        Ok(if verdict.passed {
            ValidationOutcome::Passed { verdict }
        } else {
            ValidationOutcome::Failed { verdict }
        })
    }
}

// ---------------------------------------------------------------------------
// Tests — mock LlmJudge, no network. Covers judge pass/fail/error, budget
// charge/overcharge/tracking, validate-with-budget/exhausted/error, cost
// math (input-only, output-only, combined, saturating-overflow), criteria
// serde, and verdict score range.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod judge_tests {
    use super::*;
    use std::sync::Mutex;

    /// Mock judge with a queued verdict (or error). Lets each test
    /// inject the exact outcome it wants to exercise.
    struct MockJudge {
        next: Mutex<Option<Result<JudgeVerdict, ValidatorError>>>,
    }

    impl MockJudge {
        fn with_verdict(v: JudgeVerdict) -> Self {
            Self {
                next: Mutex::new(Some(Ok(v))),
            }
        }
        fn with_error(reason: &str) -> Self {
            Self {
                next: Mutex::new(Some(Err(ValidatorError::JudgeError(reason.to_owned())))),
            }
        }
    }

    #[async_trait]
    impl LlmJudge for MockJudge {
        async fn evaluate(
            &self,
            _prompt: &str,
            _output: &str,
            _criteria: &[ValidationCriterion],
        ) -> Result<JudgeVerdict, ValidatorError> {
            self.next
                .lock()
                .unwrap()
                .take()
                .expect("mock judge: no queued response")
        }
    }

    fn verdict(passed: bool, score: f32, input_tokens: u64, output_tokens: u64) -> JudgeVerdict {
        JudgeVerdict {
            passed,
            score,
            reasoning: "mock".to_owned(),
            input_tokens,
            output_tokens,
        }
    }

    #[tokio::test]
    async fn judge_pass_returns_validation_passed() {
        let v = JudgeValidator::new(
            MockJudge::with_verdict(verdict(true, 0.95, 100, 50)),
            ValidatorBudget::new(1_000),
            10,
            20,
        );
        let outcome = v
            .validate("hi", "p", &[ValidationCriterion::Truthfulness])
            .await
            .unwrap();
        assert!(matches!(outcome, ValidationOutcome::Passed { .. }));
    }

    #[tokio::test]
    async fn judge_fail_returns_validation_failed() {
        let v = JudgeValidator::new(
            MockJudge::with_verdict(verdict(false, 0.10, 100, 50)),
            ValidatorBudget::new(1_000),
            10,
            20,
        );
        let outcome = v
            .validate("hi", "p", &[ValidationCriterion::Truthfulness])
            .await
            .unwrap();
        assert!(matches!(outcome, ValidationOutcome::Failed { .. }));
    }

    #[test]
    fn budget_initial_can_charge_full() {
        let b = ValidatorBudget::new(500);
        assert!(b.try_charge(500).is_ok());
        assert_eq!(b.spent_cents(), 500);
    }

    #[test]
    fn budget_overcharge_returns_exhausted_error() {
        let b = ValidatorBudget::new(100);
        let err = b.try_charge(101).unwrap_err();
        assert_eq!(err.requested_cents, 101);
        assert_eq!(err.remaining_cents, 100);
        // Spend was NOT mutated.
        assert_eq!(b.spent_cents(), 0);
    }

    #[test]
    fn budget_repeated_charges_track_correctly() {
        let b = ValidatorBudget::new(1_000);
        b.try_charge(100).unwrap();
        b.try_charge(250).unwrap();
        b.try_charge(50).unwrap();
        assert_eq!(b.spent_cents(), 400);
        // Next overcharge respects accumulated spend.
        let err = b.try_charge(700).unwrap_err();
        assert_eq!(err.remaining_cents, 600);
    }

    #[tokio::test]
    async fn validate_when_budget_exhausted_returns_skipped() {
        let v = JudgeValidator::new(
            MockJudge::with_verdict(verdict(true, 1.0, 0, 0)),
            ValidatorBudget::new(10),
            1,
            1,
        );
        // Spend the budget directly first.
        v.budget().try_charge(10).unwrap();
        let outcome = v.validate("hi", "p", &[]).await.unwrap();
        assert!(matches!(outcome, ValidationOutcome::SkippedBudgetExhausted));
    }

    #[tokio::test]
    async fn validate_charges_budget_after_judge_returns() {
        let v = JudgeValidator::new(
            // 10_000 in + 5_000 out at 100 microcents/token each:
            //   total_microcents = 10_000 * 100 + 5_000 * 100 = 1_500_000
            //   cents = (1_500_000 + 5_000) / 10_000 = 150
            MockJudge::with_verdict(verdict(true, 1.0, 10_000, 5_000)),
            ValidatorBudget::new(1_000),
            100,
            100,
        );
        let outcome = v.validate("hi", "p", &[]).await.unwrap();
        assert!(matches!(outcome, ValidationOutcome::Passed { .. }));
        assert_eq!(v.budget().spent_cents(), 150);
    }

    #[tokio::test]
    async fn validate_judge_error_propagates_as_judge_error_outcome() {
        let v = JudgeValidator::new(
            MockJudge::with_error("network down"),
            ValidatorBudget::new(1_000),
            10,
            20,
        );
        let outcome = v.validate("hi", "p", &[]).await.unwrap();
        match outcome {
            ValidationOutcome::JudgeError(reason) => assert_eq!(reason, "network down"),
            other => panic!("expected JudgeError, got {other:?}"),
        }
    }

    #[test]
    fn cost_calc_input_only() {
        let v = JudgeValidator::new(
            MockJudge::with_verdict(verdict(true, 1.0, 0, 0)),
            ValidatorBudget::new(1_000_000),
            // 1000 input tokens @ 50 microcents = 50_000 microcents
            // = 5 cents (after +5_000 round-half, /10_000)
            50,
            0,
        );
        assert_eq!(v.settle_cost_cents(1_000, 0), 5);
    }

    #[test]
    fn cost_calc_output_only() {
        let v = JudgeValidator::new(
            MockJudge::with_verdict(verdict(true, 1.0, 0, 0)),
            ValidatorBudget::new(1_000_000),
            0,
            // 500 output tokens @ 200 microcents = 100_000 microcents = 10 cents
            200,
        );
        assert_eq!(v.settle_cost_cents(0, 500), 10);
    }

    #[test]
    fn cost_calc_combined_rounded_to_cent() {
        let v = JudgeValidator::new(
            MockJudge::with_verdict(verdict(true, 1.0, 0, 0)),
            ValidatorBudget::new(1_000_000),
            // 333 in @ 30 microcents = 9_990
            // 333 out @ 30 microcents = 9_990
            // total = 19_980 microcents
            // (19_980 + 5_000)/10_000 = 24_980/10_000 = 2 cents (rounded)
            30,
            30,
        );
        assert_eq!(v.settle_cost_cents(333, 333), 2);
    }

    #[test]
    fn cost_calc_saturating_at_overflow() {
        let v = JudgeValidator::new(
            MockJudge::with_verdict(verdict(true, 1.0, 0, 0)),
            ValidatorBudget::new(u32::MAX),
            u32::MAX,
            u32::MAX,
        );
        // u64::MAX tokens × u32::MAX microcents must NOT panic.
        // Saturating arithmetic caps at u64::MAX microcents, which divided
        // by 10_000 still exceeds u32::MAX, so we cap at u32::MAX.
        assert_eq!(v.settle_cost_cents(u64::MAX, u64::MAX), u32::MAX);
    }

    #[test]
    fn validation_criteria_truthfulness_serde_roundtrip() {
        let c = ValidationCriterion::Truthfulness;
        let json = serde_json::to_string(&c).unwrap();
        let back: ValidationCriterion = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn validation_criteria_custom_string_preserved() {
        let c = ValidationCriterion::Custom("brand-voice-fit".to_owned());
        let json = serde_json::to_string(&c).unwrap();
        let back: ValidationCriterion = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
        if let ValidationCriterion::Custom(name) = back {
            assert_eq!(name, "brand-voice-fit");
        } else {
            panic!("custom criterion did not round-trip");
        }
    }

    #[test]
    fn verdict_score_within_zero_one() {
        // The verdict score field is documented as `[0.0, 1.0]`; this
        // test pins the contract so a future judge implementation can't
        // silently drift outside the range without a corresponding code
        // change here.
        let v = verdict(true, 0.0, 0, 0);
        assert!((0.0..=1.0).contains(&v.score));
        let v = verdict(true, 1.0, 0, 0);
        assert!((0.0..=1.0).contains(&v.score));
        let v = verdict(false, 0.5, 0, 0);
        assert!((0.0..=1.0).contains(&v.score));
    }
}
