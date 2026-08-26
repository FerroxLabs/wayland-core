//! FerroxLabs/wayland#174 item 6 — named `[budget]` presets, graded through
//! the REAL config loader.
//!
//! The point of grading here rather than on `BudgetPreset::config()` is that a
//! preset which the loader never consults is inert: a unit test on the helper
//! would pass with the wiring absent. Every test below starts from a
//! `config.toml` on disk, goes through `Config::resolve`, and then reproduces
//! the exact derivation `wcore_agent::bootstrap::SessionBudgetEnvelope` does —
//! `effective_session_envelope` → `BudgetCap` (the provider admission ledger)
//! and → `ExecutionBudget` (the operational tree) — so the assertions are
//! about the numbers the ENGINE is handed, not about an intermediate struct.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;

use serial_test::serial;
use wcore_budget::{BudgetCap, BudgetConfig, BudgetPreset, ExecutionBudget};
use wcore_config::config::{CliArgs, Config};

/// Save/restore the environment this test binary mutates. Mirrors
/// `config_resolution_provenance.rs::EnvGuard`.
struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn set(values: &[(&'static str, Option<&OsStr>)]) -> Self {
        let saved = values
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in values {
            // SAFETY: every test in this binary is `#[serial]`, so no other
            // thread observes the environment while it is being mutated.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            // SAFETY: see `EnvGuard::set`.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn cli(project_dir: &Path) -> CliArgs {
    CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("test-key-not-real".to_string()),
        project_dir: Some(project_dir.to_path_buf()),
        ..CliArgs::default()
    }
}

/// Write `budget_block` into a throwaway global `config.toml` and run the real
/// resolver over it.
fn resolve_with_budget(budget_block: &str) -> anyhow::Result<Config> {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!("[default]\nprovider = \"anthropic\"\n\n{budget_block}"),
    )
    .unwrap();
    let _env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        ("XDG_DATA_HOME", None),
        ("WAYLAND_CONFIG_PATH", None),
    ]);
    Config::resolve(&cli(project.path()))
}

/// Reproduce `SessionBudgetEnvelope::from_config_for_session`'s derivation:
/// this is what the engine is actually handed.
fn engine_view(config: &Config) -> (BudgetCap, ExecutionBudget) {
    let effective =
        BudgetConfig::effective_session_envelope(&config.budget, config.session_cap.as_ref());
    ((&effective).into(), ExecutionBudget::from(&effective))
}

/// THE WIRING ASSERTION. `[budget] preset = "tiny"` in a file on disk must
/// reach the engine as tiny numbers, on BOTH derivation paths.
#[test]
#[serial(config_provenance_env)]
fn tiny_preset_reaches_the_engine_as_tiny_limits() {
    let config = resolve_with_budget("[budget]\npreset = \"tiny\"\n").unwrap();
    assert_eq!(config.budget.preset, Some(BudgetPreset::Tiny));

    let (cap, exec) = engine_view(&config);

    // Provider admission ledger — the caps that stop a paid call.
    assert_eq!(cap.per_session_input_tokens, Some(200_000));
    assert_eq!(cap.per_session_output_tokens, Some(192_000));
    assert_eq!(cap.per_session_tokens, Some(392_000));
    assert_eq!(cap.per_session_usd, Some(4.00));
    assert_eq!(
        cap.per_user_daily_usd, None,
        "a size preset must not impose a cross-session daily ceiling"
    );

    // Operational tree — the caps that stop runaway tools / sub-agents.
    assert_eq!(exec.max_wall_time, Some(Duration::from_secs(300)));
    assert_eq!(exec.max_tool_runtime, Some(Duration::from_secs(60)));
    assert_eq!(exec.max_processes, Some(2));
    assert_eq!(exec.max_agent_depth, Some(0));

    // CONTROL: these really are the PRESET's numbers and not the Smart
    // defaults leaking through. Every one of them differs.
    let smart = BudgetConfig::smart_default();
    assert_ne!(config.budget.max_wall_time_secs, smart.max_wall_time_secs);
    assert_ne!(config.budget.max_tokens_in, smart.max_tokens_in);
    assert_ne!(config.budget.max_cost_usd, smart.max_cost_usd);
    assert_ne!(config.budget.max_agent_depth, smart.max_agent_depth);
}

