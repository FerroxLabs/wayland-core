//! Job + Target shapes for `wcore-cron`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

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
    Channel { channel_name: String, text: String },
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
            TargetRepr::Channel { channel_name, text } => Target::Channel { channel_name, text },
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
    NoSink,
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
        })
    }

    /// Compute the next-fire time strictly after `after`. Returns
    /// `Ok(None)` if the cron schedule has no future occurrence (e.g. a
    /// specific past timestamp).
    pub fn next_fire_after(&self, after: DateTime<Utc>) -> crate::Result<Option<DateTime<Utc>>> {
        crate::schedule::next_fire_after(&self.expression, after)
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
        };
        let s = serde_json::to_string(&t).unwrap();
        let back: Target = serde_json::from_str(&s).unwrap();
        assert_eq!(t, back);
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
