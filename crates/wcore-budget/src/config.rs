//! W8a A.5 — `BudgetConfig` TOML schema for `~/.wayland-core/config.toml`.
//!
//! Every cap is optional. The runtime `ExecutionBudget` is constructed from
//! this struct via `From` (defined in `execution.rs`). All fields default
//! to `None`, i.e. "no cap" — opt-in only.
//!
//! Moved verbatim from `wcore-config/src/budget.rs` in M5.3 (`wcore-config`
//! now re-exports this type so all pre-existing call sites compile
//! unchanged).
//!
//! Example TOML:
//!
//! ```toml
//! [budget]
//! max_wall_time_secs    = 600
//! max_tool_runtime_secs = 120
//! max_concurrent_process_tools = 8
//! max_agent_depth       = 4
//! max_tokens_in         = 200000
//! max_tokens_out        = 16384
//! max_cost_usd          = 1.50
//! ```
//!
//! Or, instead of naming eight numbers, name one envelope
//! ([`BudgetPreset`], FerroxLabs/wayland#174 item 6):
//!
//! ```toml
//! [budget]
//! preset = "tiny"          # tiny | small | normal | large | no-hosted-spend
//! max_cost_usd = 0.50      # optional: may TIGHTEN the preset, never widen it
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Error, PartialEq)]
pub enum BudgetConfigError {
    #[error("max_cost_usd must be finite and non-negative, got {0}")]
    InvalidMaxCostUsd(f64),
    #[error("max_daily_cost_usd must be finite and non-negative, got {0}")]
    InvalidMaxDailyCostUsd(f64),
    /// An explicit field sits alongside a `preset` and is LOOSER than it.
    ///
    /// Refused rather than silently clamped, and refused rather than silently
    /// honoured. Both silent outcomes leave the operator believing a number
    /// that is not the one in force — and for `no-hosted-spend`, honouring the
    /// explicit field would defeat the entire point of the preset. Erroring is
    /// the only outcome where what the file says and what the engine does
    /// cannot disagree.
    // The wording deliberately avoids a literal `key = value` shape. The
    // remedy-advertisement gate scrapes error strings for config assignments an
    // operator might paste, and a template placeholder inside one is advice
    // that cannot be followed.
    #[error(
        "invalid [budget]: the {preset} preset caps {field} at {preset_value}, and the \
         explicit {field} of {explicit_value} would WIDEN it. A preset is a ceiling: an \
         explicit field may tighten it, never raise it. Lower that field, remove it, or \
         choose a larger preset."
    )]
    PresetWidened {
        preset: &'static str,
        field: &'static str,
        preset_value: String,
        explicit_value: String,
    },
}