/// The other end of the range, so the test cannot pass by accident on a
/// resolver that hardcodes one envelope.
#[test]
#[serial(config_provenance_env)]
fn large_preset_reaches_the_engine_as_large_limits() {
    let config = resolve_with_budget("[budget]\npreset = \"large\"\n").unwrap();
    let (cap, exec) = engine_view(&config);
    assert_eq!(cap.per_session_input_tokens, Some(100_000_000));
    assert_eq!(cap.per_session_output_tokens, Some(9_984_000));
    assert_eq!(cap.per_session_usd, Some(450.00));
    assert_eq!(exec.max_wall_time, Some(Duration::from_secs(86_400)));
    assert_eq!(exec.max_agent_depth, Some(12));
}

/// `normal` must be `smart_default()` exactly — naming the shipped default as
/// a preset must not move anybody's numbers.
#[test]
#[serial(config_provenance_env)]
fn normal_preset_is_the_shipped_default_unchanged() {
    let config = resolve_with_budget("[budget]\npreset = \"normal\"\n").unwrap();
    let smart = BudgetConfig::smart_default();
    assert_eq!(config.budget.max_wall_time_secs, smart.max_wall_time_secs);
    assert_eq!(
        config.budget.max_tool_runtime_secs,
        smart.max_tool_runtime_secs
    );
    assert_eq!(config.budget.max_processes, smart.max_processes);
    assert_eq!(config.budget.max_agent_depth, smart.max_agent_depth);
    assert_eq!(config.budget.max_tokens_in, smart.max_tokens_in);
    assert_eq!(config.budget.max_tokens_out, smart.max_tokens_out);
    assert_eq!(config.budget.max_cost_usd, smart.max_cost_usd);
    assert_eq!(config.budget.max_daily_cost_usd, smart.max_daily_cost_usd);
}

/// An explicit field that TIGHTENS the preset is honoured.
#[test]
#[serial(config_provenance_env)]
fn an_explicit_field_may_tighten_a_preset() {
    let config = resolve_with_budget("[budget]\npreset = \"tiny\"\nmax_cost_usd = 0.50\n").unwrap();
    let (cap, _) = engine_view(&config);
    assert_eq!(
        cap.per_session_usd,
        Some(0.50),
        "the stricter of the two numbers must be the one in force"
    );
    // The rest of the envelope still comes from the preset.
    assert_eq!(cap.per_session_input_tokens, Some(200_000));
}

/// An explicit field that WIDENS the preset is REFUSED — the semantics chosen
/// for #174, because silently honouring it (or silently clamping it) leaves
/// the operator believing a number that is not in force.
#[test]
#[serial(config_provenance_env)]
fn an_explicit_field_that_widens_a_preset_is_refused() {
    let error = resolve_with_budget("[budget]\npreset = \"tiny\"\nmax_cost_usd = 100.0\n")
        .expect_err("a widening field must not resolve");
    let text = error.to_string();
    assert!(text.contains("max_cost_usd"), "must name the field: {text}");
    assert!(text.contains("tiny"), "must name the preset: {text}");
    assert!(text.contains("WIDEN"), "must say what is wrong: {text}");
}

