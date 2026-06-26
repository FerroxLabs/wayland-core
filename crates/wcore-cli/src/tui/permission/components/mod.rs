//! Permission components — one per tool (SPEC §2). W2 ships only the
//! `fallback` keystone; W3/W4 add their own `pub mod <name>;` line here via
//! the wire-up task (e.g. `pub mod bash;`, feature-gated ones as
//! `#[cfg(feature = "workflow")] pub mod workflow;`).
//!
//! `shell_common` is the W3 SHARED scaffold (not a routed component, so no
//! dispatch arm) — its `pub mod` line lands here with the S-W3a scaffold
//! task so the parallel Bash/PowerShell component agents can
//! `use super::shell_common` against an already-declared module.
pub mod fallback;
pub mod shell_common;

// W3 components.
pub mod bash;
pub mod fileedit;
pub mod filesystem;
pub mod filewrite;
pub mod powershell;

// W4 components.
pub mod ask_user;
// Crucible Stage 4a — the cross-vendor council proposal card.
pub mod crucible;
pub mod enter_plan;
pub mod exit_plan;
pub mod notebook;
pub mod skill;
pub mod webfetch;

// W4 feature-gated components.
#[cfg(feature = "monitor")]
pub mod monitor;
#[cfg(feature = "review_artifact")]
pub mod review_artifact;
#[cfg(feature = "workflow")]
pub mod workflow;