/// A named `[budget]` envelope (FerroxLabs/wayland#174 item 6).
///
/// # How the numbers were chosen
///
/// Every value below is derived, not picked for roundness. Three rules do all
/// the work, and each preset's table documents where it obeys them and where
/// it cannot.
///
/// 1. **`normal` is `smart_default()` verbatim.** It is the envelope every
///    existing installation already runs under. Re-deriving it would change
///    behaviour for people who never asked for a preset, so it is copied, not
///    computed — including its one internal asymmetry (its $25 binds before
///    its 10M/1M token envelope, which costs $45 at Sonnet list). That
///    asymmetry is inherited, not chosen.
///
/// 2. **`max_tokens_out` is a whole multiple of 64,000.** The engine reserves
///    the FULL `request.max_tokens` before every provider call
///    (`engine.rs`: `reserved_output = request.max_tokens`), and
///    `default_max_tokens()` is 64,000. A session cap below that refuses the
///    very first turn; a cap of N x 64,000 admits N maximum-length replies
///    with nothing yet committed. This is why no preset has a "small" output
///    number — the reservation model sets the floor, not taste.
///
/// 3. **Fungible axes scale, host-bound axes do not.** Tokens and dollars are
///    consumed in proportion to the size of the job, so they step
///    geometrically. Wall time, tool runtime, concurrent processes and agent
///    depth are bounded by the machine and by fan-out risk — a ten-times
///    bigger job does not get ten times more machine, and each extra level of
///    delegation MULTIPLIES cost rather than adding to it — so those step
///    additively and are justified in absolute terms instead.
///
/// The USD cap of each derived preset is the Sonnet-4 list price of that
/// preset's OWN token envelope ($3/Mtok in, $15/Mtok out — see
/// `wcore-pricing/pricing.toml`), rounded up to the next whole dollar. That
/// ordering is deliberate: on the default model the TOKEN envelope binds first
/// (a pricing-independent, deterministic stop), and the dollar cap only bites
/// on a model that costs more per token than Sonnet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BudgetPreset {
    /// One question, one answer — a CI probe, a smoke test, a single lookup.
    ///
    /// * `max_tokens_in = 200_000` — exactly one Claude Sonnet context window.
    ///   A session that has read more than one full window is not a single
    ///   question any more.
    /// * `max_tokens_out = 192_000` — 3 x 64,000, so three maximum-length
    ///   replies fit (rule 2).
    /// * `max_cost_usd = 4.00` — Sonnet list for 200k in + 192k out is
    ///   $0.60 + $2.88 = $3.48 (rule 3, rounded up).
    /// * `max_wall_time_secs = 300` — a single question that has not answered
    ///   in five minutes is wedged, not working.
    /// * `max_tool_runtime_secs = 60` — the longest single tool call admitted
    ///   is a unit test or a lint, not a build.
    /// * `max_concurrent_process_tools = 2` — the tool under test plus one
    ///   helper. A tiny session has no reason to fan out.
    /// * `max_agent_depth = 0` — no sub-agents at all. Delegation is precisely
    ///   what turns a bounded job into an unbounded one.
    Tiny,
    /// One focused change: read a few files, edit one, run its tests.
    ///
    /// * `max_tokens_in = 2_000_000` — ten context windows of reading, the
    ///   next geometric step from `tiny` (rule 3).
    /// * `max_tokens_out = 640_000` — 10 x 64,000 (rule 2).
    /// * `max_cost_usd = 16.00` — Sonnet list for that envelope is
    ///   $6.00 + $9.60 = $15.60 (rule 3, rounded up).
    /// * `max_wall_time_secs = 1_800` — one edit-and-test cycle.
    /// * `max_tool_runtime_secs = 300` — the longest single call is a package
    ///   build.
    /// * `max_concurrent_process_tools = 8` — a typical parallel build's job
    ///   count on a developer machine.
    /// * `max_agent_depth = 1` — the session may delegate once; its children
    ///   may not delegate again.
    Small,
    /// The shipped default: [`BudgetConfig::smart_default`], unchanged. See
    /// rule 1 — naming it as a preset must not move it.
    Normal,
    /// An unattended, multi-hour run across a whole repository.
    ///
    /// * `max_tokens_in = 100_000_000` / `max_tokens_out = 9_984_000` — ten
    ///   times `normal` on both token axes (rule 3); the output figure is
    ///   156 x 64,000, the largest whole multiple at or below 10M (rule 2).
    /// * `max_cost_usd = 450.00` — Sonnet list for that envelope is
    ///   $300.00 + $149.76 = $449.76 (rule 3, rounded up).
    /// * `max_wall_time_secs = 86_400` — one full day. Past that, "still
    ///   running" means hung, not progressing.
    /// * `max_tool_runtime_secs = 28_800` — eight hours: a full workspace
    ///   build-and-test matrix as ONE tool call.
    /// * `max_concurrent_process_tools = 64` — a build host's core count.
    ///   Doubled, not multiplied by ten: the money grew, the machine did not.
    /// * `max_agent_depth = 12` — `normal` plus four. Depth multiplies cost
    ///   per level, so it grows additively even when the budget grows tenfold.
    Large,
    /// Zero hosted spend permitted, enforced rather than advertised.
    ///
    /// Operationally identical to `normal` — it forbids SPEND, not WORK — with
    /// `max_cost_usd = 0.00` and `max_daily_cost_usd = 0.00`. Because the
    /// engine reserves the worst-case cost of a call BEFORE dispatching it,
    /// those two zeros are a hard pre-flight stop, not a post-hoc report:
    ///
    /// * a priced model reserves more than $0 and is refused before the
    ///   request is sent ("Provider call not started");
    /// * an UNPRICED model is refused too — an explicit `max_cost_usd` arms
    ///   the engine's existing `unpriced_provider` refusal, which exists
    ///   exactly so an unknown price cannot be silently treated as $0;
    /// * a model whose published price really is $0.00 — a local runtime —
    ///   reserves $0, clears the cap and runs. That is the point of the
    ///   preset: no money may leave, local inference still works.
    ///
    /// Any explicit `max_cost_usd`/`max_daily_cost_usd` above zero alongside
    /// this preset is REFUSED at config resolution
    /// ([`BudgetConfig::resolve_preset`]), so the guarantee cannot be edited
    /// away one field at a time. It can still be raised the way every other
    /// cap can — by an explicit, recorded operator budget grant at runtime;
    /// closing that door is issue #174's no-silent-escalation item, not this
    /// one.
    NoHostedSpend,
}

