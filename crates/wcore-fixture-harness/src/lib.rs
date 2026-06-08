//! Customer-fixture catalog. See `.blackboard/E2E-TESTING-STRATEGY-2026-05-24.md` §3 T6.
//!
//! # Design
//!
//! Each subdirectory under `fixtures/` is a "archetype" — a sanitized snapshot
//! of a real user's `$WAYLAND_HOME` that captures a specific environmental
//! shape.  The engine binary is spawned against the fixture directory; the test
//! asserts on json-stream events, stderr cleanliness, and post-run state diff.
//!
//! # Wave status
//!
//! - **Wave 1 (this commit)**: skeleton + fixture data committed.
//! - **Wave 2**: `FixtureCatalog`, `FixturePlayback`, `replay()` implemented.
//!   `sealed_env()` uses allowlist-only env model per §3.5.
//! - **Wave 3**: `wcore fixtures verify` CLI + per-fixture anti-leak sweep.
//!
//! # Sanitization
//!
//! All fixture files are sanitized per `.blackboard/E2E-FIXTURE-SANITIZATION-2026-05-24.md`.
//! No fixture file may contain a real API key, personal email, or machine path.
//! The CI anti-leak grep enforces this on every PR.
