//! Streaming verb pool + single-pick mechanics (SPEC §4, Wave 6).
//!
//! The streaming-status widget samples ONE verb per turn from
//! [`SPINNER_VERBS`] (≥150 Wayland-branded gerunds) via [`pick_turn_verb`],
//! keyed off the per-turn seed on `SessionView::turn_verb_seed`. This replaced
//! the old time-based rotation in `widgets/streaming_status.rs`: the verb is
//! constant for the whole turn (the `useState(|| sample())` equivalent) and
//! varies across turns. The RGB color-lerp stall signal (orange → error after
//! ~3s of no token deltas) lives alongside the widget in
//! `widgets/streaming_status.rs`.

mod verbs;

pub use verbs::{SPINNER_VERBS, pick_turn_verb};
