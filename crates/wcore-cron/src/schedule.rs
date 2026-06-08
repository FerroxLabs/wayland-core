//! Cron expression parsing.
//!
//! Wraps the `cron` crate. The `cron` crate's `Schedule::from_str`
//! expects a 6- or 7-field expression (`sec min hour dom mon dow [year]`),
//! but the standard `crontab` shape and what users actually type is
//! 5-field (`min hour dom mon dow`). We therefore normalize 5-field
//! input to 6-field by prepending `0` (zeroth-second-of-the-minute) and
//! pass the result to `cron`.
//!
//! Tests cover both common shapes.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use cron::Schedule;

use crate::CronError;

/// Parse a cron expression, accepting either 5-field crontab shape or
/// 6/7-field `cron`-crate shape.
pub fn parse_expression(raw: &str) -> Result<Schedule, CronError> {
    let normalized = normalize(raw)?;
    Schedule::from_str(&normalized).map_err(|e| CronError::InvalidExpression(e.to_string()))
}

/// Compute the next fire time strictly after `after`. Returns Ok(None)
/// when the schedule has no future occurrence (a pinned past expression).
pub fn next_fire_after(
    expression: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, CronError> {
    let schedule = parse_expression(expression)?;
    Ok(schedule.after(&after).next())
}

fn normalize(raw: &str) -> Result<String, CronError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CronError::InvalidExpression(
            "expression is empty".to_string(),
        ));
    }
    let field_count = trimmed.split_whitespace().count();
    match field_count {
        // Standard crontab 5-field: prepend "0" (second-of-minute) so the
        // `cron` crate sees 6-field input.
        5 => Ok(format!("0 {trimmed}")),
        // Already 6- or 7-field; pass through and let the `cron` crate
        // do the heavy lifting.
        6 | 7 => Ok(trimmed.to_string()),
        n => Err(CronError::InvalidExpression(format!(
            "expected 5, 6, or 7 fields, got {n}: {raw:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parses_5_field() {
        let s = parse_expression("0 9 * * *").unwrap();
        // Sanity: a fixed reference time before 9:00 should produce a
        // next-fire on the same day at 09:00:00.
        let anchor = Utc.with_ymd_and_hms(2026, 5, 22, 8, 0, 0).unwrap();
        let nf = s.after(&anchor).next().unwrap();
        assert_eq!(nf, Utc.with_ymd_and_hms(2026, 5, 22, 9, 0, 0).unwrap());
    }

    #[test]
    fn parses_5_field_every_15_min() {
        let anchor = Utc.with_ymd_and_hms(2026, 5, 22, 12, 7, 0).unwrap();
        let nf = next_fire_after("*/15 * * * *", anchor).unwrap().unwrap();
        assert_eq!(nf, Utc.with_ymd_and_hms(2026, 5, 22, 12, 15, 0).unwrap());
    }

    #[test]
    fn parses_6_field() {
        // 6-field: every minute at second 30.
        let s = parse_expression("30 * * * * *").unwrap();
        let anchor = Utc.with_ymd_and_hms(2026, 5, 22, 12, 0, 0).unwrap();
        let nf = s.after(&anchor).next().unwrap();
        assert_eq!(nf, Utc.with_ymd_and_hms(2026, 5, 22, 12, 0, 30).unwrap());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_expression("not a cron expr").is_err());
        assert!(parse_expression("").is_err());
        assert!(parse_expression("1 2 3").is_err());
        assert!(parse_expression("a b c d e").is_err());
    }
}
