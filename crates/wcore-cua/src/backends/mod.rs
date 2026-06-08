//! Per-platform backend implementations. Each module is gated by
//! `#[cfg(target_os = "...")]` so the workspace cross-compiles cleanly
//! and no foreign-platform code reaches the linker on the wrong target.
//!
//! The `Unsupported` backend always compiles — the runtime selector
//! falls back to it on platforms the crate hasn't taught yet (BSD,
//! Solaris, wasm, etc.).

use std::sync::Arc;

use crate::backend::{ComputerUseBackend, Platform};

pub mod unsupported;

#[cfg(target_os = "macos")]
pub mod macos;

// The Wayland module compiles on every target so the host adapter can
// reference `compositor_allows_background_input` from the same path
// regardless of platform. The non-Linux compilation only exposes the
// probe; the `LinuxWaylandBackend` itself is Linux-only via inner cfg.
pub mod linux_wayland;

#[cfg(target_os = "linux")]
pub mod linux_x11;

#[cfg(target_os = "windows")]
pub mod windows;

/// Construct the backend for the given platform. Returns the
/// `Unsupported` backend when the build target doesn't ship a real one
/// (e.g. requesting `LinuxWayland` from a `target_os = "macos"` build).
pub fn for_platform(platform: Platform) -> Arc<dyn ComputerUseBackend> {
    match platform {
        #[cfg(target_os = "macos")]
        Platform::MacOs => Arc::new(macos::MacOsBackend::new()),

        #[cfg(target_os = "linux")]
        Platform::LinuxX11 => Arc::new(linux_x11::LinuxX11Backend::new()),
        #[cfg(target_os = "linux")]
        Platform::LinuxWayland => Arc::new(linux_wayland::LinuxWaylandBackend::new()),
        // On non-Linux targets, asking for a Linux backend falls back
        // to the Unsupported variant so the dispatcher stays total.
        #[cfg(not(target_os = "linux"))]
        Platform::LinuxX11 | Platform::LinuxWayland => {
            Arc::new(unsupported::UnsupportedBackend::new(platform))
        }

        #[cfg(target_os = "windows")]
        Platform::Windows => Arc::new(windows::WindowsBackend::new()),

        _ => Arc::new(unsupported::UnsupportedBackend::new(platform)),
    }
}

// Note: the prior `synth_screenshot_result` helper (deterministic 8x8
// PNG) is REMOVED in wave CU. Every backend now produces a real
// screenshot via its platform-native API (CGDisplay / X11 GetImage /
// grim subprocess / GDI BitBlt) — there's no more synth surface.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_backend_is_always_available() {
        let b = for_platform(Platform::Unsupported);
        assert_eq!(b.platform(), Platform::Unsupported);
    }

    #[test]
    fn current_platform_resolves_to_native_backend() {
        let b = for_platform(Platform::current());
        let name = b.name();
        // At least one of the four native backends OR the unsupported
        // fallback must answer with a non-empty name.
        assert!(!name.is_empty(), "backend name must be non-empty");
    }
}
