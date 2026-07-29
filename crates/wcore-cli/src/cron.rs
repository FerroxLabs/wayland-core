//! v0.8.1 U7 + W6-K-rest: `wayland-core cron` subcommands.
//!
//! CRUD ops (add / list / remove / enable / disable) plus diagnostic
//! ops (status / history / logs) against the `wcore-cron` store, and a
//! `daemon` subcommand that spawns the runner detached.
//!
//! Store path: `$WAYLAND_HOME/cron/jobs.json`, falling back to
//! `~/.wayland/cron/jobs.json`. History: same dir, `history.jsonl`.
//!
//! The runner picks up changes on its next tick — there's no in-band
//! `reload()` signal because the file store re-reads from disk every
//! list call.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use wcore_cron::{CronFireOutcome, CronFireRecord, CronJob, CronStore, FileCronStore, Target};

#[derive(Subcommand, Debug)]
pub enum CronCmd {
    /// List every persisted cron job.
    List,

    /// Add a new job. Exactly one of `--slash`, `--channel`, or
    /// `--skill` must be provided (with matching companion flags).
    ///
    /// Examples:
    ///   wayland-core cron add "0 9 * * *" --skill hello
    ///   wayland-core cron add "*/15 * * * *" --slash "/status"
    ///   wayland-core cron add "0 8 * * 1" --channel team --text "Good morning"
    // F-075: examples surface in `cron add --help` so users can copy-paste
    // a working invocation without reading the full man page.
    Add {
        /// Cron expression (5-field crontab shape or 6-field
        /// `cron`-crate shape, e.g. "0 9 * * *" = daily at 09:00).
        ///
        /// Optional when `--trigger` or `--describe` supplies the timing
        /// instead.
        expression: Option<String>,

        /// Phase 24: any trigger in the vocabulary, as `kind:params`.
        ///
        ///   --trigger once:2026-08-01T09:00:00Z
        ///   --trigger every:900
        ///   --trigger cron:"0 9 * * *"
        ///   --trigger event:build.finished     (fire it with `cron publish`)
        ///   --trigger commit:2026-08-01T17:00:00Z:900
        ///
        /// `webhook:` and `poll:` are NOT accepted: nothing in this build can
        /// fire them. They are refused at this verb rather than persisted as a
        /// job that never runs. Use `event:` plus `cron publish` for a webhook,
        /// and `every:SECONDS` for a plain timer.
        #[arg(long, value_name = "KIND:PARAMS")]
        trigger: Option<String>,

        /// Phase 24: author the timing from a phrase.
        ///
        /// Prints the concrete trigger the phrase resolved to, together with
        /// the next few computed fire times, and writes NOTHING until
        /// `--confirm` is also given. An unparseable phrase is quoted back
        /// and nothing is written.
        ///
        ///   --describe "every weekday at 9am"
        #[arg(long, value_name = "PHRASE")]
        describe: Option<String>,

        /// Persist what `--describe` proposed. Without it, `--describe` only
        /// shows the candidate.
        #[arg(long, requires = "describe")]
        confirm: bool,

        /// Slash-command target: run the given command on fire.
        #[arg(long, conflicts_with_all = ["channel", "skill"], value_name = "COMMAND")]
        slash: Option<String>,

        /// Channel-message target: send `--text` to the named channel.
        #[arg(long, conflicts_with_all = ["slash", "skill"], value_name = "NAME")]
        channel: Option<String>,

        /// Text body for `--channel`. Required when `--channel` is set.
        #[arg(long, requires = "channel", value_name = "TEXT")]
        text: Option<String>,

        /// Skill-invocation target: invoke the named skill on fire.
        #[arg(long, conflicts_with_all = ["slash", "channel"], value_name = "NAME")]
        skill: Option<String>,

        /// JSON args for `--skill` (default `{}`).
        #[arg(long, requires = "skill", value_name = "JSON")]
        args: Option<String>,
    },

    /// Publish an event topic, firing every job with a matching
    /// `--trigger event:TOPIC`.
    ///
    /// This is the producer for event triggers. Anything that can run a
    /// command can drive the schedule with it — a CI step, a git hook, a
    /// skill, an operator at a shell:
    ///
    ///   wayland-core cron add --trigger event:build.finished --skill notify
    ///   wayland-core cron publish build.finished
    ///
    /// The topic is matched EXACTLY — no prefix, no glob. Delivery is at least
    /// once: a process killed between firing a job and clearing the event
    /// re-fires it on the next tick, so the job's action must tolerate a
    /// repeat. The event is queued on disk beside `jobs.json` and is consumed
    /// by whichever process owns the schedule (the gateway, or `cron daemon`),
    /// so publishing works whether or not one is running right now.
    Publish {
        /// Topic to publish, matching an `event:` trigger exactly.
        topic: String,
    },

    /// Remove a job by id.
    Remove {
        /// UUID returned by `cron list` / `cron add`.
        id: String,
    },

    /// Enable a job by id.
    Enable {
        /// UUID returned by `cron list`.
        id: String,
    },

    /// Disable a job by id (kept on disk, skipped by the runner).
    Disable {
        /// UUID returned by `cron list`.
        id: String,
    },

    /// Print full details for one job: id, expression, target, state,
    /// created_at, last_fired, and the outcome of the most recent fire
    /// attempt (success + duration, or error message).
    ///
    /// Example:
    ///   wayland-core cron status <id>
    Status {
        /// UUID returned by `cron list` / `cron add`.
        id: String,
    },

    /// Print the last N fire records for a job (timestamp, outcome,
    /// duration, error message if any). Records come from the JSONL
    /// ring-buffer written by the runner alongside jobs.json.
    ///
    /// Example:
    ///   wayland-core cron history <id> --limit 10
    History {
        /// UUID returned by `cron list`.
        id: String,
        /// Maximum records to show (most-recent first). Default 20.
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,
    },

