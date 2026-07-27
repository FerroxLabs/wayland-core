//! The trigger vocabulary: WHEN a job runs, separately from WHAT it does.
//!
//! Phase 24 plan 24-02, Task 2.
//!
//! # Why this is not a variant of `Target`
//!
//! A target says what to do; a trigger says when. Conflating them is what
//! makes a schedule store hard to extend: every new firing condition would
//! otherwise multiply against every action, and the on-disk discriminator
//! that `Target` already carries a compatibility shim for would have to grow
//! a second meaning. They are two independent axes and they are stored as
//! two independent fields.
//!
//! # Every type carries a bound, and the bound is not optional
//!
//! An unattended background runtime with an unbounded trigger is how a
//! machine gets consumed while nobody is watching. Each variant therefore
//! resolves to a [`TriggerBound`] with:
//!
//! - a **minimum interval** — the maximum firing RATE, expressed as the
//!   smallest gap two fires may have;
//! - a **maximum in-flight count** — how many fires of this one job may be
//!   outstanding at once;
//! - an optional **terminal deadline** — after which the trigger is spent and
//!   is never evaluated again.
//!
//! The default for each variant is stated in
//! `24-02-AUTOMATION-CONTRACT.md` and produced by
//! [`Trigger::default_bound`], not buried in a constant at a call site. A
//! persisted job may carry its own bound, and a bound WIDER than the
//! variant's default is refused rather than accepted — see
//! [`TriggerBound::clamp_to`].

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::CronError;

/// The smallest interval any trigger may be given, whatever it asks for.
///
/// One second. Below this the tick loop cannot distinguish two fires anyway,
/// and a job asking for less is asking for a spin loop.
pub const FLOOR_INTERVAL_SECS: u64 = 1;

/// The largest in-flight count any trigger may be given.
///
/// A single scheduled job with more than this outstanding is not a schedule,
/// it is a fork bomb with a cron expression.
pub const CEILING_IN_FLIGHT: u32 = 16;

/// What bounds one trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerBound {
    /// Smallest permitted gap between two fires, in seconds. This is the
    /// maximum firing rate expressed the way the tick loop can enforce it.
    pub min_interval_secs: u64,
    /// How many fires of this job may be outstanding at once.
    pub max_in_flight: u32,
    /// Instant after which this trigger is terminal and is never evaluated
    /// again. `None` means the trigger has no natural end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<DateTime<Utc>>,
}

impl TriggerBound {
    pub fn new(min_interval_secs: u64, max_in_flight: u32) -> Self {
        Self {
            min_interval_secs,
            max_in_flight,
            deadline: None,
        }
    }

    pub fn with_deadline(mut self, at: DateTime<Utc>) -> Self {
        self.deadline = Some(at);
        self
    }

    /// Narrow this bound so it can never be wider than `default`.
    ///
    /// The direction is deliberate and one-way. A persisted job may ask to be
    /// bounded MORE tightly than its variant's default and get it; a job
    /// asking to be bounded more loosely — by a hand-edited `jobs.json`, or by
    /// a Desktop write, or by an operator who mistyped — is narrowed back.
    /// A bound a caller can widen is not a bound.
    pub fn clamp_to(self, default: &TriggerBound) -> Self {
        Self {
            min_interval_secs: self
                .min_interval_secs
                .max(default.min_interval_secs)
                .max(FLOOR_INTERVAL_SECS),
            max_in_flight: self
                .max_in_flight
                .min(default.max_in_flight)
                .min(CEILING_IN_FLIGHT)
                .max(1),
            deadline: match (self.deadline, default.deadline) {
                // The EARLIER deadline wins: a job may end sooner than its
                // variant requires, never later.
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
        }
    }

    /// Whether `now` is past this bound's terminal deadline.
    pub fn is_spent(&self, now: DateTime<Utc>) -> bool {
        matches!(self.deadline, Some(d) if now > d)
    }
}

/// When a job fires.
///
/// Serialized with `kind` as the discriminator, matching the convention
/// [`crate::job::Target`] already established on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trigger {
    /// Fires exactly once, at a stated instant, and is then spent forever.
    Once { at: DateTime<Utc> },

