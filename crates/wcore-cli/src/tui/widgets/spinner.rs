//! Braille spinner — a tick-driven animation frame.
//!
//! Unlike the other widgets this is fully implemented in Wave 0: the
//! render loop needs a working spinner immediately, and the logic is
//! trivial (no theming, no layout).

/// The braille spinner cycle. Eight frames give a smooth ~4 rev/s spin
/// at the 30fps tick if advanced every few ticks.
const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠇"];

/// The spinner glyph for a given monotonically increasing `tick`.
///
/// FROZEN Wave-0 contract. Fully implemented (not a stub).
pub fn spinner_frame(tick: u64) -> &'static str {
    FRAMES[(tick as usize) % FRAMES.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_cycles_and_wraps() {
        assert_eq!(spinner_frame(0), FRAMES[0]);
        assert_eq!(spinner_frame(7), FRAMES[7]);
        // Wraps cleanly past the frame count.
        assert_eq!(spinner_frame(8), FRAMES[0]);
        assert_eq!(spinner_frame(8 * 1000 + 3), FRAMES[3]);
    }
}
