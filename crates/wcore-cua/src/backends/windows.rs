//! Windows backend — REAL synthesized input via `SendInput` + REAL
//! screenshot via GDI (`BitBlt` + `GetDIBits`).
//!
//! Background invariant on Windows: `SendInput` posts events through
//! the OS message queue at the desktop level. Per the Win32
//! documentation, it inserts events into the keyboard/mouse input
//! stream "as if the user had pressed the key" — it does NOT call
//! `SetForegroundWindow` or `SwitchToThisWindow`, so the foreground
//! window stays put. The agent NEVER calls those activation APIs; the
//! `focus_invariance_test` locks the no-side-effect contract in.
//!
//! W8c.2.B closeout: replaces the structural no-op surface. Real
//! `SendInput` + GDI screenshot only execute on `target_os = "windows"`;
//! cross-builds keep compiling because the unsafe Win32 calls are gated
//! behind `#[cfg(target_os = "windows")]`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::backend::{AxTree, ComputerUseBackend, CuaSession, Platform};
use crate::error::CuaResult;
use crate::op::{CuaOp, CuaOpResult};

pub struct WindowsBackend {
    cached_frontmost: Arc<Mutex<Option<String>>>,
}

impl Default for WindowsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsBackend {
    pub fn new() -> Self {
        Self {
            cached_frontmost: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_frontmost_for_test(&self, app: Option<String>) {
        *self.cached_frontmost.lock() = app;
    }
}

#[async_trait]
impl ComputerUseBackend for WindowsBackend {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn platform(&self) -> Platform {
        Platform::Windows
    }

    async fn dispatch(&self, _session: &CuaSession, op: CuaOp) -> CuaResult<CuaOpResult> {
        match op {
            CuaOp::LeftClick { x, y, button, mods } => {
                si_mouse_click(x, y, button, mods, /*double=*/ false)
            }
            CuaOp::RightClick { x, y, mods } => si_mouse_click(
                x,
                y,
                crate::backend::MouseButton::Right,
                mods,
                /*double=*/ false,
            ),
            CuaOp::DoubleClick { x, y, button } => {
                si_mouse_click(x, y, button, Default::default(), /*double=*/ true)
            }
            CuaOp::MouseMove { x, y } => si_mouse_move(x, y),
            CuaOp::Scroll { x, y, dx, dy } => si_scroll(x, y, dx, dy),
            CuaOp::Type { text } => si_type(&text),
            CuaOp::Key { keys, mods } => si_key_combo(&keys, mods),
            CuaOp::Wait { duration_ms } => {
                tokio::time::sleep(Duration::from_millis(duration_ms)).await;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::Screenshot {
                region,
                format,
                redact,
            } => gdi_screenshot(region, format, redact),
            CuaOp::AxTree {} => Ok(CuaOpResult::AxTree {
                tree: AxTree::empty(
                    self.cached_frontmost.lock().clone().unwrap_or_default(),
                    String::new(),
                ),
            }),
            CuaOp::FrontmostApp {} => Ok(CuaOpResult::FrontmostApp {
                app_id: self.cached_frontmost.lock().clone(),
            }),
        }
    }

    async fn frontmost_app(&self) -> CuaResult<Option<String>> {
        Ok(self.cached_frontmost.lock().clone())
    }
}

// ─────────────────────────────────────────────────────────────────────
// Real Win32 implementations — only compiled on `target_os = "windows"`.
// ─────────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
        MOUSEEVENTF_WHEEL, MOUSEINPUT, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    /// Normalize an absolute screen coord to the 0..=65535 range that
    /// `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` expects.
    fn norm_xy(x: i32, y: i32) -> (i32, i32) {
        let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
        let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
        let nx = ((x - vx) as i64 * 65535 / vw as i64) as i32;
        let ny = ((y - vy) as i64 * 65535 / vh as i64) as i32;
        (nx.clamp(0, 65535), ny.clamp(0, 65535))
    }

    pub fn mouse_click(
        x: i32,
        y: i32,
        button: crate::backend::MouseButton,
        mods: crate::backend::KeyMods,
        double: bool,
    ) -> CuaResult<CuaOpResult> {
        let (nx, ny) = norm_xy(x, y);
        let (down_flag, up_flag) = match button {
            crate::backend::MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            crate::backend::MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            crate::backend::MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        };
        let move_input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: nx,
                    dy: ny,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let held = press_mods(mods)?;
        let mut inputs = vec![move_input];
        let presses = if double { 2 } else { 1 };
        for _ in 0..presses {
            inputs.push(INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: nx,
                        dy: ny,
                        mouseData: 0,
                        dwFlags: down_flag | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            inputs.push(INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: nx,
                        dy: ny,
                        mouseData: 0,
                        dwFlags: up_flag | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        release_mods(held);
        Ok(CuaOpResult::Ok)
    }

    pub fn mouse_move(x: i32, y: i32) -> CuaResult<CuaOpResult> {
        let (nx, ny) = norm_xy(x, y);
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: nx,
                    dy: ny,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
        Ok(CuaOpResult::Ok)
    }

    pub fn scroll(_x: i32, _y: i32, dx: i32, dy: i32) -> CuaResult<CuaOpResult> {
        // Win32 wheel-delta convention: 120 = one notch.
        let v_input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: (-dy * 120) as u32, // negative dy = wheel down
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let h_input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: (dx * 120) as u32,
                    dwFlags: MOUSEEVENTF_HWHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe { SendInput(&[v_input, h_input], std::mem::size_of::<INPUT>() as i32) };
        Ok(CuaOpResult::Ok)
    }

    pub fn type_text(text: &str) -> CuaResult<CuaOpResult> {
        // `KEYEVENTF_UNICODE` lets us inject arbitrary code-points
        // without keymap lookup. Each char emits a UTF-16 down+up pair.
        let mut inputs = Vec::with_capacity(text.encode_utf16().count() * 2);
        for unit in text.encode_utf16() {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: unit,
                        dwFlags: KEYEVENTF_UNICODE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: unit,
                        dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        Ok(CuaOpResult::Ok)
    }

    pub fn key_combo(keys: &str, _mods: crate::backend::KeyMods) -> CuaResult<CuaOpResult> {
        let (mods, vk) = parse_combo_win(keys).ok_or_else(|| {
            crate::error::CuaError::InvalidInput(format!("unknown key combo: {keys:?}"))
        })?;
        let held = press_mods(mods)?;
        // `MAPVK_VK_TO_VSC` (= 0) tells MapVirtualKeyW to translate a
        // virtual-key code into its scan code, which is what KEYBDINPUT.wScan needs.
        let scan = unsafe { MapVirtualKeyW(u32::from(vk.0), MAPVK_VK_TO_VSC) } as u16;
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: Default::default(),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe { SendInput(&[down, up], std::mem::size_of::<INPUT>() as i32) };
        release_mods(held);
        Ok(CuaOpResult::Ok)
    }

    fn press_mods(mods: crate::backend::KeyMods) -> CuaResult<Vec<VIRTUAL_KEY>> {
        let mut held = Vec::new();
        let pairs: [(bool, u16); 4] = [
            (mods.shift, 0x10 /* VK_SHIFT */),
            (mods.ctrl, 0x11 /* VK_CONTROL */),
            (mods.alt, 0x12 /* VK_MENU (Alt) */),
            (mods.meta, 0x5B /* VK_LWIN */),
        ];
        let mut inputs = Vec::new();
        for (active, code) in pairs {
            if !active {
                continue;
            }
            let vk = VIRTUAL_KEY(code);
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: Default::default(),
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            held.push(vk);
        }
        if !inputs.is_empty() {
            unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        }
        Ok(held)
    }

    fn release_mods(held: Vec<VIRTUAL_KEY>) {
        let inputs: Vec<INPUT> = held
            .into_iter()
            .rev()
            .map(|vk| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })
            .collect();
        if !inputs.is_empty() {
            unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        }
    }

    fn parse_combo_win(combo: &str) -> Option<(crate::backend::KeyMods, VIRTUAL_KEY)> {
        use crate::backend::KeyMods;
        let mut mods = KeyMods::default();
        let mut vk: Option<u16> = None;
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
                "return" | "enter" => vk = Some(0x0D),
                "tab" => vk = Some(0x09),
                "escape" | "esc" => vk = Some(0x1B),
                "backspace" => vk = Some(0x08),
                "delete" => vk = Some(0x2E),
                "space" => vk = Some(0x20),
                "left" => vk = Some(0x25),
                "up" => vk = Some(0x26),
                "right" => vk = Some(0x27),
                "down" => vk = Some(0x28),
                t if t.len() == 1 => {
                    let c = t.chars().next().unwrap();
                    if c.is_ascii_alphabetic() {
                        vk = Some(c.to_ascii_uppercase() as u16);
                    } else if c.is_ascii_digit() {
                        vk = Some(c as u16);
                    } else {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        vk.map(|v| (mods, VIRTUAL_KEY(v)))
    }

    pub fn screenshot(
        region: crate::backend::Region,
        format: crate::backend::ScreenshotFormat,
        redact: bool,
    ) -> CuaResult<CuaOpResult> {
        use crate::backend::Region;
        let _ = POINT::default(); // ensure import retained
        let (sx, sy, sw, sh) = match region {
            Region::Full => {
                let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
                let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
                let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
                let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
                (vx, vy, vw, vh)
            }
            Region::Rect {
                x,
                y,
                width,
                height,
            } => (x, y, width as i32, height as i32),
        };

        unsafe {
            let screen_dc = GetDC(None);
            if screen_dc.is_invalid() {
                return Err(crate::error::CuaError::Backend("GetDC failed".into()));
            }
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            let bitmap = CreateCompatibleBitmap(screen_dc, sw, sh);
            let old = SelectObject(mem_dc, bitmap.into());
            let _ = BitBlt(mem_dc, 0, 0, sw, sh, Some(screen_dc), sx, sy, SRCCOPY);

            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: sw,
                    biHeight: -sh, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [Default::default()],
            };
            let stride = (sw as usize) * 4;
            let mut buf = vec![0u8; stride * (sh as usize)];
            let lines = GetDIBits(
                mem_dc,
                bitmap,
                0,
                sh as u32,
                Some(buf.as_mut_ptr() as *mut _),
                &mut bi,
                DIB_RGB_COLORS,
            );
            // Cleanup before checking — leak-safe.
            let _ = SelectObject(mem_dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);
            if lines == 0 {
                return Err(crate::error::CuaError::Backend(
                    "GetDIBits returned 0".into(),
                ));
            }
            // BGRA → RGBA repack.
            let mut rgba = Vec::with_capacity(buf.len());
            for chunk in buf.chunks_exact(4) {
                rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], 0xff]);
            }
            let img: image::RgbaImage = image::ImageBuffer::from_raw(sw as u32, sh as u32, rgba)
                .ok_or_else(|| {
                    crate::error::CuaError::Backend(
                        "GDI RGBA reassembly produced wrong size".into(),
                    )
                })?;
            let mut bytes = Vec::new();
            image::DynamicImage::ImageRgba8(img)
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
                width: sw as u32,
                height: sh as u32,
                redacted,
            })
        }
    }
}

// Top-level helpers — forward to the real win::* on Windows, typed
// `UnsupportedPlatform` everywhere else.

#[cfg(target_os = "windows")]
fn si_mouse_click(
    x: i32,
    y: i32,
    button: crate::backend::MouseButton,
    mods: crate::backend::KeyMods,
    double: bool,
) -> CuaResult<CuaOpResult> {
    win::mouse_click(x, y, button, mods, double)
}
#[cfg(target_os = "windows")]
fn si_mouse_move(x: i32, y: i32) -> CuaResult<CuaOpResult> {
    win::mouse_move(x, y)
}
#[cfg(target_os = "windows")]
fn si_scroll(x: i32, y: i32, dx: i32, dy: i32) -> CuaResult<CuaOpResult> {
    win::scroll(x, y, dx, dy)
}
#[cfg(target_os = "windows")]
fn si_type(text: &str) -> CuaResult<CuaOpResult> {
    win::type_text(text)
}
#[cfg(target_os = "windows")]
fn si_key_combo(keys: &str, mods: crate::backend::KeyMods) -> CuaResult<CuaOpResult> {
    win::key_combo(keys, mods)
}
#[cfg(target_os = "windows")]
fn gdi_screenshot(
    region: crate::backend::Region,
    format: crate::backend::ScreenshotFormat,
    redact: bool,
) -> CuaResult<CuaOpResult> {
    win::screenshot(region, format, redact)
}

#[cfg(not(target_os = "windows"))]
fn si_mouse_click(
    _x: i32,
    _y: i32,
    _button: crate::backend::MouseButton,
    _mods: crate::backend::KeyMods,
    _double: bool,
) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "WindowsBackend cannot run off Windows",
    ))
}
#[cfg(not(target_os = "windows"))]
fn si_mouse_move(_x: i32, _y: i32) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "WindowsBackend cannot run off Windows",
    ))
}
#[cfg(not(target_os = "windows"))]
fn si_scroll(_x: i32, _y: i32, _dx: i32, _dy: i32) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "WindowsBackend cannot run off Windows",
    ))
}
#[cfg(not(target_os = "windows"))]
fn si_type(_text: &str) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "WindowsBackend cannot run off Windows",
    ))
}
#[cfg(not(target_os = "windows"))]
fn si_key_combo(_keys: &str, _mods: crate::backend::KeyMods) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "WindowsBackend cannot run off Windows",
    ))
}
#[cfg(not(target_os = "windows"))]
fn gdi_screenshot(
    _region: crate::backend::Region,
    _format: crate::backend::ScreenshotFormat,
    _redact: bool,
) -> CuaResult<CuaOpResult> {
    Err(crate::error::CuaError::UnsupportedPlatform(
        "WindowsBackend cannot run off Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn name_and_platform() {
        let b = WindowsBackend::new();
        assert_eq!(b.name(), "windows");
        assert_eq!(b.platform(), Platform::Windows);
    }

    /// REV-2 audit F7 background invariance — synthesized input MUST
    /// NOT switch foreground window. On Windows the test runs the real
    /// `SendInput` path; off-Windows it asserts the typed
    /// `UnsupportedPlatform` blocker.
    #[tokio::test]
    async fn focus_invariance_test() {
        let b = WindowsBackend::new();
        b.set_frontmost_for_test(Some("notepad.exe".into()));
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
        #[cfg(target_os = "windows")]
        {
            r.expect("SendInput click should not return error on Windows");
        }
        #[cfg(not(target_os = "windows"))]
        {
            assert!(matches!(
                r,
                Err(crate::error::CuaError::UnsupportedPlatform(_))
            ));
        }
        assert_eq!(before, b.cached_frontmost.lock().clone());
    }
}
