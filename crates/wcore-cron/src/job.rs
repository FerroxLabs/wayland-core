//! Job + Target shapes for `wcore-cron`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::retry::{RetryPolicy, RetryState};
use crate::trigger::{Trigger, TriggerBound};

/// What a cron job does when it fires.
///
/// Canonical on-disk form uses `kind` as the discriminator. The Desktop app
/// historically writes `type` instead (see `jobs.json` writer in the Electron
/// shell). Serde does not support `#[serde(alias)]` on the `tag` field of an
/// internally-tagged enum, so the custom `Deserialize` impl below pre-renames
/// `type` → `kind` when `kind` is absent. Serialization is unchanged (derived)
/// and continues to emit `kind`, which keeps engine-authored writes canonical.
/// Mirrors the v0.8.2 `schedule`/`expression` sibling fix on [`CronJob`].
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// Run a slash command (e.g. "/memory show working").
    Slash { command: String },
    /// Send a message through a registered channel.
    ///
    /// # `conversation_id` — F-ML-5, found live on 2026-07-30
    ///
    /// This variant carried only `{ channel_name, text }`, and the dispatcher
    /// (`wcore-agent/src/cron.rs`) passed the **channel name** as the outgoing
    /// message's `conversation_id` for want of anything better. So every
    /// scheduled channel delivery addressed a conversation named after the
    /// channel, and could only arrive where those two strings happened to be
    /// equal. Measured against a real homeserver:
    ///
    /// ```text
    /// PUT /_matrix/client/v3/rooms/mxlive/send/m.room.message/cron:…
    /// 403 M_FORBIDDEN "User @… not in room mxlive"     <- `mxlive` is the CHANNEL NAME
    /// ```
    ///
    /// It is not Matrix-specific. Slack (`lib.rs:416`), WhatsApp (`:238`) and
    /// SMS (`:250`) each fall back to a configured default destination — but
    /// only when `conversation_id` **is empty**, which cron never produced. So
    /// no adapter's configured default was reachable from a scheduled delivery.
    ///
    /// `None` now means "the adapter's own default", which is what those three
    /// fallbacks were written for and what an adapter with no default should
    /// refuse rather than guess at.
    Channel {
        channel_name: String,
        text: String,
        /// Destination conversation / room / chat id. `None` defers to the
        /// adapter's configured default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },
    /// Invoke a skill by name (engine routes it).
    Skill {
        name: String,
        #[serde(default)]
        args: serde_json::Value,
    },
}

/// Mirror of [`Target`] used solely as the derived-Deserialize target. Kept
/// private so the public API stays a single `Target` type. The custom
/// `Deserialize` impl below routes through this after normalising the
/// discriminator field name.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TargetRepr {
    Slash {
        command: String,
    },
    Channel {
        channel_name: String,
        text: String,
        #[serde(default)]
        conversation_id: Option<String>,
    },
    Skill {
        name: String,
        #[serde(default)]
        args: serde_json::Value,
    },
}

impl From<TargetRepr> for Target {
    fn from(r: TargetRepr) -> Self {
        match r {
            TargetRepr::Slash { command } => Target::Slash { command },
            TargetRepr::Channel {
                channel_name,
                text,
                conversation_id,
            } => Target::Channel {
                channel_name,
                text,
                conversation_id,
            },
            TargetRepr::Skill { name, args } => Target::Skill { name, args },
        }
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;

        // Deserialize into a generic JSON value first so we can normalise the
        // discriminator field before handing off to the derived impl.
        let mut value = serde_json::Value::deserialize(deserializer)?;
        if let serde_json::Value::Object(map) = &mut value
            && !map.contains_key("kind")
            && let Some(v) = map.remove("type")
        {
            map.insert("kind".to_string(), v);
        }
        let repr: TargetRepr = serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(repr.into())
    }
}

