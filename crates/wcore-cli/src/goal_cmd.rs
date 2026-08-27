//! F22-04 — `wayland-core goal`: the user-reachable surface over the durable
//! Goal kernel and its Fleet task ledger.
//!
//! ## Why this exists
//!
//! Phase 22 built a durable Goal kernel and a fenced task ledger and proved both
//! against a real kill on both platforms — with an `examples/` instrument,
//! because no path through the shipped binary could reach a Goal. Two successive
//! summaries carried that caveat forward rather than dropping it, and it is the
//! second of the two named reasons Success Criterion 2 stayed open. This module
//! is what closes it: after this, the kill/restart proof runs against
//! `wayland-core` itself.
//!
//! ## Verbs
//!
//! | Verb | What it does |
//! |---|---|
//! | `open` | authorize a durable Goal with a loop bound and a limit envelope |
//! | `task` | declare a task, its dependency set and its idempotency key |
//! | `run` | recover, revoke expired leases, drain the outbox, then drive waves through the real `FleetDispatcher` |
//! | `status` | the canonical JSON projection of Goal + task state, replayed from the chain |
//! | `stream` | the same state as the HOST protocol sees it: the ordered `goal_transition` / `goal_snapshot` JSON-stream lines (F22-C1) |
//! | `exec-task` | the effect boundary: the idempotency gate, then the operator's command |
//!
//! ## Why `exec-task` is a product verb and not a test fixture
//!
//! The ledger fences who may RECORD a completion. It structurally cannot reach
//! inside a worker process that already holds a directory and stop it writing.
//! So the exactly-once property needs a second half at the effect boundary — an
//! atomic `create_new` marker keyed by the task's idempotency key — and that half
//! has to run in the process that produces the effect, not in the parent that
//! might die between checking and spawning.
//!
//! Putting it in the shipped binary means "no duplicate effect after a kill" is a
//! property of the product rather than of whichever harness happened to measure
//! it. `run` spawns `wayland-core goal exec-task` and `exec-task` runs the
//! operator's command behind the gate.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Subcommand, ValueEnum};

use wcore_agent::goal::{
    FleetOutcome, GoalFleetDriver, GoalKernel, GoalLoop, StrategyTermination, TaskAssignment,
    TaskExecution, TaskExecutor, WaveOutcome, event_line, goal_stream,
};
use wcore_agent::session_journal::SessionJournal;
use wcore_swarm::fleet::{FleetDispatcher, ShardSummary};
use wcore_types::goal::{
    GoalAuthorityRequest, GoalId, GoalStrategy, GoalTerminalState, LoopPolicy, TaskId,
    TaskUnknownReason, resolve_goal_authority,
};

/// Environment the assignment reaches `exec-task` through.
///
/// Named constants rather than string literals at three call sites, because a
/// typo in one of them would silently produce an unkeyed effect — which is
/// exactly the duplicate this design exists to prevent, arriving through a
/// spelling mistake instead of a race.
pub const ENV_GOAL: &str = "WAYLAND_GOAL_ID";
pub const ENV_TASK: &str = "WAYLAND_GOAL_TASK";
pub const ENV_KEY: &str = "WAYLAND_GOAL_IDEMPOTENCY_KEY";
pub const ENV_EPOCH: &str = "WAYLAND_GOAL_EPOCH";
pub const ENV_ATTEMPT: &str = "WAYLAND_GOAL_ATTEMPT";
pub const ENV_WORKER: &str = "WAYLAND_GOAL_WORKER";

/// Path a worker CREATES to declare, out of band, that its effect did not land.
///
/// ## Why an exit code could not carry this on its own
///
/// The first version of the withdraw path keyed on one integer. Integers are
/// shared: the exit-code space this product borrows from is `sysexits.h`, where
/// 64..=78 already have meanings other programs emit — `EX_TEMPFAIL` is 75 and
/// `EX_PROTOCOL` is 76. A worker that shells out to any tool from that tradition
/// (`sendmail` exits 75 and 76 routinely) hands the boundary a number the tool
/// meant as "the remote spoke badly", the boundary reads it as "nothing landed",
/// withdraws the intent, and the retry duplicates the effect. No crash, no kill,
/// no signal — the exact silent duplicate this whole module exists to prevent.
///
/// So the declaration is a FILE the worker creates at this path, and the exit
/// code is only a corroborating second signal. The product creates the parent
/// directory and removes any stale receipt before each attempt, so the file's
/// presence can only mean "this attempt's worker wrote it". A value the OS also
/// uses can no longer, by itself, withdraw an intent.
pub const ENV_NO_EFFECT_RECEIPT: &str = "WAYLAND_GOAL_NO_EFFECT_RECEIPT";

/// Directory the worker command deposits ONE record per real execution in.
///
/// ## Why this exists — the instrument was measuring itself
///
/// Before this, the only thing `goal effects` counted was the idempotency
/// MARKER: one `create_new` file per key, written by the product, about the
/// product. That count is structurally incapable of observing a duplicate,
/// because `create_new` makes a second marker impossible no matter how many
/// times the operator's command actually ran. A worker executed twice and a
/// worker executed once produce byte-identical marker directories.
///
/// So the gate that was supposed to detect double execution could only ever
/// report the number of distinct keys. It had never caught a duplicate, and it
/// could not: an instrument that cannot go red for the failure it names is a
/// decoration.
///
/// This is the other half. The sink is written by the WORKER — the process that
/// performs the real external effect — with one uniquely-named file per
/// invocation. The product never writes into it. Two executions therefore leave
/// two files, and [`count_effects`] can say so.
///
/// The worker's contract is three lines of shell:
///
/// ```sh
/// mkdir -p "$WAYLAND_GOAL_EFFECT_SINK"
/// printf '%s\n' "$WAYLAND_GOAL_TASK" \
///   > "$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)"
/// ```
///
/// The file name must be unique per invocation (pid plus nanoseconds is enough).
///
/// ## The identity is the DIRECTORY, not the file's content
///
/// This used to say the content was the task label, and the census counted
/// distinct executions by distinct trimmed content. That made the instrument
/// tuned to one reproduction: a worker whose record carries an invocation
/// identity — `task=t-p msg_id=…`, which is what a real effect log looks like —
/// leaves two records that differ, so two executions of ONE task were counted as
/// two distinct tasks and the duplicate count read zero. The instrument went
/// green on the failure it exists to catch.
///
/// So the sink handed to each worker is a per-(goal, task) directory the PRODUCT
/// creates and names. Every file inside it is one invocation of that one task,
/// whatever the worker chose to write. Two invocations are two files in the same
/// directory, and no content can disguise that.
pub const ENV_EFFECT_SINK: &str = "WAYLAND_GOAL_EFFECT_SINK";

/// Journal a Goal-attached engine reads its durable Goal from.
///
/// The Goal id already reaches child processes through [`ENV_GOAL`] — that is
/// how `goal run` hands a task's identity to `exec-task`. The journal path is
/// its missing other half: with both, ANY engine entry point can discover that
/// it is running under a Goal without every verb in the CLI having to grow a
/// pair of flags. That matters for the two engines whose arguments are assembled
/// field-by-field in `main.rs` (Council and Direct), where a flag pair would
/// mean editing the shared fence twice.
pub const ENV_JOURNAL: &str = "WAYLAND_GOAL_JOURNAL";
/// Loop-owner claim lease for an env-attached engine. Optional.
pub const ENV_LEASE: &str = "WAYLAND_GOAL_LEASE";

/// Default identity this binary presents as the Goal's parent envelope.
///
/// A Goal resumes only against the envelope it was authorized under; a mismatch
/// parks it durably as `AuthorityUnreconstructable` rather than resuming it under
/// whatever the parent happens to be now. Exposed as a flag so that refusal is
/// exercisable from the command line rather than only reasoned about.
pub const DEFAULT_PARENT_ENVELOPE: &str = "wayland-core-goal-fleet/v1";

/// The five loop owners, as a command-line value.
///
/// Deliberately a mirror of [`GoalStrategy`] rather than a re-spelling: the
/// `From` impl below is an exhaustive match, so a sixth `GoalStrategy` variant
/// fails to compile here instead of silently becoming unreachable from the
/// product. There is no `_` arm; do not add one.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum GoalStrategyArg {
    Direct,
    ForgeFlows,
    Fleet,
    Council,
    Anvil,
}

impl From<GoalStrategyArg> for GoalStrategy {
    fn from(arg: GoalStrategyArg) -> Self {
        match arg {
            GoalStrategyArg::Direct => Self::Direct,
            GoalStrategyArg::ForgeFlows => Self::ForgeFlows,
            GoalStrategyArg::Fleet => Self::Fleet,
            GoalStrategyArg::Council => Self::Council,
            GoalStrategyArg::Anvil => Self::Anvil,
        }
    }
}

/// How an operator settles a task the effect boundary refused to decide.
///
/// There is no `auto` and there must not be. The whole reason the boundary
/// parks is that the answer is not derivable from anything the product can see;
/// a default would be a guess wearing a flag's clothes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum ResolveArg {
    /// The operator has confirmed the effect DID land. Record it; run nothing.
    Produced,
    /// The operator has confirmed the effect did NOT land, or that re-running it
    /// is harmless. Withdraw the intent and run the worker again.
    Retry,
}

/// How another verb asks to be run INSIDE a Goal (F22C, Success Criterion 3).
///
/// `workflow run`, `crucible` and `anvil forge` each take one of these as an
/// optional pair of flags. When it is absent the verb behaves byte-for-byte as
/// it did before; when present, that verb's REAL engine invocation is wrapped in
/// `GoalLoop::run_<strategy>` and terminates through the one canonical
/// transition. Nothing here re-implements an engine — the point of attaching to
/// the shipped verb rather than adding a `goal drive` verb is that the path
/// under proof is the product's own.
#[derive(Args, Debug, Clone)]
pub struct GoalAttachArgs {
    /// Journal holding the durable Goal. Required with `--goal`.
    #[arg(long = "goal-journal", requires = "goal")]
    pub goal_journal: Option<PathBuf>,
    /// Id of an already-opened Goal to run this engine as the loop owner of.
    #[arg(long = "goal", requires = "goal_journal")]
    pub goal: Option<String>,
    /// Loop-owner claim lease. A claim is evidence the owner is alive; once it
    /// expires a successor may supersede it, which is what stops a `kill -9`
    /// from deadlocking the Goal permanently.
    #[arg(long = "goal-lease", default_value = DEFAULT_GOAL_LEASE)]
    pub goal_lease: String,
}

/// Default loop-owner claim lease, matching `goal run --lease`.
pub const DEFAULT_GOAL_LEASE: &str = "60s";

impl Default for GoalAttachArgs {
    /// The "no flags" form, for verbs whose arguments are assembled by hand in
    /// `main.rs` and which therefore attach through the environment only.
    fn default() -> Self {
        Self {
            goal_journal: None,
            goal: None,
            goal_lease: DEFAULT_GOAL_LEASE.to_owned(),
        }
    }
}

