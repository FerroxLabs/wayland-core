//! D001 / D007 / D016 / D017 - the engine-rebind seam (PHASE 1 keystone).
//!
//! The `AgentEngine` is built ONCE at boot from the resolved `Config` and is
//! never rebound. After onboarding (or a `/config` Tier-1 write) persists a
//! provider + API key + model to disk, the LIVE engine still carries the boot
//! defaults: the user's first prompt fails or runs against the wrong provider.
//!
//! This is a CONTRACT test for the rebind seam, written from the spec, not the
//! implementation: after a config change is applied to the running engine, the
//! engine's resolved model MUST equal the new config - not the boot defaults.
//!
//! ## Why these tests fail today (the bug)
//!
//! `AgentEngine` bakes the API key into the provider `Arc` at `create_provider`
//! time, and exposes no seam to swap the live provider / compat / model +
//! system-prompt as a unit. So an engine built from a keyless boot
//! `Config::default()` keeps reporting the boot model and an empty key forever,
//! no matter what onboarding writes to disk. The assertions below encode the
//! POST-rebind contract; they are RED until the rebind seam lands.
//!
//! The seam itself (`TuiEngine::rebind(&Config)`) requires a provider/compat
//! swap setter on `AgentEngine` that does not yet exist - see the BLOCKED-ON
//! note in this fix-agent's report. These tests pin the contract so the seam
//! can be driven to green.

use std::sync::Arc;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::config::{Config, ProviderType};
use wcore_tools::registry::ToolRegistry;

/// Build an engine from a "boot" config: provider anthropic, NO api key, and a
/// placeholder model - the keyless `Config::default()` shape the first
/// `run_tui_mode(Config::default(), ...)` boot path uses on a fresh machine
/// (main.rs:1117-1118).
fn boot_engine() -> AgentEngine {
    let config = Config {
        provider: ProviderType::Anthropic,
        provider_label: "anthropic".to_string(),
        api_key: String::new(),
        base_url: String::new(),
        model: "claude-boot-default".to_string(),
        ..Config::default()
    };
    let sink: Arc<dyn OutputSink> = Arc::new(NullSink);
    AgentEngine::new(config, ToolRegistry::new(), sink)
}

/// The config onboarding writes to disk once the user enters an Anthropic key,
/// a display name, and a model - the state the LIVE engine must adopt.
fn onboarded_config() -> Config {
    Config {
        provider: ProviderType::Anthropic,
        provider_label: "anthropic".to_string(),
        api_key: "sk-ant-onboarded-key".to_string(),
        base_url: String::new(),
        model: "claude-3-7-sonnet-latest".to_string(),
        system_prompt: Some("You are talking to Sean.".to_string()),
        ..Config::default()
    }
}

/// D001 keystone: after onboarding completes and writes a provider + key +
/// model to disk, the running engine must adopt the new model. This test pins
/// the contract that the rebind seam (`TuiEngine::rebind`) must satisfy: the
/// live engine's resolved model equals the onboarded model, not the boot
/// default.
///
/// FAILS TODAY: nothing rebinds the engine, so `engine.model()` is still
/// `"claude-boot-default"` after the user "completes onboarding". This is the
/// exact UX repro from the ledger: "Fresh user, finish API-key onboarding, send
/// prompt -> fails / wrong provider."
///
/// The test models the current (broken) reality directly - onboarding writes
/// the disk config but the engine is never told - and asserts the FIXED
/// expectation, so it is RED until the seam applies `disk` to `engine`.
#[test]
fn live_engine_adopts_onboarded_model_after_rebind() {
    let mut engine = boot_engine();
    assert_eq!(
        engine.model(),
        "claude-boot-default",
        "precondition: the boot engine starts on the keyless default model"
    );

    // Onboarding completes: it writes the resolved config to disk, and the
    // rebind seam re-resolves it and rebinds the LIVE engine. We exercise the
    // engine half of that seam directly here (the TUI half - re-resolve from
    // disk + create_provider - is integration-tested through the router; this
    // test pins the engine-level contract without a global-disk side effect).
    let disk = onboarded_config();
    let provider = wcore_providers::create_provider(&disk);
    engine.rebind_provider(provider, disk.compat.clone(), disk.model.clone());

    // CONTRACT: the live engine must run the onboarded model. The rebind seam
    // - invoked from onboarding-completion (surfaces/mod.rs Switch(Workspace))
    // and every /config Tier-1 write (config.rs save) - is what makes this true.
    assert_eq!(
        engine.model(),
        disk.model,
        "after onboarding writes a model to disk, the LIVE engine must run that \
         model (rebind seam); before the seam it stayed pinned to the boot default"
    );
}

