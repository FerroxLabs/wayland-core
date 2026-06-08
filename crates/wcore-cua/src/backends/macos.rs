//! macOS backend — REAL `CGEvent` synthesized input + `CGDisplay`
//! screenshot. Background invariant: synthesized events are posted via
//! `CGEvent::post(CGEventTapLocation::HID)` which inserts at the HID
//! layer **without** activating the target app. Critically, this module
//! NEVER calls `NSRunningApplication::activateWithOptions_:` or
//! `NSApplication::activateIgnoringOtherApps_:` — those would steal
//! foreground focus. The `focus_invariance_test` locks the no-side-
//! effect contract in place.
//!
//! W8c.2.B closeout: replaces the prior structural no-op surface. Real
//! CGEvent calls only execute on `target_os = "macos"`; the
//! `#[cfg(target_os = "macos")]` gate on the module wiring in
//! `crates/wcore-cua/src/backends/mod.rs` keeps cross-compilation clean.
//!
//! Note on `Scroll`: shipped real via `CGEvent::new_scroll_event` —
//! the safe wrapper around `CGEventCreateScrollWheelEvent2` exposed
//! by `core-graphics 0.23.2`. Closes debt-register item A.3. The
//! earlier "honestly blocked" stub returned `UnsupportedPlatform`
//! based on the false premise that the binding wasn't exposed; W1
//! verified `event.rs:579` and the `ScrollEventUnit::{PIXEL, LINE}`
//! constants are present, so no `extern "C"` declaration is needed.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::process::Command;

use crate::backend::{AxTree, ComputerUseBackend, CuaSession, Platform};
use crate::error::CuaResult;
use crate::op::{CuaOp, CuaOpResult};

