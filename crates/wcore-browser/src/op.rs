//! `BrowserOp` enum — the v1 tool-surface. **No `Evaluate` variant** per
//! design §5.16 (REV-2 audit F6 lock).
//!
//! The locked-variant-count guard + forbidden-name scan live in
//! `tests/op_enum_test.rs`; touching this enum requires bumping
//! [`BROWSER_OP_LOCKED_VARIANT_COUNT`] AND re-auditing §5.16's Evaluate-ban
//! rationale.

use serde::{Deserialize, Serialize};

use crate::aria::ElementRef;
use crate::provider::ScreenshotOpts;

/// Locked variant count per design §5.16. Bumping this requires a follow-up
/// audit per §5.16 Evaluate-ban rationale — see `tests/op_enum_test.rs`.
pub const BROWSER_OP_LOCKED_VARIANT_COUNT: usize = 18;

/// Operations a browser tool can perform. The serialized form is a tagged
/// union — `{ "kind": "navigate", "url": "...", "wait_until_loaded": true }`
/// — so the surface stays clean over the JSON tool input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserOp {
    /// Navigate the current tab to a URL.
    Navigate {
        url: String,
        #[serde(default = "default_true")]
        wait_until_loaded: bool,
    },
    /// ARIA-tree snapshot of the current page; mints fresh `@eN` refs.
    Snapshot {},
    /// Readability-style markdown extraction.
    Read { mode: ReadMode },
    /// Click an element by post-snapshot ref.
    Click { target: ElementRef },
    /// Type text into an input identified by ref.
    Fill { target: ElementRef, text: String },
    /// Press a single key (`"Enter"`, `"Tab"`, ...).
    Press { key: String },
    /// Choose a `<select>` option.
    Select { target: ElementRef, value: String },
    /// Upload a file via a file input.
    ///
    /// SECURITY: `path` is model-controlled. The tool layer
    /// (`BrowserTool::execute_with_ctx`) confines it to the operator's
    /// downloads root and rejects `..`, dotfile/config locations, and
    /// symlink-escapes BEFORE the op reaches any backend — see
    /// `tool.rs::validate_local_path`. Backends must never treat this as a
    /// pre-validated path on their own.
    Upload { target: ElementRef, path: String },
    /// Trigger a download.
    ///
    /// SECURITY: `dest_path` is model-controlled and is confined the same
    /// way as `Upload::path` — see `tool.rs::validate_local_path`.
    Download { url: String, dest_path: String },
    /// Take a screenshot of the current viewport / full page.
    Screenshot {
        #[serde(default)]
        opts: ScreenshotOpts,
    },
    /// Return the current page URL + title (no DOM).
    GetState {},
    /// Wait until a CSS selector / aria role appears (with timeout).
    WaitFor { selector: String, timeout_ms: u64 },
    /// Dump the per-session network log.
    NetworkLog {},
    /// Dump the per-session console log.
    Console {},
    /// Open a new tab.
    NewTab { url: Option<String> },
    /// Close the current tab.
    CloseTab {},
    /// Go back one entry in the tab's history.
    Back {},
    /// Go forward one entry in the tab's history.
    Forward {},
}

fn default_true() -> bool {
    true
}

/// Read modes for `BrowserOp::Read`. `MainContent` is the default; `Article`
/// is more aggressive (drops sidebars + related links).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    MainContent,
    Article,
    /// Whole-page markdown (no readability heuristic).
    Raw,
}

impl BrowserOp {
    /// Test-only helper — produces one representative value of EVERY variant
    /// so the variant-count + forbidden-name scans in `tests/op_enum_test.rs`
    /// can iterate the surface.
    #[doc(hidden)]
    pub fn all_variants_for_test() -> Vec<BrowserOp> {
        vec![
            BrowserOp::Navigate {
                url: "https://example.com/".into(),
                wait_until_loaded: true,
            },
            BrowserOp::Snapshot {},
            BrowserOp::Read {
                mode: ReadMode::MainContent,
            },
            BrowserOp::Click {
                target: ElementRef::new("e1"),
            },
            BrowserOp::Fill {
                target: ElementRef::new("e2"),
                text: "hi".into(),
            },
            BrowserOp::Press {
                key: "Enter".into(),
            },
            BrowserOp::Select {
                target: ElementRef::new("e3"),
                value: "v".into(),
            },
            BrowserOp::Upload {
                target: ElementRef::new("e4"),
                path: "/tmp/x".into(),
            },
            BrowserOp::Download {
                url: "https://example.com/x".into(),
                dest_path: "/tmp/x".into(),
            },
            BrowserOp::Screenshot {
                opts: ScreenshotOpts::default(),
            },
            BrowserOp::GetState {},
            BrowserOp::WaitFor {
                selector: "#x".into(),
                timeout_ms: 1000,
            },
            BrowserOp::NetworkLog {},
            BrowserOp::Console {},
            BrowserOp::NewTab { url: None },
            BrowserOp::CloseTab {},
            BrowserOp::Back {},
            BrowserOp::Forward {},
        ]
    }
}
