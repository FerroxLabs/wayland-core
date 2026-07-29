//! F23-05 — the multi-day wait / resume / complete journey, and the
//! wall-clock-authority determination the clock policy rests on.
//!
//! Three forms live in this one file on purpose, because they must share ONE
//! implementation of the invariants. A second implementation could describe a
//! journey that never happened.
//!
//! * **The accelerated regression form** — [`multi_day_journey_invariants_accelerated`].
//!   It runs the whole cycle against real on-disk durable state inside one test
//!   process at a compressed span, so the invariants are repeatable in CI.
//!   **It is NOT the multi-day evidence.** It never dies as a process and it
//!   never elapses days. The multi-day evidence is the live run log's own first
//!   and last timestamps.
//!
//! * **The live step form** — [`f23_journey_step`]. It executes exactly ONE day
//!   of the journey and then lets the process exit.
//!   `scripts/f23-multi-day-journey.sh` invokes it once per calendar day, so the
//!   gap between two days is real elapsed wall time during which no process of
//!   this journey exists.
//!
//! * **The clock probe step form** — [`f23_clock_probe_step`]. One process arms
//!   durable budget authority and exits; a later process binds it and reports
//!   what the product actually did. `scripts/f23-clock-probe.sh` sequences them
//!   with a REAL gap, so the determination is measured across a real process
//!   death rather than read off a type name.
//!
//! Both live forms are driven by environment variables. With none set they run
//! a self-check against a temporary directory instead of returning early, so
//! neither is a test that passes by doing nothing.
//!
//! ## What this file measured that the plan assumed
//!
//! `BudgetWallClockAuthority::AbsoluteDeadline` has NO production construction
//! site. Every `BudgetAuthoritySeed` built by shipped code — `bootstrap.rs`,
//! `engine.rs`, `recovery.rs`, `spawner.rs`, `tool_budget.rs` — hardcodes
//! `ActiveRuntime`, and `BudgetConfig` exposes no deadline field. The
//! absolute-deadline form is therefore reachable only through this crate's
//! public API, which is what the probe steps below use. That is a real
//! measurement across a real process death against real durable state; it is
//! NOT a claim that the shipped `wayland-core` binary can reach the form, and
//! the evidence records the distinction rather than blurring it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use wcore_agent::budget_authority::{BudgetAuthorityConfig, BudgetAuthorityCoordinator};
use wcore_agent::goal::strategy::{AnvilOutcome, GoalLoop, StrategyTermination};
use wcore_agent::goal::{GoalKernel, GoalLifecycle, GoalRecovery};
use wcore_agent::orchestration::anvil::TerminalState;
use wcore_agent::orchestration::anvil::engine::ClimbOutcome;
use wcore_agent::session_journal::{
    BUDGET_AUTHORITY_SCHEMA_VERSION, BudgetAuthorityCursor, BudgetAuthorityState,
    BudgetWallClockAuthority, CompletionOutcome, DeliveryCompletion, DeliveryOrigin, SessionEvent,
    SessionJournal, state_payload_digest,
};
use wcore_budget::{BudgetCap, BudgetTracker, ExecutionBudget};
use wcore_types::goal::{
    GoalAuthorityRequest, GoalId, GoalStrategy, GoalTerminalState, LoopPolicy, WaitKind,
    resolve_goal_authority,
};

// ── Shared vocabulary ────────────────────────────────────────────────────────

const JOURNEY_SESSION: &str = "f23-multi-day-journey";
const PROBE_SESSION: &str = "f23-clock-probe";
const BUDGET_SESSION_ID: &str = "f23-journey-budget";
const GOAL_PARENT_DIGEST: &str = "f23-journey-parent-v1";
const DELIVERY_ID: &str = "f23-journey-delivery";
/// Wall-time cap the probe arms, so a real gap larger than it is observable as
/// an exhaustion rather than as a number nobody compares.
const PROBE_WALL_CAP_SECS: u64 = 20;
/// Tokens charged on day one, so a later day can observe carry-forward.
const DAY_ONE_TOKENS: u64 = 1_000;

fn now_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the unix epoch")
            .as_millis(),
    )
    .expect("wall clock fits in u64 millis")
}

fn parent_limits() -> BTreeMap<String, u64> {
    [
        ("max_tokens".to_owned(), 4_000_u64),
        ("max_cost_cents".to_owned(), 100_u64),
    ]
    .into_iter()
    .collect()
}

fn goal_request() -> GoalAuthorityRequest {
    GoalAuthorityRequest {
        requested_limits: [
            ("max_tokens".to_owned(), 1_500_u64),
            ("max_cost_cents".to_owned(), 40_u64),
        ]
        .into_iter()
        .collect(),
        strategy: GoalStrategy::Anvil,
        loop_policy: LoopPolicy::Fixed { iterations: 8 },
    }
}