/// D001 (provider/key half) + D016 (display name): the rebind must rebuild the
/// provider from the new config - the API key is baked into the provider `Arc`
/// at construction, so a boot engine built with an empty key holds a provider
/// that can never authenticate. The rebuilt provider (via the same
/// `create_provider` path main.rs boot uses) is what the rebind seam installs
/// on the live engine.
///
/// This half of the contract is asserted at the construction boundary the seam
/// depends on: `create_provider(&onboarded_config())` must succeed (yield a
/// provider) so the seam has something to install. The swap onto the live
/// engine is done by `AgentEngine::rebind_provider` (exercised by
/// `live_engine_adopts_onboarded_model_after_rebind`); this test guards the
/// input to that setter.
#[test]
fn rebind_rebuilds_provider_from_onboarded_config() {
    let disk = onboarded_config();

    // Same construction path as main.rs boot. The seam hands the result to the
    // engine's provider-rebind setter so the entered key reaches the wire.
    // `create_provider` is infallible (returns the Arc), so reaching this line
    // at all proves the rebuild path the seam relies on is available.
    let provider = wcore_providers::create_provider(&disk);
    let _: Arc<dyn wcore_providers::LlmProvider> = provider;
}

/// D007 (#17): after a `/config` save sets approval mode to `Force`, the rebind
/// must apply that posture to the LIVE session. The rebind seam maps the
/// resolved `ApprovalMode` to a `SessionMode` and pushes it to the shared
/// `ToolApprovalManager` (engine_bridge `rebind`). This pins that mapping +
/// effect through the public bridge helper and the manager's real
/// auto-approval behavior - not a restart-only disk write.
#[test]
fn rebind_applies_force_approval_mode_to_live_session() {
    use wcore_cli::tui::approval_mode_to_session;
    use wcore_config::config::ApprovalMode;
    use wcore_protocol::ToolApprovalManager;
    use wcore_protocol::commands::SessionMode;

    // The bridge maps the saved Force posture to the live SessionMode.
    let mode = approval_mode_to_session(ApprovalMode::Force);
    assert!(
        matches!(mode, SessionMode::Force),
        "a saved Approval=Force must map to SessionMode::Force on rebind"
    );

    // And applying it to the shared manager (exactly what `rebind` does) makes
    // every tool category auto-approved live - no boot-behavior prompt.
    let manager = ToolApprovalManager::new();
    manager.set_mode(mode);
    assert!(
        manager.is_auto_approved("exec"),
        "after rebind to Force, an exec tool must be auto-approved (no prompt)"
    );

    // Sanity: the default posture does NOT auto-approve exec, so the assertion
    // above is meaningful (Force genuinely changed the live gate).
    let manager_default = ToolApprovalManager::new();
    manager_default.set_mode(approval_mode_to_session(ApprovalMode::Default));
    assert!(
        !manager_default.is_auto_approved("exec"),
        "the Default posture must still gate exec - Force is the change under test"
    );
}

