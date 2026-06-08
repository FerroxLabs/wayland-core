//! `CuaOp` enum — the v1 computer-use surface (design §5.18).
//!
//! Surface is intentionally small: mouse, keyboard, screenshot, ax-tree,
//! and a "wait" primitive. We deliberately omit drag-and-drop in v1 —
//! drag operations create a window of vulnerability where focus + cursor
//! state are observable between the press and the release, breaking the
//! background invariant. Future revisions can add it behind a separate
//! capability flag (`computer_use_drag`) when we have an audit-clean way
//! to implement it.
//!
//! The locked-variant-count guard lives in `tests/op_enum_test.rs`;
//! touching this enum requires bumping
//! [`CUA_OP_LOCKED_VARIANT_COUNT`].

use serde::{Deserialize, Serialize};

use crate::backend::{KeyMods, MouseButton, Region, ScreenshotFormat};

/// Locked variant count per design §5.18. Bumping this requires an audit
/// pass — see `tests/op_enum_test.rs`.
pub const CUA_OP_LOCKED_VARIANT_COUNT: usize = 11;

/// Operations a CUA tool can perform. Serialized as a tagged union:
/// `{ "kind": "left_click", "x": 100, "y": 200 }`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CuaOp {
    /// Single click at screen coords. Default button is Left.
    LeftClick {
        x: i32,
        y: i32,
        #[serde(default)]
        button: MouseButton,
        #[serde(default)]
        mods: KeyMods,
    },
    /// Right click. Convenience variant so the policy layer can gate it
    /// separately from a left click.
    RightClick {
        x: i32,
        y: i32,
        #[serde(default)]
        mods: KeyMods,
    },
    /// Double click at screen coords.
    DoubleClick {
        x: i32,
        y: i32,
        #[serde(default)]
        button: MouseButton,
    },
    /// Move the synthesized cursor pointer. On platforms where the
    /// user-visible cursor can't be decoupled from the synthesized one,
    /// backends return `CuaError::UnsupportedPlatform` — the
    /// background invariant comes first.
    MouseMove { x: i32, y: i32 },
    /// Scroll N units at screen coords. Positive `dy` scrolls down,
    /// negative scrolls up; `dx` analogously for horizontal scroll.
    Scroll { x: i32, y: i32, dx: i32, dy: i32 },
    /// Type a string of literal text (the IME-friendly path — does NOT
    /// hold modifier keys).
    Type { text: String },
    /// Press a key combination, e.g. `"cmd+shift+t"`.
    Key {
        keys: String,
        #[serde(default)]
        mods: KeyMods,
    },
    /// Screenshot a region.
    Screenshot {
        #[serde(default)]
        region: Region,
        #[serde(default)]
        format: ScreenshotFormat,
        /// Redact sensitive UI patterns (password fields, etc.) before
        /// returning the bytes. Off by default — the plugin layer flips
        /// this via `CuaToolSpec::redact_screenshots`.
        #[serde(default)]
        redact: bool,
    },
    /// Walk the accessibility tree for the frontmost app.
    AxTree {},
    /// Wait `duration_ms` milliseconds. Tracked as an op (rather than a
    /// host-side sleep) so the cancel-token race wraps it consistently
    /// with the other CUA ops.
    Wait { duration_ms: u64 },
    /// Return the frontmost-app identifier (used by policy probes +
    /// host telemetry).
    FrontmostApp {},
}

impl CuaOp {
    /// Test-only helper — one representative of every variant so the
    /// variant-count + roundtrip scans in `tests/op_enum_test.rs` can
    /// iterate the surface.
    #[doc(hidden)]
    pub fn all_variants_for_test() -> Vec<CuaOp> {
        vec![
            CuaOp::LeftClick {
                x: 10,
                y: 20,
                button: MouseButton::Left,
                mods: KeyMods::default(),
            },
            CuaOp::RightClick {
                x: 10,
                y: 20,
                mods: KeyMods::default(),
            },
            CuaOp::DoubleClick {
                x: 10,
                y: 20,
                button: MouseButton::Left,
            },
            CuaOp::MouseMove { x: 30, y: 40 },
            CuaOp::Scroll {
                x: 0,
                y: 0,
                dx: 0,
                dy: -3,
            },
            CuaOp::Type {
                text: "hello".into(),
            },
            CuaOp::Key {
                keys: "cmd+shift+t".into(),
                mods: KeyMods::default(),
            },
            CuaOp::Screenshot {
                region: Region::Full,
                format: ScreenshotFormat::Png,
                redact: false,
            },
            CuaOp::AxTree {},
            CuaOp::Wait { duration_ms: 100 },
            CuaOp::FrontmostApp {},
        ]
    }

    /// Stable serde kind tag for telemetry / `CuaEvent` emission.
    pub fn kind_tag(&self) -> &'static str {
        match self {
            CuaOp::LeftClick { .. } => "left_click",
            CuaOp::RightClick { .. } => "right_click",
            CuaOp::DoubleClick { .. } => "double_click",
            CuaOp::MouseMove { .. } => "mouse_move",
            CuaOp::Scroll { .. } => "scroll",
            CuaOp::Type { .. } => "type",
            CuaOp::Key { .. } => "key",
            CuaOp::Screenshot { .. } => "screenshot",
            CuaOp::AxTree {} => "ax_tree",
            CuaOp::Wait { .. } => "wait",
            CuaOp::FrontmostApp {} => "frontmost_app",
        }
    }
}

/// Op result. Mirrors `BrowserOp::OpResult` shape — each variant carries
/// the per-op payload (PNG bytes for screenshot, the AxTree for ax-tree,
/// etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CuaOpResult {
    /// Mouse + keyboard ops that don't carry a payload.
    Ok,
    /// Screenshot bytes (PNG by default). Encoded as base64 in the JSON
    /// surface so the wire shape stays simple.
    Screenshot {
        format: ScreenshotFormat,
        /// Base64-encoded PNG bytes.
        data_b64: String,
        width: u32,
        height: u32,
        /// `true` when the redaction pass ran (matches `CuaOp::Screenshot::redact`).
        redacted: bool,
    },
    /// Walked accessibility tree.
    AxTree { tree: crate::backend::AxTree },
    /// Resolved frontmost app id (bundle id / window class / AumId).
    FrontmostApp { app_id: Option<String> },
}