/// The one execution policy the journey and its restores run under. Wall time
/// is deliberately generous: the journey spans days, and the whole point of the
/// `ActiveRuntime` form is that downtime is not charged against it.
fn journey_execution_policy() -> ExecutionBudget {
    ExecutionBudget {
        max_wall_time: Some(Duration::from_secs(6 * 60 * 60)),
        max_tool_runtime: Some(Duration::from_secs(60 * 60)),
        max_processes: Some(4),
        max_agent_depth: Some(4),
        ..ExecutionBudget::default()
    }
}

fn provider_caps() -> BudgetCap {
    BudgetCap::builder()
        .per_session_tokens(100_000)
        .per_session_usd(25.0)
        .build()
}

/// Append the imported baseline every durable authority binding requires.
///
/// The reducer demands this be the FIRST and ONLY import, so the day-one memory
/// fact rides in its messages rather than in a second event.
fn append_baseline(journal: &SessionJournal, session_id: &str, messages: Vec<serde_json::Value>) {
    let session = json!({
        "id": session_id,
        "schema_version": 1,
        "messages": messages,
    });
    journal
        .append(SessionEvent::SessionImported {
            source_schema_version: 1,
            session_digest: state_payload_digest(&session).expect("digest the baseline"),
            session,
        })
        .expect("append the imported baseline");
}

// ── Durable budget authority, armed and restored ─────────────────────────────

/// Build and commit a durable budget authority carrying `wall_clock`.
///
/// `captured_at_unix_millis` is the reservation-time capture. `charged_tokens`
/// is charged against the provider tracker BEFORE the snapshot is taken, so a
/// later process can observe whether cumulative consumption carried forward.
fn arm_authority(
    journal: &SessionJournal,
    wall_clock: BudgetWallClockAuthority,
    execution_policy: ExecutionBudget,
    charged_tokens: u64,
    captured_at_unix_millis: u64,
) {
    let state = journal.state().expect("read reduced state");
    let root = execution_policy.start_root();

    let mut tracker = BudgetTracker::new(provider_caps());
    if charged_tokens > 0 {
        let reservation = tracker
            .reserve(BUDGET_SESSION_ID, charged_tokens, 0.0)
            .expect("reserve within the session cap");
        tracker
            .settle_reservation_conservatively(reservation)
            .expect("settle the reservation at its admitted maximum");
    }

    let authority = BudgetAuthorityState {
        schema_version: BUDGET_AUTHORITY_SCHEMA_VERSION,
        authority_epoch: 1,
        prior_cursor: BudgetAuthorityCursor {
            journal_sequence: state.last_seq,
            journal_checksum: state.last_checksum,
        },
        budget_session_id: BUDGET_SESSION_ID.to_owned(),
        provider_tracker: tracker.snapshot().expect("snapshot the provider tracker"),
        provider_reservations: BTreeMap::new(),
        execution_root: root.snapshot().expect("snapshot the execution root"),
        active_turn: None,
        captured_at_unix_millis,
        wall_clock,
        conversation_digest: state_payload_digest(&serde_json::Value::Array(state.conversation))
            .expect("digest the conversation"),
    };

    journal
        .append(SessionEvent::BudgetAuthorityCommitted { authority })
        .expect("commit the budget authority");
}

/// What a restoring process actually observed. Every field is read off the
/// bound coordinator, never off the record that armed it.
#[derive(Debug)]
struct RestoreObservation {
    exceeded: bool,
    first_exceeded_reason: Option<String>,
    remaining_wall_millis: Option<u128>,
    tokens_charged: u64,
    authority_epoch: u64,
}

fn restore_authority(
    journal: SessionJournal,
    wall_clock: BudgetWallClockAuthority,
    execution_policy: ExecutionBudget,
) -> Result<RestoreObservation, String> {
    let coordinator = BudgetAuthorityCoordinator::bind(BudgetAuthorityConfig {
        journal: Some(journal),
        budget_session_id: BUDGET_SESSION_ID.to_owned(),
        provider_caps: provider_caps(),
        preserve_committed_session_extensions: false,
        execution_policy,
        wall_clock,
        process_cleanup_proof: None,
    })
    .map_err(|error| error.to_string())?;

    let view = coordinator
        .current_execution_view()
        .map_err(|error| error.to_string())?;
    let tokens_charged = coordinator
        .inspect(|tracker, _| tracker.session_totals(BUDGET_SESSION_ID).0)
        .map_err(|error| error.to_string())?;

    Ok(RestoreObservation {
        exceeded: view.is_exceeded(),
        first_exceeded_reason: view.first_exceeded_reason().map(str::to_owned),
        remaining_wall_millis: view.remaining_wall_time().map(|d| d.as_millis()),
        tokens_charged,
        authority_epoch: coordinator.authority_epoch(),
    })
}

// ── The journey's six invariants ─────────────────────────────────────────────

/// One day's observed result. Every field is the OBSERVATION, not the intent.
#[derive(Debug)]
struct DayObservation {
    day: u64,
    loop_owners_observed: usize,
    tokens_charged: u64,
    goal_limits: BTreeMap<String, u64>,
    memory_recalled: bool,
    journal_sequence: u64,
    journal_checksum: String,
    deliveries_completed: usize,
    second_delivery_refused: bool,
    goal_lifecycle: String,
    wait_still_pending: bool,
    terminal_transition: bool,
}