    /// Tail recent log lines associated with a job's fires. Currently
    /// surfaces fire records from the history file (same data as
    /// `cron history`) formatted as structured log lines compatible
    /// with the engine's tracing output.
    ///
    /// Example:
    ///   wayland-core cron logs <id> --limit 50
    Logs {
        /// UUID returned by `cron list`.
        id: String,
        /// Maximum records to show (most-recent first). Default 50.
        #[arg(long, short = 'n', default_value = "50")]
        limit: usize,
    },

    /// Spawn the cron runner as a detached background daemon.
    ///
    /// The daemon:
    /// - Writes its PID to `$WAYLAND_HOME/cron-daemon.pid`
    /// - Logs to `$WAYLAND_HOME/cron-daemon.log`
    /// - Honours SIGTERM for clean shutdown
    /// - Persists fire history to `$WAYLAND_HOME/cron/history.jsonl`
    ///
    /// To install as a persistent system service, see the templates under
    /// `templates/cron-daemon/` (launchd.plist / systemd.service).
    Daemon,
}

pub async fn run(cmd: CronCmd) -> Result<()> {
    let store = FileCronStore::from_default_path()
        .context("could not resolve cron store path (no WAYLAND_HOME and no $HOME)")?;
    let history_path = wcore_cron::default_history_path();
    run_inner(cmd, &store, history_path.as_ref()).await
}

/// Test-friendly entry point — accepts an explicit store so tests can
/// drive the same code path against a tempdir.
pub async fn run_with_store(cmd: CronCmd, store: &FileCronStore) -> Result<()> {
    run_inner(cmd, store, None).await
}

async fn run_inner(
    cmd: CronCmd,
    store: &FileCronStore,
    history_path: Option<&PathBuf>,
) -> Result<()> {
    match cmd {
        CronCmd::List => list_cmd(store).await,
        CronCmd::Add {
            expression,
            trigger,
            describe,
            confirm,
            slash,
            channel,
            text,
            skill,
            args,
        } => {
            add_cmd(
                AddRequest {
                    expression,
                    trigger,
                    describe,
                    confirm,
                },
                slash,
                channel,
                text,
                skill,
                args,
                store,
            )
            .await
        }
        CronCmd::Publish { topic } => publish_cmd(&topic, store).await,
        CronCmd::Remove { id } => {
            store.remove(&id).await.context("cron remove failed")?;
            println!("removed {id}");
            Ok(())
        }
        CronCmd::Enable { id } => {
            store
                .set_enabled(&id, true)
                .await
                .context("cron enable failed")?;
            println!("enabled {id}");
            Ok(())
        }
        CronCmd::Disable { id } => {
            store
                .set_enabled(&id, false)
                .await
                .context("cron disable failed")?;
            println!("disabled {id}");
            Ok(())
        }
        CronCmd::Status { id } => status_cmd(&id, store).await,
        CronCmd::History { id, limit } => history_cmd(&id, limit, history_path).await,
        CronCmd::Logs { id, limit } => logs_cmd(&id, limit, history_path).await,
        CronCmd::Daemon => daemon_cmd(store).await,
    }
}

/// Publish a topic into the schedule's event queue.
///
/// Reports how many jobs are currently subscribed, because "published" alone
/// does not tell an operator whether anything will happen — and a publish that
/// matches nothing is the most likely way to mistype a topic. It is NOT an
/// error: the subscriber may be created later, and refusing here would make
/// the ordering of two independent operations load-bearing.
async fn publish_cmd(topic: &str, store: &FileCronStore) -> Result<()> {
    let cron_dir = store
        .path()
        .parent()
        .map(PathBuf::from)
        .context("cron store has no parent directory to queue events in")?;
    let event = wcore_cron::publish_event(&cron_dir, topic, chrono::Utc::now())
        .with_context(|| format!("could not publish {topic:?}"))?;

    let subscribers = store
        .list()
        .await
        .map(|jobs| {
            jobs.iter()
                .filter(|j| j.enabled)
                .filter(|j| {
                    matches!(j.effective_trigger(), wcore_cron::Trigger::Event { topic: t } if t == event.topic)
                })
                .count()
        })
        .unwrap_or(0);

    println!("published {topic:?} ({})", event.id);
    if subscribers == 0 {
        println!(
            "warning: no enabled job is subscribed to {topic:?}; it will be queued until one is"
        );
    } else {
        println!("{subscribers} subscribed job(s) will fire on the schedule owner's next tick");
    }
    Ok(())
}

async fn list_cmd(store: &FileCronStore) -> Result<()> {
    let jobs = store.list().await.context("cron list failed")?;
    if jobs.is_empty() {
        println!("(no cron jobs)");
        println!("store: {}", store.path().display());
        return Ok(());
    }
    for job in &jobs {
        let state = if job.enabled { "on " } else { "off" };
        let target = render_target(&job.target);
        // F-064 fix (MED): surface last_fired so users can confirm cron is
        // actually firing. Data was always persisted in jobs.json — just
        // never printed. "never" when the job has not fired yet this session.
        let last_fired = job
            .last_fired
            .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_else(|| "never".to_string());
        // Phase 24 plan 24-02: the trigger kind and the retry state are shown
        // on the SAME line as everything else rather than behind a second
        // verb. A job that has given up is the one an operator most needs to
        // notice, and a state only visible in `cron status <id>` is a state
        // nobody looks at until they already suspect something.
        let kind = job.effective_trigger().kind();
        let retry = if job.retry_state.gave_up {
            format!("  GAVE_UP(after {} attempts)", job.retry_state.attempts)
        } else if job.retry_state.attempts > 0 {
            format!("  retrying({})", job.retry_state.attempts)
        } else {
            String::new()
        };
        println!(
            "{state} {id}  [{kind:<10}] {expr:<28}  {target:<30}  last_fired={last_fired}{retry}",
            state = state,
            id = job.id,
            kind = kind,
            expr = job.expression,
            target = target,
            last_fired = last_fired
        );
        // 24-C2: a job whose trigger has no producer can never fire. Creating
        // one is now refused, but jobs written before that — or by the Desktop
        // app, or by hand — are still on disk, and listing them beside working
        // jobs with no distinction is the silent acceptance this repair exists
        // to remove. It is printed under the job, not as a footnote, because a
        // footnote is a thing nobody reads.
        if let Some(reason) = job.effective_trigger().no_producer_reason() {
            println!("      ^ WILL NEVER FIRE — {reason}");
        }
    }
    Ok(())
}