impl BudgetPreset {
    /// The name this preset is spelled with in `config.toml`. Kept beside the
    /// serde attribute so an error message can never name a variant the parser
    /// would not accept.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tiny => "tiny",
            Self::Small => "small",
            Self::Normal => "normal",
            Self::Large => "large",
            Self::NoHostedSpend => "no-hosted-spend",
        }
    }

    /// The envelope this preset names. See the type-level docs for how each
    /// number was derived.
    #[must_use]
    pub fn config(self) -> BudgetConfig {
        // Every size preset leaves `max_daily_cost_usd` ABSENT on purpose.
        // Every other cap here is per session; a daily ceiling binds ACROSS
        // sessions, so putting one in a size preset would stop the twentieth
        // `tiny` CI probe of the day for reasons that have nothing to do with
        // the size of any one of them. `smart_default()` states the same rule.
        match self {
            Self::Tiny => BudgetConfig {
                preset: Some(self),
                max_wall_time_secs: Some(300),
                max_tool_runtime_secs: Some(60),
                max_processes: Some(2),
                max_agent_depth: Some(0),
                max_tokens_in: Some(200_000),
                max_tokens_out: Some(3 * 64_000),
                max_cost_usd: Some(4.00),
                max_daily_cost_usd: None,
            },
            Self::Small => BudgetConfig {
                preset: Some(self),
                max_wall_time_secs: Some(30 * 60),
                max_tool_runtime_secs: Some(5 * 60),
                max_processes: Some(8),
                max_agent_depth: Some(1),
                max_tokens_in: Some(2_000_000),
                max_tokens_out: Some(10 * 64_000),
                max_cost_usd: Some(16.00),
                max_daily_cost_usd: None,
            },
            Self::Normal => BudgetConfig {
                preset: Some(self),
                ..BudgetConfig::smart_default()
            },
            Self::Large => BudgetConfig {
                preset: Some(self),
                max_wall_time_secs: Some(24 * 60 * 60),
                max_tool_runtime_secs: Some(8 * 60 * 60),
                max_processes: Some(64),
                max_agent_depth: Some(12),
                max_tokens_in: Some(100_000_000),
                max_tokens_out: Some(156 * 64_000),
                max_cost_usd: Some(450.00),
                max_daily_cost_usd: None,
            },
            Self::NoHostedSpend => BudgetConfig {
                preset: Some(self),
                max_cost_usd: Some(0.0),
                max_daily_cost_usd: Some(0.0),
                ..BudgetConfig::smart_default()
            },
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetConfig {
    /// A named envelope to start from (`tiny`/`small`/`normal`/`large`/
    /// `no-hosted-spend`). Expanded into the fields below by
    /// [`BudgetConfig::resolve_preset`] during config resolution, so
    /// everything downstream of `Config` sees concrete numbers and no consumer
    /// has to know presets exist.
    ///
    /// Retained after expansion rather than cleared: it is the audit trail for
    /// where the numbers came from, and it makes expansion idempotent (a
    /// second pass finds every field already equal to its preset value, which
    /// is a tightening of zero and therefore accepted).
    #[serde(default)]
    pub preset: Option<BudgetPreset>,
    pub max_wall_time_secs: Option<u64>,
    pub max_tool_runtime_secs: Option<u64>,
    /// Maximum concurrent tool calls that may spawn native processes. This is
    /// not a descendant-PID limit inside one admitted shell command.
    #[serde(rename = "max_concurrent_process_tools", alias = "max_processes")]
    pub max_processes: Option<usize>,
    pub max_agent_depth: Option<usize>,
    pub max_tokens_in: Option<u64>,
    pub max_tokens_out: Option<u64>,
    pub max_cost_usd: Option<f64>,
    /// Spend ceiling for one UTC day, enforced ACROSS sessions and across
    /// processes through the durable [`crate::daily::DailySpendStore`].
    ///
    /// Every other cap here is per session, so a caller that starts a fresh
    /// session per process — a crash-looping daemon, a cron job, a channel
    /// gateway answering inbound messages — is bounded by none of them: each
    /// run legitimately gets its own budget. This is the only field that binds
    /// such a caller.
    ///
    /// Opt-in: `None` (and the Smart default) leaves the ceiling absent, which
    /// is the historical behaviour.
    pub max_daily_cost_usd: Option<f64>,
}

impl BudgetConfig {
    pub fn validate(&self) -> Result<(), BudgetConfigError> {
        if let Some(usd) = self.max_cost_usd
            && (!usd.is_finite() || usd < 0.0)
        {
            return Err(BudgetConfigError::InvalidMaxCostUsd(usd));
        }
        if let Some(usd) = self.max_daily_cost_usd
            && (!usd.is_finite() || usd < 0.0)
        {
            return Err(BudgetConfigError::InvalidMaxDailyCostUsd(usd));
        }
        Ok(())
    }

    /// Finite defaults for an ordinary interactive Smart session. These are
    /// deliberately generous enough for long builds while still bounding a
    /// lost unattended loop. Explicit configuration replaces each field.
    pub fn smart_default() -> Self {
        Self {
            // Not `Some(Normal)`: `smart_default()` is the implicit envelope an
            // operator who wrote no `[budget]` block gets. Stamping a preset
            // name on it would report a choice nobody made.
            preset: None,
            max_wall_time_secs: Some(8 * 60 * 60),
            max_tool_runtime_secs: Some(4 * 60 * 60),
            max_processes: Some(32),
            max_agent_depth: Some(8),
            max_tokens_in: Some(10_000_000),
            max_tokens_out: Some(1_000_000),
            max_cost_usd: Some(25.0),
            // Deliberately absent: a cross-session daily ceiling is a
            // deployment policy, not a session default. Defaulting it would
            // silently bound long-running multi-session work that has always
            // been unbounded on this axis.
            max_daily_cost_usd: None,
        }
    }

    /// Fill omitted fields from Smart Default without overriding an explicit
    /// operator value. This is applied at runtime bootstrap so the serialized
    /// configuration remains backwards-compatible and auditable.
    pub fn with_smart_defaults(&self) -> Self {
        let defaults = Self::smart_default();
        Self {
            preset: self.preset,
            max_wall_time_secs: self.max_wall_time_secs.or(defaults.max_wall_time_secs),
            max_tool_runtime_secs: self
                .max_tool_runtime_secs
                .or(defaults.max_tool_runtime_secs),
            max_processes: self.max_processes.or(defaults.max_processes),
            max_agent_depth: self.max_agent_depth.or(defaults.max_agent_depth),
            max_tokens_in: self.max_tokens_in.or(defaults.max_tokens_in),
            max_tokens_out: self.max_tokens_out.or(defaults.max_tokens_out),
            max_cost_usd: self.max_cost_usd.or(defaults.max_cost_usd),
            max_daily_cost_usd: self.max_daily_cost_usd.or(defaults.max_daily_cost_usd),
        }
    }

    /// Resolve one session envelope from `[budget]` plus the legacy optional
    /// `[session_cap]` block. Explicit values replace Smart
    /// defaults; when both blocks explicitly constrain the same axis, the
    /// stricter value wins so adding a second policy source cannot widen the
    /// first one by accident.
    /// Expand `preset` into the concrete cap fields.
    ///
    /// Called once, during config resolution, so `Config.budget` — the value
    /// the engine's bootstrap actually converts into an `ExecutionBudget` and
    /// a `BudgetCap` — already carries real numbers. Doing it here rather than
    /// at a consumer is the difference between a preset that works and a
    /// helper function nothing calls.
    ///
    /// # Semantics of an explicit field beside a preset
    ///
    /// An explicit field may **tighten** the preset and is then honoured; a
    /// field that would **widen** it is a hard error
    /// ([`BudgetConfigError::PresetWidened`]).
    ///
    /// Not "explicit always wins", which is the obvious choice and the wrong
    /// one: it would let `preset = "no-hosted-spend"` sit next to
    /// `max_cost_usd = 5.00` and permit $5 of spend, turning the one preset
    /// with a guarantee into a comment. Not "preset always wins" either: an
    /// operator who wants a smaller cap on one axis than their preset gives
    /// has asked for something safe, and refusing it would push them back to
    /// writing all eight numbers out by hand. Tighten-or-refuse is the only
    /// rule under which the strictest number the file mentions is always the
    /// number in force.
    ///
    /// A preset field that is ABSENT (every size preset's
    /// `max_daily_cost_usd`) is unbounded, so any explicit value there is a
    /// tightening and is accepted.
    pub fn resolve_preset(&self) -> Result<Self, BudgetConfigError> {
        let Some(preset) = self.preset else {
            return Ok(self.clone());
        };
        let base = preset.config();
        let name = preset.as_str();
        Ok(Self {
            preset: Some(preset),
            max_wall_time_secs: tighten_u64(
                name,
                "max_wall_time_secs",
                base.max_wall_time_secs,
                self.max_wall_time_secs,
            )?,
            max_tool_runtime_secs: tighten_u64(
                name,
                "max_tool_runtime_secs",
                base.max_tool_runtime_secs,
                self.max_tool_runtime_secs,
            )?,
            max_processes: tighten_usize(
                name,
                "max_concurrent_process_tools",
                base.max_processes,
                self.max_processes,
            )?,
            max_agent_depth: tighten_usize(
                name,
                "max_agent_depth",
                base.max_agent_depth,
                self.max_agent_depth,
            )?,
            max_tokens_in: tighten_u64(
                name,
                "max_tokens_in",
                base.max_tokens_in,
                self.max_tokens_in,
            )?,
            max_tokens_out: tighten_u64(
                name,
                "max_tokens_out",
                base.max_tokens_out,
                self.max_tokens_out,
            )?,
            max_cost_usd: tighten_f64(name, "max_cost_usd", base.max_cost_usd, self.max_cost_usd)?,
            max_daily_cost_usd: tighten_f64(
                name,
                "max_daily_cost_usd",
                base.max_daily_cost_usd,
                self.max_daily_cost_usd,
            )?,
        })
    }

    pub fn effective_session_envelope(budget: &Self, session_cap: Option<&Self>) -> Self {
        let mut effective = budget.with_smart_defaults();
        let Some(session_cap) = session_cap else {
            return effective;
        };
        effective.max_wall_time_secs =
            strictest_u64(budget.max_wall_time_secs, session_cap.max_wall_time_secs)
                .or(effective.max_wall_time_secs);
        effective.max_tool_runtime_secs = strictest_u64(
            budget.max_tool_runtime_secs,
            session_cap.max_tool_runtime_secs,
        )
        .or(effective.max_tool_runtime_secs);
        effective.max_processes = strictest_usize(budget.max_processes, session_cap.max_processes)
            .or(effective.max_processes);
        effective.max_agent_depth =
            strictest_usize(budget.max_agent_depth, session_cap.max_agent_depth)
                .or(effective.max_agent_depth);
        effective.max_tokens_in = strictest_u64(budget.max_tokens_in, session_cap.max_tokens_in)
            .or(effective.max_tokens_in);
        effective.max_tokens_out = strictest_u64(budget.max_tokens_out, session_cap.max_tokens_out)
            .or(effective.max_tokens_out);
        effective.max_cost_usd =
            strictest_f64(budget.max_cost_usd, session_cap.max_cost_usd).or(effective.max_cost_usd);
        effective.max_daily_cost_usd =
            strictest_f64(budget.max_daily_cost_usd, session_cap.max_daily_cost_usd)
                .or(effective.max_daily_cost_usd);
        effective
    }
}

/// One axis of [`BudgetConfig::resolve_preset`]: keep the explicit value when
/// it is at least as strict as the preset's, refuse it when it is looser.
///
/// `None` on the preset side means that axis is unbounded there, so ANY
/// explicit value tightens it. `None` on the explicit side means the operator
/// said nothing and inherits the preset.
macro_rules! tighten {
    ($name:ident, $ty:ty) => {
        fn $name(
            preset: &'static str,
            field: &'static str,
            preset_value: Option<$ty>,
            explicit: Option<$ty>,
        ) -> Result<Option<$ty>, BudgetConfigError> {
            match (preset_value, explicit) {
                (_, None) => Ok(preset_value),
                (None, Some(value)) => Ok(Some(value)),
                (Some(limit), Some(value)) if value <= limit => Ok(Some(value)),
                (Some(limit), Some(value)) => Err(BudgetConfigError::PresetWidened {
                    preset,
                    field,
                    preset_value: limit.to_string(),
                    explicit_value: value.to_string(),
                }),
            }
        }
    };
}

tighten!(tighten_u64, u64);
tighten!(tighten_usize, usize);
tighten!(tighten_f64, f64);

fn strictest_usize(left: Option<usize>, right: Option<usize>) -> Option<usize> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn strictest_u64(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn strictest_f64(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml_gives_default() {
        let bc: BudgetConfig = toml::from_str("").unwrap();
        assert_eq!(bc, BudgetConfig::default());
        assert!(bc.max_wall_time_secs.is_none());
        assert!(bc.max_cost_usd.is_none());
    }

    #[test]
    fn explicit_fields_parsed() {
        let bc: BudgetConfig = toml::from_str(
            r#"
                max_wall_time_secs = 600
                max_tokens_out = 16384
                max_cost_usd = 1.5
            "#,
        )
        .unwrap();
        assert_eq!(bc.max_wall_time_secs, Some(600));
        assert_eq!(bc.max_tokens_out, Some(16384));
        assert_eq!(bc.max_cost_usd, Some(1.5));
        assert!(bc.max_processes.is_none());
    }

    #[test]
    fn roundtrip_toml() {
        let original = BudgetConfig {
            max_wall_time_secs: Some(300),
            max_processes: Some(4),
            max_cost_usd: Some(0.25),
            ..Default::default()
        };
        let s = toml::to_string(&original).unwrap();
        assert!(s.contains("max_concurrent_process_tools = 4"));
        assert!(!s.contains("max_processes"));
        let parsed: BudgetConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn legacy_max_processes_alias_remains_readable() {
        let parsed: BudgetConfig = toml::from_str("max_processes = 7").unwrap();
        assert_eq!(parsed.max_processes, Some(7));
    }

    #[test]
    fn smart_defaults_are_finite_and_explicit_values_win() {
        let effective = BudgetConfig {
            max_cost_usd: Some(5.0),
            ..Default::default()
        }
        .with_smart_defaults();

        assert_eq!(effective.max_cost_usd, Some(5.0));
        assert!(effective.max_wall_time_secs.is_some());
        assert!(effective.max_tool_runtime_secs.is_some());
        assert!(effective.max_processes.is_some());
        assert!(effective.max_agent_depth.is_some());
        assert!(effective.max_tokens_in.is_some());
        assert!(effective.max_tokens_out.is_some());
    }

    #[test]
    fn rejects_non_finite_and_negative_cost_caps() {
        for usd in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01] {
            let config = BudgetConfig {
                max_cost_usd: Some(usd),
                ..Default::default()
            };
            assert!(config.validate().is_err(), "accepted {usd}");
        }
        assert!(
            BudgetConfig {
                max_cost_usd: Some(0.0),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn daily_cost_cap_has_a_toml_counterpart() {
        let parsed: BudgetConfig = toml::from_str("max_daily_cost_usd = 12.5").unwrap();
        assert_eq!(parsed.max_daily_cost_usd, Some(12.5));

        let rendered = toml::to_string(&parsed).unwrap();
        assert!(
            rendered.contains("max_daily_cost_usd = 12.5"),
            "round-trips through TOML: {rendered}"
        );
    }

    #[test]
    fn daily_cost_cap_stays_absent_unless_an_operator_sets_it() {
        assert_eq!(BudgetConfig::smart_default().max_daily_cost_usd, None);
        assert_eq!(
            BudgetConfig::default()
                .with_smart_defaults()
                .max_daily_cost_usd,
            None,
            "a cross-session ceiling must never appear by default"
        );
    }

    #[test]
    fn daily_cost_cap_rejects_non_finite_and_negative_values() {
        for usd in [f64::NAN, f64::INFINITY, -0.01] {
            let config = BudgetConfig {
                max_daily_cost_usd: Some(usd),
                ..Default::default()
            };
            assert!(config.validate().is_err(), "accepted {usd}");
        }
    }

    #[test]
    fn session_envelope_uses_the_stricter_daily_cap() {
        let budget = BudgetConfig {
            max_daily_cost_usd: Some(10.0),
            ..Default::default()
        };
        let session_cap = BudgetConfig {
            max_daily_cost_usd: Some(4.0),
            ..Default::default()
        };
        assert_eq!(
            BudgetConfig::effective_session_envelope(&budget, Some(&session_cap))
                .max_daily_cost_usd,
            Some(4.0)
        );
        assert_eq!(
            BudgetConfig::effective_session_envelope(&budget, None).max_daily_cost_usd,
            Some(10.0)
        );
    }

    #[test]
    fn session_envelope_preserves_disjoint_explicit_caps() {
        let budget = BudgetConfig {
            max_cost_usd: Some(1.0),
            max_wall_time_secs: Some(600),
            ..Default::default()
        };
        let session_cap = BudgetConfig {
            max_tokens_in: Some(1_000),
            max_processes: Some(2),
            ..Default::default()
        };

        let effective = BudgetConfig::effective_session_envelope(&budget, Some(&session_cap));

        assert_eq!(effective.max_cost_usd, Some(1.0));
        assert_eq!(effective.max_tokens_in, Some(1_000));
        assert_eq!(effective.max_tokens_out, Some(1_000_000));
        assert_eq!(effective.max_wall_time_secs, Some(600));
        assert_eq!(effective.max_processes, Some(2));
    }

    #[test]
    fn session_envelope_uses_stricter_duplicate_explicit_cap() {
        let budget = BudgetConfig {
            max_tokens_out: Some(500),
            max_cost_usd: Some(2.0),
            max_tool_runtime_secs: Some(300),
            max_agent_depth: Some(3),
            ..Default::default()
        };
        let session_cap = BudgetConfig {
            max_tokens_out: Some(200),
            max_cost_usd: Some(3.0),
            max_tool_runtime_secs: Some(120),
            max_agent_depth: Some(5),
            ..Default::default()
        };

        let effective = BudgetConfig::effective_session_envelope(&budget, Some(&session_cap));

        assert_eq!(effective.max_tokens_out, Some(200));
        assert_eq!(effective.max_cost_usd, Some(2.0));
        assert_eq!(effective.max_tool_runtime_secs, Some(120));
        assert_eq!(effective.max_agent_depth, Some(3));
    }

    // ── #174 item 6: named presets ──────────────────────────────────────

    /// The five names round-trip through TOML exactly as documented, and the
    /// serde spelling matches [`BudgetPreset::as_str`] — an error message can
    /// then never name a variant the parser would reject.
    #[test]
    fn preset_names_parse_and_match_as_str() {
        for (spelling, expected) in [
            ("tiny", BudgetPreset::Tiny),
            ("small", BudgetPreset::Small),
            ("normal", BudgetPreset::Normal),
            ("large", BudgetPreset::Large),
            ("no-hosted-spend", BudgetPreset::NoHostedSpend),
        ] {
            let parsed: BudgetConfig = toml::from_str(&format!("preset = \"{spelling}\"")).unwrap();
            assert_eq!(parsed.preset, Some(expected), "parsing {spelling:?}");
            assert_eq!(expected.as_str(), spelling);
        }
        assert!(
            toml::from_str::<BudgetConfig>("preset = \"enormous\"").is_err(),
            "an unknown preset must not parse"
        );
    }

    /// Rule 2 from the type docs, asserted rather than asserted-in-prose:
    /// every preset's output cap is a whole multiple of the 64,000 tokens the
    /// engine reserves per call. A preset that broke this would refuse its own
    /// first turn.
    #[test]
    fn every_preset_output_cap_admits_at_least_one_full_turn() {
        const PER_TURN_RESERVATION: u64 = 64_000;
        for preset in [
            BudgetPreset::Tiny,
            BudgetPreset::Small,
            BudgetPreset::Normal,
            BudgetPreset::Large,
            BudgetPreset::NoHostedSpend,
        ] {
            let out = preset.config().max_tokens_out.expect("an output cap");
            assert!(
                out >= PER_TURN_RESERVATION,
                "{} caps output at {out}, below the {PER_TURN_RESERVATION} the \
                 engine reserves for one call",
                preset.as_str()
            );
        }
        // The derived presets are exact multiples; `normal` is inherited from
        // `smart_default()` and is NOT (1,000,000 = 15.625 turns), which is
        // why the assertion above is a floor and not an equality.
        for preset in [BudgetPreset::Tiny, BudgetPreset::Small, BudgetPreset::Large] {
            assert_eq!(
                preset.config().max_tokens_out.unwrap() % PER_TURN_RESERVATION,
                0,
                "{} should be a whole number of turns",
                preset.as_str()
            );
        }
    }

    /// Rule 3: each derived preset's USD cap is the Sonnet-4 list price of its
    /// OWN token envelope, rounded up to the next dollar — so the token
    /// envelope binds first on the default model. A preset whose dollar cap
    /// drifted below its token envelope would stop on money for reasons the
    /// operator could not predict from the tokens they asked for.
    #[test]
    fn derived_preset_cost_caps_match_their_own_token_envelopes() {
        // $/Mtok, from wcore-pricing/pricing.toml `anthropic.claude-sonnet-4-6`.
        const IN_PER_MTOK: f64 = 3.0;
        const OUT_PER_MTOK: f64 = 15.0;
        for preset in [BudgetPreset::Tiny, BudgetPreset::Small, BudgetPreset::Large] {
            let cfg = preset.config();
            let listed = (cfg.max_tokens_in.unwrap() as f64 / 1_000_000.0) * IN_PER_MTOK
                + (cfg.max_tokens_out.unwrap() as f64 / 1_000_000.0) * OUT_PER_MTOK;
            let cap = cfg.max_cost_usd.unwrap();
            assert_eq!(
                cap,
                listed.ceil(),
                "{}: ${cap} is not the ceiling of its ${listed} envelope",
                preset.as_str()
            );
        }
    }

    /// The sizes are ordered on every axis they are meant to be ordered on.
    /// A preset list nobody can rank is a preset list nobody can choose from.
    #[test]
    fn size_presets_are_monotonic() {
        let sizes = [
            BudgetPreset::Tiny.config(),
            BudgetPreset::Small.config(),
            BudgetPreset::Normal.config(),
            BudgetPreset::Large.config(),
        ];
        for pair in sizes.windows(2) {
            let (smaller, bigger) = (&pair[0], &pair[1]);
            assert!(smaller.max_wall_time_secs < bigger.max_wall_time_secs);
            assert!(smaller.max_tool_runtime_secs < bigger.max_tool_runtime_secs);
            assert!(smaller.max_processes < bigger.max_processes);
            assert!(smaller.max_agent_depth < bigger.max_agent_depth);
            assert!(smaller.max_tokens_in < bigger.max_tokens_in);
            assert!(smaller.max_tokens_out < bigger.max_tokens_out);
            assert!(smaller.max_cost_usd < bigger.max_cost_usd);
        }
    }

    /// Rule 1: `normal` IS the shipped default. This is the guard that stops a
    /// later edit to `smart_default()` from silently changing what `normal`
    /// means, or vice versa.
    #[test]
    fn normal_preset_is_smart_default() {
        assert_eq!(
            BudgetPreset::Normal.config(),
            BudgetConfig {
                preset: Some(BudgetPreset::Normal),
                ..BudgetConfig::smart_default()
            }
        );
    }

    /// No preset carries a cross-session daily ceiling except the one whose
    /// entire purpose is a spend ceiling.
    #[test]
    fn only_no_hosted_spend_sets_a_daily_ceiling() {
        for preset in [
            BudgetPreset::Tiny,
            BudgetPreset::Small,
            BudgetPreset::Normal,
            BudgetPreset::Large,
        ] {
            assert_eq!(
                preset.config().max_daily_cost_usd,
                None,
                "{} must not bind other sessions",
                preset.as_str()
            );
        }
        assert_eq!(
            BudgetPreset::NoHostedSpend.config().max_daily_cost_usd,
            Some(0.0)
        );
    }

    /// A field strictly below the preset is kept; the untouched axes come from
    /// the preset.
    #[test]
    fn explicit_field_tightens_the_preset() {
        let cfg: BudgetConfig =
            toml::from_str("preset = \"small\"\nmax_agent_depth = 0\nmax_cost_usd = 1.0").unwrap();
        let resolved = cfg.resolve_preset().expect("tightening resolves");
        assert_eq!(resolved.max_agent_depth, Some(0));
        assert_eq!(resolved.max_cost_usd, Some(1.0));
        assert_eq!(resolved.max_tokens_in, Some(2_000_000));
    }

    /// An EQUAL value is a tightening of zero, not a widening. This is what
    /// makes expansion idempotent: resolving an already-resolved config (the
    /// `preset` tag survives) must not start failing.
    #[test]
    fn resolution_is_idempotent() {
        let once = BudgetConfig {
            preset: Some(BudgetPreset::Tiny),
            ..BudgetConfig::default()
        }
        .resolve_preset()
        .expect("first pass");
        let twice = once
            .resolve_preset()
            .expect("second pass must also resolve");
        assert_eq!(once, twice);
    }

    /// Widening is refused on EVERY axis, not only the money ones — including
    /// `max_daily_cost_usd`, whose preset value is `None` for the size presets
    /// and therefore has to be handled by the unbounded branch.
    #[test]
    fn every_axis_refuses_a_widening_field() {
        let tiny = BudgetPreset::Tiny.config();
        let widened = [
            ("max_wall_time_secs", "9999999"),
            ("max_tool_runtime_secs", "9999999"),
            ("max_concurrent_process_tools", "999"),
            ("max_agent_depth", "99"),
            ("max_tokens_in", "999999999"),
            ("max_tokens_out", "999999999"),
            ("max_cost_usd", "9999.0"),
        ];
        for (field, value) in widened {
            let cfg: BudgetConfig =
                toml::from_str(&format!("preset = \"tiny\"\n{field} = {value}")).unwrap();
            let error = cfg
                .resolve_preset()
                .expect_err("{field} widened tiny and was accepted");
            assert!(
                matches!(error, BudgetConfigError::PresetWidened { field: f, .. } if f == field),
                "wrong error for {field}: {error}"
            );
        }
        // CONTROL for the loop: an axis tiny leaves UNBOUNDED accepts the same
        // shape of value. Without this, a `resolve_preset` that refused
        // everything would pass the loop above for the wrong reason.
        assert!(tiny.max_daily_cost_usd.is_none());
        let cfg: BudgetConfig =
            toml::from_str("preset = \"tiny\"\nmax_daily_cost_usd = 9999.0").unwrap();
        assert_eq!(
            cfg.resolve_preset().unwrap().max_daily_cost_usd,
            Some(9999.0),
            "an unbounded preset axis accepts any explicit value"
        );
    }

    /// No preset means no expansion — the historical path is untouched.
    #[test]
    fn absent_preset_is_a_pass_through() {
        let cfg: BudgetConfig = toml::from_str("max_cost_usd = 2.5").unwrap();
        assert_eq!(cfg.resolve_preset().unwrap(), cfg);
    }
}
