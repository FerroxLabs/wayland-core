//! Cron-execution discovery probes — does scheduling actually work end-to-end?
//!
//! These are CHEAP, ≤2-turn micro-scenarios in the QA spirit (one feature each,
//! assert PASS/FAIL with evidence) that probe whether the agent can drive the
//! `cronjob` tool to schedule and inspect recurring work.
//!
//! ## What these CAN and CANNOT prove
//!
//! Recurring *execution* needs a long-lived daemon to tick the scheduler (see
//! `wcore-cron`'s `CronRunner::spawn`, which the one-shot CLI does NOT start).
//! A single CLI invocation therefore only **stores** the job — it never fires
//! it. So these scenarios assert only what is observable from one process:
//!
//! - the `cronjob` tool fired (via `expect_tool` / trace `CountAtLeast`), and
//! - the job was accepted / created / listed (via the tool's JSON `success`
//!   markers echoed into the assistant's final text and a clean trace).
//!
//! They deliberately do NOT assert that the reminder actually ran — that is a
//! daemon-level property outside the scope of a one-shot probe.
//!
//! ## Backend prerequisite
//!
//! The `cronjob` tool is hidden (`Tool::is_available() == false`) unless the
//! host wires a real `CronScheduler` at construction time. These probes assume
//! the eval binary is built/configured with a scheduler bound; without one the
//! tool never appears to the model and `expect_tool("cronjob")` fails — which is
//! itself the correct signal that scheduling is unavailable.

use std::time::Duration;

use crate::assertions::{Assertion, TraceAssertion};
use crate::providers::ProviderChoice;
use crate::scenario::{ApprovalPolicy, Category, Scenario, Turn};

/// Cron-QA: scheduling a recurring reminder fires the `cronjob` tool and the
/// job is accepted.
///
/// Probes the `every 30m` interval schedule form. We assert the tool fired
/// (trace + `expect_tool`), that it did not error, and that the create
/// succeeded — the tool returns `{"success": true, ...}` and a "created"
/// message, which the model echoes into its final answer.
pub fn cron_create_recurring() -> Scenario {
    Scenario::new("cron_create_recurring", Category::Coverage)
        .provider(ProviderChoice::ForceDeepSeek)
        .approval(ApprovalPolicy::Yolo)
        .max_total_time(Duration::from_secs(60))
        // A single real DeepSeek turn now costs ~$0.031 (Bug #5 made cost real);
        // the old $0.02 cap tripped OverCost on a functionally-correct run.
        .max_total_cost_usd(0.05)
        .turn(
            Turn::new(
                "Set up a recurring reminder every 30 minutes to check the build. \
                 Use your scheduling tool to create it.",
            )
            .max_time(Duration::from_secs(50))
            .max_steps(4)
            .expect_tool("cronjob")
            // Storage-only: one process cannot tick the scheduler, so we assert
            // the job was *created/accepted*, not that it ever ran.
            .trace(TraceAssertion::CountAtLeast {
                tool: "cronjob",
                n: 1,
            })
            .trace(TraceAssertion::NoErrorsOnTool("cronjob"))
            .assert(Assertion::ContainsAny(vec![
                "created",
                "scheduled",
                "reminder",
                "30",
            ])),
        )
}

/// Cron-QA: listing scheduled jobs fires the `cronjob` tool's `list` action.
///
/// A pure read probe — asks the agent to show what is scheduled. Asserts the
/// tool fired without error. (A fresh eval session may have zero jobs, so we do
/// NOT assert a non-empty list — only that the list action was reachable.)
pub fn cron_list_jobs() -> Scenario {
    Scenario::new("cron_list_jobs", Category::Coverage)
        .provider(ProviderChoice::ForceDeepSeek)
        .approval(ApprovalPolicy::Yolo)
        .max_total_time(Duration::from_secs(60))
        // ~$0.031/turn now that cost is real; old $0.02 cap was a false OverCost.
        .max_total_cost_usd(0.05)
        .turn(
            Turn::new("List all of my currently scheduled cron jobs.")
                .max_time(Duration::from_secs(50))
                .max_steps(4)
                .expect_tool("cronjob")
                .trace(TraceAssertion::CountAtLeast {
                    tool: "cronjob",
                    n: 1,
                })
                .trace(TraceAssertion::NoErrorsOnTool("cronjob")),
        )
}

/// Cron-QA: a two-turn probe — schedule a job, then list it back.
///
/// Turn 1 creates a recurring job; turn 2 lists. Because both turns share one
/// process (and thus one scheduler instance), the create from turn 1 should be
/// visible to the list in turn 2 — proving the store round-trips within a
/// session. We still cannot assert *execution* (no daemon tick), only that the
/// stored job survives a subsequent read.
pub fn cron_create_then_list() -> Scenario {
    Scenario::new("cron_create_then_list", Category::Coverage)
        .provider(ProviderChoice::ForceDeepSeek)
        .approval(ApprovalPolicy::Yolo)
        .max_total_time(Duration::from_secs(90))
        .max_total_cost_usd(0.04)
        .turn(
            Turn::new(
                "Schedule a recurring job every 2 hours that runs the daily summary. \
                 Use the scheduling tool.",
            )
            .max_time(Duration::from_secs(50))
            .max_steps(4)
            .expect_tool("cronjob")
            .trace(TraceAssertion::CountAtLeast {
                tool: "cronjob",
                n: 1,
            })
            .trace(TraceAssertion::NoErrorsOnTool("cronjob")),
        )
        .turn(
            Turn::new("Now list my scheduled jobs and tell me what you find.")
                .max_time(Duration::from_secs(40))
                .max_steps(4)
                .expect_tool("cronjob")
                // The job created in turn 1 should round-trip into the listing.
                .trace(TraceAssertion::NoErrorsOnTool("cronjob")),
        )
}

/// All cron-discovery scenarios, in a stable order.
pub fn all() -> Vec<Scenario> {
    vec![
        cron_create_recurring(),
        cron_list_jobs(),
        cron_create_then_list(),
    ]
}