/// How the caller expressed the timing. Exactly one of the three must be
/// supplied — three ways to say the same thing is a usable surface, three ways
/// that silently override each other is not.
pub struct AddRequest {
    pub expression: Option<String>,
    pub trigger: Option<String>,
    pub describe: Option<String>,
    pub confirm: bool,
}

#[allow(clippy::too_many_arguments)]
async fn add_cmd(
    timing: AddRequest,
    slash: Option<String>,
    channel: Option<String>,
    text: Option<String>,
    skill: Option<String>,
    args: Option<String>,
    store: &FileCronStore,
) -> Result<()> {
    let target = match (slash, channel, text, skill, args) {
        (Some(cmd), None, None, None, None) => Target::Slash { command: cmd },
        (None, Some(ch), Some(body), None, None) => Target::Channel {
            channel_name: ch,
            text: body,
        },
        (None, Some(_), None, None, None) => {
            bail!("`--channel` requires `--text \"...\"`");
        }
        (None, None, None, Some(name), args_raw) => {
            let args_value = match args_raw {
                Some(raw) => serde_json::from_str(&raw)
                    .with_context(|| format!("`--args` is not valid JSON: {raw}"))?,
                None => serde_json::Value::Object(Default::default()),
            };
            Target::Skill {
                name,
                args: args_value,
            }
        }
        (None, None, None, None, _) => bail!(
            "must provide exactly one target: `--slash <CMD>`, `--channel <NAME> --text <TEXT>`, or `--skill <NAME>`"
        ),
        _ => bail!("`--slash`, `--channel`, and `--skill` are mutually exclusive"),
    };
    let AddRequest {
        expression,
        trigger,
        describe,
        confirm,
    } = timing;

    let supplied = [expression.is_some(), trigger.is_some(), describe.is_some()]
        .iter()
        .filter(|b| **b)
        .count();
    if supplied == 0 {
        bail!(
            "provide the timing exactly once: a cron expression, `--trigger KIND:PARAMS`, or `--describe \"phrase\"`"
        );
    }
    if supplied > 1 {
        bail!(
            "the timing was given more than once; use exactly one of a cron expression, `--trigger`, or `--describe`"
        );
    }

    let job = if let Some(phrase) = describe {
        // NATURAL-LANGUAGE AUTHORING. A phrase becomes a CANDIDATE that the
        // operator sees before anything is persisted. A background runtime
        // that silently schedules whatever a sentence was interpreted to mean
        // is a correctness problem and a safety problem at once (threat
        // T-24-02-01), so the candidate plus its next computed fire times are
        // printed and nothing is written without `--confirm`.
        let Some(t) = parse_phrase(&phrase) else {
            // Quoted back verbatim, and nothing written.
            bail!("could not interpret {phrase:?} as a schedule; nothing was written");
        };
        refuse_without_producer(&t)?;
        let candidate = CronJob::with_trigger(t.clone(), target.clone())
            .context("the interpreted trigger is not valid")?;
        println!("phrase:  {phrase:?}");
        println!("becomes: {}", wcore_cron::render_trigger(&t));
        print_next_fires(&candidate, 3);
        if !confirm {
            println!();
            println!("nothing written. re-run with --confirm to persist this schedule.");
            return Ok(());
        }
        candidate
    } else if let Some(spec) = trigger {
        let t = parse_trigger_spec(&spec)
            .with_context(|| format!("could not parse --trigger {spec:?}"))?;
        refuse_without_producer(&t)?;
        let job = CronJob::with_trigger(t, target).context("could not create cron job")?;
        print_next_fires(&job, 3);
        job
    } else {
        let expr = expression.expect("checked above");
        CronJob::new(expr, target).context("could not create cron job")?
    };

    let id = job.id.clone();
    store.insert(job).await.context("cron add failed")?;
    println!("added {id}");
    Ok(())
}

/// Refuse to create a job whose trigger nothing in this build can fire.
///
/// 24-C2, and this is the whole point of the repair. `webhook:` and `poll:`
/// validated, persisted and appeared in `cron list` with no error and no
/// warning; the operator got a job that looked healthy and never ran. Accepting
/// a trigger with no producer is the one outcome that is not acceptable, so the
/// refusal happens BEFORE anything is written, names exactly what is
/// unsupported, and points at what to use instead.
///
/// Deliberately NOT folded into [`parse_trigger_spec`]: parsing must keep
/// working, because a persisted `webhook` job still has to load and still has
/// to report its `require_auth` posture honestly in `cron list`. What is
/// refused is CREATING one, and the error says so in those terms.
fn refuse_without_producer(t: &wcore_cron::Trigger) -> Result<()> {
    if let Some(reason) = t.no_producer_reason() {
        bail!(
            "refusing to create a {kind} job: {reason}\n\
             Nothing was written.",
            kind = t.kind(),
            reason = reason
        );
    }
    Ok(())
}

