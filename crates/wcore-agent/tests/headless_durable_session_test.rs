//! Durable sessions must be all-or-nothing: an engine with `[session] enabled
//! = false` must not hold a journal.
//!
//! Live UAT found that every inbound channel turn on a headless host died with
//!
//! ```text
//! Session persistence authority unavailable: secure recovery storage is
//! unavailable: no OS keyring was usable and no encrypted credentials vault is
//! unlocked
//! ```
//!
//! and that `[session] enabled = false` fixed it. It only fixed it for a *new*
//! conversation. `run_with_content` gates the per-turn confidential preflight on
//! `session_journal`, not on the session manager, and the resume constructor
//! accepted a journal handed in from outside regardless of the setting — so a
//! conversation that already existed on disk kept its journal and kept failing.
//!
//! `channel_dispatch` reaches exactly that: it builds its own `SessionManager`,
//! calls `load_for_run_if_exists`, and passes the resulting `ActiveSession` to
//! `AgentBootstrap::resume`. Every restarted channel conversation arrives on the
//! resume path.
//!
//! These assertions are structural rather than behavioural on purpose. Holding a
//! journal only changes an outcome where confidential storage is unavailable, so
//! a behavioural test would pass vacuously on any developer machine with a
//! working OS keyring — the exact host where this defect is invisible.

mod common;

use std::sync::Arc;

use tempfile::tempdir;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_agent::session::SessionManager;
use wcore_config::config::Config;
use wcore_tools::registry::ToolRegistry;

use common::{MockLlmProvider, configure_persisted_test_session, test_config};

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// Build an engine through the production resume constructor with a real,
/// live journal, exactly as `AgentBootstrap::resume` does.
fn resume_engine_with_a_live_journal(
    sessions_enabled: bool,
    root: &std::path::Path,
) -> AgentEngine {
    let mut config: Config = test_config();
    configure_persisted_test_session(&mut config, root);
    config.session.enabled = sessions_enabled;

    // The journal is created independently of the engine's own config, which
    // is precisely how `channel_dispatch` does it.
    let manager = SessionManager::new(root.join("sessions"), 10);
    let active = manager
        .create_for_run("test-provider", "test-model", &root.to_string_lossy(), None)
        .expect("create a durable session to resume from");

    AgentEngine::resume_active_with_provider(
        Arc::new(MockLlmProvider::with_text_response("unused")),
        config,
        ToolRegistry::new(),
        silent_output(),
        active,
    )
}

#[test]
fn a_resumed_engine_drops_its_journal_when_durable_sessions_are_off() {
    let dir = tempdir().expect("tempdir");
    let engine = resume_engine_with_a_live_journal(false, dir.path());

    assert!(
        !engine.has_durable_journal(),
        "durable sessions are off, so this engine must not hold a journal writer \
         lease — holding one puts every turn back through the confidential \
         preflight that `[session] enabled = false` exists to avoid"
    );
}

/// Control. Without this the assertion above would also pass if the resume
/// constructor stopped accepting journals altogether, which would silently
/// disable crash recovery for every correctly configured install.
#[test]
fn a_resumed_engine_keeps_its_journal_when_durable_sessions_are_on() {
    let dir = tempdir().expect("tempdir");
    let engine = resume_engine_with_a_live_journal(true, dir.path());

    assert!(
        engine.has_durable_journal(),
        "durable sessions are on, so the resumed journal must be retained"
    );
}
