//! D3 regression proofs: `credentials.backend = "plaintext"` must fail at
//! session start, naming its own cause.
//!
//! Live Windows UAT (20A-LIVE-WINDOWS-UAT.md, defect D3): with
//! `backend = "plaintext"` every turn died with
//!
//! ```text
//! error: Session persistence authority unavailable: secure recovery storage is
//!        unavailable; configure an OS keyring or encrypted credentials vault
//! ```
//!
//! which tells a user to configure a backend they *have* configured. Refusing
//! plaintext for confidential material is correct and is not changed here; what
//! is fixed is that the refusal is deferred to turn 1 and never names itself.

mod common;

use std::sync::Arc;

use tempfile::tempdir;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_config::credentials::{CredentialsBackend, CredentialsStorageConfig};
use wcore_tools::registry::ToolRegistry;

use common::{MockLlmProvider, configure_persisted_test_session, test_config};

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

fn engine_with_backend(backend: CredentialsBackend, session_root: &std::path::Path) -> AgentEngine {
    let mut config = test_config();
    configure_persisted_test_session(&mut config, session_root);
    config.storage.credentials = CredentialsStorageConfig {
        backend,
        service_name: None,
    };
    AgentEngine::new_with_provider(
        Arc::new(MockLlmProvider::with_text_response("unused")),
        config,
        ToolRegistry::new(),
        silent_output(),
    )
}

/// The plaintext backend is statically incompatible with durable journaling.
/// That is decidable from config alone, so it must be reported when the
/// persisted session is opened — not on the user's first prompt.
#[test]
fn plaintext_backend_is_rejected_when_the_persisted_session_opens() {
    let dir = tempdir().expect("d3 tempdir");
    let mut engine = engine_with_backend(CredentialsBackend::Plaintext, dir.path());

    let error = engine
        .init_session("test-provider", &dir.path().to_string_lossy(), None)
        .expect_err("a plaintext credentials backend cannot open a journaled session")
        .to_string();

    assert!(
        error.contains("plaintext"),
        "the error must name the backend that is actually the cause: {error}"
    );
    assert!(
        error.contains("credentials.backend"),
        "the error must name the setting the user has to change: {error}"
    );
}

/// Control: the default backend still opens a journaled session. Without this
/// the test above would pass on any breakage of `init_session`.
#[test]
fn auto_backend_still_opens_a_persisted_session() {
    let dir = tempdir().expect("d3 control tempdir");
    let mut engine = engine_with_backend(CredentialsBackend::Auto, dir.path());

    engine
        .init_session("test-provider", &dir.path().to_string_lossy(), None)
        .expect("the default backend must keep working");
}

/// Control: a keyring backend is confidential-capable, so the static check must
/// not reject it either. Whether the OS keyring is actually reachable is a
/// runtime question answered later, not at session open.
#[test]
fn keyring_backend_is_not_rejected_statically() {
    let dir = tempdir().expect("d3 keyring tempdir");
    let mut engine = engine_with_backend(CredentialsBackend::Keyring, dir.path());

    engine
        .init_session("test-provider", &dir.path().to_string_lossy(), None)
        .expect("a keyring backend is confidential-capable and must open");
}