/// Day one's recorded position, carried forward so later days compare against
/// an observation rather than against a constant.
#[derive(Debug, Clone)]
struct DayState {
    tokens_charged: u64,
    goal_limits: BTreeMap<String, u64>,
    journal_sequence: u64,
}

fn journal_path(root: &Path) -> PathBuf {
    root.join("journey.journal")
}

fn goal_id() -> GoalId {
    GoalId::new("f23-journey-goal")
}

/// Terminate this journey's Goal the way the shipped product does.
///
/// F22C criterion 3 makes an engine verdict sayable only by the Goal's loop
/// owner: the reducer refuses `SelfChecked` and every other engine-produced
/// category on the plain `GoalKernel::terminate` path. This journey's Goal is
/// authorized for Anvil, so it claims the one loop owner and hands Anvil's own
/// `ClimbOutcome` to `from_anvil`, which is the only constructor of the
/// `StrategyTermination` the canonical transition consumes.
///
/// The journey's assertions are unchanged — the resulting terminal is byte-for
/// -byte what the raw path used to write. What changed is that this test now
/// exercises the supervised route the product actually takes.
fn terminate_as_anvil_owner(
    kernel: &GoalKernel,
    terminal: TerminalState,
) -> Result<(), Box<dyn std::error::Error>> {
    let driver = GoalLoop::new(kernel.clone());
    let outcome = ClimbOutcome {
        terminal,
        stamp: String::new(),
        checks_passed: 0,
        checks_total: 0,
        iterations: 1,
        valve_fires: 0,
        winner: None,
        best_worktree: None,
        gate_observation: None,
        landing: None,
    };
    tokio::runtime::Builder::new_current_thread()
        .build()?
        .block_on(async {
            driver
                .run_anvil(&goal_id(), |owner| async move {
                    StrategyTermination::from_anvil(owner, AnvilOutcome::Climbed(&outcome), 1)
                })
                .await
        })?;
    Ok(())
}

/// The memory fact written on day one. Recall on a later day means this exact
/// value is still readable from durable state after the process that wrote it
/// stopped existing.
fn memory_fact(nonce: &str) -> String {
    format!("f23-journey-memory-fact-{nonce}")
}

fn memory_present(journal: &SessionJournal, nonce: &str) -> bool {
    let state = journal.state().expect("read reduced state");
    let needle = memory_fact(nonce);
    serde_json::to_string(&state.conversation)
        .map(|blob| blob.contains(&needle))
        .unwrap_or(false)
}

/// Observe how many processes currently own this journey's loop.
///
/// This is deliberately read off RUNTIME state — the exclusive writer lease the
/// journal itself holds — and never off a configuration value. A second owner is
/// the defect the journey exists to catch and it does not appear in
/// configuration. A live foreign holder makes this open fail, so the day step
/// returns an error rather than a count.
fn observe_loop_owners(root: &Path) -> Result<(usize, SessionJournal), String> {
    match SessionJournal::open(journal_path(root), JOURNEY_SESSION) {
        Ok(journal) => Ok((1, journal)),
        Err(error) => Err(format!(
            "journal writer lease refused; another loop owner may be live: {error}"
        )),
    }
}

/// Day one. Opens the journey: a durable Goal parked on a wait that only real
/// elapsed time can satisfy, a memory fact, a cumulative budget charge under
/// the authority form the clock policy named, and a delegated result delivered
/// exactly once.
fn journey_open(root: &Path, nonce: &str, span_secs: u64) -> Result<DayObservation, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let (owners, journal) = observe_loop_owners(root)?;

    // The memory fact rides in the one permitted baseline. NOTE: this is the
    // durable session journal, NOT the wcore-memory SQLite store; the evidence
    // records that distinction rather than blurring it.
    append_baseline(
        &journal,
        JOURNEY_SESSION,
        vec![json!({ "role": "user", "content": memory_fact(nonce) })],
    );

    // The delegated result, delivered exactly once. `Cron` origin needs no
    // active turn, which keeps the journey free of a nonterminal turn that
    // would block the Goal's own terminal transition.
    journal
        .append(SessionEvent::DeliveryPrepared {
            delivery_id: DELIVERY_ID.to_owned(),
            origin: DeliveryOrigin::Cron {
                schedule_id: "f23-journey-schedule".to_owned(),
                fire_id: format!("f23-journey-fire-{nonce}"),
            },
            destination: "f23-journey-sink".to_owned(),
            payload: json!({ "nonce": nonce }),
        })
        .map_err(|e| format!("prepare the delivery: {e}"))?;
    journal
        .append(SessionEvent::DeliveryStarted {
            delivery_id: DELIVERY_ID.to_owned(),
        })
        .map_err(|e| format!("start the delivery: {e}"))?;
    journal
        .append(SessionEvent::DeliveryFinished {
            delivery_id: DELIVERY_ID.to_owned(),
            completion: DeliveryCompletion::Confirmed {
                outcome: CompletionOutcome::Succeeded,
                receipt: json!({ "nonce": nonce }),
            },
        })
        .map_err(|e| format!("finish the delivery: {e}"))?;

    // The durable budget authority. `ActiveRuntime` is the only form the
    // shipped product constructs, and the probe measures whether it charges
    // downtime, so a multi-day gap must not silently consume this envelope.
    arm_authority(
        &journal,
        BudgetWallClockAuthority::ActiveRuntime,
        journey_execution_policy(),
        DAY_ONE_TOKENS,
        now_millis(),
    );

    // The Goal, parked on a wait whose condition is real elapsed time.
    let snapshot = resolve_goal_authority(&goal_request(), &parent_limits(), GOAL_PARENT_DIGEST);
    let kernel = GoalKernel::new(journal.clone());
    kernel
        .open_goal(
            &goal_id(),
            &format!("complete after {span_secs}s of real elapsed time"),
            &snapshot,
            now_millis(),
        )
        .map_err(|e| format!("open the goal: {e}"))?;
    kernel
        .start_iteration(&goal_id())
        .map_err(|e| format!("start iteration one: {e}"))?;
    kernel
        .begin_wait(
            &goal_id(),
            WaitKind::Event {
                event: format!("f23-span-elapsed-{span_secs}s"),
            },
        )
        .map_err(|e| format!("begin the wait: {e}"))?;

    let mut observation = observe_day(&journal, 1, owners, nonce, false, false)?;
    observation.tokens_charged = DAY_ONE_TOKENS;
    Ok(observation)
}

