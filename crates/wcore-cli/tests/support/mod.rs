//! Shared test support for the wcore-cli E2E harness.
//!
//! Included from integration test files via `#[path = "support/mod.rs"] mod support;`.

pub mod mock_llm;
pub mod pty;

#[cfg(unix)]
pub mod proving_ground;