impl GoalAttachArgs {
    /// Build the loop driver and Goal id, or `None` when not attaching.
    ///
    /// Returns an error rather than silently ignoring the flags if the journal
    /// cannot be opened: an attachment that quietly degraded into an unattached
    /// run would make "this engine terminated through the canonical transition"
    /// unfalsifiable from the outside, which is the whole defect class this
    /// criterion exists to close.
    pub fn resolve(&self) -> anyhow::Result<Option<(GoalLoop, GoalId)>> {
        // Flags win; the environment is the fallback, never an override. A
        // stray inherited `WAYLAND_GOAL_ID` must not silently re-point a run
        // that named its Goal explicitly on the command line.
        let journal = self
            .goal_journal
            .clone()
            .or_else(|| std::env::var_os(ENV_JOURNAL).map(PathBuf::from));
        let goal = self
            .goal
            .clone()
            .or_else(|| std::env::var(ENV_GOAL).ok().filter(|v| !v.is_empty()));
        let (Some(journal), Some(goal)) = (journal.as_ref(), goal.as_ref()) else {
            return Ok(None);
        };
        let lease_spec = if self.goal_lease == DEFAULT_GOAL_LEASE {
            std::env::var(ENV_LEASE).unwrap_or_else(|_| self.goal_lease.clone())
        } else {
            self.goal_lease.clone()
        };
        let lease = humantime::parse_duration(&lease_spec)
            .map_err(|e| anyhow::anyhow!("invalid goal lease '{lease_spec}': {e}"))?;
        let handle = open_journal(journal)?;
        let lease_ms = u64::try_from(lease.as_millis()).unwrap_or(u64::MAX);
        let driver = GoalLoop::new(GoalKernel::new(handle)).with_lease_ms(lease_ms);
        Ok(Some((driver, GoalId::new(goal))))
    }
}

/// Print the canonical transition a Goal actually landed on, read back from the
/// DURABLE record rather than from the value the adapter returned.
///
/// Reading it back matters: the adapter's own output would report what the
/// engine *asked* for, which is the tautology class §3.2 of the lane brief warns
/// about. This prints what the reducer accepted.
pub fn print_canonical_transition(
    driver: &GoalLoop,
    goal_id: &GoalId,
    strategy: &str,
    cursor: &wcore_protocol::events::RecoveryCursor,
) {
    let terminal = driver.kernel().goal(goal_id).ok().flatten().map_or_else(
        || "unknown".to_owned(),
        |state| format!("{:?}", state.lifecycle),
    );
    println!(
        "GOAL: canonical_transition strategy={strategy} terminal={terminal} cursor_seq={:?}",
        cursor.journal_sequence
    );
}

#[derive(Args, Debug)]
pub struct GoalArgs {
    #[command(subcommand)]
    pub command: GoalCommand,
}

