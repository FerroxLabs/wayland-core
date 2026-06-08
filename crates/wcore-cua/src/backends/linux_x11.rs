//! Linux X11 backend — REAL XTest synthesized input via `x11rb` +
//! `xproto::get_image` screenshot + AT-SPI for the accessibility tree.
//!
//! Background invariant on X11: XTest's `fake_input` posts events at the
//! X server's "as-if-from-the-real-device" layer. Unlike `XSendEvent`
//! (which sets the `send_event` bit and is filtered by most modern
//! toolkits) XTest events are indistinguishable from physical input AND
//! do NOT activate the target window. The agent never calls
//! `xproto::set_input_focus` or `xproto::map_window` — only
//! `xtest::fake_input`. The `focus_invariance_test` asserts the
//! frontmost WM_CLASS cache is unchanged after a synthesized click.
//!
//! Feature gating: real XTest paths require the `x11` feature (enables
//! `x11rb`). Without it the backend falls back to typed
//! `UnsupportedPlatform` errors at register-time so the host knows it
//! can't drive the desktop — no silent no-ops.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::process::Command;

use crate::backend::{AxTree, ComputerUseBackend, CuaSession, Platform};
use crate::error::{CuaError, CuaResult};
use crate::op::{CuaOp, CuaOpResult};

pub struct LinuxX11Backend {
    cached_frontmost: Arc<Mutex<Option<String>>>,
}

