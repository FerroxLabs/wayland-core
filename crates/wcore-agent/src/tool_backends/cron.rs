//! v0.9.0 Wave-1 B6 — `cronjob` tool backend wired to the existing
//! `wcore-cron` infrastructure.
//!
//! ## Reused infra
//!
//! The `wcore-cron` crate already ships a production scheduler:
//! [`wcore_cron::FileCronStore`] (JSON-file persisted at
//! `$WAYLAND_HOME/cron/jobs.json` with atomic tempfile+rename writes),
//! [`wcore_cron::CronRunner`] (30 s tokio tick loop + history JSONL),
//! and [`wcore_cron::JobHandler`] / [`crate::cron::EngineJobHandler`]
//! (dispatches `Target::Slash` / `Target::Channel` / `Target::Skill`
//! through the engine's session-resolved sinks). `bootstrap.rs:1850-1892`
//! spawns the runner with that wiring on every session boot.
//!
//! The gap that B6 closes is the **tool surface**: until this commit,
//! `cronjob_tools::CronJobTool` registered with
//! [`wcore_tools::cronjob_tools::NullCronScheduler`] so every model call
//! returned "no cron scheduler configured" even though the runner was
//! ticking happily in the background. This file is the bridge: it
//! adapts the tool's [`CronScheduler`] trait to operate on the same
//! [`FileCronStore`] the runner reads.
//!
//! ## One-shot caveat (documented v0.9.x followup)
//!
//! `wcore-cron` only models recurring schedules. The tool accepts
//! one-shot schedules (`"30m"`, `"2026-02-03T14:00:00"`) via
//! [`wcore_tools::cronjob_tools::ParsedSchedule::Once`]. We persist a
//! one-shot as a `M H D MO *` cron expression pinned to that specific
//! minute. The runner anchors against `created_at`, so the job fires
//! once when the moment passes; `last_fired` then advances to that
//! moment and the schedule's *next* occurrence is one **year** later
//! (the `cron` crate has no year field in 5-field shape). Practically:
//! one-shot jobs fire once and then sit dormant — operators can
//! `remove` them after. A real one-shot lifecycle (auto-delete after
//! fire, calendar-bounded expressions) is queued for v0.9.x as part of
//! the broader cron lifecycle pass.
//!
//! ## Tool→runner action plumbing
//!
//! - `prompt` + zero skills → [`wcore_cron::Target::Slash`] with the
//!   prompt as the command body. The slash sink at bootstrap is
//!   currently `None` (cross-session dispatcher pending), so the fire
//!   is logged via [`crate::cron::EngineJobHandler`]. This matches the
//!   existing F-013-deferred behaviour for slash arms.
//! - `prompt` + N skills → [`wcore_cron::Target::Skill`] of the first
//!   skill, args carrying the prompt and any remaining skills. The
//!   skill sink IS wired at bootstrap (F-013 fix), so these fires
//!   actually invoke the skill body via `SkillTool::execute`.
//! - `deliver = "<channel-name>"` (any action) →
//!   [`wcore_cron::Target::Channel`] with the prompt as the message
//!   text. Channel sink is wired (F-014 fix).
//!
//! ## Trigger semantics
//!
//! `trigger_job` clears `last_fired` so the next runner tick
//! (≤30 s) considers the job due regardless of its anchor. No new
//! dispatcher path is opened — the existing runner does the work. The
//! tool returns the updated `CronJob` immediately; the actual fire
//! lands inside one tick.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tracing::{debug, info, warn};

use wcore_cron::{CronError, CronStore, FileCronStore, Target as CronTarget};
use wcore_tools::cronjob_tools::{
    CreateJobSpec, CronJob as ToolCronJob, CronScheduler, ParsedSchedule, SchedulerError,
    UpdateJobSpec,
};

/// Adapter that exposes the existing [`FileCronStore`] (the same store
/// the `CronRunner` ticks against) as the tool's [`CronScheduler`].
///
/// One adapter per session; the underlying `FileCronStore` is cheap to
/// clone (Arc'd mutex internally) and shared with `bootstrap.rs`'s
/// runner so model-driven `create_job` / `pause` / etc. mutations land
/// in the same JSON file the runner reads on its next tick.
pub struct WaylandCronScheduler {
    store: Arc<dyn CronStore>,
}