/// Print the next few instants a job will fire, so an operator can confirm the
/// timing means what they thought before it runs unattended.
///
/// An externally driven trigger prints that it cannot be predicted rather than
/// printing nothing — silence there reads as "it will never fire".
fn print_next_fires(job: &CronJob, n: usize) {
    let t = job.effective_trigger();
    let bound = job.effective_bound();
    if !t.is_clock_driven() {
        println!(
            "next:    driven externally ({}) — not predictable from the clock",
            t.kind()
        );
        return;
    }
    let mut at = chrono::Utc::now();
    for i in 0..n {
        match t.next_after(at, &bound) {
            Ok(Some(next)) => {
                println!("next[{i}]: {}", next.to_rfc3339());
                at = next;
            }
            Ok(None) => {
                if i == 0 {
                    println!("next:    no future occurrence");
                }
                break;
            }
            Err(e) => {
                println!("next:    could not be computed: {e}");
                break;
            }
        }
    }
}

/// Parse a `kind:params` trigger spec.
///
/// Deliberately strict: an unrecognised kind is an error rather than a
/// fallback to cron, because silently reinterpreting `webhook:/x` as a cron
/// expression would produce a job that never fires and never says why.
pub fn parse_trigger_spec(spec: &str) -> Result<wcore_cron::Trigger> {
    use wcore_cron::Trigger;
    let (kind, rest) = spec
        .split_once(':')
        .with_context(|| format!("expected KIND:PARAMS, got {spec:?}"))?;
    let t = match kind {
        "once" => Trigger::Once {
            at: parse_instant(rest)?,
        },
        "every" | "interval" => Trigger::Interval {
            every_secs: rest
                .trim_end_matches('s')
                .parse()
                .with_context(|| format!("expected seconds, got {rest:?}"))?,
        },
        "cron" => Trigger::Cron {
            expression: rest.to_string(),
        },
        "event" => Trigger::Event {
            topic: rest.to_string(),
        },
        "webhook" => {
            // `path[:open]`. The default is authenticated; opening an endpoint
            // has to be typed out.
            let (path, open) = match rest.rsplit_once(':') {
                Some((p, "open")) => (p.to_string(), true),
                _ => (rest.to_string(), false),
            };
            Trigger::Webhook {
                path,
                require_auth: !open,
            }
        }
        "poll" => {
            let (url, secs) = rest
                .rsplit_once(':')
                .with_context(|| format!("expected poll:URL:SECONDS, got {rest:?}"))?;
            Trigger::Poll {
                url: url.to_string(),
                every_secs: secs
                    .parse()
                    .with_context(|| format!("expected seconds, got {secs:?}"))?,
            }
        }
        "commit" | "commitment" => {
            let (deadline, hb) = rest
                .rsplit_once(':')
                .with_context(|| format!("expected commit:DEADLINE:HEARTBEAT, got {rest:?}"))?;
            Trigger::Commitment {
                deadline: parse_instant(deadline)?,
                heartbeat_secs: hb
                    .parse()
                    .with_context(|| format!("expected seconds, got {hb:?}"))?,
            }
        }
        other => bail!(
            "unknown trigger kind {other:?}; expected one of {}",
            wcore_cron::Trigger::KINDS.join(", ")
        ),
    };
    t.validate()?;
    Ok(t)
}

fn parse_instant(s: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&chrono::Utc))
        .with_context(|| format!("expected an RFC3339 instant, got {s:?}"))
}

/// Turn a phrase into a concrete trigger, or refuse.
///
/// Deliberately a SMALL, deterministic vocabulary rather than a fuzzy match. A
/// phrase this cannot interpret is refused and quoted back; guessing is how a
/// sentence turns into a schedule the operator did not intend, which is the
/// whole reason the confirmation step exists. Every accepted phrase is shown
/// with its next fire times before it can be persisted.
pub fn parse_phrase(phrase: &str) -> Option<wcore_cron::Trigger> {
    use wcore_cron::Trigger;
    let p = phrase.trim().to_lowercase();
    let words: Vec<&str> = p.split_whitespace().collect();

    // "every N minutes|hours|days"
    if words.len() >= 3
        && words[0] == "every"
        && let Ok(n) = words[1].parse::<u64>()
    {
        {
            let secs = match words[2].trim_end_matches('s') {
                "second" => Some(1),
                "minute" => Some(60),
                "hour" => Some(3600),
                "day" => Some(86_400),
                _ => None,
            }?;
            return Some(Trigger::Interval {
                every_secs: n.saturating_mul(secs),
            });
        }
    }

    // "every minute|hour|day" (no count)
    if words.len() == 2 && words[0] == "every" {
        return match words[1] {
            "minute" => Some(Trigger::Interval { every_secs: 60 }),
            "hour" => Some(Trigger::Interval { every_secs: 3600 }),
            "day" => Some(Trigger::Cron {
                expression: "0 0 * * *".into(),
            }),
            _ => None,
        };
    }

    // "every day at <time>", "daily at <time>", "every weekday at <time>",
    // "every <weekday> at <time>"
    let at_pos = words.iter().position(|w| *w == "at")?;
    let time = words.get(at_pos + 1)?;
    let (hour, minute) = parse_clock(time)?;
    let head = words[..at_pos].join(" ");
    let dow = match head.as_str() {
        "every day" | "daily" => "*",
        "every weekday" | "weekdays" => "1-5",
        "every weekend" => "0,6",
        "every monday" => "1",
        "every tuesday" => "2",
        "every wednesday" => "3",
        "every thursday" => "4",
        "every friday" => "5",
        "every saturday" => "6",
        "every sunday" => "0",
        _ => return None,
    };
    Some(Trigger::Cron {
        expression: format!("{minute} {hour} * * {dow}"),
    })
}