/// Every later day. Resumes through recovery in a process that shares nothing
/// with day one's, re-asserts every invariant over observed runtime state, and
/// completes the wait only on the day its condition is actually met.
fn journey_resume(
    root: &Path,
    day: u64,
    nonce: &str,
    condition_met: bool,
) -> Result<DayObservation, String> {
    let (owners, journal) = observe_loop_owners(root)?;

    // Restore the durable budget authority in this NEW process. The handle is
    // CLONED rather than reopened: the writer lease is exclusive, and a second
    // open would report a lease conflict against ourselves.
    let restored = restore_authority(
        journal.clone(),
        BudgetWallClockAuthority::ActiveRuntime,
        journey_execution_policy(),
    )?;
    if restored.exceeded {
        return Err(format!(
            "authority envelope was consumed across downtime: reason={:?}",
            restored.first_exceeded_reason
        ));
    }
    if restored.authority_epoch == 0 {
        return Err("restored authority epoch is zero: nothing durable was bound".to_owned());
    }

    let kernel = GoalKernel::new(journal.clone());
    let recovery = kernel
        .recover_with_parent_envelope(&goal_id(), GOAL_PARENT_DIGEST)
        .map_err(|e| format!("recover the goal: {e}"))?;
    let resumed_limits = match &recovery {
        GoalRecovery::Resumed { snapshot, .. } => snapshot.effective_limits.clone(),
        GoalRecovery::AlreadyTerminal { terminal } => {
            return Err(format!(
                "goal was already terminal on day {day}: {terminal:?}"
            ));
        }
        GoalRecovery::Blocked { terminal } => {
            return Err(format!("goal was blocked on day {day}: {terminal:?}"));
        }
    };

    let mut terminal = false;
    if condition_met {
        kernel
            .resume_from_wait(&goal_id())
            .map_err(|e| format!("resume from the wait: {e}"))?;
        // F22C criterion 3: `SelfChecked` is an ENGINE VERDICT, so it is only
        // sayable by the Goal's loop owner. This journey's Goal is authorized
        // for Anvil (see `snapshot`), so it terminates the way the product
        // does — claim the one loop owner, hand the engine's real
        // `ClimbOutcome` to `from_anvil`, and let the canonical transition
        // write it. The asserted terminal is unchanged; only the route to it is
        // now the sanctioned one, which is the point of the criterion.
        terminate_as_anvil_owner(&kernel, TerminalState::SelfChecked)
            .map_err(|e| format!("terminate the goal: {e}"))?;
        terminal = true;
    }

    let mut observation = observe_day(&journal, day, owners, nonce, terminal, true)?;
    observation.goal_limits = resumed_limits;
    observation.tokens_charged = restored.tokens_charged;
    Ok(observation)
}

/// Re-observe the finished journey without transitioning it.
///
/// This is what each platform's `--verify` runs. It re-binds the durable budget
/// authority in a fresh process, re-reads the Goal, and re-asserts every
/// invariant against day one's recorded position — so a platform claim rests on
/// a command that actually executed on that platform, not on a file this plan
/// wrote.
fn journey_verify(root: &Path, nonce: &str) -> Result<DayObservation, String> {
    let (owners, journal) = observe_loop_owners(root)?;

    let restored = restore_authority(
        journal.clone(),
        BudgetWallClockAuthority::ActiveRuntime,
        journey_execution_policy(),
    )?;
    if restored.exceeded {
        return Err(format!(
            "authority envelope was consumed across downtime: reason={:?}",
            restored.first_exceeded_reason
        ));
    }

    let state = journal.state().map_err(|e| e.to_string())?;
    let goal = state
        .goals
        .get(goal_id().as_str())
        .ok_or_else(|| "no goal record to verify".to_owned())?;
    let terminal = matches!(goal.lifecycle, GoalLifecycle::Terminated { .. });

    let mut observation = observe_day(&journal, 0, owners, nonce, terminal, true)?;
    observation.goal_limits = goal.authority.effective_limits.clone();
    observation.tokens_charged = restored.tokens_charged;
    Ok(observation)
}