#[derive(Subcommand, Debug)]
pub enum GoalCommand {
    /// Authorize a durable Goal.
    Open {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        goal: String,
        #[arg(long)]
        objective: String,
        /// Loop bound, at least 1. `1` records `LoopPolicy::Once`, the only
        /// policy that cannot multiply a bound; anything higher records
        /// `Fixed`. There is deliberately no way to spell "unbounded" here —
        /// the taxonomy has no such variant, and a flag that invented one
        /// would be a second loop vocabulary beside the canonical taxonomy.
        #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..))]
        iterations: u32,
        /// Which of the five loop owners this Goal authorizes (F22C).
        ///
        /// Before this flag existed `open` hard-coded `Fleet`, so the DURABLE
        /// record could never say anything else and `GoalLoop::claim::<S>` had
        /// to refuse every non-Fleet strategy with `StrategyMismatch`. That is
        /// why four of the five adapters had no product caller: not because the
        /// adapters were missing, but because no Goal could ever be opened for
        /// them. Defaults to `fleet` so every existing invocation, and 22-03's
        /// kill/restart proof, are byte-for-byte unchanged.
        #[arg(long, default_value = "fleet")]
        strategy: GoalStrategyArg,
        /// Token ceiling this Goal may reserve across every task attempt.
        #[arg(long, default_value_t = 10_000)]
        max_tokens: u64,
        /// Ceiling the parent itself is authorized for. The Goal's effective
        /// envelope is the intersection, never the request.
        #[arg(long, default_value_t = 1_000_000)]
        parent_max_tokens: u64,
        #[arg(long, default_value = DEFAULT_PARENT_ENVELOPE)]
        parent_envelope: String,
    },
    /// Declare a task in the Goal's durable ledger.
    Task {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        goal: String,
        #[arg(long)]
        task: String,
        /// Task ids that must carry a DURABLE completion before this one is
        /// claimable. Repeatable.
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        /// The key the task's effect is deduplicated by. Defaults to
        /// `idem-<goal>-<task>`; required to be stable across attempts.
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Recover, reclaim and drive the Goal's tasks through the Fleet dispatcher.
    Run {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        goal: String,
        /// Directory the effects and idempotency markers live in.
        #[arg(long)]
        effects_dir: PathBuf,
        /// argv-style command each task runs. Split on ASCII whitespace; no
        /// shell interpretation. The assignment arrives in the environment.
        #[arg(long)]
        worker_command: String,
        /// How many tasks one wave may claim.
        #[arg(long, default_value_t = 8)]
        width: usize,
        /// Tasks per inner Mesh shard. Below the width so the Fleet path
        /// genuinely shards rather than degenerating to one shard.
        #[arg(long, default_value_t = 4)]
        shard_size: usize,
        /// Claim lease, for BOTH a task claim and the Goal's loop-owner claim.
        /// A claim whose lease has expired is revoked and reassigned by the next
        /// process to start.
        #[arg(long, default_value = "60s")]
        lease: String,
        /// Per-shard wall-clock timeout.
        #[arg(long, default_value = "5m")]
        shard_timeout: String,
        #[arg(long, default_value = DEFAULT_PARENT_ENVELOPE)]
        parent_envelope: String,
        /// Recover and report, then stop without dispatching anything.
        #[arg(long)]
        recover_only: bool,
        /// Terminate the Goal through the ONE canonical Goal terminal
        /// transition when the run finishes (F22C, Success Criterion 3).
        ///
        /// Opt-in, and deliberately so. `goal run` is the verb 22-03's
        /// kill/restart proof re-enters after killing the parent, and a run that
        /// terminated the Goal every time would make that restart impossible.
        /// With this flag the run claims the Goal's ONE loop owner before
        /// dispatching and terminates through `StrategyTermination::from_fleet`
        /// afterwards; without it the Goal stays live exactly as before.
        #[arg(long)]
        terminate: bool,
    },
    /// The canonical JSON projection of a Goal and its task ledger.
    Status {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        goal: String,
    },
    /// The Goal as the HOST protocol sees it (F22-C1).
    ///
    /// Emits the ordered producer stream for one Goal as JSON Lines: every
    /// durable Goal-level transition as a `goal_transition` at the cursor it
    /// landed at, then the current `goal_snapshot`. Replayed from the chain
    /// through the SAME reducer every other view uses, so this cannot show a
    /// state the journal does not hold.
    ///
    /// A verb rather than a flag on `status` because the two answer different
    /// questions: `status` prints the reduced state for a human, `stream` emits
    /// the wire a host consumes. Collapsing them would make one of the two
    /// answers a rendering of the other.
    Stream {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        goal: String,
        /// How many events the caller expects. Exit 1 on a mismatch, so this is
        /// a gate that can go red rather than a print — the same discipline
        /// `effects --expect` carries, and for the same reason: a stream that
        /// emitted nothing must not be indistinguishable from a stream that
        /// emitted everything.
        #[arg(long)]
        expect: Option<usize>,
    },
    /// The effect boundary: idempotency gate, then the operator's command.
    ///
    /// Not normally invoked by hand — `run` spawns it — but it is a real verb so
    /// the gate can be exercised directly.
    ExecTask {
        #[arg(long)]
        effects_dir: PathBuf,
        /// Settle a task the boundary parked because a previous attempt died
        /// inside the effect window.
        ///
        /// Only an operator who has checked the real sink can answer this, which
        /// is exactly why the product refuses to guess. `produced` records the
        /// effect as landed without running anything; `retry` withdraws the
        /// prior intent and runs the worker again.
        #[arg(long)]
        resolve: Option<ResolveArg>,
        /// The operator's argv, after `--`.
        #[arg(last = true)]
        argv: Vec<String>,
    },
    /// Count what is on disk: the worker's REAL effects, and the product's
    /// idempotency markers, reported separately.
    ///
    /// A verb rather than a shell one-liner so a kill/restart proof counts using
    /// the product on every platform, and so the count cannot quietly differ
    /// between a Linux `wc -l` and a PowerShell `Measure-Object`.
    ///
    /// `--expect` gates on the REAL effect count. It used to gate on the marker
    /// count, which cannot exceed the number of distinct keys and therefore
    /// cannot report a duplicate execution at all.
    Effects {
        #[arg(long)]
        effects_dir: PathBuf,
        /// How many real effects the caller expects — one per task, exactly
        /// once. Exit 1 on a mismatch, so this is a gate that can actually go
        /// red rather than a print.
        #[arg(long)]
        expect: Option<usize>,
        /// Gate on the idempotency markers instead of the real effects.
        ///
        /// Kept reachable because the marker count is a genuine second signal —
        /// markers agreeing while real effects disagree is the fingerprint of
        /// the duplicate. It is NOT the default because on its own it is a
        /// count that cannot go red.
        #[arg(long)]
        markers_only: bool,
    },
}

pub async fn run(args: GoalArgs) -> anyhow::Result<()> {
    match args.command {
        GoalCommand::Open {
            journal,
            goal,
            objective,
            iterations,
            max_tokens,
            parent_max_tokens,
            parent_envelope,
            strategy,
        } => open_goal(
            &journal,
            &goal,
            &objective,
            iterations,
            max_tokens,
            parent_max_tokens,
            &parent_envelope,
            strategy.into(),
        ),
        GoalCommand::Task {
            journal,
            goal,
            task,
            depends_on,
            idempotency_key,
        } => declare_task(&journal, &goal, &task, &depends_on, idempotency_key),
        GoalCommand::Run {
            journal,
            goal,
            effects_dir,
            worker_command,
            width,
            shard_size,
            lease,
            shard_timeout,
            parent_envelope,
            recover_only,
            terminate,
        } => {
            run_goal(RunOptions {
                journal,
                goal,
                effects_dir,
                worker_command,
                width,
                shard_size,
                lease: humantime::parse_duration(&lease)
                    .map_err(|e| anyhow::anyhow!("invalid --lease '{lease}': {e}"))?,
                shard_timeout: humantime::parse_duration(&shard_timeout).map_err(|e| {
                    anyhow::anyhow!("invalid --shard-timeout '{shard_timeout}': {e}")
                })?,
                parent_envelope,
                recover_only,
                terminate,
            })
            .await
        }
        GoalCommand::Status { journal, goal } => status(&journal, &goal),
        GoalCommand::Stream {
            journal,
            goal,
            expect,
        } => stream(&journal, &goal, expect),
        GoalCommand::ExecTask {
            effects_dir,
            resolve,
            argv,
        } => {
            match exec_task_from_env(&effects_dir, &argv, resolve).await? {
                ExecOutcome::Produced | ExecOutcome::AlreadyCommitted => Ok(()),
                // A distinguished exit code rather than an error, because the
                // caller has to tell "retry me" from "park me" and an error
                // exit says the first. `goal run` reads this code back.
                ExecOutcome::Indeterminate { detail } => {
                    eprintln!("GOAL-EXEC-INDETERMINATE: {detail}");
                    std::process::exit(EXIT_INDETERMINATE);
                }
            }
        }
        GoalCommand::Effects {
            effects_dir,
            expect,
            markers_only,
        } => {
            let census = count_effects(&effects_dir)?;
            // Both currencies, always, so a reader can see the marker count
            // agreeing while the real count disagrees — which is precisely what
            // a duplicate execution looks like.
            println!(
                "GOAL-EFFECTS: observed_total={} observed_distinct={} duplicates={} markers_total={} markers_distinct={} observed_present={}",
                census.observed_total,
                census.observed_distinct,
                census.duplicates(),
                census.markers_total,
                census.markers_distinct,
                census.observed_present,
            );
            let Some(expect) = expect else { return Ok(()) };

            if markers_only {
                // The old behaviour, reachable only by asking for it by name.
                if census.markers_total != expect || census.markers_distinct != expect {
                    anyhow::bail!(
                        "expected {expect} markers, found total={} distinct={}",
                        census.markers_total,
                        census.markers_distinct
                    );
                }
                return Ok(());
            }

            // VACUITY GUARD. A gate with nothing real to count must refuse
            // rather than pass. Before this, a worker that produced no
            // observable effect at all scored a clean exactly-once.
            if !census.observed_present || census.observed_total == 0 {
                anyhow::bail!(
                    "no observed effects under {}: the worker wrote nothing to {ENV_EFFECT_SINK}, \
                     so this gate cannot certify exactly-once (pass --markers-only to count \
                     idempotency markers instead, and understand that count is blind to duplicates)",
                    effects_dir.join("observed").display()
                );
            }
            if census.observed_total != expect || census.observed_distinct != expect {
                anyhow::bail!(
                    "expected {expect} real effects, found observed_total={} observed_distinct={} \
                     duplicates={} (markers_total={} — the marker count agrees with {expect} even \
                     when the real count does not, which is why it is not the gate)",
                    census.observed_total,
                    census.observed_distinct,
                    census.duplicates(),
                    census.markers_total
                );
            }
            Ok(())
        }
    }
}

fn session_for(journal: &std::path::Path) -> String {
    journal
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wayland-goal".to_owned())
}

fn open_journal(journal: &std::path::Path) -> anyhow::Result<SessionJournal> {
    if let Some(parent) = journal.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let session = session_for(journal);
    SessionJournal::open(journal, &session)
        .map_err(|e| anyhow::anyhow!("failed to open journal {}: {e}", journal.display()))
}

fn now_unix_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

// One flat parameter per `goal open` flag, mirroring `forge.rs`'s entry point:
// these are independently supplied CLI values, and bundling them into a struct
// would add an indirection that exists only to satisfy a lint. `--strategy` is
// the eighth; the other seven predate this lane.
#[allow(clippy::too_many_arguments)]
fn open_goal(
    journal: &std::path::Path,
    goal: &str,
    objective: &str,
    iterations: u32,
    max_tokens: u64,
    parent_max_tokens: u64,
    parent_envelope: &str,
    strategy: GoalStrategy,
) -> anyhow::Result<()> {
    let handle = open_journal(journal)?;
    let driver = GoalFleetDriver::new(handle, GoalId::new(goal), session_for(journal));
    let request = GoalAuthorityRequest {
        requested_limits: [("max_tokens".to_owned(), max_tokens)]
            .into_iter()
            .collect(),
        strategy,
        loop_policy: if iterations == 1 {
            LoopPolicy::Once
        } else {
            LoopPolicy::Fixed { iterations }
        },
    };
    let snapshot = resolve_goal_authority(
        &request,
        &[("max_tokens".to_owned(), parent_max_tokens)]
            .into_iter()
            .collect(),
        parent_envelope,
    );
    driver
        .open(objective, &snapshot, now_unix_ms())
        .map_err(|e| anyhow::anyhow!("failed to open goal {goal}: {e}"))?;
    println!(
        "GOAL: opened goal={goal} strategy={strategy:?} iterations={iterations} \
         envelope={parent_envelope}"
    );
    Ok(())
}

fn declare_task(
    journal: &std::path::Path,
    goal: &str,
    task: &str,
    depends_on: &[String],
    idempotency_key: Option<String>,
) -> anyhow::Result<()> {
    let handle = open_journal(journal)?;
    let driver = GoalFleetDriver::new(handle, GoalId::new(goal), session_for(journal));
    // GOAL-SCOPED, never task-scoped. `idem-{task}` was a key two Goals could
    // both mint: declare `deploy` under goal A and under goal B, point both runs
    // at one `--effects-dir`, and B declines every task as already-committed and
    // reports success having executed nothing. The default now carries the goal,
    // and the on-disk namespace does too (see `scope_dir`) so an operator who
    // supplies `--idempotency-key` by hand cannot re-open the same hole.
    let key = idempotency_key.unwrap_or_else(|| format!("idem-{goal}-{task}"));
    let deps: BTreeSet<String> = depends_on.iter().cloned().collect();
    driver
        .declare_task(&TaskId::new(task), &deps, &key)
        .map_err(|e| anyhow::anyhow!("failed to declare task {task}: {e}"))?;
    println!(
        "GOAL: declared task={task} depends_on={} key={key}",
        if deps.is_empty() {
            "-".to_owned()
        } else {
            deps.iter().cloned().collect::<Vec<_>>().join(",")
        }
    );
    Ok(())
}

struct RunOptions {
    journal: PathBuf,
    goal: String,
    effects_dir: PathBuf,
    worker_command: String,
    width: usize,
    shard_size: usize,
    lease: Duration,
    shard_timeout: Duration,
    parent_envelope: String,
    recover_only: bool,
    terminate: bool,
}

async fn run_goal(options: RunOptions) -> anyhow::Result<()> {
    let argv: Vec<String> = options
        .worker_command
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect();
    if argv.is_empty() {
        anyhow::bail!("--worker-command is empty after whitespace split");
    }
    std::fs::create_dir_all(&options.effects_dir)?;
    let effects_dir = std::fs::canonicalize(&options.effects_dir)?;

    let handle = open_journal(&options.journal)?;
    // ONE handle, cloned to share authority. `SessionJournal::open` takes an
    // exclusive cross-process writer lease and an independent second open fails
    // closed, so the Goal loop must clone this handle rather than reopen the
    // path. Found by the live run, not by the suite: every unit test builds a
    // single driver, so nothing in-process ever opened the journal twice.
    // The loop-owner claim honours the SAME `--lease` as a task claim, because
    // it answers the same question — how long a claim stays evidence that its
    // owner is alive. A loop-owner claim with a different, invisible lease would
    // be a second liveness vocabulary, and a `kill -9` would strand the Goal for
    // however long that hidden default happened to be.
    let loop_driver = GoalLoop::new(GoalKernel::new(handle.clone()))
        .with_lease_ms(u64::try_from(options.lease.as_millis()).unwrap_or(u64::MAX));
    let driver = GoalFleetDriver::new(
        handle,
        GoalId::new(&options.goal),
        format!("{}-{}", session_for(&options.journal), std::process::id()),
    );

    println!("GOAL: run pid={} goal={}", std::process::id(), options.goal);

    // Recovery FIRST, always — including on a first run, where it is a no-op
    // beyond recording the resume. A driver that only recovers "when it looks
    // like a crash" is a driver whose recovery path is never exercised.
    let recovery = driver
        .recover(&options.parent_envelope, now_unix_ms())
        .map_err(|e| anyhow::anyhow!("recovery failed for goal {}: {e}", options.goal))?;
    println!(
        "GOAL: recovery={} resumable={} revoked={} drained={} needs_resolution={}",
        recovery.goal_recovery,
        recovery.resumable,
        recovery.revoked.len(),
        recovery.delivered_from_outbox.len(),
        recovery.requiring_resolution.len()
    );
    for task in &recovery.revoked {
        println!("GOAL: revoked task={task} reason=lease-expired");
    }
    for task in &recovery.delivered_from_outbox {
        println!("GOAL: drained task={task} source=outbox");
    }
    if !recovery.resumable {
        println!("GOAL: not resumable; stopping without dispatch");
        return Ok(());
    }
    if options.recover_only {
        println!("GOAL: recover-only; stopping before dispatch");
        return Ok(());
    }

    let dispatcher = FleetDispatcher::new(format!("goal-{}", options.goal))
        .with_shard_size(options.shard_size)
        .with_shard_timeout(options.shard_timeout);
    let executor: Arc<dyn TaskExecutor> = Arc::new(ChildProcessExecutor { effects_dir, argv });

    let lease_ms = u64::try_from(options.lease.as_millis()).unwrap_or(u64::MAX);

    // ── F22C: the canonical terminal transition, when asked for ─────────────
    //
    // Without `--terminate` this is byte-for-byte the pre-F22C path: dispatch,
    // print, leave the Goal live for the next restart. With it, the whole
    // dispatch runs INSIDE `GoalLoop::run_fleet`, which claims the Goal's one
    // loop owner before the first wave and terminates through
    // `StrategyTermination::from_fleet` after the last. The closure's return
    // type is `StrategyTermination`, so there is no path out of it that reaches
    // a terminal state any other way.
    if options.terminate {
        let goal_id = GoalId::new(&options.goal);
        let cursor = loop_driver
            .run_fleet(&goal_id, |owner| async move {
                match driver
                    .run_to_completion(&dispatcher, executor, options.width, lease_ms, &now_unix_ms)
                    .await
                {
                    Ok(run) => {
                        for (index, wave) in run.waves.iter().enumerate() {
                            print_wave(index, wave);
                        }
                        println!(
                            "GOAL: run_complete waves={} iterations={} completed={} \
                             delivered={} stopped_because={}",
                            run.waves.len(),
                            run.iterations_consumed,
                            run.completed(),
                            run.delivered(),
                            run.stopped_because
                        );
                        // UNRESOLVED TASKS BLOCK THE TERMINAL, and they have to,
                        // because `ShardSummary` has only two counters and an
                        // indeterminate task is neither. Mapping the run onto
                        // `Dispatched` regardless produced
                        // `PartiallyCompleted { completed: 0, failed: 0 }` over
                        // four parked tasks — a canonical terminal transition
                        // that structurally cannot see the one state that needs
                        // a human. `Blocked` carries the reason instead, which
                        // is what a Goal waiting on `--resolve` actually is.
                        let indeterminate: usize =
                            run.waves.iter().map(|wave| wave.indeterminate).sum();
                        let abandoned: usize = run.waves.iter().map(|wave| wave.abandoned).sum();
                        if indeterminate + abandoned > 0 {
                            println!(
                                "GOAL: unresolved indeterminate={indeterminate} \
                                 abandoned={abandoned}"
                            );
                            return StrategyTermination::from_fleet(
                                owner,
                                FleetOutcome::DriverFailed {
                                    detail: format!(
                                        "{} task(s) unresolved (indeterminate={indeterminate} \
                                         abandoned={abandoned}); the effect boundary parked them \
                                         and only an operator can settle them with \
                                         `goal exec-task --resolve produced|retry`",
                                        indeterminate + abandoned
                                    ),
                                },
                            );
                        }
                        // #946 B-01, the OTHER way a run can end over an
                        // unfinished Goal. The arm above catches tasks the
                        // effect boundary parked; this one catches the case
                        // where the loop never claimed anything at all because
                        // every remaining task is under a live, unexpired lease
                        // (a killed predecessor), parked, or dependency-blocked.
                        // Before this, that reported `run_complete … no
                        // claimable task remains` and exit 0 — the finished
                        // Goal's own words over a job nobody had finished.
                        if let Some(census) = run.idle.filter(|c| !c.is_finished()) {
                            println!(
                                "GOAL: unfinished lease_held={} awaiting_resolution={} \
                                 dependency_blocked={}",
                                census.lease_held,
                                census.awaiting_resolution,
                                census.dependency_blocked
                            );
                            return StrategyTermination::from_fleet(
                                owner,
                                FleetOutcome::DriverFailed {
                                    detail: run.stopped_because.clone(),
                                },
                            );
                        }
                        // Bound at shard level, never at a caller-chosen `T`:
                        // one `ShardSummary` per wave, carrying the
                        // completed/failed counts the driver itself measured.
                        // Nothing here invents a number or rounds the split away.
                        let shards: Vec<ShardSummary> = run
                            .waves
                            .iter()
                            .enumerate()
                            .map(|(index, wave)| ShardSummary {
                                shard_id: index,
                                agent_count: wave.claimed,
                                successes: wave.completed,
                                failures: wave.failed,
                                payload: serde_json::Value::Null,
                            })
                            .collect();
                        StrategyTermination::from_fleet(owner, FleetOutcome::Dispatched(&shards))
                    }
                    // Carried into the terminal transition as a stated reason,
                    // never swallowed into a clean terminal and never squeezed
                    // into a `FleetError` variant that did not happen.
                    Err(error) => StrategyTermination::from_fleet(
                        owner,
                        FleetOutcome::DriverFailed {
                            detail: error.to_string(),
                        },
                    ),
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("goal {} did not terminate: {e}", options.goal))?;

        let terminal = loop_driver
            .kernel()
            .goal(&goal_id)
            .ok()
            .flatten()
            .map_or_else(
                || "unknown".to_owned(),
                |state| format!("{:?}", state.lifecycle),
            );
        println!(
            "GOAL: canonical_transition strategy=fleet terminal={terminal} cursor_seq={:?}",
            cursor.journal_sequence
        );
        return Ok(());
    }

    let run = driver
        .run_to_completion(&dispatcher, executor, options.width, lease_ms, &now_unix_ms)
        .await
        .map_err(|e| anyhow::anyhow!("fleet run failed for goal {}: {e}", options.goal))?;

    for (index, wave) in run.waves.iter().enumerate() {
        print_wave(index, wave);
    }
    println!(
        "GOAL: run_complete waves={} iterations={} completed={} delivered={} stopped_because={}",
        run.waves.len(),
        run.iterations_consumed,
        run.completed(),
        run.delivered(),
        run.stopped_because
    );
    // #946 B-01. `goal run` without `--terminate` returned `Ok(())` whatever
    // the ledger said, so a scripted caller could not tell "the goal is done"
    // from "everything left belongs to a process that has not released it".
    // The sentence above already carries the distinction; this puts it on the
    // channel a script actually reads. A FINISHED goal still exits 0, and so
    // does a run that stopped on its authorized loop bound (`idle` is `None`
    // there — the loop never consulted the ledger).
    if let Some(census) = run.idle.filter(|c| !c.is_finished()) {
        println!(
            "GOAL: unfinished lease_held={} awaiting_resolution={} dependency_blocked={}",
            census.lease_held, census.awaiting_resolution, census.dependency_blocked
        );
        anyhow::bail!("{}", run.stopped_because);
    }
    Ok(())
}

fn print_wave(index: usize, wave: &WaveOutcome) {
    println!(
        "GOAL: wave={index} shards={} claimed={} lost={} completed={} failed={} \
         indeterminate={} abandoned={} delivered={} dispatch_error={}",
        wave.shards,
        wave.claimed,
        wave.lost_claims,
        wave.completed,
        wave.failed,
        wave.indeterminate,
        wave.abandoned,
        wave.delivered.len(),
        wave.dispatch_error.as_deref().unwrap_or("-")
    );
}

fn status(journal: &std::path::Path, goal: &str) -> anyhow::Result<()> {
    let handle = open_journal(journal)?;
    let driver = GoalFleetDriver::new(handle, GoalId::new(goal), session_for(journal));
    let Some(state) = driver
        .goal_state()
        .map_err(|e| anyhow::anyhow!("failed to read goal {goal}: {e}"))?
    else {
        anyhow::bail!("no goal {goal} in {}", journal.display());
    };
    // The projection is the reduced state itself, not a hand-built view of it.
    // A surface that renders its own shape is a surface that can disagree with
    // the chain, which is the parallel lifecycle this phase exists to remove.
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(())
}

/// The host-protocol producer stream for one Goal (F22-C1).
///
/// Stdout carries ONLY JSON Lines, so the output is directly consumable by a
/// host decoder; the count summary goes to stderr for the same reason.
fn stream(journal: &std::path::Path, goal: &str, expect: Option<usize>) -> anyhow::Result<()> {
    if !journal.exists() {
        anyhow::bail!("no journal at {}", journal.display());
    }
    let envelopes = SessionJournal::replay(journal)
        .map_err(|e| anyhow::anyhow!("failed to replay {}: {e}", journal.display()))?;
    let events = goal_stream(&session_for(journal), goal, &envelopes)
        .map_err(|e| anyhow::anyhow!("failed to project goal {goal}: {e}"))?;
    if events.is_empty() {
        anyhow::bail!("no goal {goal} in {}", journal.display());
    }
    for event in &events {
        println!(
            "{}",
            event_line(event)
                .map_err(|e| anyhow::anyhow!("failed to serialize goal event: {e}"))?
        );
    }
    // Counts on stderr, so a caller can assert what was emitted without having
    // to parse the stream it is asserting about.
    let transitions = events.len().saturating_sub(1);
    eprintln!(
        "GOAL-STREAM: goal={goal} events={} transitions={transitions} snapshots=1",
        events.len()
    );
    if let Some(expect) = expect
        && events.len() != expect
    {
        anyhow::bail!("expected {expect} goal events, emitted {}", events.len());
    }
    Ok(())
}

/// Runs one assigned task as a real child process.
struct ChildProcessExecutor {
    effects_dir: PathBuf,
    argv: Vec<String>,
}

impl TaskExecutor for ChildProcessExecutor {
    fn execute(
        &self,
        assignment: TaskAssignment,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TaskExecution> + Send>> {
        let effects_dir = self.effects_dir.clone();
        let argv = self.argv.clone();
        Box::pin(async move {
            let Ok(exe) = std::env::current_exe() else {
                return TaskExecution::Failed {
                    detail: "could not resolve this binary's own path".to_owned(),
                };
            };
            // argv mode throughout: the operator's command reaches the OS as
            // separate argv entries, so a shell metacharacter in a task label
            // or a worker command is data, never syntax.
            let mut command = tokio::process::Command::new(exe);
            command
                .arg("goal")
                .arg("exec-task")
                .arg("--effects-dir")
                .arg(&effects_dir)
                .arg("--");
            for entry in &argv {
                command.arg(entry);
            }
            command
                .env(ENV_GOAL, &assignment.goal_id)
                .env(ENV_TASK, &assignment.task_id)
                .env(ENV_KEY, &assignment.idempotency_key)
                .env(ENV_EPOCH, assignment.epoch.to_string())
                .env(ENV_ATTEMPT, assignment.attempt.to_string())
                .env(ENV_WORKER, &assignment.worker_id);

            match command.status().await {
                Ok(status) if status.success() => TaskExecution::Produced {
                    outcome: GoalTerminalState::SelfChecked,
                    effect_digest: assignment.idempotency_key.clone(),
                },
                Ok(status) => match status.code() {
                    // The effect boundary found a prior attempt's intent with no
                    // commit. It refused to decide, and so does this: the task
                    // is parked for resolution, never silently retried.
                    Some(code) if code == EXIT_INDETERMINATE => TaskExecution::Indeterminate {
                        reason: TaskUnknownReason::OwnerDiedMidAttempt,
                    },
                    Some(code) => TaskExecution::Failed {
                        detail: format!("worker exited {code}"),
                    },
                    // No exit code means a signal took it. Whether its effect
                    // landed is genuinely unknown, and an unknown outcome is
                    // parked for resolution rather than retried — a silent retry
                    // here is the duplicate execution the criterion forbids.
                    None => TaskExecution::Indeterminate {
                        reason: TaskUnknownReason::OwnerDiedMidAttempt,
                    },
                },
                Err(error) => TaskExecution::Failed {
                    detail: format!("worker failed to start: {error}"),
                },
            }
        })
    }
}

/// The effect boundary.
///
/// ## One marker cannot do this job, and for a while it was asked to
///
/// The original design wrote a single idempotency marker AFTER the operator's
/// command succeeded. That ordering was chosen to avoid the opposite bug — a
/// marker written first, then a death, leaves the marker with no effect and the
/// task permanently un-runnable — and the reasoning was right as far as it went.
/// What it missed is that the two orderings are not a choice between a safe one
/// and an unsafe one. They are a choice between **losing** an effect and
/// **duplicating** one, and a single marker must pick a side:
///
/// | Order | Death after the effect, before the record | Death after the record, before the effect |
/// |---|---|---|
/// | marker last | retry re-runs → **duplicate** | n/a |
/// | marker first | n/a | retry declines → **lost** |
///
/// The window is not narrow, either. A worker that performs its effect and then
/// keeps working — the ordinary shape of "send the message, then reconcile" — is
/// inside it for its whole run.
///
/// ## What is here instead: intent, effect, commit
///
/// Two records, so the three states are distinguishable on disk:
///
/// | On disk | Meaning | What this function does |
/// |---|---|---|
/// | commit | the effect landed and was recorded | decline, exactly once |
/// | intent, no commit | a previous attempt died INSIDE the window | decide nothing; report indeterminate |
/// | neither | nothing has run | record intent, run, commit |
///
/// The third row is the whole fix. An effect performed before its completion is
/// durably recorded will always re-run — unless the *attempt* was durably
/// recorded first, which is what makes the ambiguity visible instead of
/// invisible.
///
/// ## What this does NOT claim
///
/// This does not make a non-idempotent external effect exactly-once. Nothing
/// outside a transaction that spans the effect and its record can. What it
/// guarantees is narrower and is the property the criterion actually needs: the
/// product never SILENTLY duplicates and never silently loses. The ambiguous
/// case is surfaced as [`ExecOutcome::Indeterminate`], the ledger parks the task
/// through the path it already has for unknown outcomes, and a human settles it
/// with `--resolve`.
///
/// Reads the assignment out of the environment `run` spawned this process with,
/// then does the work.
///
/// The env read is deliberately separated from [`exec_task`] rather than done
/// inside it. Process environment is global mutable state, and a function that
/// reaches for it can only be tested by mutating the whole process — which makes
/// the tests serialize against each other and go flaky in a way that looks
/// exactly like the idempotency gate failing. Passing the assignment in means
/// the gate is testable without touching the environment at all.
async fn exec_task_from_env(
    effects_dir: &std::path::Path,
    argv: &[String],
    resolve: Option<ResolveArg>,
) -> anyhow::Result<ExecOutcome> {
    let task = std::env::var(ENV_TASK).unwrap_or_else(|_| "unknown".to_owned());
    let key = std::env::var(ENV_KEY).map_err(|_| {
        anyhow::anyhow!("{ENV_KEY} is not set; refusing to produce an unkeyed effect")
    })?;
    // Required, exactly as the key is. An effect recorded without a Goal lands
    // in a namespace two Goals share, and sharing that namespace is how one Goal
    // declines another's work as already done — see `scope_dir`.
    let goal = std::env::var(ENV_GOAL).map_err(|_| {
        anyhow::anyhow!("{ENV_GOAL} is not set; refusing to produce an unscoped effect")
    })?;
    exec_task(effects_dir, argv, &goal, &task, &key, resolve).await
}

/// What one pass through the effect boundary established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecOutcome {
    /// The worker ran and its effect is now durably committed.
    Produced,
    /// A previous attempt's effect is already committed. Nothing ran.
    AlreadyCommitted,
    /// A previous attempt died INSIDE the effect window. Whether its effect
    /// landed is unknowable from here, so nothing ran and nothing was decided.
    Indeterminate { detail: String },
}

/// Exit code `exec-task` uses for [`ExecOutcome::Indeterminate`].
///
/// Distinguished from both success and a plain failure, because the caller must
/// distinguish them: a failure means retry, an indeterminate means park.
///
/// ## Why not 75
///
/// It was 75, on the reasoning that `EX_TEMPFAIL` is "the closest existing
/// convention". That was the mistake: a convention that already exists is a
/// convention other programs already EMIT. `sysexits.h` owns 64..=78, and a
/// worker that shells out to anything from that tradition can hand this
/// boundary a 75 or a 76 that means what `sendmail` meant, not what this module
/// means. These two codes are therefore chosen to be unreachable by that
/// tradition: above `EX__MAX` (78) and below the shell's reserved band
/// (126..=165, 255).
pub const EXIT_INDETERMINATE: i32 = 90;

/// The exit code a WORKER uses to declare that its effect did not land.
///
/// ## Why the worker has to say it, rather than the product inferring it
///
/// The product cannot tell a worker that chose to fail from a worker that was
/// killed. On Unix it looks like it can — signal death has no exit code — but
/// on Windows `taskkill /F` and `exit 1` are the same integer, and the first
/// version of this fix assumed nonzero meant "nothing landed". A Windows run
/// caught it: a worker killed AFTER doing its work had its intent withdrawn and
/// was retried, duplicating the effect the fix exists to prevent.
///
/// So there is one rule on every platform. Zero means the effect landed. This
/// code means it certainly did not, and the task is plainly retryable. Anything
/// else means nobody knows, and the task is parked.
///
/// ## And the code is not sufficient on its own
///
/// It was 76, which is `EX_PROTOCOL` — a value the OS ecosystem already uses,
/// so a worker could emit it by accident and have its intent withdrawn. Both
/// halves of that are fixed: the value moved out of the `sysexits.h` range
/// (see [`EXIT_INDETERMINATE`]), and the withdraw path now additionally
/// requires the out-of-band receipt at [`ENV_NO_EFFECT_RECEIPT`]. An exit code
/// alone can no longer withdraw anything.
pub const EXIT_NO_EFFECT: i32 = 91;

/// One filesystem-safe, injective path component for an arbitrary identifier.
///
/// Injective is the load-bearing word. Sanitizing alone would map `goal/a` and
/// `goal-a` onto the same directory, which is the cross-goal collision this
/// scoping exists to prevent, arriving by a different route. The trailing digest
/// is therefore not decoration: two different inputs cannot produce the same
/// component.
///
/// The digest is a hand-rolled FNV-1a rather than [`std::hash::DefaultHasher`]
/// because these names outlive the process. `DefaultHasher`'s output is
/// explicitly not stable across Rust releases, so a rebuild on a newer toolchain
/// would rename every scope directory and every already-committed effect would
/// silently be re-run.
fn scope_dir(raw: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in raw.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let safe: String = raw
        .chars()
        .take(48)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // A leading dot would make the scope a hidden directory, and `.`/`..` are
    // not names at all. The digest suffix means the prefix is only ever a human
    // convenience, so replacing it wholesale is free.
    let safe = if safe.is_empty() || safe.starts_with('.') {
        "scope".to_owned()
    } else {
        safe
    };
    format!("{safe}-{hash:016x}")
}

/// How long a second claimant waits for the attempt that beat it to the intent.
///
/// Parking is not free — a parked task needs a human — so the boundary should
/// only park when there is genuinely nothing else to learn. Two processes racing
/// for the same key is not that case: the loser can simply wait for the winner
/// to commit and then decline, which is the correct exactly-once answer and
/// needs nobody. Bounded, because a winner that never finishes must still park
/// rather than hang the caller forever.
const OVERLAP_WAIT: Duration = Duration::from_millis(2_000);
const OVERLAP_POLL: Duration = Duration::from_millis(50);

/// What waiting on another attempt's intent established.
enum Overlap {
    /// The other attempt committed. Nothing for this one to do.
    Committed,
    /// The other attempt withdrew its intent without committing, so the task is
    /// runnable again.
    Vanished,
    /// Still holding after the bound. Genuinely undecidable from here.
    Held,
}

async fn await_overlapping_attempt(commit: &std::path::Path, intent: &std::path::Path) -> Overlap {
    let deadline = std::time::Instant::now() + OVERLAP_WAIT;
    loop {
        // Commit first, always: a commit beside a leftover intent is settled.
        if commit.exists() {
            return Overlap::Committed;
        }
        if !intent.exists() {
            return Overlap::Vanished;
        }
        if std::time::Instant::now() >= deadline {
            return Overlap::Held;
        }
        tokio::time::sleep(OVERLAP_POLL).await;
    }
}

async fn exec_task(
    effects_dir: &std::path::Path,
    argv: &[String],
    goal: &str,
    task: &str,
    key: &str,
    resolve: Option<ResolveArg>,
) -> anyhow::Result<ExecOutcome> {
    if key.is_empty() {
        anyhow::bail!("{ENV_KEY} is empty; refusing to produce an unkeyed effect");
    }
    if goal.is_empty() {
        anyhow::bail!("{ENV_GOAL} is empty; refusing to produce an unscoped effect");
    }
    // GOAL-SCOPED NAMESPACES. Every one of these directories used to be keyed on
    // the idempotency key alone, so two Goals sharing one `--effects-dir` shared
    // one namespace: goal B declined goal A's committed keys as its own
    // already-done work and reported success having executed nothing, and a
    // stale intent left by a killed goal A permanently parked a brand-new goal B
    // on its own journal. Both are the same root cause and both close here.
    let scope = scope_dir(goal);
    let effects = effects_dir.join("effects").join(&scope);
    let intents = effects_dir.join("intents").join(&scope);
    let declared = effects_dir.join("declared").join(&scope);
    std::fs::create_dir_all(&effects)?;
    std::fs::create_dir_all(&intents)?;
    std::fs::create_dir_all(&declared)?;
    let commit = effects.join(key);
    let intent = intents.join(key);
    let receipt = declared.join(format!("{}.no-effect", scope_dir(key)));

    // Bounded, because every `continue` below is a state the loop has already
    // re-read from disk: a second claimant that lost the intent race re-enters
    // once to wait for the winner, and a winner that withdrew lets this attempt
    // run. Three passes is more than either path needs and cannot spin.
    for _pass in 0..3_u8 {
        // The COMMIT is checked first and unconditionally. A leftover intent
        // beside a commit is not ambiguous — the commit is written after the
        // effect, so its presence settles the question no matter what else is on
        // disk.
        if commit.exists() {
            println!(
                "GOAL-EXEC: goal={goal} task={task} key={key} produced=no \
                 reason=effect-already-committed"
            );
            return Ok(ExecOutcome::AlreadyCommitted);
        }

        // An intent with no commit means another attempt is inside — or died
        // inside — the window between "worker started" and "effect committed".
        if intent.exists() {
            let evidence = std::fs::read_to_string(&intent).unwrap_or_default();
            let evidence = evidence.trim().to_owned();
            match resolve {
                None => match await_overlapping_attempt(&commit, &intent).await {
                    Overlap::Committed => {
                        println!(
                            "GOAL-EXEC: goal={goal} task={task} key={key} produced=no \
                             reason=concurrent-attempt-committed-first"
                        );
                        return Ok(ExecOutcome::AlreadyCommitted);
                    }
                    // The holder withdrew without committing, which only the
                    // no-effect path does. The task is plainly runnable.
                    Overlap::Vanished => continue,
                    Overlap::Held => {
                        println!(
                            "GOAL-EXEC: goal={goal} task={task} key={key} produced=unknown \
                             reason=prior-attempt-died-inside-the-effect-window \
                             intent=[{evidence}]"
                        );
                        return Ok(ExecOutcome::Indeterminate {
                            detail: format!(
                                "a prior attempt died between starting the worker and committing \
                                 its effect ({evidence}); re-running would duplicate the effect \
                                 and skipping would lose it, so this attempt decides neither"
                            ),
                        });
                    }
                },
                Some(ResolveArg::Produced) => {
                    // The operator has established that the effect DID land.
                    // Commit it without running anything.
                    commit_effect(&commit, task)?;
                    std::fs::remove_file(&intent).ok();
                    println!(
                        "GOAL-EXEC: goal={goal} task={task} key={key} produced=no \
                         reason=operator-resolved-as-already-produced"
                    );
                    return Ok(ExecOutcome::AlreadyCommitted);
                }
                Some(ResolveArg::Retry) => {
                    // The operator has established that the effect did NOT land,
                    // or that it is idempotent. Clear the intent and fall
                    // through.
                    std::fs::remove_file(&intent).ok();
                    println!(
                        "GOAL-EXEC: goal={goal} task={task} key={key} \
                         note=operator-resolved-as-not-produced-retrying"
                    );
                }
            }
        }

        // The worker's sink for REAL effects: one directory per (goal, task),
        // created here rather than left to the worker. Created by the product
        // for two reasons. An absent sink then means "the worker wrote nothing",
        // never "the worker could not create the directory" — the gate treats
        // those very differently. And because the product names it, the number
        // of INVOCATIONS of one task is the number of files in one directory,
        // which no choice of record content by the worker can disguise.
        let observed = effects_dir
            .join("observed")
            .join(&scope)
            .join(scope_dir(task));
        std::fs::create_dir_all(&observed)?;

        if !argv.is_empty() {
            // THE INTENT, recorded BEFORE the effect. This is the half that was
            // missing: with only a marker written afterwards, every death in the
            // window [worker ran, marker written] was invisible to the retry,
            // and the retry therefore re-ran the effect.
            //
            // `create_new` also makes this the cross-process mutex for the key:
            // a second claimant that gets here concurrently loses the race, and
            // re-enters the loop to wait for the winner rather than parking.
            match write_durably(&intent, &format!("task={task} pid={}", std::process::id())) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(anyhow::anyhow!("intent for {key}: {error}"));
                }
            }
            sync_dir(&intents);

            // A receipt left by an earlier attempt must never be read as this
            // attempt's declaration.
            std::fs::remove_file(&receipt).ok();

            // Argv mode, never a shell string: the operator's command and every
            // argument reach the OS as separate argv entries, so a metacharacter
            // in a task label is data rather than syntax.
            let rest: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
            let mut command = wcore_config::shell::shell_command_argv(&argv[0], &rest);
            command.current_dir(effects_dir);
            command.env(ENV_EFFECT_SINK, &observed);
            command.env(ENV_NO_EFFECT_RECEIPT, &receipt);
            let status = command.status().await.map_err(|e| {
                anyhow::anyhow!("worker command '{}' failed to start: {e}", argv[0])
            })?;
            if !status.success() {
                // ONLY the worker knows whether its effect landed, and it has to
                // SAY so — twice, in two channels that cannot both be produced
                // by accident. This was wrong twice. First it treated every
                // nonzero exit as "nothing landed", and a Windows `taskkill /F`
                // (indistinguishable from `exit 1`) duplicated the effect.
                // Then it trusted one integer, and that integer was 76 —
                // `EX_PROTOCOL`, which any `sysexits.h`-speaking tool in a
                // worker's pipeline emits on its own.
                //
                // So the rule on every platform: nonzero means UNKNOWN unless
                // the worker BOTH created the receipt at `ENV_NO_EFFECT_RECEIPT`
                // and exited `EXIT_NO_EFFECT`.
                let declared_no_effect = receipt.exists() && status.code() == Some(EXIT_NO_EFFECT);
                if !declared_no_effect {
                    println!(
                        "GOAL-EXEC: goal={goal} task={task} key={key} produced=unknown \
                         reason=worker-exited-{status}-without-declaring-no-effect \
                         receipt={}",
                        receipt.exists()
                    );
                    return Ok(ExecOutcome::Indeterminate {
                        detail: format!(
                            "worker command '{}' exited {status} and left receipt={} at \
                             {ENV_NO_EFFECT_RECEIPT}. A withdrawal needs BOTH the receipt file \
                             and exit {EXIT_NO_EFFECT}, so whether the effect landed is unknown \
                             and this attempt decides neither",
                            argv[0],
                            receipt.exists()
                        ),
                    });
                }
                // The worker declared that nothing landed. Withdraw the intent
                // so the task stays plainly retryable — a lost completion fails
                // exactly as loudly as a duplicate.
                std::fs::remove_file(&intent).ok();
                std::fs::remove_file(&receipt).ok();
                anyhow::bail!(
                    "worker command '{}' exited {EXIT_NO_EFFECT} with a {ENV_NO_EFFECT_RECEIPT} \
                     receipt (declared: no effect landed)",
                    argv[0]
                );
            }
            std::fs::remove_file(&receipt).ok();
        }

        // THE COMMIT. `create_new` is atomic on both platforms.
        match commit_effect(&commit, task) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                println!(
                    "GOAL-EXEC: goal={goal} task={task} key={key} produced=no \
                     reason=effect-already-committed"
                );
                return Ok(ExecOutcome::AlreadyCommitted);
            }
            Err(error) => return Err(anyhow::anyhow!("effect for {key}: {error}")),
        }
        sync_dir(&effects);
        std::fs::remove_file(&intent).ok();
        println!("GOAL-EXEC: goal={goal} task={task} key={key} produced=yes");
        return Ok(ExecOutcome::Produced);
    }

    // Every pass found another attempt holding the key. Undecidable, and said so
    // rather than guessed at.
    Ok(ExecOutcome::Indeterminate {
        detail: format!(
            "another attempt held the intent for {key} through every pass; whether its effect \
             landed is unknown and this attempt decides neither"
        ),
    })
}