impl WaylandCronScheduler {
    /// New scheduler over a shared store. Bootstrap clones its existing
    /// `Arc<FileCronStore>` into both the runner and this adapter so
    /// the tool and the tick loop see the same job set.
    pub fn new(store: Arc<dyn CronStore>) -> Self {
        Self { store }
    }
}

/// Resolve the tool backend. Returns `None` only when neither
/// `$WAYLAND_HOME` nor `$HOME` resolve (extremely rare; matches the
/// runner's existing graceful-skip path at `bootstrap.rs:1884-1891`).
///
/// Unlike most W1 backends this resolver does NOT key on an env var —
/// the cron store is purely local persistence, no external service,
/// no token. The `Option` shape stays for symmetry with the rest of
/// `tool_backends/*` and for the rare HOME-less environment case.
pub fn build_cron_backend() -> Option<Arc<dyn CronScheduler>> {
    match FileCronStore::from_default_path() {
        Ok(store) => {
            let store: Arc<dyn CronStore> = Arc::new(store);
            info!(
                target: "wcore_agent::tool_backends::cron",
                "cronjob: wiring WaylandCronScheduler over default FileCronStore"
            );
            Some(Arc::new(WaylandCronScheduler::new(store)))
        }
        Err(e) => {
            warn!(
                target: "wcore_agent::tool_backends::cron",
                error = %e,
                "cronjob: store path unresolvable — tool will stay hidden"
            );
            None
        }
    }
}

// ---------------------------------------------------------------------
// Tool ↔ wcore-cron model conversion
// ---------------------------------------------------------------------

/// Convert the tool's [`ParsedSchedule`] into a 5-field cron expression
/// the `cron` crate accepts. One-shot semantics caveat above.
fn schedule_to_cron_expr(schedule: &ParsedSchedule) -> Result<String, SchedulerError> {
    match schedule {
        ParsedSchedule::Cron { expr, .. } => Ok(expr.clone()),
        ParsedSchedule::Interval { minutes, .. } => {
            // Pick the most natural cron shape per granularity.
            // Sub-hour intervals → `*/N * * * *` (every N minutes).
            // Hour-multiple → `0 */H * * *` (every H hours on the hour).
            // Day-multiple → `0 0 */D * *` (every D days at midnight).
            // Other → fall back to `*/N * * * *` if N < 60 else error.
            let m = *minutes;
            if m == 0 {
                return Err(SchedulerError::Invalid(
                    "interval must be > 0 minutes".into(),
                ));
            }
            if m < 60 {
                Ok(format!("*/{m} * * * *"))
            } else if m % 60 == 0 {
                let hours = m / 60;
                if hours < 24 {
                    Ok(format!("0 */{hours} * * *"))
                } else if hours % 24 == 0 {
                    let days = hours / 24;
                    Ok(format!("0 0 */{days} * *"))
                } else {
                    // e.g. 25-hour interval has no clean cron shape;
                    // approximate with every-N-hours on the hour.
                    Ok(format!("0 */{hours} * * *"))
                }
            } else {
                // Awkward minute count > 60 with no hour divisor —
                // approximate as every-N-minutes, accepting cron will
                // refuse N > 59. Fall back to top-of-hour every hour.
                Ok("0 * * * *".to_string())
            }
        }
        ParsedSchedule::Once { run_at, .. } => {
            // Two flavours: "+30m" (relative, parsed by the tool layer
            // from a duration like "30m") or an ISO-8601 timestamp.
            if let Some(rest) = run_at.strip_prefix('+') {
                // Relative — add N minutes to now and pin a specific
                // wall-clock cron entry.
                let mins: i64 = rest
                    .trim_end_matches('m')
                    .parse()
                    .map_err(|_| SchedulerError::Invalid(format!("bad relative offset: {rest}")))?;
                let fire_at = Utc::now() + chrono::Duration::minutes(mins);
                Ok(format!(
                    "{} {} {} {} *",
                    fire_at.format("%M"),
                    fire_at.format("%H"),
                    fire_at.format("%d"),
                    fire_at.format("%m"),
                ))
            } else {
                // ISO timestamp — pin to that minute. Year is dropped
                // (the cron crate's 5-field form has no year), so the
                // schedule re-fires same minute-of-year forever; the
                // runner's `last_fired` anchor prevents re-fire within
                // the same year, but operators should remove one-shot
                // jobs after they execute (see module docstring).
                let dt = chrono::DateTime::parse_from_rfc3339(run_at)
                    .map(|d| d.with_timezone(&Utc))
                    .or_else(|_| {
                        // Lenient fallback for the "no timezone" form
                        // the parser emits — interpret as UTC.
                        chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%dT%H:%M:%S")
                            .map(|n| n.and_utc())
                    })
                    .map_err(|e| {
                        SchedulerError::Invalid(format!("bad timestamp '{run_at}': {e}"))
                    })?;
                Ok(format!(
                    "{} {} {} {} *",
                    dt.format("%M"),
                    dt.format("%H"),
                    dt.format("%d"),
                    dt.format("%m"),
                ))
            }
        }
    }
}