/// Read every invariant off the journal AFTER the day's work.
fn observe_day(
    journal: &SessionJournal,
    day: u64,
    loop_owners_observed: usize,
    nonce: &str,
    terminal_transition: bool,
    probe_delivery_uniqueness: bool,
) -> Result<DayObservation, String> {
    // Exactly-once is proved POSITIVELY: a second completion must be REFUSED by
    // the reducer, not merely absent. A count alone cannot distinguish
    // "delivered once" from "the second attempt was never made". Attempted
    // BEFORE the final state read so the refusal cannot perturb the counts.
    let second_delivery_refused = if probe_delivery_uniqueness {
        journal
            .append(SessionEvent::DeliveryFinished {
                delivery_id: DELIVERY_ID.to_owned(),
                completion: DeliveryCompletion::Confirmed {
                    outcome: CompletionOutcome::Succeeded,
                    receipt: json!({ "duplicate": true }),
                },
            })
            .is_err()
    } else {
        true
    };

    let state = journal.state().map_err(|e| e.to_string())?;

    let deliveries_completed = state
        .deliveries
        .values()
        .filter(|delivery| delivery.completion.is_some())
        .count();

    let goal = state
        .goals
        .get(goal_id().as_str())
        .ok_or_else(|| format!("no goal record on day {day}"))?;
    let lifecycle = format!("{:?}", goal.lifecycle);
    let wait_still_pending = matches!(goal.lifecycle, GoalLifecycle::Waiting { .. });
    let goal_limits = goal.authority.effective_limits.clone();

    Ok(DayObservation {
        day,
        loop_owners_observed,
        tokens_charged: 0,
        goal_limits,
        memory_recalled: memory_present(journal, nonce),
        journal_sequence: state.last_seq.unwrap_or(0),
        journal_checksum: state.last_checksum.clone(),
        deliveries_completed,
        second_delivery_refused,
        goal_lifecycle: lifecycle,
        wait_still_pending,
        terminal_transition,
    })
}

/// An envelope may only ever narrow. Empty on either side is a FAILURE, not a
/// vacuous pass — an assertion over nothing proves nothing.
fn envelope_not_widened(day_one: &BTreeMap<String, u64>, today: &BTreeMap<String, u64>) -> bool {
    if day_one.is_empty() || today.is_empty() {
        return false;
    }
    today
        .iter()
        .all(|(key, value)| day_one.get(key).is_some_and(|first| value <= first))
        && day_one.keys().all(|key| today.contains_key(key))
}

/// Emit one day's invariant results in the machine-readable form the gates read.
///
/// Returns whether every invariant passed, so the caller can fail the step
/// rather than print a FAIL line and exit zero.
fn emit_day(
    platform: &str,
    observation: &DayObservation,
    host: &str,
    day_one: Option<&DayState>,
) -> bool {
    let day = observation.day;
    println!(
        "F23_04_DAY={day} platform={platform} ts={} host={host} pid={}",
        iso_now(),
        std::process::id()
    );

    let mut all = true;
    let mut invariant = |name: &str, ok: bool| {
        println!(
            "F23_04_INVARIANT={name} platform={platform} day={day} status={}",
            if ok { "PASS" } else { "FAIL" }
        );
        all &= ok;
    };

    invariant("loop-owner", observation.loop_owners_observed == 1);
    invariant(
        "cumulative-budget",
        day_one.is_none_or(|first| observation.tokens_charged >= first.tokens_charged),
    );
    invariant(
        "authority-envelope",
        day_one
            .is_none_or(|first| envelope_not_widened(&first.goal_limits, &observation.goal_limits)),
    );
    invariant("memory-recall", observation.memory_recalled);
    invariant(
        "evidence-chain",
        day_one.is_none_or(|first| observation.journal_sequence > first.journal_sequence),
    );
    invariant(
        "delivery-once",
        observation.deliveries_completed == 1 && observation.second_delivery_refused,
    );

    println!(
        "F23_04_LOOP_OWNERS_OBSERVED={}",
        observation.loop_owners_observed
    );
    println!("F23_04_GOAL_LIFECYCLE={}", observation.goal_lifecycle);
    println!(
        "F23_04_JOURNAL_CURSOR=seq={} checksum={}",
        observation.journal_sequence, observation.journal_checksum
    );
    println!("F23_04_DAY_INVARIANTS_ALL_PASS={all}");
    all
}

