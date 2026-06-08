//! `wcore-cron` — memory-resident scheduled-trigger crate (v0.8.1 U7).
//!
//! Cron-expression parsing (via the `cron` crate) + three target types
//! (slash command, channel message, skill invocation) + a 30s tick
//! background runner. Persists jobs via [`CronStore`], with a JSON-file
//! implementation that lives under `~/.wayland/cron/jobs.json` by
//! default.
//!
//! ## Persistence choice
//!
//! The `wcore-memory` procedural partition is keyed by skill name and
//! tracks Beta scoring stats — it's not a fit for arbitrary job rows.
//! Per the spec's explicit fallback clause, this crate ships with a
//! JSON-file backed `FileCronStore` written atomically. A future move
//! to the memory crate's procedural partition is a swap-in trait impl.
//!
//! ## Production wire-up
//!
//! `crates/wcore-agent/src/bootstrap.rs` spawns a [`CronRunner`] with an
//! `EngineJobHandler` after the engine is built. Drop on `AgentEngine`
//! cancels the runner via its shutdown watch channel.

pub mod job;
pub mod runner;
pub mod schedule;
pub mod store;

pub use job::{CronFireOutcome, CronFireRecord, CronJob, Target};
pub use runner::{CronRunner, JobHandler, tick_once, tick_once_with_history};
pub use schedule::{next_fire_after, parse_expression};
pub use store::{CronStore, FileCronStore, default_history_path, default_store_path};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CronError {
    #[error("invalid cron expression: {0}")]
    InvalidExpression(String),

    #[error("job not found: {0}")]
    NotFound(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("dispatch error: {0}")]
    Dispatch(String),

    #[error("store error: {0}")]
    Store(String),
}

pub type Result<T> = std::result::Result<T, CronError>;
