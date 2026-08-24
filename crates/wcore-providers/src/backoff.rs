//! The one retry backoff curve.
//!
//! Every retryable provider failure — served, unserved, or rate-limited —
//! waits on the same schedule:
//!
//! ```text
//! base(n)  = min(500 ms * 2^(n-1), RETRY_BACKOFF_CAP)     n is 1-based
//! delay(n) = base(n) * (1 + U[0, 0.25])
//! ```
//!
//! | n              | 1   | 2   | 3   | 4   | 5   | 6    | 7+   |
//! |----------------|-----|-----|-----|-----|-----|------|------|
//! | base (s)       | 0.5 | 1   | 2   | 4   | 8   | 16   | 24   |
//! | cumulative (s) | 0.5 | 1.5 | 3.5 | 7.5 | 15.5| 31.5 | ...  |
//!
//! Ten retries spend 127.5 s of base, 143.4 s at the mean jitter draw and
//! 159.4 s at the maximum.
//!
//! ## Why one curve, and why these numbers
//!
//! **One curve.** This replaces a linear `500 ms * n` schedule for served
//! failures and a separate doubling schedule for unserved ones. The two are
//! indistinguishable at the shipped default budget (0.5 s then 1.0 s either
//! way), so the split bought nothing at n=2; at n=10 the linear arm would
//! spend 27.5 s across ten re-sends of a provider having a bad minute, which
//! is a send loop wearing the word "backoff".
//!
//! **The cap is 24 s, not 32 s.** [`RETRY_BACKOFF_CAP`] times the maximum
//! jitter draw is exactly [`wcore_config::config::DEFAULT_RECOVERY_TIMEOUT_SECS`]
//! — the breaker's own cooldown base. The cap this replaces carried a
//! doc-comment promising that the gap between two re-sends never exceeds that
//! cooldown, so the engine never probes a wedged endpoint faster than the
//! component whose job is to protect it. Jitter would have broken that promise
//! by 25 % at a 30 s cap; deriving the cap from the cooldown instead of
//! copying it keeps the promise exact. `the_cap_plus_max_jitter_is_the_breaker_cooldown`
//! pins the arithmetic.
//!
//! **Jitter is new, and it is upward only.** This product ships a fleet
//! dispatcher and parallel sub-agents. Without jitter, N workers that meet the
//! same outage re-send in lockstep against a provider that is already failing.
//! Drawing upward only means jitter can never shorten a wait below a server
//! instruction or below the curve.
//!
//! ## A server instruction outranks the curve
//!
//! [`retry_delay`] takes the `Retry-After` the provider actually sent, as a
//! NUMBER. It is never re-parsed out of a rendered error string: the value is
//! resolved once at the HTTP boundary
//! ([`crate::retry::resolve_retry_after_ms`]: header, then nested body, then
//! [`crate::retry::DEFAULT_RETRY_AFTER_MS`]) and carried through as `u64`.
//!
//! A hint wins outright — it is not `max(hint, curve)`. A provider that asks
//! for 100 ms gets 100 ms even when the curve is at 4 s, because a rate
//! limiter that says "come back in 100 ms" knows something the curve does not.
//! The small `U[0, 1 s]` added on top exists only so a herd handed an
//! identical `Retry-After: 5` does not return in the same millisecond; being
//! additive it can never shorten the wait below what the server asked for.
//!
//! ## Testing the jitter without disabling it
//!
//! [`scope_jitter`] pins the draw for the duration of one future, using the
//! same `tokio::task_local` shape as [`crate::retry::scope_max_retries`].
//! Production never sets it, so production always draws from the real RNG —
//! and `the_production_draw_is_random_not_a_constant` fails if that ever stops
//! being true.

use std::time::Duration;

/// First step of the curve, and the multiplier every later step doubles.
pub const RETRY_BASE_DELAY: Duration = Duration::from_millis(500);

/// Fraction of the base delay that jitter may ADD. Never subtracts.
pub const RETRY_JITTER_FRACTION: f64 = 0.25;

/// Ceiling on the gap between two re-sends, and so also the worst-case delay
/// between the provider healing and the run noticing.
///
/// Derived, not chosen: the largest cap whose maximum jittered draw still
/// lands on [`wcore_config::config::DEFAULT_RECOVERY_TIMEOUT_SECS`], the
/// breaker cooldown this schedule must never outpace. At
/// [`RETRY_JITTER_FRACTION`] = 0.25 that is `cooldown * 4 / 5` = 24 s.
pub const RETRY_BACKOFF_CAP: Duration =
    Duration::from_millis(wcore_config::config::DEFAULT_RECOVERY_TIMEOUT_SECS * 1000 * 4 / 5);