fn iso_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after epoch")
        .as_secs();
    let days = i64::try_from(secs / 86_400).expect("days fit");
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm. A test may not add a dependency
/// for one timestamp.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = u64::try_from(z - era * 146_097).expect("day-of-era is non-negative");
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = i64::try_from(yoe).expect("year-of-era fits") + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).expect("day fits");
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).expect("month fits");
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ── Live step: one day of the journey, then the process exits ────────────────

/// Executes exactly ONE day of the multi-day journey.
///
/// Driven by `scripts/f23-multi-day-journey.sh`:
///   `F23_JOURNEY_ROOT`           the journey state root (persists across days)
///   `F23_JOURNEY_DAY`            1 for the opening day, 2.. for each resume
///   `F23_JOURNEY_NONCE`          the caller's run nonce, planted on day one
///   `F23_JOURNEY_SPAN_SECONDS`   the wait's condition: real elapsed seconds
///   `F23_JOURNEY_PLATFORM`       linux | macos | windows
///   `F23_JOURNEY_HOST`           the host identity stamped into each day record
///
/// With `F23_JOURNEY_ROOT` unset it runs a full self-check against a temporary
/// directory, so this test is never a no-op.
#[test]
fn f23_journey_step() {
    let Ok(root) = std::env::var("F23_JOURNEY_ROOT") else {
        let dir = tempfile::tempdir().expect("tempdir");
        run_full_cycle(dir.path(), "selfcheck", 0);
        return;
    };

    let root = PathBuf::from(root);
    let verify = std::env::var("F23_JOURNEY_MODE").as_deref() == Ok("verify");
    let day: u64 = if verify {
        0
    } else {
        std::env::var("F23_JOURNEY_DAY")
            .expect("F23_JOURNEY_DAY")
            .parse()
            .expect("F23_JOURNEY_DAY is a number")
    };
    let nonce = std::env::var("F23_JOURNEY_NONCE").expect("F23_JOURNEY_NONCE");
    let span_secs: u64 = std::env::var("F23_JOURNEY_SPAN_SECONDS")
        .expect("F23_JOURNEY_SPAN_SECONDS")
        .parse()
        .expect("F23_JOURNEY_SPAN_SECONDS is a number");
    let platform = std::env::var("F23_JOURNEY_PLATFORM").expect("F23_JOURNEY_PLATFORM");
    let host = std::env::var("F23_JOURNEY_HOST").unwrap_or_else(|_| "unknown".to_owned());

    let day_one = load_day_one(&root);

    let observation = if verify {
        let observed = journey_verify(&root, &nonce).expect("verify the journey");
        println!(
            "F23_04_TERMINAL_GOAL_TRANSITION={}",
            observed.terminal_transition
        );
        let first = day_one
            .clone()
            .expect("a journey with no recorded day one cannot be verified");
        let elapsed = now_millis().saturating_sub(first.opened_at_millis) / 1000;
        println!("F23_04_WAIT_CONDITION_ELAPSED_SECONDS={elapsed} required={span_secs}");
        println!(
            "F23_04_WAIT_COMPLETED_ON_CONDITION={}",
            observed.terminal_transition && elapsed >= span_secs
        );
        assert!(
            observed.terminal_transition,
            "the journey has not reached its terminal Goal transition"
        );
        assert!(
            elapsed >= span_secs,
            "the journey's own day-one record shows {elapsed}s elapsed, \
             short of the {span_secs}s its wait condition requires"
        );
        observed
    } else if day == 1 {
        assert!(
            day_one.is_none(),
            "day one has already been recorded under {}; refusing to double-count",
            root.display()
        );
        journey_open(&root, &nonce, span_secs).expect("open the journey")
    } else {
        let first = day_one
            .clone()
            .expect("day one must have been recorded before a resume");
        // The wait's condition is REAL elapsed time, so it can only be met on
        // the day it is actually met — never on the first resume after day one.
        let elapsed = now_millis().saturating_sub(first.opened_at_millis) / 1000;
        let condition_met = elapsed >= span_secs;
        println!("F23_04_WAIT_CONDITION_ELAPSED_SECONDS={elapsed} required={span_secs}");
        println!("F23_04_WAIT_CONDITION_MET={condition_met}");
        journey_resume(&root, day, &nonce, condition_met).expect("resume the journey")
    };

    let all_passed = emit_day(
        &platform,
        &observation,
        &host,
        day_one.as_ref().map(|first| &first.state),
    );

    if day == 1 && !verify {
        persist_day_one(&root, &observation);
    }

    assert!(all_passed, "an invariant failed on day {day}");
    println!("F23_04_STEP=OK day={day} platform={platform} nonce={nonce}");
}

#[derive(Debug, Clone)]
struct DayOneRecord {
    opened_at_millis: u64,
    state: DayState,
}

fn day_one_path(root: &Path) -> PathBuf {
    root.join("day-one.json")
}

