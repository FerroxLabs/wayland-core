//! Single shared animation clock (v0.9.2 W1).
//!
//! Lets the one render loop (`mod.rs:run_loop`, `TICK = 33ms`) stop ticking
//! when nothing needs animating and pause on terminal blur / resize-to-zero.
//! The real `subscribe` / `unsubscribe` / `set_paused` / `wants_tick` /
//! `advance` logic lives in `clock.rs`; this module just re-exports the
//! stable surface that `app.rs` (the `anim` field) and the protocol bridge
//! build against. `wants_tick()` / `advance()` signatures are FROZEN.
//!
//! SPEC §1A: `wants_tick()` = `!paused && !subscribers.is_empty()`.

mod clock;

pub use clock::{AnimId, AnimationClock, Subscription};
