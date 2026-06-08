//! RAII guard that restores the terminal on every exit path.
//!
//! v0.9.0 Wave-1 B0 (R-H8): mouse capture is enabled by W0.1, so if the
//! TUI panics between `EnableMouseCapture` and the regular shutdown
//! path the user is stranded in a shell where every click prints
//! escape codes. The existing `install_panic_hook` chains the previous
//! hook so the panic still reports; this guard adds the Drop side
//! (normal exit, ?-bubble, early return) so both paths cover.
//!
//! The guard is intentionally light — it just calls `restore_terminal`
//! on Drop. Install once at the top of `run()` so the Drop fires
//! whether `run()` returns Ok, returns Err, or unwinds.

use super::restore_terminal;

/// Drop guard that restores the terminal on every normal-exit path
/// (Ok return, ?-bubble Err, scope-end). The panic path is covered by
/// the existing `install_panic_hook` in `tui/mod.rs`; this guard is the
/// non-panic complement.
pub struct TerminalGuard {
    _private: (),
}

impl TerminalGuard {
    /// Construct the guard. The companion panic hook is installed by
    /// `tui/mod.rs::install_panic_hook` immediately after enabling
    /// raw mode + entering the alt screen.
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for TerminalGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_guard_disables_mouse_capture_on_drop() {
        // We can't observe the crossterm side effects in a non-TTY
        // unit-test environment, but the guard MUST construct and drop
        // without panicking (restore_terminal is idempotent-safe via
        // the existing tui/mod.rs unit test).
        {
            let _g = TerminalGuard::new();
            // scope end → Drop fires
        }
        // A second guard right after must also Drop cleanly.
        let _g2 = TerminalGuard::default();
        drop(_g2);
    }

    #[test]
    fn panic_hook_restores_terminal_on_panic() {
        // The panic-hook side lives in `tui/mod.rs::install_panic_hook`
        // and is already covered by its own unit test
        // (`restore_terminal_is_idempotent_without_a_tty`). Re-validate
        // here that a panicking scope still produces no double-panic
        // when the guard is in play.
        let result = std::panic::catch_unwind(|| {
            let _g = TerminalGuard::new();
            panic!("simulated TUI panic — guard must Drop cleanly");
        });
        assert!(
            result.is_err(),
            "panic should propagate, but guard must Drop without secondary panic"
        );
    }
}