/// Build a wcore-cron [`Target`](wcore_cron::Target) from the tool's
/// create spec. Routing rules in module docstring.
fn spec_to_target(spec: &CreateJobSpec) -> CronTarget {
    // Channel delivery wins — `deliver = "channel-name"` means the
    // operator wants the result posted to a registered channel.
    if let Some(channel) = spec.deliver.as_ref().filter(|s| !s.trim().is_empty()) {
        return CronTarget::Channel {
            channel_name: channel.clone(),
            text: spec.prompt.clone(),
        };
    }
    // Skill route — first skill is the entry point; remaining skills +
    // prompt ride as args so the skill_sink can chain them.
    if let Some(first_skill) = spec.skills.first() {
        let args = serde_json::json!({
            "prompt": spec.prompt,
            "skills": spec.skills,
        });
        return CronTarget::Skill {
            name: first_skill.clone(),
            args,
        };
    }
    // Plain prompt — slash arm. The slash sink is currently log-only
    // pending the cross-session dispatcher; documented in
    // `bootstrap.rs:1820`.
    CronTarget::Slash {
        command: spec.prompt.clone(),
    }
}

/// Render a runner-side [`CronJob`](wcore_cron::CronJob) into the
/// tool's [`ToolCronJob`] shape. The tool surface is wider (`name`,
/// `state`, `last_status`, etc.) so missing fields are filled in from
/// the runner-side data when possible and defaulted otherwise.
fn render_runner_job(rj: &wcore_cron::CronJob, name_hint: Option<&str>) -> ToolCronJob {
    ToolCronJob {
        job_id: rj.id.clone(),
        name: name_hint
            .map(str::to_string)
            .unwrap_or_else(|| format!("cron-{}", &rj.id[..8.min(rj.id.len())])),
        skill: match &rj.target {
            CronTarget::Skill { name, .. } => Some(name.clone()),
            _ => None,
        },
        skills: match &rj.target {
            CronTarget::Skill { name, args } => {
                if let Some(arr) = args.get("skills").and_then(|v| v.as_array()) {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                } else {
                    vec![name.clone()]
                }
            }
            _ => vec![],
        },
        prompt_preview: match &rj.target {
            CronTarget::Slash { command } => command.chars().take(100).collect(),
            CronTarget::Channel { text, .. } => text.chars().take(100).collect(),
            CronTarget::Skill { args, .. } => args
                .get("prompt")
                .and_then(|v| v.as_str())
                .map(|s| s.chars().take(100).collect())
                .unwrap_or_default(),
        },
        schedule: Some(rj.expression.clone()),
        next_run_at: rj
            .next_fire_after(rj.last_fired.unwrap_or(rj.created_at))
            .ok()
            .flatten()
            .map(|dt| dt.to_rfc3339()),
        last_run_at: rj.last_fired.map(|dt| dt.to_rfc3339()),
        last_status: rj.last_result.as_ref().map(|o| match o {
            wcore_cron::CronFireOutcome::Success { .. } => "success".to_string(),
            wcore_cron::CronFireOutcome::Error { message } => format!("error: {message}"),
            wcore_cron::CronFireOutcome::NoSink => "no_sink".to_string(),
        }),
        enabled: rj.enabled,
        state: Some(if rj.enabled { "scheduled" } else { "paused" }.to_string()),
        ..Default::default()
    }
}

