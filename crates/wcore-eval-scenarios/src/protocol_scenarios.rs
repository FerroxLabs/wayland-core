//! Protocol-command (D2) scenarios — exercise the json-stream control
//! channel BEYOND the per-turn `Message` path: `set_config` (model swap),
//! `set_mode` (approval-posture change), and `stop` (mid-turn cancel).
//!
//! These are the QA-style micro-scenarios for the host control surface. Unlike
//! [`crate::qa`] (which pokes a slash command), each scenario here drives a
//! `ProtocolCommand` and asserts the engine's CONTROL response — the
//! `config_changed` / `info` events — captured by the runner into
//! [`crate::runner::ScenarioResult::info_events`].
//!
//! ## How capture works (runner D2)
//!
//! The runner sends a turn's [`crate::scenario::TurnCommand`] pre-commands
//! BEFORE the turn's user message, then drains the engine's standalone-command
//! response. A successful `set_config`/`set_mode` emits an `info` ack
//! (`"config updated: …"` / `"mode updated: …"`) FOLLOWED by `config_changed`;
//! the runner folds the `config_changed` payload into `info_events` as a
//! synthetic `"config_changed: {capabilities-json}"` line. Both the ack and
//! the synthetic line are assertable via
//! [`crate::assertions::Assertion::InfoContains`].
//!
//! ## Stop-mid-turn
//!
//! [`crate::scenario::Turn::stop_mid_turn`] makes the runner send `stop` right
//! after the turn's first event. The engine breaks its run future and emits
//! `stream_end`, so the turn halts. The scenario keeps a generous `max_time`
//! and a low `max_steps` budget; the real assertion is that the run COMPLETES
//! (doesn't hang to the wall-time `Hung` failure) — proving cancellation took.

use std::time::Duration;

use crate::assertions::Assertion;
use crate::providers::ProviderChoice;
use crate::scenario::{Category, Scenario, Turn, TurnCommand};

/// SetConfig model-swap: send `set_config { model: "deepseek-reasoner" }`
/// before the turn, assert the engine acknowledged the swap with an `info`
/// ack AND emitted `config_changed`.
///
/// Assumes the spawn model is NOT already `deepseek-reasoner` (the persona
/// matrix spawns DeepSeek with `deepseek-chat`); if it were, the swap would be
/// a no-op (`info: "set_config: no changes"`, no `config_changed`) and the
/// `config_changed` assertion would correctly FAIL.
pub fn set_config_model_swap() -> Scenario {
    Scenario::new("protocol_set_config_model_swap", Category::Coverage)
        .provider(ProviderChoice::ForceDeepSeek)
        .max_total_time(Duration::from_secs(60))
        .max_total_cost_usd(0.02)
        .turn(
            Turn::new("Reply with the single word OK.")
                .max_time(Duration::from_secs(45))
                .max_steps(2)
                .pre_command(TurnCommand::SetConfig {
                    model: Some("deepseek-reasoner".to_string()),
                    thinking: None,
                    effort: None,
                })
                // The engine emits `config_changed` on a real model change…
                .assert(Assertion::InfoContains("config_changed"))
                // …preceded by the `config updated: model: …` info ack.
                .assert(Assertion::InfoContains("config updated: model:")),
        )
}

/// GHSA-8r7g: a wire peer must NOT be able to escalate to Force. This runs the
/// real binary WITHOUT any local-operator opt-in (no --force / no
/// WAYLAND_ALLOW_WIRE_FORCE), sends `set_mode { mode: "force" }` over the
/// protocol, and asserts the engine REFUSES it (rather than acking
/// `mode updated: force`). The plain text turn still completes normally.
pub fn set_mode_force() -> Scenario {
    Scenario::new("protocol_set_mode_force_refused", Category::Coverage)
        .provider(ProviderChoice::ForceDeepSeek)
        .max_total_time(Duration::from_secs(60))
        .max_total_cost_usd(0.02)
        .turn(
            Turn::new("Reply with the single word OK.")
                .max_time(Duration::from_secs(45))
                .max_steps(2)
                .pre_command(TurnCommand::SetMode { mode: "force" })
                // The wire Force request is refused for lack of a local opt-in.
                .assert(Assertion::InfoContains("'force' refused")),
        )
}

/// Stop-mid-turn: give the model a long task, then send `stop` right after the
/// first event of the turn. The engine cancels its run future and emits
/// `stream_end`, so the turn halts and the scenario COMPLETES (rather than
/// running to the wall-time `Hung` guard). The generous `max_total_time` means
/// a `Hung` failure only fires if cancellation did NOT take — making a clean
/// PASS the proof the stop was honored.
pub fn stop_mid_turn() -> Scenario {
    Scenario::new("protocol_stop_mid_turn", Category::Coverage)
        .provider(ProviderChoice::ForceDeepSeek)
        .max_total_time(Duration::from_secs(90))
        .max_total_cost_usd(0.02)
        .turn(
            Turn::new(
                "Count slowly from 1 to 200, writing each number on its own line, \
                 with a short sentence about the number after each one.",
            )
            .max_time(Duration::from_secs(75))
            .max_steps(20)
            .stop_mid_turn(),
        )
}

/// All protocol-command (D2) scenarios, in a stable order.
pub fn all() -> Vec<Scenario> {
    vec![set_config_model_swap(), set_mode_force(), stop_mid_turn()]
}