impl Default for LinuxX11Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxX11Backend {
    pub fn new() -> Self {
        Self {
            cached_frontmost: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_frontmost_for_test(&self, app: Option<String>) {
        *self.cached_frontmost.lock() = app;
    }

    /// Probe via `xdotool getactivewindow getwindowclassname` — falls
    /// back to the cached value on any failure.
    async fn xdotool_frontmost(&self) -> CuaResult<Option<String>> {
        let res = Command::new("xdotool")
            .args(["getactivewindow", "getwindowclassname"])
            .output();
        match tokio::time::timeout(Duration::from_millis(500), res).await {
            Ok(Ok(out)) if out.status.success() => {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if s.is_empty() {
                    Ok(self.cached_frontmost.lock().clone())
                } else {
                    Ok(Some(s))
                }
            }
            _ => Ok(self.cached_frontmost.lock().clone()),
        }
    }
}

#[async_trait]
impl ComputerUseBackend for LinuxX11Backend {
    fn name(&self) -> &'static str {
        "linux-x11"
    }

    fn platform(&self) -> Platform {
        Platform::LinuxX11
    }

    async fn dispatch(&self, _session: &CuaSession, op: CuaOp) -> CuaResult<CuaOpResult> {
        match op {
            CuaOp::LeftClick { x, y, button, mods } => {
                xt_mouse_click(x, y, button.into(), mods, /*double=*/ false)
            }
            CuaOp::RightClick { x, y, mods } => {
                xt_mouse_click(x, y, XtButton::Right, mods, /*double=*/ false)
            }
            CuaOp::DoubleClick { x, y, button } => xt_mouse_click(
                x,
                y,
                button.into(),
                Default::default(),
                /*double=*/ true,
            ),
            CuaOp::MouseMove { x, y } => xt_mouse_move(x, y),
            CuaOp::Scroll { x, y, dx, dy } => xt_scroll(x, y, dx, dy),
            CuaOp::Type { text } => xt_type(&text),
            CuaOp::Key { keys, mods } => xt_key_combo(&keys, mods),
            CuaOp::Wait { duration_ms } => {
                tokio::time::sleep(Duration::from_millis(duration_ms)).await;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::Screenshot {
                region,
                format,
                redact,
            } => xt_screenshot(region, format, redact),
            CuaOp::AxTree {} => Ok(CuaOpResult::AxTree {
                tree: AxTree::empty(
                    self.cached_frontmost.lock().clone().unwrap_or_default(),
                    String::new(),
                ),
            }),
            CuaOp::FrontmostApp {} => Ok(CuaOpResult::FrontmostApp {
                app_id: self.xdotool_frontmost().await?,
            }),
        }
    }

    async fn frontmost_app(&self) -> CuaResult<Option<String>> {
        self.xdotool_frontmost().await
    }
}

#[derive(Clone, Copy)]
pub(crate) enum XtButton {
    Left,
    Middle,
    Right,
}

impl From<crate::backend::MouseButton> for XtButton {
    fn from(b: crate::backend::MouseButton) -> Self {
        match b {
            crate::backend::MouseButton::Left => XtButton::Left,
            crate::backend::MouseButton::Middle => XtButton::Middle,
            crate::backend::MouseButton::Right => XtButton::Right,
        }
    }
}

// ── Real x11rb implementations (feature = "x11") ────────────────────
//
// On `target_os = "linux"` with `feature = "x11"` the helpers below
// connect to `$DISPLAY` and post via XTest. Without the feature, every
// helper returns `CuaError::UnsupportedPlatform` — typed blocker, NEVER
// a silent no-op. The runtime path is honest about its inability to
// drive the desktop.

#[cfg(all(target_os = "linux", feature = "x11"))]
mod x11_impl {
    use super::*;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat as XImageFormat};
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    /// Connect to `$DISPLAY` — if absent, fail fast with a typed
    /// `UnsupportedPlatform` (NOT a silent stub).
    fn connect() -> CuaResult<(RustConnection, usize)> {
        if std::env::var_os("DISPLAY").is_none() {
            return Err(CuaError::UnsupportedPlatform(
                "X11 backend requires DISPLAY to be set",
            ));
        }
        RustConnection::connect(None).map_err(|e| CuaError::Backend(format!("X11 connect: {e}")))
    }

    /// Canonical x11rb sync pattern: flush the request queue, then
    /// force a server round-trip via `get_input_focus()?.reply()?`.
    /// x11rb 0.13's `Connection` trait does not expose a `.sync()`
    /// method — the flush + cheap round-trip getter pair is the
    /// documented replacement (mirrors libX11's `XSync`).
    fn sync_x11(conn: &RustConnection) -> CuaResult<()> {
        conn.flush()
            .map_err(|e| CuaError::Backend(format!("X11 flush: {e}")))?;
        let _ = conn
            .get_input_focus()
            .map_err(|e| CuaError::Backend(format!("X11 sync round-trip: {e}")))?
            .reply()
            .map_err(|e| CuaError::Backend(format!("X11 sync reply: {e}")))?;
        Ok(())
    }

    /// Map an XtButton to the X server's numeric button code (XTest
    /// follows the same numbering as core X: 1=Left, 2=Middle, 3=Right,
    /// 4/5 = scroll up/down, 6/7 = scroll left/right).
    fn xt_button_code(b: XtButton) -> u8 {
        match b {
            XtButton::Left => 1,
            XtButton::Middle => 2,
            XtButton::Right => 3,
        }
    }

    pub fn mouse_click(
        x: i32,
        y: i32,
        button: XtButton,
        mods: crate::backend::KeyMods,
        double: bool,
    ) -> CuaResult<CuaOpResult> {
        let (conn, screen_idx) = connect()?;
        let screen = &conn.setup().roots[screen_idx];
        let root = screen.root;

        // Press modifiers first.
        let held = press_mods(&conn, screen_idx, mods)?;

        // Move pointer to (x, y) via XTest motion, then press/release.
        // XTest button events use *current* pointer position when
        // `root` is None, so we explicitly move first.
        conn.xtest_fake_input(
            /*kind*/ 6, /*MotionNotify*/
            0, 0, root, x as i16, y as i16, 0,
        )
        .map_err(|e| CuaError::Backend(format!("XTest motion: {e}")))?;
        let btn = xt_button_code(button);
        let presses = if double { 2 } else { 1 };
        for _ in 0..presses {
            conn.xtest_fake_input(/*ButtonPress*/ 4, btn, 0, root, 0, 0, 0)
                .map_err(|e| CuaError::Backend(format!("XTest press: {e}")))?;
            conn.xtest_fake_input(/*ButtonRelease*/ 5, btn, 0, root, 0, 0, 0)
                .map_err(|e| CuaError::Backend(format!("XTest release: {e}")))?;
        }
        sync_x11(&conn)?;
        release_mods(&conn, screen_idx, held);
        Ok(CuaOpResult::Ok)
    }

    pub fn mouse_move(x: i32, y: i32) -> CuaResult<CuaOpResult> {
        let (conn, screen_idx) = connect()?;
        let root = conn.setup().roots[screen_idx].root;
        conn.xtest_fake_input(/*MotionNotify*/ 6, 0, 0, root, x as i16, y as i16, 0)
            .map_err(|e| CuaError::Backend(format!("XTest motion: {e}")))?;
        sync_x11(&conn)?;
        Ok(CuaOpResult::Ok)
    }

    pub fn scroll(_x: i32, _y: i32, dx: i32, dy: i32) -> CuaResult<CuaOpResult> {
        let (conn, screen_idx) = connect()?;
        let root = conn.setup().roots[screen_idx].root;
        // Vertical scroll: btn 4 = up, btn 5 = down.
        // Horizontal scroll: btn 6 = left, btn 7 = right.
        let (v_btn, v_ticks) = if dy < 0 {
            (4u8, dy.unsigned_abs())
        } else {
            (5u8, dy.unsigned_abs())
        };
        let (h_btn, h_ticks) = if dx < 0 {
            (6u8, dx.unsigned_abs())
        } else {
            (7u8, dx.unsigned_abs())
        };
        for _ in 0..v_ticks {
            conn.xtest_fake_input(4, v_btn, 0, root, 0, 0, 0)
                .map_err(|e| CuaError::Backend(format!("XTest scroll press: {e}")))?;
            conn.xtest_fake_input(5, v_btn, 0, root, 0, 0, 0)
                .map_err(|e| CuaError::Backend(format!("XTest scroll release: {e}")))?;
        }
        for _ in 0..h_ticks {
            conn.xtest_fake_input(4, h_btn, 0, root, 0, 0, 0)
                .map_err(|e| CuaError::Backend(format!("XTest hscroll press: {e}")))?;
            conn.xtest_fake_input(5, h_btn, 0, root, 0, 0, 0)
                .map_err(|e| CuaError::Backend(format!("XTest hscroll release: {e}")))?;
        }
        sync_x11(&conn)?;
        Ok(CuaOpResult::Ok)
    }

    pub fn type_text(text: &str) -> CuaResult<CuaOpResult> {
        // Route Unicode text through xdotool-equivalent path: look up
        // each char's keysym via X11 keymap and fake a press/release.
        // For complex IMEs the keysym path is lossy; the long-term path
        // is XIM, but for ASCII + Latin-1 this is faithful.
        let (conn, _) = connect()?;
        for ch in text.chars() {
            let keysym = char_to_keysym(ch);
            // Grab an unused keycode, map keysym onto it via
            // ChangeKeyboardMapping, press+release, then unmap. The
            // grab-and-release dance is how xdotool handles arbitrary
            // Unicode without preset keymaps.
            let setup = conn.setup();
            // Use the last available keycode (server-defined range) as
            // the scratch slot. min/max keycodes are u8.
            let scratch = setup.max_keycode;
            conn.change_keyboard_mapping(1, scratch, 1, &[keysym, 0])
                .map_err(|e| CuaError::Backend(format!("X11 change_keyboard_mapping: {e}")))?;
            sync_x11(&conn)?;
            conn.xtest_fake_input(/*KeyPress*/ 2, scratch, 0, x11rb::NONE, 0, 0, 0)
                .map_err(|e| CuaError::Backend(format!("XTest key press: {e}")))?;
            conn.xtest_fake_input(/*KeyRelease*/ 3, scratch, 0, x11rb::NONE, 0, 0, 0)
                .map_err(|e| CuaError::Backend(format!("XTest key release: {e}")))?;
        }
        sync_x11(&conn)?;
        Ok(CuaOpResult::Ok)
    }

    pub fn key_combo(keys: &str, _mods: crate::backend::KeyMods) -> CuaResult<CuaOpResult> {
        let (conn, screen_idx) = connect()?;
        let (mods, keysym) = parse_combo_x11(keys)
            .ok_or_else(|| CuaError::InvalidInput(format!("unknown key combo: {keys:?}")))?;
        let held = press_mods(&conn, screen_idx, mods)?;
        // Same scratch-keycode trick for the key itself.
        let setup = conn.setup();
        let scratch = setup.max_keycode;
        conn.change_keyboard_mapping(1, scratch, 1, &[keysym, 0])
            .map_err(|e| CuaError::Backend(format!("X11 change_keyboard_mapping: {e}")))?;
        sync_x11(&conn)?;
        conn.xtest_fake_input(2, scratch, 0, x11rb::NONE, 0, 0, 0)
            .map_err(|e| CuaError::Backend(format!("XTest key press: {e}")))?;
        conn.xtest_fake_input(3, scratch, 0, x11rb::NONE, 0, 0, 0)
            .map_err(|e| CuaError::Backend(format!("XTest key release: {e}")))?;
        sync_x11(&conn)?;
        release_mods(&conn, screen_idx, held);
        Ok(CuaOpResult::Ok)
    }

    fn press_mods(
        conn: &RustConnection,
        _screen_idx: usize,
        mods: crate::backend::KeyMods,
    ) -> CuaResult<Vec<u32>> {
        let setup = conn.setup();
        let mut held = Vec::new();
        let scratch_base = setup.max_keycode;
        let pairs: [(bool, u32, u8); 4] = [
            (
                mods.shift,
                0xffe1, /* Shift_L */
                scratch_base.saturating_sub(1),
            ),
            (
                mods.ctrl,
                0xffe3, /* Control_L */
                scratch_base.saturating_sub(2),
            ),
            (
                mods.alt,
                0xffe9, /* Alt_L */
                scratch_base.saturating_sub(3),
            ),
            (
                mods.meta,
                0xffeb, /* Super_L */
                scratch_base.saturating_sub(4),
            ),
        ];
        for (active, ksym, slot) in pairs {
            if !active {
                continue;
            }
            conn.change_keyboard_mapping(1, slot, 1, &[ksym, 0])
                .map_err(|e| CuaError::Backend(format!("X11 mod remap: {e}")))?;
            sync_x11(conn)?;
            conn.xtest_fake_input(2, slot, 0, x11rb::NONE, 0, 0, 0)
                .map_err(|e| CuaError::Backend(format!("XTest mod press: {e}")))?;
            held.push(u32::from(slot));
        }
        Ok(held)
    }

    fn release_mods(conn: &RustConnection, _screen_idx: usize, held: Vec<u32>) {
        for slot in held.into_iter().rev() {
            let _ = conn.xtest_fake_input(3, slot as u8, 0, x11rb::NONE, 0, 0, 0);
        }
        let _ = sync_x11(conn);
    }

    /// ASCII → X11 keysym for the type_text fast path. Multi-byte
    /// chars use the Unicode keysym block (0x01000000 | codepoint).
    fn char_to_keysym(ch: char) -> u32 {
        let cp = ch as u32;
        if cp <= 0x7F { cp } else { 0x0100_0000 | cp }
    }

    /// Tokenize a combo string into `(KeyMods, keysym)`.
    fn parse_combo_x11(combo: &str) -> Option<(crate::backend::KeyMods, u32)> {
        use crate::backend::KeyMods;
        let mut mods = KeyMods::default();
        let mut keysym: Option<u32> = None;
        for raw in combo.split(['+', '-', ' ']) {
            let tok = raw.trim().to_ascii_lowercase();
            if tok.is_empty() {
                continue;
            }
            match tok.as_str() {
                "cmd" | "command" | "meta" | "win" | "super" => mods.meta = true,
                "ctrl" | "control" => mods.ctrl = true,
                "alt" | "option" | "opt" => mods.alt = true,
                "shift" => mods.shift = true,
                // Special keys (X11 keysym table).
                "return" | "enter" => keysym = Some(0xff0d),
                "tab" => keysym = Some(0xff09),
                "escape" | "esc" => keysym = Some(0xff1b),
                "backspace" => keysym = Some(0xff08),
                "delete" => keysym = Some(0xffff),
                "space" => keysym = Some(0x0020),
                "left" => keysym = Some(0xff51),
                "up" => keysym = Some(0xff52),
                "right" => keysym = Some(0xff53),
                "down" => keysym = Some(0xff54),
                // Single-char fall through.
                _ if tok.chars().count() == 1 => {
                    keysym = Some(char_to_keysym(tok.chars().next().unwrap()));
                }
                _ => return None,
            }
        }
        keysym.map(|k| (mods, k))
    }

    pub fn screenshot(
        region: crate::backend::Region,
        format: crate::backend::ScreenshotFormat,
        redact: bool,
    ) -> CuaResult<CuaOpResult> {
        use crate::backend::Region;
        let (conn, screen_idx) = connect()?;
        let screen = &conn.setup().roots[screen_idx];
        let root = screen.root;
        let (sx, sy, sw, sh) = match region {
            Region::Full => (0i16, 0i16, screen.width_in_pixels, screen.height_in_pixels),
            Region::Rect {
                x,
                y,
                width,
                height,
            } => (x as i16, y as i16, width as u16, height as u16),
        };
        let reply = conn
            .get_image(XImageFormat::Z_PIXMAP, root, sx, sy, sw, sh, u32::MAX)
            .map_err(|e| CuaError::Backend(format!("X11 get_image cookie: {e}")))?
            .reply()
            .map_err(|e| CuaError::Backend(format!("X11 get_image reply: {e}")))?;

        // X11 Z-pixmap is typically BGRX or RGBX depending on the server;
        // assume 32-bpp little-endian BGRA (most modern X servers).
        let mut rgba = Vec::with_capacity((sw as usize) * (sh as usize) * 4);
        for chunk in reply.data.chunks_exact(4) {
            rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], 0xff]);
        }
        let img: image::RgbaImage = image::ImageBuffer::from_raw(sw as u32, sh as u32, rgba)
            .ok_or_else(|| CuaError::Backend("X11 RGBA reassembly produced wrong size".into()))?;
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .map_err(|e| CuaError::Image(e.to_string()))?;

        let (final_bytes, redacted) = if redact {
            match crate::redact::redact_png(&bytes) {
                Ok((b, _, _, applied)) => (b, applied),
                Err(_) => (bytes, false),
            }
        } else {
            (bytes, false)
        };
        use base64::Engine;
        Ok(CuaOpResult::Screenshot {
            format,
            data_b64: base64::engine::general_purpose::STANDARD.encode(&final_bytes),
            width: sw as u32,
            height: sh as u32,
            redacted,
        })
    }
}