/// Map a [`CronError`] from the store layer into the tool's
/// [`SchedulerError`] shape. Centralised so all four trait methods
/// agree on wording.
fn map_store_err(e: CronError) -> SchedulerError {
    match e {
        CronError::NotFound(id) => SchedulerError::NotFound(format!("Job '{id}' not found")),
        CronError::InvalidExpression(msg) => {
            SchedulerError::Invalid(format!("invalid cron expression: {msg}"))
        }
        other => SchedulerError::Other(other.to_string()),
    }
}

#[async_trait]
impl CronScheduler for WaylandCronScheduler {
    async fn create_job(&self, spec: CreateJobSpec) -> Result<ToolCronJob, SchedulerError> {
        let expr = schedule_to_cron_expr(&spec.schedule)?;
        let target = spec_to_target(&spec);
        let runner_job = wcore_cron::CronJob::new(&expr, target)
            .map_err(|e| SchedulerError::Invalid(format!("could not build CronJob: {e}")))?;
        self.store
            .insert(runner_job.clone())
            .await
            .map_err(map_store_err)?;
        debug!(
            target: "wcore_agent::tool_backends::cron",
            id = %runner_job.id,
            expr = %expr,
            "cronjob: created via tool surface"
        );
        Ok(render_runner_job(&runner_job, spec.name.as_deref()))
    }

    async fn list_jobs(&self, include_disabled: bool) -> Result<Vec<ToolCronJob>, SchedulerError> {
        let all = self.store.list().await.map_err(map_store_err)?;
        let out: Vec<ToolCronJob> = all
            .iter()
            .filter(|j| include_disabled || j.enabled)
            .map(|j| render_runner_job(j, None))
            .collect();
        Ok(out)
    }

    async fn get_job(&self, job_id: &str) -> Result<Option<ToolCronJob>, SchedulerError> {
        let all = self.store.list().await.map_err(map_store_err)?;
        Ok(all
            .iter()
            .find(|j| j.id == job_id)
            .map(|j| render_runner_job(j, None)))
    }

    async fn update_job(
        &self,
        job_id: &str,
        spec: UpdateJobSpec,
    ) -> Result<ToolCronJob, SchedulerError> {
        let all = self.store.list().await.map_err(map_store_err)?;
        let mut found = all
            .into_iter()
            .find(|j| j.id == job_id)
            .ok_or_else(|| SchedulerError::NotFound(format!("Job '{job_id}' not found")))?;

        if let Some(new_sched) = &spec.schedule {
            let expr = schedule_to_cron_expr(new_sched)?;
            // Validate via the cron crate before we persist.
            wcore_cron::parse_expression(&expr).map_err(map_store_err)?;
            found.expression = expr;
        }
        // Prompt / skills / deliver flow into the Target shape — we
        // rebuild the target if any of those three changed.
        if spec.prompt.is_some() || spec.skills.is_some() || spec.deliver.is_some() {
            // Rebuild a synthetic CreateJobSpec carrying the merged
            // values so `spec_to_target` does the routing decision once.
            let merged = CreateJobSpec {
                prompt: spec.prompt.clone().unwrap_or_else(|| match &found.target {
                    CronTarget::Slash { command } => command.clone(),
                    CronTarget::Channel { text, .. } => text.clone(),
                    CronTarget::Skill { args, .. } => args
                        .get("prompt")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_default(),
                }),
                schedule: spec.schedule.clone().unwrap_or_default(),
                deliver: spec.deliver.clone().or_else(|| match &found.target {
                    CronTarget::Channel { channel_name, .. } => Some(channel_name.clone()),
                    _ => None,
                }),
                skills: spec.skills.clone().unwrap_or_else(|| match &found.target {
                    CronTarget::Skill { args, .. } => args
                        .get("skills")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                    _ => vec![],
                }),
                ..Default::default()
            };
            found.target = spec_to_target(&merged);
        }
        self.store
            .update(found.clone())
            .await
            .map_err(map_store_err)?;
        Ok(render_runner_job(&found, spec.name.as_deref()))
    }