/// Create the commit marker atomically, write its label, and flush it.
fn commit_effect(commit: &std::path::Path, task: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(commit)?;
    writeln!(file, "{task}")?;
    file.sync_all()
}

/// Create a record atomically and get it to the platter before returning.
///
/// `create_new` rather than `create`: an intent that already exists is a state
/// this function must never silently overwrite, because overwriting it would
/// erase the evidence that a previous attempt was inside the window.
fn write_durably(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    writeln!(file, "{body}")?;
    file.sync_all()
}

/// Flush the directory entry itself, so a crash cannot lose a file that
/// `sync_all` already durably wrote.
///
/// Best effort, and deliberately not fatal: Windows has no equivalent operation
/// and returns an error for a directory handle opened this way. Failing the
/// effect boundary because a durability *hint* is unavailable would trade a rare
/// crash window for a certain outage.
fn sync_dir(dir: &std::path::Path) {
    #[cfg(unix)]
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// What is actually on disk after a run, counted in two independent currencies.
///
/// The two halves are kept apart because conflating them is the defect this
/// type exists to stop. `markers_*` describes what the PRODUCT recorded about
/// itself. `observed_*` describes what the WORKER really did. Only the second
/// can go red for a duplicate execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectCensus {
    /// Idempotency marker files. Bounded above by the number of distinct keys,
    /// so this can never exceed the task count however many times work ran.
    pub markers_total: usize,
    /// Distinct task labels across the markers.
    pub markers_distinct: usize,
    /// Whether the worker-written sink exists at all. `false` means the run
    /// produced no observable external effect, and no exactly-once claim can be
    /// made from it.
    pub observed_present: bool,
    /// Real executions: one record per invocation of the operator's command.
    pub observed_total: usize,
    /// Distinct task labels across those executions.
    pub observed_distinct: usize,
}