/// Ceiling on how long a `Retry-After` may park the loop.
///
/// A provider that asks for more than a minute is telling the caller to go
/// somewhere else; past this the run should surface the rate limit so the
/// resilience layer can fail over rather than sit on one throttled endpoint.
/// Deliberately smaller than `RETRY_AFTER_CAP_MS`, which
/// bounds the value RECORDED rather than the value SLEPT.
pub const RETRY_AFTER_SLEEP_CAP: Duration = Duration::from_secs(60);

/// Spread added on top of an honoured `Retry-After`.
///
/// Additive rather than proportional: a herd handed the same hint must not
/// return in the same millisecond, and no draw may return EARLIER than the
/// server asked.
pub const RETRY_AFTER_JITTER: Duration = Duration::from_secs(1);

tokio::task_local! {
    /// Pinned jitter draw for the current scope, in `[0, 1]`. Test-only in
    /// practice: production never enters a scope, so production always draws
    /// from the RNG below.
    static JITTER_DRAW: f64;
}

/// Run `future` with the jitter draw pinned to `draw` (clamped to `[0, 1]`).
///
/// Same shape as [`crate::retry::scope_max_retries`]: a `tokio::task_local`
/// that only ever narrows behaviour inside the scope. A test pins `0.0` for an
/// exact assertion or `1.0` to grade the upper bound; nothing in production
/// calls this, which is the property
/// `the_production_draw_is_random_not_a_constant` exists to keep true.
pub async fn scope_jitter<F>(draw: f64, future: F) -> F::Output
where
    F: std::future::Future,
{
    JITTER_DRAW.scope(draw.clamp(0.0, 1.0), future).await
}

/// One draw from `U[0, 1]` — pinned inside [`scope_jitter`], random outside.
fn jitter_draw() -> f64 {
    JITTER_DRAW
        .try_with(|draw| *draw)
        .unwrap_or_else(|_| rand::random::<f64>())
}

/// Un-jittered curve value for 1-based retry `attempt`.
///
/// `min(500 ms * 2^(attempt-1), RETRY_BACKOFF_CAP)`. Attempt 0 is treated as
/// attempt 1 — the loops are 1-based and a 0 would mean "retry with no wait".
pub fn base_backoff(attempt: u32) -> Duration {
    // Shift is bounded well below u64's width; the `min` below does the real
    // capping, this only keeps the shift itself defined.
    let shift = attempt.saturating_sub(1).min(20);
    RETRY_BASE_DELAY
        .saturating_mul(1u32 << shift)
        .min(RETRY_BACKOFF_CAP)
}

/// Apply the upward-only jitter to a base delay.
fn jittered(base: Duration) -> Duration {
    base.mul_f64(1.0 + RETRY_JITTER_FRACTION * jitter_draw())
}

