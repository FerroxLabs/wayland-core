//! QA-coverage scenarios — strategy doc **Lane Q** (the exhaustive "does every
//! part work?" checklist, complementing the persona user-testing lane).
//!
//! These are SMALL, targeted micro-scenarios that exercise ONE feature each (a
//! slash command, an approval decision, a mode) and assert PASS/FAIL with
//! evidence — the seed of the Commands-QA / Modes-QA specialists. Unlike a
//! persona, a QA check never types a paragraph; it pokes one button and
//! verifies the light turns on.

use std::time::Duration;

use crate::assertions::Assertion;
use crate::providers::ProviderChoice;
use crate::scenario::{ApprovalPolicy, Category, Scenario, Turn};

/// Commands-QA: `/style` acknowledges with "style updated" (D1).
///
/// In json-stream the engine slash-dispatches the message content before the
/// LLM path; `/style <x>` maps to `SlashOutcome::SetStyle` → `emit_info("style
/// updated")`. We assert the `info` event landed.
pub fn slash_style() -> Scenario {
    Scenario::new("qa_slash_style", Category::Coverage)
        .provider(ProviderChoice::ForceDeepSeek)
        .max_total_time(Duration::from_secs(40))
        .max_total_cost_usd(0.01)
        .turn(
            Turn::new("/style terse")
                .max_time(Duration::from_secs(30))
                .assert(Assertion::InfoContains("style updated")),
        )
}

/// Commands-QA: `/clear` clears the conversation and acks "conversation
/// cleared" (D1).
pub fn slash_clear() -> Scenario {
    Scenario::new("qa_slash_clear", Category::Coverage)
        .provider(ProviderChoice::ForceDeepSeek)
        .max_total_time(Duration::from_secs(40))
        .max_total_cost_usd(0.01)
        .turn(
            Turn::new("/clear")
                .max_time(Duration::from_secs(30))
                .assert(Assertion::InfoContains("conversation cleared")),
        )
}

/// Modes-QA: the approval gate ALLOWS a write when the user approves (D3).
///
/// Spawns WITHOUT `--yolo` (engine `Default` mode → `ApprovalRequired` per
/// mutating tool); the runner approves every request. The file must land.
pub fn approval_allow() -> Scenario {
    Scenario::new("qa_approval_allow", Category::Hardening)
        .provider(ProviderChoice::ForceDeepSeek)
        .approval(ApprovalPolicy::ApproveAll)
        .max_total_time(Duration::from_secs(120))
        .max_total_cost_usd(0.05)
        .turn(
            Turn::new("Create a file called approved.txt containing exactly the word HELLO.")
                .max_time(Duration::from_secs(100))
                .expect_tool("Write")
                .assert(Assertion::FileExists("approved.txt"))
                .assert(Assertion::FileContains {
                    path: "approved.txt",
                    needle: "HELLO",
                }),
        )
}

/// Modes-QA: the approval gate BLOCKS a write when the user denies (D3) — the
/// core trust-surface guarantee, currently 0%-tested.
///
/// Spawns WITHOUT `--yolo`; the runner denies every `ApprovalRequired`, so the
/// write must NOT land. `FileAbsent` proves the denial was actually enforced.
pub fn approval_deny() -> Scenario {
    Scenario::new("qa_approval_deny", Category::Hardening)
        .provider(ProviderChoice::ForceDeepSeek)
        .approval(ApprovalPolicy::DenyAll)
        .max_total_time(Duration::from_secs(120))
        .max_total_cost_usd(0.05)
        .turn(
            Turn::new("Create a file called denied.txt containing the word HELLO.")
                .max_time(Duration::from_secs(100))
                .max_steps(6)
                .assert(Assertion::FileAbsent("denied.txt")),
        )
}

/// All QA-coverage scenarios, in a stable order.
pub fn all() -> Vec<Scenario> {
    vec![
        slash_style(),
        slash_clear(),
        approval_allow(),
        approval_deny(),
    ]
}