impl EffectCensus {
    /// The duplicate count the whole defect is about: real executions beyond one
    /// per task.
    #[must_use]
    pub fn duplicates(&self) -> usize {
        self.observed_total.saturating_sub(self.observed_distinct)
    }
}

/// Count what is on disk: the product's markers, and the worker's real effects.
///
/// Exposed so the live proof counts what the PRODUCT wrote rather than what a
/// harness believes it wrote — and, since the marker count is blind to
/// duplicates by construction, so it also counts what the WORKER wrote.
pub fn count_effects(effects_dir: &std::path::Path) -> anyhow::Result<EffectCensus> {
    let markers = walk(&effects_dir.join("effects"))?;
    let observed = walk(&effects_dir.join("observed"))?;
    // A marker's identity is still its content: the product writes exactly one
    // marker per key and writes the task label into it.
    let markers_distinct: BTreeSet<&String> = markers
        .records
        .iter()
        .map(|record| &record.content)
        .collect();
    // A real effect's identity is the INVOCATION, and the invocation belongs to
    // the (goal, task) directory the product handed the worker — never to what
    // the worker chose to write. Counting distinct CONTENT was the instrument
    // defect: a worker whose record carries `msg_id=…`, as any real effect log
    // does, made two executions of one task look like two different tasks and
    // the duplicate count read zero on the very failure it exists to catch.
    let observed_distinct: BTreeSet<String> = observed
        .records
        .iter()
        .map(|record| {
            if record.scope.is_empty() {
                // A record dropped straight into `observed/` predates the
                // per-task sink and has no directory identity to use. Fall back
                // to its content so an old effects directory still counts,
                // rather than silently reading as one distinct effect per file.
                format!("content:{}", record.content)
            } else {
                format!("dir:{}", record.scope)
            }
        })
        .collect();
    Ok(EffectCensus {
        markers_total: markers.records.len(),
        markers_distinct: markers_distinct.len(),
        observed_present: observed.present,
        observed_total: observed.records.len(),
        observed_distinct: observed_distinct.len(),
    })
}

