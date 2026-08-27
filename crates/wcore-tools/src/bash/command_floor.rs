//! #693 — the command floor.
//!
//! The implementation moved to [`wcore_config::command_floor`]. It had to
//! sit below `wcore-tools`, because `BashTool` is only ONE of the two
//! shell surfaces this product has: `wcore_skills::shell::execute_shell_commands`
//! runs a skill's `` !`…` `` directive under `sh -c` on a path that never
//! touches this crate, and `wcore-skills` does not depend on `wcore-tools`.
//!
//! Leaving the floor here was a complete two-step bypass of it — see the
//! module note on the new location, and
//! `wcore-skills/tests/skill_shell_command_floor.rs` for the red arm.
//!
//! Re-exported rather than moved wholesale so that every existing caller
//! and every existing test keeps its import path.

pub use wcore_config::command_floor::check_command_floor;