/// Parse `9am`, `9:30am`, `17:00`, `09:05`.
fn parse_clock(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().trim_end_matches('.');
    let (body, shift) = if let Some(b) = s.strip_suffix("am") {
        (b, 0)
    } else if let Some(b) = s.strip_suffix("pm") {
        (b, 12)
    } else {
        (s, -1)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (body.parse::<u32>().ok()?, 0),
    };
    if m > 59 {
        return None;
    }
    let hour = match shift {
        // 24-hour: accept as written.
        -1 => {
            if h > 23 {
                return None;
            }
            h
        }
        // 12-hour: 12am is 00, 12pm is 12.
        0 => {
            if h == 0 || h > 12 {
                return None;
            }
            if h == 12 { 0 } else { h }
        }
        _ => {
            if h == 0 || h > 12 {
                return None;
            }
            if h == 12 { 12 } else { h + 12 }
        }
    };
    Some((hour, m))
}

fn render_target(t: &Target) -> String {
    match t {
        Target::Slash { command } => format!("slash    {command}"),
        Target::Channel { channel_name, text } => {
            let preview = preview(text, 40);
            format!("channel  {channel_name} :: {preview}")
        }
        Target::Skill { name, args } => format!("skill    {name} {args}"),
    }
}

fn preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max - 1).collect();
        format!("{head}...")
    }
}

/// F-065: `cron status <id>` — print full job details including last_result.
async fn status_cmd(id: &str, store: &FileCronStore) -> Result<()> {
    let jobs = store.list().await.context("cron list failed")?;
    let job = jobs
        .iter()
        .find(|j| j.id == id)
        .with_context(|| format!("job not found: {id}"))?;

    let state = if job.enabled { "enabled" } else { "disabled" };
    let target = render_target(&job.target);
    let created = job.created_at.format("%Y-%m-%dT%H:%M:%SZ");
    let last_fired = job
        .last_fired
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| "never".to_string());
    let last_result = match &job.last_result {
        None => "none (never fired)".to_string(),
        Some(CronFireOutcome::Success { duration_ms }) => {
            format!("success ({duration_ms}ms)")
        }
        Some(CronFireOutcome::Error { message }) => format!("error: {message}"),
        Some(CronFireOutcome::NoSink) => {
            "no-sink (nothing fired; last_fired not advanced)".to_string()
        }
        Some(CronFireOutcome::Staged) => {
            "staged (no live dispatcher; last_fired advanced, not a success)".to_string()
        }
        Some(CronFireOutcome::Abandoned { reason }) => {
            format!("abandoned ({reason}); the fire did not run and is still owed")
        }
        Some(CronFireOutcome::GaveUp { attempts, message }) => {
            format!("gave up after {attempts} attempts: {message}")
        }
    };

    let trigger = job.effective_trigger();
    let bound = job.effective_bound();
    let retry = job.effective_retry();

    println!("id:          {}", job.id);
    println!("trigger:     {} — {}", trigger.kind(), job.expression);
    // 24-C2: stated FIRST among the diagnostics, because every field under it
    // describes a job that is never going to run.
    if let Some(reason) = trigger.no_producer_reason() {
        println!("reachable:   NO — {reason}");
    }
    println!(
        "bound:       min_interval={}s max_in_flight={} deadline={}",
        bound.min_interval_secs,
        bound.max_in_flight,
        bound
            .deadline
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| "none".to_string())
    );
    // 24-C4-SUPPORT: `max_in_flight` above 1 promises concurrency this runtime
    // does not produce. `dispatch_and_record` is awaited inline in the runner's
    // selection loop and the production handler does not spawn, so a job's
    // fires are serialized end to end. Measured in
    // `crates/wcore-cron/tests/in_flight_bound.rs`: `max_in_flight = 8` and
    // `max_in_flight = 16` both produce peak concurrency 1, indistinguishable
    // from 1, against a probe proved able to observe 2.
    //
    // Said here rather than silently accepted, for the same reason `poll:` is
    // now refused at `add` instead of being accepted and never fired: a bound
    // the product echoes back and does not implement is a surface that lies to
    // the operator who set it. The value is NOT rewritten — narrowing a
    // persisted field on a render path would make `show` disagree with the
    // store — it is annotated.
    if bound.max_in_flight > 1 {
        println!(
            "             NOTE: fires are serialized; max_in_flight>1 grants no \
             concurrency in this build"
        );
    }
    println!(
        "retry:       max_attempts={} backoff={}s..{}s  attempts={}{}",
        retry.max_attempts,
        retry.base_backoff_secs,
        retry.max_backoff_secs,
        job.retry_state.attempts,
        if job.retry_state.gave_up {
            "  GAVE UP"
        } else {
            ""
        }
    );
    if let Some(hb) = job.heartbeat_state(chrono::Utc::now()) {
        println!("heartbeat:   {hb:?}");
    }
    println!("target:      {target}");
    println!("state:       {state}");
    println!("created_at:  {created}");
    println!("last_fired:  {last_fired}");
    println!("last_result: {last_result}");
    Ok(())
}

/// Read fire records from the JSONL history file, returning up to `limit`
/// records for `job_id`, most-recent first.
fn read_history(job_id: &str, limit: usize, path: Option<&PathBuf>) -> Vec<CronFireRecord> {
    let Some(p) = path else {
        return Vec::new();
    };
    let file = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    // Collect all matching records then take the tail (most-recent are last
    // in the append-only file).
    let mut records: Vec<CronFireRecord> = reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<CronFireRecord>(&line).ok())
        .filter(|r| r.job_id == job_id)
        .collect();
    // Most-recent first.
    records.reverse();
    records.truncate(limit);
    records
}