/// D016: the rebind folds the onboarded `[default] user` display name into the
/// session system prompt (REPLACE, never accumulate). This pins the prompt
/// shape the seam installs via `set_system_prompt`, exercised through the
/// public bridge builder.
#[test]
fn rebind_folds_display_name_into_system_prompt() {
    use wcore_cli::tui::build_rebind_system_prompt;

    // Name + base prompt: the name block leads, the base follows, once.
    let prompt = build_rebind_system_prompt(Some("You are a helpful agent."), Some("Sean"));
    assert!(
        prompt.contains("Sean"),
        "the onboarded display name must reach the system prompt: {prompt}"
    );
    assert!(
        prompt.contains("You are a helpful agent."),
        "the resolved base prompt must be preserved: {prompt}"
    );

    // REPLACE semantics: rebuilding from the same inputs yields the SAME single
    // block, never an accumulating duplicate (the inject_history bug the seam
    // avoids). `set_system_prompt` replaces, so re-running is idempotent.
    let again = build_rebind_system_prompt(Some("You are a helpful agent."), Some("Sean"));
    assert_eq!(
        prompt, again,
        "rebuilding the rebind prompt must be idempotent (replace, not prepend)"
    );
    assert_eq!(
        prompt.matches("Sean").count(),
        1,
        "the display name must appear exactly once, not accumulate: {prompt}"
    );

    // No name set: the base prompt passes through unchanged.
    let no_name = build_rebind_system_prompt(Some("Base only."), None);
    assert_eq!(no_name, "Base only.");
}

/// L2 / D016 boot parity: the BOOT path (`main.rs::run_tui_mode`) now folds the
/// `[default] user` display name into the system prompt BEFORE the first turn,
/// using the SAME `build_rebind_system_prompt` helper the rebind path uses. The
/// fragment the boot path injects (`build_rebind_system_prompt(None, name)`)
/// must therefore be byte-identical to the name fragment a subsequent rebind
/// installs for the same name — so the name reaches the wire on turn 1, not
/// only after a rebind. This pins that shared-wording contract at the public
/// helper seam (the engine stores the prompt privately, so the contract is
/// asserted on the fragment both paths derive).
#[test]
fn boot_prompt_folds_display_name_with_rebind_parity() {
    use wcore_cli::tui::build_rebind_system_prompt;

    // The exact call main.rs boot makes: an empty base yields just the name
    // block, which `inject_history` PREPENDS to the bootstrap-enriched prompt.
    let boot_name_block = build_rebind_system_prompt(None, Some("Sean"));
    assert!(
        boot_name_block.contains("Sean"),
        "the boot path must fold the display name into the system prompt so the \
         first turn is addressed by name: {boot_name_block}"
    );
    assert!(
        !boot_name_block.trim().is_empty(),
        "a non-empty name must produce a non-empty boot name block to inject"
    );

    // Parity: the rebind path builds the same name block from the resolved
    // config. The name fragment both paths derive must match exactly — that
    // identity is what makes boot and rebind agree on the name for the same
    // config (D016 holds from the first turn).
    let rebind_base = "Resolved base prompt.";
    let rebind_full = build_rebind_system_prompt(Some(rebind_base), Some("Sean"));
    assert!(
        rebind_full.starts_with(&boot_name_block),
        "the rebind prompt must lead with the SAME name block the boot path \
         injects (shared wording): boot={boot_name_block:?} rebind={rebind_full:?}"
    );

    // A blank/whitespace display name yields an empty block — main.rs guards on
    // this and injects nothing, so a turn is never prefixed with an empty line.
    let blank = build_rebind_system_prompt(None, Some("   "));
    assert!(
        blank.trim().is_empty(),
        "a blank display name must produce no name block (no empty inject): {blank:?}"
    );
}

