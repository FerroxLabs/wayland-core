//! Per-tool permission components (v0.9.2 W2 keystone). One pure
//! `PermissionComponent` per tool, routed by `permission_component_for`,
//! composed into a single inline card by `dialog::render`.
//!
//! This module FREEZES the contract that the 14 later component agents
//! (W3/W4) build against: `PermissionContext`, `PermissionComponent`,
//! `ApprovalAction`, `permission_component_for`, and `permission::render`.
//! W3/W4 add their own `components/<name>.rs` + dispatch arm; they never
//! reshape these signatures.

use ratatui::text::Line;

use crate::tui::app::ToolCardModel;
use crate::tui::theme::Theme;

pub mod components;
pub mod dialog;
pub mod dispatch;

pub use dispatch::permission_component_for;

/// The default action a bare-Enter performs on a card. PRESERVE: Enter =
/// approve once (§0 #3). Components override `default_action` only for
/// special cases (e.g. AskUserQuestion answers, ExitPlanMode approve-plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAction {
    /// Approve this single tool call (the bare-Enter default).
    ApproveOnce,
    /// Approve and persist an allow-rule for this tool/prefix.
    ApproveAlways,
    /// Deny the tool call.
    Deny,
}

/// Read-only context handed to every component. Borrowed from `App` at
/// render time — components never mutate state, so they are pure functions
/// over this context and trivially unit-testable on their text output.
pub struct PermissionContext<'a> {
    /// The pending tool call: `tool_name`, `summary`, `input_pretty`,
    /// `edit_preview`, `approval_reason`, `plan_body`.
    pub card: &'a ToolCardModel,
    /// The active theme (the chrome rule uses `theme.orange`).
    pub theme: &'a Theme,
    /// The render width in columns (full-width rule + body clamp budget).
    pub width: u16,
    /// Managed-rules gate (CC `shouldShowAlwaysAllowOptions`). When false,
    /// a component should not advertise the `[a] always` affordance.
    pub always_allow_available: bool,
    /// Live edit buffer when the Bash/PowerShell card is in prefix-edit
    /// mode; `None` otherwise.
    pub editable_prefix: Option<&'a str>,
    /// The live highlighted choice index for interactive cards
    /// (AskUserQuestion arrow-nav). Threaded from `WorkspaceSurface::
    /// approval_sel` at the render site so the rendered selection marker
    /// tracks the keys that move it. Components that have no selectable
    /// list ignore it; it defaults to 0 for the static render shim.
    pub selected_choice: usize,
    /// Whether the user has expanded a clamped card body (Ctrl+F). When
    /// true the body components render their full diff/args rather than the
    /// truncated preview. Defaults to `false` (clamped) for the shim.
    pub expanded: bool,
}

/// One tool's permission projection. Pure over `PermissionContext` so each
/// component is unit-testable on its title/body/keys text. The shared
/// `dialog::render` chrome composes `icon` + `title` + `body` + a blank +
/// `keys` into the single inline card.
pub trait PermissionComponent {
    /// The leading glyph for the card header (e.g. `⊘`, `✎`, `⬇`, `❯`).
    fn icon(&self) -> &'static str;
    /// The natural-language framing line (e.g. `Run a shell command`).
    fn title(&self, ctx: &PermissionContext) -> Line<'static>;
    /// The detail body — diff, command, args preview, reason, etc.
    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>>;
    /// The action key row (e.g. `[enter/y] approve   [a] always   …`).
    fn keys(&self, ctx: &PermissionContext) -> Line<'static>;
    /// The action a bare-Enter performs. Defaults to approve-once (§0 #3).
    fn default_action(&self) -> ApprovalAction {
        ApprovalAction::ApproveOnce
    }
}

/// Compose the routed component into the single inline approval card.
/// Re-exported for the workspace transcript render path. REPLACES the body
/// of `widgets::approval_inline::render_approval_inline`.
pub fn render(card: &ToolCardModel, ctx: &PermissionContext) -> Vec<Line<'static>> {
    dialog::render(card, ctx)
}