/// Outcome of the most recent cron fire attempt.
///
/// Persisted in the JSON store so `cron status` can surface diagnostic
/// info without grepping engine logs. `serde(default)` means old job
/// records with no field at all deserialise as `None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CronFireOutcome {
    /// Dispatch returned `Ok` within the given wall-clock duration.
    Success {
        /// How long the dispatch took, in milliseconds.
        duration_ms: u64,
    },
    /// Dispatch returned `Err` — the `message` is the `Display` of the
    /// returned `CronError`.
    Error { message: String },
    /// The handler had no sink for this target type (e.g. channel fires
    /// when no ChannelManager is wired). `last_fired` is NOT advanced
    /// when this outcome is recorded.
    ///
    /// Superseded by [`CronFireOutcome::Staged`] / [`crate::CronError::NoDispatcher`]
    /// for the no-live-dispatcher case (rank 3). Kept as a variant because
    /// removing it would be a breaking on-disk/API change.
    NoSink,
    /// The fire was recorded/staged but no live dispatcher was available
    /// to actually execute it (e.g. the cross-session slash dispatcher, or
    /// a skill/channel sink absent in this process). Distinct from
    /// [`CronFireOutcome::NoSink`] and from success: `last_fired` IS
    /// advanced (so the job does not hot-loop re-firing every tick within
    /// its window) but the outcome is NOT recorded as a success.
    Staged,
    /// The fire was SELECTED while this process owned the schedule, and
    /// ownership was lost before the dispatch could be performed — a
    /// gateway entering drain while a tick was in flight.
    ///
    /// `last_fired` is NOT advanced, because the job genuinely did not run
    /// and the incoming owner must still fire it. Recorded rather than
    /// silent: a fire that vanished between selection and dispatch with no
    /// record is indistinguishable from a fire that never came due, and
    /// that ambiguity is what makes a handover look like a lost job.
    Abandoned { reason: String },
    /// The retry cap was reached and this job has STOPPED trying.
    ///
    /// A named terminal state rather than an absence of further records:
    /// a job that gave up an hour ago and a job that is between attempts are
    /// otherwise indistinguishable to an operator reading `cron status`.
    /// `last_fired` IS advanced, so an exhausted job does not re-select on
    /// every tick.
    GaveUp { attempts: u32, message: String },
}

/// Snapshot of a single cron fire, written to the ring-buffer history
/// file (`$WAYLAND_HOME/cron/history.jsonl`) by the runner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronFireRecord {
    pub job_id: String,
    pub fired_at: DateTime<Utc>,
    pub outcome: CronFireOutcome,
}

/// A scheduled job persisted in the cron store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronJob {
    /// UUID v4 stringified.
    pub id: String,
    /// Cron expression — e.g. "0 9 * * *". Parsed via [`crate::schedule`].
    /// Serde aliases accept the Desktop app's historical field name
    /// (`schedule`) so engine-side deserialization succeeds on jobs the app
    /// has already written. The canonical name stays `expression` — the
    /// W6-L cron-bridge migration (jobs.json canonical form) handles full
    /// re-serialization on next write.
    #[serde(alias = "schedule", alias = "cron", alias = "expr")]
    pub expression: String,
    /// Action to take when due.
    pub target: Target,
    /// When false, the runner skips the job.
    pub enabled: bool,
    /// Wall-clock time the job was created. Used as the cron-anchor
    /// baseline for the first fire.
    pub created_at: DateTime<Utc>,
    /// Wall-clock time of the most recent successful dispatch. None on a
    /// brand-new job.
    pub last_fired: Option<DateTime<Utc>>,
    /// Outcome of the most recent fire attempt. Populated by the runner
    /// after every dispatch (success or error). None until the first fire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<CronFireOutcome>,

    // ---- Phase 24 plan 24-02: the trigger vocabulary ----
    //
    // All three are `#[serde(default)]` and skipped when absent, so every job
    // written before this vocabulary existed still loads byte-for-byte
    // unchanged and every job written after it stays readable by an older
    // reader. `expression` remains the canonical cron field; a job with no
    // `trigger` resolves to [`Trigger::Cron`] over it, which is exactly what
    // it did before.
    /// WHEN this job fires, separately from what it does. Absent on a job
    /// authored before the vocabulary existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
    /// This job's own bound. NARROWER than its trigger's default or it is
    /// clamped back — see [`TriggerBound::clamp_to`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<TriggerBound>,
    /// How this job retries a failed dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    /// Retry bookkeeping for the current failure run.
    #[serde(default)]
    pub retry_state: RetryState,
    /// Last heartbeat observed for a commitment trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat: Option<DateTime<Utc>>,
}

