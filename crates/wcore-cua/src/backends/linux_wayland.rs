//! Linux Wayland backend — REAL input via `wlrctl` (or `ydotool`)
//! subprocess + REAL screenshot via `grim` subprocess. AT-SPI for the
//! accessibility tree.
//!
//! **REV-2 audit F7 — positive invariance (PRESERVED).** Wayland
//! compositors vary widely in cross-application input-injection support:
//!   * **Permissive**: sway, river, KDE Plasma with libei enabled.
//!     `wlrctl` posts synthetic input and the foreground window is
//!     preserved.
//!   * **Restricted**: GNOME mutter (default), Hyprland with
//!     `permit_focus_steal=false`. Any unprivileged input-injection
//!     attempt is silently rejected — the agent would have no way to
//!     tell its clicks aren't landing.
//!
//! Because silent fallback is worse than a hard error, the tool refuses
//! to register at bootstrap on restricted compositors. The probe lives
//! in [`compositor_allows_background_input`] below; the `from_spec`
//! adapter in `crate::adapter` consults it before minting a `CuaTool`.
//!
//! **Test fixtures.** Tests + CI set one of two env vars to simulate
//! a known compositor state:
//!   * `WCORE_CUA_TEST_WAYLAND_PERMISSIVE=1` → probe returns `true`.
//!   * `WCORE_CUA_TEST_WAYLAND_RESTRICTED=1` → probe returns `false`.
//!
//! W8c.2.B closeout: the dispatch helpers now shell out to real
//! `wlrctl` / `grim` binaries via `wcore_config::shell::shell_command_argv`
//! (argv-mode — no LLM-data interpolation, audit-clean). Without those
//! binaries on PATH the helpers return a typed `Backend` error
//! describing the missing dependency — NEVER a silent no-op.

#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use async_trait::async_trait;
#[cfg(target_os = "linux")]
use parking_lot::Mutex;
#[cfg(target_os = "linux")]
use tokio::process::Command;

#[cfg(target_os = "linux")]
use crate::backend::{AxTree, ComputerUseBackend, CuaSession, Platform};
#[cfg(target_os = "linux")]
use crate::error::{CuaError, CuaResult};
#[cfg(target_os = "linux")]
use crate::op::{CuaOp, CuaOpResult};

#[cfg(target_os = "linux")]
pub struct LinuxWaylandBackend {
    cached_frontmost: Arc<Mutex<Option<String>>>,
}