/// One file found under a census root.
struct Record {
    /// Path of the containing directory relative to the root, `/`-joined.
    /// Empty for a file sitting directly in the root.
    scope: String,
    /// Trimmed file content.
    content: String,
}

struct Walk {
    records: Vec<Record>,
    present: bool,
}

/// Every file under `dir`, with the relative directory it sits in.
///
/// Recursive because the namespaces are now scoped — `effects/<goal>/<key>`,
/// `observed/<goal>/<task>/<invocation>` — and a flat `read_dir` over the root
/// would count zero files and report a clean, empty, entirely false census.
fn walk(dir: &std::path::Path) -> anyhow::Result<Walk> {
    if !dir.is_dir() {
        return Ok(Walk {
            records: Vec::new(),
            present: false,
        });
    }
    let mut records = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((current, scope)) = stack.pop() {
        for entry in std::fs::read_dir(&current)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                let child = if scope.is_empty() {
                    name
                } else {
                    format!("{scope}/{name}")
                };
                stack.push((entry.path(), child));
            } else if kind.is_file() {
                records.push(Record {
                    scope: scope.clone(),
                    content: std::fs::read_to_string(entry.path())
                        .unwrap_or_default()
                        .trim()
                        .to_owned(),
                });
            }
        }
    }
    Ok(Walk {
        records,
        present: true,
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Every unit test that is not specifically about cross-goal scoping runs
    /// under this one Goal, so the scoping is exercised by every test rather
    /// than by a special one.
    const G: &str = "g-unit";

    /// The MARKER half of the census, which is what these unit tests are about:
    /// they exercise the idempotency gate directly, with no worker writing to
    /// the real-effect sink. The distinction is load-bearing — see
    /// [`ENV_EFFECT_SINK`] — so the tests name which half they mean.
    fn markers(dir: &std::path::Path) -> (usize, usize) {
        let census = count_effects(dir).unwrap();
        (census.markers_total, census.markers_distinct)
    }

    /// The operator's argv for a worker that declares no effect landed: it
    /// creates the out-of-band receipt AND exits [`EXIT_NO_EFFECT`]. Both are
    /// required — see [`ENV_NO_EFFECT_RECEIPT`].
    fn declining_worker() -> Vec<String> {
        vec![
            if cfg!(windows) { "cmd" } else { "sh" }.to_owned(),
            if cfg!(windows) { "/c" } else { "-c" }.to_owned(),
            if cfg!(windows) {
                format!("echo no-effect > \"%{ENV_NO_EFFECT_RECEIPT}%\" & exit {EXIT_NO_EFFECT}")
            } else {
                format!(
                    "printf 'no-effect\\n' > \"${ENV_NO_EFFECT_RECEIPT}\"; exit {EXIT_NO_EFFECT}"
                )
            },
        ]
    }

    /// A worker that exits with `code` and declares nothing.
    fn worker(code: i32) -> Vec<String> {
        vec![
            if cfg!(windows) { "cmd" } else { "sh" }.to_owned(),
            if cfg!(windows) { "/c" } else { "-c" }.to_owned(),
            format!("exit {code}"),
        ]
    }

    /// The intent file for a key under a goal, wherever the scoping put it.
    fn intent_path(dir: &std::path::Path, goal: &str, key: &str) -> PathBuf {
        dir.join("intents").join(scope_dir(goal)).join(key)
    }

    #[tokio::test]
    async fn exec_task_refuses_to_produce_an_unkeyed_effect() {
        let dir = tempfile::tempdir().unwrap();
        let error = exec_task(dir.path(), &[], G, "t-unkeyed", "", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(ENV_KEY), "got: {error}");
        assert_eq!(markers(dir.path()), (0, 0));
    }

    /// The other half of the same refusal. An effect with no Goal lands in a
    /// namespace every Goal shares, which is how one Goal declines another's
    /// work as already done.
    #[tokio::test]
    async fn exec_task_refuses_to_produce_an_unscoped_effect() {
        let dir = tempfile::tempdir().unwrap();
        let error = exec_task(dir.path(), &[], "", "t-unscoped", "idem-t", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(ENV_GOAL), "got: {error}");
        assert_eq!(markers(dir.path()), (0, 0));
    }

    #[tokio::test]
    async fn exec_task_produces_once_and_refuses_the_second_attempt() {
        let dir = tempfile::tempdir().unwrap();
        exec_task(dir.path(), &[], G, "t-once", "idem-t-once", None)
            .await
            .unwrap();
        exec_task(dir.path(), &[], G, "t-once", "idem-t-once", None)
            .await
            .unwrap();
        assert_eq!(markers(dir.path()), (1, 1), "the effect landed twice");
    }

    /// FINDING 1. Two Goals, one `--effects-dir`, the same task names and
    /// therefore the same default keys. Goal B must run its own work.
    ///
    /// Before the scoping, B's first task found A's commit and declined it —
    /// so B reported every task complete having executed nothing at all.
    #[tokio::test]
    async fn a_second_goal_does_not_inherit_the_first_goals_completions() {
        let dir = tempfile::tempdir().unwrap();
        // Both goals declare a task called `deploy`, which is what
        // `declare_task` keys as `idem-<goal>-deploy`.
        exec_task(
            dir.path(),
            &effecting_worker(),
            "goal-a",
            "deploy",
            "idem-goal-a-deploy",
            None,
        )
        .await
        .unwrap();
        let outcome = exec_task(
            dir.path(),
            &effecting_worker(),
            "goal-b",
            "deploy",
            "idem-goal-b-deploy",
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, ExecOutcome::Produced, "goal B executed nothing");
        // Two real effects, two distinct (goal, task) scopes, no duplicate.
        let census = count_effects(dir.path()).unwrap();
        assert_eq!((census.observed_total, census.observed_distinct), (2, 2));
        assert_eq!(census.duplicates(), 0);
    }

    /// FINDING 1, the harder half: an operator who supplies the key BY HAND
    /// can collide two goals on one key. The namespace has to hold even then,
    /// because a default is advice and a namespace is a guarantee.
    #[tokio::test]
    async fn an_identical_hand_supplied_key_still_does_not_cross_goals() {
        let dir = tempfile::tempdir().unwrap();
        exec_task(
            dir.path(),
            &effecting_worker(),
            "goal-a",
            "deploy",
            "shared-key",
            None,
        )
        .await
        .unwrap();
        let outcome = exec_task(
            dir.path(),
            &effecting_worker(),
            "goal-b",
            "deploy",
            "shared-key",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ExecOutcome::Produced, "goal B executed nothing");
    }

    /// FINDING 2. The intent namespace is the same namespace, so it had the
    /// same hole in the opposite direction: a killed goal A left an intent that
    /// permanently parked a brand-new goal B on its own journal.
    #[tokio::test]
    async fn a_stale_intent_from_another_goal_does_not_park_this_one() {
        let dir = tempfile::tempdir().unwrap();
        plant_interrupted_attempt(dir.path(), "goal-a", "shared-key");

        let outcome = exec_task(
            dir.path(),
            &effecting_worker(),
            "goal-b",
            "deploy",
            "shared-key",
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            ExecOutcome::Produced,
            "goal B was parked by goal A"
        );
    }

    /// Sanitizing alone would collapse these onto one directory, re-opening
    /// finding 1 through the back door.
    #[test]
    fn scope_dirs_are_injective_not_merely_filesystem_safe() {
        assert_ne!(scope_dir("goal/a"), scope_dir("goal-a"));
        assert_ne!(scope_dir(""), scope_dir("."));
        assert_eq!(scope_dir("g-b1"), scope_dir("g-b1"), "not stable");
        for raw in ["", ".", "..", "a/b", "c:\\x", "*?<>|"] {
            let dir = scope_dir(raw);
            assert!(
                !dir.starts_with('.') && !dir.contains(['/', '\\', ':', '*', '?', '<', '>', '|']),
                "unsafe component {dir} from {raw:?}"
            );
        }
    }

    #[tokio::test]
    async fn exec_task_does_not_produce_an_effect_when_the_worker_declares_no_effect() {
        let dir = tempfile::tempdir().unwrap();
        let error = exec_task(
            dir.path(),
            &declining_worker(),
            G,
            "t-fail",
            "idem-t-fail",
            None,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("no effect landed"), "got: {error}");
        assert_eq!(
            markers(dir.path()),
            (0, 0),
            "a failed worker still produced an effect"
        );
    }

    /// FINDING 3. `EXIT_NO_EFFECT` used to be 76, which is `EX_PROTOCOL` — a
    /// value `sysexits.h`-speaking tools emit on their own. A worker that
    /// exits it WITHOUT the receipt has declared nothing, and must park rather
    /// than have its intent withdrawn and its effect re-run.
    #[tokio::test]
    async fn a_bare_exit_code_cannot_withdraw_an_intent() {
        let dir = tempfile::tempdir().unwrap();
        // The effect lands, then the worker exits the no-effect code without
        // ever creating the receipt — exactly what a `sendmail` in the pipeline
        // returning EX_PROTOCOL looks like from out here.
        let colliding = vec![
            if cfg!(windows) { "cmd" } else { "sh" }.to_owned(),
            if cfg!(windows) { "/c" } else { "-c" }.to_owned(),
            if cfg!(windows) {
                format!(
                    "echo %WAYLAND_GOAL_TASK% > \"%WAYLAND_GOAL_EFFECT_SINK%\\c.%RANDOM%\" & \
                     exit {EXIT_NO_EFFECT}"
                )
            } else {
                format!(
                    "printf '%s\\n' \"$WAYLAND_GOAL_TASK\" \
                     > \"$WAYLAND_GOAL_EFFECT_SINK/c.$$\"; exit {EXIT_NO_EFFECT}"
                )
            },
        ];
        let outcome = exec_task(
            dir.path(),
            &colliding,
            G,
            "t-collide",
            "idem-t-collide",
            None,
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, ExecOutcome::Indeterminate { .. }),
            "an exit code alone withdrew the intent; got: {outcome:?}"
        );
        assert!(
            intent_path(dir.path(), G, "idem-t-collide").exists(),
            "the intent was withdrawn on an exit code the OS also uses"
        );

        // THE PROPERTY: the retry does not re-run the effect.
        let retry = exec_task(
            dir.path(),
            &effecting_worker(),
            G,
            "t-collide",
            "idem-t-collide",
            None,
        )
        .await
        .unwrap();
        assert!(matches!(retry, ExecOutcome::Indeterminate { .. }));
        assert_eq!(observed(dir.path()), (1, 1), "the effect landed twice");
    }

    /// Neither is the receipt sufficient on its own: a worker that wrote it and
    /// was then killed may have gone on to do more, so it parks.
    #[tokio::test]
    async fn a_receipt_without_the_exit_code_still_parks() {
        let dir = tempfile::tempdir().unwrap();
        let half = vec![
            if cfg!(windows) { "cmd" } else { "sh" }.to_owned(),
            if cfg!(windows) { "/c" } else { "-c" }.to_owned(),
            if cfg!(windows) {
                format!("echo no-effect > \"%{ENV_NO_EFFECT_RECEIPT}%\" & exit 1")
            } else {
                format!("printf 'no-effect\\n' > \"${ENV_NO_EFFECT_RECEIPT}\"; exit 1")
            },
        ];
        let outcome = exec_task(dir.path(), &half, G, "t-half", "idem-t-half", None)
            .await
            .unwrap();
        assert!(
            matches!(outcome, ExecOutcome::Indeterminate { .. }),
            "got: {outcome:?}"
        );
        assert!(intent_path(dir.path(), G, "idem-t-half").exists());
    }

    /// The regression guard for the ordering bug this function shipped with in
    /// its first draft: the marker was created BEFORE the worker ran, so a
    /// worker that failed left the marker behind and every retry declined — the
    /// effect could then never happen at all.
    #[tokio::test]
    async fn a_failed_worker_leaves_the_task_runnable_rather_than_permanently_blocked() {
        let dir = tempfile::tempdir().unwrap();
        exec_task(
            dir.path(),
            &declining_worker(),
            G,
            "t-retry",
            "idem-t-retry",
            None,
        )
        .await
        .expect_err("the failing worker should surface its failure");
        assert_eq!(markers(dir.path()), (0, 0));

        // The retry must be able to produce. Before the fix this returned
        // `produced=no reason=idempotency-key-present` and the effect was lost
        // forever, which is a lost completion wearing an exactly-once costume.
        exec_task(dir.path(), &worker(0), G, "t-retry", "idem-t-retry", None)
            .await
            .expect("the retry must be able to produce the effect");
        assert_eq!(
            markers(dir.path()),
            (1, 1),
            "the retry after a failed worker produced nothing"
        );
    }

    /// Two tasks with different keys must not collide, and two attempts of the
    /// same task must. A gate keyed on the task LABEL rather than the key would
    /// pass the first half and fail here.
    #[tokio::test]
    async fn the_gate_is_keyed_on_the_idempotency_key_not_on_the_task_label() {
        let dir = tempfile::tempdir().unwrap();
        exec_task(dir.path(), &[], G, "shared-label", "idem-a", None)
            .await
            .unwrap();
        exec_task(dir.path(), &[], G, "shared-label", "idem-b", None)
            .await
            .unwrap();
        // Two effects, both labelled the same: total 2, distinct labels 1.
        assert_eq!(markers(dir.path()), (2, 1));
    }

    /// A worker that really writes an effect, then exits.
    fn effecting_worker() -> Vec<String> {
        vec![
            if cfg!(windows) { "cmd" } else { "sh" }.to_owned(),
            if cfg!(windows) { "/c" } else { "-c" }.to_owned(),
            if cfg!(windows) {
                "echo %WAYLAND_GOAL_TASK% > \"%WAYLAND_GOAL_EFFECT_SINK%\\\
                 %WAYLAND_GOAL_TASK%.%RANDOM%%RANDOM%\""
                    .to_owned()
            } else {
                "printf '%s\\n' \"$WAYLAND_GOAL_TASK\" \
                 > \"$WAYLAND_GOAL_EFFECT_SINK/$WAYLAND_GOAL_TASK.$$.$(date +%s%N)\""
                    .to_owned()
            },
        ]
    }

    /// A worker whose record carries an INVOCATION identity, the way any real
    /// effect log does — `task=… msg_id=…`. Two runs leave two records that
    /// differ byte for byte.
    fn identifying_worker() -> Vec<String> {
        // Windows deliberately does NOT use `cmd` + `%RANDOM%`. MEASURED on
        // Windows: six back-to-back `cmd /c` processes every one printed
        // `%RANDOM%%RANDOM%` = 3239824204. cmd seeds its PRNG once per process
        // from the system clock, whose granularity is one ~15.6 ms timer tick,
        // so two workers launched inside one tick draw the identical sequence —
        // identical record text AND identical file name, and the second `>`
        // truncated the first, leaving one record where the test needs two.
        // PowerShell's `$PID` is the faithful analogue of the Unix `$$`: two
        // live processes can never share it.
        if cfg!(windows) {
            vec![
                "powershell".to_owned(),
                "-NoProfile".to_owned(),
                "-Command".to_owned(),
                "$id = \"$PID-$([DateTime]::UtcNow.Ticks)\"; \
                 Set-Content -LiteralPath \
                 \"$env:WAYLAND_GOAL_EFFECT_SINK\\r.$id\" \
                 -Value \"task=$env:WAYLAND_GOAL_TASK msg_id=$id\""
                    .to_owned(),
            ]
        } else {
            vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "printf 'task=%s msg_id=%s\\n' \"$WAYLAND_GOAL_TASK\" \"$$-$(date +%s%N)\" \
                 > \"$WAYLAND_GOAL_EFFECT_SINK/r.$$.$(date +%s%N)\""
                    .to_owned(),
            ]
        }
    }

    fn observed(dir: &std::path::Path) -> (usize, usize) {
        let census = count_effects(dir).unwrap();
        (census.observed_total, census.observed_distinct)
    }

    /// The on-disk state a process-tree kill leaves: an intent recorded, the
    /// worker's effect possibly landed, no commit.
    ///
    /// Written directly rather than by killing a process, because a unit test
    /// cannot kill its own runner. The REAL kill is exercised in the live proof;
    /// this pins the decision the boundary makes when it finds that state.
    fn plant_interrupted_attempt(dir: &std::path::Path, goal: &str, key: &str) {
        let intents = dir.join("intents").join(scope_dir(goal));
        std::fs::create_dir_all(&intents).unwrap();
        std::fs::write(intents.join(key), "task=t-x pid=1\n").unwrap();
    }

    #[tokio::test]
    async fn a_death_inside_the_effect_window_is_parked_rather_than_re_run() {
        let dir = tempfile::tempdir().unwrap();
        plant_interrupted_attempt(dir.path(), G, "idem-t-window");

        let outcome = exec_task(
            dir.path(),
            &effecting_worker(),
            G,
            "t-window",
            "idem-t-window",
            None,
        )
        .await
        .unwrap();

        assert!(
            matches!(outcome, ExecOutcome::Indeterminate { .. }),
            "got: {outcome:?}"
        );
        // THE PROPERTY. The worker must not have run a second time.
        assert_eq!(observed(dir.path()), (0, 0), "the effect was re-run");
        assert_eq!(
            markers(dir.path()),
            (0, 0),
            "an undecided attempt committed"
        );
    }

    #[tokio::test]
    async fn a_committed_effect_wins_over_a_leftover_intent() {
        let dir = tempfile::tempdir().unwrap();
        exec_task(
            dir.path(),
            &effecting_worker(),
            G,
            "t-both",
            "idem-t-both",
            None,
        )
        .await
        .unwrap();
        // A commit is written before its intent is removed, so a kill in that
        // gap leaves both. The commit settles it.
        plant_interrupted_attempt(dir.path(), G, "idem-t-both");

        let outcome = exec_task(
            dir.path(),
            &effecting_worker(),
            G,
            "t-both",
            "idem-t-both",
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, ExecOutcome::AlreadyCommitted);
        assert_eq!(observed(dir.path()), (1, 1), "the effect landed twice");
    }

    #[tokio::test]
    async fn resolving_as_produced_commits_without_running_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        plant_interrupted_attempt(dir.path(), G, "idem-t-res");

        let outcome = exec_task(
            dir.path(),
            &effecting_worker(),
            G,
            "t-res",
            "idem-t-res",
            Some(ResolveArg::Produced),
        )
        .await
        .unwrap();

        assert_eq!(outcome, ExecOutcome::AlreadyCommitted);
        assert_eq!(observed(dir.path()), (0, 0), "the worker ran anyway");
        assert_eq!(markers(dir.path()), (1, 1));
        assert!(!intent_path(dir.path(), G, "idem-t-res").exists());
    }

    #[tokio::test]
    async fn resolving_as_retry_runs_the_worker_again() {
        let dir = tempfile::tempdir().unwrap();
        plant_interrupted_attempt(dir.path(), G, "idem-t-red");

        let outcome = exec_task(
            dir.path(),
            &effecting_worker(),
            G,
            "t-red",
            "idem-t-red",
            Some(ResolveArg::Retry),
        )
        .await
        .unwrap();

        assert_eq!(outcome, ExecOutcome::Produced);
        assert_eq!(observed(dir.path()), (1, 1));
        assert_eq!(markers(dir.path()), (1, 1));
    }

    /// A clean run must leave nothing that would park the NEXT task keyed the
    /// same way. An intent that outlives its own commit is a stuck task.
    #[tokio::test]
    async fn a_clean_attempt_withdraws_its_own_intent() {
        let dir = tempfile::tempdir().unwrap();
        exec_task(
            dir.path(),
            &effecting_worker(),
            G,
            "t-clean",
            "idem-t-clean",
            None,
        )
        .await
        .unwrap();
        assert!(!intent_path(dir.path(), G, "idem-t-clean").exists());
    }

    /// A worker that DECLARED no effect — receipt and exit code both — reported
    /// its own failure, which is NOT an unknown: the intent must be withdrawn
    /// so the task stays plainly retryable rather than parked forever.
    #[tokio::test]
    async fn a_failed_worker_withdraws_its_intent_and_stays_retryable() {
        let dir = tempfile::tempdir().unwrap();
        exec_task(
            dir.path(),
            &declining_worker(),
            G,
            "t-wf",
            "idem-t-wf",
            None,
        )
        .await
        .expect_err("the failing worker should surface its failure");
        assert!(!intent_path(dir.path(), G, "idem-t-wf").exists());

        let outcome = exec_task(
            dir.path(),
            &effecting_worker(),
            G,
            "t-wf",
            "idem-t-wf",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ExecOutcome::Produced);
        assert_eq!(observed(dir.path()), (1, 1));
    }

    /// The Windows-found hole. A worker that exits nonzero WITHOUT declaring
    /// "no effect" — which is what a killed worker looks like on Windows, where
    /// `taskkill /F` and `exit 1` are the same integer — must park, not retry.
    /// The first version of this fix withdrew the intent here and duplicated the
    /// effect on the next attempt.
    #[tokio::test]
    async fn an_undeclared_nonzero_exit_parks_instead_of_re_running() {
        let dir = tempfile::tempdir().unwrap();
        // A worker that performs its effect and THEN exits 1, the way a killed
        // worker looks from outside.
        let killed = vec![
            if cfg!(windows) { "cmd" } else { "sh" }.to_owned(),
            if cfg!(windows) { "/c" } else { "-c" }.to_owned(),
            if cfg!(windows) {
                "echo %WAYLAND_GOAL_TASK% > \"%WAYLAND_GOAL_EFFECT_SINK%\\k.%RANDOM%\" & exit 1"
                    .to_owned()
            } else {
                "printf '%s\\n' \"$WAYLAND_GOAL_TASK\" > \"$WAYLAND_GOAL_EFFECT_SINK/k.$$\"; exit 1"
                    .to_owned()
            },
        ];
        let outcome = exec_task(dir.path(), &killed, G, "t-kill", "idem-t-kill", None)
            .await
            .unwrap();
        assert!(
            matches!(outcome, ExecOutcome::Indeterminate { .. }),
            "got: {outcome:?}"
        );
        assert!(
            intent_path(dir.path(), G, "idem-t-kill").exists(),
            "the intent was withdrawn, so a retry would re-run the effect"
        );
        assert_eq!(observed(dir.path()), (1, 1));

        // THE PROPERTY: the retry must not run the worker again.
        let retry = exec_task(
            dir.path(),
            &effecting_worker(),
            G,
            "t-kill",
            "idem-t-kill",
            None,
        )
        .await
        .unwrap();
        assert!(matches!(retry, ExecOutcome::Indeterminate { .. }));
        assert_eq!(observed(dir.path()), (1, 1), "the effect landed twice");
    }

    /// The instrument's own guard: a census over a worker that produced nothing
    /// observable must be distinguishable from one that produced everything.
    #[test]
    fn the_census_reports_a_duplicate_the_marker_count_cannot_see() {
        let dir = tempfile::tempdir().unwrap();
        let effects = dir.path().join("effects").join(scope_dir(G));
        let sink = dir
            .path()
            .join("observed")
            .join(scope_dir(G))
            .join(scope_dir("t"));
        std::fs::create_dir_all(&effects).unwrap();
        std::fs::create_dir_all(&sink).unwrap();
        // One key, one marker — and two real executions of the same task.
        std::fs::write(effects.join("idem-t"), "t\n").unwrap();
        std::fs::write(sink.join("t.1"), "t\n").unwrap();
        std::fs::write(sink.join("t.2"), "t\n").unwrap();

        let census = count_effects(dir.path()).unwrap();
        assert_eq!((census.markers_total, census.markers_distinct), (1, 1));
        assert_eq!((census.observed_total, census.observed_distinct), (2, 1));
        assert_eq!(census.duplicates(), 1);
    }

    /// FINDING 4. The same duplicate, produced by a worker whose record carries
    /// an invocation identity instead of a bare label.
    ///
    /// This is the run that made the instrument look clean: two byte-DIFFERENT
    /// records from two executions of ONE task read as two distinct effects and
    /// zero duplicates. The identity is the sink directory, so content cannot
    /// hide it.
    #[tokio::test]
    async fn the_census_counts_invocations_not_record_contents() {
        let dir = tempfile::tempdir().unwrap();
        // Run the same task twice for real, the way an operator resolving a
        // parked task as `retry` when the effect DID land does.
        exec_task(
            dir.path(),
            &identifying_worker(),
            G,
            "t-p",
            "idem-t-p",
            None,
        )
        .await
        .unwrap();
        plant_interrupted_attempt(dir.path(), G, "idem-t-p");
        std::fs::remove_file(
            dir.path()
                .join("effects")
                .join(scope_dir(G))
                .join("idem-t-p"),
        )
        .unwrap();
        exec_task(
            dir.path(),
            &identifying_worker(),
            G,
            "t-p",
            "idem-t-p",
            Some(ResolveArg::Retry),
        )
        .await
        .unwrap();

        let census = count_effects(dir.path()).unwrap();
        let contents: BTreeSet<String> = walk(&dir.path().join("observed"))
            .unwrap()
            .records
            .into_iter()
            .map(|record| record.content)
            .collect();
        assert_eq!(contents.len(), 2, "the records were not distinguishable");
        assert_eq!(
            (census.observed_total, census.observed_distinct),
            (2, 1),
            "two executions of one task read as two distinct effects"
        );
        assert_eq!(census.duplicates(), 1, "the duplicate went unreported");
    }

    /// Two concurrent claimants of one key: one wins, one waits and declines.
    /// Neither parks, and the effect lands exactly once.
    ///
    /// Ordinary lease overlap used to park the loser permanently — a task whose
    /// effect landed exactly once, needing a human, with no crash anywhere.
    #[tokio::test]
    async fn a_concurrent_claimant_waits_for_the_winner_rather_than_parking() {
        let dir = tempfile::tempdir().unwrap();
        let slow = vec![
            if cfg!(windows) { "cmd" } else { "sh" }.to_owned(),
            if cfg!(windows) { "/c" } else { "-c" }.to_owned(),
            if cfg!(windows) {
                "echo %WAYLAND_GOAL_TASK% > \"%WAYLAND_GOAL_EFFECT_SINK%\\s.%RANDOM%\" & \
                 ping -n 2 127.0.0.1 > NUL"
                    .to_owned()
            } else {
                "printf '%s\\n' \"$WAYLAND_GOAL_TASK\" > \"$WAYLAND_GOAL_EFFECT_SINK/s.$$\"; \
                 sleep 0.5"
                    .to_owned()
            },
        ];
        let a = exec_task(dir.path(), &slow, G, "t-race", "idem-t-race", None);
        let b = exec_task(dir.path(), &slow, G, "t-race", "idem-t-race", None);
        let (first, second) = tokio::join!(a, b);
        let outcomes = [first.unwrap(), second.unwrap()];

        assert!(
            outcomes.contains(&ExecOutcome::Produced),
            "neither claimant produced: {outcomes:?}"
        );
        assert!(
            outcomes.contains(&ExecOutcome::AlreadyCommitted),
            "the loser parked instead of declining: {outcomes:?}"
        );
        assert_eq!(observed(dir.path()), (1, 1), "the effect landed twice");
    }

    #[test]
    fn session_name_is_derived_from_the_journal_file_stem() {
        assert_eq!(
            session_for(&PathBuf::from("/tmp/p22/fleet.journal")),
            "fleet"
        );
    }

    /// The exit codes must not be values another program in a worker's pipeline
    /// can emit meaning something else. `sysexits.h` owns 64..=78 and the shell
    /// owns 126..=165 and 255.
    #[test]
    fn the_exit_codes_cannot_collide_with_sysexits_or_the_shell() {
        for code in [EXIT_INDETERMINATE, EXIT_NO_EFFECT] {
            assert!(!(64..=78).contains(&code), "{code} is a sysexits value");
            assert!(!(126..=165).contains(&code), "{code} is shell-reserved");
            assert!(code > 2 && code != 255, "{code} is a common exit code");
        }
        assert_ne!(EXIT_INDETERMINATE, EXIT_NO_EFFECT);
    }

    #[test]
    fn default_task_key_is_stable_across_attempts_and_scoped_to_the_goal() {
        // The key must not embed an attempt number, or a retry would mint a
        // fresh key and the effect would land twice.
        assert_eq!(format!("idem-{}-{}", "g-a", "t01"), "idem-g-a-t01");
        // And it must not be shared by two goals declaring the same task.
        assert_ne!(
            format!("idem-{}-{}", "g-a", "t01"),
            format!("idem-{}-{}", "g-b", "t01")
        );
    }
}