impl CronJob {
    /// Construct a new enabled job with a fresh v4 UUID and `now`
    /// timestamp. Returns an error if the cron expression doesn't parse.
    pub fn new(expression: impl Into<String>, target: Target) -> crate::Result<Self> {
        let expression = expression.into();
        // Validate up-front; we don't want a job persisted with an
        // expression that will permanently fail to schedule.
        crate::schedule::parse_expression(&expression)?;
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            expression,
            target,
            enabled: true,
            created_at: Utc::now(),
            last_fired: None,
            last_result: None,
            trigger: None,
            bound: None,
            retry: None,
            retry_state: RetryState::default(),
            last_heartbeat: None,
        })
    }

    /// Construct a job on any trigger in the vocabulary.
    ///
    /// `expression` is filled with the trigger's rendered descriptor so the
    /// field an operator already reads keeps meaning "when this fires" for
    /// every variant rather than only for cron. The trigger's own parameters
    /// are validated; the descriptor is display, not authority.
    pub fn with_trigger(trigger: Trigger, target: Target) -> crate::Result<Self> {
        trigger.validate()?;
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            expression: render_trigger(&trigger),
            target,
            enabled: true,
            created_at: Utc::now(),
            last_fired: None,
            last_result: None,
            trigger: Some(trigger),
            bound: None,
            retry: None,
            retry_state: RetryState::default(),
            last_heartbeat: None,
        })
    }

    /// The trigger this job actually fires on.
    ///
    /// A job with no stored trigger resolves to [`Trigger::Cron`] over its
    /// `expression`, which is precisely the behaviour every job had before
    /// the vocabulary existed.
    pub fn effective_trigger(&self) -> Trigger {
        self.trigger.clone().unwrap_or_else(|| Trigger::Cron {
            expression: self.expression.clone(),
        })
    }

    /// The bound actually applied, after clamping any stored bound to the
    /// trigger's default. Never wider than the default.
    pub fn effective_bound(&self) -> TriggerBound {
        let default = self.effective_trigger().default_bound();
        match self.bound.clone() {
            Some(b) => b.clamp_to(&default),
            None => default,
        }
    }

    /// The retry policy actually applied, after clamping.
    pub fn effective_retry(&self) -> RetryPolicy {
        self.retry.clone().unwrap_or_default().clamped()
    }

    /// The observable heartbeat state, for a commitment trigger only.
    pub fn heartbeat_state(&self, now: DateTime<Utc>) -> Option<crate::trigger::HeartbeatState> {
        crate::trigger::heartbeat_state(&self.effective_trigger(), self.last_heartbeat, now)
    }

    /// Compute the next-fire time strictly after `after`, under this job's
    /// trigger and bound.
    ///
    /// `Ok(None)` when there is no further occurrence: a spent one-shot, a
    /// commitment past its deadline, a cron expression pinned to the past, or
    /// an externally driven trigger whose next fire the clock cannot predict.
    pub fn next_fire_after(&self, after: DateTime<Utc>) -> crate::Result<Option<DateTime<Utc>>> {
        self.effective_trigger()
            .next_after(after, &self.effective_bound())
    }
}

