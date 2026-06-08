//! Shared widget library — every widget is a free function.
//!
//! FROZEN Wave-0 contract: the function signatures re-exported here are
//! the integration boundary the surfaces paint with.
//!
//! Convention: every drawing widget takes `(frame, area, &model, &theme)`
//! and renders into `area`; it never returns or mutates state.

// The widget free fns are re-exported here as the flat `widgets::` API
// surface, but no caller exists until the Wave-1 surface agents wire them
// up — so the `pub use` lines below read as unused re-exports. The allow
// is scoped to this module and is the deliberate cost of publishing the
// frozen widget contract ahead of its consumers; it stops mattering the
// moment a surface calls `widgets::status_bar` etc. (`unused_imports` is
// a distinct lint from the `dead_code` allow in `tui/mod.rs`, so the
// re-exports need their own allow until Wave 1 wires them up.)
#![allow(unused_imports)]

mod approval_inline;
mod banner;
mod diff;
mod header;
mod panel;
mod sources_block;
mod spinner;
mod statusbar;
mod streaming_status;
mod toolcard;
mod tree;

pub use approval_inline::render_approval_inline;
pub use banner::wayland_banner;
pub use diff::{diff_lines, diff_view};
pub use header::{SystemSample, SystemSampler, top_chrome};
pub use panel::panel;
pub use sources_block::render_sources;
pub use spinner::spinner_frame;
pub use statusbar::status_bar;
// v0.9.2 audit M2: the run loop clears an expired toast through
// `App::dismiss_expired_toast` using the SAME dwell the status bar draws
// against, so the contract is single-sourced.
pub(crate) use statusbar::TOAST_DWELL;
pub use streaming_status::{format_duration, render_streaming_status};
pub use toolcard::tool_card;
pub use tree::path_tree;