    async fn pause_job(
        &self,
        job_id: &str,
        _reason: Option<&str>,
    ) -> Result<ToolCronJob, SchedulerError> {
        self.store
            .set_enabled(job_id, false)
            .await
            .map_err(map_store_err)?;
        let all = self.store.list().await.map_err(map_store_err)?;
        let j = all
            .iter()
            .find(|j| j.id == job_id)
            .ok_or_else(|| SchedulerError::NotFound(format!("Job '{job_id}' not found")))?;
        Ok(render_runner_job(j, None))
    }

    async fn resume_job(&self, job_id: &str) -> Result<ToolCronJob, SchedulerError> {
        self.store
            .set_enabled(job_id, true)
            .await
            .map_err(map_store_err)?;
        let all = self.store.list().await.map_err(map_store_err)?;
        let j = all
            .iter()
            .find(|j| j.id == job_id)
            .ok_or_else(|| SchedulerError::NotFound(format!("Job '{job_id}' not found")))?;
        Ok(render_runner_job(j, None))
    }

    async fn trigger_job(&self, job_id: &str) -> Result<ToolCronJob, SchedulerError> {
        // Force the next tick to consider this job due. We do NOT
        // dispatch inline — the runner owns the EngineJobHandler;
        // clearing `last_fired` is the supported "fire now" signal.
        let all = self.store.list().await.map_err(map_store_err)?;
        let mut found = all
            .into_iter()
            .find(|j| j.id == job_id)
            .ok_or_else(|| SchedulerError::NotFound(format!("Job '{job_id}' not found")))?;
        found.last_fired = None;
        // Re-anchor created_at into the deep past so the next-fire
        // computation lands before `now` regardless of the cron shape.
        found.created_at = Utc::now() - chrono::Duration::days(365);
        self.store
            .update(found.clone())
            .await
            .map_err(map_store_err)?;
        debug!(
            target: "wcore_agent::tool_backends::cron",
            id = %job_id,
            "cronjob: trigger queued — next runner tick will fire"
        );
        Ok(render_runner_job(&found, None))
    }

