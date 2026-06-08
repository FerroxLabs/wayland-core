//! State store — transient/toast slice + redraw-skip (SPEC §1B, Wave 10).
//!
//! v0.9.2 W10. The custom subscriber [`Store`] (CC `src/state/store.ts`
//! reimplemented) plus the conservative `TransientSlice` that wraps ONLY the
//! cost / mcp-status / context / toast slices — the transient status-bar
//! surfaces. The §10 risk-2 mandate: do NOT rewrite `App` / `SessionView`
//! wholesale; this proves the store pattern and gives the toast-demotion
//! (W5) and the redraw-skip optimization a clean, comparable home. The bulk
//! migration is a v0.9.4 follow-up.
//!
//! The four `App` fields (`cost`, `mcp_status`, `context`, `toast` +
//! `toast_at`) remain the canonical render-read surface so the ~40 existing
//! `app.rs` / `protocol_bridge.rs` tests and every reader keep working
//! untouched. The protocol bridge routes its writes through
//! [`App::set_transient`] (a `Store::set`), so a no-op write skips listeners
//! and the loop's redraw-skip consults the store's `select` snapshot.

mod store;

pub use store::Store;

use std::collections::HashMap;
use std::time::Instant;

use crate::tui::app::{ContextView, McpServerStatus, SessionCostView};

/// The transient status-bar slice the redraw-skip + toast-demotion ride on.
///
/// A faithful clone of the four `App` transient fields, wrapped in a
/// [`Store`] so an identical-value write is a no-op (the `Object.is` guard)
/// and the render loop can `select` a comparable snapshot to decide whether
/// the transient row needs a repaint. `PartialEq` + `Clone` are required by
/// `Store<T>`; every member already supports both (see the additive derives
/// on `SessionCostView` / `ContextView`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransientSlice {
    /// Session token usage + spend — the status bar's cost readout and the
    /// `/cost` screen. `None` until the first `SessionCost` event.
    pub cost: Option<SessionCostView>,
    /// MCP server readiness keyed by server name — `/doctor` + the
    /// right-rail Activity panel.
    pub mcp_status: HashMap<String, McpServerStatus>,
    /// Context-window usage for the status meter.
    pub context: ContextView,
    /// The transient toast string (e.g. `McpReady` demoted to a status-bar
    /// toast, W5). `None` when nothing is showing.
    pub toast: Option<String>,
    /// When the current `toast` was set, for auto-dismiss. `None` when no
    /// toast is showing.
    pub toast_at: Option<Instant>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_transient_slice_is_all_empty() {
        let t = TransientSlice::default();
        assert!(t.cost.is_none());
        assert!(t.mcp_status.is_empty());
        assert_eq!(t.context.used_tokens, 0);
        assert!(t.toast.is_none());
        assert!(t.toast_at.is_none());
    }

    #[test]
    fn equal_slices_compare_equal_for_the_no_op_guard() {
        // The whole point of the slice: two structurally-equal snapshots
        // compare equal so `Store::set` can skip a no-op write.
        let a = TransientSlice {
            toast: Some("ready".into()),
            ..Default::default()
        };
        let b = TransientSlice {
            toast: Some("ready".into()),
            ..Default::default()
        };
        assert_eq!(a, b);
        let c = TransientSlice {
            toast: Some("other".into()),
            ..Default::default()
        };
        assert_ne!(a, c);
    }
}