/// How long to wait before re-sending, for 1-based retry `attempt`.
///
/// - `retry_after_ms` — the hint the provider actually sent, as a number.
///   Honoured outright (capped at [`RETRY_AFTER_SLEEP_CAP`], plus
///   [`RETRY_AFTER_JITTER`] of spread). It is NOT combined with the curve:
///   a server that asks for less than the curve gets what it asked for.
/// - `rate_limited` — this failure was a 429 but carried no usable hint. The
///   curve applies with [`crate::retry::DEFAULT_RETRY_AFTER_MS`] as a FLOOR,
///   so an early retry does not hammer a limiter at 500 ms.
/// - neither — the plain curve.
pub fn retry_delay(attempt: u32, retry_after_ms: Option<u64>, rate_limited: bool) -> Duration {
    match retry_after_ms {
        Some(hint) => {
            Duration::from_millis(hint).min(RETRY_AFTER_SLEEP_CAP)
                + RETRY_AFTER_JITTER.mul_f64(jitter_draw())
        }
        None if rate_limited => jittered(
            base_backoff(attempt).max(Duration::from_millis(crate::retry::DEFAULT_RETRY_AFTER_MS)),
        ),
        None => jittered(base_backoff(attempt)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table in this module's docs, asserted rather than described.
    #[test]
    fn the_curve_doubles_from_half_a_second_and_holds_the_cap() {
        let ms = |attempt| base_backoff(attempt).as_millis();
        assert_eq!(ms(1), 500);
        assert_eq!(ms(2), 1_000);
        assert_eq!(ms(3), 2_000);
        assert_eq!(ms(4), 4_000);
        assert_eq!(ms(5), 8_000);
        assert_eq!(ms(6), 16_000);
        // 500 * 2^6 = 32 s, past the cap.
        for attempt in 7..=30u32 {
            assert_eq!(
                base_backoff(attempt),
                RETRY_BACKOFF_CAP,
                "attempt {attempt} must hold the cap, not keep doubling"
            );
        }
        // Attempt 0 must not mean "re-send immediately".
        assert_eq!(ms(0), 500);
    }

    /// The whole reason the cap is 24 and not 30: with jitter on top, a 30 s
    /// cap would break the promise that a re-send never outpaces the breaker
    /// cooldown. Bound to the config constant so the two cannot drift.
    #[test]
    fn the_cap_plus_max_jitter_is_the_breaker_cooldown() {
        let cooldown = Duration::from_secs(wcore_config::config::DEFAULT_RECOVERY_TIMEOUT_SECS);
        assert_eq!(RETRY_BACKOFF_CAP, Duration::from_secs(24));
        assert_eq!(
            RETRY_BACKOFF_CAP.mul_f64(1.0 + RETRY_JITTER_FRACTION),
            cooldown,
            "the maximum jittered gap must land exactly on the breaker \
             cooldown, not past it"
        );
    }

    #[tokio::test]
    async fn jitter_is_upward_only_and_bounded_at_a_quarter() {
        for attempt in 1..=8u32 {
            let base = base_backoff(attempt);
            let floor = scope_jitter(0.0, async { retry_delay(attempt, None, false) }).await;
            let ceiling = scope_jitter(1.0, async { retry_delay(attempt, None, false) }).await;
            assert_eq!(
                floor, base,
                "the minimum draw must be the un-jittered curve; jitter that \
                 can subtract would re-send earlier than the schedule allows"
            );
            assert_eq!(
                ceiling,
                base.mul_f64(1.25),
                "the maximum draw must be exactly a quarter above the base"
            );
        }
    }

    /// The property that a test-only constant would silently destroy: with no
    /// scope in force — which is every production call — the draw must be a
    /// real random variable inside the documented band.
    ///
    /// Mutation-checked by construction: replace `rand::random` with any
    /// constant and the distinctness assertion fails; widen the band and the
    /// bounds assertion fails.
    #[test]
    fn the_production_draw_is_random_not_a_constant() {
        let base = base_backoff(3);
        let draws: Vec<Duration> = (0..64).map(|_| retry_delay(3, None, false)).collect();
        for d in &draws {
            assert!(
                *d >= base && *d <= base.mul_f64(1.25),
                "an unscoped draw of {d:?} left the band [{base:?}, {:?}]",
                base.mul_f64(1.25)
            );
        }
        let distinct: std::collections::HashSet<u128> =
            draws.iter().map(|d| d.as_nanos()).collect();
        assert!(
            distinct.len() > 32,
            "64 unscoped draws produced only {} distinct delays — production \
             is not jittering",
            distinct.len()
        );
    }

    #[tokio::test]
    async fn a_server_hint_outranks_the_curve_in_both_directions() {
        // Larger than the curve.
        let big = scope_jitter(0.0, async { retry_delay(1, Some(7_000), true) }).await;
        assert_eq!(big, Duration::from_millis(7_000));
        // SMALLER than the curve — the half this gets wrong if the loop
        // computes `max(hint, base)`.
        let small = scope_jitter(0.0, async { retry_delay(6, Some(100), true) }).await;
        assert_eq!(
            small,
            Duration::from_millis(100),
            "attempt 6 sits at a 16 s base; a 100 ms server instruction must \
             still win outright"
        );
    }

    #[tokio::test]
    async fn a_hint_is_capped_but_its_jitter_never_shortens_it() {
        let capped = scope_jitter(0.0, async { retry_delay(1, Some(600_000), true) }).await;
        assert_eq!(
            capped, RETRY_AFTER_SLEEP_CAP,
            "a ten-minute hint must be capped at the sleep ceiling"
        );
        for draw in [0.0, 0.5, 1.0] {
            let d = scope_jitter(draw, async { retry_delay(1, Some(5_000), true) }).await;
            assert!(
                d >= Duration::from_millis(5_000),
                "draw {draw} returned {d:?}, EARLIER than the 5 s the server \
                 asked for"
            );
            assert!(d <= Duration::from_millis(5_000) + RETRY_AFTER_JITTER);
        }
    }

    #[tokio::test]
    async fn a_hintless_rate_limit_floors_at_the_default_retry_after() {
        let floor = Duration::from_millis(crate::retry::DEFAULT_RETRY_AFTER_MS);
        for attempt in 1..=3u32 {
            let d = scope_jitter(0.0, async { retry_delay(attempt, None, true) }).await;
            assert!(
                d >= floor,
                "attempt {attempt} waited {d:?} on a 429 with no usable hint; \
                 the default retry-after is the floor"
            );
        }
        // Past the floor the curve takes over again rather than pinning at 5 s.
        let late = scope_jitter(0.0, async { retry_delay(6, None, true) }).await;
        assert_eq!(late, base_backoff(6));
        // Control: the same attempts WITHOUT the rate-limit flag are not
        // floored, so the floor is doing the work and not the curve.
        let unflagged = scope_jitter(0.0, async { retry_delay(1, None, false) }).await;
        assert_eq!(unflagged, RETRY_BASE_DELAY);
        assert!(unflagged < floor);
    }
}