    /// Fires every `every_secs` from its anchor.
    Interval { every_secs: u64 },

    /// Fires on a cron expression. The historical shape — every job written
    /// before this vocabulary existed resolves to this variant.
    Cron { expression: String },

    /// Fires when a named local event is published.
    ///
    /// The event is produced INSIDE the runtime, so the bound here is about
    /// a runaway publisher rather than about a hostile one.
    Event { topic: String },

    /// Fires when an inbound HTTP request arrives on a path.
    ///
    /// `require_auth` defaults to `true` and is the whole reason this variant
    /// is not just `Event` with a different name: the input is REMOTE, so an
    /// unauthenticated caller must not be able to cause work (threat
    /// T-24-02-02). It is a stored field rather than a global setting so a
    /// job that deliberately opens a public endpoint has to say so in its own
    /// record, where an operator reading `cron list` can see it.
    Webhook {
        path: String,
        #[serde(default = "default_true")]
        require_auth: bool,
    },

    /// Fires after polling a remote resource, when the response says work is
    /// due.
    ///
    /// A remote the runtime does not control decides whether work runs, so
    /// the poll rate is bounded harder than the local variants.
    Poll { url: String, every_secs: u64 },

    /// A commitment with a deadline and a heartbeat.
    ///
    /// The heartbeat is what makes a stalled commitment OBSERVABLE rather
    /// than merely late, and the deadline is what stops it retrying forever.
    Commitment {
        deadline: DateTime<Utc>,
        heartbeat_secs: u64,
    },
}

fn default_true() -> bool {
    true
}