    async fn remove_job(&self, job_id: &str) -> Result<ToolCronJob, SchedulerError> {
        // Snapshot before delete so we can return the removed shape.
        let all = self.store.list().await.map_err(map_store_err)?;
        let found = all
            .iter()
            .find(|j| j.id == job_id)
            .cloned()
            .ok_or_else(|| SchedulerError::NotFound(format!("Job '{job_id}' not found")))?;
        self.store.remove(job_id).await.map_err(map_store_err)?;
        Ok(render_runner_job(&found, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use wcore_cron::FileCronStore;
    use wcore_tools::cronjob_tools::{ParsedSchedule, parse_schedule};

    fn store_in(path: std::path::PathBuf) -> Arc<dyn CronStore> {
        Arc::new(FileCronStore::new(path))
    }

    // -----------------------------------------------------------------
    // schedule conversion — guards the load-bearing tool↔runner bridge
    // -----------------------------------------------------------------

    #[test]
    fn cron_expression_parses_standard_5field() {
        // Tool layer's `parse_schedule` recognises a 5-field cron expr
        // verbatim; the adapter must round-trip it unchanged.
        let parsed = parse_schedule("0 9 * * 1-5").unwrap();
        let expr = schedule_to_cron_expr(&parsed).unwrap();
        assert_eq!(expr, "0 9 * * 1-5");
        // And the runner crate must accept it.
        assert!(wcore_cron::parse_expression(&expr).is_ok());
    }

    #[test]
    fn cron_expression_parses_6field_with_seconds() {
        // 6-field shape goes through parse_schedule's `looks_like_cron`
        // path (which only checks the first 5 fields) and is preserved.
        let parsed = parse_schedule("30 0 9 * * *").unwrap();
        let expr = schedule_to_cron_expr(&parsed).unwrap();
        // Round-trip preserved; cron crate accepts 6-field directly.
        assert_eq!(expr, "30 0 9 * * *");
        assert!(wcore_cron::parse_expression(&expr).is_ok());
    }

    #[test]
    fn interval_minutes_renders_slash_star_form() {
        let parsed = parse_schedule("every 15m").unwrap();
        let expr = schedule_to_cron_expr(&parsed).unwrap();
        assert_eq!(expr, "*/15 * * * *");
        assert!(wcore_cron::parse_expression(&expr).is_ok());
    }

    #[test]
    fn interval_hours_renders_slash_hour_form() {
        let parsed = parse_schedule("every 2h").unwrap();
        let expr = schedule_to_cron_expr(&parsed).unwrap();
        assert_eq!(expr, "0 */2 * * *");
        assert!(wcore_cron::parse_expression(&expr).is_ok());
    }

    #[test]
    fn interval_days_renders_slash_day_form() {
        let parsed = parse_schedule("every 2d").unwrap();
        let expr = schedule_to_cron_expr(&parsed).unwrap();
        assert_eq!(expr, "0 0 */2 * *");
        assert!(wcore_cron::parse_expression(&expr).is_ok());
    }

    #[test]
    fn relative_once_resolves_to_minute_pinned_expr() {
        let parsed = parse_schedule("30m").unwrap();
        let expr = schedule_to_cron_expr(&parsed).unwrap();
        // Shape: "<MM> <HH> <DD> <MM> *" — five fields, no `*` at min/hour.
        let fields: Vec<&str> = expr.split_whitespace().collect();
        assert_eq!(fields.len(), 5, "got {expr}");
        assert!(wcore_cron::parse_expression(&expr).is_ok());
    }

    #[test]
    fn iso_once_resolves_to_minute_pinned_expr() {
        let parsed = parse_schedule("2030-12-31T23:59:00").unwrap();
        let expr = schedule_to_cron_expr(&parsed).unwrap();
        assert_eq!(expr, "59 23 31 12 *");
        assert!(wcore_cron::parse_expression(&expr).is_ok());
    }

    // -----------------------------------------------------------------
    // persistence round-trip — verifies the adapter writes to the same
    // file shape the runner reads
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn cron_persistence_round_trips_through_json() {
        let dir = tempdir().unwrap();
        let store = store_in(dir.path().join("jobs.json"));
        let sched = WaylandCronScheduler::new(store.clone());

        let spec = CreateJobSpec {
            prompt: "summarise inbox".into(),
            schedule: parse_schedule("every 30m").unwrap(),
            name: Some("inbox-digest".into()),
            ..Default::default()
        };
        let created = sched.create_job(spec).await.unwrap();
        assert_eq!(created.name, "inbox-digest");
        assert!(created.enabled);

        // Independent reader sees the same row.
        let listed = sched.list_jobs(false).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].job_id, created.job_id);

        // The on-disk JSON is the same shape `FileCronStore` reads —
        // re-list via a fresh store handle to prove cross-process
        // durability.
        let fresh_store = store_in(dir.path().join("jobs.json"));
        let fresh_sched = WaylandCronScheduler::new(fresh_store);
        let listed_again = fresh_sched.list_jobs(false).await.unwrap();
        assert_eq!(listed_again.len(), 1);
        assert_eq!(listed_again[0].job_id, created.job_id);
    }

    // -----------------------------------------------------------------
    // tick safety — atomic last_fired prevents double-fire (covered by
    // wcore_cron::runner; here we just confirm pause/resume flows write)
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn cron_tick_does_not_fire_job_twice_in_same_minute() {
        // Drives the runner's `tick_once` directly so we don't need a
        // wall-clock tokio sleep. Two ticks in the same minute against
        // a daily 9am schedule that has already fired once must result
        // in exactly one dispatch — the property B6's docstring claims.
        use std::time::Duration as StdDuration;
        let dir = tempdir().unwrap();
        let store = store_in(dir.path().join("jobs.json"));

        // Insert a job whose anchor is 2 days in the past so the first
        // tick fires it; second tick must NOT re-fire (anchor advanced).
        let mut rj = wcore_cron::CronJob::new(
            "0 9 * * *",
            CronTarget::Slash {
                command: "/morning".into(),
            },
        )
        .unwrap();
        rj.created_at = Utc::now() - chrono::Duration::days(2);
        store.insert(rj.clone()).await.unwrap();

        let handler = wcore_cron::runner::RecordingHandler::new();
        let handler_arc: Arc<dyn wcore_cron::JobHandler> = Arc::new(handler.clone());

        wcore_cron::tick_once(&store, &handler_arc).await.unwrap();
        wcore_cron::tick_once(&store, &handler_arc).await.unwrap();

        assert_eq!(handler.count().await, 1, "double-fire detected");
        // Silence unused-import in this test scope.
        let _ = StdDuration::from_secs(0);
    }

    // -----------------------------------------------------------------
    // crash isolation — a panicking handler must not kill the loop. The
    // runner's loop is `tokio::select! { tick => tick_once }`; a panic
    // in tick_once propagates out of the spawned task and would kill it.
    // We assert the supported isolation: an Err return from the handler
    // is recorded as `CronFireOutcome::Error` and the runner keeps
    // ticking. (True panic isolation requires `catch_unwind` which
    // wcore-cron does not implement; documented v0.9.x followup.)
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn cron_handler_error_does_not_kill_loop_or_advance_last_fired() {
        struct ExplodingHandler;
        #[async_trait]
        impl wcore_cron::JobHandler for ExplodingHandler {
            async fn dispatch(&self, _t: &CronTarget) -> Result<(), CronError> {
                Err(CronError::Dispatch("boom".into()))
            }
        }
        let dir = tempdir().unwrap();
        let store = store_in(dir.path().join("jobs.json"));
        let mut rj = wcore_cron::CronJob::new(
            "0 9 * * *",
            CronTarget::Slash {
                command: "/x".into(),
            },
        )
        .unwrap();
        rj.created_at = Utc::now() - chrono::Duration::days(2);
        store.insert(rj.clone()).await.unwrap();

        let handler_arc: Arc<dyn wcore_cron::JobHandler> = Arc::new(ExplodingHandler);
        // First tick: handler errors out — runner records, does NOT
        // advance last_fired (F-063), and returns Ok.
        wcore_cron::tick_once(&store, &handler_arc).await.unwrap();
        let listed = store.list().await.unwrap();
        assert!(
            listed[0].last_fired.is_none(),
            "last_fired must NOT advance on dispatch error"
        );
        assert!(matches!(
            listed[0].last_result,
            Some(wcore_cron::CronFireOutcome::Error { .. })
        ));
        // Second tick after error — still no double-charge of last_fired.
        wcore_cron::tick_once(&store, &handler_arc).await.unwrap();
        let listed2 = store.list().await.unwrap();
        assert!(listed2[0].last_fired.is_none());
    }

    // -----------------------------------------------------------------
    // tool-side gate
    // -----------------------------------------------------------------

    #[test]
    fn null_default_skips_registration() {
        // The Default CronJobTool (no scheduler wired) MUST hide itself.
        // The actual ToolRegistry filtering lives in wcore_tools::lib —
        // here we just verify the gate that drives it.
        use wcore_tools::Tool as _;
        use wcore_tools::cronjob_tools::CronJobTool;
        let tool = CronJobTool::default();
        assert!(
            !tool.is_available(),
            "Default CronJobTool must hide until a real scheduler is wired"
        );
    }

    // -----------------------------------------------------------------
    // trigger semantics
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn trigger_clears_last_fired_so_next_tick_re_fires() {
        let dir = tempdir().unwrap();
        let store = store_in(dir.path().join("jobs.json"));
        let sched = WaylandCronScheduler::new(store.clone());
        let created = sched
            .create_job(CreateJobSpec {
                prompt: "p".into(),
                schedule: ParsedSchedule::Cron {
                    expr: "0 9 * * *".into(),
                    display: "0 9 * * *".into(),
                },
                ..Default::default()
            })
            .await
            .unwrap();

        // Mark it as recently fired so without trigger it won't fire.
        let mut all = store.list().await.unwrap();
        all[0].last_fired = Some(Utc::now());
        store.update(all[0].clone()).await.unwrap();

        // Trigger.
        sched.trigger_job(&created.job_id).await.unwrap();

        // last_fired must now be None.
        let after = store.list().await.unwrap();
        assert!(
            after[0].last_fired.is_none(),
            "trigger must clear last_fired so the runner refires"
        );
    }
}
