//! Plateau heuristic: if the rolling-window top score hasn't improved by
//! `min_delta` over `window` generations, declare a plateau and terminate.
//!
//! **Default `window = 3`** (High 1 audit fix). With four mutators rotated
//! round-robin and `fan_out = 4` children, a single noisy generation can
//! produce a momentary below-baseline top score; a window of 3 gives every
//! mutator at least one shot before declaring no improvement. Window MUST
//! be ≥ number of mutator strategies in rotation, or this default flag
//! turns into a false-plateau hazard.
//!
//! Wave RC (audit MAJOR #9) — non-finite scores (`NaN`, `±inf`) are
//! rejected at `push` time with [`PlateauError::NonFiniteScore`].
//! IEEE 754 makes `NaN == NaN` false and every NaN comparison false,
//! so a single NaN would leave `should_terminate` permanently `false`
//! and let the evolution loop run forever. The detector now refuses
//! to accept the bad sample; the loop surfaces the failure via
//! `TerminationReason::ScoreInvalid` rather than hanging.

/// Error returned from [`PlateauDetector::push`] when an invalid score
/// is encountered. Caught by the evolution loop and surfaced as
/// `TerminationReason::ScoreInvalid` to the host.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlateauError {
    /// A non-finite score (`NaN`, `+inf`, `-inf`) was pushed. The
    /// detector cannot make progress on this input because IEEE 754
    /// comparison semantics make every comparison against NaN false,
    /// so improvement and regression are both indeterminate.
    #[error("plateau detector received non-finite score: bits={bits:#x}")]
    NonFiniteScore {
        /// IEEE 754 bit pattern of the offending score, retained for
        /// logging without losing precision on NaN payloads.
        bits: u64,
    },
}

pub struct PlateauDetector {
    window: usize,
    min_delta: f64,
    history: Vec<f64>,
}

impl PlateauDetector {
    pub fn new(window: usize, min_delta: f64) -> Self {
        Self {
            window,
            min_delta,
            history: Vec::new(),
        }
    }

    /// Append a generation's top score. Wave RC: non-finite scores
    /// are refused so the detector cannot enter an indeterminate
    /// "neither improving nor regressing" state.
    pub fn push(&mut self, score: f64) -> Result<(), PlateauError> {
        if !score.is_finite() {
            return Err(PlateauError::NonFiniteScore {
                bits: score.to_bits(),
            });
        }
        self.history.push(score);
        Ok(())
    }

    /// Test-helper view of the recorded history.
    #[cfg(test)]
    pub(crate) fn history(&self) -> &[f64] {
        &self.history
    }

    pub fn should_terminate(&self) -> bool {
        // We need at least `window + 1` entries to compare a baseline to a
        // post-baseline window. Without that, no plateau can be declared.
        if self.history.len() <= self.window {
            return false;
        }
        let start = self.history.len().saturating_sub(self.window + 1);
        let recent = self.history.get(start..).unwrap_or(&[]);
        let baseline = match recent.first() {
            Some(&b) => b,
            None => return false,
        };
        let best = recent
            .iter()
            .skip(1)
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        best - baseline < self.min_delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_rejects_nan() {
        let mut d = PlateauDetector::new(3, 0.01);
        assert!(matches!(
            d.push(f64::NAN),
            Err(PlateauError::NonFiniteScore { .. })
        ));
        assert!(d.history().is_empty(), "NaN must not be retained");
        assert!(!d.should_terminate());
    }

    #[test]
    fn push_rejects_positive_infinity() {
        let mut d = PlateauDetector::new(3, 0.01);
        assert!(matches!(
            d.push(f64::INFINITY),
            Err(PlateauError::NonFiniteScore { .. })
        ));
        assert!(d.history().is_empty());
    }

    #[test]
    fn push_rejects_negative_infinity() {
        let mut d = PlateauDetector::new(3, 0.01);
        assert!(matches!(
            d.push(f64::NEG_INFINITY),
            Err(PlateauError::NonFiniteScore { .. })
        ));
        assert!(d.history().is_empty());
    }

    #[test]
    fn push_accepts_zero_and_negative_finite() {
        let mut d = PlateauDetector::new(3, 0.01);
        assert!(d.push(0.0).is_ok(), "zero is finite");
        assert!(d.push(-1.0).is_ok(), "negative-but-finite is accepted");
        assert_eq!(d.history().len(), 2);
    }
}