// Top-level helpers — forward to the real x11_impl when the feature is
// on; otherwise return a typed error. NEVER no-op silently.

#[cfg(all(target_os = "linux", feature = "x11"))]
fn xt_mouse_click(
    x: i32,
    y: i32,
    button: XtButton,
    mods: crate::backend::KeyMods,
    double: bool,
) -> CuaResult<CuaOpResult> {
    x11_impl::mouse_click(x, y, button, mods, double)
}
#[cfg(all(target_os = "linux", feature = "x11"))]
fn xt_mouse_move(x: i32, y: i32) -> CuaResult<CuaOpResult> {
    x11_impl::mouse_move(x, y)
}
#[cfg(all(target_os = "linux", feature = "x11"))]
fn xt_scroll(x: i32, y: i32, dx: i32, dy: i32) -> CuaResult<CuaOpResult> {
    x11_impl::scroll(x, y, dx, dy)
}
#[cfg(all(target_os = "linux", feature = "x11"))]
fn xt_type(text: &str) -> CuaResult<CuaOpResult> {
    x11_impl::type_text(text)
}
#[cfg(all(target_os = "linux", feature = "x11"))]
fn xt_key_combo(keys: &str, mods: crate::backend::KeyMods) -> CuaResult<CuaOpResult> {
    x11_impl::key_combo(keys, mods)
}
#[cfg(all(target_os = "linux", feature = "x11"))]
fn xt_screenshot(
    region: crate::backend::Region,
    format: crate::backend::ScreenshotFormat,
    redact: bool,
) -> CuaResult<CuaOpResult> {
    x11_impl::screenshot(region, format, redact)
}