/// `no-hosted-spend` means it. The zero caps reach the engine's provider
/// admission ledger, where the pre-flight reservation refuses any call that
/// would cost more than $0.
#[test]
#[serial(config_provenance_env)]
fn no_hosted_spend_reaches_the_engine_as_a_hard_zero() {
    let config = resolve_with_budget("[budget]\npreset = \"no-hosted-spend\"\n").unwrap();
    let (cap, _) = engine_view(&config);
    assert_eq!(cap.per_session_usd, Some(0.0));
    assert_eq!(cap.per_user_daily_usd, Some(0.0));

    // ENFORCEMENT, not advertisement: a tracker holding these caps refuses a
    // priced reservation and admits a genuinely free one. This is the same
    // `reserve_turn` the engine calls before it dispatches a provider request,
    // wired the way `AgentBootstrap` wires it — a zero DAILY ceiling needs the
    // durable ledger behind it or the tracker fails closed on every call,
    // which would make the "free local call still runs" half untestable and,
    // more importantly, untrue.
    let ledger = tempfile::tempdir().unwrap();
    let daily = || {
        wcore_budget::DailyAuthority::new(
            std::sync::Arc::new(wcore_budget::DailySpendStore::at(
                ledger.path().join("daily-spend.json"),
            )),
            "budget-preset-test-subject",
        )
    };
    let mut tracker = wcore_budget::BudgetTracker::new(cap.clone());
    tracker.set_daily_authority(daily());
    assert!(
        tracker.reserve_turn("s", 1_000, 1_000, 0.000_1).is_err(),
        "a call that costs anything at all must be refused"
    );
    let mut tracker = wcore_budget::BudgetTracker::new(cap);
    tracker.set_daily_authority(daily());
    assert!(
        tracker.reserve_turn("s", 1_000, 1_000, 0.0).is_ok(),
        "a genuinely free (local) call must still run — the preset forbids \
         spend, not work"
    );

    // The operational envelope is untouched: `normal`'s.
    let smart = BudgetConfig::smart_default();
    assert_eq!(config.budget.max_wall_time_secs, smart.max_wall_time_secs);
    assert_eq!(config.budget.max_processes, smart.max_processes);
}

/// The guarantee cannot be edited away one field at a time.
#[test]
#[serial(config_provenance_env)]
fn no_hosted_spend_cannot_be_widened_by_an_explicit_cost() {
    let error = resolve_with_budget("[budget]\npreset = \"no-hosted-spend\"\nmax_cost_usd = 5.0\n")
        .expect_err("a positive cost cap must not survive this preset");
    assert!(error.to_string().contains("no-hosted-spend"));

    let error =
        resolve_with_budget("[budget]\npreset = \"no-hosted-spend\"\nmax_daily_cost_usd = 5.0\n")
            .expect_err("a positive daily ceiling must not survive this preset");
    assert!(error.to_string().contains("max_daily_cost_usd"));
}

/// A misspelled preset is refused at parse time, naming the accepted set,
/// rather than resolving to something arbitrary.
#[test]
#[serial(config_provenance_env)]
fn an_unknown_preset_name_is_refused() {
    let error = resolve_with_budget("[budget]\npreset = \"enormous\"\n")
        .expect_err("an unknown preset must not resolve");
    let text = error.to_string();
    assert!(
        text.contains("no-hosted-spend") || text.contains("unknown variant"),
        "the refusal should name the accepted set: {text}"
    );
}

/// CONTROL for every test above: with no `[budget]` block at all, resolution
/// still produces the historical unbounded config and the Smart-default
/// envelope. If presets had changed the no-preset path, this fails.
#[test]
#[serial(config_provenance_env)]
fn absent_budget_block_is_unchanged_by_presets() {
    let config = resolve_with_budget("").unwrap();
    assert_eq!(config.budget, BudgetConfig::default());
    assert_eq!(config.budget.preset, None);
    let (cap, exec) = engine_view(&config);
    let smart = BudgetConfig::smart_default();
    assert_eq!(cap.per_session_usd, smart.max_cost_usd);
    assert_eq!(
        exec.max_wall_time,
        smart.max_wall_time_secs.map(Duration::from_secs)
    );
}