impl Trigger {
    /// The stable name used in the contract document, in `cron list` output
    /// and in the history records.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Once { .. } => "once",
            Self::Interval { .. } => "interval",
            Self::Cron { .. } => "cron",
            Self::Event { .. } => "event",
            Self::Webhook { .. } => "webhook",
            Self::Poll { .. } => "poll",
            Self::Commitment { .. } => "commitment",
        }
    }

    /// Every variant name, in contract order. The trigger-matrix test walks
    /// this so a new variant cannot be added without a case.
    pub const KINDS: &'static [&'static str] = &[
        "once",
        "interval",
        "cron",
        "event",
        "webhook",
        "poll",
        "commitment",
    ];

    /// This variant's default bound.
    ///
    /// Stated per variant rather than shared, because the reason each one is
    /// bounded differs: a local timer is bounded against a spin, a remote
    /// input is bounded against an unauthenticated flood, and a commitment is
    /// bounded by the thing it committed to.
    pub fn default_bound(&self) -> TriggerBound {
        match self {
            // Deliberately NO deadline, and the reason is a measured red.
            //
            // Setting the deadline to the fire instant makes the trigger spent
            // at exactly the moment it becomes due, so a one-shot could never
            // fire at all — the tick evaluates spentness against NOW, and NOW
            // is always at or after the instant by the time the job is
            // selected. A one-shot's terminal property is structural instead:
            // once it has fired, the anchor moves past `at` and
            // [`Trigger::next_after`] returns `None` forever after.
            Self::Once { .. } => TriggerBound::new(1, 1),
            // A minute floor: the tick is 30s, so anything faster cannot be
            // honoured evenly and would simply fire on every tick.
            Self::Interval { every_secs } => TriggerBound::new((*every_secs).max(60), 1),
            Self::Cron { .. } => TriggerBound::new(60, 1),
            // A local publisher can be fast; two in flight is enough to
            // absorb a burst without letting a runaway publisher stack up.
            Self::Event { .. } => TriggerBound::new(1, 2),
            // Remote and unattended: the tightest rate of the seven, because
            // this is the only variant a party outside the machine can
            // trigger at will.
            Self::Webhook { .. } => TriggerBound::new(5, 1),
            // A remote decides whether work runs, so the poll itself is
            // floored at five minutes regardless of what the job asked for.
            Self::Poll { every_secs, .. } => TriggerBound::new((*every_secs).max(300), 1),
            // Bounded by the commitment: past the deadline it is terminal, and
            // the heartbeat sets how often it may report.
            Self::Commitment {
                deadline,
                heartbeat_secs,
            } => TriggerBound::new((*heartbeat_secs).max(1), 1).with_deadline(*deadline),
        }
    }

    /// Validate the variant's own parameters.
    ///
    /// Separate from the bound: a bound narrows a valid trigger, it does not
    /// rescue an invalid one. An empty webhook path or a zero heartbeat is
    /// nonsense whatever the bound says.
    pub fn validate(&self) -> crate::Result<()> {
        match self {
            Self::Once { .. } => Ok(()),
            Self::Interval { every_secs } => {
                if *every_secs == 0 {
                    return Err(CronError::InvalidExpression(
                        "interval trigger needs a non-zero period".into(),
                    ));
                }
                Ok(())
            }
            Self::Cron { expression } => crate::schedule::parse_expression(expression).map(|_| ()),
            Self::Event { topic } => {
                if topic.trim().is_empty() {
                    return Err(CronError::InvalidExpression(
                        "event trigger needs a topic".into(),
                    ));
                }
                Ok(())
            }
            Self::Webhook { path, .. } => {
                if !path.starts_with('/') || path.len() < 2 {
                    return Err(CronError::InvalidExpression(format!(
                        "webhook trigger needs an absolute path, got {path:?}"
                    )));
                }
                Ok(())
            }
            Self::Poll { url, every_secs } => {
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(CronError::InvalidExpression(format!(
                        "poll trigger needs an http(s) url, got {url:?}"
                    )));
                }
                if *every_secs == 0 {
                    return Err(CronError::InvalidExpression(
                        "poll trigger needs a non-zero period".into(),
                    ));
                }
                Ok(())
            }
            Self::Commitment { heartbeat_secs, .. } => {
                if *heartbeat_secs == 0 {
                    return Err(CronError::InvalidExpression(
                        "commitment trigger needs a non-zero heartbeat".into(),
                    ));
                }
                Ok(())
            }
        }
    }

    /// The next instant this trigger is due strictly after `after`, under
    /// `bound`.
    ///
    /// `Ok(None)` means the trigger has no further occurrence — spent, past
    /// its deadline, or externally driven and therefore not predictable from
    /// the clock alone.
    pub fn next_after(
        &self,
        after: DateTime<Utc>,
        bound: &TriggerBound,
    ) -> crate::Result<Option<DateTime<Utc>>> {
        if bound.is_spent(after) {
            return Ok(None);
        }
        let raw = match self {
            Self::Once { at } => {
                if *at > after {
                    Some(*at)
                } else {
                    None
                }
            }
            Self::Interval { every_secs } => {
                Some(after + Duration::seconds(*every_secs.max(&1) as i64))
            }
            Self::Cron { expression } => crate::schedule::next_fire_after(expression, after)?,
            // Externally driven: the clock cannot say when a publisher, a
            // caller or a remote will next make it due. Reporting a
            // predicted instant for these would be a fabricated number in an
            // operator-facing field.
            Self::Event { .. } | Self::Webhook { .. } => None,
            Self::Poll { every_secs, .. } => {
                Some(after + Duration::seconds(*every_secs.max(&1) as i64))
            }
            Self::Commitment { heartbeat_secs, .. } => {
                Some(after + Duration::seconds(*heartbeat_secs.max(&1) as i64))
            }
        };
        // The bound is applied to the RESULT, not only to the parameters: a
        // job whose stored period was narrowed must actually fire at the
        // narrowed rate rather than at the rate it asked for.
        let floored = raw.map(|t| {
            let earliest = after + Duration::seconds(bound.min_interval_secs.max(1) as i64);
            t.max(earliest)
        });
        Ok(match floored {
            Some(t) if bound.is_spent(t) => None,
            other => other,
        })
    }

    /// Whether the runtime's clock alone can say when this fires. `false` for
    /// the externally driven variants, whose next fire is genuinely unknown.
    pub fn is_clock_driven(&self) -> bool {
        !matches!(self, Self::Event { .. } | Self::Webhook { .. })
    }
}