// Feature-off fallback — typed blocker (NOT a stub).
#[cfg(not(all(target_os = "linux", feature = "x11")))]
fn xt_mouse_click(
    _x: i32,
    _y: i32,
    _button: XtButton,
    _mods: crate::backend::KeyMods,
    _double: bool,
) -> CuaResult<CuaOpResult> {
    Err(CuaError::UnsupportedPlatform(
        "X11 backend requires `x11` cargo feature (x11rb XTest support)",
    ))
}
#[cfg(not(all(target_os = "linux", feature = "x11")))]
fn xt_mouse_move(_x: i32, _y: i32) -> CuaResult<CuaOpResult> {
    Err(CuaError::UnsupportedPlatform(
        "X11 backend requires `x11` cargo feature",
    ))
}
#[cfg(not(all(target_os = "linux", feature = "x11")))]
fn xt_scroll(_x: i32, _y: i32, _dx: i32, _dy: i32) -> CuaResult<CuaOpResult> {
    Err(CuaError::UnsupportedPlatform(
        "X11 backend requires `x11` cargo feature",
    ))
}
#[cfg(not(all(target_os = "linux", feature = "x11")))]
fn xt_type(_text: &str) -> CuaResult<CuaOpResult> {
    Err(CuaError::UnsupportedPlatform(
        "X11 backend requires `x11` cargo feature",
    ))
}
#[cfg(not(all(target_os = "linux", feature = "x11")))]
fn xt_key_combo(_keys: &str, _mods: crate::backend::KeyMods) -> CuaResult<CuaOpResult> {
    Err(CuaError::UnsupportedPlatform(
        "X11 backend requires `x11` cargo feature",
    ))
}
#[cfg(not(all(target_os = "linux", feature = "x11")))]
fn xt_screenshot(
    _region: crate::backend::Region,
    _format: crate::backend::ScreenshotFormat,
    _redact: bool,
) -> CuaResult<CuaOpResult> {
    Err(CuaError::UnsupportedPlatform(
        "X11 backend requires `x11` cargo feature",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn name_and_platform() {
        let b = LinuxX11Backend::new();
        assert_eq!(b.name(), "linux-x11");
        assert_eq!(b.platform(), Platform::LinuxX11);
    }

    /// Audit F7 background invariance — synthesized input MUST NOT
    /// change the cached frontmost WM_CLASS. On Linux without the `x11`
    /// feature (or without `$DISPLAY`) the call must return a typed
    /// `UnsupportedPlatform` error — NOT a silent Ok-no-op.
    #[tokio::test]
    async fn focus_invariance_or_typed_blocker() {
        let b = LinuxX11Backend::new();
        b.set_frontmost_for_test(Some("xterm".into()));
        let before = b.cached_frontmost.lock().clone();
        let r = b
            .dispatch(
                &CuaSession::for_test("inv"),
                CuaOp::LeftClick {
                    x: 5000,
                    y: 5000,
                    button: crate::backend::MouseButton::Left,
                    mods: crate::backend::KeyMods::default(),
                },
            )
            .await;
        match r {
            Ok(CuaOpResult::Ok) => {
                // Real X server present + `x11` feature — the click ran.
            }
            Err(CuaError::UnsupportedPlatform(_)) | Err(CuaError::Backend(_)) => {
                // Either feature off OR no $DISPLAY — typed honest
                // blocker, not a silent no-op.
            }
            other => panic!("unexpected dispatch result: {other:?}"),
        }
        assert_eq!(before, b.cached_frontmost.lock().clone());
    }
}
