//! `ComputerUseBackend` trait + shared platform-neutral types.
//!
//! Each platform backend (`backends::macos`, `backends::linux_x11`,
//! `backends::linux_wayland`, `backends::windows`) implements this trait.
//! The runtime selects a backend via `Platform::current()` and the tool
//! dispatcher hands every `CuaOp` to it.
//!
//! Background-mode invariant: every method must execute WITHOUT moving the
//! user's cursor, WITHOUT raising any window, and WITHOUT changing the
//! foreground window. Each backend ships a `focus_invariance_test` to
//! lock this in.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::CuaResult;
use crate::op::{CuaOp, CuaOpResult};

/// The host operating-system identity for backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    MacOs,
    LinuxX11,
    LinuxWayland,
    Windows,
    /// Build target the crate cannot satisfy at runtime.
    Unsupported,
}

impl Platform {
    /// Return the platform the crate was compiled for. On Linux, falls
    /// back to X11 unless `WAYLAND_DISPLAY` is set in the environment.
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Platform::MacOs
        }
        #[cfg(target_os = "linux")]
        {
            // Cheap runtime probe — Wayland sessions always export
            // `WAYLAND_DISPLAY`; X11 sessions don't.
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                Platform::LinuxWayland
            } else {
                Platform::LinuxX11
            }
        }
        #[cfg(target_os = "windows")]
        {
            Platform::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            Platform::Unsupported
        }
    }
}

/// A rectangular region in screen coordinates. `Full` covers the entire
/// virtual desktop spanning all displays.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(untagged)]
pub enum Region {
    #[default]
    Full,
    Rect {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

/// Modifier mask for keyboard ops. All flags default to false.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct KeyMods {
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    /// macOS Command / Windows Meta key.
    #[serde(default)]
    pub meta: bool,
}

/// On-wire screenshot format. PNG is the only required encoding.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotFormat {
    #[default]
    Png,
}

/// Per-sub-agent isolation handle. Matches `BrowserSession::ctx` shape so
/// the host can carry CUA + Browser sessions through the same plumbing.
#[derive(Debug, Clone)]
pub struct CuaSession {
    /// Stable session id. Each sub-agent gets its own session so an op's
    /// in-flight modifier state (held shift, locked caps) doesn't bleed
    /// across agents.
    pub session_id: String,
    /// Sub-agent name. `None` = main agent.
    pub sub_agent: Option<String>,
}

impl CuaSession {
    pub fn new(session_id: impl Into<String>, sub_agent: Option<String>) -> Self {
        Self {
            session_id: session_id.into(),
            sub_agent,
        }
    }

    /// Convenience for tests.
    pub fn for_test(id: &str) -> Self {
        Self::new(id, None)
    }
}

/// AT-SPI / UI Automation / NSAccessibility node. The shape is platform-
/// neutral: backends translate their native a11y trees into this normalized
/// form. v1 ships the minimal field set that maps cleanly onto all four
/// host APIs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxNode {
    pub role: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// Bounding box in screen coordinates (x, y, w, h). Optional — some
    /// nodes (group containers, the root window) don't have one.
    #[serde(default)]
    pub bounds: Option<[i32; 4]>,
    #[serde(default)]
    pub children: Vec<AxNode>,
}

impl AxNode {
    pub fn leaf(role: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            name: name.into(),
            value: String::new(),
            bounds: None,
            children: Vec::new(),
        }
    }
}

/// Top-level accessibility tree returned by `Backend::ax_tree`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxTree {
    /// Frontmost-app identifier (bundle id on macOS, window class on X11
    /// / Wayland, AumId on Windows).
    pub app_id: String,
    pub window_title: String,
    pub root: AxNode,
}

impl AxTree {
    pub fn empty(app_id: impl Into<String>, window_title: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            window_title: window_title.into(),
            root: AxNode::leaf("Application", ""),
        }
    }
}

/// Computer-use backend contract. Each platform provides one
/// implementation; the [`crate::tool::CuaTool`] dispatches `CuaOp`s
/// through this trait.
///
/// Cancellation: backends are NOT responsible for racing against the
/// cancel token themselves — `CuaTool::execute_with_ctx` handles the
/// race via `tokio::select!` so this trait stays cheap to implement.
#[async_trait]
pub trait ComputerUseBackend: Send + Sync {
    /// Identifier for the backend, e.g. `"macos"`, `"linux-x11"`.
    fn name(&self) -> &'static str;

    /// The `Platform` this backend serves.
    fn platform(&self) -> Platform;

    /// Dispatch a `CuaOp`. Returns the matching `CuaOpResult`.
    async fn dispatch(&self, session: &CuaSession, op: CuaOp) -> CuaResult<CuaOpResult>;

    /// Return the current frontmost-app identifier (used by the focus
    /// invariance tests + policy checks). Backends that can't determine
    /// it return `None` — the policy layer treats `None` as "no app
    /// match" and falls through to the default rule.
    async fn frontmost_app(&self) -> CuaResult<Option<String>>;
}