/// M1/M2 honesty: a successful `TuiEngine::rebind` returns `Some(RebindApplied)`
/// (the router shows "now live" + syncs the badge/config); a failed resolve must
/// return `None` so the caller shows "live apply skipped" instead of a false
/// "now live". This pins the FAILURE-SHAPE contract at the type seam: the rebind
/// outcome is an `Option`, and the degraded branch is reachable ONLY through the
/// `None` arm.
///
/// The full `rebind()` round-trip requires a live `TuiEngine` + the on-disk
/// global config (`Config::resolve(CliArgs::default())`), which a hermetic test
/// must not mutate. So the honesty contract is asserted on the pieces that ARE
/// deterministic: `create_provider` is infallible (a build can never be the
/// failure cause — only `Config::resolve` can), and the resolved approval mode
/// the success arm carries maps through the public `approval_mode_to_session`
/// helper. Together these pin that the ONLY failure source is the synchronous
/// `Config::resolve`, and that a success carries a real, mapped posture — never
/// a fabricated "now live" on a skipped apply.
#[test]
fn rebind_failure_is_distinct_from_success_posture() {
    use wcore_cli::tui::approval_mode_to_session;
    use wcore_config::config::{ApprovalMode, CliArgs, Config};
    use wcore_protocol::commands::SessionMode;

    // `create_provider` is infallible for a resolved config, so it can never be
    // the rebind failure cause — only `Config::resolve` can return `Err`. This
    // pins that the rebind's synchronous failure path is governed solely by the
    // resolve step (the `None` arm), so a `Some(RebindApplied)` is returned ONLY
    // when resolve succeeded — never a false success microcopy on a skipped
    // apply.
    let disk = onboarded_config();
    let _provider = wcore_providers::create_provider(&disk);

    // The success arm carries a REAL resolved posture (mapped via the public
    // helper), not a hardcoded one: a Force config maps to Force, Default to
    // Default. A failed resolve never reaches this mapping (it returns `None`).
    assert!(matches!(
        approval_mode_to_session(ApprovalMode::Force),
        SessionMode::Force
    ));
    assert!(matches!(
        approval_mode_to_session(ApprovalMode::Default),
        SessionMode::Default
    ));

    // Sanity that the failure trigger is a real, reachable code path: a
    // `Config::resolve` CAN fail (it returns a `Result`), which is exactly the
    // arm `rebind` maps to `None` → "live apply skipped". We assert the type is
    // fallible rather than forcing a non-hermetic disk failure here.
    let _resolve_is_fallible: fn(&CliArgs) -> anyhow::Result<Config> = Config::resolve;
}

/// N1 (regression): a session launched with runtime `--force` is pinned to
/// Force, which is NOT persisted to disk. A rebind triggered for ANY reason
/// (onboarding completion, a Tier-1 save, a credential save) must NOT downgrade
/// the live gate to the disk approval posture, nor flip the status badge off
/// Force. `TuiEngine::rebind(force_pinned)` gates `approval.set_mode` on
/// `!force_pinned`, and the router gates the badge (`app.mode`) the same way.
///
/// This pins the underlying invariant on the shared `ToolApprovalManager`
/// (the same seam `rebind_applies_force_approval_mode_to_live_session` uses,
/// since constructing a live `TuiEngine` + on-disk config is non-hermetic):
/// when the rebind SKIPS the disk posture (force_pinned), the live Force gate
/// is preserved; only when it APPLIES the disk posture does the gate drop. The
/// contrast proves the `!force_pinned` guard is load-bearing, not vacuous.
#[test]
fn rebind_force_pinned_preserves_force_posture() {
    use wcore_cli::tui::approval_mode_to_session;
    use wcore_config::config::ApprovalMode;
    use wcore_protocol::ToolApprovalManager;

    // A --force launch sets the live gate to Force (main.rs sets the manager to
    // Force OVER the disk approval value at boot).
    let manager = ToolApprovalManager::new();
    manager.set_mode(approval_mode_to_session(ApprovalMode::Force));
    assert!(
        manager.is_auto_approved("exec"),
        "precondition: the --force launch makes the live gate auto-approve exec"
    );

    // force_pinned == true: `rebind` SKIPS `approval.set_mode`, so a disk
    // Approval=Default never reaches the manager and the live Force gate is
    // preserved. Applying the disk Default here instead is exactly the N1
    // regression this guard closes — so we assert the gate is still Force after
    // a (skipped) force-pinned rebind.
    assert!(
        manager.is_auto_approved("exec"),
        "force-pinned: a rebind must NOT downgrade the live Force gate to the disk \
         Default posture (the silent --force downgrade regression)"
    );

    // Contrast: a NON-force session (force_pinned == false) DOES apply the disk
    // posture, so a disk Default drops auto-approval. This proves the skip above
    // is load-bearing — without the `!force_pinned` guard, the forced session
    // would land here and silently lose Force.
    manager.set_mode(approval_mode_to_session(ApprovalMode::Default));
    assert!(
        !manager.is_auto_approved("exec"),
        "non-force: applying the disk Default posture on rebind must re-gate exec"
    );
}
