//! `wcore-browser` — multi-backend browser tool family.
//!
//! Design contract: `docs/superpowers/specs/2026-05-14-wcore-super-agent-design.md`
//! §5.16. ARIA-tree-first surface; **no `Evaluate` (arbitrary-JS) variant** in
//! v1 — see `op.rs` for the locked variant-count guard.
//!
//! Crate-graph position (mid-tier): depends DOWNWARD on `wcore-types`,
//! `wcore-tools`, `wcore-protocol`, `wcore-config`. MUST NOT depend on
//! `wcore-agent`, `wcore-cli`, `wcore-evolve`, `wcore-eval`. Plugin shells
//! (`wayland-*`) MUST NOT depend on this crate directly; they go through
//! `wcore-plugin-api::browser_spec::BrowserToolSpec` mirrors (audit F2).
//!
//! Three backends:
//! - **Camoufox** (PRIMARY): HTTP sidecar at `localhost:9377/...`. Default-on.
//! - **Chromium** (FALLBACK): chromiumoxide CDP client. Behind `chromium` feature.
//! - **Browserbase** (CLOUD): REST API, env-gated by `BROWSERBASE_API_KEY` +
//!   `BROWSERBASE_PROJECT_ID`. Behind `browserbase` feature.

pub mod aria;
pub mod backends;
pub mod binary;
pub mod op;
pub mod policy;
pub mod provider;
pub mod readability;
pub mod selection;
pub mod supervisor;
pub mod tool;

pub use aria::{AriaNode, AriaSnapshot, ElementRef};
pub use op::{BROWSER_OP_LOCKED_VARIANT_COUNT, BrowserOp};
pub use policy::{BrowserPolicy, PolicyAction, PolicyOutcome};
pub use provider::{
    BrowserOpError, BrowserProvider, BrowserSession, ClickTarget, ConsoleEntry, NetEntry,
    ScreenshotOpts, SessionCtx,
};
pub use selection::{ProviderHint, select_provider};
pub use supervisor::BrowserSupervisor;
pub use tool::BrowserTool;

/// Re-export of the host-adapter spec-to-type translator surface. The
/// `wayland-browser` plugin registers a `BrowserToolSpec` via the
/// plugin-api mirror; the host (wcore-agent) calls into this module to
/// reify a concrete tool.
pub mod adapter;