/// The observable state of a commitment's heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatState {
    /// A beat arrived inside its interval.
    Alive,
    /// No beat inside the interval. Recorded and observable — a commitment
    /// that went quiet must be distinguishable from one that is merely
    /// between beats.
    Missed,
    /// Past the deadline. Terminal: it does not retry, and it does not
    /// resume if a beat arrives afterwards.
    Expired,
}

/// Classify a commitment from its last beat.
///
/// `Expired` is checked FIRST and wins outright. A commitment that beat once
/// after its deadline has still failed its commitment, and reporting it alive
/// because the most recent beat was recent would hide exactly the case the
/// deadline exists to surface.
pub fn heartbeat_state(
    trigger: &Trigger,
    last_beat: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<HeartbeatState> {
    let Trigger::Commitment {
        deadline,
        heartbeat_secs,
    } = trigger
    else {
        return None;
    };
    if now > *deadline {
        return Some(HeartbeatState::Expired);
    }
    let interval = Duration::seconds((*heartbeat_secs).max(1) as i64);
    match last_beat {
        Some(b) if now - b <= interval => Some(HeartbeatState::Alive),
        _ => Some(HeartbeatState::Missed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 27, 12, 0, 0).unwrap()
    }

    #[test]
    fn every_kind_name_is_covered_by_the_constant() {
        // A new variant that forgets to extend KINDS would make the matrix
        // test silently skip it.
        let all = [
            Trigger::Once { at: t0() },
            Trigger::Interval { every_secs: 60 },
            Trigger::Cron {
                expression: "0 9 * * *".into(),
            },
            Trigger::Event { topic: "t".into() },
            Trigger::Webhook {
                path: "/hook".into(),
                require_auth: true,
            },
            Trigger::Poll {
                url: "https://x.test".into(),
                every_secs: 300,
            },
            Trigger::Commitment {
                deadline: t0(),
                heartbeat_secs: 60,
            },
        ];
        assert_eq!(all.len(), Trigger::KINDS.len());
        for t in &all {
            assert!(
                Trigger::KINDS.contains(&t.kind()),
                "{} missing from KINDS",
                t.kind()
            );
        }
    }

    #[test]
    fn a_bound_cannot_be_widened_by_a_stored_value() {
        let default = Trigger::Webhook {
            path: "/h".into(),
            require_auth: true,
        }
        .default_bound();
        // A hand-edited record asking for a one-second rate and 100 in flight.
        let hostile = TriggerBound::new(1, 100);
        let clamped = hostile.clamp_to(&default);
        assert_eq!(clamped.min_interval_secs, default.min_interval_secs);
        assert_eq!(clamped.max_in_flight, default.max_in_flight);
    }

    #[test]
    fn a_bound_can_be_narrowed_by_a_stored_value() {
        let default = Trigger::Event { topic: "t".into() }.default_bound();
        let cautious = TriggerBound::new(3600, 1);
        let clamped = cautious.clamp_to(&default);
        assert_eq!(clamped.min_interval_secs, 3600);
        assert_eq!(clamped.max_in_flight, 1);
    }

    #[test]
    fn the_earlier_deadline_wins() {
        let default = TriggerBound::new(1, 1).with_deadline(t0() + Duration::hours(10));
        let asked = TriggerBound::new(1, 1).with_deadline(t0() + Duration::hours(1));
        assert_eq!(
            asked.clamp_to(&default).deadline,
            Some(t0() + Duration::hours(1))
        );
        let later = TriggerBound::new(1, 1).with_deadline(t0() + Duration::hours(100));
        assert_eq!(
            later.clamp_to(&default).deadline,
            Some(t0() + Duration::hours(10)),
            "a job must not be able to extend its own deadline"
        );
    }

    #[test]
    fn a_one_shot_is_spent_once_its_anchor_passes_its_instant() {
        let t = Trigger::Once {
            at: t0() + Duration::minutes(5),
        };
        let b = t.default_bound();
        assert_eq!(
            t.next_after(t0(), &b).unwrap(),
            Some(t0() + Duration::minutes(5))
        );
        assert_eq!(
            t.next_after(t0() + Duration::minutes(6), &b).unwrap(),
            None,
            "a one-shot must not re-arm once its anchor has passed its instant"
        );
    }

    #[test]
    fn a_one_shot_carries_no_terminal_deadline() {
        // Measured red: a deadline equal to the fire instant makes the trigger
        // spent at exactly the moment it becomes due, because the runner
        // evaluates spentness against NOW and NOW is always at or past the
        // instant by the time the job is selected. The one-shot then never
        // fires at all.
        let t = Trigger::Once {
            at: t0() + Duration::minutes(5),
        };
        assert_eq!(t.default_bound().deadline, None);
        assert!(!t.default_bound().is_spent(t0() + Duration::days(400)));
    }

    #[test]
    fn an_externally_driven_trigger_predicts_nothing() {
        for t in [
            Trigger::Event { topic: "x".into() },
            Trigger::Webhook {
                path: "/x".into(),
                require_auth: true,
            },
        ] {
            let b = t.default_bound();
            assert_eq!(
                t.next_after(t0(), &b).unwrap(),
                None,
                "{} must not fabricate a next-fire instant",
                t.kind()
            );
            assert!(!t.is_clock_driven());
        }
    }

    #[test]
    fn a_narrowed_rate_is_applied_to_the_result_not_only_the_parameters() {
        let t = Trigger::Interval { every_secs: 5 };
        // The variant default already floors an interval at 60s.
        let b = t.default_bound();
        let next = t.next_after(t0(), &b).unwrap().unwrap();
        assert!(
            next >= t0() + Duration::seconds(60),
            "a job asking for 5s must actually fire no faster than its bound: {next}"
        );
    }

    #[test]
    fn a_webhook_defaults_to_requiring_authentication() {
        // Absence of the field in a persisted record must NOT read as "open".
        let t: Trigger = serde_json::from_str(r#"{"kind":"webhook","path":"/h"}"#).unwrap();
        match t {
            Trigger::Webhook { require_auth, .. } => assert!(
                require_auth,
                "an unspecified webhook must default to authenticated, not open"
            ),
            other => panic!("expected a webhook, got {other:?}"),
        }
    }

    #[test]
    fn expiry_beats_a_recent_beat() {
        let t = Trigger::Commitment {
            deadline: t0(),
            heartbeat_secs: 60,
        };
        let after = t0() + Duration::minutes(1);
        assert_eq!(
            heartbeat_state(&t, Some(after), after),
            Some(HeartbeatState::Expired),
            "a beat after the deadline must not resurrect the commitment"
        );
    }

    #[test]
    fn a_missed_beat_is_its_own_state() {
        let t = Trigger::Commitment {
            deadline: t0() + Duration::hours(5),
            heartbeat_secs: 60,
        };
        assert_eq!(
            heartbeat_state(&t, Some(t0() - Duration::hours(1)), t0()),
            Some(HeartbeatState::Missed)
        );
        assert_eq!(
            heartbeat_state(&t, Some(t0() - Duration::seconds(30)), t0()),
            Some(HeartbeatState::Alive)
        );
        assert_eq!(
            heartbeat_state(&t, None, t0()),
            Some(HeartbeatState::Missed),
            "a commitment that never beat is missed, not alive"
        );
    }

    #[test]
    fn invalid_parameters_are_refused_regardless_of_bound() {
        assert!(
            Trigger::Webhook {
                path: "relative".into(),
                require_auth: true
            }
            .validate()
            .is_err()
        );
        assert!(
            Trigger::Poll {
                url: "ftp://x".into(),
                every_secs: 300
            }
            .validate()
            .is_err()
        );
        assert!(
            Trigger::Commitment {
                deadline: t0(),
                heartbeat_secs: 0
            }
            .validate()
            .is_err()
        );
        assert!(Trigger::Event { topic: "  ".into() }.validate().is_err());
    }
}