fn load_day_one(root: &Path) -> Option<DayOneRecord> {
    let raw = std::fs::read_to_string(day_one_path(root)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(DayOneRecord {
        opened_at_millis: value["opened_at_millis"].as_u64()?,
        state: DayState {
            tokens_charged: value["tokens_charged"].as_u64()?,
            goal_limits: serde_json::from_value(value["goal_limits"].clone()).ok()?,
            journal_sequence: value["journal_sequence"].as_u64()?,
        },
    })
}

fn persist_day_one(root: &Path, observation: &DayObservation) {
    let value = json!({
        "opened_at_millis": now_millis(),
        "tokens_charged": observation.tokens_charged,
        "goal_limits": observation.goal_limits,
        "journal_sequence": observation.journal_sequence,
    });
    std::fs::write(
        day_one_path(root),
        serde_json::to_string_pretty(&value).expect("serialize day one"),
    )
    .expect("persist day one");
}

// ── The accelerated regression form ──────────────────────────────────────────

/// Runs the whole open / resume-still-waiting / resume-and-complete cycle
/// against real on-disk durable state at a compressed span.
///
/// **This is the regression form, not the multi-day evidence.** It proves the
/// invariants hold over persisted state; it does not prove the process stopped
/// existing for days, which only the live run log's own timestamps can show.
#[test]
fn multi_day_journey_invariants_accelerated() {
    let dir = tempfile::tempdir().expect("tempdir");
    run_full_cycle(dir.path(), "accel", 2);
}

fn run_full_cycle(root: &Path, nonce: &str, span_secs: u64) {
    let day_one = journey_open(root, nonce, span_secs).expect("day one opens");
    assert_eq!(day_one.loop_owners_observed, 1, "exactly one loop owner");
    assert!(day_one.memory_recalled, "the day-one fact is durable");
    assert_eq!(day_one.deliveries_completed, 1, "delivered exactly once");
    assert!(day_one.wait_still_pending, "day one parks on the wait");
    assert!(!day_one.terminal_transition);
    assert!(
        !day_one.goal_limits.is_empty(),
        "the authority envelope must be recorded, not empty"
    );
    let first = DayState {
        tokens_charged: day_one.tokens_charged,
        goal_limits: day_one.goal_limits.clone(),
        journal_sequence: day_one.journal_sequence,
    };
    assert_eq!(
        first.tokens_charged, DAY_ONE_TOKENS,
        "day one charged the tokens a later day must still see"
    );

    // Day two: the condition is NOT met, so the wait must still be pending.
    let day_two = journey_resume(root, 2, nonce, false).expect("day two resumes");
    assert_eq!(day_two.loop_owners_observed, 1, "still one loop owner");
    assert!(
        day_two.wait_still_pending,
        "a wait whose condition is unmet must not complete on the first resume"
    );
    assert!(day_two.memory_recalled, "day-one memory survives the gap");
    assert!(
        day_two.journal_sequence > first.journal_sequence,
        "the evidence chain advanced rather than restarting"
    );
    assert!(
        day_two.second_delivery_refused,
        "a second delivery completion must be refused by the reducer"
    );
    assert_eq!(
        day_two.deliveries_completed, 1,
        "still delivered exactly once"
    );
    assert!(
        envelope_not_widened(&first.goal_limits, &day_two.goal_limits),
        "the authority envelope must be no wider than day one: {:?} -> {:?}",
        first.goal_limits,
        day_two.goal_limits
    );
    assert!(
        day_two.tokens_charged >= first.tokens_charged,
        "cumulative provider consumption must carry forward, not reset: {} < {}",
        day_two.tokens_charged,
        first.tokens_charged
    );

    // Let the condition become genuinely true, then complete on that day.
    if span_secs > 0 {
        std::thread::sleep(Duration::from_secs(span_secs + 1));
    }
    let day_three = journey_resume(root, 3, nonce, true).expect("day three completes");
    assert!(
        day_three.terminal_transition,
        "the journey completes through a real terminal Goal transition"
    );
    assert!(
        !day_three.wait_still_pending,
        "the wait is no longer pending once its condition is met"
    );
    assert_eq!(day_three.deliveries_completed, 1, "no duplicate delivery");
    assert!(
        day_three.journal_sequence > day_two.journal_sequence,
        "the evidence chain is continuous across every resume"
    );

    // The read-only re-observation each platform's `--verify` runs.
    persist_day_one(root, &day_one);
    let verified = journey_verify(root, nonce).expect("verify the finished journey");
    assert!(
        verified.terminal_transition,
        "verify must observe the terminal Goal transition"
    );
    assert_eq!(verified.deliveries_completed, 1, "verify sees one delivery");
    assert!(verified.memory_recalled, "verify recalls the day-one fact");
    assert!(
        envelope_not_widened(&first.goal_limits, &verified.goal_limits),
        "verify must observe an unwidened envelope"
    );
}

// ── Clock probe steps ────────────────────────────────────────────────────────

/// One step of the wall-clock authority determination.
///
/// Driven by `scripts/f23-clock-probe.sh`:
///   `F23_PROBE_ROOT`  directory holding the armed journal
///   `F23_PROBE_MODE`  `arm` or `restore`
///   `F23_PROBE_FORM`  `absolute-deadline` or `active-runtime`
///   `F23_PROBE_TAG`   distinguishes experiment A from its control B
///
/// With `F23_PROBE_MODE` unset it self-checks both forms against a temporary
/// directory, so this test is never a no-op.
#[test]
fn f23_clock_probe_step() {
    let Ok(mode) = std::env::var("F23_PROBE_MODE") else {
        let dir = tempfile::tempdir().expect("tempdir");
        // Arm and immediately restore both forms; with no real gap neither may
        // be exceeded. This is the self-check, NOT the determination — the
        // determination needs a real gap, which only the script can supply.
        for form in ["absolute-deadline", "active-runtime"] {
            let root = dir.path().join(form);
            std::fs::create_dir_all(&root).expect("mkdir");
            probe_arm(&root, form);
            let observed = probe_restore(&root, form).expect("restore the armed authority");
            assert!(
                !observed.exceeded,
                "{form} must not be exceeded with no elapsed gap: {observed:?}"
            );
            assert_eq!(
                observed.tokens_charged, 500,
                "{form} must carry its armed provider consumption across the restore"
            );
        }
        return;
    };

    let root = PathBuf::from(std::env::var("F23_PROBE_ROOT").expect("F23_PROBE_ROOT"));
    let form = std::env::var("F23_PROBE_FORM").expect("F23_PROBE_FORM");
    let tag = std::env::var("F23_PROBE_TAG").unwrap_or_else(|_| "untagged".to_owned());

    match mode.as_str() {
        "arm" => {
            std::fs::create_dir_all(&root).expect("mkdir");
            let captured_at = probe_arm(&root, &form);
            println!(
                "F23_04_PROBE_ARMED form={form} tag={tag} captured_at_unix_millis={captured_at} \
                 wall_cap_secs={PROBE_WALL_CAP_SECS} pid={}",
                std::process::id()
            );
        }
        "restore" => match probe_restore(&root, &form) {
            Ok(observed) => {
                println!(
                    "F23_04_PROBE_RESTORED form={form} tag={tag} exceeded={} reason={} \
                     remaining_wall_millis={:?} tokens_charged={} pid={}",
                    observed.exceeded,
                    observed.first_exceeded_reason.as_deref().unwrap_or("none"),
                    observed.remaining_wall_millis,
                    observed.tokens_charged,
                    std::process::id()
                );
            }
            Err(error) => {
                // A refusal is a MEASURED outcome, not a skip. It is printed and
                // then failed, so the script sees both the reason and a non-zero
                // status.
                println!("F23_04_PROBE_REFUSED form={form} tag={tag} error={error}");
                panic!("probe restore refused: {error}");
            }
        },
        other => panic!("unknown F23_PROBE_MODE {other}"),
    }
}

fn probe_policy() -> ExecutionBudget {
    ExecutionBudget {
        max_wall_time: Some(Duration::from_secs(PROBE_WALL_CAP_SECS)),
        max_tool_runtime: Some(Duration::from_secs(PROBE_WALL_CAP_SECS)),
        max_processes: Some(2),
        max_agent_depth: Some(2),
        ..ExecutionBudget::default()
    }
}

fn probe_wall_clock(form: &str, captured_at: u64) -> BudgetWallClockAuthority {
    match form {
        "absolute-deadline" => BudgetWallClockAuthority::AbsoluteDeadline {
            deadline_unix_millis: captured_at + PROBE_WALL_CAP_SECS * 1_000,
        },
        "active-runtime" => BudgetWallClockAuthority::ActiveRuntime,
        other => panic!("unknown F23_PROBE_FORM {other}"),
    }
}

fn probe_journal_path(root: &Path) -> PathBuf {
    root.join("probe.journal")
}

fn probe_arm(root: &Path, form: &str) -> u64 {
    let journal =
        SessionJournal::open(probe_journal_path(root), PROBE_SESSION).expect("open probe journal");
    append_baseline(&journal, PROBE_SESSION, Vec::new());
    let captured_at = now_millis();
    arm_authority(
        &journal,
        probe_wall_clock(form, captured_at),
        probe_policy(),
        500,
        captured_at,
    );
    captured_at
}

fn probe_restore(root: &Path, form: &str) -> Result<RestoreObservation, String> {
    let journal =
        SessionJournal::open(probe_journal_path(root), PROBE_SESSION).map_err(|e| e.to_string())?;
    // The armed capture time is the one the DURABLE record carries; the config
    // only supplies the semantic variant, which is what `restore` checks. Read
    // it back rather than recomputing, so the restore cannot accidentally arm a
    // deadline relative to its own `now`.
    let captured_at = journal
        .state()
        .map_err(|e| e.to_string())?
        .budget_authority
        .as_ref()
        .map(|authority| authority.captured_at_unix_millis)
        .ok_or_else(|| "no durable budget authority was armed".to_owned())?;
    restore_authority(journal, probe_wall_clock(form, captured_at), probe_policy())
}