/// F-065: `cron history <id> [--limit N]` — recent fire records.
async fn history_cmd(id: &str, limit: usize, history_path: Option<&PathBuf>) -> Result<()> {
    let records = read_history(id, limit, history_path);
    if records.is_empty() {
        println!("(no fire records for {id})");
        return Ok(());
    }
    for rec in &records {
        let ts = rec.fired_at.format("%Y-%m-%dT%H:%M:%SZ");
        let outcome = match &rec.outcome {
            CronFireOutcome::Success { duration_ms } => format!("success ({duration_ms}ms)"),
            CronFireOutcome::Error { message } => format!("error: {message}"),
            CronFireOutcome::NoSink => "no-sink".to_string(),
            CronFireOutcome::Staged => "staged (no live dispatcher)".to_string(),
            CronFireOutcome::Abandoned { reason } => format!("abandoned: {reason}"),
            CronFireOutcome::GaveUp { attempts, message } => {
                format!("gave up after {attempts}: {message}")
            }
        };
        println!("{ts}  {outcome}");
    }
    Ok(())
}

/// F-065: `cron logs <id> [--limit N]` — fire records as structured log lines.
async fn logs_cmd(id: &str, limit: usize, history_path: Option<&PathBuf>) -> Result<()> {
    let records = read_history(id, limit, history_path);
    if records.is_empty() {
        println!("(no log records for {id})");
        return Ok(());
    }
    for rec in &records {
        let ts = rec.fired_at.format("%Y-%m-%dT%H:%M:%SZ");
        let (level, outcome) = match &rec.outcome {
            CronFireOutcome::Success { duration_ms } => {
                ("INFO ", format!("fired ok duration_ms={duration_ms}"))
            }
            CronFireOutcome::Error { message } => ("WARN ", format!("dispatch failed: {message}")),
            CronFireOutcome::NoSink => ("WARN ", "no sink; last_fired not advanced".to_string()),
            CronFireOutcome::Abandoned { reason } => (
                "WARN ",
                format!("abandoned mid-tick: {reason}; still owed by the next owner"),
            ),
            CronFireOutcome::GaveUp { attempts, message } => (
                "ERROR",
                format!("gave up after {attempts} attempts: {message}"),
            ),
            CronFireOutcome::Staged => (
                "INFO ",
                "staged — no live dispatcher; last_fired advanced".to_string(),
            ),
        };
        println!("{ts}  {level}  wcore_cron::runner  job_id={id}  {outcome}");
    }
    Ok(())
}

/// F-066: `cron daemon` — detached runner.
///
/// Spawns a child process that runs the cron runner detached from the
/// controlling terminal. The child:
/// - Writes its PID to `$WAYLAND_HOME/cron-daemon.pid`
/// - Logs to `$WAYLAND_HOME/cron-daemon.log`
/// - Honours SIGTERM for clean shutdown (via tokio::signal)
/// - Persists fire history to `$WAYLAND_HOME/cron/history.jsonl`
///
/// On non-Unix platforms, prints an informational error and exits cleanly.
async fn daemon_cmd(store: &FileCronStore) -> Result<()> {
    use std::fs;

    // Resolve home dir for PID + log files.
    let wayland_home = std::env::var_os("WAYLAND_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".wayland")))
        .context("cannot resolve WAYLAND_HOME for daemon files")?;

    fs::create_dir_all(&wayland_home).context("cannot create WAYLAND_HOME")?;

    let pid_path = wayland_home.join("cron-daemon.pid");
    let log_path = wayland_home.join("cron-daemon.log");
    let history_path = wayland_home.join("cron").join("history.jsonl");

    // If we are the daemon child body, run the runner loop.
    if std::env::var("WAYLAND_CRON_DAEMON_CHILD").is_ok() {
        return daemon_body(store, &pid_path, &history_path).await;
    }

    // Check for a stale PID file — if the process is still alive, refuse to
    // start a second daemon.
    if pid_path.exists() {
        if let Ok(raw) = fs::read_to_string(&pid_path) {
            let existing_pid = raw.trim().parse::<u32>().unwrap_or(0);
            if existing_pid > 0 && process_is_alive(existing_pid) {
                bail!(
                    "cron daemon already running (pid {existing_pid}). \
                     Use `kill {existing_pid}` to stop it first."
                );
            }
        }
        // Stale file — remove it.
        let _ = fs::remove_file(&pid_path);
    }

    // Re-exec the current binary with the sentinel env var, redirecting
    // stdout/stderr to the log file so the child is decoupled from the
    // calling terminal.
    let current_exe = std::env::current_exe().context("cannot resolve current binary")?;
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("cannot open log file: {}", log_path.display()))?;

    #[cfg(unix)]
    let child = {
        use std::os::unix::process::CommandExt as _;
        std::process::Command::new(&current_exe)
            .args(["cron", "daemon"])
            .env("WAYLAND_CRON_DAEMON_CHILD", "1")
            .env("WAYLAND_HOME", wayland_home.to_string_lossy().as_ref())
            .stdin(std::process::Stdio::null())
            .stdout(log_file.try_clone().context("log file clone")?)
            .stderr(log_file)
            // process_group(0) calls setsid() in the child — detaches from
            // the parent's process group and controlling terminal.
            .process_group(0)
            .spawn()
            .context("failed to spawn daemon child")?
    };

    #[cfg(not(unix))]
    let child = {
        let mut cmd = std::process::Command::new(&current_exe);
        cmd.args(["cron", "daemon"])
            .env("WAYLAND_CRON_DAEMON_CHILD", "1")
            .env("WAYLAND_HOME", wayland_home.to_string_lossy().as_ref())
            .stdin(std::process::Stdio::null())
            .stdout(log_file.try_clone().context("log file clone")?)
            .stderr(log_file);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // F24-01 Task 3, MEASURED on SEANDESKTOP 2026-07-26. This branch
            // previously set NO creation flags while its Unix sibling above
            // calls process_group(0) (setsid). A probe compiled with rustc on
            // the box, spawning through this exact std::process::Command path
            // and then exiting as this function does, wrote 1 of 600
            // heartbeats and was gone: the child stayed inside the launching
            // session's job object and died with it. The SAME probe with these
            // three flags wrote 600 of 600 and exited normally. One variable.
            //
            // CREATE_BREAKAWAY_FROM_JOB is the load-bearing one. Detaching the
            // console and leaving the process group is not enough on their own
            // — Windows OpenSSH reaps session children through a Job Object,
            // and only a breakaway leaves it.
            //
            // Evidence: 24-01-GATEWAY-CONTRACT.md, probes `detach-baseline`
            // and `detached-flags`.
            cmd.creation_flags(
                wcore_gateway::service::DETACHED_PROCESS
                    | wcore_gateway::service::CREATE_NEW_PROCESS_GROUP
                    | wcore_gateway::service::CREATE_BREAKAWAY_FROM_JOB,
            );
        }
        cmd.spawn().context("failed to spawn daemon child")?
    };

    let child_pid = child.id();
    fs::write(&pid_path, format!("{child_pid}\n"))
        .with_context(|| format!("cannot write PID file: {}", pid_path.display()))?;

    println!(
        "cron daemon started (pid {child_pid})\n  pid:  {}\n  log:  {}",
        pid_path.display(),
        log_path.display()
    );
    Ok(())
}

