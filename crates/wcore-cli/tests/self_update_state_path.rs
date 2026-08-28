//! `FreezeState::default_path()` resolves through `WAYLAND_HOME`.
//!
//! ITS OWN TEST BINARY, and that is the whole design. Proving the resolver
//! honours the variable means WRITING the variable, and that is a process
//! global: `cargo test` runs every test of a binary on threads of ONE process,
//! so this assertion inside `self_update_trust.rs` pointed the twenty-one other
//! tests in that binary — and any production path they reach — at a `TempDir`
//! it was about to delete. Alone in a binary there is no sibling to
//! contaminate. `scripts/check-test-env-globals.py` is what keeps it that way:
//! adding a second test to this file turns it back into a reported hazard.

use tempfile::TempDir;

use wcore_cli::self_update::update_trust::FreezeState;

#[test]
fn the_persisted_state_path_honours_wayland_home() {
    let temp = TempDir::new().unwrap();
    let previous = std::env::var("WAYLAND_HOME").ok();
    // SAFETY: the only test in this binary, so nothing else is running in this
    // process to observe the write.
    unsafe { std::env::set_var("WAYLAND_HOME", temp.path()) };

    let resolved = FreezeState::default_path();

    // SAFETY: as above. Restored anyway so the process is left as it was found.
    match previous {
        Some(value) => unsafe { std::env::set_var("WAYLAND_HOME", value) },
        None => unsafe { std::env::remove_var("WAYLAND_HOME") },
    }

    assert!(
        resolved.starts_with(temp.path()),
        "the freeze state must live under WAYLAND_HOME, got {}",
        resolved.display()
    );
}