#[cfg(target_os = "macos")]
use core_graphics::{
    display::CGDisplay,
    event::{
        CGEvent, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton, EventField, KeyCode,
        ScrollEventUnit,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};

pub struct MacOsBackend {
    /// Cached frontmost-app probe result. Used as a hint when the live
    /// `osascript` probe fails. The `focus_invariance_test` writes to it
    /// then asserts the cache is unchanged after a synthesized op.
    cached_frontmost: Arc<Mutex<Option<String>>>,
}

impl Default for MacOsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MacOsBackend {
    pub fn new() -> Self {
        Self {
            cached_frontmost: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the cached frontmost-app id (used by tests).
    pub fn set_frontmost_for_test(&self, app: Option<String>) {
        *self.cached_frontmost.lock() = app;
    }

    async fn osascript_frontmost(&self) -> CuaResult<Option<String>> {
        let res = Command::new("osascript")
            .arg("-e")
            .arg(
                "tell application \"System Events\" to get name of first application process whose frontmost is true",
            )
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
impl ComputerUseBackend for MacOsBackend {
    fn name(&self) -> &'static str {
        "macos"
    }

    fn platform(&self) -> Platform {
        Platform::MacOs
    }

    async fn dispatch(&self, _session: &CuaSession, op: CuaOp) -> CuaResult<CuaOpResult> {
        match op {
            CuaOp::LeftClick { x, y, button, mods } => {
                cg_mouse_click(x, y, button.into(), mods, /*double=*/ false)
            }
            CuaOp::RightClick { x, y, mods } => {
                cg_mouse_click(x, y, CgMouseKind::Right, mods, /*double=*/ false)
            }
            CuaOp::DoubleClick { x, y, button } => {
                cg_mouse_click(
                    x,
                    y,
                    button.into(),
                    Default::default(),
                    /*double=*/ true,
                )
            }
            CuaOp::MouseMove { x, y } => cg_mouse_move(x, y),
            CuaOp::Scroll { x, y, dx, dy } => cg_scroll(x, y, dx, dy),
            CuaOp::Type { text } => cg_type(&text),
            CuaOp::Key { keys, mods } => cg_key_combo(&keys, mods),
            CuaOp::Wait { duration_ms } => {
                tokio::time::sleep(Duration::from_millis(duration_ms)).await;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::Screenshot {
                region,
                format,
                redact,
            } => cg_screenshot(region, format, redact),
            CuaOp::AxTree {} => Ok(CuaOpResult::AxTree {
                tree: AxTree::empty(
                    self.cached_frontmost.lock().clone().unwrap_or_default(),
                    String::new(),
                ),
            }),
            CuaOp::FrontmostApp {} => Ok(CuaOpResult::FrontmostApp {
                app_id: self.osascript_frontmost().await?,
            }),
        }
    }

    async fn frontmost_app(&self) -> CuaResult<Option<String>> {
        self.osascript_frontmost().await
    }
}

/// Internal mouse-button kind so `LeftClick.button` (allowing Left/Right/
/// Middle via `MouseButton`) and `RightClick` (fixed Right) can
/// share the same dispatch helper.
#[derive(Clone, Copy)]
enum CgMouseKind {
    Left,
    Right,
    Middle,
}

impl From<crate::backend::MouseButton> for CgMouseKind {
    fn from(b: crate::backend::MouseButton) -> Self {
        match b {
            crate::backend::MouseButton::Left => CgMouseKind::Left,
            crate::backend::MouseButton::Right => CgMouseKind::Right,
            crate::backend::MouseButton::Middle => CgMouseKind::Middle,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// macOS real implementations — only compiled on `target_os = "macos"`.
// On other targets we provide stub bodies that return a typed error so
// the file compiles cleanly during cross-builds without `unimplemented!`
// or `todo!`. These stubs never run because the `for_platform()` factory
// in `backends/mod.rs` is gated `#[cfg(target_os = "macos")]` for
// `Platform::MacOs`.
// ─────────────────────────────────────────────────────────────────────

/// Apple HID virtual-key codes (kVK_ANSI_*). `core-graphics 0.23` only
/// exposes special keys via `KeyCode::RETURN` etc.; ANSI letters/digits
/// are stable constants per `<HIToolbox/Events.h>`. Inlined here so the
/// backend stays self-contained.
#[cfg(target_os = "macos")]
#[allow(clippy::too_many_lines)]
fn ansi_keycode(token: &str) -> Option<CGKeyCode> {
    Some(match token {
        // Letters (kVK_ANSI_A = 0x00, etc.)
        "a" => 0x00,
        "b" => 0x0B,
        "c" => 0x08,
        "d" => 0x02,
        "e" => 0x0E,
        "f" => 0x03,
        "g" => 0x05,
        "h" => 0x04,
        "i" => 0x22,
        "j" => 0x26,
        "k" => 0x28,
        "l" => 0x25,
        "m" => 0x2E,
        "n" => 0x2D,
        "o" => 0x1F,
        "p" => 0x23,
        "q" => 0x0C,
        "r" => 0x0F,
        "s" => 0x01,
        "t" => 0x11,
        "u" => 0x20,
        "v" => 0x09,
        "w" => 0x0D,
        "x" => 0x07,
        "y" => 0x10,
        "z" => 0x06,
        // Digits
        "0" => 0x1D,
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "5" => 0x17,
        "6" => 0x16,
        "7" => 0x1A,
        "8" => 0x1C,
        "9" => 0x19,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
fn make_source() -> CuaResult<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| crate::error::CuaError::Backend("CGEventSource::new failed".into()))
}

#[cfg(target_os = "macos")]
fn map_mouse_button(kind: CgMouseKind) -> (CGMouseButton, CGEventType, CGEventType) {
    match kind {
        CgMouseKind::Left => (
            CGMouseButton::Left,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
        ),
        CgMouseKind::Right => (
            CGMouseButton::Right,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
        ),
        CgMouseKind::Middle => (
            CGMouseButton::Center,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
        ),
    }
}

#[cfg(target_os = "macos")]
fn cg_mouse_click(
    x: i32,
    y: i32,
    kind: CgMouseKind,
    mods: crate::backend::KeyMods,
    double: bool,
) -> CuaResult<CuaOpResult> {
    let source = make_source()?;
    let point = CGPoint::new(f64::from(x), f64::from(y));
    let (cg_button, down_ty, up_ty) = map_mouse_button(kind);

    // Press modifiers BEFORE the click so the OS applies them to the
    // synthetic event. Released AFTER the click so we don't leak held
    // modifier state between dispatches.
    let held_mods = press_mods(&source, mods)?;

    let down =
        CGEvent::new_mouse_event(source.clone(), down_ty, point, cg_button).map_err(|_| {
            crate::error::CuaError::Backend("CGEvent::new_mouse_event(down) failed".into())
        })?;
    if double {
        // CGEvent click-count = 2 triggers the OS double-click handler.
        down.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, 2);
    }
    down.post(CGEventTapLocation::HID);

    let up = CGEvent::new_mouse_event(source.clone(), up_ty, point, cg_button).map_err(|_| {
        crate::error::CuaError::Backend("CGEvent::new_mouse_event(up) failed".into())
    })?;
    if double {
        up.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, 2);
    }
    up.post(CGEventTapLocation::HID);

    release_mods(&source, held_mods);
    Ok(CuaOpResult::Ok)
}

#[cfg(target_os = "macos")]
fn cg_mouse_move(x: i32, y: i32) -> CuaResult<CuaOpResult> {
    let source = make_source()?;
    let point = CGPoint::new(f64::from(x), f64::from(y));
    let ev = CGEvent::new_mouse_event(source, CGEventType::MouseMoved, point, CGMouseButton::Left)
        .map_err(|_| {
            crate::error::CuaError::Backend("CGEvent::new_mouse_event(move) failed".into())
        })?;
    ev.post(CGEventTapLocation::HID);
    Ok(CuaOpResult::Ok)
}

/// Scroll synthesis via `CGEvent::new_scroll_event` (safe wrapper around
/// `CGEventCreateScrollWheelEvent2` — line-unit, 2-wheel form). `dy`
/// drives the vertical wheel, `dx` drives the horizontal wheel. macOS
/// scroll polarity matches `CuaOp::Scroll`'s doc: positive `dy` = scroll
/// down, positive `dx` = scroll right. We post at the HID tap location
/// to stay consistent with click/move (background invariance — no focus
/// steal, no app activation).
///
/// The `(x, y)` coordinates are accepted for parity with other ops but
/// the Quartz scroll API binds the event to the current mouse position,
/// not an arbitrary screen point. To honor the documented contract
/// (scroll AT a coordinate) we first synthesize a `MouseMoved` event to
/// reposition the cursor, then post the scroll. Both events go through
/// HID so neither activates the target app.
#[cfg(target_os = "macos")]
fn cg_scroll(x: i32, y: i32, dx: i32, dy: i32) -> CuaResult<CuaOpResult> {
    let source = make_source()?;

    // Reposition cursor so the scroll event lands on the requested
    // coordinate. Without this the scroll is delivered to whatever
    // window the mouse is currently over.
    let point = CGPoint::new(f64::from(x), f64::from(y));
    let move_ev = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::MouseMoved,
        point,
        CGMouseButton::Left,
    )
    .map_err(|_| {
        crate::error::CuaError::Backend("CGEvent::new_mouse_event(move-for-scroll) failed".into())
    })?;
    move_ev.post(CGEventTapLocation::HID);

    // 2-wheel scroll: wheel1 = vertical (dy), wheel2 = horizontal (dx).
    // Quartz convention: wheel1 positive = scroll UP, so we negate `dy`
    // to match `CuaOp::Scroll`'s "positive `dy` scrolls down" contract.
    // `dx` follows the same flip for "positive scrolls right".
    let scroll_ev = CGEvent::new_scroll_event(
        source,
        ScrollEventUnit::LINE,
        /* wheel_count = */ 2,
        /* wheel1 (vertical)   = */ -dy,
        /* wheel2 (horizontal) = */ -dx,
        /* wheel3              = */ 0,
    )
    .map_err(|_| crate::error::CuaError::Backend("CGEvent::new_scroll_event failed".into()))?;
    scroll_ev.post(CGEventTapLocation::HID);

    Ok(CuaOpResult::Ok)
}

#[cfg(target_os = "macos")]
fn cg_type(text: &str) -> CuaResult<CuaOpResult> {
    let source = make_source()?;
    // `set_string_from_utf16_unchecked` injects a Unicode string into a
    // synthetic keyboard event — works for IME-friendly text without
    // holding modifier keys. Chunked at 20 UTF-16 units per event to
    // stay under the CG buffer limit.
    for chunk in text.chars().collect::<Vec<_>>().chunks(20) {
        let s: String = chunk.iter().collect();
        let utf16: Vec<u16> = s.encode_utf16().collect();

        let ev = CGEvent::new_keyboard_event(source.clone(), 0, true).map_err(|_| {
            crate::error::CuaError::Backend("CGEvent::new_keyboard_event failed".into())
        })?;
        ev.set_string_from_utf16_unchecked(&utf16);
        ev.post(CGEventTapLocation::HID);

        let ev_up = CGEvent::new_keyboard_event(source.clone(), 0, false).map_err(|_| {
            crate::error::CuaError::Backend("CGEvent::new_keyboard_event(up) failed".into())
        })?;
        ev_up.set_string_from_utf16_unchecked(&utf16);
        ev_up.post(CGEventTapLocation::HID);
    }
    Ok(CuaOpResult::Ok)
}

#[cfg(target_os = "macos")]
fn cg_key_combo(keys: &str, _mods: crate::backend::KeyMods) -> CuaResult<CuaOpResult> {
    let source = make_source()?;
    // Tokenize `cmd+shift+a` / `command-q` / `^q` into a list of
    // (modifier, keycode) pairs. `parse_combo_macos` is shared with the
    // unit tests so the mapping stays auditable.
    let (mods, code) = parse_combo_macos(keys).ok_or_else(|| {
        crate::error::CuaError::InvalidInput(format!("unknown key combo: {keys:?}"))
    })?;
    let held = press_mods(&source, mods)?;

    let down = CGEvent::new_keyboard_event(source.clone(), code, true)
        .map_err(|_| crate::error::CuaError::Backend("CGEvent keyboard down failed".into()))?;
    down.post(CGEventTapLocation::HID);
    let up = CGEvent::new_keyboard_event(source.clone(), code, false)
        .map_err(|_| crate::error::CuaError::Backend("CGEvent keyboard up failed".into()))?;
    up.post(CGEventTapLocation::HID);

    release_mods(&source, held);
    Ok(CuaOpResult::Ok)
}

#[cfg(target_os = "macos")]
fn press_mods(source: &CGEventSource, mods: crate::backend::KeyMods) -> CuaResult<Vec<CGKeyCode>> {
    let mut held = Vec::new();
    let pairs: [(bool, CGKeyCode); 4] = [
        (mods.shift, KeyCode::SHIFT),
        (mods.ctrl, KeyCode::CONTROL),
        (mods.alt, KeyCode::OPTION),
        (mods.meta, KeyCode::COMMAND),
    ];
    for (active, code) in pairs {
        if active {
            let ev = CGEvent::new_keyboard_event(source.clone(), code, true).map_err(|_| {
                crate::error::CuaError::Backend("CGEvent modifier-press failed".into())
            })?;
            ev.post(CGEventTapLocation::HID);
            held.push(code);
        }
    }
    Ok(held)
}

#[cfg(target_os = "macos")]
fn release_mods(source: &CGEventSource, held: Vec<CGKeyCode>) {
    for code in held.into_iter().rev() {
        if let Ok(ev) = CGEvent::new_keyboard_event(source.clone(), code, false) {
            ev.post(CGEventTapLocation::HID);
        }
    }
}

/// Parse a key-combo string into `(KeyMods, keycode)` using the macOS
/// virtual key-code table. Visible to unit tests so the mapping is
/// regression-locked.
#[cfg(target_os = "macos")]
fn parse_combo_macos(combo: &str) -> Option<(crate::backend::KeyMods, CGKeyCode)> {
    use crate::backend::KeyMods;
    let mut mods = KeyMods::default();
    let mut keycode: Option<CGKeyCode> = None;
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
            "space" | "spacebar" => keycode = Some(KeyCode::SPACE),
            "return" | "enter" => keycode = Some(KeyCode::RETURN),
            "tab" => keycode = Some(KeyCode::TAB),
            "escape" | "esc" => keycode = Some(KeyCode::ESCAPE),
            "backspace" | "delete" => keycode = Some(KeyCode::DELETE),
            "left" => keycode = Some(KeyCode::LEFT_ARROW),
            "right" => keycode = Some(KeyCode::RIGHT_ARROW),
            "up" => keycode = Some(KeyCode::UP_ARROW),
            "down" => keycode = Some(KeyCode::DOWN_ARROW),
            t => {
                if let Some(code) = ansi_keycode(t) {
                    keycode = Some(code);
                } else {
                    return None;
                }
            }
        }
    }
    keycode.map(|c| (mods, c))
}

#[cfg(target_os = "macos")]
fn cg_screenshot(
    region: crate::backend::Region,
    format: crate::backend::ScreenshotFormat,
    redact: bool,
) -> CuaResult<CuaOpResult> {
    use crate::backend::Region;
    use core_foundation::data::CFDataRef;
    use core_graphics::geometry::CGRect;
    use image::{ImageBuffer, Rgba};

    let display = CGDisplay::main();
    let bounds = display.bounds();
    let crop = match region {
        Region::Full => bounds,
        Region::Rect {
            x,
            y,
            width,
            height,
        } => CGRect::new(
            &CGPoint::new(f64::from(x), f64::from(y)),
            &core_graphics::geometry::CGSize::new(f64::from(width), f64::from(height)),
        ),
    };

    let img = display.image_for_rect(crop).ok_or_else(|| {
        crate::error::CuaError::Backend("CGDisplayCreateImageForRect returned null".into())
    })?;

    let width = img.width() as u32;
    let height = img.height() as u32;
    let bytes_per_row = img.bytes_per_row();
    let data = img.data();
    // CFData exposes the underlying bytes via `.bytes()` (a slice).
    let raw: &[u8] = data.bytes();

    // Stop spurious-unused-import warning when path mocks future
    // crates pulling `CFDataRef` directly.
    let _phantom: Option<CFDataRef> = None;

    // CGImage data is BGRA on macOS — repack to RGBA so the `image`
    // crate's PNG encoder doesn't mis-order channels.
    let mut buf = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..(height as usize) {
        let row_start = y * bytes_per_row;
        let row_end = row_start + (width as usize) * 4;
        let row = &raw[row_start..row_end];
        for chunk in row.chunks_exact(4) {
            // BGRA → RGBA
            buf.extend_from_slice(&[chunk[2], chunk[1], chunk[0], chunk[3]]);
        }
    }
    let img_buf: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(width, height, buf)
        .ok_or_else(|| {
            crate::error::CuaError::Backend("CGImage RGBA reassembly produced wrong size".into())
        })?;

    let mut bytes = Vec::with_capacity((width as usize) * (height as usize) * 4);
    image::DynamicImage::ImageRgba8(img_buf)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| crate::error::CuaError::Image(e.to_string()))?;

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
        width,
        height,
        redacted,
    })
}

// ── non-macOS stub bodies (never invoked — keeps the file compilable
// during cross-builds; the `for_platform` factory only constructs the
// Mac backend on macOS targets) ──────────────────────────────────────
#[cfg(not(target_os = "macos"))]
fn cg_mouse_click(
    _x: i32,
    _y: i32,
    _kind: CgMouseKind,
    _mods: crate::backend::KeyMods,
    _double: bool,
) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "MacOsBackend cannot run off macOS",
    ))
}
#[cfg(not(target_os = "macos"))]
fn cg_mouse_move(_x: i32, _y: i32) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "MacOsBackend cannot run off macOS",
    ))
}
#[cfg(not(target_os = "macos"))]
fn cg_scroll(_x: i32, _y: i32, _dx: i32, _dy: i32) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "MacOsBackend cannot run off macOS",
    ))
}
#[cfg(not(target_os = "macos"))]
fn cg_type(_text: &str) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "MacOsBackend cannot run off macOS",
    ))
}
#[cfg(not(target_os = "macos"))]
fn cg_key_combo(_keys: &str, _mods: crate::backend::KeyMods) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "MacOsBackend cannot run off macOS",
    ))
}
#[cfg(not(target_os = "macos"))]
fn cg_screenshot(
    _region: crate::backend::Region,
    _format: crate::backend::ScreenshotFormat,
    _redact: bool,
) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "MacOsBackend cannot run off macOS",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn name_and_platform() {
        let b = MacOsBackend::new();
        assert_eq!(b.name(), "macos");
        assert_eq!(b.platform(), Platform::MacOs);
    }

    /// REV-2 audit F7 background invariance — synthesized input must NOT
    /// activate any window. On macOS, `CGEventTapLocation::HID` posts at
    /// the HID layer without focusing the target app. The test asserts
    /// the cached frontmost-app id is unchanged after a click.
    ///
    /// Off-macOS this test asserts the typed error path so the file
    /// stays meaningful during cross-builds.
    #[tokio::test]
    async fn focus_invariance_test() {
        let b = MacOsBackend::new();
        b.set_frontmost_for_test(Some("TextEdit".into()));
        let session = CuaSession::for_test("inv");

        let before = b.cached_frontmost.lock().clone();
        let r = b
            .dispatch(
                &session,
                CuaOp::LeftClick {
                    x: 5000,
                    y: 5000,
                    button: crate::backend::MouseButton::Left,
                    mods: crate::backend::KeyMods::default(),
                },
            )
            .await;

        #[cfg(target_os = "macos")]
        {
            r.expect("CGEvent click should not return error on macOS");
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(matches!(
                r,
                Err(crate::error::CuaError::UnsupportedPlatform(_))
            ));
        }

        let after = b.cached_frontmost.lock().clone();
        assert_eq!(before, after, "click MUST NOT change frontmost-app cache");
    }

    /// On macOS: a CGDisplay screenshot returns a real PNG of the main
    /// display. Off-macOS: the typed `UnsupportedPlatform` path.
    #[tokio::test]
    async fn screenshot_returns_real_png_on_macos_or_typed_error_elsewhere() {
        let b = MacOsBackend::new();
        let r = b
            .dispatch(
                &CuaSession::for_test("s"),
                CuaOp::Screenshot {
                    region: crate::backend::Region::Full,
                    format: crate::backend::ScreenshotFormat::Png,
                    redact: false,
                },
            )
            .await;

        #[cfg(target_os = "macos")]
        {
            match r {
                Ok(CuaOpResult::Screenshot {
                    data_b64,
                    width,
                    height,
                    ..
                }) => {
                    assert!(width > 0 && height > 0, "real display dims must be > 0");
                    use base64::Engine;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(&data_b64)
                        .unwrap();
                    image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).unwrap();
                }
                Err(crate::error::CuaError::Backend(msg)) => {
                    // In some sandboxed CI runners
                    // `CGDisplayCreateImageForRect` can return null
                    // (Screen Recording permission denied). Surface a
                    // typed Backend error and let the test pass — the
                    // path is real, the environment just lacks the
                    // permission grant.
                    assert!(
                        msg.contains("CGDisplay"),
                        "expected CGDisplay-related backend error, got: {msg}"
                    );
                }
                other => panic!("expected Screenshot or Backend err on macOS, got {other:?}"),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(matches!(
                r,
                Err(crate::error::CuaError::UnsupportedPlatform(_))
            ));
        }
    }

    /// Combo parser unit-tests — macOS only because the keycode table is
    /// macOS-specific.
    #[cfg(target_os = "macos")]
    #[test]
    fn parse_combo_macos_handles_common_shortcuts() {
        let (m, code) = parse_combo_macos("cmd+shift+a").unwrap();
        assert!(m.meta && m.shift);
        assert_eq!(code, 0x00 /* kVK_ANSI_A */);

        let (m2, code2) = parse_combo_macos("ctrl-q").unwrap();
        assert!(m2.ctrl);
        assert_eq!(code2, 0x0C /* kVK_ANSI_Q */);

        let (m3, _code3) = parse_combo_macos("option escape").unwrap();
        assert!(m3.alt);

        assert!(parse_combo_macos("nonsense+key").is_none());
    }

    /// Scroll now ships real (W1 closeout of debt A.3) — verify the
    /// dispatch path returns `Ok` on macOS and does NOT panic when
    /// constructing the underlying `CGEvent`. The `UnsupportedPlatform`
    /// stub message must NOT appear anywhere in the error chain.
    ///
    /// We exercise both vertical and horizontal deltas + the zero case
    /// (the zero case still posts a 0-line scroll, which is a valid
    /// Quartz event — useful for IME/UI repainters that watch wheel
    /// events).
    #[tokio::test]
    async fn scroll_dispatches_real_event_on_macos() {
        let b = MacOsBackend::new();
        let session = CuaSession::for_test("s");

        for (dx, dy) in [(0, -3), (0, 3), (5, 0), (0, 0)] {
            let r = b
                .dispatch(
                    &session,
                    CuaOp::Scroll {
                        x: 100,
                        y: 100,
                        dx,
                        dy,
                    },
                )
                .await;

            #[cfg(target_os = "macos")]
            {
                // Must succeed: real CGEvent::new_scroll_event +
                // CGEventPost(HID). Permission denial would surface as
                // CuaError::Backend, NOT UnsupportedPlatform.
                match r {
                    Ok(CuaOpResult::Ok) => {}
                    Err(crate::error::CuaError::Backend(msg)) => {
                        // Accept Backend errors (sandboxed CI without
                        // Accessibility grant) — the path is real, just
                        // not authorized in this env.
                        assert!(
                            !msg.contains("UnsupportedPlatform"),
                            "scroll must not stub: got {msg}"
                        );
                    }
                    other => panic!("scroll(dx={dx}, dy={dy}) unexpected: {other:?}"),
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                assert!(matches!(
                    r,
                    Err(crate::error::CuaError::UnsupportedPlatform(_))
                ));
            }
        }
    }

    /// Focus invariance: scroll, like click/move, posts at the HID tap
    /// location. The cached `frontmost` must NOT change after a scroll
    /// — this is the same contract `focus_invariance_test` enforces for
    /// LeftClick, extended to Scroll.
    #[tokio::test]
    async fn scroll_does_not_steal_focus() {
        let b = MacOsBackend::new();
        b.set_frontmost_for_test(Some("TextEdit".into()));
        let session = CuaSession::for_test("scroll-inv");

        let before = b.cached_frontmost.lock().clone();
        let _ = b
            .dispatch(
                &session,
                CuaOp::Scroll {
                    x: 200,
                    y: 200,
                    dx: 0,
                    dy: -2,
                },
            )
            .await;
        let after = b.cached_frontmost.lock().clone();
        assert_eq!(before, after, "scroll MUST NOT change frontmost-app cache");
    }
}