/// Daemon body — runs inside the re-exec'd child process.
async fn daemon_body(
    store: &FileCronStore,
    pid_path: &std::path::Path,
    history_path: &PathBuf,
) -> Result<()> {
    let my_pid = std::process::id();
    let _ = std::fs::write(pid_path, format!("{my_pid}\n"));

    eprintln!("[cron-daemon] started pid={my_pid}");

    let cron_store: Arc<dyn wcore_cron::CronStore> = Arc::new(store.clone());
    // rank 3: the daemon has no engine session, but it CAN dispatch Skill and
    // Channel jobs headlessly. Build a real handler (engine-less skill sink +
    // started ChannelManager); Slash stays None (no cross-session dispatcher),
    // so slash fires stage → Staged. Previously this installed a log-only
    // RecordingHandler, so every Skill/Channel daemon fire silently no-op'd.
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    let handler: Arc<dyn wcore_cron::JobHandler> =
        Arc::new(wcore_agent::cron::build_headless_cron_handler(&cwd).await);
    eprintln!("[cron-daemon] headless cron handler initialized (skill + channel sinks wired)");

    // Phase 24 plan 24-02, Task 1 — SCHEDULE OWNERSHIP IS LEASED HERE TOO.
    //
    // This is the OTHER half of the double-fire. Before the lease, a session
    // runner and this daemon both ticked one `jobs.json`, and the only thing
    // between that and a duplicated job was the store's advance-on-fire
    // bookkeeping, which is a read-then-write race rather than a guarantee.
    // Leasing only the session side would have left the race exactly where it
    // was, just with one participant that knew better.
    //
    // A daemon that loses the race still runs: it observes and reports and
    // never fires, so its log, its pid file and its shutdown path stay
    // identical in both roles.
    let lease_dir = wcore_cron::default_lease_dir();
    let lease = match &lease_dir {
        Some(dir) => match wcore_cron::ScheduleLease::attempt(dir, "cron-daemon") {
            Ok(a) => a,
            Err(e) => {
                // Fail CLOSED: an unprovable claim is exactly what the lease
                // exists to refuse, and firing anyway would reinstate the
                // double-fire under a worse name.
                eprintln!("[cron-daemon] schedule ownership could not be evaluated: {e}");
                wcore_cron::LeaseAttempt::Observer { holder_pid: None }
            }
        },
        None => wcore_cron::LeaseAttempt::Observer { holder_pid: None },
    };
    let lease_handle = match &lease {
        wcore_cron::LeaseAttempt::Owner(l) => {
            eprintln!("[cron-daemon] role=owner — this process fires the schedule");
            l.handle()
        }
        wcore_cron::LeaseAttempt::Observer { holder_pid } => {
            eprintln!(
                "[cron-daemon] role=observer — pid {} already owns this schedule; firing nothing",
                holder_pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            );
            wcore_cron::LeaseHandle::observer()
        }
    };

    let mut ticker = tokio::time::interval(wcore_cron::runner::TICK_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // eat the immediate first tick

    // SIGTERM handler via tokio::signal (safe, no raw libc required).
    // On non-Unix, Ctrl+C is the closest equivalent.
    let shutdown = shutdown_signal();

    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                eprintln!("[cron-daemon] shutdown signal received; stopping");
                break;
            }
            _ = ticker.tick() => {
                if let Err(e) = wcore_cron::tick_once_at(
                    &cron_store,
                    &handler,
                    Some(history_path),
                    &lease_handle,
                    chrono::Utc::now(),
                ).await {
                    eprintln!("[cron-daemon] tick error: {e}");
                }
            }
        }
    }

    // Surrender the schedule BEFORE the pid file goes, so a successor that
    // wins the lease cannot briefly see a pid file naming a process that no
    // longer owns anything.
    drop(lease);

    // Remove PID file on graceful exit so a subsequent `cron daemon` start
    // doesn't see a stale entry.
    let _ = std::fs::remove_file(pid_path);
    eprintln!("[cron-daemon] stopped");
    Ok(())
}

/// Returns a future that completes on the first daemon-appropriate shutdown
/// signal: SIGTERM on Unix, Ctrl+C on other platforms.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        } else {
            // Fallback: Ctrl+C.
            let _ = tokio::signal::ctrl_c().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Returns `true` if a process with the given PID appears to be alive.