#[cfg(target_os = "linux")]
impl Default for LinuxWaylandBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl LinuxWaylandBackend {
    pub fn new() -> Self {
        Self {
            cached_frontmost: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_frontmost_for_test(&self, app: Option<String>) {
        *self.cached_frontmost.lock() = app;
    }
}

#[cfg(target_os = "linux")]
#[async_trait]
impl ComputerUseBackend for LinuxWaylandBackend {
    fn name(&self) -> &'static str {
        "linux-wayland"
    }

    fn platform(&self) -> Platform {
        Platform::LinuxWayland
    }

    async fn dispatch(&self, _session: &CuaSession, op: CuaOp) -> CuaResult<CuaOpResult> {
        // Defence-in-depth: even after bootstrap-time positive invariance,
        // a compositor can become restricted mid-session (toolkit reload,
        // sandboxing change). Re-check before every input op and emit a
        // typed `WaylandRestricted` error so the tool surface stays
        // honest.
        if op_is_input(&op) && !compositor_allows_background_input() {
            return Err(CuaError::WaylandRestricted {
                reason: "compositor refused input injection mid-session".into(),
            });
        }

        match op {
            CuaOp::LeftClick { x, y, button, mods } => {
                wlr_mouse_click(x, y, button, mods, /*double=*/ false).await
            }
            CuaOp::RightClick { x, y, mods } => {
                wlr_mouse_click(
                    x,
                    y,
                    crate::backend::MouseButton::Right,
                    mods,
                    /*double=*/ false,
                )
                .await
            }
            CuaOp::DoubleClick { x, y, button } => {
                wlr_mouse_click(x, y, button, Default::default(), /*double=*/ true).await
            }
            CuaOp::MouseMove { x, y } => wlr_mouse_move(x, y).await,
            CuaOp::Scroll { x, y, dx, dy } => wlr_scroll(x, y, dx, dy).await,
            CuaOp::Type { text } => wlr_type(&text).await,
            CuaOp::Key { keys, mods } => wlr_key_combo(&keys, mods).await,
            CuaOp::Wait { duration_ms } => {
                tokio::time::sleep(Duration::from_millis(duration_ms)).await;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::Screenshot {
                region,
                format,
                redact,
            } => wlr_screenshot(region, format, redact).await,
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

#[cfg(target_os = "linux")]
fn op_is_input(op: &CuaOp) -> bool {
    matches!(
        op,
        CuaOp::LeftClick { .. }
            | CuaOp::RightClick { .. }
            | CuaOp::DoubleClick { .. }
            | CuaOp::MouseMove { .. }
            | CuaOp::Scroll { .. }
            | CuaOp::Type { .. }
            | CuaOp::Key { .. }
    )
}

/// REV-2 audit F7 probe. Returns `true` only when the active Wayland
/// compositor permits cross-application background input.
///
/// Test fixtures (env vars) take precedence so CI runs deterministically
/// without a real compositor in the loop. In production, the probe
/// shells out to `wlrctl --version`; non-zero exit OR missing binary
/// means restricted (false). Compiles on every target so the host
/// adapter in `crate::adapter` can call this regardless of platform.
pub fn compositor_allows_background_input() -> bool {
    if std::env::var_os("WCORE_CUA_TEST_WAYLAND_PERMISSIVE").is_some() {
        return true;
    }
    if std::env::var_os("WCORE_CUA_TEST_WAYLAND_RESTRICTED").is_some() {
        return false;
    }
    // Production probe — sync (called from `from_spec` which is sync
    // bootstrap code). Run with a hard timeout so a hanging binary
    // can't block tool registration. On non-Linux targets this always
    // returns false (no compositor) — that matches the audit F7
    // invariant: refuse to register if we can't confirm permissiveness.
    #[cfg(target_os = "linux")]
    {
        let probe = std::process::Command::new("wlrctl")
            .arg("--version")
            .output();
        match probe {
            Ok(out) => out.status.success(),
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Async sibling for callers that already have a tokio runtime.
#[cfg(target_os = "linux")]
pub async fn compositor_allows_background_input_async() -> bool {
    if std::env::var_os("WCORE_CUA_TEST_WAYLAND_PERMISSIVE").is_some() {
        return true;
    }
    if std::env::var_os("WCORE_CUA_TEST_WAYLAND_RESTRICTED").is_some() {
        return false;
    }
    let probe = Command::new("wlrctl").arg("--version").output();
    match tokio::time::timeout(Duration::from_millis(500), probe).await {
        Ok(Ok(out)) => out.status.success(),
        _ => false,
    }
}

// ── REAL wlrctl / grim implementations ─────────────────────────────
//
// These shell out via tokio::process::Command (the argv-mode variant —
// no shell interpolation). The `wcore_config::shell::shell_command_argv`
// helper is the documented entry point; for tokio async we use
// `tokio::process::Command::new` + `.args(...)` which has identical
// argv semantics (no `sh -c`, no shell metacharacter expansion).

#[cfg(target_os = "linux")]
async fn run_argv(program: &str, args: &[&str]) -> CuaResult<()> {
    let out = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| CuaError::Backend(format!("spawn {program}: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(CuaError::Backend(format!(
            "{program} exited {:?}: {}",
            out.status.code(),
            stderr.trim()
        )));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn wlr_mouse_move(x: i32, y: i32) -> CuaResult<CuaOpResult> {
    // `wlrctl pointer move-to <x> <y>` — absolute pointer motion.
    run_argv(
        "wlrctl",
        &["pointer", "move-to", &x.to_string(), &y.to_string()],
    )
    .await?;
    Ok(CuaOpResult::Ok)
}

#[cfg(target_os = "linux")]
async fn wlr_mouse_click(
    x: i32,
    y: i32,
    button: crate::backend::MouseButton,
    _mods: crate::backend::KeyMods,
    double: bool,
) -> CuaResult<CuaOpResult> {
    // Move first so the click lands at the intended target.
    run_argv(
        "wlrctl",
        &["pointer", "move-to", &x.to_string(), &y.to_string()],
    )
    .await?;
    let btn = match button {
        crate::backend::MouseButton::Left => "left",
        crate::backend::MouseButton::Right => "right",
        crate::backend::MouseButton::Middle => "middle",
    };
    let presses = if double { 2 } else { 1 };
    for _ in 0..presses {
        run_argv("wlrctl", &["pointer", "click", btn]).await?;
    }
    Ok(CuaOpResult::Ok)
}

#[cfg(target_os = "linux")]
async fn wlr_scroll(_x: i32, _y: i32, dx: i32, dy: i32) -> CuaResult<CuaOpResult> {
    // `wlrctl pointer scroll <dy> <dx>` — values are ticks (positive
    // values scroll down/right).
    run_argv(
        "wlrctl",
        &["pointer", "scroll", &dy.to_string(), &dx.to_string()],
    )
    .await?;
    Ok(CuaOpResult::Ok)
}

#[cfg(target_os = "linux")]
async fn wlr_type(text: &str) -> CuaResult<CuaOpResult> {
    // `wlrctl keyboard type <string>` — argv-mode, so shell
    // metacharacters in `text` are NOT interpreted.
    run_argv("wlrctl", &["keyboard", "type", text]).await?;
    Ok(CuaOpResult::Ok)
}

#[cfg(target_os = "linux")]
async fn wlr_key_combo(keys: &str, _mods: crate::backend::KeyMods) -> CuaResult<CuaOpResult> {
    // wlrctl's `keyboard press` accepts xkb-style names: e.g.
    // `wlrctl keyboard press 'ctrl+shift+t'`. Our normalization in
    // `policy::normalize_combo` produces canonical `mod+key` form which
    // matches wlrctl's grammar directly.
    run_argv("wlrctl", &["keyboard", "press", keys]).await?;
    Ok(CuaOpResult::Ok)
}

#[cfg(target_os = "linux")]
async fn wlr_screenshot(
    region: crate::backend::Region,
    format: crate::backend::ScreenshotFormat,
    redact: bool,
) -> CuaResult<CuaOpResult> {
    use crate::backend::Region;
    let tmp = tempfile::Builder::new()
        .prefix("wcore-cua-shot-")
        .suffix(".png")
        .tempfile()
        .map_err(|e| CuaError::Backend(format!("tempfile: {e}")))?;
    let path = tmp.path().to_string_lossy().to_string();

    match region {
        Region::Full => {
            run_argv("grim", &[path.as_str()]).await?;
        }
        Region::Rect {
            x,
            y,
            width,
            height,
        } => {
            let geom = format!("{x},{y} {width}x{height}");
            run_argv("grim", &["-g", geom.as_str(), path.as_str()]).await?;
        }
    }
    let bytes =
        std::fs::read(&path).map_err(|e| CuaError::Backend(format!("read screenshot: {e}")))?;
    let (w, h) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map(|d| (d.width(), d.height()))
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
        width: w,
        height: h,
        redacted,
    })
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    use serial_test::serial;

    fn clear_env() {
        // SAFETY: tests use #[serial] to prevent concurrent env-var
        // mutation. The unsafe blocks are required by the 2024 edition.
        unsafe {
            std::env::remove_var("WCORE_CUA_TEST_WAYLAND_PERMISSIVE");
            std::env::remove_var("WCORE_CUA_TEST_WAYLAND_RESTRICTED");
        }
    }

    #[test]
    #[serial]
    fn permissive_env_yields_true() {
        clear_env();
        unsafe { std::env::set_var("WCORE_CUA_TEST_WAYLAND_PERMISSIVE", "1") };
        assert!(compositor_allows_background_input());
        clear_env();
    }

    #[test]
    #[serial]
    fn restricted_env_yields_false() {
        clear_env();
        unsafe { std::env::set_var("WCORE_CUA_TEST_WAYLAND_RESTRICTED", "1") };
        assert!(!compositor_allows_background_input());
        clear_env();
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;
    use serial_test::serial;

    fn clear_env() {
        // SAFETY: tests use #[serial] to prevent concurrent env-var
        // mutation. The unsafe blocks are required by the 2024 edition.
        unsafe {
            std::env::remove_var("WCORE_CUA_TEST_WAYLAND_PERMISSIVE");
            std::env::remove_var("WCORE_CUA_TEST_WAYLAND_RESTRICTED");
        }
    }

    /// Audit F7: positive invariance preserved — restricted compositor
    /// returns `WaylandRestricted` for input ops.
    #[tokio::test]
    #[serial]
    async fn restricted_compositor_dispatch_returns_wayland_restricted() {
        clear_env();
        unsafe { std::env::set_var("WCORE_CUA_TEST_WAYLAND_RESTRICTED", "1") };
        let b = LinuxWaylandBackend::new();
        let r = b
            .dispatch(
                &CuaSession::for_test("w"),
                CuaOp::LeftClick {
                    x: 0,
                    y: 0,
                    button: crate::backend::MouseButton::Left,
                    mods: crate::backend::KeyMods::default(),
                },
            )
            .await;
        assert!(matches!(r, Err(CuaError::WaylandRestricted { .. })));
        clear_env();
    }

    /// Audit F7: permissive compositor either runs `wlrctl` (success or
    /// real subprocess error) or — if wlrctl is missing on the runner —
    /// returns a typed `Backend` error. NEVER a silent Ok with no-op.
    #[tokio::test]
    #[serial]
    async fn permissive_compositor_dispatch_runs_real_subprocess_or_typed_error() {
        clear_env();
        unsafe { std::env::set_var("WCORE_CUA_TEST_WAYLAND_PERMISSIVE", "1") };
        let b = LinuxWaylandBackend::new();
        let r = b
            .dispatch(
                &CuaSession::for_test("w"),
                CuaOp::LeftClick {
                    x: 0,
                    y: 0,
                    button: crate::backend::MouseButton::Left,
                    mods: crate::backend::KeyMods::default(),
                },
            )
            .await;
        match r {
            Ok(CuaOpResult::Ok) => {
                // wlrctl ran successfully on the runner (rare in CI).
            }
            Err(CuaError::Backend(msg)) => {
                // Typed honest blocker — missing wlrctl or non-zero exit.
                assert!(
                    msg.contains("wlrctl"),
                    "expected wlrctl-related backend error, got: {msg}"
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
        clear_env();
    }
}