/// Render a trigger as the human-readable descriptor stored in
/// `CronJob::expression`.
///
/// A cron trigger renders as its plain expression, so nothing about the
/// historical shape changes on disk. Every other variant renders with a
/// leading `@` so it is unmistakably not a cron expression to any reader —
/// including one that tries to parse it.
pub fn render_trigger(t: &Trigger) -> String {
    match t {
        Trigger::Cron { expression } => expression.clone(),
        Trigger::Once { at } => format!("@once {}", at.to_rfc3339()),
        Trigger::Interval { every_secs } => format!("@every {every_secs}s"),
        Trigger::Event { topic } => format!("@event {topic}"),
        Trigger::Webhook { path, require_auth } => {
            let auth = if *require_auth { "auth" } else { "OPEN" };
            format!("@webhook {path} ({auth})")
        }
        Trigger::Poll { url, every_secs } => format!("@poll {url} every {every_secs}s"),
        Trigger::Commitment {
            deadline,
            heartbeat_secs,
        } => format!(
            "@commit by {} heartbeat {heartbeat_secs}s",
            deadline.to_rfc3339()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_round_trips_slash() {
        let t = Target::Slash {
            command: "/memory show".into(),
        };
        let s = serde_json::to_string(&t).unwrap();
        let back: Target = serde_json::from_str(&s).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn target_round_trips_channel() {
        let t = Target::Channel {
            channel_name: "team-slack".into(),
            text: "status check".into(),
            conversation_id: None,
        };
        let s = serde_json::to_string(&t).unwrap();
        let back: Target = serde_json::from_str(&s).unwrap();
        assert_eq!(t, back);
    }

    /// F-ML-5 back-compat and forward-compat, in one test.
    ///
    /// Every job already on disk was written WITHOUT `conversation_id`. If it
    /// failed to load, the fix would silently disable every existing schedule,
    /// which is a worse outcome than the defect. And a job that DOES carry one
    /// must round-trip it, or the flag is decorative.
    #[test]
    fn a_channel_target_written_before_conversation_id_existed_still_loads() {
        let legacy = r#"{"kind":"channel","channel_name":"team-slack","text":"status check"}"#;
        let back: Target = serde_json::from_str(legacy).expect("legacy records must still load");
        assert_eq!(
            back,
            Target::Channel {
                channel_name: "team-slack".into(),
                text: "status check".into(),
                conversation_id: None,
            }
        );
        // …and a legacy record must not GAIN the field when rewritten, so a
        // downgrade reads what it wrote.
        assert!(
            !serde_json::to_string(&back)
                .unwrap()
                .contains("conversation_id"),
            "an absent destination must not be serialized as null"
        );

        // Known-positive in the same test: a destination that IS set survives
        // the round trip. Without this the assertion above passes on a field
        // that is never written under any circumstances.
        let addressed = Target::Channel {
            channel_name: "mxlive".into(),
            text: "status check".into(),
            conversation_id: Some("!kntRqkQCkPjhPvMMvf:matrix.org".into()),
        };
        let s = serde_json::to_string(&addressed).unwrap();
        assert!(s.contains("!kntRqkQCkPjhPvMMvf:matrix.org"), "got {s}");
        assert_eq!(serde_json::from_str::<Target>(&s).unwrap(), addressed);
    }

    #[test]
    fn target_round_trips_skill() {
        let t = Target::Skill {
            name: "morning-brief".into(),
            args: serde_json::json!({"k": "v"}),
        };
        let s = serde_json::to_string(&t).unwrap();
        let back: Target = serde_json::from_str(&s).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn new_job_validates_expression() {
        let bad = CronJob::new(
            "not-a-cron-expression",
            Target::Slash {
                command: "/x".into(),
            },
        );
        assert!(bad.is_err());
    }

    #[test]
    fn new_job_round_trips() {
        let j = CronJob::new(
            "0 9 * * *",
            Target::Slash {
                command: "/memory show".into(),
            },
        )
        .unwrap();
        let s = serde_json::to_string(&j).unwrap();
        let back: CronJob = serde_json::from_str(&s).unwrap();
        assert_eq!(j, back);
    }

    /// Desktop app writes `"schedule"` where the engine struct expects
    /// `"expression"`. The serde alias must absorb that field name so
    /// deserialization succeeds without a `missing field 'expression'` error.
    #[test]
    fn schedule_alias_deserializes_desktop_app_field_name() {
        let json = r#"{
            "id": "aaaaaaaa-0000-0000-0000-000000000001",
            "schedule": "0 9 * * *",
            "target": {"kind": "slash", "command": "/brief"},
            "enabled": true,
            "created_at": "2026-01-01T00:00:00Z",
            "last_fired": null
        }"#;
        let job: CronJob = serde_json::from_str(json)
            .expect("CronJob must deserialise when 'schedule' is used as the expression field");
        assert_eq!(job.expression, "0 9 * * *");
    }

    /// Verify the `cron` and `expr` aliases work the same way.
    #[test]
    fn cron_and_expr_aliases_deserialize() {
        for field in &["cron", "expr"] {
            let json = format!(
                r#"{{
                    "id": "aaaaaaaa-0000-0000-0000-000000000002",
                    "{field}": "*/5 * * * *",
                    "target": {{"kind": "slash", "command": "/ping"}},
                    "enabled": true,
                    "created_at": "2026-01-01T00:00:00Z",
                    "last_fired": null
                }}"#
            );
            let job: CronJob = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("alias '{field}' failed: {e}"));
            assert_eq!(job.expression, "*/5 * * * *");
        }
    }

    /// Desktop app writes the cron action discriminator as `"type"` in
    /// `jobs.json`. The engine's `Target` enum uses serde `tag = "kind"`,
    /// which serde does NOT permit `alias` on. A custom `Deserialize` impl
    /// must accept `type` as a fallback discriminator so app-authored jobs
    /// don't silently disappear on engine load (sibling fix to the
    /// `schedule`/`expression` alias).
    #[test]
    fn desktop_app_type_field_deserializes_as_kind_slash() {
        let json = r#"{"type": "slash", "command": "/brief"}"#;
        let target: Target = serde_json::from_str(json)
            .expect("Target must deserialise when 'type' is the discriminator");
        assert_eq!(
            target,
            Target::Slash {
                command: "/brief".into()
            }
        );
    }

    #[test]
    fn desktop_app_type_field_deserializes_as_kind_channel() {
        let json = r#"{"type": "channel", "channel_name": "team-slack", "text": "hi"}"#;
        let target: Target = serde_json::from_str(json).expect("channel via 'type' must work");
        assert_eq!(
            target,
            Target::Channel {
                channel_name: "team-slack".into(),
                text: "hi".into(),
                conversation_id: None,
            }
        );
    }

    #[test]
    fn desktop_app_type_field_deserializes_as_kind_skill() {
        let json = r#"{"type": "skill", "name": "morning-brief", "args": {"k": "v"}}"#;
        let target: Target = serde_json::from_str(json).expect("skill via 'type' must work");
        assert_eq!(
            target,
            Target::Skill {
                name: "morning-brief".into(),
                args: serde_json::json!({"k": "v"}),
            }
        );
    }

    /// End-to-end: a full Desktop-app-shaped CronJob (with `schedule` AND
    /// `target.type`) must round-trip-deserialize.
    #[test]
    fn full_desktop_app_job_deserializes() {
        let json = r#"{
            "id": "aaaaaaaa-0000-0000-0000-000000000003",
            "schedule": "0 9 * * *",
            "target": {"type": "slash", "command": "/brief"},
            "enabled": true,
            "created_at": "2026-01-01T00:00:00Z",
            "last_fired": null
        }"#;
        let job: CronJob =
            serde_json::from_str(json).expect("Full Desktop-app-shaped CronJob must deserialise");
        assert_eq!(job.expression, "0 9 * * *");
        assert_eq!(
            job.target,
            Target::Slash {
                command: "/brief".into()
            }
        );
    }

    /// Canonical writes must still emit `kind` (not `type`). Guards against a
    /// regression where the custom Deserialize impl gets paired with a
    /// custom Serialize impl that breaks the on-disk format.
    #[test]
    fn target_serializes_with_kind_not_type() {
        let t = Target::Slash {
            command: "/x".into(),
        };
        let s = serde_json::to_string(&t).unwrap();
        assert!(s.contains("\"kind\""), "expected 'kind' in: {s}");
        assert!(!s.contains("\"type\""), "did not expect 'type' in: {s}");
    }
}