/// Uses `/proc/<pid>` on Linux or `kill -0` on macOS; uses
/// `OpenProcess` + `GetExitCodeProcess` on Windows.
///
/// Audit W-1 fix (E2E-WINDOWS-ADDENDUM-2026-05-24 §2.2):
/// The previous `#[cfg(not(unix))]` branch returned hardcoded `false`,
/// causing every Windows `cron daemon` invocation to spawn a duplicate
/// daemon because the PID check always reported "dead."
pub(crate) fn process_is_alive(pid: u32) -> bool {
    // ZOMBIE-PROBE lane: every platform arm moved to
    // `wcore_types::process_liveness`, and three defects moved out with them.
    //
    //  * The Linux arm tested `/proc/<pid>` existence, which a **zombie**
    //    satisfies — a cron daemon that had exited without being reaped held
    //    its PID file forever and `cron daemon` refused to start.
    //  * The macOS fallback shelled out to the `kill` binary, which the
    //    slim CI image does not ship (the same ENOENT that took nextest down
    //    in run 26396718138), and which reports a zombie as alive anyway.
    //  * The Windows arm compared against `STILL_ACTIVE` (259), so a process
    //    whose genuine exit code was 259 read as still running. The
    //    centralised arm uses `WaitForSingleObject`, which has no such
    //    ambiguity.
    wcore_types::process_liveness::process_is_alive(pid)
}

#[cfg(test)]
#[cfg(windows)]
mod windows_tests {
    /// Audit W-1 regression guard: process_is_alive() must return true for
    /// the current process on Windows (not the hardcoded-false stub).
    ///
    /// This test would have caught the W-1 bug on any Windows CI run.
    #[test]
    fn process_is_alive_current_process_is_alive() {
        let my_pid = std::process::id();
        assert!(
            super::process_is_alive(my_pid),
            "process_is_alive() returned false for the running process (pid={my_pid}); \
             W-1 regression: the Windows stub returned hardcoded false"
        );
    }

    /// process_is_alive() must return false for a PID that cannot exist.
    #[test]
    fn process_is_alive_invalid_pid_is_dead() {
        // PID 0 is the System Idle Process; OpenProcess with
        // PROCESS_QUERY_LIMITED_INFORMATION will fail → returns false.
        assert!(
            !super::process_is_alive(0),
            "process_is_alive(0) must return false"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use wcore_cron::FileCronStore;

    fn store(dir: &std::path::Path) -> FileCronStore {
        FileCronStore::new(dir.join("jobs.json"))
    }

    #[tokio::test]
    async fn add_slash_round_trip() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        run_with_store(
            CronCmd::Add {
                expression: Some("0 9 * * *".into()),
                trigger: None,
                describe: None,
                confirm: false,
                slash: Some("/morning".into()),
                channel: None,
                text: None,
                skill: None,
                args: None,
            },
            &s,
        )
        .await
        .unwrap();
        let jobs = s.list().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].target, Target::Slash { .. }));
    }

    #[tokio::test]
    async fn add_channel_requires_text() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        let r = run_with_store(
            CronCmd::Add {
                expression: Some("*/15 * * * *".into()),
                trigger: None,
                describe: None,
                confirm: false,
                slash: None,
                channel: Some("team".into()),
                text: None,
                skill: None,
                args: None,
            },
            &s,
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn add_channel_ok() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        run_with_store(
            CronCmd::Add {
                expression: Some("*/15 * * * *".into()),
                trigger: None,
                describe: None,
                confirm: false,
                slash: None,
                channel: Some("team-slack".into()),
                text: Some("status check".into()),
                skill: None,
                args: None,
            },
            &s,
        )
        .await
        .unwrap();
        let jobs = s.list().await.unwrap();
        assert!(matches!(jobs[0].target, Target::Channel { .. }));
    }

    #[tokio::test]
    async fn add_skill_default_args() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        run_with_store(
            CronCmd::Add {
                expression: Some("0 8 * * *".into()),
                trigger: None,
                describe: None,
                confirm: false,
                slash: None,
                channel: None,
                text: None,
                skill: Some("morning-brief".into()),
                args: None,
            },
            &s,
        )
        .await
        .unwrap();
        let jobs = s.list().await.unwrap();
        match &jobs[0].target {
            Target::Skill { name, args } => {
                assert_eq!(name, "morning-brief");
                assert!(args.is_object());
            }
            _ => panic!("expected skill target"),
        }
    }

    #[tokio::test]
    async fn add_no_target_errors() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        let r = run_with_store(
            CronCmd::Add {
                expression: Some("0 9 * * *".into()),
                trigger: None,
                describe: None,
                confirm: false,
                slash: None,
                channel: None,
                text: None,
                skill: None,
                args: None,
            },
            &s,
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn enable_disable_remove() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        run_with_store(
            CronCmd::Add {
                expression: Some("0 9 * * *".into()),
                trigger: None,
                describe: None,
                confirm: false,
                slash: Some("/x".into()),
                channel: None,
                text: None,
                skill: None,
                args: None,
            },
            &s,
        )
        .await
        .unwrap();
        let id = s.list().await.unwrap()[0].id.clone();

        run_with_store(CronCmd::Disable { id: id.clone() }, &s)
            .await
            .unwrap();
        assert!(!s.list().await.unwrap()[0].enabled);

        run_with_store(CronCmd::Enable { id: id.clone() }, &s)
            .await
            .unwrap();
        assert!(s.list().await.unwrap()[0].enabled);

        run_with_store(CronCmd::Remove { id }, &s).await.unwrap();
        assert!(s.list().await.unwrap().is_empty());
    }
}
